//! Snapshots in, one shared graph plus the list of changes out.

pub mod delta;
pub mod graph;

use std::collections::HashSet;

pub use delta::Change;
pub use graph::{Graph, NodeId};

use crate::git::{HeadState, Oid, RefInfo, RefKind, Repo, Snapshot, reader};

/// A commit that does not exist (yet): a merge preview or a merge stopped by conflicts.
#[derive(Clone, Debug)]
pub struct Phantom {
    pub parents: Vec<Oid>,
    pub summary: String,
    /// Move the current branch (and HEAD) onto the phantom in the after state.
    pub retarget_head: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// Names mentioned on the command line (branches, remotes, tags).
    pub named: Vec<String>,
    /// Show every local branch when there are at most this many (0: only the
    /// branches involved in the operation).
    pub locals: usize,
    pub context: usize,
    pub phantom: Option<Phantom>,
}

pub struct Transition {
    pub graph: Graph,
    pub before: Snapshot,
    pub after: Snapshot,
    pub changes: Vec<Change>,
    pub phantom: Option<NodeId>,
}

impl Transition {
    pub fn build(repo: &Repo, before: Snapshot, mut after: Snapshot, hints: &Hints) -> Transition {
        let tips = collect_tips(repo, &before, &after, hints);
        let limits = graph::WalkLimits { context: hints.context.max(1), max: 2000 };
        let mut graph = Graph::load(repo, &tips, &limits);

        let mut phantom = None;
        if let Some(p) = &hints.phantom {
            let parents: Vec<NodeId> = p.parents.iter().filter_map(|o| graph.find(o)).collect();
            if !parents.is_empty() {
                let id = graph.add_phantom(&parents, &p.summary);
                if p.retarget_head {
                    retarget_head(&mut after, graph.nodes[id].oid);
                }
                phantom = Some(id);
            }
        }

        let before_tips = state_tips(&graph, &before);
        let mut after_tips = state_tips(&graph, &after);
        after_tips.extend(phantom);
        graph.mark_visibility(&before_tips, &after_tips);

        // Pin everything an animation needs to point at before collapsing.
        let mut keep = vec![false; graph.nodes.len()];
        for n in before_tips.iter().chain(after_tips.iter()) {
            keep[*n] = true;
        }
        let new_summaries: HashSet<&str> =
            graph.nodes.iter().filter(|n| n.after && !n.before).map(|n| n.summary.as_str()).collect();
        for (i, n) in graph.nodes.iter().enumerate() {
            if n.before && n.after && new_summaries.contains(n.summary.as_str()) {
                keep[i] = true;
            }
        }
        for oid in after.pick_head.iter().chain(after.merge_heads.iter()) {
            if let Some(i) = graph.find(oid) {
                keep[i] = true;
            }
        }
        let phantom_oid = phantom.map(|p| graph.nodes[p].oid);
        graph.collapse(&keep);
        let phantom = phantom_oid.and_then(|o| graph.find(&o));

        let changes = delta::diff(&graph, &before, &after);
        Transition { graph, before, after, changes, phantom }
    }

    /// A transition with no change: used for `git graft log` and the idle watch view.
    pub fn still(repo: &Repo, snap: Snapshot, hints: &Hints) -> Transition {
        Transition::build(repo, snap.clone(), snap, hints)
    }

    pub fn is_noop(&self) -> bool {
        self.changes.is_empty()
    }
}

fn retarget_head(snap: &mut Snapshot, oid: Oid) {
    match &mut snap.head {
        HeadState::Attached { branch, oid: head } => {
            *head = Some(oid);
            let branch = branch.clone();
            if let Some(r) = snap.refs.iter_mut().find(|r| r.name == branch) {
                r.target = oid;
            }
        }
        head => *head = HeadState::Detached(oid),
    }
}

/// Nodes a state can "see": its refs and HEAD.
fn state_tips(graph: &Graph, snap: &Snapshot) -> Vec<NodeId> {
    let mut tips: Vec<NodeId> = snap.refs.iter().filter_map(|r| graph.find(&r.target)).collect();
    tips.extend(snap.head_oid().and_then(|o| graph.find(&o)));
    tips
}

fn collect_tips(repo: &Repo, before: &Snapshot, after: &Snapshot, hints: &Hints) -> Vec<Oid> {
    let mut tips: Vec<Oid> = Vec::new();
    tips.extend(after.head_oid());
    tips.extend(before.head_oid());
    if let Some(p) = &hints.phantom {
        tips.extend(p.parents.iter().copied());
    }
    tips.extend(after.merge_heads.iter().copied());
    tips.extend(after.pick_head);

    // Every ref that moved, at both ends.
    for r in before.refs.iter().chain(after.refs.iter()) {
        let b = before.find_ref(&r.name).map(|x| x.target);
        let a = after.find_ref(&r.name).map(|x| x.target);
        if a != b {
            tips.extend(a);
            tips.extend(b);
        }
    }

    for name in &hints.named {
        for snap in [after, before] {
            if let Some(r) = reader::resolve_name(&snap.refs, name) {
                tips.push(r.target);
            }
        }
    }

    for snap in [after, before] {
        if let Some(branch) = snap.head_branch() {
            let short = branch.trim_start_matches("refs/heads/");
            if let Some(up) = repo.upstream(short).and_then(|u| snap.find_ref(&u).map(|r| r.target)) {
                tips.push(up);
            }
        }
    }

    let locals: Vec<&RefInfo> = after.refs.iter().filter(|r| r.kind == RefKind::Local).collect();
    if locals.len() <= hints.locals {
        tips.extend(locals.iter().map(|r| r.target));
    }
    tips
}
