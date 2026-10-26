//! The wire to the remote: fetched commits slide in from it, pushed commits travel
//! out along it, and rejected pushes bounce back in red.

use std::collections::HashSet;

use super::{HOLD, LABEL, Stage};
use crate::anim::timeline::{Prop, Target};
use crate::anim::tween::*;
use crate::git::RefKind;
use crate::model::{Change, NodeId};
use crate::op::Op;

/// New commits that arrived through a fetch (reachable from a remote-tracking ref that moved).
pub(super) fn remote_sources(s: &Stage) -> HashSet<NodeId> {
    if !matches!(s.info.op, Op::Fetch | Op::Pull) {
        return HashSet::new();
    }
    let tips: Vec<NodeId> = s
        .tr
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::RefMoved { kind: RefKind::Remote, to: Some(n), .. } => Some(*n),
            _ => None,
        })
        .collect();
    let reach = s.tr.graph.reachable(&tips);
    s.tr.graph.nodes.iter().enumerate().filter(|(i, n)| reach[*i] && n.after && !n.before).map(|(i, _)| i).collect()
}

pub(super) fn push(s: &mut Stage) {
    s.caption_in(0.05);
    let mut t = s.expand(HOLD);
    let Some(panel) = s.panel_remote else {
        let t = s.move_labels(t);
        let t = s.settle(t);
        s.end(t);
        return;
    };
    let target = s.panel_center(panel);

    let moved: Vec<(Option<NodeId>, NodeId)> = s
        .tr
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::RefMoved { kind: RefKind::Remote, from, to: Some(to), .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();

    if moved.is_empty() && s.info.exit_code != 0 {
        rejected(s, t);
        return;
    }

    // Commits travel up the wire, oldest first.
    let mut arrive = t;
    for (from, to) in moved {
        let nodes = travelling(s, from, to);
        let n = nodes.len().max(1) as f32;
        let stagger = (0.6 / n).min(0.14);
        for (k, node) in nodes.into_iter().enumerate() {
            let st = t + k as f32 * stagger;
            let color = s.sc.nodes[node].color;
            s.tl.flash(Target::Node(node as u32), Prop::Glow, 1.0, 0.0, st, 0.05, 0.4);
            let a = s.fly(Stage::node(node), target, st, 0.5, color, 1.5);
            s.flash_panel(Some(panel), a - 0.04, false);
            arrive = arrive.max(a);
        }
    }
    t = s.move_labels(arrive - 0.15);
    let t = s.settle(t);
    s.end(t);
}

/// Commits a push sends: between the old remote tip (exclusive) and the new one.
fn travelling(s: &Stage, from: Option<NodeId>, to: NodeId) -> Vec<NodeId> {
    let mut nodes: Vec<NodeId> = match from.and_then(|f| s.tr.graph.route(to, f)) {
        Some(route) => route.into_iter().map(|(c, _)| c).collect(),
        None => vec![to],
    };
    nodes.reverse();
    nodes.truncate(8);
    nodes
}

/// Local commits fly toward the remote and bounce back red.
fn rejected(s: &mut Stage, t: f32) {
    let panel = s.panel_remote.unwrap();
    let target = s.panel_center(panel);
    let tr = s.tr;
    let head = tr.after.head_oid().and_then(|o| tr.graph.find(&o));
    let upstream = tr
        .after
        .head_branch()
        .map(|b| b.trim_start_matches("refs/heads/"))
        .and_then(|b| tr.after.refs.iter().find(|r| r.kind == RefKind::Remote && r.short.ends_with(&format!("/{b}"))))
        .and_then(|r| tr.graph.find(&r.target));
    let Some(head) = head else {
        s.end(t);
        return;
    };
    let nodes = travelling(s, upstream.filter(|u| tr.graph.is_ancestor(*u, head)), head);
    let mut back = t;
    for (k, node) in nodes.iter().enumerate() {
        let st = t + k as f32 * 0.1;
        let color = s.sc.nodes[*node].color;
        let a = s.fly(Stage::node(*node), target, st, 0.4, color, 1.5);
        let b = s.fly_red(target, Stage::node(*node), a, 0.4, color, -1.5);
        s.flash_panel(Some(panel), a - 0.03, true);
        let id = Target::Node(*node as u32);
        s.tl.flash(id, Prop::Red, 1.0, 0.0, b - 0.05, 0.05, 0.8);
        s.tl.tween(id, Prop::Shake, 1.0, 0.0, b - 0.05, 0.4, ease_out_cubic);
        back = back.max(b);
    }
    if let Some(&item) = s.labels.get("HEAD") {
        s.tl.flash(Target::Item(item as u32), Prop::Red, 1.0, 0.0, back - 0.05, 0.05, 0.8);
    }
    s.end(back + LABEL * 0.5);
}
