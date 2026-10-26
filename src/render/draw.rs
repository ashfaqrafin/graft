//! Turn (Scene, SceneState) into cells.
//!
//! Edges are accumulated as direction masks per cell, so crossings and junctions
//! pick the right box-drawing glyph on their own. A partially drawn edge reveals
//! half a cell at a time (`╵` then `│`) with a bright head. Particles leave
//! braille trails for sub-cell motion.

use super::buffer::{BOLD, Buffer, DIM, ITALIC, Style};
use super::color::{Depth, Rgb, Theme};
use crate::anim::scene::{ItemKind, ItemStyle, NodeKind, Scene};
use crate::anim::timeline::SceneState;

const U: u8 = 1;
const D: u8 = 2;
const L: u8 = 4;
const R: u8 = 8;

#[derive(Clone, Copy, Default)]
struct Line {
    mask: u8,
    color: Rgb,
    weight: f32,
    dashed: bool,
}

#[derive(Clone, Copy, Default)]
struct Dots {
    bits: u8,
    color: Rgb,
    weight: f32,
}

/// Reusable per-frame storage so drawing does not allocate.
#[derive(Default)]
pub struct Scratch {
    path: Vec<(i32, i32)>,
    lines: Vec<Line>,
    dots: Vec<Dots>,
    xy: Vec<(i32, i32)>,
    heads: Vec<(i32, i32, char, Rgb)>,
}

pub fn draw(scene: &Scene, st: &SceneState, theme: &Theme, buf: &mut Buffer, sc: &mut Scratch) {
    buf.clear();
    let (w, h) = (buf.w as i32, buf.h as i32);
    let cells = (w * h) as usize;
    sc.lines.clear();
    sc.lines.resize(cells, Line::default());
    sc.dots.clear();
    sc.dots.resize(cells, Dots::default());

    let phase = st.global.phase;
    sc.xy.clear();
    for (i, _) in scene.nodes.iter().enumerate() {
        let (x, y) = scene.node_pos(i, phase);
        let shake = st.nodes.get(i).map_or(0.0, |n| n.shake);
        let dx = if shake > 0.0 { ((st.global.time * 55.0).sin() * shake).round() } else { 0.0 };
        sc.xy.push(((x + dx).round() as i32, y.round() as i32));
    }

    draw_edges(scene, st, theme, sc, w, h);
    draw_stubs(scene, st, theme, sc, w);
    flush_lines(sc, theme, buf, scene.graph_bottom as i32);
    draw_nodes(scene, st, theme, buf, sc);
    draw_panels(scene, st, theme, buf);
    draw_items(scene, st, theme, buf);
    draw_particles(scene, st, theme, buf, sc);
}

fn dir(from: (i32, i32), to: (i32, i32)) -> u8 {
    match (to.0 - from.0, to.1 - from.1) {
        (0, d) if d < 0 => U,
        (0, _) => D,
        (d, _) if d < 0 => L,
        _ => R,
    }
}

/// Cells of an edge from child to parent, both excluded: along the child's row to
/// the travel lane, down the lane, then along the parent's row into the parent.
pub(crate) fn edge_path(path: &mut Vec<(i32, i32)>, c: (i32, i32), lane: i32, p: (i32, i32)) {
    path.clear();
    let (xc, yc) = c;
    let (xp, yp) = p;
    if lane != xc {
        let step = (lane - xc).signum();
        let mut x = xc + step;
        while x != lane {
            path.push((x, yc));
            x += step;
        }
        path.push((lane, yc));
    }
    for y in yc + 1..yp {
        path.push((lane, y));
    }
    if lane != xp {
        path.push((lane, yp));
        let step = (xp - lane).signum();
        let mut x = lane + step;
        while x != xp {
            path.push((x, yp));
            x += step;
        }
    }
}

fn draw_edges(scene: &Scene, st: &SceneState, theme: &Theme, sc: &mut Scratch, w: i32, h: i32) {
    for (e, edge) in scene.edges.iter().enumerate() {
        let es = st.edges[e];
        if es.progress <= 0.001 || es.alpha <= 0.02 {
            continue;
        }
        let c = sc.xy[edge.child];
        let p = sc.xy[edge.parent];
        let lane = scene.lane_x(e, st.global.phase).round() as i32;
        let mut path = std::mem::take(&mut sc.path);
        if scene.horizontal {
            // Same routing with the axes swapped: time runs right to left from
            // child to parent, lanes are rows.
            if p.0 >= c.0 {
                sc.path = path;
                continue;
            }
            edge_path(&mut path, (c.1, -c.0), lane, (p.1, -p.0));
            for cell in path.iter_mut() {
                *cell = (-cell.1, cell.0);
            }
        } else {
            if p.1 <= c.1 {
                sc.path = path;
                continue;
            }
            edge_path(&mut path, c, lane, p);
        }
        let len = path.len();
        let halves = 2 * len;
        let shown = ((es.progress.clamp(0.0, 1.0) * halves as f32).round() as usize).min(halves);
        let drawing = shown < halves;
        let base = edge.color.lerp(theme.err, es.red);
        for j in 0..len {
            let s = if edge.from_parent { len - 1 - j } else { j };
            let full = 2 * s + 2 <= shown;
            let half = 2 * s + 1 == shown;
            if !full && !half {
                continue;
            }
            let (x, y) = path[j];
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let prev = if j == 0 { c } else { path[j - 1] };
            let next = if j + 1 == len { p } else { path[j + 1] };
            let (din, dout) = (dir((x, y), prev), dir((x, y), next));
            let mask = if full {
                din | dout
            } else if edge.from_parent {
                dout
            } else {
                din
            };
            let mut glow = es.glow;
            if drawing && (shown.saturating_sub(1)) / 2 == s {
                glow += 0.9; // the bright tip of a growing edge
            }
            if es.pulse >= 0.0 {
                let d = (j as f32 + 0.5 - es.pulse * len as f32).abs();
                glow += (1.0 - d / 2.2).max(0.0);
            }
            let color = theme.fade(theme.glow(base, glow.min(1.0)), es.alpha);
            let weight = es.alpha * (1.0 + glow);
            let cell = &mut sc.lines[(y * w + x) as usize];
            cell.mask |= mask;
            cell.dashed |= edge.dashed;
            if weight >= cell.weight {
                cell.weight = weight;
                cell.color = color;
            }
        }
        sc.path = path;
    }
}

/// A short dotted tail under commits whose history continues off-screen.
fn draw_stubs(scene: &Scene, st: &SceneState, theme: &Theme, sc: &mut Scratch, w: i32) {
    for (i, node) in scene.nodes.iter().enumerate() {
        let a = st.nodes[i].alpha;
        if !node.truncated || a < 0.1 {
            continue;
        }
        let (x, y) = sc.xy[i];
        let (x, y, mask) = if scene.horizontal { (x - 1, y, L | R) } else { (x, y + 1, U | D) };
        if x < 0 || x >= w || y >= scene.graph_bottom as i32 {
            continue;
        }
        let cell = &mut sc.lines[(y * w + x) as usize];
        if cell.mask == 0 {
            *cell = Line { mask, color: theme.fade(node.color, a * 0.45), weight: 0.01, dashed: true };
        }
    }
}

fn line_glyph(mask: u8, dashed: bool, unicode: bool) -> char {
    if !unicode {
        return match mask {
            m if m == U | D || m == U || m == D => if dashed { ':' } else { '|' },
            m if m == L | R || m == L || m == R => '-',
            m if m == U | L || m == D | R => '/',
            m if m == U | R || m == D | L => '\\',
            _ => '+',
        };
    }
    match mask {
        m if m == U | D => if dashed { '╎' } else { '│' },
        m if m == L | R => if dashed { '╌' } else { '─' },
        m if m == U | L => '╯',
        m if m == U | R => '╰',
        m if m == D | L => '╮',
        m if m == D | R => '╭',
        m if m == U | D | R => '├',
        m if m == U | D | L => '┤',
        m if m == L | R | D => '┬',
        m if m == L | R | U => '┴',
        m if m == U => '╵',
        m if m == D => '╷',
        m if m == L => '╴',
        m if m == R => '╶',
        _ => '┼',
    }
}

fn flush_lines(sc: &Scratch, theme: &Theme, buf: &mut Buffer, bottom: i32) {
    let w = buf.w as i32;
    for (i, line) in sc.lines.iter().enumerate() {
        if line.mask == 0 {
            continue;
        }
        let (x, y) = (i as i32 % w, i as i32 / w);
        if y >= bottom {
            continue;
        }
        let mut style = Style::fg(line.color);
        if theme.depth == Depth::Mono && line.weight < 0.55 {
            style.attr |= DIM;
        }
        buf.put(x, y, line_glyph(line.mask, line.dashed, theme.unicode), style);
    }
}

fn head_node(scene: &Scene, st: &SceneState) -> Option<usize> {
    let (i, item) = scene.items.iter().enumerate().find(|(_, it)| it.head)?;
    let s = st.items[i];
    if s.alpha < 0.5 {
        return None;
    }
    let anchor = if s.mv < 0.5 { &item.a } else { &item.b };
    match *anchor {
        crate::anim::scene::Anchor::Label { node, .. } => Some(node),
        _ => None,
    }
}

fn draw_nodes(scene: &Scene, st: &SceneState, theme: &Theme, buf: &mut Buffer, sc: &Scratch) {
    let head = head_node(scene, st);
    let u = theme.unicode;
    for (i, node) in scene.nodes.iter().enumerate() {
        let ns = st.nodes[i];
        if ns.alpha < 0.04 {
            continue;
        }
        let (x, y) = sc.xy[i];
        if y < 0 || y >= scene.graph_bottom as i32 || x < 0 {
            continue;
        }
        let glyph = match node.kind {
            NodeKind::Collapsed(_) if scene.horizontal => if u { '┄' } else { '~' },
            NodeKind::Collapsed(_) => if u { '┊' } else { ':' },
            NodeKind::Phantom => if u { '◌' } else { '?' },
            _ if ns.scale < 0.35 => if u { '·' } else { '.' },
            _ if ns.scale < 0.7 => if u { '•' } else { 'o' },
            _ if node.doomed && ns.alpha < 0.8 => if u { '○' } else { 'o' },
            _ if head == Some(i) => if u { '◉' } else { '@' },
            _ => if u { '●' } else { '*' },
        };
        let base = node.color.lerp(theme.err, ns.red);
        let color = theme.fade(theme.glow(base, ns.glow), ns.alpha);
        let mut style = Style::fg(color).bold();
        if theme.depth == Depth::Mono && ns.alpha < 0.55 {
            style.attr = DIM;
        }
        buf.put(x, y, glyph, style);

        // Text: short id + summary, or the collapsed count.
        if scene.horizontal {
            continue; // no room for summaries; labels and the caption carry the story
        }
        let ta = ns.alpha.min(1.0);
        let tx = scene.text_x as i32;
        let end = scene.text_end as i32;
        let mono_dim = |s: Style| if theme.depth == Depth::Mono && ta < 0.55 { s.with_attr(DIM) } else { s };
        match node.kind {
            NodeKind::Collapsed(_) => {
                buf.put_str(tx, y, &node.summary, end, Style::fg(theme.fade(theme.muted, ta)).with_attr(ITALIC));
            }
            NodeKind::Phantom => {
                let c = theme.fade(theme.glow(base, ns.glow * 0.5), ta);
                buf.put_str(tx, y, &node.summary, end, mono_dim(Style::fg(c).with_attr(ITALIC | BOLD)));
            }
            _ => {
                let oid_color = theme.fade(theme.glow(theme.oid, ns.glow * 0.6), ta * if node.doomed { 0.8 } else { 1.0 });
                let next = buf.put_str(tx, y, &node.oid, end, mono_dim(Style::fg(oid_color)));
                let fg = if node.doomed { theme.muted } else { theme.fg };
                let sum_color = theme.fade(theme.glow(fg, ns.glow * 0.5), ta);
                buf.put_str(next + 1, y, &node.summary, end, mono_dim(Style::fg(sum_color)));
            }
        }
    }
}

fn draw_panels(scene: &Scene, st: &SceneState, theme: &Theme, buf: &mut Buffer) {
    for (i, p) in scene.panels.iter().enumerate() {
        let ps = st.panels[i];
        if ps.alpha < 0.03 {
            continue;
        }
        let base = p.color.lerp(theme.err, ps.red);
        let color = theme.fade(theme.glow(base, ps.glow), ps.alpha);
        let style = Style::fg(color);
        let (x0, y0) = (p.x as i32, p.y as i32);
        let (x1, y1) = (x0 + p.w as i32 - 1, y0 + p.h as i32 - 1);
        let (tl, tr, bl, br, hz, vt) =
            if theme.unicode { ('╭', '╮', '╰', '╯', '─', '│') } else { ('+', '+', '+', '+', '-', '|') };
        for x in x0 + 1..x1 {
            buf.put(x, y0, hz, style);
            buf.put(x, y1, hz, style);
        }
        for y in y0 + 1..y1 {
            buf.put(x0, y, vt, style);
            buf.put(x1, y, vt, style);
        }
        buf.put(x0, y0, tl, style);
        buf.put(x1, y0, tr, style);
        buf.put(x0, y1, bl, style);
        buf.put(x1, y1, br, style);
        buf.put_str(x0 + 2, y0, &p.title, x1 - 1, Style::fg(theme.fade(theme.glow(base, ps.glow + 0.3), ps.alpha)).bold());
    }
}

fn draw_items(scene: &Scene, st: &SceneState, theme: &Theme, buf: &mut Buffer) {
    for (i, item) in scene.items.iter().enumerate() {
        let is = st.items[i];
        if is.alpha < 0.03 || is.reveal <= 0.0 {
            continue;
        }
        let (x, y) = scene.item_pos(i, st);
        let (x, y) = (x.round() as i32, y.round() as i32);
        if item.kind == ItemKind::Label && (y < 0 || y >= scene.graph_bottom as i32) {
            continue;
        }
        let max_x = item.max_x as i32;
        let n = item.text.chars().count();
        let shown = ((is.reveal.min(1.0) * n as f32).ceil() as usize).min(n);
        let text: &str = match item.text.char_indices().nth(shown) {
            Some((b, _)) => &item.text[..b],
            None => &item.text,
        };
        match item.style {
            ItemStyle::Pill { color } => {
                let base = color.lerp(theme.err, is.red);
                let bg = theme.fade(theme.glow(base, is.glow), is.alpha);
                let fg = theme.fade(theme.pill_fg, is.alpha);
                if theme.depth == Depth::Mono {
                    let style = Style { fg: None, bg: Some(bg), attr: BOLD };
                    buf.put_str(x, y, text, max_x, style);
                } else {
                    buf.put_str(x, y, text, max_x, Style::fg(fg).on(bg).bold());
                }
            }
            ItemStyle::Text { color, bold } => {
                let base = color.lerp(theme.err, is.red);
                let fg = theme.fade(theme.glow(base, is.glow), is.alpha);
                let mut style = Style::fg(fg);
                if bold {
                    style.attr |= BOLD;
                }
                if theme.depth == Depth::Mono && is.alpha < 0.55 {
                    style.attr |= DIM;
                }
                buf.put_str(x, y, text, max_x, style);
            }
        }
    }
}

const BRAILLE_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

fn draw_particles(scene: &Scene, st: &SceneState, theme: &Theme, buf: &mut Buffer, sc: &mut Scratch) {
    let w = buf.w as i32;
    let h = buf.h as i32;
    let mut heads = std::mem::take(&mut sc.heads);
    heads.clear();
    for (k, p) in scene.particles.iter().enumerate() {
        let ps = st.particles[k];
        if ps.progress <= 0.0 || ps.progress >= 1.0 {
            continue;
        }
        let a = scene.anchor_pos(&p.from, st);
        let b = scene.anchor_pos(&p.to, st);
        let (a, b) = ((a.0 + 0.5, a.1 + 0.5), (b.0 + 0.5, b.1 + 0.5));
        let ctrl = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0 - p.lift);
        let at = |t: f32| {
            let u = 1.0 - t;
            (u * u * a.0 + 2.0 * u * t * ctrl.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * ctrl.1 + t * t * b.1)
        };
        let color = p.color.lerp(theme.err, ps.red);
        let fade = if p.fade { 1.0 - ps.progress } else { 1.0 };
        if theme.unicode {
            const TRAIL: usize = 12;
            for i in 0..TRAIL {
                let t = ps.progress - i as f32 * 0.022;
                if t < 0.0 {
                    break;
                }
                let (x, y) = at(t);
                let (bx, by) = ((x * 2.0).floor() as i32, (y * 4.0).floor() as i32);
                let (cx, cy) = (bx.div_euclid(2), by.div_euclid(4));
                if cx < 0 || cy < 0 || cx >= w || cy >= h {
                    continue;
                }
                let weight = (1.0 - i as f32 / TRAIL as f32) * fade;
                let cell = &mut sc.dots[(cy * w + cx) as usize];
                cell.bits |= BRAILLE_BITS[bx.rem_euclid(2) as usize][by.rem_euclid(4) as usize];
                if weight > cell.weight {
                    cell.weight = weight;
                    cell.color = color;
                }
            }
        }
        if let Some(ch) = p.head {
            let (x, y) = at(ps.progress);
            heads.push((x.floor() as i32, y.floor() as i32, ch, theme.glow(color, 0.35)));
        }
    }
    for (i, d) in sc.dots.iter().enumerate() {
        if d.bits == 0 {
            continue;
        }
        let (x, y) = (i as i32 % w, i as i32 / w);
        if buf.is_blank(x, y) {
            let ch = char::from_u32(0x2800 + d.bits as u32).unwrap_or('⠂');
            buf.put(x, y, ch, Style::fg(theme.fade(theme.glow(d.color, d.weight * 0.4), d.weight)));
        }
    }
    for &(x, y, ch, color) in &heads {
        let ch = if theme.unicode { ch } else { '*' };
        buf.put(x, y, ch, Style::fg(color).bold());
    }
    sc.heads = heads;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_paths_are_connected() {
        let mut path = Vec::new();
        // merge: child at lane 0 row 0, second parent at lane 2 row 4
        edge_path(&mut path, (0, 0), 2, (2, 4));
        assert_eq!(path, vec![(1, 0), (2, 0), (2, 1), (2, 2), (2, 3)]);
        // fork: child at lane 2 row 0 joins its parent in lane 0 on row 4
        edge_path(&mut path, (2, 0), 2, (0, 4));
        assert_eq!(path, vec![(2, 1), (2, 2), (2, 3), (2, 4), (1, 4)]);
        assert_eq!(line_glyph(U | L, false, true), '╯');
        assert_eq!(line_glyph(D | L, false, true), '╮');
    }
}
