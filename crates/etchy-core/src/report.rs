//! The single in-memory report that the (future) HTML, SVG and JSON outputs all
//! derive from, so the three can never disagree. `schema_version` is the
//! integration contract consumers/CI gate on.

use serde::Serialize;

use crate::diff::LayerChange;
use crate::model::LayerKind;

/// Schema version of the machine-readable JSON output — the integration contract.
pub const SCHEMA_VERSION: u32 = 1;

/// Per-layer pairing status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayerStatus {
    Unchanged,
    Changed,
    /// Present only in the new revision.
    AddedLayer,
    /// Present only in the old revision.
    RemovedLayer,
}

/// One layer's entry in the report.
#[derive(Debug, Clone, Serialize)]
pub struct LayerReport {
    /// Stable kebab-case kind tag (`top-copper`, `inner-copper`, …).
    pub kind: &'static str,
    /// Inner-copper index (1-based), when `kind == "inner-copper"`.
    pub inner_index: Option<u8>,
    pub label_old: Option<String>,
    pub label_new: Option<String>,
    pub status: LayerStatus,
    pub added_area_mm2: f64,
    pub removed_area_mm2: f64,
    /// Exact integer nm² as a decimal **string** (JSON numbers can't hold i128).
    pub added_area_nm2: String,
    pub removed_area_nm2: String,
    pub added_regions: u32,
    pub removed_regions: u32,
}

impl LayerReport {
    pub fn new(
        kind: LayerKind,
        label_old: Option<String>,
        label_new: Option<String>,
        status: LayerStatus,
        change: &LayerChange,
    ) -> Self {
        Self {
            kind: kind.kind_str(),
            inner_index: kind.inner_index(),
            label_old,
            label_new,
            status,
            added_area_mm2: change.added_area_mm2(),
            removed_area_mm2: change.removed_area_mm2(),
            added_area_nm2: change.added_area_nm2.to_string(),
            removed_area_nm2: change.removed_area_nm2.to_string(),
            added_regions: change.added_region_count,
            removed_regions: change.removed_region_count,
        }
    }

    fn is_changed(&self) -> bool {
        self.status != LayerStatus::Unchanged
    }
}

/// Board-level totals.
#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    pub added_area_mm2: f64,
    pub removed_area_mm2: f64,
    pub added_regions: u32,
    pub removed_regions: u32,
    pub layers_changed: u32,
    pub layers_total: u32,
}

/// A whole comparison result — the source of truth for every output format.
#[derive(Debug, Clone, Serialize)]
pub struct DiffReport {
    pub schema_version: u32,
    pub tool_version: &'static str,
    pub any_changes: bool,
    pub totals: Totals,
    pub layers: Vec<LayerReport>,
    /// Non-fatal notices (e.g. ignored metadata commands). Empty in this
    /// increment; the field is part of the v1 contract for forward-compatibility.
    pub warnings: Vec<String>,
}

impl DiffReport {
    /// Assemble from per-layer reports: compute totals, sort changed-first.
    pub fn new(mut layers: Vec<LayerReport>, warnings: Vec<String>) -> Self {
        // changed-first, then stable (the input is already in stack order).
        layers.sort_by_key(|l| !l.is_changed());

        let layers_total = layers.len() as u32;
        let layers_changed = layers.iter().filter(|l| l.is_changed()).count() as u32;
        let totals = Totals {
            added_area_mm2: layers.iter().map(|l| l.added_area_mm2).sum(),
            removed_area_mm2: layers.iter().map(|l| l.removed_area_mm2).sum(),
            added_regions: layers.iter().map(|l| l.added_regions).sum(),
            removed_regions: layers.iter().map(|l| l.removed_regions).sum(),
            layers_changed,
            layers_total,
        };
        Self {
            schema_version: SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION"),
            any_changes: layers_changed > 0,
            totals,
            layers,
            warnings,
        }
    }

    /// True if any layer changed.
    pub fn any_changes(&self) -> bool {
        self.any_changes
    }

    /// Serialize to compact JSON (the machine-readable contract).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("DiffReport serializes")
    }

    /// Serialize to pretty JSON.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("DiffReport serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_has_no_changes() {
        let r = DiffReport::new(vec![], vec![]);
        assert!(!r.any_changes());
        assert_eq!(r.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn changed_layers_sort_first_and_count() {
        let unchanged = LayerReport::new(
            LayerKind::BottomCopper,
            Some("b".into()),
            Some("b".into()),
            LayerStatus::Unchanged,
            &LayerChange::default(),
        );
        let changed = LayerReport::new(
            LayerKind::TopCopper,
            Some("f".into()),
            Some("f".into()),
            LayerStatus::Changed,
            &LayerChange {
                added_area_nm2: 5,
                added_region_count: 1,
                ..Default::default()
            },
        );
        let r = DiffReport::new(vec![unchanged, changed], vec![]);
        assert!(r.any_changes());
        assert_eq!(r.totals.layers_changed, 1);
        assert_eq!(r.layers[0].status, LayerStatus::Changed); // changed-first
        assert!(r.to_json().contains("\"schema_version\":1"));
    }
}
