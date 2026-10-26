//! The visible subgraph: only the commits between the interesting tips and their
//! merge base(s), plus a little context. Long linear runs collapse into a single
//! `… n commits` node, so the graph stays small whatever the size of the repo.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use smallvec::SmallVec;

use crate::git::{CommitInfo, Oid, Repo};

pub type NodeId = usize;

#[derive(Clone, Debug)]
pub struct Node {
    /// Phantom nodes (predicted merges, conflicts) carry a synthetic id.
    pub oid: Oid,
    pub parents: SmallVec<[NodeId; 2]>,
    pub summary: String,
    pub author: String,
    pub time: i64,
    /// Non-zero when this node stands for that many hidden commits.
    pub collapsed: u32,
    /// History continues below this node but was not loaded.
    pub truncated: bool,
    /// Not a real commit: a preview or a merge that stopped on conflicts.
    pub phantom: bool,
    /// Visible in the before / after state.
    pub before: bool,
    pub after: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    index: HashMap<Oid, NodeId>,
}

pub struct WalkLimits {
    /// Commits to show beyond the point where all tips have converged.
    pub context: usize,
    /// Hard cap on commits loaded.
    pub max: usize,
}

impl Graph {
    /// Paint-down walk from `tips` in commit-date order. Every commit carries a bitmask
    /// of the tips that reach it; once a commit is reachable from all tips we are past
    /// the merge base and only take `context` more.
    pub fn load(repo: &Repo, tips: &[Oid], limits: &WalkLimits) -> Graph {
        let mut uniq: Vec<Oid> = Vec::new();
        for t in tips {
            if !uniq.contains(t) && uniq.len() < 64 {
                uniq.push(*t);
            }
        }
        let tipset: HashSet<Oid> = uniq.iter().copied().collect();
        let all: u64 = if uniq.len() >= 64 { u64::MAX } else { (1u64 << uniq.len()) - 1 };
        let context = if uniq.len() <= 1 { limits.context.max(8) } else { limits.context };

        let mut info: HashMap<Oid, CommitInfo> = HashMap::new();
        let mut flags: HashMap<Oid, u64> = HashMap::new();
        let mut heap: BinaryHeap<(i64, Reverse<u64>, Oid)> = BinaryHeap::new();
        let mut seq = 0u64;
        for (i, &t) in uniq.iter().enumerate() {
            if let Some(ci) = repo.commit(t) {
                *flags.entry(t).or_default() |= 1 << i;
                heap.push((ci.time, Reverse(seq), t));
                seq += 1;
                info.insert(t, ci);
            }
        }

        let mut order: Vec<Oid> = Vec::new();
        let mut common = 0usize;
        while let Some((_, _, oid)) = heap.pop() {
            let f = flags[&oid];
            if f == all && !tipset.contains(&oid) {
                if common >= context {
                    continue;
                }
                common += 1;
            }
            order.push(oid);
            if order.len() >= limits.max {
                break;
            }
            let parents = info[&oid].parents.clone();
            for p in parents {
                let known = flags.contains_key(&p);
                *flags.entry(p).or_default() |= f;
                if !known
                    && let Some(ci) = repo.commit(p) {
                        heap.push((ci.time, Reverse(seq), p));
                        seq += 1;
                        info.insert(p, ci);
                    }
            }
        }

        let index: HashMap<Oid, NodeId> = order.iter().enumerate().map(|(i, o)| (*o, i)).collect();
        let nodes = order
            .iter()
            .map(|oid| {
                let ci = &info[oid];
                let parents: SmallVec<[NodeId; 2]> = ci.parents.iter().filter_map(|p| index.get(p).copied()).collect();
                Node {
                    oid: *oid,
                    truncated: parents.len() < ci.parents.len(),
                    parents,
                    summary: ci.summary.clone(),
                    author: ci.author.clone(),
                    time: ci.time,
                    collapsed: 0,
                    phantom: false,
                    before: true,
                    after: true,
                }
            })
            .collect();
        Graph { nodes, index }
    }

    /// A graph from parent lists alone (property tests). Node `i` is newer than
    /// node `i + 1`.
    #[cfg(test)]
    pub fn synthetic(parents: &[Vec<NodeId>]) -> Graph {
        let n = parents.len();
        let mut g = Graph::default();
        for (i, ps) in parents.iter().enumerate() {
            let mut bytes = [0u8; 20];
            bytes[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
            let oid = Oid::from_bytes_or_panic(&bytes);
            g.index.insert(oid, i);
            g.nodes.push(Node {
                oid,
                parents: ps.iter().copied().collect(),
                summary: format!("c{i}"),
                author: String::new(),
                time: (n - i) as i64,
                collapsed: 0,
                truncated: false,
                phantom: false,
                before: true,
                after: true,
            });
        }
        g
    }

    pub fn find(&self, oid: &Oid) -> Option<NodeId> {
        self.index.get(oid).copied()
    }

    /// Add a predicted commit (merge preview, conflicted merge) on top of `parents`.
    pub fn add_phantom(&mut self, parents: &[NodeId], summary: &str) -> NodeId {
        let id = self.nodes.len();
        let mut bytes = [0xee_u8; 20];
        bytes[19] = id as u8;
        let oid = Oid::from_bytes_or_panic(&bytes);
        let time = parents.iter().map(|p| self.nodes[*p].time).max().unwrap_or(0) + 1;
        self.nodes.push(Node {
            oid,
            parents: parents.iter().copied().collect(),
            summary: summary.to_string(),
            author: String::new(),
            time,
            collapsed: 0,
            truncated: false,
            phantom: true,
            before: false,
            after: true,
        });
        self.index.insert(oid, id);
        id
    }

    pub fn children(&self) -> Vec<SmallVec<[NodeId; 2]>> {
        let mut ch: Vec<SmallVec<[NodeId; 2]>> = vec![SmallVec::new(); self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            for &p in &n.parents {
                ch[p].push(i);
            }
        }
        ch
    }

    /// All (child, parent) edges.
    pub fn edges(&self) -> Vec<(NodeId, NodeId)> {
        let mut out = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            for &p in &n.parents {
                out.push((i, p));
            }
        }
        out
    }

    pub fn reachable(&self, tips: &[NodeId]) -> Vec<bool> {
        let mut seen = vec![false; self.nodes.len()];
        let mut stack: Vec<NodeId> = tips.to_vec();
        while let Some(n) = stack.pop() {
            if std::mem::replace(&mut seen[n], true) {
                continue;
            }
            stack.extend(self.nodes[n].parents.iter().copied());
        }
        seen
    }

    /// Mark which nodes each state can see. Phantom nodes keep their own visibility.
    pub fn mark_visibility(&mut self, before_tips: &[NodeId], after_tips: &[NodeId]) {
        let b = self.reachable(before_tips);
        let a = self.reachable(after_tips);
        for (i, n) in self.nodes.iter_mut().enumerate() {
            if !n.phantom {
                n.before = b[i];
                n.after = a[i];
            }
        }
    }

    /// Is `anc` an ancestor of (or equal to) `desc`, within the loaded graph?
    pub fn is_ancestor(&self, anc: NodeId, desc: NodeId) -> bool {
        self.reachable(&[desc])[anc]
    }

    /// Path of edges from `desc` down to its ancestor `anc`, following first parents
    /// where possible. Returns (child, parent) pairs, top to bottom.
    pub fn route(&self, desc: NodeId, anc: NodeId) -> Option<Vec<(NodeId, NodeId)>> {
        let mut prev: HashMap<NodeId, NodeId> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([desc]);
        let mut seen = HashSet::from([desc]);
        while let Some(n) = queue.pop_front() {
            if n == anc {
                let mut path = Vec::new();
                let mut cur = anc;
                while cur != desc {
                    let child = prev[&cur];
                    path.push((child, cur));
                    cur = child;
                }
                path.reverse();
                return Some(path);
            }
            for &p in &self.nodes[n].parents {
                if seen.insert(p) {
                    prev.insert(p, n);
                    queue.push_back(p);
                }
            }
        }
        None
    }

    /// Collapse linear runs of uninteresting commits. `keep[n]` pins a node.
    /// Unchanged runs of 3+ become one node; runs that change (new or dropped
    /// commits) keep their two ends so the animation still has something to show.
    #[allow(clippy::needless_range_loop)]
    pub fn collapse(&mut self, keep: &[bool]) {
        let n = self.nodes.len();
        let children = self.children();
        let collapsible = |i: NodeId| {
            let node = &self.nodes[i];
            !keep.get(i).copied().unwrap_or(false)
                && !node.phantom
                && !node.truncated
                && node.parents.len() == 1
                && children[i].len() == 1
        };
        let same = |a: NodeId, b: NodeId| self.nodes[a].before == self.nodes[b].before && self.nodes[a].after == self.nodes[b].after;

        let mut group: Vec<Option<usize>> = vec![None; n];
        let mut groups: Vec<Vec<NodeId>> = Vec::new();
        for i in 0..n {
            if !collapsible(i) {
                continue;
            }
            let child = children[i][0];
            if collapsible(child) && same(child, i) {
                continue; // not the head of its run
            }
            let mut run = vec![i];
            let mut cur = i;
            loop {
                let p = self.nodes[cur].parents[0];
                if collapsible(p) && same(p, cur) && !run.contains(&p) {
                    run.push(p);
                    cur = p;
                } else {
                    break;
                }
            }
            let changed = self.nodes[i].before != self.nodes[i].after;
            let span = if changed {
                if run.len() < 8 { continue } else { &run[2..run.len() - 2] }
            } else if run.len() < 3 {
                continue;
            } else {
                &run[..]
            };
            for &m in span {
                group[m] = Some(groups.len());
            }
            groups.push(span.to_vec());
        }
        if groups.is_empty() {
            return;
        }

        let mut map = vec![0usize; n];
        let mut group_node: Vec<Option<NodeId>> = vec![None; groups.len()];
        let mut nodes: Vec<Node> = Vec::with_capacity(n);
        for i in 0..n {
            match group[i] {
                Some(g) => {
                    if let Some(id) = group_node[g] {
                        map[i] = id;
                        continue;
                    }
                    let members = &groups[g];
                    let last = *members.last().unwrap();
                    let mut node = self.nodes[i].clone();
                    node.collapsed = members.len() as u32;
                    node.summary = String::new();
                    node.parents = self.nodes[last].parents.clone();
                    node.truncated = self.nodes[last].truncated;
                    group_node[g] = Some(nodes.len());
                    map[i] = nodes.len();
                    nodes.push(node);
                }
                None => {
                    map[i] = nodes.len();
                    nodes.push(self.nodes[i].clone());
                }
            }
        }
        for node in &mut nodes {
            let mut ps: SmallVec<[NodeId; 2]> = SmallVec::new();
            for p in &node.parents {
                let m = map[*p];
                if !ps.contains(&m) {
                    ps.push(m);
                }
            }
            node.parents = ps;
        }
        for v in self.index.values_mut() {
            *v = map[*v];
        }
        self.nodes = nodes;
    }
}
