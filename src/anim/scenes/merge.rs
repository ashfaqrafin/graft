//! Scenes built around a phantom commit: a merge that stopped on conflicts, or a
//! `--dry-run` preview of what a merge would do.

use super::{EDGE, HOLD, POP, Stage, panels};
use crate::anim::timeline::{Prop, Target};
use crate::anim::tween::*;

/// Edges reach for the merge but stop short, the node pulses red and the
/// conflicted files appear in the side panel.
pub(super) fn conflict(s: &mut Stage) {
    let ph = s.tr.phantom.unwrap();
    s.caption_in(0.05);
    let mut t = s.expand(HOLD);

    // A rebase or cherry-pick that stopped: the commit being applied flies toward its new home.
    if let Some(src) = s.reparent_source(ph) {
        let color = s.sc.nodes[src].color;
        t = s.fly(Stage::node(src), Stage::node(ph), t, 0.45, color, 1.5) - 0.15;
    }

    let id = Target::Node(ph as u32);
    let reach = s.grow_edges(ph, t, 0.45, 0.8);
    for (e, edge) in s.sc.edges.iter().enumerate() {
        if edge.child == ph {
            s.tl.tween(Target::Edge(e as u32), Prop::Red, 0.0, 1.0, t + 0.15, 0.35, ease_in_out);
        }
    }
    let pop = reach - 0.1;
    s.tl.set(id, Prop::Red, 1.0, pop);
    s.pop(ph, pop);
    s.tl.tween(id, Prop::Shake, 1.0, 0.0, pop + POP, 0.4, ease_out_cubic);
    let mut pulse = pop + POP;
    for _ in 0..2 {
        pulse = s.tl.flash(id, Prop::Glow, 0.9, 0.0, pulse, 0.1, 0.24);
    }

    // Conflicted files fly from the stalled merge into the working tree panel.
    let mut end = pulse;
    let conflicted: Vec<usize> = s.wt.iter().filter(|e| e.conflict && e.after && !e.before).map(|e| e.item).collect();
    for (k, item) in conflicted.iter().enumerate() {
        let st = pop + POP + k as f32 * 0.08;
        let to = s.sc.items[*item].b;
        let a = s.fly_red(Stage::node(ph), to, st, 0.4, s.theme.err, 1.0);
        s.fade_in_item(*item, a - 0.05);
        end = end.max(a);
    }
    s.flash_panel(s.panel_wt, pop + POP, true);
    end = end.max(panels::transition(s, pop + POP));
    end = end.max(s.move_labels(pop));
    let t = s.settle(end);
    s.end(t);
}

/// A dry run: the predicted merge commit appears dashed and the branch label
/// hops onto it. Predicted conflicts stop short in red like the real thing.
pub(super) fn preview(s: &mut Stage) {
    let ph = s.tr.phantom.unwrap();
    let conflicts = s.tr.graph.nodes[ph].summary.contains("conflict");
    s.caption_in(0.05);
    let t = s.expand(HOLD);
    let id = Target::Node(ph as u32);
    // Commits the prediction replaces (an amend) turn to ghosts.
    let vanished = s.vanished();
    for &v in &vanished {
        s.fade_node(v, 1.0, 0.35, t, 0.25);
    }
    if conflicts {
        if let Some(src) = s.reparent_source(ph) {
            let color = s.sc.nodes[src].color;
            s.fly(Stage::node(src), Stage::node(ph), t, 0.45, color, 1.5);
        }
        let reach = s.grow_edges(ph, t, 0.55, 0.8);
        for (e, edge) in s.sc.edges.iter().enumerate() {
            if edge.child == ph {
                s.tl.tween(Target::Edge(e as u32), Prop::Red, 0.0, 1.0, t + 0.2, 0.4, ease_in_out);
            }
        }
        s.tl.set(id, Prop::Red, 1.0, reach - 0.15);
        let p = s.pop(ph, reach - 0.15);
        s.tl.tween(id, Prop::Shake, 1.0, 0.0, p, 0.4, ease_out_cubic);
        let t = s.settle(p + 0.4);
        s.end(t);
        return;
    }
    // A predicted commit gathers the staged files first.
    let arrive = if s.info.op == crate::op::Op::Commit && s.panel_wt.is_some() {
        panels::staged_into(s, ph, t)
    } else {
        t + EDGE * 1.6
    };
    s.grow_edges(ph, arrive - EDGE * 1.6, EDGE * 1.6, 1.0);
    let p = s.pop(ph, arrive - 0.05);
    let mut end = s.move_labels(p - 0.1);
    end = end.max(panels::transition(s, arrive - 0.15));
    for &v in &vanished {
        end = end.max(s.fade_node(v, 0.35, 0.0, p, 0.3));
    }
    let t = s.settle(end);
    s.end(t);
}
