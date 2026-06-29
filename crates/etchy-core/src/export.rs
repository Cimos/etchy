//! Headless SVG export of a layer's diff — a pure string builder (no I/O, no
//! extra dependencies). The CLI calls [`layer_svg`] per changed layer and writes
//! the returned document to disk; core itself never touches the filesystem.
//!
//! The SVG draws three stacked groups: the faint **base** board (new revision),
//! then **removed** copper (`#ff5d73`) and **added** copper (`#46d18a`) on top.
//! etchy contours are outer-CCW / holes-CW, so each shape becomes one `<path>`
//! with `fill-rule:evenodd` — holes fall out for free. Integer-nm coordinates are
//! scaled to mm (÷1e6) and the Y axis is flipped so the export reads the same way
//! up as a board does (SVG Y grows downward; board Y grows upward).

use crate::geo::{PolygonSet, NM_PER_MM};
use crate::view::{BoardDiff, LayerView};

/// Base (unchanged board) fill — faint grey so the diff colours read on top.
const BASE_FILL: &str = "#cfcfcf";
/// Removed copper (present only in the old revision). etchy red.
const REMOVED_FILL: &str = "#ff5d73";
/// Added copper (present only in the new revision). etchy green.
const ADDED_FILL: &str = "#46d18a";

/// nm → mm for an SVG coordinate, formatted compactly (trailing zeros trimmed).
fn mm(nm: i64) -> String {
    let v = nm as f64 / NM_PER_MM as f64;
    // Up to 6 dp (1 nm precision at mm scale), trimmed.
    let mut s = format!("{v:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// One `PolygonSet` → SVG `<path>` data. Each shape (outer + holes) is one
/// subpath group; `fill-rule:evenodd` on the element cuts the holes. Y is flipped
/// about `max_y` (the bbox top) so board-up maps to SVG-down.
fn paths_for(set: &PolygonSet, fill: &str, max_y: i64) -> Vec<String> {
    let mut out = Vec::new();
    for shape in &set.shapes {
        let mut d = String::new();
        for ring in shape {
            if ring.len() < 3 {
                continue;
            }
            for (i, p) in ring.iter().enumerate() {
                let cmd = if i == 0 { 'M' } else { 'L' };
                d.push_str(&format!("{cmd}{} {} ", mm(p.x), mm(max_y - p.y)));
            }
            d.push('Z');
        }
        if !d.is_empty() {
            out.push(format!(
                "<path d=\"{}\" fill=\"{fill}\" fill-rule=\"evenodd\"/>",
                d.trim_end()
            ));
        }
    }
    out
}

/// Combined bounding box of a layer's base + diff geometry, in nm, or `None` when
/// the layer is entirely empty.
fn layer_bbox(layer: &LayerView) -> Option<[i64; 4]> {
    let mut bb: Option<[i64; 4]> = None;
    for set in [&layer.new, &layer.removed, &layer.added] {
        if let Some(b) = set.bbox_nm() {
            bb = Some(match bb {
                None => b,
                Some(a) => [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ],
            });
        }
    }
    bb
}

/// Render one paired layer's diff to a standalone SVG document string.
///
/// Pure: no I/O. The CLI writes the result to `<layer>.svg`. The viewBox is in
/// mm, computed from the layer's combined bounding box; an empty layer yields a
/// minimal valid (1×1) document so callers never have to special-case it.
pub fn layer_svg(layer: &LayerView) -> String {
    let bb = layer_bbox(layer).unwrap_or([0, 0, NM_PER_MM, NM_PER_MM]);
    let [min_x, min_y, max_x, max_y] = bb;
    let w_nm = (max_x - min_x).max(1);
    let h_nm = (max_y - min_y).max(1);

    let mut body = Vec::new();
    body.extend(paths_for(&layer.new, BASE_FILL, max_y));
    body.extend(paths_for(&layer.removed, REMOVED_FILL, max_y));
    body.extend(paths_for(&layer.added, ADDED_FILL, max_y));

    // viewBox: min-x/min-y in mm (Y origin shifts to 0 after the flip), width/height in mm.
    let view_box = format!("{} {} {} {}", mm(min_x), mm(0), mm(w_nm), mm(h_nm));
    let mut doc = String::new();
    doc.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{view_box}\" \
         width=\"{}mm\" height=\"{}mm\">\n",
        mm(w_nm),
        mm(h_nm)
    ));
    doc.push_str(&format!(
        "<title>etchy diff — {}</title>\n",
        xml_escape(&layer.name())
    ));
    for p in body {
        doc.push_str(&p);
        doc.push('\n');
    }
    doc.push_str("</svg>\n");
    doc
}

/// Minimal XML text escaping for the `<title>`.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Per-layer copper-area report as CSV (#60), for downstream use (e.g. a thermal
/// estimate). `old_copper_mm2`/`new_copper_mm2` are the **unioned** total copper on
/// each side (overlaps counted once — see [`PolygonSet::copper_area_mm2`]);
/// `added`/`removed` are the diff magnitudes (already clean from the boolean engine).
pub fn board_areas_csv(diff: &BoardDiff) -> String {
    let mut s = String::from("layer,old_copper_mm2,new_copper_mm2,added_mm2,removed_mm2\n");
    for l in &diff.layers {
        s.push_str(&format!(
            "{},{:.5},{:.5},{:.5},{:.5}\n",
            l.name(),
            l.old.copper_area_mm2(),
            l.new.copper_area_mm2(),
            l.added.area_mm2(),
            l.removed.area_mm2(),
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Pt;
    use crate::model::LayerKind;
    use crate::report::LayerStatus;

    fn square(x: i64, y: i64, s: i64) -> Vec<Vec<Pt>> {
        vec![vec![
            Pt::new(x, y),
            Pt::new(x + s, y),
            Pt::new(x + s, y + s),
            Pt::new(x, y + s),
        ]]
    }

    fn synthetic_layer() -> LayerView {
        let s = 1_000_000; // 1 mm
                           // base/new: square at origin; removed: small square; added: small square.
        let new = PolygonSet::new(vec![square(0, 0, 4 * s)]);
        let removed = PolygonSet::new(vec![square(s, s, s)]);
        let added = PolygonSet::new(vec![square(2 * s, 2 * s, s)]);
        LayerView {
            kind: LayerKind::TopCopper,
            label_old: Some("F_Cu".into()),
            label_new: Some("F_Cu".into()),
            status: LayerStatus::Changed,
            old: std::sync::Arc::new(PolygonSet::default()),
            new: std::sync::Arc::new(new),
            added,
            removed,
            change: crate::diff::LayerChange::default(),
        }
    }

    #[test]
    fn svg_has_expected_paths_viewbox_and_colours() {
        let svg = layer_svg(&synthetic_layer());

        // One <path> per shape: base + removed + added = 3.
        assert_eq!(svg.matches("<path").count(), 3, "one path per shape");

        // mm-scaled viewBox: 4 mm board → width/height 4 mm.
        assert!(
            svg.contains("viewBox=\"0 0 4 4\""),
            "viewBox should be mm-scaled (0 0 4 4); got:\n{svg}"
        );
        assert!(svg.contains("width=\"4mm\""));

        // Diff colours present.
        assert!(svg.contains(ADDED_FILL), "added colour #46d18a");
        assert!(svg.contains(REMOVED_FILL), "removed colour #ff5d73");
        // evenodd so holes fall out.
        assert!(svg.contains("fill-rule=\"evenodd\""));
        assert!(svg.trim_start().starts_with("<svg"));
    }

    #[test]
    fn empty_layer_yields_valid_minimal_svg() {
        let layer = LayerView {
            kind: LayerKind::TopCopper,
            label_old: None,
            label_new: None,
            status: LayerStatus::Unchanged,
            old: std::sync::Arc::new(PolygonSet::default()),
            new: std::sync::Arc::new(PolygonSet::default()),
            added: PolygonSet::default(),
            removed: PolygonSet::default(),
            change: crate::diff::LayerChange::default(),
        };
        let svg = layer_svg(&layer);
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert_eq!(svg.matches("<path").count(), 0);
    }

    #[test]
    fn y_is_flipped_not_upside_down() {
        // A point at the board top (max y) must map to SVG y=0 (top of the doc).
        let s = 1_000_000;
        let layer = LayerView {
            kind: LayerKind::TopCopper,
            label_old: None,
            label_new: Some("F_Cu".into()),
            status: LayerStatus::Changed,
            old: std::sync::Arc::new(PolygonSet::default()),
            new: std::sync::Arc::new(PolygonSet::new(vec![square(0, 0, 2 * s)])),
            added: PolygonSet::default(),
            removed: PolygonSet::default(),
            change: crate::diff::LayerChange::default(),
        };
        let svg = layer_svg(&layer);
        // The first vertex (0,0) in board space is the BOTTOM; flipped it is y=2.
        assert!(
            svg.contains("M0 2 "),
            "board-bottom maps to SVG y=max; got:\n{svg}"
        );
    }
}
