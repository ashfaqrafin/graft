//! Scenes: a function from (Transition, operation) to (Scene, Timeline).
//!
//! Every scene starts from the exact before picture (the compact before layout),
//! morphs to the union layout to make room, plays the operation, then morphs to
//! the compact after layout so the final frame looks like `graft log` would.

pub mod caption;
mod merge;
mod panels;
mod remote;

use std::collections::{HashMap, HashSet};

use super::scene::{Anchor, ItemKind, ItemStyle, NodeKind, SEdge, SItem, SNode, SPanel, SParticle, Scene};
use super::timeline::{EdgeSt, GlobalSt, ItemSt, NodeSt, PanelSt, Prop, SceneState, Target, Timeline};
use super::tween::*;
use crate::git::{RefKind, Snapshot};
use crate::layout::{self, Horizontal, Layout, Orientation, Vertical};
use crate::model::{Change, NodeId, Transition};
use crate::op::{Op, OpInfo};
use crate::render::color::{Rgb, Theme};

// Timing, in seconds at --speed 1.
const HOLD: f32 = 0.1;
const EXPAND: f32 = 0.26;
const GHOST: f32 = 0.22;
const FLY: f32 = 0.38;
const EDGE: f32 = 0.24;
const POP: f32 = 0.26;
const LABEL: f32 = 0.34;
const SETTLE: f32 = 0.3;
const TAIL: f32 = 0.15;

const LANE_W: f32 = 2.0;
const GX: f32 = 2.0;
const TOP: f32 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub w: u16,
    pub max_h: u16,
    /// Lay the graph out left to right.
    pub horizontal: bool,
}

/// Build the scene and its timeline.
pub fn build(tr: &Transition, info: &OpInfo, theme: &Theme, vp: Viewport) -> (Scene, Timeline) {
    let mut s = Stage::new(tr, info, theme, vp);
    if tr.phantom.is_some() {
        if info.preview {
            merge::preview(&mut s);
        } else {
            merge::conflict(&mut s);
        }
    } else if info.op == Op::Push {
        remote::push(&mut s);
    } else if !tr.is_noop() || info.exit_code != 0 {
        generic(&mut s);
    } else {
        // Nothing moves (e.g. a preview of a no-op): still say so.
        s.caption_in(0.05);
        let t = s.settle(0.05);
        s.end(t + 0.45);
    }
    s.finish()
}

/// A still frame of the current state (used by `graft log` and idle `graft watch`).
pub fn still(tr: &Transition, theme: &Theme, vp: Viewport, caption: bool) -> (Scene, Timeline) {
    let info = OpInfo::new(Op::Other, Vec::new());
    let mut s = Stage::new(tr, &info, theme, vp);
    if !caption {
        // Horizontal graphs have no room for summaries: describe HEAD instead.
        let head = tr.after.head_oid().and_then(|o| tr.graph.find(&o)).filter(|_| vp.horizontal);
        s.sc.items[s.caption].text = match head {
            Some(n) => format!("{} {}", s.sc.nodes[n].oid, s.sc.nodes[n].summary),
            None => String::new(),
        };
        s.sc.items[s.caption].style = ItemStyle::Text { color: theme.fg, bold: false };
    }
    s.tl.base.global.phase = 2.0;
    s.tl.base.items[s.caption].reveal = 1.0;
    s.tl.base.items[s.caption].mv = 1.0;
    s.finish()
}

pub(super) struct PanelEntry {
    pub item: usize,
    pub path: String,
    pub before: bool,
    pub after: bool,
    pub conflict: bool,
}

pub(super) struct Stage<'a> {
    pub tr: &'a Transition,
    pub info: &'a OpInfo,
    pub theme: &'a Theme,
    pub sc: Scene,
    pub tl: Timeline,
    pub vis: [Vec<bool>; 3],
    pub layouts: [Layout; 3],
    pub edge_of: HashMap<(NodeId, NodeId), usize>,
    pub labels: HashMap<String, usize>,
    pub wt: Vec<PanelEntry>,
    pub stash: Vec<PanelEntry>,
    pub panel_wt: Option<usize>,
    pub panel_stash: Option<usize>,
    pub panel_remote: Option<usize>,
    pub caption: usize,
    /// When each node has finished appearing.
    pub appear: Vec<f32>,
    /// Items whose motion has been scheduled already.
    pub handled: HashSet<usize>,
    morph: [bool; 2],
}

struct LabelSpec {
    key: String,
    text: String,
    color: Rgb,
    node: NodeId,
    rank: u8,
    dx: f32,
    /// Stacking slot above the commit (horizontal layout only).
    dy: f32,
}

impl<'a> Stage<'a> {
    fn new(tr: &'a Transition, info: &'a OpInfo, theme: &'a Theme, vp: Viewport) -> Stage<'a> {
        let g = &tr.graph;
        let n = g.nodes.len();
        let keep_ghosts = info.op == Op::Reset;
        let vis0: Vec<bool> = g.nodes.iter().map(|x| x.before).collect();
        let vis2: Vec<bool> = g.nodes.iter().map(|x| x.after || (keep_ghosts && x.before)).collect();
        let vis1: Vec<bool> = (0..n).map(|i| vis0[i] || vis2[i]).collect();

        // The before picture is ordered around the old HEAD and the after picture
        // around the new one, so the first and last frames match `graft log`.
        let node_of = |snap: &Snapshot| snap.head_oid().and_then(|o| g.find(&o));
        let priority = |first: &Snapshot, second: &Snapshot| -> Vec<NodeId> {
            tr.phantom.into_iter().chain(node_of(first)).chain(node_of(second)).collect()
        };
        let before_order = layout::topo_order(g, &priority(&tr.before, &tr.after));
        let after_order = layout::topo_order(g, &priority(&tr.after, &tr.before));
        let layouts = [
            layout::assign(g, &before_order, &vis0),
            layout::assign(g, &before_order, &vis1),
            layout::assign(g, &after_order, &vis2),
        ];
        let lanes = layouts.iter().map(|l| l.lanes).max().unwrap_or(1).max(1);
        let rows = layouts[1].rows.max(1);

        // Labels first: they decide where the text column starts.
        let mut before_labels = phase_labels(tr, &tr.before, &vis0, theme);
        let mut after_labels = phase_labels(tr, &tr.after, &vis2, theme);
        let label_w = before_labels
            .iter()
            .chain(after_labels.iter())
            .map(|l| l.dx as u16 + text_w(&l.text))
            .max()
            .unwrap_or(0)
            .min(vp.w / 3);

        let graph_w = (lanes - 1) as f32 * LANE_W + 1.0;
        let label_x = (GX + graph_w + 1.0) as u16;
        let text_x = if label_w > 0 { label_x + label_w + 1 } else { label_x };

        // Side panels.
        let ops_with_wt = matches!(info.op, Op::Commit | Op::Stash | Op::Reset | Op::Stage | Op::Checkout);
        let wt_changed = tr.changes.iter().any(|c| matches!(c, Change::WorktreeChanged | Change::ConflictFiles(_)));
        let have_wt = tr.before.worktree.is_some() && tr.after.worktree.is_some();
        let stash_changed = tr.changes.iter().any(|c| matches!(c, Change::StashPushed(_) | Change::StashPopped(_)));
        let panels_fit = vp.w >= 72;
        let show_wt = panels_fit && have_wt && (wt_changed || (ops_with_wt && info.op != Op::Checkout));
        let show_stash = panels_fit && (stash_changed || info.op == Op::Stash);
        let show_remote = panels_fit && matches!(info.op, Op::Fetch | Op::Push | Op::Pull);
        let pw = ((vp.w as f32 * 0.3) as u16).clamp(26, 40);
        let px = vp.w.saturating_sub(pw + 1);
        let text_end = if show_wt || show_stash || show_remote { px.saturating_sub(2) } else { vp.w.saturating_sub(1) };

        let mut sc = Scene { w: vp.w, label_x, text_x, text_end, horizontal: vp.horizontal, ..Default::default() };
        let mut stage_panels: Vec<(SPanel, f32)> = Vec::new();
        let mut items: Vec<(SItem, ItemSt)> = Vec::new();
        let mut py = TOP as u16;
        let mut panel_remote = None;
        if show_remote {
            panel_remote = Some(stage_panels.len());
            stage_panels.push((SPanel { x: px, y: py, w: pw, h: 3, title: " remote ".into(), color: theme.remote }, 1.0));
            let name = remote_name(tr, info);
            let icon = if theme.unicode { "⇅" } else { "<>" };
            let text = format!("{icon} {name}");
            let at = Anchor::Fixed(px as f32 + 2.0, py as f32 + 1.0);
            items.push((panel_item(text, theme.remote, at, at, px + pw - 1), ItemSt { alpha: 1.0, ..Default::default() }));
            py += 3;
        }
        let mut wt = Vec::new();
        let mut panel_wt = None;
        if show_wt {
            let b = panels::worktree_entries(tr.before.worktree.as_ref().unwrap(), theme);
            let a = panels::worktree_entries(tr.after.worktree.as_ref().unwrap(), theme);
            let (h, entries) = panels::place(&b, &a, px, py, pw, &mut items);
            wt = entries;
            panel_wt = Some(stage_panels.len());
            stage_panels.push((SPanel { x: px, y: py, w: pw, h, title: " working tree ".into(), color: theme.muted }, 1.0));
            py += h;
        }
        let mut stash = Vec::new();
        let mut panel_stash = None;
        if show_stash {
            let b = panels::stash_entries(&tr.before.stash, theme);
            let a = panels::stash_entries(&tr.after.stash, theme);
            let (h, entries) = panels::place(&b, &a, px, py, pw, &mut items);
            stash = entries;
            panel_stash = Some(stage_panels.len());
            stage_panels.push((SPanel { x: px, y: py, w: pw, h, title: " stash ".into(), color: theme.lanes[2] }, 1.0));
            py += h;
        }

        let stub = g.nodes.iter().any(|n| n.truncated) as u16;
        let orient: Box<dyn Orientation> = if vp.horizontal {
            // Labels sit above their commit, stacked into slots when they would overlap.
            let margin = group_width(&before_labels).max(group_width(&after_labels)).clamp(2.0, 24.0);
            let right = text_end as f32 - margin;
            let col_w = if (rows as f32 - 1.0) * 4.0 <= right - GX { 4.0 } else { 3.0 };
            let probe = Horizontal { right, y0: 0.0, col_w, lane_h: 0.0 };
            let at = |l: &Layout, n: NodeId| Some((probe.place(l.lane[n]?, l.row[n]?).0, l.lane[n]?));
            let s0 = stack_labels(&mut before_labels, |n| at(&layouts[0], n).or_else(|| at(&layouts[1], n)));
            let s2 = stack_labels(&mut after_labels, |n| at(&layouts[2], n).or_else(|| at(&layouts[1], n)));
            let slots = s0.max(s2).max(1) as f32;
            let lane_h = slots + 2.0;
            let y0 = TOP + slots;
            let graph_h = (y0 - TOP + (lanes - 1) as f32 * lane_h + 1.0) as u16;
            sc.h = ((TOP as u16 + graph_h).max(py) + 2).min(vp.max_h.max(6));
            sc.row_h = 1;
            Box::new(Horizontal { right, y0, col_w, lane_h })
        } else {
            // Two rows per commit when it fits, else one.
            let chrome = TOP as u16 + 2;
            let tall = (rows - 1) as u16 * 2 + 1 + stub;
            let row_h: u16 = if tall + chrome <= vp.max_h { 2 } else { 1 };
            let graph_h = (rows - 1) as u16 * row_h + 1 + stub;
            sc.h = ((TOP as u16 + graph_h).max(py) + 2).min(vp.max_h.max(6));
            sc.row_h = row_h;
            Box::new(Vertical { x0: GX, y0: TOP, lane_w: LANE_W, row_h: row_h as f32 })
        };
        let h = sc.h;
        sc.graph_bottom = h.saturating_sub(2);

        // Nodes.
        let pos = |l: &Layout, i: NodeId| -> Option<(f32, f32)> { Some(orient.place(l.lane[i]?, l.row[i]?)) };
        for (i, node) in g.nodes.iter().enumerate() {
            let p1 = pos(&layouts[1], i).unwrap_or_else(|| orient.place(0, 0));
            let p0 = pos(&layouts[0], i).unwrap_or(p1);
            let p2 = pos(&layouts[2], i).unwrap_or(p1);
            let lane = layouts[1].lane[i].unwrap_or(0) as usize;
            let kind = if node.phantom {
                NodeKind::Phantom
            } else if node.collapsed > 0 {
                NodeKind::Collapsed(node.collapsed)
            } else if node.parents.len() > 1 {
                NodeKind::Merge
            } else {
                NodeKind::Commit
            };
            let summary = match kind {
                NodeKind::Collapsed(c) => {
                    let dots = if theme.unicode { "⋯" } else { "..." };
                    format!("{dots} {c} more commit{}", if c == 1 { "" } else { "s" })
                }
                _ => node.summary.clone(),
            };
            sc.nodes.push(SNode {
                pos: [p0, p1, p2],
                color: if node.phantom { theme.accent } else { theme.lane(lane) },
                kind,
                oid: if node.phantom { String::new() } else { node.oid.to_hex_with_len(7).to_string() },
                summary,
                truncated: node.truncated,
                doomed: node.before && !node.after,
            });
        }

        // Edges.
        let mut edge_of = HashMap::new();
        for (e, (c, p)) in g.edges().into_iter().enumerate() {
            let l1 = layouts[1].edge_lane[e].unwrap_or(0);
            let lx = |l: &Layout| orient.lane_coord(l.edge_lane[e].unwrap_or(l1));
            sc.edges.push(SEdge {
                child: c,
                parent: p,
                lane_x: [lx(&layouts[0]), lx(&layouts[1]), lx(&layouts[2])],
                color: theme.lane(l1 as usize),
                from_parent: true,
                dashed: g.nodes[c].phantom,
            });
            edge_of.insert((c, p), e);
        }

        // Labels.
        let mut labels = HashMap::new();
        let mut keys: Vec<&str> = Vec::new();
        for l in before_labels.iter().chain(after_labels.iter()) {
            if !keys.contains(&l.key.as_str()) {
                keys.push(&l.key);
            }
        }
        for key in keys {
            let b = before_labels.iter().find(|l| l.key == key);
            let a = after_labels.iter().find(|l| l.key == key);
            let spec = b.or(a).unwrap();
            let anchor = |l: &LabelSpec| Anchor::Label { node: l.node, dx: l.dx, dy: l.dy };
            let aa = b.map(anchor).unwrap_or_else(|| anchor(a.unwrap()));
            let bb = a.map(anchor).unwrap_or(aa);
            let item = SItem {
                text: spec.text.clone(),
                style: ItemStyle::Pill { color: spec.color },
                kind: ItemKind::Label,
                a: aa,
                b: bb,
                max_x: if vp.horizontal { text_end } else { text_x.saturating_sub(1) },
                head: key == "HEAD",
            };
            labels.insert(key.to_string(), items.len());
            items.push((item, ItemSt { alpha: if b.is_some() { 1.0 } else { 0.0 }, ..Default::default() }));
        }

        // Caption.
        let (text, tone) = caption::caption(tr, info, theme.unicode);
        let color = match tone {
            caption::Tone::Ok => theme.ok,
            caption::Tone::Info => theme.accent,
            caption::Tone::Warn => theme.warn,
            caption::Tone::Err => theme.err,
        };
        let caption = items.len();
        // The caption sits under the tallest picture while things move, then rises
        // to sit just under the final graph.
        let mut bottom = TOP;
        for (i, nd) in sc.nodes.iter().enumerate() {
            if vis2[i] {
                bottom = bottom.max(nd.pos[2].1 + if nd.truncated { 1.0 } else { 0.0 });
            }
        }
        let rest_y = ((bottom as u16 + 1).max(py) + 1).min(h - 1);
        let at = Anchor::Fixed(1.0, (h - 1) as f32);
        items.push((
            SItem {
                text,
                style: ItemStyle::Text { color, bold: true },
                kind: ItemKind::Caption,
                a: at,
                b: Anchor::Fixed(1.0, rest_y as f32),
                max_x: vp.w.saturating_sub(1),
                head: false,
            },
            ItemSt { alpha: 1.0, reveal: 0.0, ..Default::default() },
        ));

        let base = SceneState {
            nodes: g
                .nodes
                .iter()
                .map(|x| {
                    let v = if x.before { 1.0 } else { 0.0 };
                    NodeSt { alpha: v, scale: v, ..Default::default() }
                })
                .collect(),
            edges: sc
                .edges
                .iter()
                .map(|e| EdgeSt { progress: if vis0[e.child] && vis0[e.parent] { 1.0 } else { 0.0 }, ..Default::default() })
                .collect(),
            items: items.iter().map(|(_, st)| *st).collect(),
            particles: Vec::new(),
            panels: stage_panels.iter().map(|(_, a)| PanelSt { alpha: *a, ..Default::default() }).collect(),
            global: GlobalSt::default(),
        };
        sc.items = items.into_iter().map(|(i, _)| i).collect();
        sc.panels = stage_panels.into_iter().map(|(p, _)| p).collect();

        let moved = |a: usize, b: usize| {
            sc.nodes.iter().any(|nd| nd.pos[a] != nd.pos[b]) || sc.edges.iter().any(|e| e.lane_x[a] != e.lane_x[b])
        };
        let morph = [moved(0, 1), moved(1, 2)];

        Stage {
            tr,
            info,
            theme,
            tl: Timeline::new(base),
            sc,
            vis: [vis0, vis1, vis2],
            layouts,
            edge_of,
            labels,
            wt,
            stash,
            panel_wt,
            panel_stash,
            panel_remote,
            caption,
            appear: vec![0.0; n],
            handled: HashSet::new(),
            morph,
        }
    }

    fn finish(mut self) -> (Scene, Timeline) {
        // Particles are created during choreography; give them base state now.
        self.tl.base.particles = vec![Default::default(); self.sc.particles.len()];
        self.tl.finish();
        (self.sc, self.tl)
    }

    // ---- building blocks -------------------------------------------------

    pub fn node(n: NodeId) -> Anchor {
        Anchor::Node { node: n, dx: 0.0, dy: 0.0 }
    }

    pub fn panel_center(&self, p: usize) -> Anchor {
        let p = &self.sc.panels[p];
        Anchor::Fixed(p.x as f32 + p.w as f32 / 2.0, p.y as f32 + p.h as f32 / 2.0 - 0.5)
    }

    pub fn caption_in(&mut self, t: f32) {
        let len = self.sc.items[self.caption].text.chars().count() as f32;
        let dur = (len * 0.012).clamp(0.25, 0.6);
        self.tl.tween(Target::Item(self.caption as u32), Prop::Reveal, 0.0, 1.0, t, dur, linear);
    }

    /// Morph from the compact before layout to the union layout.
    pub fn expand(&mut self, t: f32) -> f32 {
        if self.morph[0] {
            self.tl.tween(Target::Global, Prop::Phase, 0.0, 1.0, t, EXPAND, ease_in_out_cubic)
        } else {
            self.tl.set(Target::Global, Prop::Phase, 1.0, t);
            t
        }
    }

    /// Morph from the union layout to the compact after layout.
    pub fn settle(&mut self, t: f32) -> f32 {
        let cap = Target::Item(self.caption as u32);
        if self.morph[1] {
            self.tl.tween(cap, Prop::Move, 0.0, 1.0, t, SETTLE, ease_in_out_cubic);
            self.tl.tween(Target::Global, Prop::Phase, 1.0, 2.0, t, SETTLE, ease_in_out_cubic)
        } else {
            self.tl.set(cap, Prop::Move, 1.0, t);
            self.tl.set(Target::Global, Prop::Phase, 2.0, t);
            t
        }
    }

    pub fn fly(&mut self, from: Anchor, to: Anchor, t: f32, dur: f32, color: Rgb, lift: f32) -> f32 {
        let id = self.sc.particles.len() as u32;
        self.sc.particles.push(SParticle { from, to, lift, color, head: Some('●'), fade: false });
        self.tl.tween(Target::Particle(id), Prop::Progress, 0.0, 1.0, t, dur, ease_in_out_cubic)
    }

    /// A particle that turns red on the way (rejections).
    pub fn fly_red(&mut self, from: Anchor, to: Anchor, t: f32, dur: f32, color: Rgb, lift: f32) -> f32 {
        let id = self.sc.particles.len() as u32;
        let end = self.fly(from, to, t, dur, color, lift);
        self.tl.set(Target::Particle(id), Prop::Red, 1.0, t);
        end
    }

    /// A small burst of braille sparks around a node.
    pub fn sparks(&mut self, n: NodeId, t: f32, color: Rgb) {
        if !self.theme.unicode {
            return;
        }
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::TAU / 8.0 + 0.3;
            let id = self.sc.particles.len() as u32;
            self.sc.particles.push(SParticle {
                from: Self::node(n),
                to: Anchor::Node { node: n, dx: a.cos() * 3.2, dy: a.sin() * 1.4 },
                lift: 0.0,
                color,
                head: None,
                fade: true,
            });
            self.tl.tween(Target::Particle(id), Prop::Progress, 0.0, 1.0, t, 0.34, ease_out_cubic);
        }
    }

    /// Grow the edges from `n` to its parents, starting at the parent end.
    pub fn grow_edges(&mut self, n: NodeId, t: f32, dur: f32, upto: f32) -> f32 {
        let parents = self.tr.graph.nodes[n].parents.clone();
        for p in parents {
            if !self.vis[2][p] {
                continue;
            }
            if let Some(&e) = self.edge_of.get(&(n, p)) {
                self.sc.edges[e].from_parent = true;
                self.tl.tween(Target::Edge(e as u32), Prop::Progress, 0.0, upto, t, dur, ease_out_cubic);
            }
        }
        t + dur
    }

    /// Pop a node into existence with a little overshoot, a flash and sparks.
    pub fn pop(&mut self, n: NodeId, t: f32) -> f32 {
        let id = Target::Node(n as u32);
        self.tl.tween(id, Prop::Alpha, 0.0, 1.0, t, 0.08, linear);
        self.tl.tween(id, Prop::Scale, 0.0, 1.0, t, POP, ease_out_back);
        self.tl.flash(id, Prop::Glow, 1.0, 0.0, t, 0.05, 0.6);
        let color = self.sc.nodes[n].color;
        self.sparks(n, t + 0.08, color);
        self.appear[n] = t + 0.06;
        t + POP
    }

    /// The edges grow up from the parents and arrive just as the node pops at `at`.
    pub fn spawn(&mut self, n: NodeId, at: f32) -> f32 {
        self.grow_edges(n, at - EDGE, EDGE, 1.0);
        self.pop(n, at)
    }

    /// Fade a node (and the edges touching it) between two alphas.
    pub fn fade_node(&mut self, n: NodeId, from: f32, to: f32, t: f32, dur: f32) -> f32 {
        self.tl.tween(Target::Node(n as u32), Prop::Alpha, from, to, t, dur, ease_in_out);
        for (e, edge) in self.sc.edges.iter().enumerate() {
            if edge.child == n || (edge.parent == n && self.sc.nodes[edge.child].doomed) {
                self.tl.tween(Target::Edge(e as u32), Prop::Alpha, from, to, t, dur, ease_in_out);
            }
        }
        t + dur
    }

    pub fn move_item(&mut self, item: usize, t: f32, dur: f32) -> f32 {
        self.handled.insert(item);
        let id = Target::Item(item as u32);
        self.tl.flash(id, Prop::Glow, 0.8, 0.0, t, dur * 0.3, dur);
        self.tl.tween(id, Prop::Move, 0.0, 1.0, t, dur, ease_in_out_cubic)
    }

    pub fn fade_in_item(&mut self, item: usize, t: f32) -> f32 {
        self.handled.insert(item);
        let id = Target::Item(item as u32);
        self.tl.flash(id, Prop::Glow, 1.0, 0.0, t, 0.05, 0.6);
        self.tl.tween(id, Prop::Alpha, 0.0, 1.0, t, 0.25, ease_out_cubic)
    }

    pub fn fade_out_item(&mut self, item: usize, t: f32, red: bool) -> f32 {
        self.handled.insert(item);
        let id = Target::Item(item as u32);
        if red {
            self.tl.tween(id, Prop::Red, 0.0, 1.0, t, 0.1, linear);
        }
        self.tl.tween(id, Prop::Alpha, 1.0, 0.0, t, 0.24, ease_in_out)
    }

    pub fn flash_panel(&mut self, p: Option<usize>, t: f32, red: bool) {
        if let Some(p) = p {
            let id = Target::Panel(p as u32);
            self.tl.flash(id, Prop::Glow, 1.0, 0.0, t, 0.06, 0.5);
            if red {
                self.tl.flash(id, Prop::Red, 1.0, 0.0, t, 0.06, 0.8);
            }
        }
    }

    /// Send a bright pulse along the edges between `desc` and its ancestor `anc`.
    pub fn pulse_route(&mut self, desc: NodeId, anc: NodeId, t: f32, dur: f32, upward: bool) {
        let Some(route) = self.tr.graph.route(desc, anc) else { return };
        if route.is_empty() {
            return;
        }
        let steps: Vec<(NodeId, NodeId)> = if upward { route.into_iter().rev().collect() } else { route };
        let per = dur / steps.len() as f32;
        for (k, (c, p)) in steps.into_iter().enumerate() {
            let Some(&e) = self.edge_of.get(&(c, p)) else { continue };
            let s = t + k as f32 * per;
            let (from, to, reached) = if upward { (1.0, 0.0, c) } else { (0.0, 1.0, p) };
            let id = Target::Edge(e as u32);
            let end = self.tl.tween(id, Prop::Pulse, from, to, s, per * 1.3, linear);
            self.tl.set(id, Prop::Pulse, -1.0, end);
            self.tl.flash(Target::Node(reached as u32), Prop::Glow, 0.9, 0.0, s + per * 0.9, 0.05, 0.35);
        }
    }

    /// Move every label to where it belongs in the after state, waiting for target
    /// commits to exist. Returns when the last label lands.
    pub fn move_labels(&mut self, t: f32) -> f32 {
        let tr = self.tr;
        let mut end = t;
        let mut pulsed: HashSet<(NodeId, NodeId)> = HashSet::new();
        let mut moves: Vec<(String, Option<NodeId>, Option<NodeId>)> = Vec::new();
        for c in &tr.changes {
            match c {
                Change::RefMoved { name, from, to, .. } => moves.push((name.clone(), *from, *to)),
                Change::HeadMoved { from, to } => moves.push(("HEAD".into(), from.node, to.node)),
                _ => {}
            }
        }
        // Branches first, so HEAD's pulse is skipped when its branch already pulsed.
        moves.sort_by_key(|(k, _, _)| k == "HEAD");
        for (key, from, to) in moves {
            let Some(&item) = self.labels.get(&key) else { continue };
            let start = to.map_or(t, |n| t.max(self.appear[n]));
            match (from, to) {
                (Some(f), Some(n)) => {
                    end = end.max(self.move_item(item, start, LABEL));
                    if f != n && pulsed.insert((f, n)) {
                        if tr.graph.is_ancestor(f, n) {
                            self.pulse_route(n, f, start, LABEL, true);
                        } else if tr.graph.is_ancestor(n, f) {
                            self.pulse_route(f, n, start, LABEL, false);
                        }
                    }
                }
                (None, Some(_)) => end = end.max(self.fade_in_item(item, start)),
                (Some(_), None) => end = end.max(self.fade_out_item(item, start, true)),
                (None, None) => {}
            }
        }
        // Labels that only shift sideways, or appear/disappear with their commit.
        for &item in self.labels.clone().values() {
            if self.handled.contains(&item) {
                continue;
            }
            let it = &self.sc.items[item];
            let before = self.tl.base.items[item].alpha > 0.5;
            let after = self.label_in_after(item);
            if !anchor_eq(&it.a, &it.b) && before && after {
                end = end.max(self.move_item(item, t, LABEL));
            } else if before && !after {
                end = end.max(self.fade_out_item(item, t, false));
            } else if !before && after {
                let start = match it.b {
                    Anchor::Label { node, .. } => t.max(self.appear[node]),
                    _ => t,
                };
                end = end.max(self.fade_in_item(item, start));
            }
        }
        end
    }

    fn label_in_after(&self, item: usize) -> bool {
        let key = self.labels.iter().find(|(_, i)| **i == item).map(|(k, _)| k.as_str());
        let Some(key) = key else { return false };
        let snap = &self.tr.after;
        let node = if key == "HEAD" {
            snap.head_oid()
        } else {
            snap.find_ref(key).map(|r| r.target)
        };
        node.and_then(|o| self.tr.graph.find(&o)).is_some_and(|n| self.vis[2][n])
    }

    pub fn spawned(&self) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self
            .tr
            .changes
            .iter()
            .filter_map(|c| match c {
                Change::Spawn(n) if !self.tr.graph.nodes[*n].phantom => Some(*n),
                _ => None,
            })
            .collect();
        // Parents first: bottom rows of the union layout spawn first.
        v.sort_by_key(|n| std::cmp::Reverse(self.layouts[1].row[*n].unwrap_or(0)));
        v
    }

    pub fn vanished(&self) -> Vec<NodeId> {
        self.tr.changes.iter().filter_map(|c| if let Change::Vanish(n) = c { Some(*n) } else { None }).collect()
    }

    pub fn reparent_source(&self, n: NodeId) -> Option<NodeId> {
        self.tr.changes.iter().find_map(|c| match c {
            Change::Reparent { old, new } if *new == n => Some(*old),
            _ => None,
        })
    }

    pub fn end(&mut self, t: f32) {
        self.tl.extend_to(t + TAIL);
    }
}

fn anchor_eq(a: &Anchor, b: &Anchor) -> bool {
    match (a, b) {
        (Anchor::Label { node: n1, dx: x1, dy: y1 }, Anchor::Label { node: n2, dx: x2, dy: y2 }) => {
            n1 == n2 && x1 == x2 && y1 == y2
        }
        (Anchor::Fixed(x1, y1), Anchor::Fixed(x2, y2)) => x1 == x2 && y1 == y2,
        _ => false,
    }
}

fn text_w(s: &str) -> u16 {
    use unicode_width::UnicodeWidthStr;
    s.width() as u16
}

fn panel_item(text: String, color: Rgb, a: Anchor, b: Anchor, max_x: u16) -> SItem {
    SItem { text, style: ItemStyle::Text { color, bold: false }, kind: ItemKind::Panel, a, b, max_x, head: false }
}

fn remote_name(tr: &Transition, info: &OpInfo) -> String {
    for c in &tr.changes {
        if let Change::RefMoved { kind: RefKind::Remote, short, .. } = c
            && let Some((remote, _)) = short.split_once('/') {
                return remote.to_string();
            }
    }
    info.positionals().first().filter(|p| !p.contains('/')).map(|s| s.to_string()).unwrap_or_else(|| "origin".into())
}

/// Labels for one state, with their horizontal offsets in the label column.
fn phase_labels(tr: &Transition, snap: &Snapshot, vis: &[bool], theme: &Theme) -> Vec<LabelSpec> {
    let g = &tr.graph;
    let current = snap.head_branch();
    let mut out: Vec<LabelSpec> = Vec::new();
    if let Some(n) = snap.head_oid().and_then(|o| g.find(&o)).filter(|n| vis[*n]) {
        out.push(LabelSpec { key: "HEAD".into(), text: " HEAD ".into(), color: theme.head, node: n, rank: 0, dx: 0.0, dy: 0.0 });
    }
    for r in &snap.refs {
        let Some(n) = g.find(&r.target).filter(|n| vis[*n]) else { continue };
        let (rank, color) = match r.kind {
            RefKind::Local if Some(r.name.as_str()) == current => (1, theme.local),
            RefKind::Local => (2, theme.local),
            RefKind::Remote => (3, theme.remote),
            RefKind::Tag => (4, theme.tag),
        };
        out.push(LabelSpec { key: r.name.clone(), text: format!(" {} ", r.short), color, node: n, rank, dx: 0.0, dy: 0.0 });
    }
    out.sort_by(|a, b| (a.node, a.rank, &a.key).cmp(&(b.node, b.rank, &b.key)));
    let mut cursor: HashMap<NodeId, f32> = HashMap::new();
    for l in &mut out {
        let x = cursor.entry(l.node).or_insert(0.0);
        l.dx = *x;
        *x += text_w(&l.text) as f32 + 1.0;
    }
    out
}

/// Width of the widest group of labels on one commit.
fn group_width(labels: &[LabelSpec]) -> f32 {
    labels.iter().map(|l| l.dx + text_w(&l.text) as f32).fold(0.0, f32::max)
}

/// Horizontal layout: give each commit's label group a slot above its lane so groups
/// on neighbouring commits do not overlap. `at` gives a node's x and lane. Returns
/// the number of slots used.
fn stack_labels(labels: &mut [LabelSpec], at: impl Fn(NodeId) -> Option<(f32, u32)>) -> usize {
    let mut groups: Vec<(u32, f32, f32, NodeId)> = Vec::new();
    for l in labels.iter() {
        let Some((x, lane)) = at(l.node) else { continue };
        let end = x - 1.0 + l.dx + text_w(&l.text) as f32;
        match groups.iter_mut().find(|g| g.3 == l.node) {
            Some(g) => g.2 = g.2.max(end),
            None => groups.push((lane, x - 1.0, end, l.node)),
        }
    }
    groups.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal));
    let mut ends: HashMap<(u32, usize), f32> = HashMap::new();
    let mut slot_of: HashMap<NodeId, usize> = HashMap::new();
    let mut slots = 0;
    for (lane, start, end, node) in groups {
        let mut s = 0;
        while ends.get(&(lane, s)).is_some_and(|e| start <= *e) {
            s += 1;
        }
        ends.insert((lane, s), end);
        slot_of.insert(node, s);
        slots = slots.max(s + 1);
    }
    for l in labels.iter_mut() {
        l.dy = slot_of.get(&l.node).copied().unwrap_or(0) as f32;
    }
    slots
}

// ---- the generic choreography ------------------------------------------------

/// Ghost what is being rewritten, grow new commits (flying in from their
/// originals, the staging area or the remote), move labels, settle.
fn generic(s: &mut Stage) {
    let op = s.info.op;
    s.caption_in(0.05);
    let expanded = s.expand(HOLD);
    // Flights may start while the layout is still opening up: their anchors follow it.
    let t = HOLD + (expanded - HOLD) * 0.5;

    let vanished = s.vanished();
    let spawned = s.spawned();
    if !vanished.is_empty() && !spawned.is_empty() {
        for &v in &vanished {
            s.fade_node(v, 1.0, 0.35, HOLD, GHOST);
        }
    }

    let from_remote = remote::remote_sources(s);
    let staged_fly = op == Op::Commit && s.panel_wt.is_some();
    let n = spawned.len();
    let stagger = if n > 0 { (0.8 / n as f32).min(0.18) } else { 0.0 };
    let mut landed = expanded;
    let mut first_pop = f32::MAX;
    for (k, &node) in spawned.iter().enumerate() {
        let start = t + k as f32 * stagger;
        let color = s.sc.nodes[node].color;
        let arrive = if let Some(old) = s.reparent_source(node) {
            s.tl.flash(Target::Node(old as u32), Prop::Glow, 1.0, 0.0, start, 0.05, 0.4);
            s.fly(Stage::node(old), Stage::node(node), start, FLY, s.sc.nodes[old].color, 1.5)
        } else if from_remote.contains(&node) && s.panel_remote.is_some() {
            let from = s.panel_center(s.panel_remote.unwrap());
            s.fly(from, Stage::node(node), start, FLY, color, 0.0)
        } else if staged_fly && k + 1 == n {
            panels::staged_into(s, node, start)
        } else {
            start.max(expanded) + EDGE
        };
        s.spawn(node, arrive);
        first_pop = first_pop.min(arrive);
        landed = landed.max(arrive);
    }

    let t_labels = s.move_labels(expanded);
    let t_panels = panels::transition(s, if n > 0 { first_pop - 0.15 } else { expanded });
    let mut t_end = t_labels.max(t_panels).max(landed + POP * 0.6);

    if !vanished.is_empty() {
        let keep = op == Op::Reset;
        let (from, at) = if spawned.is_empty() { (1.0, expanded) } else { (0.35, landed) };
        for &v in &vanished {
            let to = if keep { 0.3 } else { 0.0 };
            t_end = t_end.max(s.fade_node(v, from, to, at, GHOST));
        }
    }
    if s.info.exit_code != 0 {
        s.flash_panel(s.panel_wt, expanded, true);
    }
    let t = s.settle(t_end);
    s.end(t);
}
