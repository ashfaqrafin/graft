//! snapshot BEFORE -> run real git -> snapshot AFTER -> diff -> animate.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use crate::anim::player::{self, Outcome};
use crate::anim::scenes::{self, Viewport};
use crate::config::Config;
use crate::gitargs;
use crate::git::{RepoState, Repo, Snapshot, runner};
use crate::model::{Hints, Phantom, Transition};
use crate::op::{Op, OpInfo};
use crate::render::buffer::Buffer;
use crate::render::color::{Depth, Theme};
use crate::render::draw::Scratch;
use crate::render::term::{self, Term};

/// Set when the user pressed q / Esc / Ctrl-C during an animation.
pub static QUIT: AtomicBool = AtomicBool::new(false);

/// Settings given on the command line; `None`/`false` defer to env and config.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub speed: Option<f32>,
    pub no_anim: bool,
    pub ascii: bool,
    pub horizontal: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub speed: f32,
    pub no_anim: bool,
    pub ascii: bool,
    /// Show the final frame without motion.
    pub reduced_motion: bool,
    pub horizontal: bool,
    pub theme: Option<String>,
    pub colors: Vec<(String, Vec<String>)>,
}

impl Settings {
    /// Flags beat environment variables, which beat the config file.
    pub fn new(flags: Flags, config: Config) -> Settings {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let env_bool = |k: &str| env(k).map(|v| v != "0" && v != "false");
        let speed = flags
            .speed
            .or_else(|| env("GRAFT_SPEED").and_then(|s| s.parse().ok()))
            .or(config.speed)
            .filter(|s: &f32| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0);
        let horizontal = flags
            .horizontal
            .or_else(|| env("GRAFT_LAYOUT").map(|l| l == "horizontal"))
            .or(config.layout.as_deref().map(|l| l == "horizontal"))
            .unwrap_or(false);
        Settings {
            speed,
            no_anim: flags.no_anim
                || env_bool("GRAFT_NO_ANIM").unwrap_or(false)
                || env_bool("CI").unwrap_or(false)
                || config.animate == Some(false),
            ascii: flags.ascii || env_bool("GRAFT_ASCII").or(config.ascii).unwrap_or(false),
            reduced_motion: env_bool("GRAFT_REDUCED_MOTION").or(config.reduced_motion).unwrap_or(false),
            horizontal,
            theme: config.theme,
            colors: config.colors,
        }
    }

    pub fn animate(&self) -> bool {
        !self.no_anim && term::is_interactive()
    }

    pub fn theme(&self, bg: Option<crate::render::color::Rgb>) -> Theme {
        Theme::detect(self.ascii, bg, self.theme.as_deref(), &self.colors)
    }

    pub fn viewport(&self, w: u16, max_h: u16) -> Viewport {
        Viewport { w, max_h, horizontal: self.horizontal }
    }
}

/// How git itself is run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Commands graft does not animate replace the graft process (the normal case).
    Exec,
    /// Always run git as a child (`graft live` must survive the command).
    Spawn,
    /// Child process with git's output hidden (the demo).
    Quiet,
}

/// `graft <git args>`: run git for real, then animate what changed. Returns git's exit code.
pub fn wrap(args: Vec<OsString>, settings: &Settings) -> i32 {
    git(args, settings, RunMode::Exec)
}

/// Like `wrap`, with git's own output hidden (the demo).
pub fn wrap_quiet(args: &[&str], settings: &Settings) -> i32 {
    git(args.iter().map(OsString::from).collect(), settings, RunMode::Quiet)
}

pub fn passthrough_now(args: &[OsString]) -> i32 {
    passthrough(args, RunMode::Exec)
}

/// Hand the command to git untouched. In `Exec` mode this replaces the graft
/// process with git, so pagers, signals and exit codes behave exactly as usual.
fn passthrough(args: &[OsString], mode: RunMode) -> i32 {
    if mode == RunMode::Quiet {
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        return runner::quiet(Path::new("."), &refs);
    }
    if mode == RunMode::Spawn {
        return runner::run(args);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new("git").args(args).env("GRAFT_ACTIVE", "1").exec();
        eprintln!("graft: failed to run git: {err}");
        127
    }
    #[cfg(not(unix))]
    runner::run(args)
}

/// The heart of `git` integration: any git command line, global options and
/// aliases included. Commands that change nothing graft draws go straight to git.
pub fn git(mut all: Vec<OsString>, settings: &Settings, mode: RunMode) -> i32 {
    // `--preview` is not a git option anywhere, so it is safe to claim; plain
    // `git merge` has no --dry-run either.
    let preview_flag = all.iter().any(|a| a == "--preview");
    all.retain(|a| a != "--preview");
    let cmd = gitargs::parse(all);
    let Some(sub) = cmd.sub() else { return passthrough(&cmd.all, mode) };
    let quick = Op::from_subcommand(&sub).is_none() && gitargs::is_builtin(&sub);
    if quick && !preview_flag {
        return passthrough(&cmd.all, mode); // e.g. status, log, diff: no alias lookup needed
    }
    let mut words = cmd.command();
    let dry_merge = words.first().is_some_and(|w| w == "merge") && words.iter().any(|w| w == "--dry-run");
    if preview_flag || dry_merge {
        words.retain(|w| w != "--dry-run");
        return match preview(&cmd.dir, &words, settings, true) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("graft: {e:#}");
                128
            }
        };
    }
    let Some(op) = words.first().and_then(|w| Op::from_subcommand(w)) else { return passthrough(&cmd.all, mode) };
    if !settings.animate() || !cmd.trackable {
        return passthrough(&cmd.all, mode);
    }
    let Ok(repo) = Repo::discover(&cmd.dir) else { return passthrough(&cmd.all, mode) };
    let before = Snapshot::take(&repo, op.needs_status());
    drop(repo);

    let code = if mode == RunMode::Quiet { passthrough(&cmd.all, mode) } else { runner::run(&cmd.all) };

    // Reopen so refs, packs and config reflect what git just did.
    let Ok(repo) = Repo::discover(&cmd.dir) else { return code };
    let after = Snapshot::take(&repo, op.needs_status());
    let mut info = OpInfo::new(op, words);
    info.exit_code = code;
    if let Err(e) = animate(&repo, before, after, &info, settings)
        && std::env::var_os("GRAFT_DEBUG").is_some()
    {
        eprintln!("graft: {e:#}");
    }
    code
}

/// Show what `words` (a git command without `git`) would do. Nothing changes.
pub fn preview(dir: &Path, words: &[String], settings: &Settings, network: bool) -> Result<i32> {
    let repo = Repo::discover(dir).context("not a git repository")?;
    let prediction = crate::preview::predict(&repo, words, network)?;
    let tr = prediction.transition(&repo);
    if !settings.animate() {
        let (text, _) = scenes::caption::caption(&tr, &prediction.info, true);
        println!("{text}");
        return Ok(0);
    }
    show(&tr, &prediction.info, settings)?;
    Ok(0)
}

/// `graft preview --line "<what you typed>"`, used by the shell key binding.
pub fn preview_line(line: &str, settings: &Settings) -> Result<i32> {
    let mut words = gitargs::split_words(line);
    if words.first().is_some_and(|w| w == "git" || w == "graft") {
        words.remove(0);
    }
    let cmd = gitargs::parse(words.iter().map(OsString::from).collect());
    let words = cmd.command();
    preview(&cmd.dir, &words, settings, true)
}

pub fn hints_for(info: &OpInfo, repo: &Repo, after: &Snapshot) -> Hints {
    Hints {
        named: info.positionals().iter().map(|s| s.to_string()).collect(),
        locals: 0,
        context: 3,
        phantom: conflict_phantom(repo, after),
    }
}

/// A merge/rebase/cherry-pick that stopped on conflicts gets a phantom commit.
fn conflict_phantom(repo: &Repo, after: &Snapshot) -> Option<Phantom> {
    let head = after.head_oid()?;
    let files = after.conflicted_files().len();
    match after.state {
        RepoState::Merging if !after.merge_heads.is_empty() => {
            let mut parents = vec![head];
            parents.extend(after.merge_heads.iter().copied());
            Some(Phantom { parents, summary: format!("merge stopped · {} conflicted", plural(files, "file")), retarget_head: false })
        }
        RepoState::Rebasing | RepoState::CherryPicking | RepoState::Reverting => {
            let pick = after.pick_head?;
            let summary = repo.commit(pick).map(|c| c.summary).unwrap_or_default();
            Some(Phantom { parents: vec![head], summary: format!("{summary} · conflict"), retarget_head: false })
        }
        _ => None,
    }
}

fn plural(n: usize, w: &str) -> String {
    format!("{n} {w}{}", if n == 1 { "" } else { "s" })
}

pub fn animate(repo: &Repo, before: Snapshot, after: Snapshot, info: &OpInfo, settings: &Settings) -> Result<()> {
    let hints = hints_for(info, repo, &after);
    let tr = Transition::build(repo, before, after, &hints);
    if tr.is_noop() && !(info.op == Op::Push && info.exit_code != 0) {
        return Ok(());
    }
    show(&tr, info, settings)?;
    Ok(())
}

/// Play a transition in an inline viewport under the cursor.
pub fn show(tr: &Transition, info: &OpInfo, settings: &Settings) -> io::Result<Outcome> {
    let (w, rows) = term::size();
    if w < 40 || rows < 8 {
        let theme = settings.theme(None);
        let (text, _) = scenes::caption::caption(tr, info, theme.unicode);
        println!("{text}");
        return Ok(Outcome::Finished);
    }
    term::install_panic_hook();
    term::enter_raw()?;
    let bg = term::query_background();
    let theme = settings.theme(bg);
    let vp = settings.viewport(w - 1, rows.saturating_sub(2).min(30));
    let (scene, tl) = scenes::build(tr, info, &theme, vp);
    let mut term = Term::inline(scene.h, theme.depth)?;
    let rebuild = |w: u16, h: u16| scenes::build(tr, info, &theme, settings.viewport(w.saturating_sub(1), h));
    let opts = player::PlayOpts {
        speed: if settings.reduced_motion { 1e9 } else { settings.speed },
        hint: !settings.reduced_motion,
        header: None,
        fill: false,
        rebuild: Some(&rebuild),
    };
    let outcome = player::play(&scene, &tl, &theme, &mut term, opts)?;
    term.finish();
    if outcome == Outcome::Quit {
        QUIT.store(true, Ordering::SeqCst);
    }
    Ok(outcome)
}

/// `graft log`: print the graph as a still frame.
pub fn log(settings: &Settings, all: bool, context: usize) -> Result<i32> {
    let repo = Repo::discover(".").context("not a git repository")?;
    let snap = Snapshot::take(&repo, false);
    let hints = Hints { locals: if all { usize::MAX } else { 6 }, context, ..Default::default() };
    let tr = Transition::still(&repo, snap, &hints);
    if tr.graph.nodes.is_empty() {
        println!("(no commits yet)");
        return Ok(0);
    }
    let tty = io::stdout().is_terminal();
    let mut theme = settings.theme(None);
    if !tty {
        theme.depth = Depth::Mono;
    }
    let (w, rows) = if tty { term::size() } else { (120, 400) };
    let vp = settings.viewport(w.saturating_sub(1).max(40), rows.max(12));
    let (scene, tl) = scenes::still(&tr, &theme, vp, false);
    let mut buf = Buffer::new(scene.w, scene.h);
    let mut st = Default::default();
    player::frame(&scene, &tl, &theme, tl.duration, &mut buf, &mut st, &mut Scratch::default());
    let text = if !tty && std::env::var_os("GRAFT_FORCE_COLOR").is_none() { buf.to_plain() } else { buf.to_ansi(theme.depth) };
    let mut out = io::stdout().lock();
    // Drop the blank caption rows at the bottom.
    let trimmed = text.trim_start_matches('\n').trim_end_matches('\n');
    writeln!(out, "{trimmed}")?;
    Ok(0)
}
