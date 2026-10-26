//! Scenario tests: script a fixture repository with the real git, take snapshots
//! around an operation, and check the diff, the layout and sampled frames.
//! Run with `GRAFT_FRAMES=1 cargo test -- --nocapture` to print the frames.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::anim::scenes::{self, Viewport};
use crate::anim::timeline::SceneState;
use crate::app;
use crate::git::{Repo, Snapshot, runner};
use crate::model::{Change, Transition};
use crate::op::{Op, OpInfo};
use crate::render::buffer::Buffer;
use crate::render::color::{Depth, Theme};
use crate::render::draw::{Scratch, draw};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Fixture {
    pub dir: PathBuf,
    /// Advances the fake clock so every commit gets a distinct, reproducible date.
    clock: std::cell::Cell<u64>,
}

impl Fixture {
    pub fn new() -> Fixture {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("graft-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = Fixture { dir, clock: std::cell::Cell::new(1_700_000_000) };
        f.git(&["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "T"),
            ("user.email", "t@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            f.git(&["config", k, v]);
        }
        f
    }

    /// Run git with a fixed clock and no user/system config, so commit ids are
    /// the same on every run and every machine (golden frames depend on it).
    pub fn git(&self, args: &[&str]) -> i32 {
        let t = self.clock.get() + 60;
        self.clock.set(t);
        let date = format!("{t} +0000");
        std::process::Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env("GIT_EDITOR", "true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(runner::exit_code)
            .unwrap_or(127)
    }

    pub fn commit(&self, file: &str, text: &str, msg: &str) {
        std::fs::write(self.dir.join(file), text).unwrap();
        self.git(&["add", file]);
        assert_eq!(self.git(&["commit", "-q", "-m", msg]), 0, "commit {msg}");
    }

    pub fn repo(&self) -> Repo {
        Repo::discover(&self.dir).unwrap()
    }

    pub fn snap(&self) -> Snapshot {
        Snapshot::take(&self.repo(), true)
    }

    /// Run `args` between two snapshots and build the transition.
    pub fn transition(&self, args: &[&str]) -> (Transition, OpInfo) {
        let before = self.snap();
        let code = self.git(args);
        let after = self.snap();
        let mut info = OpInfo::new(Op::from_subcommand(args[0]).unwrap_or(Op::Other), args.iter().map(|s| s.to_string()).collect());
        info.exit_code = code;
        let repo = self.repo();
        let hints = app::hints_for(&info, &repo, &after);
        (Transition::build(&repo, before, after, &hints), info)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn vp(horizontal: bool) -> Viewport {
    Viewport { w: 100, max_h: 30, horizontal }
}

fn theme() -> Theme {
    Theme::dark(Depth::True, true)
}

/// Render frames at evenly spaced times as plain text.
pub fn frames(tr: &Transition, info: &OpInfo, steps: usize) -> Vec<String> {
    frames_in(tr, info, steps, false)
}

pub fn frames_in(tr: &Transition, info: &OpInfo, steps: usize, horizontal: bool) -> Vec<String> {
    let theme = theme();
    let (scene, tl) = scenes::build(tr, info, &theme, vp(horizontal));
    let mut buf = Buffer::new(scene.w, scene.h);
    let mut st = SceneState::default();
    let mut sc = Scratch::default();
    (0..=steps)
        .map(|i| {
            let t = tl.duration * i as f32 / steps as f32;
            tl.sample_into(t, &mut st);
            draw(&scene, &st, &theme, &mut buf, &mut sc);
            let text = buf.to_plain();
            if std::env::var_os("GRAFT_FRAMES").is_some() {
                println!("--- {} t={t:.2}/{:.2}\n{text}", info.args.join(" "), tl.duration);
            }
            text
        })
        .collect()
}

fn base_repo() -> Fixture {
    let f = Fixture::new();
    f.commit("a.txt", "a\n", "Initial commit");
    f.commit("b.txt", "b\n", "Add b");
    f.commit("c.txt", "c\n", "Add c");
    f
}

fn diverged() -> Fixture {
    let f = base_repo();
    f.git(&["switch", "-q", "-c", "feature"]);
    f.commit("f1.txt", "1\n", "Feature one");
    f.commit("f2.txt", "2\n", "Feature two");
    f.git(&["switch", "-q", "main"]);
    f.commit("m1.txt", "m\n", "Main moves on");
    f
}

fn count(tr: &Transition, pred: impl Fn(&Change) -> bool) -> usize {
    tr.changes.iter().filter(|c| pred(c)).count()
}

#[test]
fn commit_spawns_one_node_and_moves_labels() {
    let f = base_repo();
    std::fs::write(f.dir.join("d.txt"), "d\n").unwrap();
    f.git(&["add", "d.txt"]);
    let (tr, info) = f.transition(&["commit", "-q", "-m", "Add d"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::Spawn(_))), 1);
    assert_eq!(count(&tr, |c| matches!(c, Change::RefMoved { .. })), 1);
    assert_eq!(count(&tr, |c| matches!(c, Change::HeadMoved { .. })), 1);
    let fr = frames(&tr, &info, 6);
    assert!(!fr[0].contains("Add d"), "new commit must not exist in the first frame");
    assert!(fr.last().unwrap().contains("Add d"));
    assert!(fr.last().unwrap().contains("HEAD"));
}

#[test]
fn fast_forward_moves_label_without_spawning() {
    let f = base_repo();
    f.git(&["switch", "-q", "-c", "feature"]);
    f.commit("f.txt", "f\n", "Feature");
    f.git(&["switch", "-q", "main"]);
    let (tr, info) = f.transition(&["merge", "-q", "feature"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::Spawn(_))), 0);
    assert!(tr.changes.iter().any(|c| matches!(c, Change::RefMoved { short, .. } if short == "main")));
    let fr = frames(&tr, &info, 6);
    assert!(fr.last().unwrap().contains("fast-forward"));
}

#[test]
fn true_merge_spawns_merge_node() {
    let f = diverged();
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "feature"]);
    let spawned: Vec<_> = tr.changes.iter().filter_map(|c| if let Change::Spawn(n) = c { Some(*n) } else { None }).collect();
    assert_eq!(spawned.len(), 1);
    assert_eq!(tr.graph.nodes[spawned[0]].parents.len(), 2);
    let fr = frames(&tr, &info, 8);
    let last = fr.last().unwrap();
    assert!(last.contains("Merge branch 'feature'"));
    assert!(last.contains('╮') || last.contains('╯'), "merge edges drawn:\n{last}");
}

#[test]
fn conflicting_merge_shows_phantom() {
    let f = base_repo();
    f.git(&["switch", "-q", "-c", "feature"]);
    f.commit("a.txt", "feature\n", "Change a on feature");
    f.git(&["switch", "-q", "main"]);
    f.commit("a.txt", "main\n", "Change a on main");
    let (tr, info) = f.transition(&["merge", "--no-edit", "feature"]);
    assert_ne!(info.exit_code, 0);
    assert!(tr.phantom.is_some());
    let fr = frames(&tr, &info, 6);
    let last = fr.last().unwrap();
    assert!(last.contains("conflict"), "{last}");
    assert!(last.contains("a.txt"), "conflicted file listed:\n{last}");
}

#[test]
fn rebase_reparents_commits() {
    let f = diverged();
    f.git(&["switch", "-q", "feature"]);
    let (tr, info) = f.transition(&["rebase", "-q", "main"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::Reparent { .. })), 2);
    assert_eq!(count(&tr, |c| matches!(c, Change::Vanish(_))), 2);
    let fr = frames(&tr, &info, 8);
    let last = fr.last().unwrap();
    assert_eq!(last.matches("Feature one").count(), 1, "old copies are gone at the end:\n{last}");
}

#[test]
fn reset_keeps_orphans_dimmed() {
    let f = base_repo();
    let (tr, info) = f.transition(&["reset", "-q", "--hard", "HEAD~2"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::Vanish(_))), 2);
    let fr = frames(&tr, &info, 6);
    assert!(fr.last().unwrap().contains("orphaned"));
}

#[test]
fn stash_push_and_pop() {
    let f = base_repo();
    std::fs::write(f.dir.join("a.txt"), "dirty\n").unwrap();
    let (tr, info) = f.transition(&["stash", "-q"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::StashPushed(_))), 1);
    let fr = frames(&tr, &info, 6);
    assert!(fr.last().unwrap().contains("stash"));
    let (tr, info) = f.transition(&["stash", "pop", "-q"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::StashPopped(_))), 1);
    frames(&tr, &info, 6);
}

#[test]
fn cherry_pick_flies_from_source() {
    let f = diverged();
    let (tr, info) = f.transition(&["cherry-pick", "feature"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::Reparent { .. })), 1);
    frames(&tr, &info, 6);
}

#[test]
fn checkout_moves_head() {
    let f = diverged();
    let (tr, info) = f.transition(&["switch", "-q", "feature"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::HeadMoved { .. })), 1);
    let fr = frames(&tr, &info, 4);
    assert!(fr.last().unwrap().contains("switched to feature"));
}

#[test]
fn layout_has_no_shared_cells() {
    let f = diverged();
    f.git(&["merge", "-q", "--no-edit", "feature"]);
    let tr = Transition::still(&f.repo(), f.snap(), &Default::default());
    let theme = theme();
    let (scene, _) = scenes::still(&tr, &theme, Viewport { w: 100, max_h: 40, horizontal: false }, false);
    let mut seen = std::collections::HashSet::new();
    for n in &scene.nodes {
        let p = n.pos[2];
        assert!(seen.insert((p.0 as i32, p.1 as i32)), "two nodes share a cell");
    }
}

#[test]
fn sample_is_pure() {
    let f = diverged();
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "feature"]);
    let theme = theme();
    let (_, tl) = scenes::build(&tr, &info, &theme, vp(false));
    let a = format!("{:?}", tl.sample(tl.duration * 0.4).nodes);
    let _ = tl.sample(tl.duration * 0.9);
    let b = format!("{:?}", tl.sample(tl.duration * 0.4).nodes);
    assert_eq!(a, b);
}


#[test]
fn frame_time_budget() {
    let f = diverged();
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "feature"]);
    let theme = theme();
    let (scene, tl) = scenes::build(&tr, &info, &theme, Viewport { w: 160, max_h: 40, horizontal: false });
    let mut buf = Buffer::new(scene.w, scene.h);
    let mut st = SceneState::default();
    let mut sc = Scratch::default();
    let n = 300;
    let start = std::time::Instant::now();
    for i in 0..n {
        tl.sample_into(tl.duration * i as f32 / n as f32, &mut st);
        draw(&scene, &st, &theme, &mut buf, &mut sc);
    }
    let per = start.elapsed() / n;
    if std::env::var_os("GRAFT_FRAMES").is_some() {
        println!("frame: {per:?}");
    }
    assert!(per < std::time::Duration::from_millis(16), "frame took {per:?}");
}

#[test]
fn first_commit_in_empty_repo() {
    let f = Fixture::new();
    std::fs::write(f.dir.join("a.txt"), "a\n").unwrap();
    f.git(&["add", "a.txt"]);
    let (tr, info) = f.transition(&["commit", "-q", "-m", "Root"]);
    assert_eq!(count(&tr, |c| matches!(c, Change::Spawn(_))), 1);
    let fr = frames(&tr, &info, 4);
    assert!(fr.last().unwrap().contains("Root"));
}

#[test]
fn octopus_merge() {
    let f = base_repo();
    for b in ["x", "y", "z"] {
        f.git(&["switch", "-q", "-c", b, "main"]);
        f.commit(&format!("{b}.txt"), b, &format!("Work on {b}"));
    }
    f.git(&["switch", "-q", "main"]);
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "x", "y", "z"]);
    assert_eq!(info.exit_code, 0);
    let merge = tr.graph.nodes.iter().find(|n| n.parents.len() >= 3);
    assert!(merge.is_some(), "octopus merge node");
    let last = frames(&tr, &info, 6).pop().unwrap();
    for b in ["Work on x", "Work on y", "Work on z"] {
        assert!(last.contains(b), "{b} missing:\n{last}");
    }
}

#[test]
fn detached_checkout() {
    let f = base_repo();
    let (tr, info) = f.transition(&["checkout", "-q", "HEAD~1"]);
    let last = frames(&tr, &info, 4).pop().unwrap();
    assert!(last.contains("detached HEAD"), "{last}");
}

#[test]
fn horizontal_merge_and_rebase() {
    let f = diverged();
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "feature"]);
    let last = frames_in(&tr, &info, 6, true).pop().unwrap();
    assert!(last.contains("main") && last.contains("feature"), "{last}");
    assert!(last.contains('╯') || last.contains('╮') || last.contains('╰') || last.contains('╭'), "{last}");
    f.git(&["reset", "-q", "--hard", "HEAD~1"]);
    f.git(&["switch", "-q", "feature"]);
    let (tr, info) = f.transition(&["rebase", "-q", "main"]);
    frames_in(&tr, &info, 6, true);
}

/// Golden frames at t = 0, 1/4, 1/2, 3/4 and 1 of each scene. Review changes with
/// `cargo insta review` (or `INSTA_UPDATE=always cargo test` to accept them).
mod golden {
    use super::*;

    fn golden(tr: &Transition, info: &OpInfo, horizontal: bool) -> String {
        frames_in(tr, info, 4, horizontal)
            .iter()
            .enumerate()
            .map(|(i, f)| format!("── t = {}/4 ──\n{}", i, f.trim_end()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn commit() {
        let f = base_repo();
        std::fs::write(f.dir.join("d.txt"), "d\n").unwrap();
        f.git(&["add", "d.txt"]);
        let (tr, info) = f.transition(&["commit", "-q", "-m", "Add d"]);
        insta::assert_snapshot!(golden(&tr, &info, false));
    }

    #[test]
    fn merge() {
        let f = diverged();
        let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "feature"]);
        insta::assert_snapshot!(golden(&tr, &info, false));
    }

    #[test]
    fn merge_horizontal() {
        let f = diverged();
        let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "feature"]);
        insta::assert_snapshot!(golden(&tr, &info, true));
    }

    #[test]
    fn fast_forward() {
        let f = base_repo();
        f.git(&["switch", "-q", "-c", "feature"]);
        f.commit("f.txt", "f\n", "Feature");
        f.git(&["switch", "-q", "main"]);
        let (tr, info) = f.transition(&["merge", "-q", "feature"]);
        insta::assert_snapshot!(golden(&tr, &info, false));
    }

    #[test]
    fn conflict() {
        let f = base_repo();
        f.git(&["switch", "-q", "-c", "feature"]);
        f.commit("a.txt", "feature\n", "Change a on feature");
        f.git(&["switch", "-q", "main"]);
        f.commit("a.txt", "main\n", "Change a on main");
        let (tr, info) = f.transition(&["merge", "--no-edit", "feature"]);
        insta::assert_snapshot!(golden(&tr, &info, false));
    }

    #[test]
    fn rebase() {
        let f = diverged();
        f.git(&["switch", "-q", "feature"]);
        let (tr, info) = f.transition(&["rebase", "-q", "main"]);
        insta::assert_snapshot!(golden(&tr, &info, false));
    }

    #[test]
    fn stash() {
        let f = base_repo();
        std::fs::write(f.dir.join("a.txt"), "dirty\n").unwrap();
        let (tr, info) = f.transition(&["stash", "-q"]);
        insta::assert_snapshot!(golden(&tr, &info, false));
    }
}

/// Many branches: forks off forks, a merged sub-branch, then everything at once.
fn tangle() -> Fixture {
    let f = base_repo();
    f.git(&["switch", "-q", "-c", "api", "main"]);
    f.commit("api.txt", "1", "API: routes");
    f.commit("api.txt", "2", "API: handlers");
    f.git(&["switch", "-q", "-c", "auth", "api"]);
    f.commit("auth.txt", "1", "Auth: tokens");
    f.git(&["switch", "-q", "api"]);
    f.commit("api.txt", "3", "API: pagination");
    f.git(&["merge", "-q", "--no-edit", "--no-ff", "auth"]);
    f.git(&["switch", "-q", "-c", "ui", "main~1"]);
    f.commit("ui.txt", "1", "UI: layout");
    f.git(&["switch", "-q", "-c", "docs", "main"]);
    f.commit("docs.txt", "1", "Docs: intro");
    f.git(&["switch", "-q", "-c", "ci", "main~2"]);
    f.commit("ci.txt", "1", "CI: workflow");
    f.git(&["switch", "-q", "main"]);
    f.commit("main.txt", "1", "Main: bump version");
    f
}

#[test]
fn octopus_of_four_branches() {
    let f = tangle();
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "api", "ui", "docs", "ci"]);
    assert_eq!(info.exit_code, 0);
    let merge = tr.graph.nodes.iter().find(|n| n.parents.len() == 5).expect("octopus with five parents");
    assert!(merge.after && !merge.before);
    let last = frames(&tr, &info, 6).pop().unwrap();
    for s in ["API: pagination", "UI: layout", "Docs: intro", "CI: workflow", "Auth: tokens", "api, ui, docs, ci"] {
        assert!(last.contains(s), "{s} missing:\n{last}");
    }
}

#[test]
fn merge_branch_that_contains_a_merge() {
    let f = tangle();
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "api"]);
    let last = frames(&tr, &info, 6).pop().unwrap();
    assert!(last.contains("Auth: tokens") && last.contains("Merge branch 'auth'"), "{last}");
}

#[test]
fn criss_cross_merge() {
    let f = base_repo();
    f.git(&["switch", "-q", "-c", "b", "main"]);
    f.commit("b.txt", "1", "B one");
    f.git(&["switch", "-q", "main"]);
    f.commit("m.txt", "1", "M one");
    f.git(&["merge", "-q", "--no-edit", "b"]);
    f.git(&["switch", "-q", "b"]);
    f.git(&["merge", "-q", "--no-edit", "main~1"]);
    f.commit("b.txt", "2", "B two");
    f.git(&["switch", "-q", "main"]);
    f.commit("m.txt", "2", "M two");
    let (tr, info) = f.transition(&["merge", "-q", "--no-edit", "b"]);
    assert_eq!(info.exit_code, 0);
    frames(&tr, &info, 6);
}

#[test]
fn rebase_branch_with_merges_and_long_ff() {
    let f = tangle();
    f.git(&["switch", "-q", "api"]);
    let (tr, info) = f.transition(&["rebase", "-q", "main"]);
    assert_eq!(info.exit_code, 0);
    assert!(count(&tr, |c| matches!(c, Change::Reparent { .. })) >= 3);
    let last = frames(&tr, &info, 6).pop().unwrap();
    assert!(last.contains("rebased api onto main"), "{last}");
    f.git(&["switch", "-q", "main"]);
    let (tr, info) = f.transition(&["merge", "-q", "api"]);
    assert!(frames(&tr, &info, 4).pop().unwrap().contains("fast-forward"));
}

/// Previews: predict, check the caption, and prove nothing in the repo changed.
mod previews {
    use super::*;
    use crate::anim::scenes::caption;

    fn state(f: &Fixture) -> String {
        let out = std::process::Command::new("git")
            .current_dir(&f.dir)
            .args(["for-each-ref", "--format=%(refname) %(objectname)"])
            .output()
            .unwrap();
        let status = std::process::Command::new("git").current_dir(&f.dir).args(["status", "--porcelain"]).output().unwrap();
        let head = std::fs::read_to_string(f.dir.join(".git/HEAD")).unwrap();
        format!("{}{}{head}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&status.stdout))
    }

    fn predict(f: &Fixture, cmd: &str) -> (String, Transition) {
        let before = state(f);
        let words: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        let repo = f.repo();
        let p = crate::preview::predict(&repo, &words, false).unwrap();
        let tr = p.transition(&repo);
        let (text, _) = caption::caption(&tr, &p.info, true);
        frames(&tr, &p.info, 4);
        assert_eq!(before, state(f), "previewing `{cmd}` changed the repository");
        (text, tr)
    }

    #[test]
    fn rebase_uses_real_rewritten_commits() {
        let f = diverged();
        f.git(&["switch", "-q", "feature"]);
        let (text, tr) = predict(&f, "rebase main");
        assert!(text.contains("preview") && text.contains("2 commits replayed"), "{text}");
        assert_eq!(count(&tr, |c| matches!(c, Change::Reparent { .. })), 2);
    }

    #[test]
    fn rebase_conflict_names_the_stopping_commit() {
        let f = base_repo();
        f.git(&["switch", "-q", "-c", "feature"]);
        f.commit("x.txt", "ok\n", "Harmless");
        f.commit("a.txt", "feature\n", "Touch a");
        f.git(&["switch", "-q", "main"]);
        f.commit("a.txt", "main\n", "Main touches a");
        f.git(&["switch", "-q", "feature"]);
        let (text, tr) = predict(&f, "rebase main");
        assert!(text.contains("stop on a conflict") && text.contains("Touch a"), "{text}");
        assert!(tr.phantom.is_some());
    }

    #[test]
    fn merges_fast_forwards_and_octopus() {
        let f = tangle();
        let (text, _) = predict(&f, "merge api ui docs ci");
        assert!(text.contains("merge commit"), "{text}");
        let (text, _) = predict(&f, "merge --ff-only api");
        assert!(text.contains("--ff-only"), "{text}");
    }

    #[test]
    fn cherry_pick_revert_reset_switch_branch_tag() {
        let f = diverged();
        assert!(predict(&f, "cherry-pick feature").0.contains("cherry-picked"));
        assert!(predict(&f, "revert HEAD").0.contains("revert"));
        assert!(predict(&f, "reset --hard HEAD~1").0.contains("orphaned"));
        assert!(predict(&f, "switch feature").0.contains("switched to feature"));
        assert!(predict(&f, "switch -c topic HEAD~2").0.contains("switched to topic"));
        assert!(predict(&f, "branch -d feature").0.contains("not fully merged"));
        assert!(predict(&f, "tag v1.0").0.contains("created tag v1.0"));
    }

    #[test]
    fn commit_stash_and_staging() {
        let f = base_repo();
        assert!(predict(&f, "commit -m nothing").0.contains("nothing to commit"));
        std::fs::write(f.dir.join("a.txt"), "changed\n").unwrap();
        assert!(predict(&f, "commit -am Change").0.contains("would commit"), "short flags like -am");
        assert!(predict(&f, "add a.txt").0.contains("staged 1 change"));
        assert!(predict(&f, "stash").0.contains("stashed 1 change"));
        assert!(predict(&f, "commit --amend").0.contains("amend"));
    }

    #[test]
    fn push_rejection_is_predicted() {
        let f = base_repo();
        let remote = f.dir.join("remote.git");
        f.git(&["init", "-q", "--bare", remote.to_str().unwrap()]);
        f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        f.git(&["push", "-q", "-u", "origin", "main"]);
        f.commit("d.txt", "d\n", "Local work");
        assert!(predict(&f, "push").0.contains("pushed main"));
        // Pretend the remote moved on: point origin/main at a commit we lack.
        f.git(&["switch", "-q", "-c", "other", "HEAD~1"]);
        f.commit("e.txt", "e\n", "Someone else");
        f.git(&["update-ref", "refs/remotes/origin/main", "other"]);
        f.git(&["switch", "-q", "main"]);
        assert!(predict(&f, "push").0.contains("rejected"));
    }
}
