//! Predict what a git command would do, without doing it.
//!
//! Wherever git can compute the answer itself we let it: `git merge-tree` for
//! merges, and `git replay --ref-action=print` for rebases, cherry-picks and
//! reverts (it writes the rewritten commits as unreferenced objects and moves
//! no refs, so the preview animates real commits). Ref-only operations
//! (switch, branch, tag, reset, push) are simple enough to work out directly.
//! Nothing here touches refs, the index or the working tree.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::git::{FileEntry, FileKind, HeadState, Oid, RefInfo, RefKind, Repo, RepoState, Snapshot, StashEntry, runner};
use crate::model::{Hints, Phantom, Transition};
use crate::op::{Op, OpInfo};

pub struct Prediction {
    pub before: Snapshot,
    pub after: Snapshot,
    pub phantom: Option<Phantom>,
    pub info: OpInfo,
}

impl Prediction {
    pub fn transition(&self, repo: &Repo) -> Transition {
        let hints = Hints {
            named: self.info.positionals().iter().map(|s| s.to_string()).collect(),
            locals: 0,
            context: 3,
            phantom: self.phantom.clone(),
        };
        Transition::build(repo, self.before.clone(), self.after.clone(), &hints)
    }

    fn note(&mut self, text: impl Into<String>, warn: bool) {
        self.info.note = Some((text.into(), warn));
    }
}

/// `words` is a git command without the leading `git`, e.g. `["rebase", "main"]`.
/// `network` allows `fetch` previews, which have to ask the remote.
pub fn predict(repo: &Repo, words: &[String], network: bool) -> Result<Prediction> {
    let before = Snapshot::take(repo, true);
    let sub = words.first().map(String::as_str).unwrap_or("");
    let op = Op::from_subcommand(sub).unwrap_or(Op::Other);
    let mut info = OpInfo::new(op, words.to_vec());
    info.preview = true;
    let mut p = Prediction { after: before.clone(), before, phantom: None, info };
    let cx = Cx { repo, dir: repo.workdir.clone().unwrap_or_else(|| repo.git_dir.clone()) };
    let args: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
    if p.before.head_oid().is_none() && !matches!(sub, "commit" | "add" | "rm" | "restore" | "") {
        p.note("this repository has no commits yet", true);
        return Ok(p);
    }
    match sub {
        "" => p.note("type a git command to see what it would do", false),
        "merge" => merge(&cx, &mut p, &args)?,
        "pull" => pull(&cx, &mut p, &args)?,
        "rebase" => rebase(&cx, &mut p, &args)?,
        "cherry-pick" => pick(&cx, &mut p, &args, false)?,
        "revert" => pick(&cx, &mut p, &args, true)?,
        "reset" => reset(&cx, &mut p, &args),
        "checkout" | "switch" => switch(&cx, &mut p, &args, sub == "switch"),
        "branch" => branch(&cx, &mut p, &args),
        "tag" => tag(&cx, &mut p, &args),
        "commit" => commit(&cx, &mut p, &args),
        "stash" => stash(&cx, &mut p, &args),
        "push" => push(&cx, &mut p, &args),
        "fetch" => fetch(&cx, &mut p, &args, network),
        "add" | "rm" | "restore" | "mv" => stage(&mut p, sub, &args),
        "status" | "log" | "diff" | "show" | "blame" | "grep" | "ls-files" | "describe" | "reflog" | "shortlog"
        | "remote" | "config" | "help" | "version" | "whatchanged" | "range-diff" | "cherry" => {
            p.note(format!("git {sub} only reads: nothing would change"), false)
        }
        other => p.note(format!("no preview for git {other} yet"), false),
    }
    Ok(p)
}

struct Cx<'a> {
    repo: &'a Repo,
    dir: PathBuf,
}

impl Cx<'_> {
    fn git(&self, args: &[&str]) -> Option<(i32, String)> {
        let (code, out) = runner::output(Some(&self.dir), args)?;
        Some((code, String::from_utf8_lossy(&out).into_owned()))
    }

    fn rev(&self, spec: &str) -> Option<Oid> {
        let (code, out) = self.git(&["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{spec}^{{commit}}")])?;
        (code == 0).then(|| Oid::from_hex(out.trim().as_bytes()).ok()).flatten()
    }

    fn is_ancestor(&self, a: Oid, b: Oid) -> bool {
        a == b || self.git(&["merge-base", "--is-ancestor", &a.to_string(), &b.to_string()]).is_some_and(|(c, _)| c == 0)
    }

    fn remotes(&self) -> Vec<String> {
        self.git(&["remote"]).map(|(_, o)| o.lines().map(String::from).collect()).unwrap_or_default()
    }

    fn summary(&self, oid: Oid) -> String {
        self.repo.commit(oid).map(|c| c.summary).unwrap_or_default()
    }

    /// Is `git replay` available, and which of its options does it know?
    fn replay(&self) -> Option<Replay> {
        let (_, help) = runner::output(Some(&self.dir), &["replay", "-h"]).map(|(c, o)| (c, String::from_utf8_lossy(&o).into_owned()))?;
        if !help.contains("--onto") {
            return None;
        }
        // Newer git updates refs by default; only ever run it in print mode.
        let print = help.contains("--ref-action");
        Some(Replay { print, has_ref: help.contains("--ref "), linearize: help.contains("--linearize"), revert: help.contains("--revert") })
    }

    /// Run `git replay` read-only. Returns the new tip, or None on conflicts.
    fn run_replay(&self, rp: &Replay, mode: &[&str], ranges: &[String], dest_ref: Option<&str>) -> Option<Option<Oid>> {
        let mut args: Vec<String> = vec!["replay".into()];
        if rp.print {
            args.push("--ref-action=print".into());
        }
        if rp.linearize {
            args.push("--linearize".into());
        }
        args.extend(mode.iter().map(|s| s.to_string()));
        if let (Some(r), true) = (dest_ref, rp.has_ref) {
            args.push(format!("--ref={r}"));
        }
        args.extend(ranges.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, out) = self.git(&refs)?;
        match code {
            0 => Some(out.lines().find_map(|l| {
                let mut w = l.split_whitespace();
                (w.next() == Some("update")).then(|| w.nth(1).and_then(|o| Oid::from_hex(o.as_bytes()).ok())).flatten()
            })),
            1 => Some(None),
            _ => None,
        }
    }
}

struct Replay {
    print: bool,
    has_ref: bool,
    linearize: bool,
    revert: bool,
}

// ---- snapshot edits ------------------------------------------------------------

fn current_branch(s: &Snapshot) -> Option<String> {
    s.head_branch().map(|b| b.trim_start_matches("refs/heads/").to_string())
}

fn set_ref(s: &mut Snapshot, full: &str, target: Option<Oid>) {
    s.refs.retain(|r| r.name != full);
    if let Some(target) = target {
        let (kind, short) = if let Some(x) = full.strip_prefix("refs/heads/") {
            (RefKind::Local, x)
        } else if let Some(x) = full.strip_prefix("refs/remotes/") {
            (RefKind::Remote, x)
        } else {
            (RefKind::Tag, full.trim_start_matches("refs/tags/"))
        };
        s.refs.push(RefInfo { name: full.to_string(), short: short.to_string(), kind, target });
    }
}

/// Move whatever HEAD points at (its branch, or HEAD itself when detached).
fn advance_head(s: &mut Snapshot, oid: Oid) {
    match s.head.clone() {
        HeadState::Attached { branch, .. } => {
            set_ref(s, &branch, Some(oid));
            s.head = HeadState::Attached { branch, oid: Some(oid) };
        }
        _ => s.head = HeadState::Detached(oid),
    }
}

fn attach(s: &mut Snapshot, branch_short: &str, oid: Oid) {
    let full = format!("refs/heads/{branch_short}");
    set_ref(s, &full, Some(oid));
    s.head = HeadState::Attached { branch: full, oid: Some(oid) };
}

fn flag(args: &[&str], names: &[&str]) -> bool {
    args.iter().any(|a| names.contains(a))
}

/// One parsed option or positional argument.
#[derive(Debug, PartialEq)]
enum Arg<'a> {
    Short(char, Option<String>),
    Long(&'a str, Option<String>),
    Pos(&'a str),
}

/// Parse arguments the way git's option parser does: `-am msg` is `-a -m msg`,
/// `-mmsg` is `-m msg`, `--opt=v` and `--opt v` both work. `short_values` lists the
/// short options that take a value, `long_values` the long ones. Stops at `--`.
fn scan<'a>(args: &[&'a str], short_values: &str, long_values: &[&str]) -> Vec<Arg<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        i += 1;
        if a == "--" {
            break;
        }
        if let Some(long) = a.strip_prefix("--") {
            match long.split_once('=') {
                Some((n, v)) => out.push(Arg::Long(&a[2..2 + n.len()], Some(v.to_string()))),
                None if long_values.contains(&a) => {
                    out.push(Arg::Long(long, args.get(i).map(|v| v.to_string())));
                    i += 1;
                }
                None => out.push(Arg::Long(long, None)),
            }
        } else if a.len() > 1 && a.starts_with('-') && !a[1..].starts_with(|c: char| c.is_ascii_digit()) {
            let chars: Vec<char> = a[1..].chars().collect();
            for (k, c) in chars.iter().enumerate() {
                if short_values.contains(*c) {
                    let rest: String = chars[k + 1..].iter().collect();
                    let value = if rest.is_empty() {
                        i += 1;
                        args.get(i - 1).map(|v| v.to_string())
                    } else {
                        Some(rest)
                    };
                    out.push(Arg::Short(*c, value));
                    break;
                }
                out.push(Arg::Short(*c, None));
            }
        } else {
            out.push(Arg::Pos(a));
        }
    }
    out
}

/// Values of an option, by short letter and/or long name (without dashes).
fn opt_values(parsed: &[Arg], short: Option<char>, long: &str) -> Vec<String> {
    parsed
        .iter()
        .filter_map(|a| match a {
            Arg::Short(c, v) if Some(*c) == short => v.clone(),
            Arg::Long(n, v) if *n == long => v.clone(),
            _ => None,
        })
        .collect()
}

fn has_opt(parsed: &[Arg], short: Option<char>, long: &str) -> bool {
    parsed.iter().any(|a| match a {
        Arg::Short(c, _) => Some(*c) == short,
        Arg::Long(n, _) => *n == long,
        _ => false,
    })
}

fn pos_of<'a>(parsed: &[Arg<'a>]) -> Vec<&'a str> {
    parsed.iter().filter_map(|a| if let Arg::Pos(p) = a { Some(*p) } else { None }).collect()
}

/// Positional arguments, given which short and long options take a value.
fn positionals<'a>(args: &[&'a str], takes_value: &[&str]) -> Vec<&'a str> {
    let short: String = takes_value.iter().filter(|o| o.len() == 2 && !o.starts_with("--")).map(|o| &o[1..]).collect();
    pos_of(&scan(args, &short, takes_value))
}

/// Values of `--long=x` / `--long x` (or a short alias).
fn values(args: &[&str], short: &str, long: &str) -> Vec<String> {
    let s = short.strip_prefix('-').and_then(|s| s.chars().next());
    let parsed = scan(args, &s.map(String::from).unwrap_or_default(), &[long]);
    opt_values(&parsed, s, long.trim_start_matches("--"))
}

fn after_dashdash<'a>(args: &[&'a str]) -> Vec<&'a str> {
    args.iter().position(|a| *a == "--").map(|i| args[i + 1..].to_vec()).unwrap_or_default()
}

fn matches_path(path: &str, specs: &[&str]) -> bool {
    specs.iter().any(|s| {
        let s = s.trim_start_matches("./").trim_end_matches('/');
        s == "." || s == ":/" || s.is_empty() || path == s || path.starts_with(&format!("{s}/")) || {
            s.strip_suffix('*').is_some_and(|prefix| path.starts_with(prefix))
        }
    })
}

fn plural(n: usize, w: &str) -> String {
    format!("{n} {w}{}", if n == 1 { "" } else { "s" })
}

// ---- operations -----------------------------------------------------------------

fn merge(cx: &Cx, p: &mut Prediction, args: &[&str]) -> Result<()> {
    if flag(args, &["--abort", "--continue", "--quit"]) {
        p.note("no preview for finishing or aborting a merge", false);
        return Ok(());
    }
    let mut targets: Vec<String> = positionals(args, &["-m", "-F", "-s", "-X", "--strategy", "--strategy-option"])
        .into_iter()
        .map(String::from)
        .collect();
    if targets.is_empty() {
        let up = upstream(cx, &p.before).context("no branch to merge (and no upstream configured)")?;
        targets.push(up.0);
    }
    merge_targets(cx, p, &targets, flag(args, &["--no-ff"]), flag(args, &["--ff-only"]), flag(args, &["--squash"]))
}

fn merge_targets(cx: &Cx, p: &mut Prediction, targets: &[String], no_ff: bool, ff_only: bool, squash: bool) -> Result<()> {
    let head = p.before.head_oid().context("HEAD does not point at a commit")?;
    let mut theirs = Vec::new();
    for t in targets {
        let oid = cx.rev(t).with_context(|| format!("unknown revision '{t}'"))?;
        if !cx.is_ancestor(oid, head) {
            theirs.push((t.clone(), oid));
        }
    }
    if theirs.is_empty() {
        p.note(format!("{} is already up to date with {}", current_branch(&p.before).unwrap_or("HEAD".into()), targets.join(", ")), false);
        return Ok(());
    }
    if squash {
        p.note("--squash would stage the combined changes without creating a merge commit", false);
        return Ok(());
    }
    if theirs.len() == 1 && !no_ff && cx.is_ancestor(head, theirs[0].1) {
        advance_head(&mut p.after, theirs[0].1);
        return Ok(());
    }
    if ff_only {
        p.note("--ff-only would stop: the branches have diverged, so a fast-forward is impossible", true);
        return Ok(());
    }
    let mut conflicts: Vec<String> = Vec::new();
    for (_, oid) in &theirs {
        let (code, out) = cx
            .git(&["merge-tree", "--write-tree", "--name-only", "--no-messages", &head.to_string(), &oid.to_string()])
            .context("git merge-tree failed (needs git 2.38+)")?;
        if code == 1 {
            conflicts.extend(out.lines().skip(1).take_while(|l| !l.is_empty()).map(String::from));
        } else if code != 0 {
            anyhow::bail!("git merge-tree failed (needs git 2.38+)");
        }
    }
    conflicts.dedup();
    let branch = current_branch(&p.before).unwrap_or_else(|| "HEAD".into());
    let names: Vec<&str> = theirs.iter().map(|(n, _)| n.as_str()).collect();
    let summary = if !conflicts.is_empty() {
        let mut list = conflicts.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if conflicts.len() > 3 {
            list.push_str(", …");
        }
        format!("conflicts in {}: {list}", plural(conflicts.len(), "file"))
    } else if names.len() == 1 {
        format!("Merge branch '{}' into {branch}", names[0])
    } else {
        format!("Merge branches {} into {branch}", names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", "))
    };
    let mut parents = vec![head];
    parents.extend(theirs.iter().map(|(_, o)| *o));
    p.phantom = Some(Phantom { parents, summary, retarget_head: conflicts.is_empty() });
    Ok(())
}

/// (short name, oid) of the current branch's upstream.
fn upstream(cx: &Cx, s: &Snapshot) -> Option<(String, Oid)> {
    let branch = current_branch(s)?;
    let full = cx.repo.upstream(&branch)?;
    let r = s.find_ref(&full)?;
    Some((r.short.clone(), r.target))
}

fn pull(cx: &Cx, p: &mut Prediction, args: &[&str]) -> Result<()> {
    let Some((name, _)) = upstream(cx, &p.before) else {
        p.note("no upstream branch to pull from (set one with git branch -u)", true);
        return Ok(());
    };
    let rebase_cfg = cx.git(&["config", "--get", "pull.rebase"]).is_some_and(|(_, v)| v.trim() == "true");
    let rebase_mode = (rebase_cfg || flag(args, &["--rebase", "-r"])) && !flag(args, &["--no-rebase"]);
    if rebase_mode {
        rebase_onto(cx, p, &name, None, None)
    } else {
        merge_targets(cx, p, &[name], flag(args, &["--no-ff"]), flag(args, &["--ff-only"]), false)
    }
}

fn rebase(cx: &Cx, p: &mut Prediction, args: &[&str]) -> Result<()> {
    if flag(args, &["--abort", "--continue", "--skip", "--quit", "--edit-todo"]) {
        p.note("no preview for continuing or aborting a rebase", false);
        return Ok(());
    }
    let onto = values(args, "", "--onto").into_iter().next();
    let pos = positionals(args, &["--onto", "-x", "--exec", "-s", "-X", "--strategy", "--strategy-option"]);
    let upstream_name = match pos.first() {
        Some(u) => u.to_string(),
        None => match upstream(cx, &p.before) {
            Some((u, _)) => u,
            None => {
                p.note("no upstream to rebase onto: name a branch, e.g. git rebase main", true);
                return Ok(());
            }
        },
    };
    rebase_onto(cx, p, &upstream_name, onto.as_deref(), pos.get(1).copied())
}

fn rebase_onto(cx: &Cx, p: &mut Prediction, upstream: &str, onto: Option<&str>, branch: Option<&str>) -> Result<()> {
    let base = cx.rev(upstream).with_context(|| format!("unknown revision '{upstream}'"))?;
    let newbase = match onto {
        Some(o) => cx.rev(o).with_context(|| format!("unknown revision '{o}'"))?,
        None => base,
    };
    if let Some(b) = branch {
        let oid = cx.rev(b).with_context(|| format!("unknown branch '{b}'"))?;
        attach(&mut p.after, b, oid);
    }
    let Some(branch) = current_branch(&p.after) else {
        p.note("rebase previews need a branch checked out", false);
        return Ok(());
    };
    let tip = p.after.head_oid().context("HEAD does not point at a commit")?;
    if cx.is_ancestor(tip, newbase) {
        advance_head(&mut p.after, newbase); // nothing of ours to replay: fast-forward
        return Ok(());
    }
    let Some(rp) = cx.replay() else {
        p.note("rebase previews need git 2.44 or newer (git replay)", true);
        return Ok(());
    };
    let full = format!("refs/heads/{branch}");
    let onto_arg = newbase.to_string();
    let range = format!("{base}..{tip}");
    match cx.run_replay(&rp, &["--onto", &onto_arg], std::slice::from_ref(&range), Some(&full)) {
        Some(Some(new)) => {
            advance_head(&mut p.after, new);
            Ok(())
        }
        Some(None) => {
            rebase_conflict(cx, p, &rp, base, newbase, tip, &full);
            Ok(())
        }
        None => {
            p.note(format!("{branch} is already up to date with {upstream}"), false);
            Ok(())
        }
    }
}

/// Replay one more commit at a time to find where the rebase would stop.
fn rebase_conflict(cx: &Cx, p: &mut Prediction, rp: &Replay, base: Oid, newbase: Oid, tip: Oid, full: &str) {
    let list = cx
        .git(&["rev-list", "--reverse", "--no-merges", &format!("{base}..{tip}")])
        .map(|(_, o)| o.lines().filter_map(|l| Oid::from_hex(l.as_bytes()).ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut last_good = newbase;
    let mut stop = list.first().copied();
    if rp.has_ref {
        for (k, c) in list.iter().enumerate() {
            let range = format!("{base}..{c}");
            match cx.run_replay(rp, &["--onto", &newbase.to_string()], &[range], Some(full)) {
                Some(Some(new)) => last_good = new,
                _ => {
                    stop = Some(list[k]);
                    break;
                }
            }
        }
    }
    p.after.head = HeadState::Detached(last_good);
    p.after.state = RepoState::Rebasing;
    p.after.pick_head = stop;
    let what = stop.map(|s| format!("{} \u{201c}{}\u{201d}", &s.to_string()[..7], cx.summary(s))).unwrap_or_default();
    p.phantom = Some(Phantom {
        parents: vec![last_good],
        summary: format!("{} · conflict", stop.map(|s| cx.summary(s)).unwrap_or_default()),
        retarget_head: false,
    });
    p.note(format!("◌ preview · the rebase would stop on a conflict at {what} · nothing ran yet"), true);
}

fn pick(cx: &Cx, p: &mut Prediction, args: &[&str], revert: bool) -> Result<()> {
    if flag(args, &["--abort", "--continue", "--skip", "--quit"]) {
        p.note("no preview for continuing or aborting", false);
        return Ok(());
    }
    let commits = positionals(args, &["-m", "--mainline", "-X", "--strategy-option", "--strategy"]);
    if commits.is_empty() {
        p.note(format!("name a commit to {}", if revert { "revert" } else { "cherry-pick" }), false);
        return Ok(());
    }
    let head = p.before.head_oid().context("HEAD does not point at a commit")?;
    let first = commits[0].split("..").last().unwrap_or(commits[0]);
    let source = cx.rev(first).with_context(|| format!("unknown revision '{first}'"))?;
    let branch = current_branch(&p.before);
    let rp = cx.replay();
    let usable = rp.as_ref().is_some_and(|r| if revert { r.revert } else { true });
    if let (Some(branch), Some(rp), true) = (branch.as_deref(), rp.as_ref(), usable) {
        let ranges: Vec<String> = commits
            .iter()
            .map(|c| if c.contains("..") { c.to_string() } else { format!("{c}^..{c}") })
            .collect();
        let mode = if revert { "--revert" } else { "--advance" };
        match cx.run_replay(rp, &[mode, branch], &ranges, None) {
            Some(Some(new)) => {
                advance_head(&mut p.after, new);
                return Ok(());
            }
            Some(None) => {
                p.phantom = Some(Phantom { parents: vec![head], summary: format!("{} · conflict", cx.summary(source)), retarget_head: false });
                p.after.pick_head = Some(source);
                p.after.state = if revert { RepoState::Reverting } else { RepoState::CherryPicking };
                p.note(format!("◌ preview · applying {} would stop on a conflict · nothing ran yet", &source.to_string()[..7]), true);
                return Ok(());
            }
            None => {}
        }
    }
    // Older git, or a detached HEAD: show the new commit as a phantom.
    let summary = if revert { format!("Revert \"{}\"", cx.summary(source)) } else { cx.summary(source) };
    p.phantom = Some(Phantom { parents: vec![head], summary, retarget_head: true });
    Ok(())
}

fn diff_files(cx: &Cx, from: Oid, to: Oid) -> Vec<FileEntry> {
    cx.git(&["diff", "--name-status", "--no-renames", &from.to_string(), &to.to_string()])
        .map(|(_, o)| {
            o.lines()
                .filter_map(|l| {
                    let (k, path) = l.split_once('\t')?;
                    let kind = match k.chars().next()? {
                        'A' => FileKind::Added,
                        'D' => FileKind::Deleted,
                        _ => FileKind::Modified,
                    };
                    Some(FileEntry { path: path.to_string(), kind })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn merge_files(into: &mut Vec<FileEntry>, more: impl IntoIterator<Item = FileEntry>) {
    for f in more {
        if !into.iter().any(|x| x.path == f.path) {
            into.push(f);
        }
    }
}

fn reset(cx: &Cx, p: &mut Prediction, args: &[&str]) {
    let pos = positionals(args, &[]);
    let mut paths = after_dashdash(args);
    let target = match pos.first() {
        Some(t) if cx.rev(t).is_some() => {
            paths.extend(pos[1..].iter().copied());
            cx.rev(t)
        }
        _ => {
            paths.extend(pos.iter().copied());
            None
        }
    };
    let Some(wt) = p.after.worktree.as_mut() else { return };
    if !paths.is_empty() {
        // `git reset <paths>`: unstage.
        let (moved, kept): (Vec<FileEntry>, Vec<FileEntry>) = wt.staged.drain(..).partition(|f| matches_path(&f.path, &paths));
        wt.staged = kept;
        merge_files(&mut wt.unstaged, moved);
        return;
    }
    let head = p.before.head_oid().unwrap();
    let target = target.unwrap_or(head);
    let changes = diff_files(cx, target, head);
    if flag(args, &["--hard"]) {
        wt.staged.clear();
        wt.unstaged.clear();
        wt.conflicted.clear();
    } else if flag(args, &["--soft"]) {
        merge_files(&mut wt.staged, changes);
    } else {
        let staged: Vec<FileEntry> = std::mem::take(&mut wt.staged);
        merge_files(&mut wt.unstaged, staged);
        merge_files(&mut wt.unstaged, changes);
    }
    advance_head(&mut p.after, target);
}

fn switch(cx: &Cx, p: &mut Prediction, args: &[&str], is_switch: bool) {
    let paths = after_dashdash(args);
    if !paths.is_empty() {
        restore_paths(p, &paths, false, true);
        return;
    }
    let parsed = scan(args, "cCbB", &["--create", "--force-create", "--orphan"]);
    let creates = ['c', 'C', 'b', 'B']
        .iter()
        .flat_map(|c| opt_values(&parsed, Some(*c), if *c == 'c' { "create" } else { "force-create" }))
        .next();
    let pos = pos_of(&parsed);
    let head = p.before.head_oid().unwrap();
    if let Some(new) = creates {
        let start = pos.first().and_then(|s| cx.rev(s)).unwrap_or(head);
        attach(&mut p.after, &new, start);
        return;
    }
    let Some(target) = pos.first().copied() else {
        p.note("name a branch to switch to", false);
        return;
    };
    let target = if target == "-" {
        match cx.git(&["rev-parse", "--abbrev-ref", "@{-1}"]) {
            Some((0, name)) => name.trim().to_string(),
            _ => {
                p.note("there is no previous branch", true);
                return;
            }
        }
    } else {
        target.to_string()
    };
    let local = format!("refs/heads/{target}");
    if flag(args, &["--detach", "-d"]) {
        match cx.rev(&target) {
            Some(oid) => p.after.head = HeadState::Detached(oid),
            None => p.note(format!("unknown revision '{target}'"), true),
        }
    } else if let Some(r) = p.before.find_ref(&local) {
        let oid = r.target;
        p.after.head = HeadState::Attached { branch: local, oid: Some(oid) };
    } else if let Some(remote) = p.before.refs.iter().find(|r| r.kind == RefKind::Remote && r.short.ends_with(&format!("/{target}"))) {
        // git's DWIM: create a local branch tracking the remote one.
        let oid = remote.target;
        attach(&mut p.after, &target, oid);
    } else if let Some(oid) = cx.rev(&target) {
        if is_switch {
            p.note(format!("git switch needs --detach to check out the commit {target}"), true);
        } else {
            p.after.head = HeadState::Detached(oid);
        }
    } else if !is_switch {
        restore_paths(p, &[target.as_str()], false, true);
    } else {
        p.note(format!("no branch named '{target}'"), true);
    }
}

fn restore_paths(p: &mut Prediction, paths: &[&str], staged: bool, worktree: bool) {
    let Some(wt) = p.after.worktree.as_mut() else { return };
    if staged {
        let (moved, kept): (Vec<FileEntry>, Vec<FileEntry>) = wt.staged.drain(..).partition(|f| matches_path(&f.path, paths));
        wt.staged = kept;
        if !worktree {
            merge_files(&mut wt.unstaged, moved);
        }
    }
    if worktree {
        wt.unstaged.retain(|f| !matches_path(&f.path, paths));
    }
}

fn branch(cx: &Cx, p: &mut Prediction, args: &[&str]) {
    let pos = positionals(args, &["-u", "--set-upstream-to", "--contains", "--merged", "--no-merged", "--points-at", "--sort", "--format"]);
    let head = p.before.head_oid().unwrap();
    if flag(args, &["-d", "-D", "--delete"]) {
        for name in &pos {
            let full = format!("refs/heads/{name}");
            if let Some(r) = p.before.find_ref(&full) {
                if !flag(args, &["-D", "--force", "-f"]) && !cx.is_ancestor(r.target, head) {
                    p.note(format!("{name} is not fully merged: -d would refuse (-D deletes it anyway)"), true);
                    return;
                }
                set_ref(&mut p.after, &full, None);
            }
        }
        return;
    }
    if flag(args, &["-m", "-M", "--move", "-c", "-C", "--copy"]) {
        let copy = flag(args, &["-c", "-C", "--copy"]);
        let (old, new) = match pos.as_slice() {
            [new] => (current_branch(&p.before).unwrap_or_default(), new.to_string()),
            [old, new, ..] => (old.to_string(), new.to_string()),
            _ => return,
        };
        let Some(oid) = p.before.find_ref(&format!("refs/heads/{old}")).map(|r| r.target) else { return };
        if !copy {
            set_ref(&mut p.after, &format!("refs/heads/{old}"), None);
        }
        set_ref(&mut p.after, &format!("refs/heads/{new}"), Some(oid));
        if !copy && current_branch(&p.before).as_deref() == Some(old.as_str()) {
            p.after.head = HeadState::Attached { branch: format!("refs/heads/{new}"), oid: Some(oid) };
        }
        return;
    }
    let listing = pos.is_empty() || flag(args, &["-l", "--list", "-a", "--all", "-r", "--remotes", "-v", "-vv", "--show-current"]);
    if listing || flag(args, &["-u", "--set-upstream-to", "--unset-upstream", "--edit-description"]) {
        p.note("this only lists branches or changes config: the graph would not change", false);
        return;
    }
    let start = pos.get(1).and_then(|s| cx.rev(s)).unwrap_or(head);
    set_ref(&mut p.after, &format!("refs/heads/{}", pos[0]), Some(start));
}

fn tag(cx: &Cx, p: &mut Prediction, args: &[&str]) {
    let pos = positionals(args, &["-m", "-F", "-u", "--contains", "--points-at", "--sort", "--format"]);
    if flag(args, &["-d", "--delete"]) {
        for name in pos {
            set_ref(&mut p.after, &format!("refs/tags/{name}"), None);
        }
        return;
    }
    if pos.is_empty() || flag(args, &["-l", "--list", "-v", "--verify"]) {
        p.note("this only lists tags: the graph would not change", false);
        return;
    }
    let target = pos.get(1).and_then(|s| cx.rev(s)).or(p.before.head_oid());
    set_ref(&mut p.after, &format!("refs/tags/{}", pos[0]), target);
}

fn commit(cx: &Cx, p: &mut Prediction, args: &[&str]) {
    let Some(wt) = p.before.worktree.clone() else { return };
    let parsed = scan(
        args,
        "mFCct",
        &["--message", "--file", "--reuse-message", "--reedit-message", "--fixup", "--squash", "--author", "--date", "--template", "--cleanup"],
    );
    let amend = has_opt(&parsed, None, "amend");
    let all = has_opt(&parsed, Some('a'), "all");
    let mut paths = after_dashdash(args);
    paths.extend(pos_of(&parsed));
    let mut committed: Vec<FileEntry> = wt.staged.clone();
    if all {
        merge_files(&mut committed, wt.unstaged.iter().cloned());
    }
    if !paths.is_empty() {
        committed = wt.staged.iter().chain(wt.unstaged.iter()).filter(|f| matches_path(&f.path, &paths)).cloned().collect();
    }
    if committed.is_empty() && !amend && !flag(args, &["--allow-empty"]) {
        p.note("nothing to commit: stage changes with git add, or use git commit -a", true);
        return;
    }
    let head = p.before.head_oid();
    let messages = opt_values(&parsed, Some('m'), "message");
    let fixup = opt_values(&parsed, None, "fixup").into_iter().next();
    let summary = if let Some(m) = messages.first() {
        m.lines().next().unwrap_or("").to_string()
    } else if let Some(f) = fixup.as_deref().and_then(|f| cx.rev(f)) {
        format!("fixup! {}", cx.summary(f))
    } else if let (true, Some(h)) = (amend, head) {
        cx.summary(h)
    } else {
        "(message from your editor)".to_string()
    };
    let parents: Vec<Oid> = match (amend, head) {
        (true, Some(h)) => cx.repo.commit(h).map(|c| c.parents.to_vec()).unwrap_or_default(),
        (false, Some(h)) => vec![h],
        _ => Vec::new(),
    };
    if parents.is_empty() {
        p.note(format!("◌ preview · would create the first commit \u{201c}{summary}\u{201d} · nothing ran yet"), false);
        return;
    }
    p.phantom = Some(Phantom { parents, summary, retarget_head: true });
    if let Some(w) = p.after.worktree.as_mut() {
        w.staged.retain(|f| !committed.iter().any(|c| c.path == f.path));
        w.unstaged.retain(|f| !committed.iter().any(|c| c.path == f.path));
    }
}

fn stash(cx: &Cx, p: &mut Prediction, args: &[&str]) {
    let pos = positionals(args, &["-m", "--message"]);
    let action = pos.first().copied().unwrap_or("push");
    let action = if action == "save" { "push" } else { action };
    let index: usize = pos
        .get(1)
        .and_then(|s| s.trim_start_matches("stash@{").trim_end_matches('}').parse().ok())
        .unwrap_or(0);
    match action {
        "push" => {
            let Some(wt) = p.after.worktree.as_mut() else { return };
            let untracked = flag(args, &["-u", "--include-untracked", "-a", "--all"]);
            if wt.staged.is_empty() && wt.unstaged.is_empty() && (!untracked || wt.untracked.is_empty()) {
                p.note("no local changes to stash", false);
                return;
            }
            if !flag(args, &["--keep-index", "-k"]) {
                wt.staged.clear();
            }
            wt.unstaged.clear();
            if untracked {
                wt.untracked.clear();
            }
            let branch = current_branch(&p.before).unwrap_or_else(|| "(no branch)".into());
            let head = p.before.head_oid().unwrap();
            let message = match values(args, "-m", "--message").first() {
                Some(m) => format!("On {branch}: {m}"),
                None => format!("WIP on {branch}: {} {}", &head.to_string()[..7], cx.summary(head)),
            };
            let mut bytes = [0xdd_u8; 20];
            bytes[19] = p.before.stash.len() as u8;
            p.after.stash.insert(0, StashEntry { oid: Oid::from_bytes_or_panic(&bytes), message });
        }
        "pop" | "apply" => {
            if index >= p.before.stash.len() {
                p.note("there is no such stash entry", true);
                return;
            }
            let spec = format!("stash@{{{index}}}");
            let files = cx
                .git(&["stash", "show", "--name-status", "--include-untracked", &spec])
                .filter(|(c, _)| *c == 0)
                .or_else(|| cx.git(&["stash", "show", "--name-status", &spec]))
                .map(|(_, o)| o)
                .unwrap_or_default();
            if let Some(wt) = p.after.worktree.as_mut() {
                for l in files.lines() {
                    if let Some((k, path)) = l.split_once('\t') {
                        let kind = if k.starts_with('A') { FileKind::Added } else if k.starts_with('D') { FileKind::Deleted } else { FileKind::Modified };
                        merge_files(&mut wt.unstaged, [FileEntry { path: path.to_string(), kind }]);
                    }
                }
            }
            if action == "pop" {
                p.after.stash.remove(index);
            }
        }
        "drop" => {
            if index < p.after.stash.len() {
                p.after.stash.remove(index);
            }
        }
        "clear" => p.after.stash.clear(),
        _ => p.note(format!("git stash {action} would not change anything graft draws"), false),
    }
}

fn push(cx: &Cx, p: &mut Prediction, args: &[&str]) {
    let pos = positionals(args, &["-o", "--push-option", "--repo", "--receive-pack", "--exec"]);
    let branch = current_branch(&p.before);
    let configured = branch
        .as_deref()
        .and_then(|b| cx.git(&["config", "--get", &format!("branch.{b}.remote")]))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let remotes = cx.remotes();
    let remote = pos.first().map(|s| s.to_string()).or(configured).or_else(|| remotes.first().cloned()).unwrap_or_else(|| "origin".into());
    if !remotes.contains(&remote) {
        p.note(format!("there is no remote named '{remote}' to push to (add one with git remote add)"), true);
        return;
    }
    let mut specs: Vec<String> = pos.iter().skip(1).map(|s| s.to_string()).collect();
    if specs.is_empty() {
        let Some(b) = branch.clone() else {
            p.note("push previews need a branch checked out", false);
            return;
        };
        let dst = cx
            .git(&["config", "--get", &format!("branch.{b}.merge")])
            .map(|(_, v)| v.trim().trim_start_matches("refs/heads/").to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| b.clone());
        specs.push(format!("{b}:{dst}"));
    }
    let force_all = flag(args, &["-f", "--force", "--force-with-lease", "--force-if-includes"]);
    let delete = flag(args, &["-d", "--delete"]);
    let mut rejected = Vec::new();
    for spec in specs {
        let (force, spec) = match spec.strip_prefix('+') {
            Some(s) => (true, s.to_string()),
            None => (force_all, spec),
        };
        let (src, dst) = match spec.split_once(':') {
            Some((s, d)) => (s.to_string(), d.to_string()),
            None => (spec.clone(), spec.clone()),
        };
        let dst = dst.trim_start_matches("refs/heads/").to_string();
        let tracking = format!("refs/remotes/{remote}/{dst}");
        if delete || src.is_empty() {
            set_ref(&mut p.after, &tracking, None);
            continue;
        }
        let Some(local) = cx.rev(&src) else { continue };
        match p.before.find_ref(&tracking).map(|r| r.target) {
            Some(old) if old == local => {}
            Some(old) if !force && !cx.is_ancestor(old, local) => rejected.push(dst),
            _ => set_ref(&mut p.after, &tracking, Some(local)),
        }
    }
    if !rejected.is_empty() {
        p.info.exit_code = 1; // the rejection scene
    } else if p.after.refs == p.before.refs {
        p.note(format!("{remote} is already up to date (as of your last fetch)"), false);
    }
}

fn fetch(cx: &Cx, p: &mut Prediction, args: &[&str], network: bool) {
    if cx.remotes().is_empty() {
        p.note("this repository has no remote to fetch from", true);
        return;
    }
    if !network {
        p.note("fetch has to ask the remote: run it to see what arrives", false);
        return;
    }
    let mut cmd: Vec<&str> = vec!["fetch", "--dry-run", "--porcelain"];
    cmd.extend(args.iter().filter(|a| **a != "--dry-run"));
    let out = runner::output_env(Some(&cx.dir), &cmd, &[("GIT_TERMINAL_PROMPT", "0")]);
    let Some((0, text)) = out.map(|(c, o)| (c, String::from_utf8_lossy(&o).into_owned())) else {
        p.note("could not ask the remote (offline, or git older than 2.41)", true);
        return;
    };
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let (_flag, _old, new, name) = (w.next(), w.next(), w.next(), w.next());
        if let (Some(new), Some(name)) = (new, name) {
            let target = Oid::from_hex(new.as_bytes()).ok().filter(|o| !o.is_null());
            set_ref(&mut p.after, name, target);
        }
    }
    if p.after.refs == p.before.refs {
        p.note("already up to date: the remote has nothing new", false);
    }
}

fn stage(p: &mut Prediction, sub: &str, args: &[&str]) {
    let mut paths = positionals(args, &["--chmod", "--pathspec-from-file"]);
    paths.extend(after_dashdash(args));
    let all = flag(args, &["-A", "--all", "-u", "--update"]);
    if paths.is_empty() && !all {
        p.note(format!("name the files to {sub}"), false);
        return;
    }
    let specs: Vec<&str> = if all { vec!["."] } else { paths.clone() };
    let Some(wt) = p.after.worktree.as_mut() else { return };
    match sub {
        "add" => {
            let tracked_only = flag(args, &["-u", "--update"]);
            let (moved, kept): (Vec<FileEntry>, Vec<FileEntry>) = wt.unstaged.drain(..).partition(|f| matches_path(&f.path, &specs));
            wt.unstaged = kept;
            merge_files(&mut wt.staged, moved);
            if !tracked_only {
                let (moved, kept): (Vec<FileEntry>, Vec<FileEntry>) =
                    wt.untracked.drain(..).partition(|f| matches_path(&f.path, &specs));
                wt.untracked = kept;
                merge_files(&mut wt.staged, moved.into_iter().map(|f| FileEntry { kind: FileKind::Added, ..f }));
            }
            wt.conflicted.retain(|f| !matches_path(&f.path, &specs));
        }
        "rm" => {
            for spec in &specs {
                wt.unstaged.retain(|f| !matches_path(&f.path, &[spec]));
                merge_files(&mut wt.staged, [FileEntry { path: spec.to_string(), kind: FileKind::Deleted }]);
            }
        }
        "mv" => {
            if let [_, to] = specs.as_slice() {
                merge_files(&mut wt.staged, [FileEntry { path: to.to_string(), kind: FileKind::Renamed }]);
            }
        }
        _ => {
            let staged = flag(args, &["-S", "--staged"]);
            let worktree = flag(args, &["-W", "--worktree"]) || !staged;
            restore_paths(p, &specs, staged, worktree);
        }
    }
}
