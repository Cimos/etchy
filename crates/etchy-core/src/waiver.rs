//! Waivers (#425): accept known changes so a CI gate can still pass.
//!
//! A [`Waiver`] is a box on one layer, in board millimetres, with a required
//! reason. [`apply_waivers`] sorts every piece of changed geometry on a
//! [`BoardDiff`] into three groups:
//!
//! - **waived**: its bounding box lies entirely inside one waiver's box;
//! - **partial**: its bounding box crosses a waiver's box but is not inside any
//!   single one, so it is *not* waived and is reported as such;
//! - **unwaived**: everything else.
//!
//! Containment is all or nothing so a waiver can never hide part of a change
//! that spills out of it (fail loud). Waived changes are never dropped: the
//! returned [`WaiverReport`] lists each waiver with its region count, area and
//! reason, and a waiver that matched nothing is marked stale (TRUST-3).
//!
//! Pure: the caller reads and parses the waiver file and decides what the
//! report does to the exit code.

use serde::Serialize;

use crate::diff::{nm2_to_mm2, REGION_NOISE_FLOOR_NM2};
use crate::export::xml_escape;
use crate::geo::{shoelace_2x_nm2, Shape, NM_PER_MM};
use crate::report::{md_code_span, RegionChange};
use crate::view::BoardDiff;

/// Layer names a waiver may use: the display names etchy reports, plus
/// `inner-copperN` (N ≥ 1).
pub const WAIVER_LAYER_NAMES: &[&str] = &[
    "top-copper",
    "bottom-copper",
    "top-mask",
    "bottom-mask",
    "top-silk",
    "bottom-silk",
    "top-paste",
    "bottom-paste",
    "drill",
    "drill-pth",
    "drill-npth",
    "outline",
    "documentation",
    "placement",
    "other",
];

/// Coordinates beyond ±1 m are certainly a typo (or a unit mix-up).
const MAX_COORD_MM: f64 = 1_000.0;

/// Why a waiver was rejected. Every one is a hard error for the caller.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WaiverError {
    #[error(
        "unknown layer `{0}`; use a layer name as etchy reports it: {names}, or inner-copperN",
        names = WAIVER_LAYER_NAMES.join(", ")
    )]
    UnknownLayer(String),
    #[error("malformed box: {0}")]
    BadBox(String),
    #[error("a waiver needs a non-empty `reason`")]
    EmptyReason,
}

/// One accepted change: a box on one layer, in board millimetres.
#[derive(Debug, Clone, PartialEq)]
pub struct Waiver {
    pub layer: String,
    /// `[x_min, y_min, x_max, y_max]` in mm, in the board's own coordinates
    /// (the same numbers etchy prints as a region's bbox).
    pub box_mm: [f64; 4],
    pub reason: String,
    box_nm: [i64; 4],
}

impl Waiver {
    /// Validate and build a waiver. Rejects an unknown layer name, a box that is
    /// not finite, inverted, zero-width or absurdly large, and an empty reason.
    pub fn new(layer: &str, box_mm: [f64; 4], reason: &str) -> Result<Self, WaiverError> {
        if !is_layer_name(layer) {
            return Err(WaiverError::UnknownLayer(layer.to_string()));
        }
        if let Some(v) = box_mm.iter().find(|v| !v.is_finite()) {
            return Err(WaiverError::BadBox(format!("{v} is not a finite number")));
        }
        if let Some(v) = box_mm.iter().find(|v| v.abs() > MAX_COORD_MM) {
            return Err(WaiverError::BadBox(format!(
                "{v} mm is outside ±{MAX_COORD_MM} mm (coordinates are millimetres)"
            )));
        }
        let [x0, y0, x1, y1] = box_mm;
        if x0 >= x1 || y0 >= y1 {
            return Err(WaiverError::BadBox(format!(
                "[{x0}, {y0}, {x1}, {y1}] must be [x_min, y_min, x_max, y_max] with \
                 x_min < x_max and y_min < y_max"
            )));
        }
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(WaiverError::EmptyReason);
        }
        let box_nm = box_mm.map(|v| (v * NM_PER_MM as f64).round() as i64);
        Ok(Self {
            layer: layer.to_string(),
            box_mm,
            reason: reason.to_string(),
            box_nm,
        })
    }

    /// The box in integer nanometres (rounded to the nearest nm).
    pub fn box_nm(&self) -> [i64; 4] {
        self.box_nm
    }

    fn contains(&self, bb: &[i64; 4]) -> bool {
        let w = &self.box_nm;
        w[0] <= bb[0] && w[1] <= bb[1] && bb[2] <= w[2] && bb[3] <= w[3]
    }

    /// The interiors overlap (touching edges do not count).
    fn overlaps(&self, bb: &[i64; 4]) -> bool {
        let w = &self.box_nm;
        bb[0] < w[2] && w[0] < bb[2] && bb[1] < w[3] && w[1] < bb[3]
    }
}

fn is_layer_name(name: &str) -> bool {
    if WAIVER_LAYER_NAMES.contains(&name) {
        return true;
    }
    name.strip_prefix("inner-copper")
        .and_then(|n| {
            // Digits only: `u8::from_str` would also accept `+2`.
            n.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| n.parse::<u8>().ok())
                .flatten()
        })
        .is_some_and(|n| n >= 1)
}

/// One changed region a waiver accepted.
#[derive(Debug, Clone, Serialize)]
pub struct WaivedRegion {
    pub layer: String,
    pub change: RegionChange,
    pub area_mm2: f64,
    pub area_nm2: String,
    pub bbox_nm: [i64; 4],
}

/// One waiver from the file and everything it matched.
#[derive(Debug, Clone, Serialize)]
pub struct WaiverMatch {
    /// 1-based position in the waiver file.
    pub number: usize,
    pub layer: String,
    pub box_mm: [f64; 4],
    pub reason: String,
    /// Regions above the noise floor that this waiver accepted (the same
    /// regions the report counts).
    pub matched_regions: u32,
    /// Sub-floor slivers it accepted: not counted as regions, but their area is
    /// in `matched_area_*`.
    pub matched_slivers: u32,
    pub matched_area_mm2: f64,
    pub matched_area_nm2: String,
    /// True when the waiver matched nothing at all.
    pub stale: bool,
    /// The matched regions, largest first.
    pub regions: Vec<WaivedRegion>,
}

/// A changed region that crosses a waiver's box without lying inside any one
/// waiver. It is **not** waived.
#[derive(Debug, Clone, Serialize)]
pub struct PartialOverlap {
    pub layer: String,
    pub change: RegionChange,
    pub area_mm2: f64,
    pub area_nm2: String,
    pub bbox_nm: [i64; 4],
    /// Numbers of the waivers whose boxes it crosses.
    pub waivers: Vec<usize>,
}

/// Waived and unwaived change on one changed layer. The CI gate reads the
/// unwaived figures.
#[derive(Debug, Clone, Serialize)]
pub struct LayerWaiverTotals {
    pub layer: String,
    /// Stable kind tag (`top-silk`, `inner-copper`, …), as in the layer report.
    pub kind: &'static str,
    pub label_old: Option<String>,
    pub label_new: Option<String>,
    pub waived_regions: u32,
    pub waived_area_mm2: f64,
    pub waived_area_nm2: String,
    pub unwaived_regions: u32,
    pub unwaived_area_mm2: f64,
    pub unwaived_area_nm2: String,
    #[serde(skip)]
    pub unwaived_nm2: i128,
}

/// The result of applying a set of waivers to a diff.
#[derive(Debug, Clone, Serialize)]
pub struct WaiverReport {
    pub waived_regions: u32,
    pub waived_area_mm2: f64,
    pub waived_area_nm2: String,
    pub stale_waivers: u32,
    pub partial_overlaps: Vec<PartialOverlap>,
    pub waivers: Vec<WaiverMatch>,
    /// One entry per changed layer, in stack order.
    pub layers: Vec<LayerWaiverTotals>,
}

/// Bounding box of a shape over all its rings; `None` for an empty shape.
fn shape_bbox(shape: &Shape) -> Option<[i64; 4]> {
    let mut points = shape.iter().flatten();
    let first = points.next()?;
    let mut b = [first.x, first.y, first.x, first.y];
    for p in points {
        b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
    }
    Some(b)
}

/// Twice the shape's area (exact; halving per shape would round).
fn shape_area_2x(shape: &Shape) -> i128 {
    shape.iter().map(|r| shoelace_2x_nm2(r)).sum::<i128>().abs()
}

/// Apply `waivers` to every changed layer of `diff`. A region is waived by the
/// first waiver (in file order) whose box wholly contains its bounding box.
pub fn apply_waivers(diff: &BoardDiff, waivers: &[Waiver]) -> WaiverReport {
    let mut matches: Vec<WaiverMatch> = waivers
        .iter()
        .enumerate()
        .map(|(i, w)| WaiverMatch {
            number: i + 1,
            layer: w.layer.clone(),
            box_mm: w.box_mm,
            reason: w.reason.clone(),
            matched_regions: 0,
            matched_slivers: 0,
            matched_area_mm2: 0.0,
            matched_area_nm2: String::new(),
            stale: true,
            regions: Vec::new(),
        })
        .collect();
    let mut matched_2x = vec![0i128; waivers.len()];
    let mut partial_overlaps = Vec::new();
    let mut layers = Vec::new();

    for view in diff.layers.iter().filter(|v| v.is_changed()) {
        let name = view.name();
        let on_layer: Vec<usize> = waivers
            .iter()
            .enumerate()
            .filter(|(_, w)| w.layer == name)
            .map(|(i, _)| i)
            .collect();
        let mut waived_regions = 0u32;
        let mut waived_2x = 0i128;
        let mut unwaived_2x = 0i128;
        let mut any_waived = false;
        for (change, set) in [
            (RegionChange::Added, &view.added),
            (RegionChange::Removed, &view.removed),
        ] {
            for shape in &set.shapes {
                let Some(bb) = shape_bbox(shape) else {
                    continue;
                };
                let area_2x = shape_area_2x(shape);
                let is_region = area_2x / 2 >= REGION_NOISE_FLOOR_NM2;
                let region = || WaivedRegion {
                    layer: name.clone(),
                    change,
                    area_mm2: nm2_to_mm2(area_2x / 2),
                    area_nm2: (area_2x / 2).to_string(),
                    bbox_nm: bb,
                };
                if let Some(&i) = on_layer.iter().find(|&&i| waivers[i].contains(&bb)) {
                    any_waived = true;
                    waived_2x += area_2x;
                    matched_2x[i] += area_2x;
                    let m = &mut matches[i];
                    m.stale = false;
                    if is_region {
                        waived_regions += 1;
                        m.matched_regions += 1;
                        m.regions.push(region());
                    } else {
                        m.matched_slivers += 1;
                    }
                    continue;
                }
                unwaived_2x += area_2x;
                let crossed: Vec<usize> = on_layer
                    .iter()
                    .filter(|&&i| waivers[i].overlaps(&bb))
                    .map(|&i| i + 1)
                    .collect();
                if !crossed.is_empty() {
                    let r = region();
                    partial_overlaps.push(PartialOverlap {
                        layer: r.layer,
                        change,
                        area_mm2: r.area_mm2,
                        area_nm2: r.area_nm2,
                        bbox_nm: bb,
                        waivers: crossed,
                    });
                }
            }
        }
        let c = &view.change;
        let total_regions = c.added_region_count + c.removed_region_count;
        // With nothing waived, keep the layer's own exact total so the gate reads
        // the same number it reads without waivers.
        let unwaived_nm2 = if any_waived {
            unwaived_2x / 2
        } else {
            c.added_area_nm2 + c.removed_area_nm2
        };
        layers.push(LayerWaiverTotals {
            layer: name,
            kind: view.kind.kind_str(),
            label_old: view.label_old.clone(),
            label_new: view.label_new.clone(),
            waived_regions,
            waived_area_mm2: nm2_to_mm2(waived_2x / 2),
            waived_area_nm2: (waived_2x / 2).to_string(),
            unwaived_regions: total_regions.saturating_sub(waived_regions),
            unwaived_area_mm2: nm2_to_mm2(unwaived_nm2),
            unwaived_area_nm2: unwaived_nm2.to_string(),
            unwaived_nm2,
        });
    }

    for (m, twice) in matches.iter_mut().zip(&matched_2x) {
        m.matched_area_mm2 = nm2_to_mm2(twice / 2);
        m.matched_area_nm2 = (twice / 2).to_string();
        m.regions.sort_by(|a, b| {
            b.area_mm2
                .total_cmp(&a.area_mm2)
                .then_with(|| a.bbox_nm.cmp(&b.bbox_nm))
        });
    }
    let waived_2x: i128 = matched_2x.iter().sum();
    WaiverReport {
        waived_regions: matches.iter().map(|m| m.matched_regions).sum(),
        waived_area_mm2: nm2_to_mm2(waived_2x / 2),
        waived_area_nm2: (waived_2x / 2).to_string(),
        stale_waivers: matches.iter().filter(|m| m.stale).count() as u32,
        partial_overlaps,
        waivers: matches,
        layers,
    }
}

fn mm_text(v: [i64; 4]) -> String {
    v.map(|n| format!("{:.4}", n as f64 / 1e6)).join(", ")
}

fn box_text(b: [f64; 4]) -> String {
    b.map(|v| format!("{v:.4}")).join(", ")
}

fn waivers_list(nums: &[usize]) -> String {
    nums.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl WaiverReport {
    fn status(m: &WaiverMatch) -> String {
        if m.stale {
            "stale: matched nothing".to_string()
        } else if m.matched_slivers > 0 {
            format!("matched (+{} sub-floor sliver(s))", m.matched_slivers)
        } else {
            "matched".to_string()
        }
    }

    /// Warnings for the report's warning list: one per stale waiver and one per
    /// waiver that a change crosses without being waived.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for m in self.waivers.iter().filter(|m| m.stale) {
            out.push(format!(
                "stale waiver #{} ({}, \"{}\") matched no changed region; remove it if the \
                 change it accepted is gone",
                m.number, m.layer, m.reason
            ));
        }
        for m in &self.waivers {
            let n = self
                .partial_overlaps
                .iter()
                .filter(|p| p.waivers.contains(&m.number))
                .count();
            if n > 0 {
                out.push(format!(
                    "{n} changed region(s) on {} partially overlap waiver #{} and are NOT waived; \
                     a region is waived only when it lies entirely inside a waiver box",
                    m.layer, m.number
                ));
            }
        }
        out
    }

    /// Plain-text block for the terminal summary.
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "waivers: {} region(s) waived, {:.5} mm²; {} stale waiver(s); {} region(s) partially \
             overlap a waiver (not waived)",
            self.waived_regions,
            self.waived_area_mm2,
            self.stale_waivers,
            self.partial_overlaps.len()
        );
        for m in &self.waivers {
            let _ = writeln!(
                s,
                "  #{} {} [{}] — {} region(s), {:.5} mm² — {} — reason: {}",
                m.number,
                m.layer,
                box_text(m.box_mm),
                m.matched_regions,
                m.matched_area_mm2,
                Self::status(m),
                m.reason
            );
        }
        for p in &self.partial_overlaps {
            let _ = writeln!(
                s,
                "  not waived: {} {} {:.5} mm² bbox [{}] partially overlaps waiver {}",
                p.layer,
                change_str(p.change),
                p.area_mm2,
                mm_text(p.bbox_nm),
                waivers_list(&p.waivers)
            );
        }
        s
    }

    /// Markdown section (GitHub-flavoured). The reason is user text that ends up
    /// in a bot comment, so it goes in a code span.
    pub fn to_markdown(&self, limit: usize) -> String {
        let mut s = format!(
            "\n### Waivers — {} region(s) waived, {:.4} mm²\n\n",
            self.waived_regions, self.waived_area_mm2
        );
        s.push_str("| # | layer | box mm [x₀,y₀,x₁,y₁] | reason | regions | area mm² | status |\n");
        s.push_str("|--:|---|---|---|--:|--:|---|\n");
        for m in &self.waivers {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {:.4} | {} |\n",
                m.number,
                m.layer,
                box_text(m.box_mm),
                md_code_span(&m.reason).replace('|', "\\|"),
                m.matched_regions,
                m.matched_area_mm2,
                if m.stale {
                    "⚠️ stale: matched nothing".to_string()
                } else {
                    Self::status(m)
                },
            ));
        }
        let waived = self.top_waived(limit);
        if !waived.is_empty() {
            s.push_str(&format!(
                "\n#### Waived regions (top {limit})\n\n\
                 | layer | change | area mm² | bbox mm [x₀,y₀,x₁,y₁] | waiver |\n\
                 |---|---|--:|---|--:|\n"
            ));
            for (n, r) in waived {
                s.push_str(&format!(
                    "| {} | {} | {:.4} | {} | #{} |\n",
                    r.layer,
                    change_str(r.change),
                    r.area_mm2,
                    mm_text(r.bbox_nm),
                    n
                ));
            }
        }
        if !self.partial_overlaps.is_empty() {
            s.push_str(&format!(
                "\n#### ⚠️ Not waived — {} region(s) partially overlap a waiver\n\n\
                 | layer | change | area mm² | bbox mm [x₀,y₀,x₁,y₁] | overlaps |\n\
                 |---|---|--:|---|---|\n",
                self.partial_overlaps.len()
            ));
            for p in self.partial_overlaps.iter().take(limit) {
                s.push_str(&format!(
                    "| {} | {} | {:.4} | {} | {} |\n",
                    p.layer,
                    change_str(p.change),
                    p.area_mm2,
                    mm_text(p.bbox_nm),
                    waivers_list(&p.waivers)
                ));
            }
            if self.partial_overlaps.len() > limit {
                s.push_str(&format!(
                    "\n…and {} more (see the JSON report).\n",
                    self.partial_overlaps.len() - limit
                ));
            }
        }
        s
    }

    /// Self-contained HTML section for the report.
    pub fn to_html(&self, limit: usize) -> String {
        let mut s = format!(
            "<section><h2>Waivers</h2><div class=\"mag\">{} region(s) waived, {:.4} mm&sup2;; \
             {} stale; {} region(s) partially overlap a waiver (not waived)</div>\
             <table><thead><tr><th>#</th><th>Layer</th><th>Box mm</th><th>Reason</th>\
             <th>Regions</th><th>Area mm²</th><th>Status</th></tr></thead><tbody>",
            self.waived_regions,
            self.waived_area_mm2,
            self.stale_waivers,
            self.partial_overlaps.len()
        );
        for m in &self.waivers {
            s.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.4}</td>\
                 <td>{}</td></tr>",
                m.number,
                xml_escape(&m.layer),
                box_text(m.box_mm),
                xml_escape(&m.reason),
                m.matched_regions,
                m.matched_area_mm2,
                xml_escape(&Self::status(m)),
            ));
        }
        s.push_str("</tbody></table>");
        let waived = self.top_waived(limit);
        if !waived.is_empty() {
            s.push_str(&format!(
                "<h3>Waived regions (top {limit})</h3><table><thead><tr><th>Layer</th>\
                 <th>Change</th><th>Area mm²</th><th>Bbox mm</th><th>Waiver</th></tr></thead><tbody>"
            ));
            for (n, r) in waived {
                s.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{:.4}</td><td>{}</td><td>#{}</td></tr>",
                    xml_escape(&r.layer),
                    change_str(r.change),
                    r.area_mm2,
                    mm_text(r.bbox_nm),
                    n
                ));
            }
            s.push_str("</tbody></table>");
        }
        if !self.partial_overlaps.is_empty() {
            s.push_str(
                "<h3>Not waived: partially overlap a waiver</h3><table><thead><tr>\
                 <th>Layer</th><th>Change</th><th>Area mm²</th><th>Bbox mm</th><th>Overlaps</th>\
                 </tr></thead><tbody>",
            );
            for p in self.partial_overlaps.iter().take(limit) {
                s.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{:.4}</td><td>{}</td><td>{}</td></tr>",
                    xml_escape(&p.layer),
                    change_str(p.change),
                    p.area_mm2,
                    mm_text(p.bbox_nm),
                    waivers_list(&p.waivers)
                ));
            }
            s.push_str("</tbody></table>");
            if self.partial_overlaps.len() > limit {
                s.push_str(&format!(
                    "<p>…and {} more (see the JSON report).</p>",
                    self.partial_overlaps.len() - limit
                ));
            }
        }
        s.push_str("</section>\n");
        s
    }

    /// The `limit` largest waived regions across all waivers, with the waiver
    /// number that accepted each.
    fn top_waived(&self, limit: usize) -> Vec<(usize, &WaivedRegion)> {
        let mut all: Vec<(usize, &WaivedRegion)> = self
            .waivers
            .iter()
            .flat_map(|m| m.regions.iter().map(move |r| (m.number, r)))
            .collect();
        all.sort_by(|a, b| {
            b.1.area_mm2
                .total_cmp(&a.1.area_mm2)
                .then_with(|| a.1.bbox_nm.cmp(&b.1.bbox_nm))
        });
        all.truncate(limit);
        all
    }
}

fn change_str(c: RegionChange) -> &'static str {
    match c {
        RegionChange::Added => "added",
        RegionChange::Removed => "removed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::LayerChange;
    use crate::geo::{PolygonSet, Pt};
    use crate::model::LayerKind;
    use crate::report::{DiffReport, LayerStatus};
    use crate::view::LayerView;
    use std::sync::Arc;

    const MM: i64 = 1_000_000;

    fn square(x: i64, y: i64, s: i64) -> Shape {
        vec![vec![
            Pt::new(x, y),
            Pt::new(x + s, y),
            Pt::new(x + s, y + s),
            Pt::new(x, y + s),
        ]]
    }

    /// A one-layer diff with the given added shapes on `kind`.
    fn diff_with(kind: LayerKind, added: Vec<Shape>) -> BoardDiff {
        let added = PolygonSet::new(added);
        let regions = added
            .shapes
            .iter()
            .filter(|s| shape_area_2x(s) / 2 >= REGION_NOISE_FLOOR_NM2)
            .count() as u32;
        let change = LayerChange {
            added_area_nm2: added.area_nm2(),
            removed_area_nm2: 0,
            added_region_count: regions,
            removed_region_count: 0,
            has_added: !added.is_empty(),
            has_removed: false,
        };
        BoardDiff {
            report: DiffReport::new(vec![], vec![]),
            layers: vec![LayerView {
                kind,
                label_old: Some("a".into()),
                label_new: Some("b".into()),
                status: LayerStatus::Changed,
                old: Arc::new(PolygonSet::default()),
                new: Arc::new(PolygonSet::default()),
                added,
                removed: PolygonSet::default(),
                change,
            }],
        }
    }

    fn silk_waiver(b: [f64; 4]) -> Waiver {
        Waiver::new("top-silk", b, "date code").unwrap()
    }

    #[test]
    fn region_inside_box_is_waived() {
        let d = diff_with(LayerKind::TopSilk, vec![square(2 * MM, 2 * MM, MM)]);
        let r = apply_waivers(&d, &[silk_waiver([1.0, 1.0, 5.0, 5.0])]);
        assert_eq!(r.waived_regions, 1);
        assert_eq!(r.waived_area_nm2, (MM as i128 * MM as i128).to_string());
        assert_eq!(r.stale_waivers, 0);
        assert!(r.partial_overlaps.is_empty());
        assert_eq!(r.layers[0].unwaived_nm2, 0);
        assert_eq!(r.layers[0].unwaived_regions, 0);
        assert_eq!(r.waivers[0].regions.len(), 1);
    }

    #[test]
    fn region_on_box_edge_is_still_inside() {
        let d = diff_with(LayerKind::TopSilk, vec![square(MM, MM, MM)]);
        let r = apply_waivers(&d, &[silk_waiver([1.0, 1.0, 2.0, 2.0])]);
        assert_eq!(r.waived_regions, 1);
        assert_eq!(r.layers[0].unwaived_nm2, 0);
    }

    #[test]
    fn partial_overlap_is_not_waived() {
        // The square spans x 2..4 mm; the box stops at x = 3 mm.
        let d = diff_with(LayerKind::TopSilk, vec![square(2 * MM, 2 * MM, 2 * MM)]);
        let r = apply_waivers(&d, &[silk_waiver([1.0, 1.0, 3.0, 5.0])]);
        assert_eq!(r.waived_regions, 0);
        assert_eq!(r.layers[0].unwaived_regions, 1);
        assert_eq!(r.layers[0].unwaived_nm2, 4 * MM as i128 * MM as i128);
        assert_eq!(r.partial_overlaps.len(), 1);
        assert_eq!(r.partial_overlaps[0].waivers, vec![1]);
        // It matched nothing wholly, so it is also stale.
        assert_eq!(r.stale_waivers, 1);
        let warnings = r.warnings();
        assert!(warnings
            .iter()
            .any(|w| w.contains("partially overlap waiver #1")));
    }

    #[test]
    fn region_spanning_two_boxes_is_not_waived() {
        let d = diff_with(LayerKind::TopSilk, vec![square(2 * MM, 2 * MM, 2 * MM)]);
        let r = apply_waivers(
            &d,
            &[
                silk_waiver([1.0, 1.0, 3.0, 5.0]),
                silk_waiver([3.0, 1.0, 5.0, 5.0]),
            ],
        );
        assert_eq!(r.waived_regions, 0);
        assert_eq!(r.partial_overlaps[0].waivers, vec![1, 2]);
    }

    #[test]
    fn waiver_on_another_layer_does_not_apply_and_is_stale() {
        let d = diff_with(LayerKind::TopCopper, vec![square(2 * MM, 2 * MM, MM)]);
        let r = apply_waivers(&d, &[silk_waiver([0.0, 0.0, 10.0, 10.0])]);
        assert_eq!(r.waived_regions, 0);
        assert_eq!(r.stale_waivers, 1);
        assert!(r.partial_overlaps.is_empty());
        assert_eq!(r.layers[0].unwaived_regions, 1);
        assert!(r.warnings()[0].contains("stale waiver #1"));
    }

    #[test]
    fn mixed_layer_keeps_unwaived_area_exact() {
        let d = diff_with(
            LayerKind::TopSilk,
            vec![square(2 * MM, 2 * MM, MM), square(20 * MM, 20 * MM, 3 * MM)],
        );
        let r = apply_waivers(&d, &[silk_waiver([1.0, 1.0, 5.0, 5.0])]);
        assert_eq!(r.waived_regions, 1);
        assert_eq!(r.layers[0].unwaived_regions, 1);
        assert_eq!(r.layers[0].unwaived_nm2, 9 * MM as i128 * MM as i128);
    }

    #[test]
    fn sub_floor_sliver_inside_is_waived_by_area_not_count() {
        // 5 µm × 5 µm = 2.5e7 nm², under the 1e8 nm² region floor.
        let d = diff_with(
            LayerKind::TopSilk,
            vec![square(2 * MM, 2 * MM, MM), square(3 * MM, 3 * MM, 5_000)],
        );
        let r = apply_waivers(&d, &[silk_waiver([1.0, 1.0, 5.0, 5.0])]);
        assert_eq!(r.waived_regions, 1);
        assert_eq!(r.waivers[0].matched_slivers, 1);
        assert_eq!(r.layers[0].unwaived_nm2, 0);
        assert_eq!(r.layers[0].unwaived_regions, 0);
    }

    #[test]
    fn first_containing_waiver_wins() {
        let d = diff_with(LayerKind::TopSilk, vec![square(2 * MM, 2 * MM, MM)]);
        let r = apply_waivers(
            &d,
            &[
                silk_waiver([0.0, 0.0, 10.0, 10.0]),
                silk_waiver([1.0, 1.0, 5.0, 5.0]),
            ],
        );
        assert_eq!(r.waivers[0].matched_regions, 1);
        assert!(r.waivers[1].stale);
    }

    #[test]
    fn inner_copper_layer_names() {
        assert!(Waiver::new("inner-copper2", [0.0, 0.0, 1.0, 1.0], "x").is_ok());
        for bad in [
            "inner-copper0",
            "inner-copper",
            "inner-copper+2",
            "top_silk",
            "silk",
        ] {
            assert_eq!(
                Waiver::new(bad, [0.0, 0.0, 1.0, 1.0], "x"),
                Err(WaiverError::UnknownLayer(bad.into())),
                "{bad}"
            );
        }
        let d = diff_with(LayerKind::InnerCopper(2), vec![square(2 * MM, 2 * MM, MM)]);
        let w = Waiver::new("inner-copper2", [0.0, 0.0, 10.0, 10.0], "x").unwrap();
        assert_eq!(apply_waivers(&d, &[w]).waived_regions, 1);
    }

    #[test]
    fn malformed_waivers_are_rejected() {
        for b in [
            [0.0, 0.0, 0.0, 1.0],
            [2.0, 0.0, 1.0, 1.0],
            [0.0, f64::NAN, 1.0, 1.0],
            [0.0, 0.0, f64::INFINITY, 1.0],
            [0.0, 0.0, 5000.0, 1.0],
        ] {
            assert!(
                matches!(Waiver::new("top-silk", b, "x"), Err(WaiverError::BadBox(_))),
                "{b:?}"
            );
        }
        assert_eq!(
            Waiver::new("top-silk", [0.0, 0.0, 1.0, 1.0], "  "),
            Err(WaiverError::EmptyReason)
        );
    }

    #[test]
    fn outputs_list_waivers_and_escape_the_reason() {
        let d = diff_with(LayerKind::TopSilk, vec![square(2 * MM, 2 * MM, MM)]);
        let w = Waiver::new("top-silk", [1.0, 1.0, 5.0, 5.0], "rev <b>|@team").unwrap();
        let r = apply_waivers(&d, &[w]);
        let md = r.to_markdown(10);
        assert!(md.contains("1 region(s) waived"));
        assert!(md.contains("`rev <b>\\|@team`"));
        assert!(md.contains("Waived regions"));
        let html = r.to_html(10);
        assert!(html.contains("rev &lt;b&gt;|@team"));
        assert!(r.to_text().contains("reason: rev <b>|@team"));
    }
}
