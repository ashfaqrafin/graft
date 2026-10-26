//! `graft watch`: a full-screen live view. Whenever refs, the index or the stash
//! change (from any tool), the graph animates from the old state to the new one.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use notify::{RecursiveMode, Watcher};

use crate::anim::player::{self, Outcome};
use crate::anim::scenes;
use crate::app::{self, Settings};
use crate::git::{HeadState, RefKind, Repo, Snapshot};
use crate::model::{Hints, Transition};
use crate::op::{Op, OpInfo};
use crate::render::buffer::Buffer;
use crate::render::color::Theme;
use crate::render::draw::Scratch;
use crate::render::term::{self, Term};

const DEBOUNCE: Duration = Duration::from_millis(150);

fn relevant(path: &Path) -> bool {
    let s = path.to_string_lossy();
    !(s.contains("/objects/")
        || s.ends_with(".lock")
        || s.ends_with("COMMIT_EDITMSG")
        || s.contains("/hooks/")
        || s.ends_with("FETCH_HEAD")
        || s.ends_with("/graft/events.old"))
}

pub fn run(settings: &Settings) -> Result<i32> {
    let repo = Repo::discover(".").context("not a git repository")?;
    if !term::is_interactive() {
        bail!("graft watch needs an interactive terminal");
    }
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && ev.paths.iter().any(|p| relevant(p)) {
                let _ = tx.send(());
            }
    })?;
    watcher.watch(&repo.common_dir, RecursiveMode::Recursive)?;
    if repo.git_dir != repo.common_dir {
        watcher.watch(&repo.git_dir, RecursiveMode::Recursive)?;
    }
    let cwd = std::env::current_dir()?;
    let title = format!(" graft watch · {} · q to quit ", repo.display_name());

    term::install_panic_hook();
    term::enter_raw()?;
    let bg = term::query_background();
    let theme = settings.theme(bg);
    let mut term = Term::alt(theme.depth)?;

    let mut prev = Snapshot::take(&repo, true);
    draw_still(&mut term, &repo, &prev, &theme, settings, &title)?;
    drop(repo);

    let mut pending: Option<Instant> = None;
    loop {
        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let quit = matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                        || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                    if quit {
                        break;
                    }
                    if k.code == KeyCode::Char('r') {
                        pending = Some(Instant::now() - DEBOUNCE);
                    }
                }
                Event::Resize(w, h) => {
                    term.resize(w, h);
                    let repo = Repo::discover(&cwd)?;
                    draw_still(&mut term, &repo, &prev, &theme, settings, &title)?;
                }
                _ => {}
            }
        }
        while rx.try_recv().is_ok() {
            pending = Some(Instant::now());
        }
        let Some(at) = pending else { continue };
        if at.elapsed() < DEBOUNCE {
            continue;
        }
        pending = None;
        let repo = Repo::discover(&cwd)?;
        let next = Snapshot::take(&repo, true);
        let op = infer_op(&repo, &prev, &next);
        let mut info = match repo.last_reflog_message() {
            Some(msg) if prev.head != next.head && Op::from_reflog(&msg) == op => OpInfo::from_reflog(&msg),
            _ => OpInfo::new(op, Vec::new()),
        };
        info.exit_code = 0;
        let mut hints = app::hints_for(&info, &repo, &next);
        hints.locals = 6;
        let tr = Transition::build(&repo, prev.clone(), next.clone(), &hints);
        prev = next;
        if tr.is_noop() {
            continue;
        }
        let vp = settings.viewport(term.w.saturating_sub(1), term.h);
        let (scene, tl) = scenes::build(&tr, &info, &theme, vp);
        let rebuild = |w: u16, h: u16| scenes::build(&tr, &info, &theme, settings.viewport(w.saturating_sub(1), h));
        let opts =
            player::PlayOpts { speed: settings.speed, hint: false, header: Some(&title), fill: true, rebuild: Some(&rebuild) };
        let outcome = player::play(&scene, &tl, &theme, &mut term, opts)?;
        if outcome == Outcome::Quit {
            break;
        }
    }
    drop(term);
    Ok(0)
}

fn draw_still(term: &mut Term, repo: &Repo, snap: &Snapshot, theme: &Theme, settings: &Settings, title: &str) -> Result<()> {
    let tr = Transition::still(repo, snap.clone(), &Hints { context: 3, locals: 6, ..Default::default() });
    let vp = settings.viewport(term.w.saturating_sub(1), term.h);
    let (scene, tl) = scenes::still(&tr, theme, vp, false);
    let mut buf = Buffer::new(term.w, term.h);
    player::frame(&scene, &tl, theme, tl.duration, &mut buf, &mut Default::default(), &mut Scratch::default());
    player::header(&mut buf, theme, title);
    term.present(&buf)?;
    Ok(())
}

/// Best guess at what just happened, from the HEAD reflog and the shape of the change.
fn infer_op(repo: &Repo, prev: &Snapshot, next: &Snapshot) -> Op {
    if prev.stash != next.stash {
        return Op::Stash;
    }
    if prev.head != next.head {
        let moved_branch = matches!((&prev.head, &next.head),
            (HeadState::Attached { branch: a, .. }, HeadState::Attached { branch: b, .. }) if a != b);
        if moved_branch {
            return Op::Checkout;
        }
        return repo.last_reflog_message().map(|m| Op::from_reflog(&m)).unwrap_or(Op::Other);
    }
    let changed = |kind: RefKind| {
        next.refs.iter().filter(|r| r.kind == kind).any(|r| prev.find_ref(&r.name).map(|p| p.target) != Some(r.target))
            || prev.refs.iter().filter(|r| r.kind == kind).any(|r| next.find_ref(&r.name).is_none())
    };
    if changed(RefKind::Remote) {
        // A push makes a remote-tracking ref point at something we already had locally.
        let pushed = next.refs.iter().filter(|r| r.kind == RefKind::Remote).any(|r| {
            prev.find_ref(&r.name).map(|p| p.target) != Some(r.target)
                && prev.refs.iter().any(|l| l.kind == RefKind::Local && l.target == r.target)
        });
        return if pushed { Op::Push } else { Op::Fetch };
    }
    if changed(RefKind::Local) {
        return Op::Branch;
    }
    if changed(RefKind::Tag) {
        return Op::Tag;
    }
    if prev.worktree != next.worktree {
        return Op::Stage;
    }
    Op::Other
}
