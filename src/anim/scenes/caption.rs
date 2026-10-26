//! The one-line summary under the graph.

use crate::git::{RefKind, RepoState};
use crate::model::{Change, NodeId, Transition};
use crate::op::{Op, OpInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Info,
    Warn,
    Err,
}

struct Sym {
    ok: &'static str,
    err: &'static str,
    arrow: &'static str,
    back: &'static str,
    swap: &'static str,
    down: &'static str,
    up: &'static str,
    ghost: &'static str,
}

const UNI: Sym = Sym { ok: "✔", err: "✖", arrow: "→", back: "↶", swap: "⇄", down: "⇣", up: "⇡", ghost: "◌" };
const ASCII: Sym = Sym { ok: "ok", err: "x", arrow: "->", back: "<-", swap: "<>", down: "v", up: "^", ghost: "o" };

fn short(tr: &Transition, n: NodeId) -> String {
    let node = &tr.graph.nodes[n];
    if node.phantom { "(merge)".into() } else { node.oid.to_hex_with_len(7).to_string() }
}

fn quote(s: &str) -> String {
    let s: String = s.chars().take(48).collect();
    format!("\u{201c}{s}\u{201d}")
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Commits a ref advanced by, counting collapsed runs.
fn distance(tr: &Transition, from: NodeId, to: NodeId) -> usize {
    tr.graph
        .route(to, from)
        .map(|r| r.iter().map(|(c, _)| tr.graph.nodes[*c].collapsed.max(1) as usize).sum())
        .unwrap_or(0)
}

pub fn caption(tr: &Transition, info: &OpInfo, unicode: bool) -> (String, Tone) {
    let y = if unicode { &UNI } else { &ASCII };
    let (text, tone) = match &info.note {
        Some((note, warn)) => {
            let text = if info.preview && !note.starts_with(y.ghost) { format!("{} preview · {note}", y.ghost) } else { note.clone() };
            (text, if *warn { Tone::Warn } else { Tone::Info })
        }
        None => {
            let (text, tone) = compose(tr, info, unicode);
            if info.preview && !text.starts_with(y.ghost) {
                // "✔ rebased x onto y" -> "◌ preview · rebased x onto y · nothing ran yet"
                let body = text.split_once(' ').map(|(_, rest)| rest).unwrap_or(&text);
                let when = if info.op == Op::Pull { " (as of your last fetch)" } else { "" };
                let tone = if tone == Tone::Err { Tone::Warn } else { Tone::Info };
                (format!("{} preview · {body}{when} · nothing ran yet", y.ghost), tone)
            } else {
                (text, tone)
            }
        }
    };
    if unicode {
        return (text, tone);
    }
    let ascii = text.replace('·', "-").replace(['\u{201c}', '\u{201d}'], "\"").replace('…', "...");
    (ascii, tone)
}

fn compose(tr: &Transition, info: &OpInfo, unicode: bool) -> (String, Tone) {
    let y = if unicode { &UNI } else { &ASCII };
    let branch = tr.after.head_branch().map(|b| b.trim_start_matches("refs/heads/").to_string());
    let on = branch.clone().unwrap_or_else(|| "detached HEAD".into());
    let spawned: Vec<NodeId> = tr
        .changes
        .iter()
        .filter_map(|c| if let Change::Spawn(n) = c { Some(*n) } else { None })
        .filter(|n| !tr.graph.nodes[*n].phantom)
        .collect();
    let vanished = tr.changes.iter().filter(|c| matches!(c, Change::Vanish(_))).count();
    let head_move = tr.changes.iter().find_map(|c| match c {
        Change::HeadMoved { from, to } => Some((from.clone(), to.clone())),
        _ => None,
    });
    let target = match info.op {
        Op::Pull => info.positionals().join(" "),
        _ => info.positionals().join(", "),
    };
    let conflicts = tr.after.conflicted_files().len();
    let new_count = spawned.iter().map(|n| tr.graph.nodes[*n].collapsed.max(1) as usize).sum::<usize>();

    if info.preview && info.op == Op::Commit
        && let Some(ph) = tr.phantom {
            let amend = info.args.iter().any(|a| a == "--amend");
            let what = if amend { "amend HEAD as" } else { "commit" };
            return (format!("{} preview · would {what} {} on {on} · nothing ran yet", y.ghost, quote(&tr.graph.nodes[ph].summary)), Tone::Info);
        }
    if info.preview && matches!(info.op, Op::CherryPick | Op::Revert) && let Some(ph) = tr.phantom {
        return (format!("{} preview · would add {} on {on} · nothing ran yet", y.ghost, quote(&tr.graph.nodes[ph].summary)), Tone::Info);
    }
    if info.preview && info.op == Op::Merge {
        if let Some(ph) = tr.phantom {
            let summary = &tr.graph.nodes[ph].summary;
            if summary.contains("conflict") {
                return (format!("{} preview: merging {target} into {on} would stop on {summary} · nothing changed", y.ghost), Tone::Warn);
            }
            return (format!("{} preview: merging {target} into {on} creates a merge commit · nothing changed", y.ghost), Tone::Info);
        }
        if let Some((from, to)) = head_move.as_ref().and_then(|(f, t)| Some((f.node?, t.node?))) {
            let n = distance(tr, from, to);
            return (
                format!("{} preview: {on} would fast-forward {} {} (+{}) · nothing changed", y.ghost, y.arrow, short(tr, to), plural(n, "commit")),
                Tone::Info,
            );
        }
        return (format!("{} preview: {on} is already up to date with {target}", y.ghost), Tone::Info);
    }

    if !info.preview && (tr.phantom.is_some() || (conflicts > 0 && tr.after.state != RepoState::Clean)) {
        let files = plural(conflicts, "file");
        let next = match tr.after.state {
            RepoState::Rebasing => "fix, git add, then git rebase --continue",
            RepoState::CherryPicking => "fix, git add, then git cherry-pick --continue",
            RepoState::Reverting => "fix, git add, then git revert --continue",
            _ => "fix them, git add, then git commit",
        };
        return (format!("{} {} stopped: conflict in {files} · {next}", y.err, info.op.verb()), Tone::Err);
    }

    let ref_move = |kind: RefKind| {
        tr.changes.iter().find_map(|c| match c {
            Change::RefMoved { kind: k, short, from, to, .. } if *k == kind => Some((short.clone(), *from, *to)),
            _ => None,
        })
    };

    let text = match info.op {
        Op::Commit => match spawned.last() {
            Some(&n) if vanished > 0 => {
                format!("{} amended {} {} {} on {on}", y.ok, quote(&tr.graph.nodes[n].summary), y.arrow, short(tr, n))
            }
            Some(&n) => format!("{} committed {} {} on {on}", y.ok, short(tr, n), quote(&tr.graph.nodes[n].summary)),
            None => return fallback(info, y),
        },
        Op::Merge | Op::Pull => {
            let merge = spawned.iter().copied().find(|n| tr.graph.nodes[*n].parents.len() > 1);
            let from_remote = ref_move(RefKind::Remote);
            let mut s = match (merge, head_move.as_ref().and_then(|(f, t)| Some((f.node?, t.node?)))) {
                (Some(m), _) => {
                    let what = if target.is_empty() { "upstream".to_string() } else { target.clone() };
                    format!("{} merged {what} into {on} {} {}", y.ok, y.arrow, short(tr, m))
                }
                (None, Some((f, t))) => {
                    let n = distance(tr, f, t);
                    format!("{} fast-forward {on} {} {} (+{})", y.ok, y.arrow, short(tr, t), plural(n, "commit"))
                }
                _ if from_remote.is_some() => format!("{} fetched, {on} already up to date", y.ok),
                _ => return fallback(info, y),
            };
            if info.op == Op::Pull && new_count > 0 && merge.is_none() {
                s.push_str(&format!(" · {} pulled", plural(new_count, "commit")));
            }
            s
        }
        Op::Rebase => {
            if spawned.is_empty() {
                match head_move.as_ref().and_then(|(f, t)| Some((f.node?, t.node?))) {
                    Some((_, t)) => format!("{} {on} fast-forwarded to {}", y.ok, short(tr, t)),
                    None => return fallback(info, y),
                }
            } else {
                let onto = if target.is_empty() { "upstream".into() } else { target.clone() };
                format!("{} rebased {on} onto {onto} · {} replayed", y.ok, plural(new_count, "commit"))
            }
        }
        Op::CherryPick | Op::Revert => {
            let verb = if info.op == Op::Revert { "reverted" } else { "cherry-picked" };
            let src = tr.changes.iter().find_map(|c| match c {
                Change::Reparent { old, .. } => Some(*old),
                _ => None,
            });
            match (src, spawned.last()) {
                (Some(o), Some(&n)) => format!("{} {verb} {} as {} on {on}", y.ok, short(tr, o), short(tr, n)),
                (None, Some(&n)) => format!("{} {verb} onto {on} {} {}", y.ok, y.arrow, short(tr, n)),
                _ => return fallback(info, y),
            }
        }
        Op::Reset => match head_move.as_ref().and_then(|(_, t)| t.node) {
            Some(t) => {
                let mode = info.reset_mode().map(|m| format!(" --{m}")).unwrap_or_default();
                let mut s = format!("{} reset{mode} {on} {} {}", y.back, y.arrow, short(tr, t));
                if vanished > 0 {
                    s.push_str(&format!(" · {} orphaned (still in the reflog)", plural(vanished, "commit")));
                }
                s
            }
            None => format!("{} reset{}", y.back, info.reset_mode().map(|m| format!(" --{m}")).unwrap_or_default()),
        },
        Op::Checkout => match &head_move {
            Some((_, to)) => match (&to.branch, to.node) {
                (Some(b), _) => format!("{} switched to {b}", y.swap),
                (None, Some(n)) => format!("{} detached HEAD at {}", y.swap, short(tr, n)),
                _ => return fallback(info, y),
            },
            None if tr.changes.iter().any(|c| matches!(c, Change::RefMoved { from: None, .. })) => {
                format!("{} created branch", y.ok)
            }
            None => format!("{} restored files", y.ok),
        },
        Op::Stash => {
            let pushed = tr.changes.iter().find_map(|c| if let Change::StashPushed(e) = c { Some(e) } else { None });
            let popped = tr.changes.iter().find_map(|c| if let Change::StashPopped(e) = c { Some(e) } else { None });
            let dirty = tr.before.worktree.as_ref().map_or(0, |w| w.dirty_count())
                .saturating_sub(tr.after.worktree.as_ref().map_or(0, |w| w.dirty_count()));
            match (pushed, popped) {
                (Some(_), _) => {
                    let left = tr.after.worktree.as_ref().map_or(0, |w| w.dirty_count());
                    let rest = if left == 0 { "working tree clean".to_string() } else { format!("{} left (untracked)", plural(left, "file")) };
                    format!("{} stashed {} · {rest}", y.down, plural(dirty, "change"))
                }
                (None, Some(_)) if info.stash_action() == "drop" => format!("{} dropped a stash entry", y.ok),
                (None, Some(_)) => format!("{} popped the stash back into the working tree", y.up),
                _ if info.stash_action() == "apply" => format!("{} applied the stash", y.up),
                _ => return fallback(info, y),
            }
        }
        Op::Fetch => {
            if new_count > 0 {
                format!("{} fetched {}", y.down, plural(new_count, "new commit"))
            } else {
                format!("{} fetched", y.down)
            }
        }
        Op::Push => match ref_move(RefKind::Remote) {
            Some((name, from, Some(to))) => {
                let n = from.map(|f| distance(tr, f, to)).unwrap_or(1);
                format!("{} pushed {} {} {name} (+{})", y.up, branch.clone().unwrap_or_default(), y.arrow, plural(n, "commit"))
            }
            Some((name, _, None)) => format!("{} deleted {name}", y.up),
            None if info.exit_code != 0 => {
                return (format!("{} push rejected: the remote has commits you don't · try git pull", y.err), Tone::Err);
            }
            None => return fallback(info, y),
        },
        Op::Branch | Op::Tag => {
            let what = if info.op == Op::Tag { "tag" } else { "branch" };
            let kind = if info.op == Op::Tag { RefKind::Tag } else { RefKind::Local };
            match ref_move(kind) {
                Some((name, None, Some(n))) => format!("{} created {what} {name} at {}", y.ok, short(tr, n)),
                Some((name, Some(_), None)) => format!("{} deleted {what} {name}", y.err),
                Some((name, _, Some(n))) => format!("{} moved {what} {name} {} {}", y.ok, y.arrow, short(tr, n)),
                _ => return fallback(info, y),
            }
        }
        Op::Stage => {
            let (b, a) = match (&tr.before.worktree, &tr.after.worktree) {
                (Some(b), Some(a)) => (b.staged.len(), a.staged.len()),
                _ => (0, 0),
            };
            if a >= b {
                format!("{} staged {}", y.ok, plural(a - b, "change"))
            } else {
                format!("{} unstaged {}", y.ok, plural(b - a, "change"))
            }
        }
        Op::Other => return fallback(info, y),
    };
    let tone = if info.exit_code != 0 { Tone::Warn } else { Tone::Ok };
    (text, tone)
}

fn fallback(info: &OpInfo, y: &Sym) -> (String, Tone) {
    if info.exit_code != 0 {
        return (format!("{} git {} exited with {}", y.err, info.op.verb(), info.exit_code), Tone::Err);
    }
    if info.args.is_empty() {
        return (String::new(), Tone::Info);
    }
    (format!("{} git {}", y.ok, info.args.join(" ")), Tone::Ok)
}
