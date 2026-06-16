//! Per-layer boolean diff (`added = B − A`, `removed = A − B`) and the change
//! magnitudes CI thresholds read. The boolean engine lives in `crate::boolean`;
//! this module is the diff semantics + measurement on top of it.
//!
//! One computation feeds all three views (overlay, heatmap, magnitudes — see
//! DEVELOPER_GUIDE "one computation, three views").

use crate::boolean;
use crate::geo::PolygonSet;

/// Magnitudes for one layer's change. Areas are exact integer nm² (`i128` avoids
/// overflow on big pours); mm² is derived for humans / CI thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerChange {
    pub added_area_nm2: i128,
    pub removed_area_nm2: i128,
    pub added_region_count: u32,
    pub removed_region_count: u32,
}

impl LayerChange {
    /// True when nothing changed on this layer. Checks region counts too, not just
    /// area: a sub-1-nm² sliver truncates to zero area under `area_nm2`'s integer
    /// `/2`, but a nonzero region count still means geometry changed (no silent miss).
    pub fn is_unchanged(&self) -> bool {
        self.added_area_nm2 == 0
            && self.removed_area_nm2 == 0
            && self.added_region_count == 0
            && self.removed_region_count == 0
    }
    pub fn added_area_mm2(&self) -> f64 {
        nm2_to_mm2(self.added_area_nm2)
    }
    pub fn removed_area_mm2(&self) -> f64 {
        nm2_to_mm2(self.removed_area_nm2)
    }
}

/// One layer-pair's diff: the two difference polygon sets, from a single pair of
/// boolean computations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerDiff {
    pub added: PolygonSet,
    pub removed: PolygonSet,
}

impl LayerDiff {
    pub fn measure(&self) -> LayerChange {
        LayerChange {
            added_area_nm2: self.added.area_nm2(),
            removed_area_nm2: self.removed.area_nm2(),
            added_region_count: self.added.region_count(),
            removed_region_count: self.removed.region_count(),
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
