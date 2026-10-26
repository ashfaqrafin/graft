//! The static half of an animation: geometry, text and colors. The dynamic half
//! (`SceneState`) comes from sampling the timeline; the renderer combines both.

use super::timeline::SceneState;
use super::tween::lerp;
use crate::render::color::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Commit,
    Merge,
    /// Stands for this many hidden commits.
    Collapsed(u32),
    /// Predicted commit (preview) or a merge that stopped on conflicts.
    Phantom,
}

#[derive(Clone, Debug)]
pub struct SNode {
    /// Cell position in the before, union and after layouts.
    pub pos: [(f32, f32); 3],
    pub color: Rgb,
    pub kind: NodeKind,
    pub oid: String,
    pub summary: String,
    pub truncated: bool,
    /// Only exists in the before state (drawn hollow while fading).
    pub doomed: bool,
}

#[derive(Clone, Debug)]
pub struct SEdge {
    pub child: usize,
    pub parent: usize,
    /// Where the edge's run sits along the lane axis in each layout (a column
    /// when vertical, a row when horizontal).
    pub lane_x: [f32; 3],
    pub color: Rgb,
    /// Grow from the parent end (true) or the child end.
    pub from_parent: bool,
    pub dashed: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    /// Follows a node's current position, offset by (dx, dy).
    Node { node: usize, dx: f32, dy: f32 },
    /// A label for a node: in the label column on its row (vertical), or stacked
    /// `dy` slots above it (horizontal).
    Label { node: usize, dx: f32, dy: f32 },
    Fixed(f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// Branch/tag pill in the label column.
    Label,
    /// Text in a side panel.
    Panel,
    /// The status line under the graph.
    Caption,
}

#[derive(Clone, Copy, Debug)]
pub enum ItemStyle {
    Pill { color: Rgb },
    Text { color: Rgb, bold: bool },
}

#[derive(Clone, Debug)]
pub struct SItem {
    pub text: String,
    pub style: ItemStyle,
    pub kind: ItemKind,
    pub a: Anchor,
    pub b: Anchor,
    /// Clip column (exclusive).
    pub max_x: u16,
    /// For the HEAD label: the node it points to before / after.
    pub head: bool,
}

#[derive(Clone, Debug)]
pub struct SPanel {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub title: String,
    pub color: Rgb,
}

#[derive(Clone, Debug)]
pub struct SParticle {
    pub from: Anchor,
    pub to: Anchor,
    /// Arc height in cells (0 = straight line). Positive bows upward.
    pub lift: f32,
    pub color: Rgb,
    /// Glyph drawn at the head; None draws only the braille trail (sparks).
    pub head: Option<char>,
    /// Sparks fade out as they fly.
    pub fade: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub w: u16,
    pub h: u16,
    pub row_h: u16,
    pub nodes: Vec<SNode>,
    pub edges: Vec<SEdge>,
    pub items: Vec<SItem>,
    pub particles: Vec<SParticle>,
    pub panels: Vec<SPanel>,
    pub label_x: u16,
    pub text_x: u16,
    pub text_end: u16,
    /// Bottom of the graph area (exclusive); nodes below are clipped.
    pub graph_bottom: u16,
    /// Time runs left to right instead of top to bottom.
    pub horizontal: bool,
}

impl Scene {
    pub fn node_pos(&self, i: usize, phase: f32) -> (f32, f32) {
        let p = &self.nodes[i].pos;
        morph(p[0], p[1], p[2], phase)
    }

    pub fn lane_x(&self, e: usize, phase: f32) -> f32 {
        let l = &self.edges[e].lane_x;
        morph((l[0], 0.0), (l[1], 0.0), (l[2], 0.0), phase).0
    }

    pub fn anchor_pos(&self, a: &Anchor, st: &SceneState) -> (f32, f32) {
        match *a {
            Anchor::Node { node, dx, dy } => {
                let (x, y) = self.node_pos(node, st.global.phase);
                (x + dx, y + dy)
            }
            Anchor::Label { node, dx, dy } => {
                let (x, y) = self.node_pos(node, st.global.phase);
                if self.horizontal { (x + dx - 1.0, y - 1.0 - dy) } else { (self.label_x as f32 + dx, y) }
            }
            Anchor::Fixed(x, y) => (x, y),
        }
    }

    pub fn item_pos(&self, i: usize, st: &SceneState) -> (f32, f32) {
        let it = &self.items[i];
        let a = self.anchor_pos(&it.a, st);
        let b = self.anchor_pos(&it.b, st);
        let t = st.items[i].mv;
        (lerp(a.0, b.0, t), lerp(a.1, b.1, t))
    }
}

fn morph(a: (f32, f32), b: (f32, f32), c: (f32, f32), phase: f32) -> (f32, f32) {
    if phase <= 1.0 {
        let t = phase.clamp(0.0, 1.0);
        (lerp(a.0, b.0, t), lerp(a.1, b.1, t))
    } else {
        let t = (phase - 1.0).clamp(0.0, 1.0);
        (lerp(b.0, c.0, t), lerp(b.1, c.1, t))
    }
}
