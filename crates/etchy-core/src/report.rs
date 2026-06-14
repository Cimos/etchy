//! The single in-memory report that the HTML, SVG and JSON outputs all derive
//! from, so the three can never disagree. Phase-0 scaffold.

use crate::diff::LayerChange;
use crate::model::LayerKind;

/// Schema version of the machine-readable JSON output — the integration contract.
pub const SCHEMA_VERSION: u32 = 1;

/// A whole comparison result.
#[derive(Debug, Clone, Default)]
pub struct DiffReport {
    pub layers: Vec<(LayerKind, LayerChange)>,
}

impl DiffReport {
    /// True if any layer changed.
    pub fn any_changes(&self) -> bool {
        self.layers.iter().any(|(_, change)| !change.is_unchanged())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_has_no_changes() {
        assert!(!DiffReport::default().any_changes());
    }
}
