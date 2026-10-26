//! The same idea as `git log --graph`: walk commits in topological order and give
//! each one a lane. A commit inherits the lane of its first child; extra parents
//! claim the leftmost free lane.

use crate::model::{Graph, NodeId};

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub row: Vec<Option<u32>>,
    pub lane: Vec<Option<u32>>,
    /// Lane each edge travels down in, indexed like `Graph::edges()`.
    pub edge_lane: Vec<Option<u32>>,
    pub rows: u32,
    pub lanes: u32,
}

/// Children before parents, keeping each first-parent chain contiguous. Tips listed
/// in `priority` come first, so the most important branch ends up in lane 0.
pub fn topo_order(graph: &Graph, priority: &[NodeId]) -> Vec<NodeId> {
    let n = graph.nodes.len();
    let mut pending = vec![0u32; n];
    for node in &graph.nodes {
        for &p in &node.parents {
            pending[p] += 1;
        }
    }
    let rank = |i: NodeId| priority.iter().position(|&p| p == i).unwrap_or(usize::MAX);
    let mut roots: Vec<NodeId> = (0..n).filter(|&i| pending[i] == 0).collect();
    roots.sort_by_key(|&i| (rank(i), std::cmp::Reverse(graph.nodes[i].time), i));
    let mut stack: Vec<NodeId> = roots.into_iter().rev().collect();
    let mut out = Vec::with_capacity(n);
    while let Some(i) = stack.pop() {
        out.push(i);
        for &p in graph.nodes[i].parents.iter().rev() {
            pending[p] -= 1;
            if pending[p] == 0 {
                stack.push(p);
            }
        }
    }
    out
}

pub fn assign(graph: &Graph, order: &[NodeId], visible: &[bool]) -> Layout {
    let n = graph.nodes.len();
    let mut edge_start = Vec::with_capacity(n);
    let mut total = 0;
    for node in &graph.nodes {
        edge_start.push(total);
        total += node.parents.len();
    }
    let mut out = Layout { row: vec![None; n], lane: vec![None; n], edge_lane: vec![None; total], rows: 0, lanes: 0 };

    let mut active: Vec<Option<NodeId>> = Vec::new();
    let mut blocked: Vec<usize> = Vec::new();
    let mut row = 0u32;
    for &c in order.iter().filter(|&&c| visible[c]) {
        blocked.clear();
        let expecting: Vec<usize> = (0..active.len()).filter(|&l| active[l] == Some(c)).collect();
        let lane = match expecting.first() {
            Some(&l) => l,
            None => free_lane(&mut active, &blocked),
        };
        for &l in &expecting {
            active[l] = None;
            if l != lane {
                blocked.push(l); // the edge turns into `c` on this row; reuse from the next one
            }
        }
        out.row[c] = Some(row);
        out.lane[c] = Some(lane as u32);

        let mut first = true;
        for (k, &p) in graph.nodes[c].parents.iter().enumerate() {
            if !visible[p] {
                continue;
            }
            let e = edge_start[c] + k;
            if first {
                active[lane] = Some(p);
                out.edge_lane[e] = Some(lane as u32);
                first = false;
            } else if let Some(l) = active.iter().position(|a| *a == Some(p)) {
                out.edge_lane[e] = Some(l as u32);
            } else {
                blocked.push(lane);
                let l = free_lane(&mut active, &blocked);
                active[l] = Some(p);
                out.edge_lane[e] = Some(l as u32);
            }
        }
        out.lanes = out.lanes.max(active.len() as u32).max(lane as u32 + 1);
        row += 1;
    }
    out.rows = row;
    out.lanes = out.lanes.max(1);
    // Trailing lanes that never held anything do not count.
    let used = out.lane.iter().chain(out.edge_lane.iter()).flatten().max().map_or(1, |m| m + 1);
    out.lanes = out.lanes.min(used);
    out
}

fn free_lane(active: &mut Vec<Option<NodeId>>, blocked: &[usize]) -> usize {
    match (0..active.len()).find(|l| active[*l].is_none() && !blocked.contains(l)) {
        Some(l) => l,
        None => {
            active.push(None);
            active.len() - 1
        }
    }
}
