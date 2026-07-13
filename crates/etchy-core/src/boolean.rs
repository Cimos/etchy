//! The one place i_overlay's integer engine is invoked. Confines i_overlay's
//! concrete types here; everyone else works in our [`Contour`]/[`PolygonSet`].

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::Overlay;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::i_shape::int::shape::{IntContour, IntShape, IntShapes};
use i_triangle::int::triangulatable::IntTriangulatable;

use crate::geo::{Contour, PolygonSet, Pt, Shape};

fn to_int(contours: &[Contour]) -> Vec<IntContour<i64>> {
    contours
        .iter()
        .map(|c| c.iter().map(|p| IntPoint::new(p.x, p.y)).collect())
        .collect()
}

fn from_int(shapes: IntShapes<i64>) -> PolygonSet {
    PolygonSet::new(
        shapes
            .into_iter()
            .map(|shape| {
                shape
                    .into_iter()
                    .map(|c| {
                        c.into_iter()
                            .map(|p| Pt::new(p.x, p.y))
                            .collect::<Contour>()
                    })
                    .collect::<Shape>()
            })
            .collect(),
    )
}

/// `subj − clip`, resolving overlaps/winding with NonZero. With an empty `clip`
/// this is just the NonZero union of `subj` (self-overlaps merged, holes via CW
/// winding) — which is how a layer's `dark` set is unioned and `clear` subtracted
/// in one pass.
pub(crate) fn difference(subj: &[Contour], clip: &[Contour]) -> PolygonSet {
    let s = to_int(subj);
    let c = to_int(clip);
    from_int(
        Overlay::<i64>::with_contours(&s, &c).overlay(OverlayRule::Difference, FillRule::NonZero),
    )
}

/// Triangulate every shape in the set (each outer ring with its holes
/// subtracted) into one flat triangle list (nm), for filled rendering.
pub(crate) fn triangulate_set(ps: &PolygonSet) -> Vec<[Pt; 3]> {
    ps.shapes
        .iter()
        .flat_map(|s| triangulate_shape(s))
        .collect()
}

/// Triangulate one shape — its outer ring with **holes subtracted** — into a flat
/// triangle list (nm). Uses i_triangle's integer monotone triangulator over an
/// `IntShape` (outer CCW, holes CW, NonZero fill): concave-correct, holes cut, and
/// no Steiner points for simple polygons so coordinates stay exact.
pub(crate) fn triangulate_shape(shape: &[Contour]) -> Vec<[Pt; 3]> {
    match shape.first() {
        Some(outer) if outer.len() >= 3 => {}
        _ => return Vec::new(),
    }
    let int_shape: IntShape<i64> = shape
        .iter()
        .map(|c| c.iter().map(|p| IntPoint::new(p.x, p.y)).collect())
        .collect();
    int_shape
        .triangulate()
        .into_triangulation::<u32>()
        .triangles()
        .map(|[a, b, c]| [Pt::new(a.x, a.y), Pt::new(b.x, b.y), Pt::new(c.x, c.y)])
        .collect()
}

/// NonZero union of `a` and `b` — accumulates copper across a dark polarity span
/// (self-overlapping flashes/traces merge; the running result is fed back in).
pub(crate) fn union(a: &[Contour], b: &[Contour]) -> PolygonSet {
    let s = to_int(a);
    let c = to_int(b);
    from_int(Overlay::<i64>::with_contours(&s, &c).overlay(OverlayRule::Union, FillRule::NonZero))
}

/// Flatten a [`PolygonSet`] back to a flat contour list (outer + holes), e.g. to
/// feed a resolved layer into another boolean op.
pub(crate) fn flatten(ps: &PolygonSet) -> Vec<Contour> {
    ps.shapes.iter().flat_map(|s| s.iter().cloned()).collect()
}

/// Fill a region's boundary loops with the **even-odd** rule and return the
/// normalised (CCW outer / CW hole) contours. Even-odd makes the fill independent
/// of how the exporter wound each loop, and turns nested loops into proper holes —
/// the correct semantics for a Gerber `G36/G37` region.
pub(crate) fn fill_even_odd(contours: &[Contour]) -> Vec<Contour> {
    let s = to_int(contours);
    let out =
        Overlay::<i64>::with_contours(&s, &[]).overlay(OverlayRule::Subject, FillRule::EvenOdd);
    flatten(&from_int(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned CCW square `[x0,x1] × [y0,y1]` (positive winding).
    fn sq(x0: i64, y0: i64, x1: i64, y1: i64) -> Contour {
        vec![
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ]
    }

    /// Total (unsigned) area of a flat triangle list, in nm² — via the integer
    /// cross product, summed and halved.
    fn tri_area(ts: &[[Pt; 3]]) -> i128 {
        let doubled: i128 = ts
            .iter()
            .map(|[a, b, c]| {
                let ux = (b.x - a.x) as i128;
                let uy = (b.y - a.y) as i128;
                let vx = (c.x - a.x) as i128;
                let vy = (c.y - a.y) as i128;
                (ux * vy - uy * vx).abs()
            })
            .sum();
        doubled / 2
    }

    #[test]
    fn union_merges_overlap_not_double_counts() {
        // Two 10×10 squares overlapping in a 5×10 strip. The union is one shape of
        // area 150 (100 + 100 − 50), NOT the 200 a naive sum would give.
        let a = sq(0, 0, 10, 10);
        let b = sq(5, 0, 15, 10);
        let u = union(&[a], &[b]);
        assert_eq!(
            u.shapes.len(),
            1,
            "overlapping squares merge into one region"
        );
        assert_eq!(
            u.area_nm2(),
            150,
            "union area must not double-count overlap"
        );
    }

    #[test]
    fn union_keeps_disjoint_regions_separate() {
        // Far-apart squares stay two regions, area summed.
        let u = union(&[sq(0, 0, 10, 10)], &[sq(100, 100, 110, 110)]);
        assert_eq!(u.shapes.len(), 2);
        assert_eq!(u.area_nm2(), 200);
    }

    #[test]
    fn union_self_overlap_via_empty_clip() {
        // `union(a, &[])` is the self-merge path (the running dark-span accumulator):
        // two coincident squares collapse to one, counted once.
        let u = union(&[sq(0, 0, 10, 10), sq(0, 0, 10, 10)], &[]);
        assert_eq!(u.shapes.len(), 1);
        assert_eq!(u.area_nm2(), 100);
    }

    #[test]
    fn difference_subtracts_clip() {
        // 10×10 minus a 5×10 clip on the right → a 5×10 rectangle (area 50).
        let d = difference(&[sq(0, 0, 10, 10)], &[sq(5, 0, 10, 10)]);
        assert_eq!(d.area_nm2(), 50);
    }

    #[test]
    fn difference_disjoint_clip_is_noop() {
        // A clip that touches nothing leaves the subject whole.
        let d = difference(&[sq(0, 0, 10, 10)], &[sq(100, 100, 110, 110)]);
        assert_eq!(d.area_nm2(), 100);
    }

    #[test]
    fn difference_enclosing_clip_empties() {
        // Subtracting a strictly larger clip erases the subject entirely.
        let d = difference(&[sq(0, 0, 10, 10)], &[sq(-1, -1, 11, 11)]);
        assert!(d.is_empty(), "fully-covered subject must vanish");
        assert_eq!(d.area_nm2(), 0);
    }

    #[test]
    fn triangulate_shape_subtracts_hole() {
        // Outer 10×10 (CCW) with a 6×6 hole (CW). Triangulated area = 100 − 36 = 64
        // — the hole is cut, not filled. A regression that ignored holes would
        // report 100 (solid disk for an annular pad — the silent miss this guards).
        let outer = sq(0, 0, 10, 10);
        let mut hole = sq(2, 2, 8, 8);
        hole.reverse(); // CW winding marks it a hole
        let ts = triangulate_shape(&[outer, hole]);
        assert_eq!(tri_area(&ts), 64, "hole must be subtracted from the fill");
    }

    #[test]
    fn triangulate_shape_solid_covers_full_area() {
        // No hole → triangles tile the whole square exactly.
        let ts = triangulate_shape(&[sq(0, 0, 10, 10)]);
        assert_eq!(tri_area(&ts), 100);
    }

    #[test]
    fn triangulate_shape_rejects_degenerate() {
        // Fewer than three points can't triangulate → empty, not a panic.
        assert!(triangulate_shape(&[]).is_empty());
        assert!(triangulate_shape(&[vec![Pt::new(0, 0), Pt::new(1, 0)]]).is_empty());
    }

    #[test]
    fn flatten_round_trips_outer_and_hole() {
        // A shape with a hole (built via difference) flattens to two contours; the
        // signed shoelace over them nets the holed area (outer − hole).
        let holed = difference(&[sq(0, 0, 10, 10)], &[sq(2, 2, 8, 8)]);
        let flat = flatten(&holed);
        assert_eq!(flat.len(), 2, "outer ring + one hole");
        // Rebuild a PolygonSet from the flat list and confirm the net area holds.
        let rebuilt = PolygonSet::new(vec![flat]);
        assert_eq!(rebuilt.area_nm2(), 64);
    }

    #[test]
    fn fill_even_odd_turns_nested_loop_into_hole() {
        // Two nested loops wound the SAME way. Even-odd fill treats the inner loop
        // as a hole regardless of winding (a G36 region with an island cut-out):
        // net area = 100 − 36 = 64.
        let outer = sq(0, 0, 10, 10);
        let inner = sq(2, 2, 8, 8); // same (CCW) winding as outer
        let filled = fill_even_odd(&[outer, inner]);
        let area = PolygonSet::new(vec![filled]).area_nm2();
        assert_eq!(area, 64, "nested same-wound loop must become a hole");
    }
}
