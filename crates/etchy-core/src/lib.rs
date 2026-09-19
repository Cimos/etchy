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
pub mod imagediff;
pub mod model;
pub mod naming;
pub mod pagealign;
pub mod placement;
pub mod report;
pub mod view;

pub use diff::{diff_layer, nm2_to_mm2, LayerChange, LayerDiff};
pub use error::{EngineError, GeoError, Result};
pub use excellon::{looks_like_excellon, resolve_excellon};
pub use export::{board_areas_csv, board_report_html, layer_svg, layer_svg_with_colors};
pub use geo::{
    quantize_mm, simplify_contour, triangulate_shape, Aperture, Contour, Polarity, PolygonSet,
    Primitive, Pt, Shape, GRID_NM, NM_PER_MM,
};
pub use geom::CIRCLE_SEGMENTS;
pub use gerber::{coordinate_mismatch_warning, gerber_format, resolve_layer, GerberFormat, Units};
pub use imagediff::{
    classify_pair_size, crop_top_left, diff_images, Image, ImageDiffOptions, ImageDiffResult,
    ImageDiffStats, PairSizing, SIZE_TOLERANCE_PX,
};
pub use model::{
    pair_layers, same_board_guard, Board, DrillKind, FilePolarity, Layer, LayerKind, LayerPairing,
};
pub use naming::{
    classify, drill_kind, file_function, file_polarity, looks_like_gerber, reconcile_kind,
};
pub use pagealign::{
    align_pages, dissimilarity, fingerprint, PageAlignment, PageFingerprint, PageMatch, Pairing,
    MAX_ALIGN_PAGES,
};
pub use placement::{looks_like_placement, resolve_placement};
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
    // A pairing can fail (a negative layer against a positive one, #317); the
    // first error wins and aborts the whole compare — never a partial report.
    let pairs: Vec<(LayerReport, LayerView)> = {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            pairings
                .into_par_iter()
                .map(diff_one_layer)
                .collect::<Result<Vec<_>>>()?
        }
        #[cfg(target_arch = "wasm32")]
        {
            pairings
                .into_iter()
                .map(diff_one_layer)
                .collect::<Result<Vec<_>>>()?
        }
    };
    let (reports, views): (Vec<LayerReport>, Vec<LayerView>) = pairs.into_iter().unzip();
    Ok(BoardDiff {
        report: DiffReport::new(reports, Vec::new()),
        layers: views,
    })
}

/// Diff a single layer pairing into its (report, view). Pure and independent across
/// layers, so `compare_detailed` can run it in parallel (perf) without affecting
/// output order or determinism.
///
/// Errors only on a polarity mismatch: a `%TF.FilePolarity,Negative` layer paired
/// with a positive one describes the *complement* of the other's image, so any
/// diff between them is a full-layer lie — refused loud (#317).
fn diff_one_layer(pairing: LayerPairing) -> Result<(LayerReport, LayerView)> {
    // `a`/`b` are `Arc<PolygonSet>` handles shared with the source Board, so the
    // per-layer `.clone()`s below are refcount bumps, not deep copies — the parallel
    // fan-out doesn't multiply peak memory (#81). `empty` is the shared placeholder
    // for a one-sided layer's absent side.
    let empty: Arc<PolygonSet> = Arc::new(PolygonSet::default());
    // (a, b) are the (old, new) geometry for this kind; one is empty for a one-sided
    // layer. An empty one-sided layer stays Unchanged (no false gate).
    let (kind, label_old, label_new, a, b, default_status, negative) = match pairing {
        LayerPairing::Both { kind, old, new } => {
            if old.negative != new.negative {
                return Err(EngineError::PolarityMismatch {
                    label_old: old.label.clone(),
                    label_new: new.label.clone(),
                });
            }
            (
                kind,
                Some(old.label.clone()),
                Some(new.label.clone()),
                old.geometry.clone(),
                new.geometry.clone(),
                LayerStatus::Changed,
                old.negative,
            )
        }
        LayerPairing::OnlyOld(l) => (
            l.kind,
            Some(l.label.clone()),
            None,
            l.geometry.clone(),
            empty,
            LayerStatus::RemovedLayer,
            l.negative,
        ),
        LayerPairing::OnlyNew(l) => (
            l.kind,
            None,
            Some(l.label.clone()),
            empty,
            l.geometry.clone(),
            LayerStatus::AddedLayer,
            l.negative,
        ),
    };

    let mut d = diff_layer(&a, &b); // removed = a−b, added = b−a
    if negative {
        // The drawn objects are clearances, so geometry that appears in `b` is
        // material that *went away*. Swap so `added`/`removed` keep meaning copper
        // (#317). A one-sided negative layer is left as drawn: its status
        // (AddedLayer/RemovedLayer) is about the layer, not its material.
        std::mem::swap(&mut d.added, &mut d.removed);
    }
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
    Ok((report, view))
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
            negative: false,
        };
        let lb = Layer {
            kind: LayerKind::TopCopper,
            label: "F_Cu".into(),
            geometry: Arc::new(polygonize_gerber(b_txt.as_bytes()).unwrap()),
            negative: false,
        };
        let old = Board { layers: vec![la] };
        let new = Board { layers: vec![lb] };
        let rep = compare(&old, &new).unwrap();
        assert!(rep.any_changes());
        assert_eq!(rep.totals.added_regions, 1);
        assert_eq!(rep.totals.removed_regions, 0);
        assert_eq!(rep.layers[0].status, LayerStatus::Changed);
    }

    // ---- #240: public-API integration (through compare / compare_detailed) ----

    const HDR: &str = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";

    #[test]
    fn negative_layer_pair_swaps_added_and_removed() {
        // #317: on a %TF.FilePolarity,Negative layer the objects are clearances.
        // New geometry in `b` is material that went away, so the report must say
        // "removed", not "added" — the same input as a positive pair, mirrored.
        let hdr = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";
        let corners = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";
        let a_txt = format!("{hdr}{corners}M02*\n");
        let b_txt = format!("{hdr}{corners}X25000000Y25000000D03*\nM02*\n");
        let mk = |txt: &str, negative: bool| Layer {
            kind: LayerKind::InnerCopper(1),
            label: "In1_Cu".into(),
            geometry: Arc::new(polygonize_gerber(txt.as_bytes()).unwrap()),
            negative,
        };
        let pos = compare(
            &Board {
                layers: vec![mk(&a_txt, false)],
            },
            &Board {
                layers: vec![mk(&b_txt, false)],
            },
        )
        .unwrap();
        let neg = compare_detailed(
            &Board {
                layers: vec![mk(&a_txt, true)],
            },
            &Board {
                layers: vec![mk(&b_txt, true)],
            },
        )
        .unwrap();
        let p = &pos.layers[0];
        let n = &neg.report.layers[0];
        assert!(p.added_area_mm2 > 0.0 && p.removed_area_mm2 == 0.0);
        assert_eq!(n.removed_area_mm2, p.added_area_mm2, "swapped area");
        assert_eq!(n.added_area_mm2, p.removed_area_mm2);
        assert_eq!(n.removed_regions, p.added_regions, "swapped region count");
        assert_eq!(n.added_regions, p.removed_regions);
        // The view's geometry is swapped the same way, so overlay and report agree.
        let v = &neg.layers[0];
        assert!(!v.removed.is_empty() && v.added.is_empty());
        assert_eq!(v.status, LayerStatus::Changed);
    }

    #[test]
    fn negative_vs_positive_layer_fails_loud() {
        // #317: one revision negative, the other positive — complements, so any
        // diff is a whole-layer lie. Refused, naming both files.
        let hdr = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";
        let txt = format!("{hdr}X5000000Y5000000D03*\nX50000000Y50000000D03*\nM02*\n");
        let g = Arc::new(polygonize_gerber(txt.as_bytes()).unwrap());
        let mk = |label: &str, negative: bool| Layer {
            kind: LayerKind::InnerCopper(1),
            label: label.into(),
            geometry: Arc::clone(&g),
            negative,
        };
        let err = compare(
            &Board {
                layers: vec![mk("revA-In1_Cu.gbr", true)],
            },
            &Board {
                layers: vec![mk("revB-In1_Cu.gbr", false)],
            },
        )
        .unwrap_err();
        match err {
            EngineError::PolarityMismatch {
                label_old,
                label_new,
            } => {
                assert_eq!(label_old, "revA-In1_Cu.gbr");
                assert_eq!(label_new, "revB-In1_Cu.gbr");
            }
            other => panic!("expected PolarityMismatch, got {other:?}"),
        }
    }

    /// A copper layer flashed at the given `X…Y…` positions (nm), through the real
    /// Gerber pipeline, wrapped as a [`Layer`] of `kind`.
    fn cu_layer(kind: LayerKind, label: &str, flashes: &str) -> Layer {
        let txt = format!("{HDR}{flashes}M02*\n");
        Layer {
            kind,
            label: label.into(),
            geometry: Arc::new(polygonize_gerber(txt.as_bytes()).unwrap()),
            negative: false,
        }
    }

    #[test]
    fn compare_reports_removed_and_added_layers() {
        // A layer present on only one side must surface through the public `compare`
        // as a Removed/Added layer (not just at the unit level). Both boards share
        // the same physical extent (corner pads) so the same-board guard passes; the
        // old rev additionally carries a B_Cu layer the new rev drops, and the new
        // rev carries an F_Silk the old rev lacks.
        let corners = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";
        let old = Board {
            layers: vec![
                cu_layer(LayerKind::TopCopper, "F_Cu", corners),
                cu_layer(LayerKind::BottomCopper, "B_Cu", corners),
            ],
        };
        let new = Board {
            layers: vec![
                cu_layer(LayerKind::TopCopper, "F_Cu", corners),
                cu_layer(LayerKind::TopSilk, "F_Silk", corners),
            ],
        };
        let rep = compare(&old, &new).unwrap();
        let status = |k: LayerKind| {
            rep.layers
                .iter()
                .find(|l| l.kind == k.kind_str())
                .map(|l| l.status)
                .unwrap_or_else(|| panic!("no {k:?} layer in report"))
        };
        assert_eq!(status(LayerKind::BottomCopper), LayerStatus::RemovedLayer);
        assert_eq!(status(LayerKind::TopSilk), LayerStatus::AddedLayer);
        assert_eq!(status(LayerKind::TopCopper), LayerStatus::Unchanged);
        assert!(
            rep.any_changes(),
            "an added and a removed layer are changes"
        );
    }

    #[test]
    fn compare_fails_loud_on_board_mismatch() {
        // Grossly different board extents (a wrong-pair) must fail loud through the
        // public API — the same-board guard is not bypassable except via --force,
        // which lives in the CLI, not here.
        let old = Board {
            layers: vec![cu_layer(
                LayerKind::TopCopper,
                "F_Cu",
                "X5000000Y5000000D03*\nX10000000Y10000000D03*\n",
            )],
        };
        let new = Board {
            layers: vec![cu_layer(
                LayerKind::TopCopper,
                "F_Cu",
                "X5000000Y5000000D03*\nX200000000Y200000000D03*\n",
            )],
        };
        assert!(matches!(
            compare(&old, &new),
            Err(EngineError::BoardMismatch { .. })
        ));
        // compare_detailed shares the guard.
        assert!(matches!(
            compare_detailed(&old, &new),
            Err(EngineError::BoardMismatch { .. })
        ));
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
                negative: false,
            }],
        };
        let new = Board {
            layers: vec![Layer {
                kind: LayerKind::TopCopper,
                label: "F_Cu".into(),
                geometry: Arc::clone(&gb),
                negative: false,
            }],
        };
        let diff = compare_detailed(&old, &new).unwrap();
        // Same allocation, not a copy: the view points at the board's geometry.
        assert!(Arc::ptr_eq(&diff.layers[0].old, &ga));
        assert!(Arc::ptr_eq(&diff.layers[0].new, &gb));
    }
}
