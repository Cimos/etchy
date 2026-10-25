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
pub mod native;
pub mod pagealign;
pub mod placement;
pub mod report;
pub mod view;

pub use diff::{diff_layer, nm2_to_mm2, LayerChange, LayerDiff};
pub use error::{EngineError, GeoError, Result};
pub use excellon::{
    looks_like_excellon, resolve_excellon, resolve_excellon_with_tools, DrillTools, ToolCount,
};
pub use export::{
    board_areas_csv, board_report_html, board_report_html_with_limit, layer_svg,
    layer_svg_with_colors,
};
pub use geo::{
    quantize_mm, simplify_contour, triangulate_shape, Aperture, Contour, Polarity, PolygonSet,
    Primitive, Pt, Shape, GRID_NM, NM_PER_MM,
};
pub use geom::CIRCLE_SEGMENTS;
pub use gerber::{
    coordinate_mismatch_warning, gerber_format, resolve_layer, resolve_layer_with_labels,
    GerberFormat, Units,
};
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
pub use native::*;
pub use pagealign::{
    align_pages, dissimilarity, fingerprint, PageAlignment, PageFingerprint, PageMatch, Pairing,
    MAX_ALIGN_PAGES,
};
pub use placement::{looks_like_placement, resolve_placement};
pub use report::{
    DiffReport, DrillCounts, DrillRowChange, DrillTableReport, DrillTableRow, LayerReport,
    LayerStatus, NativeDiagnosticsReport, NativeDiffReport, NativeInputReport, ObjectChange,
    ObjectChangeFlag, ObjectChangeStatus, ObjectIdentity, ObjectKind, ObjectSnapshot,
    ObjectSummary, Totals, NATIVE_SCHEMA_VERSION, SCHEMA_VERSION,
};
pub use view::{BoardDiff, LayerView};

use std::collections::BTreeSet;
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
    // The tool table compares only when every present side has one: an Excellon
    // file paired with a Gerber-format drill file would otherwise read as every
    // size added or removed.
    let drill_table = match &pairing {
        LayerPairing::Both { old, new, .. } => match (&old.drill_tools, &new.drill_tools) {
            (Some(o), Some(n)) => report::DrillTableReport::compare(Some(o), Some(n)),
            _ => None,
        },
        LayerPairing::OnlyOld(l) => report::DrillTableReport::compare(l.drill_tools.as_ref(), None),
        LayerPairing::OnlyNew(l) => report::DrillTableReport::compare(None, l.drill_tools.as_ref()),
    };
    // (a, b) are the (old, new) geometry for this kind; one is empty for a one-sided
    // layer. An empty one-sided layer stays Unchanged (no false gate).
    let (kind, label_old, label_new, a, b, old_x2, new_x2, default_status, negative) = match pairing
    {
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
                old.x2_objects.clone(),
                new.x2_objects.clone(),
                LayerStatus::Changed,
                old.negative,
            )
        }
        // A one-sided layer is never swapped: its status (AddedLayer/RemovedLayer)
        // is about the layer's presence, and its diff is simply its geometry as
        // drawn, negative or not.
        LayerPairing::OnlyOld(l) => (
            l.kind,
            Some(l.label.clone()),
            None,
            l.geometry.clone(),
            empty,
            l.x2_objects.clone(),
            None,
            LayerStatus::RemovedLayer,
            false,
        ),
        LayerPairing::OnlyNew(l) => (
            l.kind,
            None,
            Some(l.label.clone()),
            empty,
            l.geometry.clone(),
            None,
            l.x2_objects.clone(),
            LayerStatus::AddedLayer,
            false,
        ),
    };

    let mut d = diff_layer(&a, &b); // removed = a−b, added = b−a
    if negative {
        // Both revisions are negative images: the drawn objects are clearances, so
        // geometry that appears in `b` is material that *went away*. Swap so
        // `added`/`removed` keep meaning copper (#317).
        std::mem::swap(&mut d.added, &mut d.removed);
    }
    let (added, removed) = d.reported_regions();
    let change = d.measure_regions(&added, &removed);
    let status = if change.is_unchanged() {
        LayerStatus::Unchanged
    } else {
        default_status
    };
    let mut report = LayerReport::new(kind, label_old.clone(), label_new.clone(), status, &change);
    let (added_x2, removed_x2) = if negative {
        (&old_x2, &new_x2)
    } else {
        (&new_x2, &old_x2)
    };
    report.labels_available = old_x2.is_some() || new_x2.is_some();
    report.drill_table = drill_table;
    // Index each side's labelled objects once per layer, not once per region:
    // a scan of every object for every region is quadratic on big changes.
    let added_index = added_x2.as_deref().map(|o| LabelIndex::new(o));
    let removed_index = removed_x2.as_deref().map(|o| LabelIndex::new(o));
    report.regions = added
        .iter()
        .map(|r| region_report(r, report::RegionChange::Added, added_index.as_ref()))
        .chain(
            removed
                .iter()
                .map(|r| region_report(r, report::RegionChange::Removed, removed_index.as_ref())),
        )
        .collect();
    debug_assert_eq!(
        report.regions.len(),
        (change.added_region_count + change.removed_region_count) as usize
    );
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

const MAX_LABELS_PER_KIND: usize = 20;

/// Cells per grid axis at most; the grid is sized to about one object per cell.
const LABEL_GRID_MAX: usize = 128;
/// An object spanning more cells than this (a pour, a long track) is kept in a
/// short always-checked list instead of being copied into every cell it covers.
const LABEL_GRID_LARGE: usize = 64;

/// A uniform grid over one side's labelled objects, keyed by outline bounding
/// box. It only narrows the candidates: every hit is still confirmed by the
/// bounding-box test and the exact boolean overlap, so labels match a full scan.
struct LabelIndex<'a> {
    objects: &'a [gerber::LabeledContour],
    /// `[min_x, min_y, max_x, max_y]` of each object's outline (first ring).
    boxes: Vec<[i64; 4]>,
    origin: [i64; 2],
    cell: i64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    large: Vec<u32>,
}

impl<'a> LabelIndex<'a> {
    fn new(objects: &'a [gerber::LabeledContour]) -> Self {
        // The outline bounds the object; holes lie inside it.
        let boxes: Vec<[i64; 4]> = objects
            .iter()
            .map(|o| {
                let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
                for p in o.shape.first().into_iter().flatten() {
                    b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
                }
                b
            })
            .collect();
        let mut ext = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
        for b in boxes.iter().filter(|b| b[0] <= b[2]) {
            ext = [
                ext[0].min(b[0]),
                ext[1].min(b[1]),
                ext[2].max(b[2]),
                ext[3].max(b[3]),
            ];
        }
        let mut index = LabelIndex {
            objects,
            boxes,
            origin: [0, 0],
            cell: 1,
            cols: 1,
            rows: 1,
            cells: vec![Vec::new()],
            large: Vec::new(),
        };
        if ext[0] > ext[2] {
            return index; // no object has an outline
        }
        let side = ((objects.len() as f64).sqrt().ceil() as usize).clamp(1, LABEL_GRID_MAX);
        let span = (ext[2] - ext[0]).max(ext[3] - ext[1]).max(1);
        index.origin = [ext[0], ext[1]];
        index.cell = (span / side as i64 + 1).max(1);
        index.cols = ((ext[2] - ext[0]) / index.cell) as usize + 1;
        index.rows = ((ext[3] - ext[1]) / index.cell) as usize + 1;
        index.cells = vec![Vec::new(); index.cols * index.rows];
        for (i, b) in index.boxes.iter().enumerate() {
            if b[0] > b[2] {
                continue;
            }
            let (c0, r0, c1, r1) = index.cell_range(b);
            if (c1 - c0 + 1) * (r1 - r0 + 1) > LABEL_GRID_LARGE {
                index.large.push(i as u32);
                continue;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    index.cells[r * index.cols + c].push(i as u32);
                }
            }
        }
        index
    }

    /// Inclusive grid cell range covered by a box, clamped to the grid.
    fn cell_range(&self, b: &[i64; 4]) -> (usize, usize, usize, usize) {
        let col = |x: i64| ((x - self.origin[0]).max(0) / self.cell).min(self.cols as i64 - 1);
        let row = |y: i64| ((y - self.origin[1]).max(0) / self.cell).min(self.rows as i64 - 1);
        (
            col(b[0]) as usize,
            row(b[1]) as usize,
            col(b[2]) as usize,
            row(b[3]) as usize,
        )
    }

    /// Objects whose outline box touches `bb`, in index order (deterministic).
    fn candidates(&self, bb: &[i64; 4]) -> Vec<u32> {
        let (c0, r0, c1, r1) = self.cell_range(bb);
        let mut out = self.large.clone();
        for r in r0..=r1 {
            for c in c0..=c1 {
                out.extend_from_slice(&self.cells[r * self.cols + c]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&i| {
            let b = &self.boxes[i as usize];
            b[0] <= b[2] && !(b[2] < bb[0] || b[0] > bb[2] || b[3] < bb[1] || b[1] > bb[3])
        });
        out
    }
}

fn region_report(
    region: &diff::ChangedRegion<'_>,
    direction: report::RegionChange,
    index: Option<&LabelIndex<'_>>,
) -> report::RegionReport {
    let mut result = report::RegionReport::from_region(region, direction);
    let Some(index) = index else {
        return result;
    };
    let mut refs = BTreeSet::new();
    let mut pins = BTreeSet::new();
    let mut nets = BTreeSet::new();
    for i in index.candidates(&region.bbox_nm) {
        let object = &index.objects[i as usize];
        if !boolean::overlaps(region.shape, &object.shape) {
            continue;
        }
        if let Some(name) = &object.labels.refdes {
            refs.insert(name.clone());
        }
        if let Some(name) = &object.labels.pin_refdes {
            refs.insert(name.clone());
        }
        if let Some(name) = &object.labels.pin {
            pins.insert(name.clone());
        }
        nets.extend(object.labels.nets.iter().cloned());
    }
    let truncated = refs.len() > MAX_LABELS_PER_KIND
        || pins.len() > MAX_LABELS_PER_KIND
        || nets.len() > MAX_LABELS_PER_KIND;
    result.labels = Some(report::RegionLabels {
        refdes: refs.into_iter().take(MAX_LABELS_PER_KIND).collect(),
        pins: pins.into_iter().take(MAX_LABELS_PER_KIND).collect(),
        nets: nets.into_iter().take(MAX_LABELS_PER_KIND).collect(),
        truncated,
    });
    result
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
            x2_objects: None,
            drill_tools: None,
        };
        let lb = Layer {
            kind: LayerKind::TopCopper,
            label: "F_Cu".into(),
            geometry: Arc::new(polygonize_gerber(b_txt.as_bytes()).unwrap()),
            negative: false,
            x2_objects: None,
            drill_tools: None,
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
    fn region_labels_use_the_changed_side_and_mark_unavailable() {
        let header = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10R,1X1*%\nD10*\n";
        let anchors = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";
        let old_text =
            format!("{header}{anchors}%TO.C,R1*%\n%TO.N,OLD*%\nX15000000Y15000000D03*\nM02*\n");
        let new_text = format!("{header}{anchors}%TO.C,U2*%\n%TO.P,U2,4*%\n%TO.N,NEW*%\nX25000000Y25000000D03*\nM02*\n");
        let make = |text: &str, labels: bool| {
            let (geometry, objects) = resolve_layer_with_labels(text.as_bytes()).unwrap();
            Layer {
                kind: LayerKind::TopCopper,
                label: "F_Cu".into(),
                geometry: Arc::new(geometry),
                negative: false,
                x2_objects: labels.then(|| Arc::new(objects.unwrap())),
                drill_tools: None,
            }
        };
        let old = Board {
            layers: vec![make(&old_text, true)],
        };
        let new = Board {
            layers: vec![make(&new_text, true)],
        };
        let report = compare(&old, &new).unwrap();
        let regions = &report.layers[0].regions;
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].labels.as_ref().unwrap().refdes, ["U2"]);
        assert_eq!(regions[0].labels.as_ref().unwrap().pins, ["U2:4"]);
        assert_eq!(regions[0].labels.as_ref().unwrap().nets, ["NEW"]);
        assert_eq!(regions[1].labels.as_ref().unwrap().refdes, ["R1"]);
        assert_eq!(regions[1].labels.as_ref().unwrap().nets, ["OLD"]);
        let json = report.to_json();
        assert!(json.contains("\"regions\""));
        assert!(report.to_markdown_summary().contains("U2:4"));
        let unlabelled = Board {
            layers: vec![make(&new_text, false)],
        };
        let report = compare(&old, &unlabelled).unwrap();
        assert!(report.layers[0].regions[0].labels.is_none());
        assert!(report.to_markdown_summary().contains("unavailable"));
    }

    #[test]
    fn region_label_lists_are_capped_and_announce_truncation() {
        let contour = vec![
            Pt::new(0, 0),
            Pt::new(1_000_000, 0),
            Pt::new(1_000_000, 1_000_000),
            Pt::new(0, 1_000_000),
        ];
        let shape = vec![contour.clone()];
        let region = diff::ChangedRegion {
            area_nm2: 1_000_000_000_000,
            bbox_nm: [0, 0, 1_000_000, 1_000_000],
            centroid_nm: [500_000, 500_000],
            shape: &shape,
        };
        let objects: Vec<_> = (0..25)
            .map(|i| gerber::LabeledContour {
                shape: vec![contour.clone()],
                labels: gerber::ObjectLabels {
                    refdes: Some(format!("U{i}")),
                    ..Default::default()
                },
            })
            .collect();
        let index = LabelIndex::new(&objects);
        let report = region_report(&region, report::RegionChange::Added, Some(&index));
        let labels = report.labels.unwrap();
        assert_eq!(labels.refdes.len(), 20);
        assert!(labels.truncated);
    }

    #[test]
    fn label_index_finds_the_same_objects_as_a_full_scan() {
        // A 30x30 grid of 1 mm pads at 2 mm pitch plus one board-wide pour (kept
        // in the always-checked list). Every query box must get exactly the
        // objects a plain bounding-box scan finds.
        let sq = |x: i64, y: i64, s: i64| {
            vec![
                Pt::new(x, y),
                Pt::new(x + s, y),
                Pt::new(x + s, y + s),
                Pt::new(x, y + s),
            ]
        };
        let mut objects: Vec<_> = (0..30)
            .flat_map(|i| (0..30).map(move |j| (i, j)))
            .map(|(i, j)| gerber::LabeledContour {
                shape: vec![sq(i * 2_000_000, j * 2_000_000, 1_000_000)],
                labels: gerber::ObjectLabels::default(),
            })
            .collect();
        objects.push(gerber::LabeledContour {
            shape: vec![sq(-5_000_000, -5_000_000, 70_000_000)],
            labels: gerber::ObjectLabels::default(),
        });
        let index = LabelIndex::new(&objects);
        assert_eq!(index.large, [900]);
        for bb in [
            [0, 0, 1_000_000, 1_000_000],
            [1_500_000, 1_500_000, 4_200_000, 2_100_000],
            [-9_000_000, -9_000_000, -6_000_000, -6_000_000],
            [10_000_000, 10_000_000, 90_000_000, 90_000_000],
            [59_000_000, 0, 99_000_000, 1_000],
        ] {
            let full: Vec<u32> = index
                .boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| !(b[2] < bb[0] || b[0] > bb[2] || b[3] < bb[1] || b[1] > bb[3]))
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(index.candidates(&bb), full, "query {bb:?}");
        }
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
            x2_objects: None,
            drill_tools: None,
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
    fn one_sided_negative_layer_is_reported_as_drawn() {
        // #317: a negative plane present only in the old revision is a removed
        // layer whose geometry is `removed`, exactly like a positive one — the
        // swap applies to paired negative layers only.
        let hdr = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";
        let corners = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";
        let cu = |kind: LayerKind, negative: bool| Layer {
            kind,
            label: format!("{kind:?}"),
            geometry: Arc::new(
                polygonize_gerber(format!("{hdr}{corners}M02*\n").as_bytes()).unwrap(),
            ),
            negative,
            x2_objects: None,
            drill_tools: None,
        };
        // Top copper on both sides so the same-board guard has a shared extent.
        let old = Board {
            layers: vec![
                cu(LayerKind::TopCopper, false),
                cu(LayerKind::InnerCopper(1), true),
            ],
        };
        let new = Board {
            layers: vec![cu(LayerKind::TopCopper, false)],
        };
        let d = compare_detailed(&old, &new).unwrap();
        let inner = d
            .layers
            .iter()
            .find(|l| l.kind == LayerKind::InnerCopper(1))
            .unwrap();
        assert_eq!(inner.status, LayerStatus::RemovedLayer);
        assert!(!inner.removed.is_empty() && inner.added.is_empty());
        let row = d
            .report
            .layers
            .iter()
            .find(|l| l.kind == "inner-copper" && l.inner_index == Some(1))
            .unwrap();
        assert!(row.removed_area_mm2 > 0.0 && row.added_area_mm2 == 0.0);
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
            x2_objects: None,
            drill_tools: None,
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
            x2_objects: None,
            drill_tools: None,
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

    /// A PTH drill layer, with its tool table, from Excellon source.
    fn drill_layer(src: &str) -> Layer {
        let (geometry, tools) = resolve_excellon_with_tools(src.as_bytes()).unwrap();
        Layer {
            kind: LayerKind::Drill(DrillKind::Plated),
            label: "PTH.drl".into(),
            geometry: Arc::new(geometry),
            negative: false,
            x2_objects: None,
            drill_tools: Some(tools),
        }
    }

    #[test]
    fn compare_reports_the_drill_table_across_units() {
        // Old is metric, new is inch; 0.8 mm and 0.0315 in are one size. New
        // adds a 0.8 mm hole and drops the 0.3 mm tool.
        let old = drill_layer(
            "M48\nMETRIC,TZ\nT1C0.300\nT2C0.800\n%\nT1\nX5.0Y5.0\nT2\nX50.0Y50.0\nM30\n",
        );
        let new =
            drill_layer("M48\nINCH,TZ\nT2C0.0315\n%\nT2\nX0.19685Y0.19685\nX1.9685Y1.9685\nM30\n");
        let report = compare(&Board { layers: vec![old] }, &Board { layers: vec![new] }).unwrap();
        let table = report.layers[0].drill_table.as_ref().unwrap();
        let rows: Vec<_> = table
            .rows
            .iter()
            .map(|r| {
                (
                    r.diameter_um,
                    r.change,
                    r.counts.old_holes,
                    r.counts.new_holes,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (300, DrillRowChange::Removed, 1, 0),
                (800, DrillRowChange::Changed, 1, 2),
            ]
        );
        assert!(report.to_json().contains("\"drill_table\""));
    }

    #[test]
    fn drill_table_needs_a_tool_table_on_every_present_side() {
        // An Excellon file paired with a Gerber-format drill file: no table,
        // rather than every size reading as added or removed.
        let excellon = drill_layer("M48\nMETRIC,TZ\nT1C0.300\n%\nT1\nX5.0Y5.0\nX50.0Y50.0\nM30\n");
        let mut gerber = excellon.clone();
        gerber.drill_tools = None;
        let report = compare(
            &Board {
                layers: vec![excellon.clone()],
            },
            &Board {
                layers: vec![gerber],
            },
        )
        .unwrap();
        assert!(report.layers[0].drill_table.is_none());
        assert!(!report.to_json().contains("drill_table"));
        // A one-sided Excellon layer gets its table, every size removed.
        let copper = cu_layer(
            LayerKind::TopCopper,
            "F_Cu",
            "X5000000Y5000000D03*\nX50000000Y50000000D03*\n",
        );
        let report = compare(
            &Board {
                layers: vec![copper.clone(), excellon],
            },
            &Board {
                layers: vec![copper],
            },
        )
        .unwrap();
        let drill = report
            .layers
            .iter()
            .find(|l| l.kind == "drill-pth")
            .unwrap();
        assert_eq!(drill.status, LayerStatus::RemovedLayer);
        let table = drill.drill_table.as_ref().unwrap();
        assert_eq!(table.rows[0].change, DrillRowChange::Removed);
        assert_eq!(table.total.old_holes, 2);
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
                x2_objects: None,
                drill_tools: None,
            }],
        };
        let new = Board {
            layers: vec![Layer {
                kind: LayerKind::TopCopper,
                label: "F_Cu".into(),
                geometry: Arc::clone(&gb),
                negative: false,
                x2_objects: None,
                drill_tools: None,
            }],
        };
        let diff = compare_detailed(&old, &new).unwrap();
        // Same allocation, not a copy: the view points at the board's geometry.
        assert!(Arc::ptr_eq(&diff.layers[0].old, &ga));
        assert!(Arc::ptr_eq(&diff.layers[0].new, &gb));
    }
}
