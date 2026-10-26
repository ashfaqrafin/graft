//! Everything is a function of time. A timeline is a base state plus a list of
//! tracks; `sample(t)` is pure, so skipping, speed control and golden-frame tests
//! come for free.

use super::tween::{Ease, lerp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Node(u32),
    Edge(u32),
    Item(u32),
    Particle(u32),
    Panel(u32),
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    Alpha,
    /// Node size while popping in (0 = nothing, 1 = full glyph).
    Scale,
    /// Brightening toward white.
    Glow,
    /// Tint toward the error color.
    Red,
    /// Horizontal wobble amplitude in cells (rejections).
    Shake,
    /// How much of an edge is drawn, or how far a particle has flown.
    Progress,
    /// Position of a bright pulse along an edge, measured from the child end. < 0 = none.
    Pulse,
    /// An item moving between its two anchors.
    Move,
    /// Typewriter reveal of an item's text.
    Reveal,
    /// Layout morph: 0 = before layout, 1 = union layout, 2 = after layout.
    Phase,
}

#[derive(Clone, Copy, Debug)]
pub struct Track {
    pub target: Target,
    pub prop: Prop,
    pub from: f32,
    pub to: f32,
    pub start: f32,
    pub dur: f32,
    pub ease: Ease,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NodeSt {
    pub alpha: f32,
    pub scale: f32,
    pub glow: f32,
    pub red: f32,
    pub shake: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct EdgeSt {
    pub alpha: f32,
    pub progress: f32,
    pub pulse: f32,
    pub glow: f32,
    pub red: f32,
}

impl Default for EdgeSt {
    fn default() -> Self {
        EdgeSt { alpha: 1.0, progress: 0.0, pulse: -1.0, glow: 0.0, red: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ItemSt {
    pub alpha: f32,
    pub mv: f32,
    pub glow: f32,
    pub red: f32,
    pub reveal: f32,
}

impl Default for ItemSt {
    fn default() -> Self {
        ItemSt { alpha: 0.0, mv: 0.0, glow: 0.0, red: 0.0, reveal: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ParticleSt {
    pub progress: f32,
    pub red: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PanelSt {
    pub alpha: f32,
    pub glow: f32,
    pub red: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GlobalSt {
    pub phase: f32,
    /// Current time, for effects that oscillate (shake, conflict pulse).
    pub time: f32,
}

#[derive(Clone, Debug, Default)]
pub struct SceneState {
    pub nodes: Vec<NodeSt>,
    pub edges: Vec<EdgeSt>,
    pub items: Vec<ItemSt>,
    pub particles: Vec<ParticleSt>,
    pub panels: Vec<PanelSt>,
    pub global: GlobalSt,
}

impl SceneState {
    fn slot(&mut self, target: Target, prop: Prop) -> Option<&mut f32> {
        Some(match (target, prop) {
            (Target::Node(i), Prop::Alpha) => &mut self.nodes.get_mut(i as usize)?.alpha,
            (Target::Node(i), Prop::Scale) => &mut self.nodes.get_mut(i as usize)?.scale,
            (Target::Node(i), Prop::Glow) => &mut self.nodes.get_mut(i as usize)?.glow,
            (Target::Node(i), Prop::Red) => &mut self.nodes.get_mut(i as usize)?.red,
            (Target::Node(i), Prop::Shake) => &mut self.nodes.get_mut(i as usize)?.shake,
            (Target::Edge(i), Prop::Alpha) => &mut self.edges.get_mut(i as usize)?.alpha,
            (Target::Edge(i), Prop::Progress) => &mut self.edges.get_mut(i as usize)?.progress,
            (Target::Edge(i), Prop::Pulse) => &mut self.edges.get_mut(i as usize)?.pulse,
            (Target::Edge(i), Prop::Glow) => &mut self.edges.get_mut(i as usize)?.glow,
            (Target::Edge(i), Prop::Red) => &mut self.edges.get_mut(i as usize)?.red,
            (Target::Item(i), Prop::Alpha) => &mut self.items.get_mut(i as usize)?.alpha,
            (Target::Item(i), Prop::Move) => &mut self.items.get_mut(i as usize)?.mv,
            (Target::Item(i), Prop::Glow) => &mut self.items.get_mut(i as usize)?.glow,
            (Target::Item(i), Prop::Red) => &mut self.items.get_mut(i as usize)?.red,
            (Target::Item(i), Prop::Reveal) => &mut self.items.get_mut(i as usize)?.reveal,
            (Target::Particle(i), Prop::Progress) => &mut self.particles.get_mut(i as usize)?.progress,
            (Target::Particle(i), Prop::Red) => &mut self.particles.get_mut(i as usize)?.red,
            (Target::Panel(i), Prop::Alpha) => &mut self.panels.get_mut(i as usize)?.alpha,
            (Target::Panel(i), Prop::Glow) => &mut self.panels.get_mut(i as usize)?.glow,
            (Target::Panel(i), Prop::Red) => &mut self.panels.get_mut(i as usize)?.red,
            (Target::Global, Prop::Phase) => &mut self.global.phase,
            _ => return None,
        })
    }
}

#[derive(Clone, Default)]
pub struct Timeline {
    pub base: SceneState,
    tracks: Vec<Track>,
    pub duration: f32,
}

impl Timeline {
    pub fn new(base: SceneState) -> Timeline {
        Timeline { base, tracks: Vec::new(), duration: 0.0 }
    }

    /// Add a tween and return the time it ends.
    #[allow(clippy::too_many_arguments)]
    pub fn tween(&mut self, target: Target, prop: Prop, from: f32, to: f32, start: f32, dur: f32, ease: Ease) -> f32 {
        let start = start.max(0.0);
        self.tracks.push(Track { target, prop, from, to, start, dur: dur.max(0.0), ease });
        self.duration = self.duration.max(start + dur);
        start + dur
    }

    /// Jump a property to a value at `at`.
    pub fn set(&mut self, target: Target, prop: Prop, value: f32, at: f32) {
        self.tween(target, prop, value, value, at, 0.0, super::tween::linear);
    }

    /// Rise to `peak` then fall back to `rest`.
    #[allow(clippy::too_many_arguments)]
    pub fn flash(&mut self, target: Target, prop: Prop, peak: f32, rest: f32, start: f32, up: f32, down: f32) -> f32 {
        self.tween(target, prop, rest, peak, start, up, super::tween::ease_out_quad);
        self.tween(target, prop, peak, rest, start + up, down, super::tween::ease_in_out)
    }

    /// Hold the final frame a little longer.
    pub fn extend_to(&mut self, t: f32) {
        self.duration = self.duration.max(t);
    }

    pub fn finish(&mut self) {
        // Later-starting tracks override earlier ones on the same property.
        self.tracks.sort_by(|a, b| a.start.total_cmp(&b.start));
    }

    pub fn sample_into(&self, t: f32, out: &mut SceneState) {
        out.clone_from(&self.base);
        out.global.time = t;
        for tr in &self.tracks {
            if t < tr.start {
                break;
            }
            let k = if tr.dur <= 0.0 { 1.0 } else { ((t - tr.start) / tr.dur).clamp(0.0, 1.0) };
            if let Some(slot) = out.slot(tr.target, tr.prop) {
                *slot = lerp(tr.from, tr.to, (tr.ease)(k));
            }
        }
    }

    #[cfg(test)]
    pub fn sample(&self, t: f32) -> SceneState {
        let mut s = SceneState::default();
        self.sample_into(t, &mut s);
        s
    }
}
