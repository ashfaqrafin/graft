//! Row/column assignment. Layout is computed once per state (before, union,
//! after) and the animation interpolates between them; nothing is laid out per frame.

pub mod lanes;
pub mod orient;
#[cfg(test)]
mod props;

pub use lanes::{Layout, assign, topo_order};
pub use orient::{Horizontal, Orientation, Vertical};
