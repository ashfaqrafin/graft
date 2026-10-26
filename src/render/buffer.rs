//! A grid of styled cells. Frames are drawn into a back buffer and only the cells
//! that differ from the front buffer are written to the terminal.

use std::fmt::Write as _;

use unicode_width::UnicodeWidthChar;

use super::color::{Depth, Rgb};

pub const BOLD: u8 = 1;
pub const DIM: u8 = 2;
pub const ITALIC: u8 = 4;

/// Marks the second column of a double-width character.
pub const WIDE_TAIL: char = '\0';

pub fn text_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub attr: u8,
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', fg: None, bg: None, attr: 0 };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub attr: u8,
}

impl Style {
    pub fn fg(c: Rgb) -> Style {
        Style { fg: Some(c), bg: None, attr: 0 }
    }
    pub fn bold(mut self) -> Style {
        self.attr |= BOLD;
        self
    }
    pub fn with_attr(mut self, attr: u8) -> Style {
        self.attr |= attr;
        self
    }
    pub fn on(mut self, bg: Rgb) -> Style {
        self.bg = Some(bg);
        self
    }
}

#[derive(Clone, Debug)]
pub struct Buffer {
    pub w: u16,
    pub h: u16,
    pub cells: Vec<Cell>,
}

impl Buffer {
    pub fn new(w: u16, h: u16) -> Buffer {
        Buffer { w, h, cells: vec![Cell::BLANK; w as usize * h as usize] }
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::BLANK);
    }

    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.w as i32 && y < self.h as i32).then(|| y as usize * self.w as usize + x as usize)
    }

    pub fn get(&self, x: i32, y: i32) -> Option<&Cell> {
        self.idx(x, y).map(|i| &self.cells[i])
    }

    pub fn is_blank(&self, x: i32, y: i32) -> bool {
        self.get(x, y).is_some_and(|c| c.ch == ' ' && c.bg.is_none())
    }

    pub fn put(&mut self, x: i32, y: i32, ch: char, style: Style) {
        if let Some(i) = self.idx(x, y) {
            // Overwriting half of a wide character blanks the other half.
            if self.cells[i].ch == WIDE_TAIL && x > 0 {
                self.cells[i - 1] = Cell::BLANK;
            }
            self.cells[i] = Cell { ch, fg: style.fg, bg: style.bg, attr: style.attr };
        }
    }

    /// Draw `s` starting at `x`, never past `max_x` (exclusive). Overlong text ends in an
    /// ellipsis. Returns the column after the last cell written.
    pub fn put_str(&mut self, x: i32, y: i32, s: &str, max_x: i32, style: Style) -> i32 {
        let max_x = max_x.min(self.w as i32);
        let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
        let fits = x + total as i32 <= max_x;
        let mut cx = x;
        for ch in s.chars() {
            let w = ch.width().unwrap_or(0) as i32;
            if w == 0 {
                continue;
            }
            if !fits && cx + w > max_x - 1 {
                if cx < max_x {
                    self.put(cx, y, '…', style);
                    cx += 1;
                }
                break;
            }
            if cx >= 0 {
                self.put(cx, y, ch, style);
                if w == 2 {
                    self.put(cx + 1, y, WIDE_TAIL, style);
                }
            }
            cx += w;
        }
        cx
    }

    /// Plain text rows, trailing spaces trimmed (golden-frame tests).
    pub fn to_plain(&self) -> String {
        let mut out = String::new();
        for y in 0..self.h as usize {
            let row = &self.cells[y * self.w as usize..(y + 1) * self.w as usize];
            let line: String = row.iter().filter(|c| c.ch != WIDE_TAIL).map(|c| c.ch).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Rows with SGR styling and no cursor movement, for printing a still frame.
    pub fn to_ansi(&self, depth: Depth) -> String {
        let mut out = String::new();
        for y in 0..self.h as usize {
            let row = &self.cells[y * self.w as usize..(y + 1) * self.w as usize];
            let end = row.iter().rposition(|c| c.ch != ' ' || c.bg.is_some()).map_or(0, |p| p + 1);
            let mut cur: Option<Style> = None;
            for c in &row[..end] {
                if c.ch == WIDE_TAIL {
                    continue;
                }
                let st = Style { fg: c.fg, bg: c.bg, attr: c.attr };
                if cur != Some(st) {
                    push_sgr(&mut out, st, depth);
                    cur = Some(st);
                }
                out.push(c.ch);
            }
            if cur.is_some_and(|s| s != Style::default()) {
                out.push_str("\x1b[0m");
            }
            out.push('\n');
        }
        out
    }
}

/// Append the SGR sequence that selects `st` from a reset state.
pub fn push_sgr(out: &mut String, st: Style, depth: Depth) {
    out.push_str("\x1b[0");
    if st.attr & BOLD != 0 {
        out.push_str(";1");
    }
    if st.attr & DIM != 0 {
        out.push_str(";2");
    }
    if st.attr & ITALIC != 0 {
        out.push_str(";3");
    }
    let mut color = |c: Rgb, bg: bool| match depth {
        Depth::True => {
            let _ = write!(out, ";{};2;{};{};{}", if bg { 48 } else { 38 }, c.0, c.1, c.2);
        }
        Depth::C256 => {
            let _ = write!(out, ";{};5;{}", if bg { 48 } else { 38 }, c.to_256());
        }
        Depth::C16 => {
            let code = c.to_16();
            let _ = write!(out, ";{}", if bg { code + 10 } else { code });
        }
        Depth::Mono => {}
    };
    if let Some(fg) = st.fg {
        color(fg, false);
    }
    if let Some(bg) = st.bg {
        if depth == Depth::Mono {
            out.push_str(";7");
        } else {
            color(bg, true);
        }
    }
    out.push('m');
}
