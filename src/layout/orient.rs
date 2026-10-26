//! Where a (lane, row) lands on screen. Lane assignment is orientation-free; an
//! `Orientation` turns it into cells.

pub trait Orientation {
    /// Cell position of the commit in `lane` at topological `row` (0 = newest).
    fn place(&self, lane: u32, row: u32) -> (f32, f32);
    /// Coordinate of a lane along the lane axis: x when vertical, y when horizontal.
    fn lane_coord(&self, lane: u32) -> f32;
}

/// Newest on top, lanes as columns (like `git log --graph`).
pub struct Vertical {
    pub x0: f32,
    pub y0: f32,
    pub lane_w: f32,
    pub row_h: f32,
}

impl Orientation for Vertical {
    fn place(&self, lane: u32, row: u32) -> (f32, f32) {
        (self.lane_coord(lane), self.y0 + row as f32 * self.row_h)
    }
    fn lane_coord(&self, lane: u32) -> f32 {
        self.x0 + lane as f32 * self.lane_w
    }
}

/// Newest on the right, lanes as rows (like GitHub's network view). Anchored at
/// the right edge, so older history scrolls off to the left.
pub struct Horizontal {
    pub right: f32,
    pub y0: f32,
    pub col_w: f32,
    pub lane_h: f32,
}

impl Orientation for Horizontal {
    fn place(&self, lane: u32, row: u32) -> (f32, f32) {
        (self.right - row as f32 * self.col_w, self.lane_coord(lane))
    }
    fn lane_coord(&self, lane: u32) -> f32 {
        self.y0 + lane as f32 * self.lane_h
    }
}
