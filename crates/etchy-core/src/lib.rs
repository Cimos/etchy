//! etchy-core — the format-agnostic PCB diff engine.
//!
//! Pipeline (see `docs/DEVELOPER_GUIDE.md` and `docs/M1_ENGINE_DESIGN.md`):
//! `parse → resolve graphics state → polygonize → boolean diff → measure → report`.
//!
//! **Pure logic, no I/O policy.** The engine takes bytes and an already-classified
//! [`Board`], and returns a [`DiffReport`] or a typed [`EngineError`]; it never
//! reads a file, prints, or exits — the CLI/GUI own all I/O (CLAUDE.md).
//!
//! This Milestone-1 increment renders **flash of circle/rect apertures**; every
//! other feature fails loud (the trust bar). See `docs/M1_ENGINE_DESIGN.md`.

pub mod diff;
pub mod error;
pub mod geo;
pub mod gerber;
pub mod model;
pub mod polygonize;
pub mod report;

pub use diff::{diff_layer, nm2_to_mm2, LayerChange, LayerDiff};
pub use error::{EngineError, GeoError, Result};
pub use geo::{
    quantize_mm, Aperture, Contour, PolygonSet, Primitive, Pt, Shape, GRID_NM, NM_PER_MM,
};
pub use gerber::parse_gerber;
pub use model::{pair_layers, same_board_guard, Board, Layer, LayerKind, LayerPairing};
pub use polygonize::{polygonize, CIRCLE_SEGMENTS};
pub use report::{DiffReport, LayerReport, LayerStatus, Totals, SCHEMA_VERSION};

/// The crate version, from Cargo.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse + polygonize one Gerber layer's bytes into its filled geometry — the
/// convenience the CLI calls per file.
pub fn polygonize_gerber(bytes: &[u8]) -> Result<PolygonSet> {
    polygonize(&parse_gerber(bytes)?)
}

/// Compare two revisions: same-board guard → pair by kind → per-layer diff →
/// measure → assemble the report. The pure entry point the CLI and GUI both call.
pub fn compare(old: &Board, new: &Board) -> Result<DiffReport> {
    if old.layers.is_empty() && new.layers.is_empty() {
        return Err(EngineError::NoLayers);
    }
    // Refuse ambiguous boards (two layers of one kind) — pairing would drop one.
    model::ensure_unique_kinds(old, "old")?;
    model::ensure_unique_kinds(new, "new")?;
    same_board_guard(old, new)?;

    let empty = PolygonSet::default();
    let mut reports = Vec::new();
    for pairing in pair_layers(old, new) {
        let report = match pairing {
            LayerPairing::Both { kind, old, new } => {
                let change = diff_layer(&old.geometry, &new.geometry).measure();
                let status = if change.is_unchanged() {
                    LayerStatus::Unchanged
                } else {
                    LayerStatus::Changed
                };
                LayerReport::new(
                    kind,
                    Some(old.label.clone()),
                    Some(new.label.clone()),
                    status,
                    &change,
                )
            }
            // A layer present in only one revision is a removed/added layer — but
            // only if it actually carries geometry. An empty one-sided layer must
            // NOT flip `any_changes` (a false CI gate); it reports as Unchanged.
            LayerPairing::OnlyOld(l) => {
                let change = diff_layer(&l.geometry, &empty).measure();
                let status = if change.is_unchanged() {
                    LayerStatus::Unchanged
                } else {
                    LayerStatus::RemovedLayer
                };
                LayerReport::new(l.kind, Some(l.label.clone()), None, status, &change)
            }
            LayerPairing::OnlyNew(l) => {
                let change = diff_layer(&empty, &l.geometry).measure();
                let status = if change.is_unchanged() {
                    LayerStatus::Unchanged
                } else {
                    LayerStatus::AddedLayer
                };
                LayerReport::new(l.kind, None, Some(l.label.clone()), status, &change)
            }
        };
        reports.push(report);
    }
    Ok(DiffReport::new(reports, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!version().is_empty());
    }

    #[test]
    fn compare_empty_boards_errors() {
        let r = compare(&Board::default(), &Board::default());
        assert!(matches!(r, Err(EngineError::NoLayers)));
    }

    #[test]
    fn end_to_end_flash_diff() {
        // Both revisions share the same board extent (corner pads at 5,5 and 50,50);
        // the new revision adds one pad inside that extent → 1 added region, guard passes.
        let hdr = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";
        let corners = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";
        let a_txt = format!("{hdr}{corners}M02*\n");
        let b_txt = format!("{hdr}{corners}X25000000Y25000000D03*\nM02*\n");
        let la = Layer {
            kind: LayerKind::TopCopper,
            label: "F_Cu".into(),
            geometry: polygonize_gerber(a_txt.as_bytes()).unwrap(),
        };
        let lb = Layer {
            kind: LayerKind::TopCopper,
            label: "F_Cu".into(),
            geometry: polygonize_gerber(b_txt.as_bytes()).unwrap(),
        };
        let old = Board { layers: vec![la] };
        let new = Board { layers: vec![lb] };
        let rep = compare(&old, &new).unwrap();
        assert!(rep.any_changes());
        assert_eq!(rep.totals.added_regions, 1);
        assert_eq!(rep.totals.removed_regions, 0);
        assert_eq!(rep.layers[0].status, LayerStatus::Changed);
    }
}
