//! The frame loop: a fixed 60 fps clock, input polled with a timeout equal to
//! the time until the next frame, any key skips to the end.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::scene::Scene;
use super::timeline::{SceneState, Timeline};
use crate::render::buffer::{Buffer, Style};
use crate::render::color::Theme;
use crate::render::draw::{Scratch, draw};
use crate::render::term::Term;

const FRAME: Duration = Duration::from_micros(16_667);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Skipped,
    /// The user asked to quit (q / Esc / Ctrl-C), only meaningful in watch mode.
    Quit,
}

/// Render the frame at time `t` into `buf`.
pub fn frame(scene: &Scene, tl: &Timeline, theme: &Theme, t: f32, buf: &mut Buffer, st: &mut SceneState, sc: &mut Scratch) {
    tl.sample_into(t, st);
    draw(scene, st, theme, buf, sc);
}

/// Rebuilds the scene for a new terminal size (width, viewport height).
pub type Rebuild<'a> = &'a dyn Fn(u16, u16) -> (Scene, Timeline);

#[derive(Clone, Copy)]
pub struct PlayOpts<'a> {
    pub speed: f32,
    /// Show "any key to skip" while playing.
    pub hint: bool,
    /// Title drawn on the first row (watch mode).
    pub header: Option<&'a str>,
    /// Size the frame to the whole terminal (alternate screen).
    pub fill: bool,
    /// Called when the terminal is resized mid-animation.
    pub rebuild: Option<Rebuild<'a>>,
}

/// Draw a title bar on the first row.
pub fn header(buf: &mut Buffer, theme: &Theme, title: &str) {
    let x = buf.w as i32 - crate::render::buffer::text_width(title) as i32 - 1;
    buf.put_str(x.max(0), 0, title, buf.w as i32, Style::fg(theme.pill_fg).on(theme.accent).bold());
}

pub fn play(scene: &Scene, tl: &Timeline, theme: &Theme, term: &mut Term, opts: PlayOpts) -> io::Result<Outcome> {
    let PlayOpts { speed, hint, header: title, fill, rebuild } = opts;
    let sized = |scene: &Scene, term: &Term| {
        if fill { Buffer::new(term.w, term.h) } else { Buffer::new(scene.w.min(term.w), scene.h.min(term.h)) }
    };
    let mut buf = sized(scene, term);
    let mut st = SceneState::default();
    let mut sc = Scratch::default();
    let speed = if speed.is_finite() && speed > 0.0 { speed } else { 1.0 };
    let start = Instant::now();
    let mut next = start;
    let mut outcome = Outcome::Finished;
    let mut rebuilt: Option<(Scene, Timeline)> = None;
    loop {
        let (scene, tl) = match &rebuilt {
            Some((s, t)) => (s, t),
            None => (scene, tl),
        };
        let t = start.elapsed().as_secs_f32() * speed;
        let done = t >= tl.duration || outcome != Outcome::Finished;
        let t = if done { tl.duration } else { t };
        frame(scene, tl, theme, t, &mut buf, &mut st, &mut sc);
        if let Some(title) = title {
            header(&mut buf, theme, title);
        }
        if hint && !done {
            let text = "any key to skip";
            let x = buf.w as i32 - text.len() as i32 - 1;
            buf.put_str(x, scene.h.min(buf.h) as i32 - 1, text, buf.w as i32, Style::fg(theme.fade(theme.muted, 0.7)));
        }
        term.present(&buf)?;
        if done {
            return Ok(outcome);
        }
        next += FRAME;
        let now = Instant::now();
        if next < now {
            next = now; // fell behind; do not try to catch up with a burst of frames
        }
        if !event::poll(next.saturating_duration_since(now))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                let quit = matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                outcome = if quit { Outcome::Quit } else { Outcome::Skipped };
            }
            Event::Resize(w, h) => {
                term.reflow(w, h)?;
                if let Some(rebuild) = rebuild {
                    let fresh = rebuild(term.w, term.h);
                    buf = sized(&fresh.0, term);
                    rebuilt = Some(fresh);
                } else {
                    outcome = Outcome::Skipped;
                }
            }
            _ => {}
        }
    }
}
