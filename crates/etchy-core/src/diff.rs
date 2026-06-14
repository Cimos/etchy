//! Per-layer boolean diff (`added = B − A`, `removed = A − B`) and the change
//! measures CI thresholds read. Phase-0 scaffold: types only; the `i_overlay`
//! boolean implementation + region clustering land in Milestone 1.

/// Magnitudes for one layer's change — one computation feeds the visual overlay,
/// the heatmap, and these numbers (see DEVELOPER_GUIDE "one computation, three
/// views"). Areas are in fixed-point nm² (`i128` to avoid overflow on big pours).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerChange {
    pub added_area_nm2: i128,
    pub removed_area_nm2: i128,
    pub region_count: u32,
}

impl LayerChange {
    /// True when nothing changed on this layer.
    pub fn is_unchanged(&self) -> bool {
        self.added_area_nm2 == 0 && self.removed_area_nm2 == 0
    }
}
