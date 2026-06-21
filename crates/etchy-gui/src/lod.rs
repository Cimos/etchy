//! Level-of-detail for the diff canvas (G9).
//!
//! Three pure kernels keep the zoom behaviour testable, away from egui:
//! - [`geometry_alpha`] — how opaque a feature is at a given on-screen size, so
//!   tiny features fade out (true-to-scale) instead of clamping to a fixed dot.
//! - [`ring_area_nm2`] — a region's area, for the surfaced min-area threshold.
//! - [`HeatGrid`] — a screen-space density grid: when features are sub-pixel they
//!   feed a green/red glow that shows *where* changes are without any dot growing.

use etchy_core::Pt;

/// Geometry opacity for a feature `px` wide on screen: 0 at/below `lo`, 1 at/above
/// `hi`, linear between. Below `lo` the feature is left to the heatmap (weight =
/// `1 - geometry_alpha`); this is what makes tiny features fade instead of clamping
/// to a fixed dot.
pub fn geometry_alpha(px: f32, lo: f32, hi: f32) -> f32 {
    if px <= lo {
        0.0
    } else if px >= hi {
        1.0
    } else {
        (px - lo) / (hi - lo)
    }
}

/// Absolute area of a closed ring (shoelace), in nm² for nm input. Winding-
/// independent; a ring with fewer than 3 points has zero area.
pub fn ring_area_nm2(ring: &[Pt]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        s += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    (s * 0.5).abs()
}

/// A screen-space density grid. Sub-pixel diff features deposit their weight into
/// the cell under their centroid; the renderer then draws each non-empty cell as a
/// green/red glow scaled by intensity — so many tiny changes read as one soft field
/// instead of a speckle or a blob.
pub struct HeatGrid {
    pub cols: usize,
    pub rows: usize,
    cell: f32,
    origin: (f32, f32),
    added: Vec<f32>,
    removed: Vec<f32>,
}

impl HeatGrid {
    pub fn new(origin: (f32, f32), width: f32, height: f32, cell: f32) -> Self {
        let cell = cell.max(1.0);
        let cols = (width / cell).ceil().max(1.0) as usize;
        let rows = (height / cell).ceil().max(1.0) as usize;
        Self {
            cols,
            rows,
            cell,
            origin,
            added: vec![0.0; cols * rows],
            removed: vec![0.0; cols * rows],
        }
    }

    fn idx(&self, x: f32, y: f32) -> Option<usize> {
        let cx = ((x - self.origin.0) / self.cell).floor();
        let cy = ((y - self.origin.1) / self.cell).floor();
        if cx < 0.0 || cy < 0.0 {
            return None;
        }
        let (cx, cy) = (cx as usize, cy as usize);
        if cx >= self.cols || cy >= self.rows {
            return None;
        }
        Some(cy * self.cols + cx)
    }

    /// Deposit `weight` at a screen position into the added (green) or removed
    /// (red) channel. Positions outside the grid are ignored.
    pub fn add(&mut self, pos: (f32, f32), weight: f32, is_added: bool) {
        if let Some(i) = self.idx(pos.0, pos.1) {
            if is_added {
                self.added[i] += weight;
            } else {
                self.removed[i] += weight;
            }
        }
    }

    /// Largest single-channel accumulation, for normalizing intensity to [0,1].
    pub fn max(&self) -> f32 {
        self.added
            .iter()
            .chain(self.removed.iter())
            .copied()
            .fold(0.0, f32::max)
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    /// Top-left screen position of a cell.
    pub fn cell_origin(&self, col: usize, row: usize) -> (f32, f32) {
        (
            self.origin.0 + col as f32 * self.cell,
            self.origin.1 + row as f32 * self.cell,
        )
    }

    /// Non-empty cells as `(col, row, added, removed)`.
    pub fn cells(&self) -> impl Iterator<Item = (usize, usize, f32, f32)> + '_ {
        (0..self.added.len()).filter_map(move |i| {
            let (a, r) = (self.added[i], self.removed[i]);
            if a == 0.0 && r == 0.0 {
                None
            } else {
                Some((i % self.cols, i / self.cols, a, r))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etchy_core::Pt;

    #[test]
    fn geometry_alpha_ramps_and_clamps() {
        assert_eq!(geometry_alpha(0.0, 1.0, 4.0), 0.0); // below lo: invisible
        assert_eq!(geometry_alpha(1.0, 1.0, 4.0), 0.0); // at lo
        assert_eq!(geometry_alpha(4.0, 1.0, 4.0), 1.0); // at hi: full
        assert_eq!(geometry_alpha(9.0, 1.0, 4.0), 1.0); // above hi: clamped
        assert_eq!(geometry_alpha(2.5, 1.0, 4.0), 0.5); // midpoint
    }

    #[test]
    fn ring_area_of_a_square_is_side_squared() {
        // 1000nm x 1000nm square -> 1_000_000 nm² regardless of winding.
        let sq = [
            Pt::new(0, 0),
            Pt::new(1000, 0),
            Pt::new(1000, 1000),
            Pt::new(0, 1000),
        ];
        assert_eq!(ring_area_nm2(&sq), 1_000_000.0);
        let rev: Vec<Pt> = sq.iter().rev().copied().collect();
        assert_eq!(ring_area_nm2(&rev), 1_000_000.0); // abs: winding-independent
        assert_eq!(ring_area_nm2(&sq[..2]), 0.0); // degenerate: <3 points
    }

    #[test]
    fn heatgrid_accumulates_into_the_right_cell() {
        // 100x100 px area, 10px cells -> 10x10 grid.
        let mut g = HeatGrid::new((0.0, 0.0), 100.0, 100.0, 10.0);
        assert_eq!((g.cols, g.rows), (10, 10));
        g.add((5.0, 5.0), 2.0, true); // cell (0,0) added
        g.add((15.0, 5.0), 3.0, false); // cell (1,0) removed
        g.add((5.0, 5.0), 1.0, true); // same cell again -> sums
        g.add((999.0, 999.0), 5.0, true); // out of bounds -> ignored
        let cells: Vec<_> = g.cells().collect();
        assert_eq!(cells.len(), 2);
        assert!(cells.contains(&(0, 0, 3.0, 0.0)));
        assert!(cells.contains(&(1, 0, 0.0, 3.0)));
        assert_eq!(g.max(), 3.0);
    }
}
