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
    let n = ((sweep / max_step).ceil() as usize).clamp(2, 4096);
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

    fn area(c: Contour) -> f64 {
        PolygonSet::new(vec![vec![c]]).area_nm2() as f64
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
}
