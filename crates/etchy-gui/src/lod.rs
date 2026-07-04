//! Level-of-detail for the diff canvas (G9).
//!
//! Two pure kernels keep the zoom behaviour testable, away from egui:
//! - [`geometry_alpha`] — how opaque a feature is at a given on-screen size, so
//!   tiny features fade to nothing (true-to-scale) instead of clamping to a fixed
//!   dot (the old blob / all-green-when-zoomed-out bug).
//! - [`ring_area_nm2`] — a region's area, for the surfaced min-area threshold that
//!   drops noise (e.g. the sub-µm rims from a units/precision mismatch).

use etchy_core::Pt;

/// How a diff region draws at a given on-screen size (#14). The kernel below maps a
/// region's screen-px size + its noise verdict to exactly one of these, so the cull
/// (genuine noise) and the LOD fade (real-but-tiny) are no longer entangled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lod {
    /// Genuine noise (area below the absolute min) — dropped at every zoom.
    Cull,
    /// A real diff too small to draw to scale — show a fixed-size marker dot so it
    /// stays visible instead of phantoming.
    Marker,
    /// Draw the geometry to scale at this opacity (1.0 = full).
    Fade(f32),
}

/// Below this fade opacity, keep showing the fixed marker dot instead of the
/// to-scale geometry (#156). Without it a small feature (e.g. a drill hole) is a
/// solid dot at `px <= lo` but a near-invisible ~0-alpha smear just above `lo`,
/// so it *disappears* for a mid-zoom band and only fades back in near `hi` — the
/// "drill holes vanish then come back as I zoom" dropout. The marker now hands off
/// to the geometry only once the geometry is opaque enough to stand on its own.
pub const LOD_MARKER_ALPHA_FLOOR: f32 = 0.5;

/// Decide how a diff region renders from its on-screen width `px`, the fade band
/// `[lo, hi]`, and whether it is genuine noise (area below the absolute min).
///
/// - `is_noise` → [`Lod::Cull`] (the only cull path; surfaced in the caption).
/// - else opacity `< LOD_MARKER_ALPHA_FLOOR` → [`Lod::Marker`] (a fixed dot; covers
///   `px <= lo` AND the low-opacity start of the fade band, so a real feature is
///   never invisible — the #14 phantom AND the #156 mid-zoom dropout).
/// - else → [`Lod::Fade`] with [`geometry_alpha`] (≥ `hi` clamps to full).
pub fn lod_render(px: f32, lo: f32, hi: f32, is_noise: bool) -> Lod {
    if is_noise {
        return Lod::Cull;
    }
    let alpha = geometry_alpha(px, lo, hi);
    if alpha < LOD_MARKER_ALPHA_FLOOR {
        Lod::Marker
    } else {
        Lod::Fade(alpha)
    }
}

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
    fn lod_render_genuine_noise_is_culled() {
        // #14: a region below the absolute min-area (is_noise = true) is dropped at
        // every zoom — even when it would render large on screen. This is the ONLY
        // path that culls; it's surfaced in the caption, never a silent miss.
        assert_eq!(lod_render(0.2, 1.5, 5.0, true), Lod::Cull);
        assert_eq!(lod_render(100.0, 1.5, 5.0, true), Lod::Cull);
    }

    #[test]
    fn lod_render_below_lo_is_a_marker_not_culled() {
        // A real diff (not noise) that renders sub-pixel must show as a fixed-size
        // MARKER dot, never vanish (the #14 phantom/all-green bug).
        assert_eq!(lod_render(0.0, 1.5, 5.0, false), Lod::Marker);
        assert_eq!(lod_render(1.5, 1.5, 5.0, false), Lod::Marker); // at lo: still a marker
        assert_eq!(lod_render(0.3, 1.5, 5.0, false), Lod::Marker); // deep sub-pixel
    }

    #[test]
    fn lod_render_no_mid_zoom_dropout_above_lo() {
        // #156: just above `lo` the to-scale geometry would be near-invisible
        // (~0 alpha), so the feature must STAY a marker dot rather than vanish.
        // At lo=1.5,hi=5.0 the floor (0.5) is reached at px=3.25.
        for &px in &[1.6_f32, 2.0, 3.0] {
            assert_eq!(
                lod_render(px, 1.5, 5.0, false),
                Lod::Marker,
                "px={px} is above lo but sub-floor opacity — must be a marker, not invisible"
            );
        }
    }

    #[test]
    fn lod_render_fades_once_opaque_enough() {
        // At/after the floor opacity, hand off to to-scale geometry.
        match lod_render(3.25, 1.5, 5.0, false) {
            Lod::Fade(a) => assert!((a - 0.5).abs() < 1e-6, "expected ~0.5, got {a}"),
            other => panic!("expected Fade at the floor, got {other:?}"),
        }
        match lod_render(4.5, 1.5, 5.0, false) {
            Lod::Fade(a) => assert!(a > 0.5, "expected >0.5, got {a}"),
            other => panic!("expected Fade above the floor, got {other:?}"),
        }
    }

    #[test]
    fn lod_render_large_is_full_geometry() {
        // At/above hi: full opaque geometry (Fade(1.0)).
        assert_eq!(lod_render(5.0, 1.5, 5.0, false), Lod::Fade(1.0));
        assert_eq!(lod_render(50.0, 1.5, 5.0, false), Lod::Fade(1.0));
    }

    #[test]
    fn lod_render_fade_band_is_continuous_with_geometry_alpha() {
        // Where it Fades (opacity at/above the floor), it must agree with
        // geometry_alpha so wiring it in can't change that part of the ramp.
        for &px in &[4.0_f32, 4.9] {
            let a = geometry_alpha(px, 1.5, 5.0);
            assert_eq!(lod_render(px, 1.5, 5.0, false), Lod::Fade(a));
        }
    }

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
