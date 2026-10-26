//! `graft live`: type git commands with the predicted result animating
//! underneath as you type. Enter runs the command for real.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::anim::player;
use crate::anim::scene::Scene;
use crate::anim::scenes;
use crate::anim::timeline::{SceneState, Timeline};
use crate::app::{self, RunMode, Settings};
use crate::git::{Repo, Snapshot};
use crate::gitargs;
use crate::model::{Hints, Transition};
use crate::render::buffer::{BOLD, Buffer, Style};
use crate::render::color::Theme;
use crate::render::draw::Scratch;
use crate::render::term::{self, Term};

const DEBOUNCE: Duration = Duration::from_millis(120);
const FRAME: Duration = Duration::from_millis(16);

const SUBCOMMANDS: &[&str] = &[
    "add", "branch", "checkout", "cherry-pick", "commit", "fetch", "merge", "pull", "push", "rebase", "reset", "restore",
    "revert", "rm", "stash", "status", "switch", "tag",
];

struct Request {
    seq: u64,
    line: String,
    w: u16,
    h: u16,
}

struct Ready {
    seq: u64,
    scene: Scene,
    tl: Timeline,
}

/// Builds previews off the UI thread so typing never stutters.
fn predictor(cwd: PathBuf, theme: Theme, settings: Settings, rx: mpsc::Receiver<Request>, tx: mpsc::Sender<Ready>) {
    while let Ok(mut req) = rx.recv() {
        // Only the newest request matters.
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        let vp = settings.viewport(req.w, req.h);
        let built = (|| -> Option<(Scene, Timeline)> {
            let mut words = gitargs::split_words(&req.line);
            if words.first().is_some_and(|w| w == "git") {
                words.remove(0);
            }
            let cmd = gitargs::parse(words.iter().map(OsString::from).collect());
            let dir = if cmd.dir.is_absolute() { cmd.dir.clone() } else { cwd.join(&cmd.dir) };
            let repo = Repo::discover(&dir).ok()?;
            let words = cmd.command();
            if words.is_empty() {
                let tr = Transition::still(&repo, Snapshot::take(&repo, false), &Hints { context: 3, locals: 6, ..Default::default() });
                let (mut scene, tl) = scenes::still(&tr, &theme, vp, false);
                if let Some(c) = scene.items.iter_mut().rev().find(|i| i.kind == crate::anim::scene::ItemKind::Caption) {
                    c.text = "type a git command to see what it would do".into();
                }
                return Some((scene, tl));
            }
            let prediction = crate::preview::predict(&repo, &words, false).ok()?;
            let tr = prediction.transition(&repo);
            Some(scenes::build(&tr, &prediction.info, &theme, vp))
        })();
        if let Some((scene, tl)) = built
            && tx.send(Ready { seq: req.seq, scene, tl }).is_err() {
                return;
            }
    }
}

struct Editor {
    text: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    matches: Vec<String>,
}

impl Editor {
    fn line(&self) -> String {
        self.text.iter().collect()
    }

    fn set(&mut self, s: &str) {
        self.text = s.chars().collect();
        self.cursor = self.text.len();
    }

    /// Start of the word under the cursor.
    fn word_start(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && !self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn complete(&mut self, refs: &[String]) {
        let start = self.word_start();
        let word: String = self.text[start..self.cursor].iter().collect();
        let before: String = self.text[..start].iter().collect();
        let first = before.split_whitespace().filter(|w| *w != "git").count() == 0;
        let pool: Vec<&str> = if first { SUBCOMMANDS.to_vec() } else { refs.iter().map(String::as_str).collect() };
        let found: Vec<&str> = pool.into_iter().filter(|c| c.starts_with(&word)).collect();
        self.matches = if found.len() > 1 { found.iter().take(8).map(|s| s.to_string()).collect() } else { Vec::new() };
        let insert = match found.as_slice() {
            [] => return,
            [one] => format!("{one} "),
            many => {
                let mut prefix = many[0].to_string();
                for m in many {
                    while !m.starts_with(&prefix) {
                        prefix.pop();
                    }
                }
                prefix
            }
        };
        let rest: Vec<char> = self.text[self.cursor..].to_vec();
        self.text.truncate(start);
        self.text.extend(insert.chars());
        self.cursor = self.text.len();
        self.text.extend(rest);
    }
}

enum Action {
    None,
    Changed,
    Run(String),
    Quit,
}

fn key(ed: &mut Editor, k: KeyEvent, refs: &[String]) -> Action {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if k.code != KeyCode::Tab {
        ed.matches.clear();
    }
    match k.code {
        KeyCode::Esc => return Action::Quit,
        KeyCode::Char('c') if ctrl => {
            if ed.text.is_empty() {
                return Action::Quit;
            }
            ed.set("");
        }
        KeyCode::Char('d') if ctrl => {
            if ed.text.is_empty() {
                return Action::Quit;
            }
            if ed.cursor < ed.text.len() {
                ed.text.remove(ed.cursor);
            }
        }
        KeyCode::Char('a') if ctrl => {
            ed.cursor = 0;
            return Action::None;
        }
        KeyCode::Char('e') if ctrl => {
            ed.cursor = ed.text.len();
            return Action::None;
        }
        KeyCode::Char('u') if ctrl => {
            ed.text.drain(..ed.cursor);
            ed.cursor = 0;
        }
        KeyCode::Char('k') if ctrl => ed.text.truncate(ed.cursor),
        KeyCode::Char('w') if ctrl => {
            let mut start = ed.cursor;
            while start > 0 && ed.text[start - 1].is_whitespace() {
                start -= 1;
            }
            while start > 0 && !ed.text[start - 1].is_whitespace() {
                start -= 1;
            }
            ed.text.drain(start..ed.cursor);
            ed.cursor = start;
        }
        KeyCode::Char(c) if !ctrl => {
            ed.text.insert(ed.cursor, c);
            ed.cursor += 1;
        }
        KeyCode::Backspace if ed.cursor > 0 => {
            ed.cursor -= 1;
            ed.text.remove(ed.cursor);
        }
        KeyCode::Delete if ed.cursor < ed.text.len() => {
            ed.text.remove(ed.cursor);
        }
        KeyCode::Left => {
            ed.cursor = ed.cursor.saturating_sub(1);
            return Action::None;
        }
        KeyCode::Right => {
            ed.cursor = (ed.cursor + 1).min(ed.text.len());
            return Action::None;
        }
        KeyCode::Home => {
            ed.cursor = 0;
            return Action::None;
        }
        KeyCode::End => {
            ed.cursor = ed.text.len();
            return Action::None;
        }
        KeyCode::Up | KeyCode::Down => {
            if ed.history.is_empty() {
                return Action::None;
            }
            let last = ed.history.len() - 1;
            let pos = match (k.code, ed.hist_pos) {
                (KeyCode::Up, None) => Some(last),
                (KeyCode::Up, Some(p)) => Some(p.saturating_sub(1)),
                (_, Some(p)) if p < last => Some(p + 1),
                _ => None,
            };
            ed.hist_pos = pos;
            let line = pos.map(|p| ed.history[p].clone()).unwrap_or_default();
            ed.set(&line);
        }
        KeyCode::Tab => ed.complete(refs),
        KeyCode::Enter => {
            let line = ed.line().trim().to_string();
            if line.is_empty() {
                return Action::None;
            }
            ed.hist_pos = None;
            if ed.history.last() != Some(&line) {
                ed.history.push(line.clone());
            }
            ed.set("");
            return Action::Run(line);
        }
        _ => return Action::None,
    }
    Action::Changed
}

fn history_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("graft/history"))
}

fn ref_names(cwd: &std::path::Path) -> Vec<String> {
    let Ok(repo) = Repo::discover(cwd) else { return Vec::new() };
    let mut names: Vec<String> = repo.refs().into_iter().map(|r| r.short).collect();
    names.push("HEAD".into());
    names.sort();
    names.dedup();
    names
}

pub fn run(settings: &Settings) -> Result<i32> {
    if !term::is_interactive() {
        bail!("graft live needs an interactive terminal");
    }
    let cwd = std::env::current_dir()?;
    Repo::discover(&cwd).context("not a git repository")?;
    term::install_panic_hook();
    term::enter_raw()?;
    let bg = term::query_background();
    let theme = settings.theme(bg);

    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
    {
        let (cwd, theme, settings) = (cwd.clone(), theme.clone(), settings.clone());
        std::thread::spawn(move || predictor(cwd, theme, settings, req_rx, ready_tx));
    }

    let history: Vec<String> = history_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().map(String::from).collect())
        .unwrap_or_default();
    let mut ed = Editor { text: Vec::new(), cursor: 0, history, hist_pos: None, matches: Vec::new() };
    let mut refs = ref_names(&cwd);

    let viewport = || {
        let (w, rows) = term::size();
        (w, rows.saturating_sub(2).clamp(8, 34))
    };
    let (_, height) = viewport();
    let mut term = Term::inline(height, theme.depth)?;
    let mut buf = Buffer::new(term.w, term.h);
    let mut scene_buf = Buffer::new(1, 1);
    let mut st = SceneState::default();
    let mut scratch = Scratch::default();

    let mut seq = 0u64;
    let mut sent = u64::MAX;
    let mut changed_at = Instant::now() - DEBOUNCE;
    let mut shown: Option<(Ready, Instant)> = None;

    loop {
        // Ask for a new prediction once typing pauses.
        if sent != seq && changed_at.elapsed() >= DEBOUNCE {
            let _ = req_tx.send(Request { seq, line: ed.line(), w: term.w.saturating_sub(1), h: term.h.saturating_sub(2) });
            sent = seq;
        }
        while let Ok(ready) = ready_rx.try_recv() {
            if ready.seq == seq {
                shown = Some((ready, Instant::now()));
            }
        }
        let busy = shown.as_ref().is_none_or(|(r, _)| r.seq != seq);

        // Frame: prompt, preview, hints.
        buf.clear();
        let accent = Style::fg(theme.accent).bold();
        let mut x = buf.put_str(1, 0, "git ❯ ", buf.w as i32, accent);
        let avail = (buf.w as i32 - x - 2).max(4) as usize;
        let skip = ed.cursor.saturating_sub(avail);
        for (i, c) in ed.text.iter().enumerate().skip(skip) {
            let style = if i == ed.cursor { Style::fg(theme.bg).on(theme.fg) } else { Style::fg(theme.fg) };
            buf.put(x, 0, *c, style);
            x += 1;
            if x >= buf.w as i32 - 1 {
                break;
            }
        }
        if ed.cursor >= ed.text.len() {
            buf.put(x, 0, ' ', Style::fg(theme.bg).on(theme.fg));
        }
        let mut hint_row = 2;
        if let Some((ready, at)) = &shown {
            let t = (at.elapsed().as_secs_f32() * settings.speed).min(ready.tl.duration);
            if scene_buf.w != ready.scene.w || scene_buf.h != ready.scene.h {
                scene_buf = Buffer::new(ready.scene.w, ready.scene.h);
            }
            player::frame(&ready.scene, &ready.tl, &theme, t, &mut scene_buf, &mut st, &mut scratch);
            hint_row = (scene_buf.h as i32 + 1).min(buf.h as i32 - 1);
            for y in 0..scene_buf.h.min(buf.h.saturating_sub(2)) {
                for x in 0..scene_buf.w.min(buf.w) {
                    let c = scene_buf.cells[y as usize * scene_buf.w as usize + x as usize];
                    buf.cells[(y as usize + 1) * buf.w as usize + x as usize] = c;
                }
            }
        }
        let hint = if !ed.matches.is_empty() {
            ed.matches.join("  ")
        } else {
            let mark = if busy { "⋯ predicting   " } else { "" };
            format!("{mark}enter run · tab complete · ↑↓ history · esc quit")
        };
        let muted = Style::fg(theme.muted).with_attr(if busy { BOLD } else { 0 });
        buf.put_str(1, hint_row, &hint, buf.w as i32 - 1, muted);
        term.present(&buf)?;

        if !event::poll(FRAME)? {
            continue;
        }
        let action = match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => key(&mut ed, k, &refs),
            Event::Paste(p) => {
                for c in p.chars().filter(|c| !c.is_control()) {
                    ed.text.insert(ed.cursor, c);
                    ed.cursor += 1;
                }
                Action::Changed
            }
            Event::Resize(w, h) => {
                term.reflow(w, h)?;
                buf = Buffer::new(term.w, term.h);
                Action::Changed
            }
            _ => Action::None,
        };
        match action {
            Action::None => {}
            Action::Changed => {
                seq += 1;
                changed_at = Instant::now();
            }
            Action::Quit => break,
            Action::Run(line) => {
                term.abandon();
                let mut words = gitargs::split_words(&line);
                if words.first().is_some_and(|w| w == "git") {
                    words.remove(0);
                }
                println!("\x1b[1mgit ❯\x1b[0m {}", line);
                app::git(words.into_iter().map(OsString::from).collect(), settings, RunMode::Spawn);
                refs = ref_names(&cwd);
                term::enter_raw()?;
                let (_, height) = viewport();
                term = Term::inline(height, theme.depth)?;
                buf = Buffer::new(term.w, term.h);
                shown = None;
                seq += 1;
                changed_at = Instant::now() - DEBOUNCE;
            }
        }
    }
    term.abandon();
    if let Some(path) = history_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let keep = ed.history.len().saturating_sub(500);
        let _ = std::fs::write(path, ed.history[keep..].join("\n") + "\n");
    }
    Ok(0)
}
