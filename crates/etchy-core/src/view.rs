//! Rich diff result for *visual* consumers (the GUI, and later SVG/HTML).
//!
//! [`crate::compare`] returns just the numeric [`crate::DiffReport`]; the viewer
//! also needs the actual per-layer geometry to draw. [`BoardDiff`] carries both,
//! from the same single computation.

use std::sync::Arc;

use crate::diff::LayerChange;
use crate::geo::PolygonSet;
use crate::model::LayerKind;
use crate::report::{DiffReport, LayerStatus};

/// One paired layer's full geometry, ready to render.
#[derive(Debug, Clone)]
pub struct LayerView {
    pub kind: LayerKind,
    pub label_old: Option<String>,
    pub label_new: Option<String>,
    pub status: LayerStatus,
    /// Old-revision filled geometry (empty for an added layer). Shared with the
    /// source [`crate::Board`] via `Arc` — not deep-copied per layer (#81).
    pub old: Arc<PolygonSet>,
    /// New-revision filled geometry (empty for a removed layer). Shared via `Arc`.
    pub new: Arc<PolygonSet>,
    /// `B − A` — copper present only in the new revision.
    pub added: PolygonSet,
    /// `A − B` — copper present only in the old revision.
    pub removed: PolygonSet,
    pub change: LayerChange,
}

impl LayerView {
    /// Display name, e.g. `inner-copper2` or `top-copper`.
    pub fn name(&self) -> String {
        match self.kind.inner_index() {
            Some(n) => format!("{}{}", self.kind.kind_str(), n),
            None => self.kind.kind_str().to_string(),
        }
    }
    pub fn is_changed(&self) -> bool {
        self.status != LayerStatus::Unchanged
    }
}

/// The full comparison: numeric report + per-layer geometry (in board stack order).
#[derive(Debug, Clone)]
pub struct BoardDiff {
    pub report: DiffReport,
    pub layers: Vec<LayerView>,
}
