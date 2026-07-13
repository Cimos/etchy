//! Geometry builders: apertures and stroked paths → filled integer contours.
//!
//! All work in nanometres. Curves are tessellated with enough segments to keep
//! the chord error well under a micron, so the integer (nm) result is faithful.
//! Outer rings are wound **CCW** (positive area) so i_overlay's NonZero fill
//! treats them as solid.

use std::f64::consts::PI;

use crate::geo::{Contour, Pt};

/// Segments for a full circle. 64 keeps the n-gon area within ~0.13% of the true
/// circle and matches the golden-corpus ground-truth maths.
pub const CIRCLE_SEGMENTS: usize = 64;

/// Max chord (sagitta) error for arc/curve tessellation, in nm (1 µm).
const SAG_TOL_NM: f64 = 1000.0;

fn p(x: f64, y: f64) -> Pt {
    Pt::new(x.round() as i64, y.round() as i64)
}

/// CCW regular n-gon, radius `r` (nm) at `(cx, cy)`.
pub fn ngon(cx: f64, cy: f64, r: f64) -> Contour {
    (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let a = 2.0 * PI * (k as f64) / CIRCLE_SEGMENTS as f64;
            p(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// CCW axis-aligned rectangle centred at `(cx, cy)`, full width/height `w`,`h` (nm).
pub fn rect(cx: f64, cy: f64, w: f64, h: f64) -> Contour {
    let (hw, hh) = (w / 2.0, h / 2.0);
    vec![
        p(cx - hw, cy - hh),
        p(cx + hw, cy - hh),
        p(cx + hw, cy + hh),
        p(cx - hw, cy + hh),
    ]
}

/// CCW rectangle with corners rotated `deg` degrees about its centre.
pub fn rect_rot(cx: f64, cy: f64, w: f64, h: f64, deg: f64) -> Contour {
    let (hw, hh) = (w / 2.0, h / 2.0);
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)]
        .iter()
        .map(|&(x, y)| {
            let (rx, ry) = rotate(x, y, deg);
            p(cx + rx, cy + ry)
        })
        .collect()
}

/// CCW "stadium" (capsule): the Minkowski sum of segment `a`–`b` with a disk of
/// radius `r` (nm). This is a round-aperture **stroke**, and an obround pad.
pub fn stadium(ax: f64, ay: f64, bx: f64, by: f64, r: f64) -> Contour {
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return ngon(ax, ay, r); // degenerate segment → a disk
    }
    let theta = dy.atan2(dx);
    let half = (CIRCLE_SEGMENTS / 2).max(2);
    let mut pts = Vec::with_capacity(2 * (half + 1));
    // Right cap (around b): theta-90° .. theta+90°, CCW.
    for k in 0..=half {
        let a = theta - PI / 2.0 + PI * (k as f64) / half as f64;
        pts.push(p(bx + r * a.cos(), by + r * a.sin()));
    }
    // Left cap (around a): theta+90° .. theta+270°, CCW.
    for k in 0..=half {
        let a = theta + PI / 2.0 + PI * (k as f64) / half as f64;
        pts.push(p(ax + r * a.cos(), ay + r * a.sin()));
    }
    pts
}

/// CCW obround (stadium) pad centred at `(cx, cy)`, full size `w`×`h` (nm).
pub fn obround(cx: f64, cy: f64, w: f64, h: f64) -> Contour {
    if (w - h).abs() < 1.0 {
        return ngon(cx, cy, w.min(h) / 2.0);
    }
    if w > h {
        let r = h / 2.0;
        let d = (w - h) / 2.0;
        stadium(cx - d, cy, cx + d, cy, r)
    } else {
        let r = w / 2.0;
        let d = (h - w) / 2.0;
        stadium(cx, cy - d, cx, cy + d, r)
    }
}

/// Tessellate an arc from `(fx,fy)` to `(tx,ty)` about `(cx,cy)`, sweeping CCW (or
/// CW). Returns the polyline points (nm) including both endpoints. A coincident
/// from/to is a full circle. Multi-quadrant (sweep may exceed 90°, up to 360°).
pub fn arc_points(fx: f64, fy: f64, tx: f64, ty: f64, cx: f64, cy: f64, ccw: bool) -> Vec<Pt> {
    let r = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
    if r < 1.0 {
        return vec![p(fx, fy), p(tx, ty)];
    }
    let a0 = (fy - cy).atan2(fx - cx);
    let a1 = (ty - cy).atan2(tx - cx);
    let tau = 2.0 * PI;
    // Sweep magnitude in the chosen direction, in (0, 2π]; coincident ⇒ full turn.
    let mut sweep = if ccw { a1 - a0 } else { a0 - a1 };
    while sweep <= 0.0 {
        sweep += tau;
    }
    // Step from the sagitta tolerance.
    let max_step = 2.0 * (1.0 - (SAG_TOL_NM / r).min(1.0)).acos();
    // Floor the segment count in proportion to sweep so a tiny-radius arc keeps real
    // area instead of collapsing to a doubled-back sliver (#236): at r <= the µm
    // tolerance the step saturates at π (n=2, zero area). A full turn gets at least
    // CIRCLE_SEGMENTS, matching flash-circle fidelity.
    let floor = (sweep / (2.0 * PI) * CIRCLE_SEGMENTS as f64).ceil() as usize;
    let n = ((sweep / max_step).ceil() as usize)
        .max(floor)
        .clamp(2, 4096);
    (0..=n)
        .map(|k| {
            let frac = k as f64 / n as f64;
            let a = if ccw {
                a0 + sweep * frac
            } else {
                a0 - sweep * frac
            };
            p(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Resolve the arc centre for **G74 single-quadrant** mode. The `i`/`j` offsets
/// are magnitudes (unsigned per RS-274X), so the true centre is one of the four
/// `(fx ± i, fy ± j)` corners. The valid corner is the one that places the end
/// point on the same circle as the start (an equidistant centre) **and** whose
/// directed sweep (CCW/CW) is at most 90°. Returns that centre, or `None` when no
/// corner satisfies both — an inconsistent arc the caller must reject loud rather
/// than render with a guessed centre (the trust bar).
pub fn single_quadrant_center(
    fx: f64,
    fy: f64,
    tx: f64,
    ty: f64,
    i: f64,
    j: f64,
    ccw: bool,
) -> Option<(f64, f64)> {
    // Start radius is fixed by the offsets and identical for all four corners.
    let r_start = i.hypot(j);
    // End-point-on-circle tolerance: coordinates and offsets are quantized to nm,
    // so allow a little slack; wrong corners miss by order-of-radius, not nm.
    let r_tol = (r_start * 1e-3).max(2.0);
    // A true 90° arc can round just over π/2; permit a hair over.
    let sweep_max = PI / 2.0 + 1e-3;
    let tau = 2.0 * PI;

    let mut best: Option<((f64, f64), f64)> = None;
    for (si, sj) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let (cx, cy) = (fx + si * i, fy + sj * j);
        let r_mis = ((tx - cx).hypot(ty - cy) - r_start).abs();
        if r_mis > r_tol {
            continue; // end point isn't on this candidate circle
        }
        let a0 = (fy - cy).atan2(fx - cx);
        let a1 = (ty - cy).atan2(tx - cx);
        let mut sweep = if ccw { a1 - a0 } else { a0 - a1 };
        while sweep < 0.0 {
            sweep += tau;
        }
        if sweep > sweep_max {
            continue; // single-quadrant arcs never exceed 90°
        }
        match best {
            Some((_, m)) if r_mis >= m => {}
            _ => best = Some(((cx, cy), r_mis)),
        }
    }
    best.map(|(c, _)| c)
}

/// Rotate `(x, y)` by `deg` degrees CCW about the origin.
pub fn rotate(x: f64, y: f64, deg: f64) -> (f64, f64) {
    if deg == 0.0 {
        return (x, y);
    }
    let r = deg * PI / 180.0;
    let (s, c) = r.sin_cos();
    (x * c - y * s, x * s + y * c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::PolygonSet;
    use proptest::prelude::*;

    fn area(c: Contour) -> f64 {
        PolygonSet::new(vec![vec![c]]).area_nm2() as f64
    }

    /// All tessellated points lie on the defining circle, within ~2× the sagitta
    /// tolerance (rounding to integer nm costs up to ~1 nm per coordinate).
    fn on_radius(pts: &[Pt], cx: f64, cy: f64, r: f64) -> bool {
        pts.iter().all(|p| {
            let d = (((p.x as f64) - cx).powi(2) + ((p.y as f64) - cy).powi(2)).sqrt();
            (d - r).abs() < 2.0 * SAG_TOL_NM
        })
    }

    #[test]
    fn ngon_area_close_to_circle() {
        let r = 250_000.0; // 0.25 mm
        let got = area(ngon(0.0, 0.0, r));
        let ideal = PI * r * r;
        assert!(
            (got / ideal - 1.0).abs() < 0.01,
            "ngon area {got} vs {ideal}"
        );
    }

    #[test]
    fn stadium_area_matches_rect_plus_disk() {
        // Horizontal capsule length L between caps, radius r: rect 2r×L + π r².
        let (l, r) = (1_000_000.0, 100_000.0);
        let got = area(stadium(0.0, 0.0, l, 0.0, r));
        let ideal = 2.0 * r * l + PI * r * r;
        assert!(
            (got / ideal - 1.0).abs() < 0.01,
            "stadium area {got} vs {ideal}"
        );
    }

    #[test]
    fn obround_is_positive_and_bounded() {
        let a = area(obround(0.0, 0.0, 700_000.0, 250_000.0));
        let bbox = 700_000.0 * 250_000.0;
        assert!(a > 0.0 && a < bbox, "obround area {a} vs bbox {bbox}");
    }

    #[test]
    fn tiny_full_circle_arc_keeps_area() {
        // #236: at a radius <= the µm sagitta tolerance the step saturates at π, so
        // with no segment floor a full-circle arc collapsed to 3 collinear points →
        // zero enclosed area (a silent miss). The sweep-proportional floor keeps
        // CIRCLE_SEGMENTS for a full turn, matching flash-circle fidelity, so a real
        // circle survives.
        let r = 800.0; // nm, below SAG_TOL_NM (1 µm) — the collapse regime
        let pts = arc_points(r, 0.0, r, 0.0, 0.0, 0.0, true);
        assert!(
            pts.len() > 16,
            "tiny circle collapsed to {} points",
            pts.len()
        );
        let a = area(pts);
        let ideal = PI * r * r;
        assert!(
            (a / ideal - 1.0).abs() < 0.05,
            "tiny-circle-arc area {a} vs {ideal}"
        );
    }

    #[test]
    fn single_quadrant_center_picks_valid_corner() {
        // #234: G74 I/J are unsigned magnitudes; the true centre is one of the four
        // (f ± i, f ± j) corners — the one that puts the end point on the same
        // circle and whose directed sweep is <= 90°. A 45°→90° CCW arc (both offsets
        // non-zero): centre at the origin, only the (-i, -j) corner qualifies.
        let r = 1_000_000.0;
        let a0 = PI / 4.0; // 45°
        let (fx, fy) = (r * a0.cos(), r * a0.sin());
        let (tx, ty) = (0.0, r); // 90°
        let (i, j) = (fx.abs(), fy.abs()); // unsigned offsets to the origin
        let c = single_quadrant_center(fx, fy, tx, ty, i, j, true)
            .expect("a valid <=90° single-quadrant centre exists");
        assert!(
            c.0.abs() < 1.0 && c.1.abs() < 1.0,
            "single-quadrant centre {c:?} should be ~(0,0)"
        );
        // No corner yields a valid <=90° arc for a bogus (too-far) endpoint.
        assert!(
            single_quadrant_center(fx, fy, 5.0 * r, 5.0 * r, i, j, true).is_none(),
            "inconsistent single-quadrant arc must be rejected, not guessed"
        );
    }

    #[test]
    fn full_circle_arc_closes() {
        // from==to about a center ⇒ full circle. At a 1 mm radius the µm-sagitta
        // tolerance forces many segments; the tessellated loop area ≈ π r².
        let r = 1_000_000.0;
        let pts = arc_points(r, 0.0, r, 0.0, 0.0, 0.0, true);
        assert!(pts.len() > 30, "got {} points", pts.len());
        let a = area(pts);
        let ideal = PI * r * r;
        assert!(
            (a / ideal - 1.0).abs() < 0.01,
            "circle-arc area {a} vs {ideal}"
        );
    }

    // ---- #89: every builder emits CCW (positive signed area) ----
    // The whole NonZero diff scheme rests on "dark copper is wound CCW", asserted
    // nowhere before this. A future CW builder would silently reintroduce the #48
    // notch on non-macro paths, so pin the invariant as a property across the
    // builders' input space. `area` is the signed shoelace (CCW > 0).
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn ngon_is_ccw(cx in -1e8..1e8f64, cy in -1e8..1e8f64, r in 1e3..1e7f64) {
            prop_assert!(area(ngon(cx, cy, r)) > 0.0);
        }

        #[test]
        fn rect_is_ccw(
            cx in -1e8..1e8f64, cy in -1e8..1e8f64, w in 1e3..1e7f64, h in 1e3..1e7f64,
        ) {
            prop_assert!(area(rect(cx, cy, w, h)) > 0.0);
        }

        #[test]
        fn rect_rot_is_ccw(
            cx in -1e8..1e8f64, cy in -1e8..1e8f64,
            w in 1e3..1e7f64, h in 1e3..1e7f64, deg in 0.0..360.0f64,
        ) {
            prop_assert!(area(rect_rot(cx, cy, w, h, deg)) > 0.0);
        }

        #[test]
        fn stadium_is_ccw(
            ax in -1e7..1e7f64, ay in -1e7..1e7f64,
            bx in -1e7..1e7f64, by in -1e7..1e7f64, r in 1e3..1e6f64,
        ) {
            prop_assert!(area(stadium(ax, ay, bx, by, r)) > 0.0);
        }

        #[test]
        fn obround_is_ccw(
            cx in -1e8..1e8f64, cy in -1e8..1e8f64, w in 1e3..1e7f64, h in 1e3..1e7f64,
        ) {
            prop_assert!(area(obround(cx, cy, w, h)) > 0.0);
        }
    }

    // ---- #90: arc tessellation — multi-quadrant, on-radius, correct direction ----
    #[test]
    fn arc_quarter_half_threequarter_on_radius() {
        let r = 1_000_000.0;
        // 90° CCW: (r,0) -> (0,r).
        let q = arc_points(r, 0.0, 0.0, r, 0.0, 0.0, true);
        assert!(on_radius(&q, 0.0, 0.0, r), "90° off radius");
        let last = *q.last().unwrap();
        assert!(
            (last.x as f64).abs() < 2.0 && (last.y as f64 - r).abs() < 2.0,
            "90° endpoint {last:?} should be ~(0, r)"
        );
        // 180° CCW: (r,0) -> (-r,0).
        let h = arc_points(r, 0.0, -r, 0.0, 0.0, 0.0, true);
        assert!(on_radius(&h, 0.0, 0.0, r), "180° off radius");
        // 270° CCW: (r,0) -> (0,-r) the long way; more points than 90°.
        let t = arc_points(r, 0.0, 0.0, -r, 0.0, 0.0, true);
        assert!(on_radius(&t, 0.0, 0.0, r), "270° off radius");
        assert!(t.len() > q.len(), "270° should tessellate more than 90°");
    }

    #[test]
    fn arc_direction_is_honoured() {
        let r = 1_000_000.0;
        // Same endpoints (r,0)->(0,r): CCW is the short 90°, CW the long 270°.
        let ccw = arc_points(r, 0.0, 0.0, r, 0.0, 0.0, true);
        let cw = arc_points(r, 0.0, 0.0, r, 0.0, 0.0, false);
        assert!(
            cw.len() > ccw.len(),
            "CW (long way) should have more points"
        );
        assert!(on_radius(&ccw, 0.0, 0.0, r) && on_radius(&cw, 0.0, 0.0, r));
        // CCW midpoint sits in the +x/+y quadrant; CW goes the long way through -y.
        let cm = ccw[ccw.len() / 2];
        assert!(cm.x > 0 && cm.y > 0, "CCW midpoint {cm:?} should be +x/+y");
        let wm = cw[cw.len() / 2];
        assert!(wm.y < 0, "CW midpoint {wm:?} should pass through -y");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Any arc (any centre, radius, endpoints on the circle, direction) keeps
        /// every tessellated point on the defining circle.
        #[test]
        fn arc_points_lie_on_radius(
            cx in -1e7..1e7f64, cy in -1e7..1e7f64,
            r in 1e4..5e6f64,
            a0 in 0.0..std::f64::consts::TAU,
            a1 in 0.0..std::f64::consts::TAU,
            ccw in any::<bool>(),
        ) {
            let (fx, fy) = (cx + r * a0.cos(), cy + r * a0.sin());
            let (tx, ty) = (cx + r * a1.cos(), cy + r * a1.sin());
            let pts = arc_points(fx, fy, tx, ty, cx, cy, ccw);
            for p in &pts {
                let d = (((p.x as f64) - cx).powi(2) + ((p.y as f64) - cy).powi(2)).sqrt();
                prop_assert!((d - r).abs() < 2.0 * SAG_TOL_NM, "point {p:?} off radius {r}: d={d}");
            }
        }
    }
}
