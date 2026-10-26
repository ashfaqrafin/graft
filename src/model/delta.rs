//! diff(before, after) -> Vec<Change>: the single source of truth every scene
//! animates from.

use std::collections::HashSet;

use super::graph::{Graph, NodeId};
use crate::git::{HeadState, RefKind, Snapshot, StashEntry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadPos {
    /// Short branch name when attached.
    pub branch: Option<String>,
    pub node: Option<NodeId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A commit that only exists afterwards.
    Spawn(NodeId),
    /// A commit no longer reachable from anything we track.
    Vanish(NodeId),
    RefMoved { name: String, short: String, kind: RefKind, from: Option<NodeId>, to: Option<NodeId> },
    /// `new` is a rewritten copy of `old` (rebase, cherry-pick, amend, revert).
    Reparent { old: NodeId, new: NodeId },
    HeadMoved { from: HeadPos, to: HeadPos },
    WorktreeChanged,
    StashPushed(StashEntry),
    StashPopped(StashEntry),
    ConflictFiles(Vec<String>),
}

pub fn head_pos(graph: &Graph, snap: &Snapshot) -> HeadPos {
    let node = snap.head_oid().and_then(|o| graph.find(&o));
    let branch = match &snap.head {
        HeadState::Attached { branch, .. } => Some(branch.trim_start_matches("refs/heads/").to_string()),
        _ => None,
    };
    HeadPos { branch, node }
}

pub fn diff(graph: &Graph, before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut out = Vec::new();

    let mut vanished = Vec::new();
    for (i, n) in graph.nodes.iter().enumerate() {
        match (n.before, n.after) {
            (false, true) => out.push(Change::Spawn(i)),
            (true, false) => {
                vanished.push(i);
                out.push(Change::Vanish(i));
            }
            _ => {}
        }
    }

    // Pair rewritten commits with their originals by summary + author.
    let mut used: HashSet<NodeId> = HashSet::new();
    let spawned: Vec<NodeId> = out
        .iter()
        .filter_map(|c| if let Change::Spawn(i) = c { Some(*i) } else { None })
        .collect();
    for &s in &spawned {
        let new = &graph.nodes[s];
        if new.collapsed > 0 {
            continue;
        }
        let source = if new.phantom {
            after.pick_head.and_then(|o| graph.find(&o)).filter(|o| *o != s)
        } else {
            None
        };
        let reverted = new.summary.strip_prefix("Revert \"").and_then(|r| r.strip_suffix('"'));
        let source = source.or_else(|| {
            let matches = |i: NodeId| {
                let old = &graph.nodes[i];
                i != s && old.collapsed == 0 && !old.phantom && !used.contains(&i) && old.before && {
                    old.summary == new.summary || reverted == Some(old.summary.as_str())
                }
            };
            vanished
                .iter()
                .copied()
                .find(|&i| matches(i) && graph.nodes[i].author == new.author)
                .or_else(|| (0..graph.nodes.len()).find(|&i| matches(i)))
        });
        if let Some(old) = source {
            used.insert(old);
            out.push(Change::Reparent { old, new: s });
        }
    }

    // Ref moves.
    let mut names: Vec<(&str, &str, RefKind)> = Vec::new();
    for r in before.refs.iter().chain(after.refs.iter()) {
        if !names.iter().any(|(n, _, _)| *n == r.name) {
            names.push((&r.name, &r.short, r.kind));
        }
    }
    for (name, short, kind) in names {
        let b = before.find_ref(name);
        let a = after.find_ref(name);
        if b.map(|r| r.target) == a.map(|r| r.target) {
            continue;
        }
        let from = b.and_then(|r| graph.find(&r.target));
        let to = a.and_then(|r| graph.find(&r.target));
        if from.is_none() && to.is_none() {
            continue;
        }
        out.push(Change::RefMoved { name: name.to_string(), short: short.to_string(), kind, from, to });
    }

    let hb = head_pos(graph, before);
    let ha = head_pos(graph, after);
    if hb != ha {
        out.push(Change::HeadMoved { from: hb, to: ha });
    }

    if let (Some(b), Some(a)) = (&before.worktree, &after.worktree) {
        if b != a {
            out.push(Change::WorktreeChanged);
        }
        if !a.conflicted.is_empty() && a.conflicted != b.conflicted {
            out.push(Change::ConflictFiles(a.conflicted.iter().map(|f| f.path.clone()).collect()));
        }
    }

    for e in &after.stash {
        if !before.stash.iter().any(|b| b.oid == e.oid) {
            out.push(Change::StashPushed(e.clone()));
        }
    }
    for e in &before.stash {
        if !after.stash.iter().any(|a| a.oid == e.oid) {
            out.push(Change::StashPopped(e.clone()));
        }
    }
    out
}
