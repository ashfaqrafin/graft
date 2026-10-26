//! Working tree and stash panels: files flying into commits, into the stash and
//! back out again.

use super::{PanelEntry, Stage, panel_item};
use crate::anim::scene::{Anchor, ItemStyle, SItem};
use crate::anim::timeline::ItemSt;
use crate::git::{FileKind, StashEntry, WorktreeStatus};
use crate::model::{Change, NodeId};
use crate::op::Op;
use crate::render::color::{Rgb, Theme};

const MAX_FILES: usize = 7;
const MAX_STASH: usize = 4;

pub(super) struct Entry {
    key: String,
    text: String,
    color: Rgb,
    path: String,
    section: char,
}

pub(super) fn worktree_entries(ws: &WorktreeStatus, theme: &Theme) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut push = |section: char, sym: &str, path: &str, color: Rgb| {
        out.push(Entry {
            key: format!("{section}{path}"),
            text: format!("{sym} {path}"),
            color,
            path: path.to_string(),
            section,
        })
    };
    let sym = |k: FileKind| match k {
        FileKind::Added => "+",
        FileKind::Deleted => "-",
        FileKind::Renamed => ">",
        FileKind::Untracked => "?",
        FileKind::Conflicted => "!",
        FileKind::Modified => "~",
    };
    for f in &ws.conflicted {
        push('!', "!", &f.path, theme.err);
    }
    for f in &ws.staged {
        push('S', sym(f.kind), &f.path, theme.ok);
    }
    for f in &ws.unstaged {
        push('U', sym(f.kind), &f.path, theme.warn);
    }
    for f in &ws.untracked {
        push('?', "?", &f.path, theme.muted);
    }
    let total = out.len();
    if total > MAX_FILES {
        out.truncate(MAX_FILES - 1);
        let more = total - out.len();
        out.push(Entry {
            key: format!("+{more}"),
            text: format!("  +{more} more"),
            color: theme.muted,
            path: String::new(),
            section: '+',
        });
    }
    if out.is_empty() {
        let tick = if theme.unicode { "✔" } else { "ok" };
        out.push(Entry { key: "clean".into(), text: format!("{tick} clean"), color: theme.ok, path: String::new(), section: 'c' });
    }
    out
}

pub(super) fn stash_entries(stash: &[StashEntry], theme: &Theme) -> Vec<Entry> {
    let icon = if theme.unicode { "≡" } else { "=" };
    let mut out: Vec<Entry> = stash
        .iter()
        .take(MAX_STASH)
        .map(|e| {
            let msg = e.message.strip_prefix("WIP on ").or_else(|| e.message.strip_prefix("On ")).unwrap_or(&e.message);
            Entry {
                key: e.oid.to_string(),
                text: format!("{icon} {msg}"),
                color: theme.lanes[2],
                path: String::new(),
                section: 's',
            }
        })
        .collect();
    if stash.len() > MAX_STASH {
        let more = stash.len() - MAX_STASH;
        out.push(Entry { key: format!("+{more}"), text: format!("  +{more} more"), color: theme.muted, path: String::new(), section: '+' });
    }
    if out.is_empty() {
        out.push(Entry { key: "empty".into(), text: "(empty)".into(), color: theme.muted, path: String::new(), section: 'c' });
    }
    out
}

/// Create items for the union of before and after entries; returns the panel height.
pub(super) fn place(
    b: &[Entry],
    a: &[Entry],
    px: u16,
    py: u16,
    pw: u16,
    items: &mut Vec<(SItem, ItemSt)>,
) -> (u16, Vec<PanelEntry>) {
    let mut out = Vec::new();
    let mut keys: Vec<&Entry> = b.iter().collect();
    keys.extend(a.iter().filter(|e| !b.iter().any(|x| x.key == e.key)));
    for e in keys {
        let ib = b.iter().position(|x| x.key == e.key);
        let ia = a.iter().position(|x| x.key == e.key);
        let y = |i: usize| py as f32 + 1.0 + i as f32;
        let at_b = Anchor::Fixed(px as f32 + 2.0, y(ib.or(ia).unwrap()));
        let at_a = Anchor::Fixed(px as f32 + 2.0, y(ia.or(ib).unwrap()));
        out.push(PanelEntry {
            item: items.len(),
            path: e.path.clone(),
            before: ib.is_some(),
            after: ia.is_some(),
            conflict: e.section == '!',
        });
        let mut item = panel_item(e.text.clone(), e.color, at_b, at_a, px + pw - 1);
        if e.section == '!' {
            item.style = ItemStyle::Text { color: e.color, bold: true };
        }
        items.push((item, ItemSt { alpha: if ib.is_some() { 1.0 } else { 0.0 }, ..Default::default() }));
    }
    let rows = b.len().max(a.len()).max(1) as u16;
    (rows + 2, out)
}

fn item_color(s: &Stage, item: usize) -> Rgb {
    match s.sc.items[item].style {
        ItemStyle::Text { color, .. } | ItemStyle::Pill { color } => color,
    }
}

fn anchor_of(s: &Stage, item: usize, after: bool) -> Anchor {
    let it = &s.sc.items[item];
    if after { it.b } else { it.a }
}

/// Staged files fly from the working tree panel into the new commit. Returns arrival time.
pub(super) fn staged_into(s: &mut Stage, node: NodeId, start: f32) -> f32 {
    let leaving: Vec<usize> = s
        .wt
        .iter()
        .filter(|e| e.before && !e.after && !e.path.is_empty())
        .map(|e| e.item)
        .take(5)
        .collect();
    let mut arrive = start;
    for (k, item) in leaving.into_iter().enumerate() {
        let t = start + k as f32 * 0.05;
        let color = item_color(s, item);
        let from = anchor_of(s, item, false);
        arrive = arrive.max(s.fly(from, Stage::node(node), t, super::FLY, color, 2.0));
        s.fade_out_item(item, t - 0.05, false);
    }
    arrive
}

/// Panel changes for everything except staged-file flights. Returns when done.
pub(super) fn transition(s: &mut Stage, t: f32) -> f32 {
    let mut end = t;
    if s.panel_wt.is_some() && s.tr.changes.contains(&Change::WorktreeChanged) || s.info.op == Op::Checkout {
        s.flash_panel(s.panel_wt, t, false);
    }

    let pushed = s.tr.changes.iter().any(|c| matches!(c, Change::StashPushed(_)));
    let popped = s.tr.changes.iter().any(|c| matches!(c, Change::StashPopped(_)));
    // stash push: dirty files fly into the new stash entry.
    if let (true, Some(ps)) = (pushed, s.panel_stash) {
        let target = s.stash.iter().find(|e| e.after && !e.before).map(|e| e.item);
        let leaving: Vec<usize> = s.wt.iter().filter(|e| e.before && !e.after).map(|e| e.item).take(6).collect();
        let to = target.map(|i| anchor_of(s, i, true)).unwrap_or_else(|| s.panel_center(ps));
        let mut arrive = t;
        for (k, item) in leaving.iter().enumerate() {
            let st = t + k as f32 * 0.07;
            let from = anchor_of(s, *item, false);
            let color = s.theme.lanes[2];
            arrive = arrive.max(s.fly(from, to, st, 0.5, color, 2.5));
            s.fade_out_item(*item, st - 0.05, false);
        }
        if let Some(i) = target {
            end = end.max(s.fade_in_item(i, arrive - 0.05));
        }
        s.flash_panel(Some(ps), arrive - 0.05, false);
        end = end.max(arrive);
    }

    // stash pop/apply: the entry flies back into the working tree.
    if let (true, Some(ps)) = (popped, s.panel_stash) {
        let source = s.stash.iter().find(|e| e.before && !e.after).map(|e| e.item);
        if let Some(src) = source {
            let from = anchor_of(s, src, false);
            let arriving: Vec<usize> = s.wt.iter().filter(|e| e.after && !e.before).map(|e| e.item).take(6).collect();
            let mut arrive = t;
            s.fade_out_item(src, t, s.info.stash_action() == "drop");
            if s.info.stash_action() != "drop" {
                for (k, item) in arriving.iter().enumerate() {
                    let to = anchor_of(s, *item, true);
                    let color = s.theme.lanes[2];
                    arrive = arrive.max(s.fly(from, to, t + k as f32 * 0.07, 0.5, color, 2.5));
                }
                for item in arriving {
                    s.fade_in_item(item, arrive - 0.1);
                }
            }
            s.flash_panel(Some(ps), t, false);
            end = end.max(arrive);
        }
    }

    // reset --soft / --mixed: orphaned commits pour their changes back into the tree.
    if s.info.op == Op::Reset && s.info.reset_mode() != Some("hard") {
        let doomed: Vec<NodeId> = s.vanished();
        if !doomed.is_empty() {
            let arriving: Vec<usize> = s.wt.iter().filter(|e| e.after && !e.before).map(|e| e.item).take(6).collect();
            for (k, item) in arriving.into_iter().enumerate() {
                let src = doomed[k % doomed.len()];
                let to = anchor_of(s, item, true);
                let color = s.sc.nodes[src].color;
                let a = s.fly(Stage::node(src), to, t + k as f32 * 0.07, 0.5, color, 2.0);
                end = end.max(s.fade_in_item(item, a - 0.08));
            }
        }
    }

    // Everything else crossfades; a file that changed section (staged <-> unstaged)
    // hops across.
    let entries: Vec<(usize, String, bool, bool)> =
        s.wt.iter().chain(s.stash.iter()).map(|e| (e.item, e.path.clone(), e.before, e.after)).collect();
    for (item, path, before, after) in &entries {
        if s.handled.contains(item) {
            continue;
        }
        match (before, after) {
            (true, true) => {
                let it = &s.sc.items[*item];
                if !super::anchor_eq(&it.a, &it.b) {
                    end = end.max(s.move_item(*item, t, 0.35));
                }
            }
            (true, false) => end = end.max(s.fade_out_item(*item, t, false)),
            (false, true) => {
                let hop = entries.iter().find(|(i, p, b, a)| i != item && !p.is_empty() && p == path && *b && !*a);
                let mut at = t + 0.2;
                if let Some((src, ..)) = hop {
                    let from = anchor_of(s, *src, false);
                    let to = anchor_of(s, *item, true);
                    let color = item_color(s, *item);
                    at = s.fly(from, to, t, 0.35, color, 0.8) - 0.05;
                }
                end = end.max(s.fade_in_item(*item, at));
            }
            (false, false) => {}
        }
    }
    end
}
