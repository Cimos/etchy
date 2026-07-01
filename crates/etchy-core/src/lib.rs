//! etchy-core — the format-agnostic PCB diff engine.
//!
//! Pipeline (see `docs/DEVELOPER_GUIDE.md` and `docs/M1_ENGINE_DESIGN.md`):
//! `parse → resolve graphics state → polygonize → boolean diff → measure → report`.
//!
//! **Pure logic, no I/O policy.** The engine takes bytes and an already-classified
//! [`Board`], and returns a [`DiffReport`] or a typed [`EngineError`]; it never
//! reads a file, prints, or exits — the CLI/GUI own all I/O (CLAUDE.md).
//!
//! The Gerber front-end resolves flashes (circle/rect/obround/polygon/macro),
//! stroked `D01` lines and `G02/G03` arcs, `G36/G37` region fills, and `LPD/LPC`
//! polarity. Features it cannot render faithfully fail loud (the trust bar).
//! Excellon, and the SVG/HTML renderers, are later increments.

mod boolean;
pub mod diff;
pub mod error;
pub mod excellon;
pub mod export;
pub mod geo;
pub mod geom;
pub mod gerber;
pub mod model;
pub mod naming;
pub mod report;
pub mod view;

pub use diff::{diff_layer, nm2_to_mm2, LayerChange, LayerDiff};
pub use error::{EngineError, GeoError, Result};
pub use excellon::{looks_like_excellon, resolve_excellon};
pub use export::layer_svg;
pub use geo::{
    quantize_mm, simplify_contour, triangulate_shape, Aperture, Contour, Polarity, PolygonSet,
    Primitive, Pt, Shape, GRID_NM, NM_PER_MM,
};
pub use geom::CIRCLE_SEGMENTS;
pub use gerber::{coordinate_mismatch_warning, gerber_format, resolve_layer, GerberFormat, Units};
pub use model::{pair_layers, same_board_guard, Board, Layer, LayerKind, LayerPairing};
pub use naming::{classify, looks_like_gerber};
pub use report::{DiffReport, LayerReport, LayerStatus, Totals, SCHEMA_VERSION};
pub use view::{BoardDiff, LayerView};

use std::sync::Arc;

/// The crate version, from Cargo.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse + resolve one Gerber layer's bytes into its filled geometry — the
/// convenience the CLI/GUI call per file.
pub fn polygonize_gerber(bytes: &[u8]) -> Result<PolygonSet> {
    resolve_layer(bytes)
}

/// Compare two revisions, returning just the numeric report. The pure entry point
/// the CLI calls; delegates to [`compare_detailed`].
pub fn compare(old: &Board, new: &Board) -> Result<DiffReport> {
    compare_detailed(old, new).map(|d| d.report)
}

/// Compare two revisions and return both the numeric report **and** the per-layer
/// geometry: same-board guard → pair by label → per-layer diff → measure → assemble.
/// The viewer/exporters call this; `compare` is the numbers-only shortcut.
pub fn compare_detailed(old: &Board, new: &Board) -> Result<BoardDiff> {
    if old.layers.is_empty() && new.layers.is_empty() {
        return Err(EngineError::NoLayers);
    }
    same_board_guard(old, new)?;

    let pairings = pair_layers(old, new);
    // The expensive part — the boolean diff per layer — is independent across layers,
    // so we map each pairing to its (report, view) and run the map in parallel on
    // native (large boards have many layers; near-linear speedup). wasm stays serial.
    // Order is preserved, so the result is identical to the old sequential loop.
    // Geometry is shared via Arc inside diff_one_layer (#81), so the parallel fan-out
    // holds refcount handles, not per-layer deep copies (no 16x memory spike).
    let (reports, views): (Vec<LayerReport>, Vec<LayerView>) = {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            pairings.into_par_iter().map(diff_one_layer).unzip()
        }
        #[cfg(target_arch = "wasm32")]
        {
            pairings.into_iter().map(diff_one_layer).unzip()
        }
    };
    Ok(BoardDiff {
        report: DiffReport::new(reports, Vec::new()),
        layers: views,
    })
}

/// Diff a single layer pairing into its (report, view). Pure and independent across
/// layers, so `compare_detailed` can run it in parallel (perf) without affecting
/// output order or determinism.
fn diff_one_layer(pairing: LayerPairing) -> (LayerReport, LayerView) {
    // `a`/`b` are `Arc<PolygonSet>` handles shared with the source Board, so the
    // per-layer `.clone()`s below are refcount bumps, not deep copies — the parallel
    // fan-out doesn't multiply peak memory (#81). `empty` is the shared placeholder
    // for a one-sided layer's absent side.
    let empty: Arc<PolygonSet> = Arc::new(PolygonSet::default());
    // (a, b) are the (old, new) geometry for this kind; one is empty for a one-sided
    // layer. An empty one-sided layer stays Unchanged (no false gate).
    let (kind, label_old, label_new, a, b, default_status) = match pairing {
        LayerPairing::Both { kind, old, new } => (
            kind,
            Some(old.label.clone()),
            Some(new.label.clone()),
            old.geometry.clone(),
            new.geometry.clone(),
            LayerStatus::Changed,
        ),
        LayerPairing::OnlyOld(l) => (
            l.kind,
            Some(l.label.clone()),
            None,
            l.geometry.clone(),
            empty,
            LayerStatus::RemovedLayer,
        ),
        LayerPairing::OnlyNew(l) => (
            l.kind,
            None,
            Some(l.label.clone()),
            empty,
            l.geometry.clone(),
            LayerStatus::AddedLayer,
        ),
    };

    let d = diff_layer(&a, &b); // removed = a−b, added = b−a
    let change = d.measure();
    let status = if change.is_unchanged() {
        LayerStatus::Unchanged
    } else {
        default_status
    };
    let report = LayerReport::new(kind, label_old.clone(), label_new.clone(), status, &change);
    let view = LayerView {
        kind,
        label_old,
        label_new,
        status,
        old: a,
        new: b,
        added: d.added,
        removed: d.removed,
        change,
    };
    (report, view)
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
            geometry: Arc::new(polygonize_gerber(a_txt.as_bytes()).unwrap()),
        };
        let lb = Layer {
            kind: LayerKind::TopCopper,
            label: "F_Cu".into(),
            geometry: Arc::new(polygonize_gerber(b_txt.as_bytes()).unwrap()),
        };
        let old = Board { layers: vec![la] };
        let new = Board { layers: vec![lb] };
        let rep = compare(&old, &new).unwrap();
        assert!(rep.any_changes());
        assert_eq!(rep.totals.added_regions, 1);
        assert_eq!(rep.totals.removed_regions, 0);
        assert_eq!(rep.layers[0].status, LayerStatus::Changed);
    }

    #[test]
    fn compare_detailed_shares_geometry_via_arc() {
        // #81: the view must reuse each board layer's geometry allocation, not
        // deep-copy it. Asserted by pointer identity so a future regression that
        // reintroduces a clone fails loudly.
        let hdr = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";
        let corners = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";
        let ga = Arc::new(polygonize_gerber(format!("{hdr}{corners}M02*\n").as_bytes()).unwrap());
        let gb = Arc::new(
            polygonize_gerber(format!("{hdr}{corners}X25000000Y25000000D03*\nM02*\n").as_bytes())
                .unwrap(),
        );
        let old = Board {
            layers: vec![Layer {
                kind: LayerKind::TopCopper,
                label: "F_Cu".into(),
                geometry: Arc::clone(&ga),
            }],
        };
        let new = Board {
            layers: vec![Layer {
                kind: LayerKind::TopCopper,
                label: "F_Cu".into(),
                geometry: Arc::clone(&gb),
            }],
        };
        let diff = compare_detailed(&old, &new).unwrap();
        // Same allocation, not a copy: the view points at the board's geometry.
        assert!(Arc::ptr_eq(&diff.layers[0].old, &ga));
        assert!(Arc::ptr_eq(&diff.layers[0].new, &gb));
    }
}
