//! The one place i_overlay's integer engine is invoked. Confines i_overlay's
//! concrete types here; everyone else works in our [`Contour`]/[`PolygonSet`].

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::Overlay;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::i_shape::int::shape::{IntContour, IntShapes};
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

/// Triangulate each shape's **outer ring** into a flat triangle list (nm), for
/// filled rendering. Uses i_triangle's integer monotone triangulator — which
/// adds no Steiner points for a simple polygon, so coordinates stay exact — and
/// is concave-correct, unlike a vertex-0 fan. Holes are not yet subtracted.
pub(crate) fn triangulate_outer(ps: &PolygonSet) -> Vec<[Pt; 3]> {
    ps.shapes
        .iter()
        .filter_map(|s| s.first())
        .flat_map(|outer| triangulate_one(outer))
        .collect()
}

/// Triangulate a single closed ring (the outer contour) into a flat triangle
/// list (nm). i_triangle's integer monotone triangulator: concave-correct, and
/// adds no Steiner points for a simple polygon so coordinates stay exact.
pub(crate) fn triangulate_one(outer: &[Pt]) -> Vec<[Pt; 3]> {
    if outer.len() < 3 {
        return Vec::new();
    }
    let contour: IntContour<i64> = outer.iter().map(|p| IntPoint::new(p.x, p.y)).collect();
    contour
        .triangulate()
        .into_triangulation::<u32>()
        .triangles()
        .map(|[a, b, c]| [Pt::new(a.x, a.y), Pt::new(b.x, b.y), Pt::new(c.x, c.y)])
        .collect()
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
