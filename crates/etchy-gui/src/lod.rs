//! Level-of-detail for the diff canvas (G9).
//!
//! Two pure kernels keep the zoom behaviour testable, away from egui:
//! - [`geometry_alpha`] — how opaque a feature is at a given on-screen size, so
//!   tiny features fade to nothing (true-to-scale) instead of clamping to a fixed
//!   dot (the old blob / all-green-when-zoomed-out bug).
//! - [`ring_area_nm2`] — a region's area, for the surfaced min-area threshold that
//!   drops noise (e.g. the sub-µm rims from a units/precision mismatch).

use etchy_core::Pt;

/// Geometry opacity for a feature `px` wide on screen: 0 at/below `lo`, 1 at/above
/// `hi`, linear between. Below `lo` the feature is simply not drawn, so tiny changes
/// fade out as you zoom out rather than clamping to a fixed size.
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
}
