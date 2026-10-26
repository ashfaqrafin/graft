//! Raw mode, the inline viewport, synchronized output and cleanup.
//!
//! Wrap mode draws into a fixed block of rows below the prompt so scrollback is
//! kept and the final graph stays on screen. `graft watch` uses the alternate screen.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::terminal;

use super::buffer::{Buffer, Cell, Style, WIDE_TAIL, push_sgr};
use super::color::{Depth, Rgb};

static RAW: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);
static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Put the terminal back the way we found it. Safe to call any number of times.
pub fn restore() {
    let mut out = io::stdout();
    if ALT.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[?1049l");
    }
    if HIDDEN.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[0m\x1b[?25h");
    }
    let _ = out.flush();
    if RAW.swap(false, Ordering::SeqCst) {
        let _ = terminal::disable_raw_mode();
    }
}

pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
    }));
}

pub fn enter_raw() -> io::Result<()> {
    if !RAW.swap(true, Ordering::SeqCst) {
        terminal::enable_raw_mode()?;
    }
    Ok(())
}

pub fn is_interactive() -> bool {
    io::stdout().is_terminal() && io::stdin().is_terminal()
}

pub fn size() -> (u16, u16) {
    terminal::size().unwrap_or((80, 24))
}

pub struct Term {
    pub w: u16,
    pub h: u16,
    alt: bool,
    /// Cursor row relative to the viewport (inline mode tracks it to use relative moves).
    cy: u16,
    front: Option<Buffer>,
    depth: Depth,
    out: String,
    finished: bool,
}

impl Term {
    /// Reserve `h` rows starting at the cursor's line.
    pub fn inline(h: u16, depth: Depth) -> io::Result<Term> {
        enter_raw()?;
        let (w, rows) = size();
        let h = h.min(rows.saturating_sub(1)).max(1);
        let mut out = io::stdout().lock();
        HIDDEN.store(true, Ordering::SeqCst);
        out.write_all(b"\x1b[?25l\r")?;
        for _ in 1..h {
            out.write_all(b"\n")?;
        }
        out.flush()?;
        Ok(Term { w, h, alt: false, cy: h - 1, front: None, depth, out: String::new(), finished: false })
    }

    pub fn alt(depth: Depth) -> io::Result<Term> {
        enter_raw()?;
        let (w, h) = size();
        ALT.store(true, Ordering::SeqCst);
        HIDDEN.store(true, Ordering::SeqCst);
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J")?;
        out.flush()?;
        Ok(Term { w, h, alt: true, cy: 0, front: None, depth, out: String::new(), finished: false })
    }

    /// Alternate-screen resize: forget the front buffer so the next frame repaints.
    pub fn resize(&mut self, w: u16, h: u16) {
        self.w = w;
        self.h = h;
        self.front = None;
        let _ = io::stdout().write_all(b"\x1b[2J");
    }

    fn move_to(&mut self, x: u16, y: u16) {
        use std::fmt::Write as _;
        if self.alt {
            let _ = write!(self.out, "\x1b[{};{}H", y + 1, x + 1);
        } else {
            if y < self.cy {
                let _ = write!(self.out, "\x1b[{}A", self.cy - y);
            } else if y > self.cy {
                let _ = write!(self.out, "\x1b[{}B", y - self.cy);
            }
            let _ = write!(self.out, "\x1b[{}G", x + 1);
            self.cy = y;
        }
    }

    /// Write the cells of `back` that changed since the last frame.
    pub fn present(&mut self, back: &Buffer) -> io::Result<()> {
        self.out.clear();
        self.out.push_str("\x1b[?2026h");
        let full = self.front.as_ref().is_none_or(|f| f.w != back.w || f.h != back.h);
        let h = back.h.min(self.h);
        let w = back.w.min(self.w);
        let mut style: Option<Style> = None;
        for y in 0..h {
            if full {
                self.move_to(0, y);
                self.out.push_str("\x1b[0m\x1b[2K");
                style = None;
            }
            let mut cx: Option<u16> = None;
            let mut x = 0u16;
            while x < w {
                let i = y as usize * back.w as usize + x as usize;
                let cell = back.cells[i];
                let old = if full { Cell::BLANK } else { self.front.as_ref().unwrap().cells[i] };
                if cell == old || cell.ch == WIDE_TAIL {
                    x += 1;
                    continue;
                }
                if cx != Some(x) {
                    self.move_to(x, y);
                }
                let st = Style { fg: cell.fg, bg: cell.bg, attr: cell.attr };
                if style != Some(st) {
                    push_sgr(&mut self.out, st, self.depth);
                    style = Some(st);
                }
                self.out.push(cell.ch);
                let wide = back.cells.get(i + 1).is_some_and(|c| c.ch == WIDE_TAIL) && x + 1 < w;
                x += if wide { 2 } else { 1 };
                cx = Some(x);
            }
        }
        // Park the cursor at column 0: if the terminal later reflows our rows on a
        // resize, the cursor stays on the first segment of its row.
        if !self.alt {
            self.out.push('\r');
        }
        self.out.push_str("\x1b[0m\x1b[?2026l");
        let mut stdout = io::stdout().lock();
        stdout.write_all(self.out.as_bytes())?;
        stdout.flush()?;
        match &mut self.front {
            Some(f) if !full => f.clone_from(back),
            _ => self.front = Some(back.clone()),
        }
        Ok(())
    }

    /// The terminal changed size mid-animation: clear what we drew and start the
    /// viewport afresh at its old top row. Terminals that reflow lines when they
    /// get narrower can leave fragments of the old frame above it.
    pub fn reflow(&mut self, w: u16, h: u16) -> io::Result<()> {
        if self.alt {
            self.resize(w, h);
            return Ok(());
        }
        let rows = h;
        // Never climb further than our own rows: if the terminal re-wrapped
        // them we leave a fragment of the old frame, rather than erasing the
        // user's output above it.
        self.out.clear();
        self.move_to(0, 0);
        self.out.push_str("\x1b[0m\x1b[J");
        self.h = self.h.min(rows.saturating_sub(1)).max(1);
        for _ in 1..self.h {
            self.out.push('\n');
        }
        self.cy = self.h - 1;
        self.w = w;
        self.front = None;
        let mut stdout = io::stdout().lock();
        stdout.write_all(self.out.as_bytes())?;
        stdout.flush()
    }

    /// Erase everything we drew and give the terminal back, cursor at our first row.
    pub fn abandon(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if !self.alt {
            self.out.clear();
            self.move_to(0, 0);
            self.out.push_str("\x1b[0m\x1b[J");
            let mut stdout = io::stdout();
            let _ = stdout.write_all(self.out.as_bytes());
            let _ = stdout.flush();
        }
        restore();
    }

    /// Leave the final frame in place and put the cursor below it.
    pub fn finish(mut self) {
        self.close();
    }

    fn close(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if !self.alt {
            // Continue right after the last row with anything on it, and hand the
            // unused reserved rows back.
            let last = self.front.as_ref().map_or(self.h - 1, |f| {
                (0..f.h).rev().find(|&y| f.cells[y as usize * f.w as usize..(y as usize + 1) * f.w as usize].iter().any(|c| *c != Cell::BLANK)).unwrap_or(0)
            });
            self.out.clear();
            self.move_to(0, last);
            self.out.push_str("\x1b[0m\r\n\x1b[J");
            let mut stdout = io::stdout();
            let _ = stdout.write_all(self.out.as_bytes());
            let _ = stdout.flush();
        }
        restore();
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.close();
    }
}

/// Ask the terminal for its background color (OSC 11). A DA1 query is sent right
/// after it: every terminal answers DA1, so we never wait out a timeout on
/// terminals that ignore OSC 11. Must be called in raw mode.
#[cfg(unix)]
pub fn query_background() -> Option<Rgb> {
    if !is_interactive() || std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        return None;
    }
    let mut out = io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
    out.flush().ok()?;
    let deadline = Instant::now() + Duration::from_millis(120);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32) };
        if ready <= 0 {
            break;
        }
        let mut tmp = [0u8; 256];
        let n = unsafe { libc::read(0, tmp.as_mut_ptr().cast(), tmp.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
        if has_da1(&buf) {
            break;
        }
    }
    parse_osc11(&buf)
}

#[cfg(not(unix))]
pub fn query_background() -> Option<Rgb> {
    None
}

fn has_da1(buf: &[u8]) -> bool {
    buf.windows(3).enumerate().any(|(i, w)| {
        w == b"\x1b[?" && buf[i + 3..].iter().position(|&b| b == b'c').is_some_and(|end| {
            buf[i + 3..i + 3 + end].iter().all(|b| b.is_ascii_digit() || *b == b';')
        })
    })
}

fn parse_osc11(buf: &[u8]) -> Option<Rgb> {
    let s = String::from_utf8_lossy(buf);
    let rest = &s[s.find("11;rgb:")? + 7..];
    let mut parts = rest.split(['/', '\x1b', '\x07']);
    let mut comp = || {
        let p = parts.next()?;
        let v = u32::from_str_radix(p, 16).ok()?;
        let max = (1u32 << (4 * p.len() as u32)) - 1;
        Some((v * 255 / max.max(1)) as u8)
    };
    Some(Rgb(comp()?, comp()?, comp()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_background_reply() {
        let reply = b"\x1b]11;rgb:1a1a/1b1b/2626\x1b\\\x1b[?62;22c";
        assert!(has_da1(reply));
        assert_eq!(parse_osc11(reply), Some(Rgb(0x1a, 0x1b, 0x26)));
        assert!(!has_da1(b"\x1b]11;rgb:0/0/0"));
    }
}
