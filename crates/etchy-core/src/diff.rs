//! Per-layer boolean diff (`added = B − A`, `removed = A − B`) and the change
//! magnitudes CI thresholds read. The boolean engine lives in `crate::boolean`;
//! this module is the diff semantics + measurement on top of it.
//!
//! One computation feeds all three views (overlay, heatmap, magnitudes — see
//! DEVELOPER_GUIDE "one computation, three views").

use crate::boolean;
use crate::geo::{shape_area_nm2, PolygonSet, Shape};

/// Region-count noise floor (nm²): a diff region smaller than this is a sub-feature
/// sliver — a sub-µm rim produced along near-coincident edges — whose *number* swings
/// with tessellation granularity (a fixed 64-gon over-produces them). It's excluded
/// from the reported region count so that count is stable and meaningful (#94 research
/// found the raw count moved ~40% purely with tessellation). ~1e-4 mm² is far below
/// any real PCB feature and far above the slivers. Change-detection still uses raw
/// presence (`has_added`/`has_removed`), so a sub-floor sliver is never a silent miss.
const REGION_NOISE_FLOOR_NM2: i128 = 100_000_000; // 1e-4 mm²

/// Magnitudes for one layer's change. Areas are exact integer nm² (`i128` avoids
/// overflow on big pours); mm² is derived for humans / CI thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerChange {
    pub added_area_nm2: i128,
    pub removed_area_nm2: i128,
    /// Reported region counts — only regions above [`REGION_NOISE_FLOOR_NM2`], so the
    /// number is tessellation-robust (#94).
    pub added_region_count: u32,
    pub removed_region_count: u32,
    /// Raw presence of ANY diff geometry (even sub-floor slivers), kept separate so
    /// change-detection can't miss a tiny real change (the trust bar).
    pub has_added: bool,
    pub has_removed: bool,
}

impl LayerChange {
    /// True when nothing changed on this layer. Uses raw presence, not the (noise-
    /// floored) region count: a sub-floor sliver truncates to zero reported area and
    /// count, but `has_*` still flags it as changed (no silent miss — the trust bar).
    pub fn is_unchanged(&self) -> bool {
        self.added_area_nm2 == 0
            && self.removed_area_nm2 == 0
            && !self.has_added
            && !self.has_removed
    }
    pub fn added_area_mm2(&self) -> f64 {
        nm2_to_mm2(self.added_area_nm2)
    }
    pub fn removed_area_mm2(&self) -> f64 {
        nm2_to_mm2(self.removed_area_nm2)
    }
}

/// One connected component of a boolean difference. The same area floor used by
/// `LayerChange` determines membership, so the list and count cannot diverge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedRegion<'a> {
    pub area_nm2: i128,
    pub bbox_nm: [i64; 4],
    pub centroid_nm: [i64; 2],
    pub shape: &'a Shape,
}

fn regions(set: &PolygonSet) -> Vec<ChangedRegion<'_>> {
    let mut out: Vec<_> = set
        .shapes
        .iter()
        .filter_map(|shape| {
            let area_nm2 = shape_area_nm2(shape).abs();
            if area_nm2 < REGION_NOISE_FLOOR_NM2 {
                return None;
            }
            let first = shape.iter().flat_map(|ring| ring.iter()).next()?;
            let mut bbox_nm = [first.x, first.y, first.x, first.y];
            for point in shape.iter().flat_map(|ring| ring.iter()) {
                bbox_nm[0] = bbox_nm[0].min(point.x);
                bbox_nm[1] = bbox_nm[1].min(point.y);
                bbox_nm[2] = bbox_nm[2].max(point.x);
                bbox_nm[3] = bbox_nm[3].max(point.y);
            }
            // Compute the filled polygon centroid (holes subtract). Translate first
            // to avoid cancellation at large absolute board coordinates.
            let mut twice_area = 0.0;
            let mut cx_sum = 0.0;
            let mut cy_sum = 0.0;
            for ring in shape {
                for (a, b) in ring
                    .iter()
                    .zip(ring.iter().cycle().skip(1))
                    .take(ring.len())
                {
                    let ax = (a.x - bbox_nm[0]) as f64;
                    let ay = (a.y - bbox_nm[1]) as f64;
                    let bx = (b.x - bbox_nm[0]) as f64;
                    let by = (b.y - bbox_nm[1]) as f64;
                    let cross = ax * by - bx * ay;
                    twice_area += cross;
                    cx_sum += (ax + bx) * cross;
                    cy_sum += (ay + by) * cross;
                }
            }
            let centroid_nm = if twice_area == 0.0 {
                [
                    (bbox_nm[0] / 2 + bbox_nm[2] / 2),
                    (bbox_nm[1] / 2 + bbox_nm[3] / 2),
                ]
            } else {
                [
                    bbox_nm[0] + (cx_sum / (3.0 * twice_area)).round() as i64,
                    bbox_nm[1] + (cy_sum / (3.0 * twice_area)).round() as i64,
                ]
            };
            Some(ChangedRegion {
                area_nm2,
                bbox_nm,
                centroid_nm,
                shape,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.area_nm2
            .cmp(&a.area_nm2)
            .then_with(|| a.bbox_nm.cmp(&b.bbox_nm))
    });
    out
}

/// One layer-pair's diff: the two difference polygon sets, from a single pair of
/// boolean computations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerDiff {
    pub added: PolygonSet,
    pub removed: PolygonSet,
}

impl LayerDiff {
    pub fn reported_regions(&self) -> (Vec<ChangedRegion<'_>>, Vec<ChangedRegion<'_>>) {
        (regions(&self.added), regions(&self.removed))
    }
    pub fn measure(&self) -> LayerChange {
        let (added, removed) = self.reported_regions();
        self.measure_regions(&added, &removed)
    }
    pub fn measure_regions(
        &self,
        added: &[ChangedRegion<'_>],
        removed: &[ChangedRegion<'_>],
    ) -> LayerChange {
        LayerChange {
            added_area_nm2: self.added.area_nm2(),
            removed_area_nm2: self.removed.area_nm2(),
            // Reported counts exclude sub-floor slivers (tessellation-robust, #94)...
            added_region_count: added.len() as u32,
            removed_region_count: removed.len() as u32,
            // ...but raw presence still flags any change, so nothing is silently missed.
            has_added: !self.added.is_empty(),
            has_removed: !self.removed.is_empty(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
    pub fn added_regions(&self) -> usize {
        self.added.shapes.len()
    }
    pub fn removed_regions(&self) -> usize {
        self.removed.shapes.len()
    }
    /// Added area in mm² (convenience for tests / display).
    pub fn added_area(&self) -> f64 {
        self.added.area_mm2()
    }
    /// Removed area in mm² (convenience for tests / display).
    pub fn removed_area(&self) -> f64 {
        self.removed.area_mm2()
    }
}

/// Diff two filled layers: `removed = A − B`, `added = B − A`. Deterministic — a
/// pure function of the integer input (no floating-point engine internals).
pub fn diff_layer(a: &PolygonSet, b: &PolygonSet) -> LayerDiff {
    let ac = boolean::flatten(a);
    let bc = boolean::flatten(b);
    LayerDiff {
        removed: boolean::difference(&ac, &bc),
        added: boolean::difference(&bc, &ac),
    }
}

/// nm² → mm² (1 mm² = 1e12 nm²).
pub fn nm2_to_mm2(nm2: i128) -> f64 {
    nm2 as f64 / 1.0e12
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::{Pt, Shape};

    fn square(x0: i64, y0: i64, s: i64) -> Shape {
        vec![vec![
            Pt::new(x0, y0),
            Pt::new(x0 + s, y0),
            Pt::new(x0 + s, y0 + s),
            Pt::new(x0, y0 + s),
        ]]
    }

    #[test]
    fn regions_are_area_sorted_with_exact_bounds_and_centroid() {
        let set = PolygonSet::new(vec![
            square(5_000_000, 0, 1_000_000),
            square(0, 0, 2_000_000),
        ]);
        let d = LayerDiff {
            added: set,
            removed: PolygonSet::default(),
        };
        let (added, removed) = d.reported_regions();
        assert!(removed.is_empty());
        assert_eq!(d.measure_regions(&added, &removed).added_region_count, 2);
        assert_eq!(added[0].area_nm2, 4_000_000_000_000);
        assert_eq!(added[0].bbox_nm, [0, 0, 2_000_000, 2_000_000]);
        assert_eq!(added[0].centroid_nm, [1_000_000, 1_000_000]);
        assert_eq!(added[1].area_nm2, 1_000_000_000_000);
    }

    #[test]
    fn difference_strip_area() {
        // A = 10x10 at origin, B = 10x10 shifted by 5 in x → removed/added strips 5x10.
        let a = PolygonSet::new(vec![square(0, 0, 10)]);
        let b = PolygonSet::new(vec![square(5, 0, 10)]);
        let d = diff_layer(&a, &b);
        assert_eq!(d.removed.area_nm2(), 50);
        assert_eq!(d.added.area_nm2(), 50);
        assert_eq!(d.removed_regions(), 1);
        assert_eq!(d.added_regions(), 1);
    }

    #[test]
    fn self_diff_empty() {
        let a = PolygonSet::new(vec![square(0, 0, 1_000_000)]);
        assert!(diff_layer(&a, &a).is_empty());
    }
}
