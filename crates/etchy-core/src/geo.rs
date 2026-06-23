//! Fixed-point geometry — the deterministic substrate every front-end and the
//! diff share.
//!
//! Coordinates are integers on a **1 nm grid** (`GRID_NM`). Gerber/Excellon floats
//! are snapped to this grid exactly once, at the front-end boundary
//! ([`quantize_mm`]), so the diff is a pure function of integer input — determinism
//! is a property of *our* code, not of a floating-point engine's internals
//! (see `docs/SPIKE_1.md` "coordinate snap" and `docs/M1_ENGINE_DESIGN.md`).

use crate::error::GeoError;

/// Nanometres per fixed-point unit — the grid Gerber/Excellon floats snap to.
pub const GRID_NM: i64 = 1;

/// Nanometres per millimetre.
pub const NM_PER_MM: i64 = 1_000_000;

/// Largest absolute coordinate we accept, in nm (= ±100 m — absurdly beyond any
/// real board, yet small enough that every downstream `i64` operation is safe:
/// coordinate *differences* (≤ 2·MAX) stay well inside `i64`, i_overlay's internal
/// cross-products fit, and `i128` shoelace sums have vast headroom. Guards the
/// `f64 -> i64` cast against silent saturation **and** against feeding i_overlay a
/// value whose differences would overflow its `i64` engine (a silent-miss hazard).
const MAX_ABS_NM: f64 = 1.0e14;

/// Quantize a millimetre coordinate onto the integer nm grid. **Guarded**: a
/// non-finite or out-of-range value is a loud error, never a saturating cast.
pub fn quantize_mm(mm: f64) -> Result<i64, GeoError> {
    if !mm.is_finite() {
        return Err(GeoError::NonFiniteCoord);
    }
    let nm = (mm * NM_PER_MM as f64 / GRID_NM as f64).round();
    if nm.abs() > MAX_ABS_NM {
        return Err(GeoError::CoordOutOfRange { mm });
    }
    Ok(nm as i64)
}

/// Snap an **already-in-nm** float onto the integer grid, guarded exactly like
/// [`quantize_mm`]: a non-finite or out-of-range value fails loud rather than
/// silently saturating the `f64 -> i64` cast. Use for coordinates/dimensions that
/// are computed in nm (e.g. aperture-macro primitives) and so never pass through
/// `quantize_mm`'s mm-space guard.
pub fn snap_nm(nm: f64) -> Result<i64, GeoError> {
    if !nm.is_finite() {
        return Err(GeoError::NonFiniteCoord);
    }
    if nm.abs() > MAX_ABS_NM {
        return Err(GeoError::CoordOutOfRange {
            mm: nm / NM_PER_MM as f64,
        });
    }
    Ok(nm.round() as i64)
}

/// Image polarity: dark adds copper, clear removes it (LPD/LPC, and macro
/// primitive exposure). A layer's filled geometry is `dark − clear`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Dark,
    Clear,
}

/// A point in fixed-point nanometres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pt {
    pub x: i64,
    pub y: i64,
}

impl Pt {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

/// Stroke / flash shape. Minimal set (circle + rect); macros and more shapes
/// arrive in later increments. Dimensions are in nm (already quantized).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aperture {
    Circle { diameter_nm: i64 },
    Rect { w_nm: i64, h_nm: i64 },
}

/// A drawn graphic object — the format-agnostic IR both the Gerber and (future)
/// Excellon front-ends emit and the polygonizer consumes. Only `Flash` is
/// rendered in this increment; the other variants are the documented growth seam.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Line {
        from: Pt,
        to: Pt,
        aperture: Aperture,
    },
    Arc {
        from: Pt,
        to: Pt,
        center: Pt,
        cw: bool,
        aperture: Aperture,
    },
    Flash {
        at: Pt,
        aperture: Aperture,
    },
    Region {
        outline: Vec<Pt>,
        holes: Vec<Vec<Pt>>,
    },
}

/// A single closed ring of points (nm).
pub type Contour = Vec<Pt>;

/// One filled shape: `[outer, hole, hole, …]` — the i_overlay convention (CCW
/// outer first, CW holes). Kept as our own type so i_overlay's concrete types
/// never leak across the public API.
pub type Shape = Vec<Contour>;

/// The filled geometry of a layer (or a diff result): a set of disjoint shapes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolygonSet {
    pub shapes: Vec<Shape>,
}

impl PolygonSet {
    pub fn new(shapes: Vec<Shape>) -> Self {
        Self { shapes }
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Number of disjoint filled regions (top-level shapes). In this increment
    /// this is the i_overlay shape count; heatmap clustering refines it later.
    pub fn region_count(&self) -> u32 {
        self.shapes.len() as u32
    }

    /// Exact filled area in nm², via integer shoelace summed over every contour
    /// (CCW outer adds, CW holes subtract — the signed sum is the net filled area,
    /// independent of contour order).
    ///
    /// Correct for **non-overlapping, correctly-wound** shapes — which is exactly
    /// what `i_overlay` returns from a diff (the only geometry this is measured on
    /// in the live pipeline). On *raw* polygonized geometry (pre-diff), coincident
    /// flashes are separate overlapping shapes and would be double-counted; don't
    /// measure that — union it through the diff first.
    pub fn area_nm2(&self) -> i128 {
        let doubled: i128 = self
            .shapes
            .iter()
            .flat_map(|s| s.iter())
            .map(|c| shoelace_2x_nm2(c))
            .sum();
        doubled / 2
    }

    /// Filled area in mm² (derived, lossy — for humans / CI thresholds).
    pub fn area_mm2(&self) -> f64 {
        self.area_nm2() as f64 / (NM_PER_MM as f64 * NM_PER_MM as f64)
    }

    /// Triangulate every shape (outer ring with **holes subtracted**) into a flat
    /// list of triangles (nm), for filled rendering. Concave-correct — unlike
    /// egui's vertex-0 fan, which only tiles convex polygons and throws spurious
    /// "spike" triangles across the concave copper shapes a PCB is full of.
    pub fn triangulate(&self) -> Vec<[Pt; 3]> {
        crate::boolean::triangulate_set(self)
    }

    /// Axis-aligned bounding box `[min_x, min_y, max_x, max_y]` in nm, or `None`
    /// if empty.
    pub fn bbox_nm(&self) -> Option<[i64; 4]> {
        let mut it = self
            .shapes
            .iter()
            .flat_map(|s| s.iter())
            .flat_map(|c| c.iter());
        let first = it.next()?;
        let mut bb = [first.x, first.y, first.x, first.y];
        for p in self
            .shapes
            .iter()
            .flat_map(|s| s.iter())
            .flat_map(|c| c.iter())
        {
            bb[0] = bb[0].min(p.x);
            bb[1] = bb[1].min(p.y);
            bb[2] = bb[2].max(p.x);
            bb[3] = bb[3].max(p.y);
        }
        Some(bb)
    }
}

/// Triangulate a full shape — its outer ring **with holes subtracted** — into a
/// flat triangle list (nm). For per-shape filled rendering of annular pads and
/// pour cut-outs without rebuilding a [`PolygonSet`]. Concave-correct.
pub fn triangulate_shape(shape: &Shape) -> Vec<[Pt; 3]> {
    crate::boolean::triangulate_shape(shape)
}

/// Twice the signed area of a ring (shoelace), accumulated in `i128` so the
/// per-term products (up to ~nm·nm) and their sum never overflow. CCW > 0.
fn shoelace_2x_nm2(c: &[Pt]) -> i128 {
    let n = c.len();
    if n < 3 {
        return 0;
    }
    let mut s: i128 = 0;
    for i in 0..n {
        let p = c[i];
        let q = c[(i + 1) % n];
        s += (p.x as i128) * (q.y as i128) - (q.x as i128) * (p.y as i128);
    }
    s
}

/// Return `c` wound CCW (`ccw == true`) or CW (`ccw == false`), reversing it in
/// place if needed. Dark copper must be CCW and clearances CW so the NonZero
/// union/difference *adds* overlapping same-polarity primitives instead of
/// cancelling them: a CW-wound dark macro-outline pad would otherwise sum to
/// winding 0 where a CCW track overlaps it and punch a hole (the notch at a
/// track→pad junction). A degenerate (zero-area) ring is returned unchanged.
pub(crate) fn wind(mut c: Contour, ccw: bool) -> Contour {
    let a = shoelace_2x_nm2(&c);
    if a != 0 && (a > 0) != ccw {
        c.reverse();
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_guards_bad_input() {
        assert_eq!(quantize_mm(f64::NAN), Err(GeoError::NonFiniteCoord));
        assert_eq!(quantize_mm(f64::INFINITY), Err(GeoError::NonFiniteCoord));
        assert!(matches!(
            quantize_mm(1e16),
            Err(GeoError::CoordOutOfRange { .. })
        ));
        assert_eq!(quantize_mm(5.0), Ok(5_000_000));
        assert_eq!(quantize_mm(-0.5), Ok(-500_000));
    }

    #[test]
    fn snap_nm_guards_range_and_finite() {
        assert_eq!(snap_nm(5_000_000.0), Ok(5_000_000));
        assert_eq!(snap_nm(-1.4), Ok(-1));
        assert!(matches!(snap_nm(f64::NAN), Err(GeoError::NonFiniteCoord)));
        assert!(matches!(
            snap_nm(f64::INFINITY),
            Err(GeoError::NonFiniteCoord)
        ));
        // > MAX_ABS_NM (1e14) must fail loud, not saturate.
        assert!(matches!(
            snap_nm(2.0e14),
            Err(GeoError::CoordOutOfRange { .. })
        ));
    }

    #[test]
    fn quantize_cap_keeps_downstream_i64_safe() {
        // A value just past the cap (±100 m) is rejected; just under is accepted,
        // and 2× it (the worst-case coordinate difference) still fits i64.
        assert!(matches!(
            quantize_mm(200_000_000.0), // 2e8 mm = 2e14 nm > cap
            Err(GeoError::CoordOutOfRange { .. })
        ));
        let at_cap = quantize_mm(100_000_000.0).unwrap(); // 1e8 mm = 1e14 nm
        assert_eq!(at_cap, 100_000_000_000_000);
        assert!(
            at_cap.checked_mul(2).is_some(),
            "2x a valid coord must fit i64"
        );
    }

    #[test]
    fn area_at_large_valid_scale_does_not_overflow() {
        // A square at the coordinate cap: area = (1e14)² = 1e28 nm² — exact in i128.
        let s = 100_000_000_000_000; // 1e14 nm
        let sq: Shape = vec![vec![
            Pt::new(0, 0),
            Pt::new(s, 0),
            Pt::new(s, s),
            Pt::new(0, s),
        ]];
        assert_eq!(
            PolygonSet::new(vec![sq]).area_nm2(),
            10_000_000_000_000_000_000_000_000_000
        );
    }

    #[test]
    fn area_of_unit_square_mm() {
        // 1 mm square = 1e6 nm square -> area 1e12 nm² = 1 mm².
        let s = 1_000_000;
        let square: Shape = vec![vec![
            Pt::new(0, 0),
            Pt::new(s, 0),
            Pt::new(s, s),
            Pt::new(0, s),
        ]];
        let ps = PolygonSet::new(vec![square]);
        assert_eq!(ps.area_nm2(), 1_000_000_000_000);
        assert!((ps.area_mm2() - 1.0).abs() < 1e-9);
        assert_eq!(ps.region_count(), 1);
        assert_eq!(ps.bbox_nm(), Some([0, 0, s, s]));
    }

    #[test]
    fn triangulate_concave_tiles_exactly() {
        // An L-shape (concave hexagon, CCW). A vertex-0 fan would throw triangles
        // outside the shape (the rendering "spikes"); a correct triangulation tiles
        // the interior. Discriminator: the sum of *unsigned* triangle areas equals
        // the polygon area iff the triangles stay inside and don't overlap.
        let s = 1_000_000;
        let l: Shape = vec![vec![
            Pt::new(0, 0),
            Pt::new(2 * s, 0),
            Pt::new(2 * s, s),
            Pt::new(s, s),
            Pt::new(s, 2 * s),
            Pt::new(0, 2 * s),
        ]];
        let ps = PolygonSet::new(vec![l]);
        let tris = ps.triangulate();
        assert!(!tris.is_empty(), "expected a triangulation");
        let sum2: i128 = tris.iter().map(tri_abs_area2).sum();
        assert_eq!(
            sum2 / 2,
            ps.area_nm2(),
            "triangles must tile the polygon exactly (no spikes/overlap)"
        );
    }

    #[test]
    fn triangulate_shape_cuts_holes() {
        // A square with a square hole. The hole (CW) must be excluded from the
        // triangulation — outer-ring-only triangulation would fill it in.
        let s = 1_000_000;
        let h = 500_000;
        let shape: Shape = vec![
            vec![Pt::new(0, 0), Pt::new(s, 0), Pt::new(s, s), Pt::new(0, s)],
            vec![
                Pt::new(250_000, 250_000),
                Pt::new(250_000, 250_000 + h),
                Pt::new(250_000 + h, 250_000 + h),
                Pt::new(250_000 + h, 250_000),
            ],
        ];
        let tris = triangulate_shape(&shape);
        let sum2: i128 = tris.iter().map(tri_abs_area2).sum();
        // Net filled area = outer − hole; a hole-blind triangulation would equal
        // the full outer area instead.
        let net = PolygonSet::new(vec![shape.clone()]).area_nm2();
        assert_eq!(sum2 / 2, net, "triangulation must exclude the hole");
    }

    #[test]
    fn triangulate_antipad_no_triangle_inside_hole() {
        // #13 probe: an anti-pad (square copper with a square clearance hole — the
        // "square in a footprint" case). Beyond matching the net area, assert no
        // triangle's centroid lands inside the hole, so the hole reads as empty and
        // a hole-blind fan can't sneak a spike across the clearance.
        let s = 2_000_000;
        let (hx0, hy0, hx1, hy1) = (700_000, 700_000, 1_300_000, 1_300_000);
        let shape: Shape = vec![
            vec![Pt::new(0, 0), Pt::new(s, 0), Pt::new(s, s), Pt::new(0, s)],
            // hole wound CW (i_overlay hole convention)
            vec![
                Pt::new(hx0, hy0),
                Pt::new(hx0, hy1),
                Pt::new(hx1, hy1),
                Pt::new(hx1, hy0),
            ],
        ];
        let tris = triangulate_shape(&shape);
        assert!(!tris.is_empty(), "expected a triangulation");
        for t in &tris {
            let cx = (t[0].x + t[1].x + t[2].x) / 3;
            let cy = (t[0].y + t[1].y + t[2].y) / 3;
            assert!(
                !(cx > hx0 && cx < hx1 && cy > hy0 && cy < hy1),
                "triangle centroid ({cx},{cy}) fell inside the anti-pad hole"
            );
        }
        // And exact net area (outer − hole), as a second guard.
        let sum2: i128 = tris.iter().map(tri_abs_area2).sum();
        let net = PolygonSet::new(vec![shape.clone()]).area_nm2();
        assert_eq!(sum2 / 2, net, "triangulation must net out to outer − hole");
    }

    /// Twice the unsigned area of a triangle, in nm² (i128).
    fn tri_abs_area2(t: &[Pt; 3]) -> i128 {
        ((t[1].x as i128 - t[0].x as i128) * (t[2].y as i128 - t[0].y as i128)
            - (t[2].x as i128 - t[0].x as i128) * (t[1].y as i128 - t[0].y as i128))
            .abs()
    }

    #[test]
    fn square_with_hole_subtracts() {
        let s = 1_000_000;
        let h = 500_000;
        // outer CCW, hole CW
        let shape: Shape = vec![
            vec![Pt::new(0, 0), Pt::new(s, 0), Pt::new(s, s), Pt::new(0, s)],
            vec![
                Pt::new(250_000, 250_000),
                Pt::new(250_000, 250_000 + h),
                Pt::new(250_000 + h, 250_000 + h),
                Pt::new(250_000 + h, 250_000),
            ],
        ];
        let ps = PolygonSet::new(vec![shape]);
        // 1e12 - (5e5)^2 = 1e12 - 2.5e11
        assert_eq!(ps.area_nm2(), 1_000_000_000_000 - 250_000_000_000);
    }
}
