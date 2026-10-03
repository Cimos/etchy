//! The single in-memory report that the (future) HTML, SVG and JSON outputs all
//! derive from, so the three can never disagree. `schema_version` is the
//! integration contract consumers/CI gate on.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diff::LayerChange;
use crate::model::LayerKind;
use crate::native::{BoardSide, NativeDiagnostic, PointNm, RecordAccounting};

/// Schema version of the machine-readable JSON output — the integration contract.
pub const SCHEMA_VERSION: u32 = 1;
pub const NATIVE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectKind {
    Footprint,
    Pad,
    Track,
    Arc,
    Via,
    Zone,
    Keepout,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectChangeStatus {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectChangeFlag {
    Moved,
    Rotated,
    Flipped,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectIdentity {
    pub reference: Option<String>,
    pub pad_number: Option<String>,
    pub old_id: Option<String>,
    pub new_id: Option<String>,
}

impl ObjectIdentity {
    fn sort_key(&self) -> (&str, &str, &str, &str) {
        (
            self.reference.as_deref().unwrap_or(""),
            self.pad_number.as_deref().unwrap_or(""),
            self.old_id.as_deref().unwrap_or(""),
            self.new_id.as_deref().unwrap_or(""),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSnapshot {
    pub position_nm: Option<PointNm>,
    pub angle_udeg: Option<i32>,
    pub side: Option<BoardSide>,
    pub layer_ids: Vec<String>,
    pub layer_span: Option<[String; 2]>,
    pub net_label: Option<String>,
    pub bounds_nm: Option<[i64; 4]>,
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectChange {
    pub kind: ObjectKind,
    pub status: ObjectChangeStatus,
    pub flags: Vec<ObjectChangeFlag>,
    pub identity: ObjectIdentity,
    pub old: Option<ObjectSnapshot>,
    pub new: Option<ObjectSnapshot>,
    pub changed_fields: Vec<String>,
}

impl ObjectChange {
    fn sort_key(&self) -> (ObjectKind, (&str, &str, &str, &str), &str, i64, i64) {
        let snapshot = self.old.as_ref().or(self.new.as_ref());
        let layer = snapshot
            .and_then(|s| s.layer_ids.first())
            .map(String::as_str)
            .unwrap_or("");
        let position = snapshot.and_then(|s| s.position_nm).unwrap_or(PointNm {
            x: i64::MIN,
            y: i64::MIN,
        });
        (
            self.kind,
            self.identity.sort_key(),
            layer,
            position.x,
            position.y,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeInputReport {
    pub family: String,
    pub old_format_version: String,
    pub new_format_version: String,
    pub projection_policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ObjectSummary {
    pub added: u64,
    pub removed: u64,
    pub modified: u64,
    pub by_kind: BTreeMap<ObjectKind, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDiagnosticsReport {
    pub partial: bool,
    pub warnings: Vec<NativeDiagnostic>,
    pub record_accounting: Vec<RecordAccounting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDiffReport {
    pub schema_version: u32,
    pub input: NativeInputReport,
    pub object_summary: ObjectSummary,
    pub object_changes: Vec<ObjectChange>,
    pub native_diagnostics: NativeDiagnosticsReport,
}

impl NativeDiffReport {
    pub fn empty(input: NativeInputReport) -> Self {
        Self {
            schema_version: NATIVE_SCHEMA_VERSION,
            input,
            object_summary: ObjectSummary::default(),
            object_changes: Vec::new(),
            native_diagnostics: NativeDiagnosticsReport {
                partial: false,
                warnings: Vec::new(),
                record_accounting: Vec::new(),
            },
        }
    }

    pub fn with_object_changes(
        input: NativeInputReport,
        mut object_changes: Vec<ObjectChange>,
        native_diagnostics: NativeDiagnosticsReport,
    ) -> Self {
        object_changes.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        for change in &mut object_changes {
            change.flags.sort();
            change.flags.dedup();
            change.changed_fields.sort();
            change.changed_fields.dedup();
        }
        let mut object_summary = ObjectSummary::default();
        for change in &object_changes {
            match change.status {
                ObjectChangeStatus::Added => object_summary.added += 1,
                ObjectChangeStatus::Removed => object_summary.removed += 1,
                ObjectChangeStatus::Modified => object_summary.modified += 1,
            }
            *object_summary.by_kind.entry(change.kind).or_default() += 1;
        }
        Self {
            schema_version: NATIVE_SCHEMA_VERSION,
            input,
            object_summary,
            object_changes,
            native_diagnostics,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("NativeDiffReport serializes")
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("NativeDiffReport serializes")
    }
}

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

    /// GitHub-flavoured Markdown summary, for a CI step-summary or PR comment.
    /// Pure (no I/O); the CLI just prints it. Lists only the changed layers.
    pub fn to_markdown_summary(&self) -> String {
        let t = &self.totals;
        let mut s = String::from("## etchy — PCB diff\n\n");
        if !self.any_changes {
            s.push_str("✅ **No differences found.**\n");
        } else {
            s.push_str(&format!(
                "**{} of {} layer(s) changed** · +{:.4} mm² added · −{:.4} mm² removed · {}+/{}− regions\n\n",
                t.layers_changed, t.layers_total, t.added_area_mm2, t.removed_area_mm2,
                t.added_regions, t.removed_regions,
            ));
            s.push_str("| layer | +mm² | −mm² | +regions | −regions |\n");
            s.push_str("|---|--:|--:|--:|--:|\n");
            for l in self.layers.iter().filter(|l| l.is_changed()) {
                let name = match l.inner_index {
                    Some(n) => format!("{}{}", l.kind, n),
                    None => l.kind.to_string(),
                };
                s.push_str(&format!(
                    "| {} | {:.4} | {:.4} | {} | {} |\n",
                    name, l.added_area_mm2, l.removed_area_mm2, l.added_regions, l.removed_regions,
                ));
            }
        }
        if !self.warnings.is_empty() {
            s.push_str("\n⚠️ **Warnings**\n");
            for w in &self.warnings {
                // Warnings embed on-disk filenames, and the CI action posts this
                // text verbatim as a bot-authored PR comment (#298). A code span
                // keeps `@`, `[`, `<`, `|`, `#` … inert.
                s.push_str(&format!("- {}\n", md_code_span(w)));
            }
        }
        s
    }
}

/// Wrap `s` in a Markdown code span so nothing inside it is Markdown- or
/// HTML-active (#298). Per CommonMark the fence must be a backtick run longer
/// than any run inside the content, and content that starts or ends with a
/// backtick is padded with a space so the pad, not the backtick, meets the fence.
/// A code span cannot contain a line break at this position, so `\r`/`\n` are
/// collapsed to a space first.
fn md_code_span(s: &str) -> String {
    let text = s.replace(['\r', '\n'], " ");
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_input() -> NativeInputReport {
        NativeInputReport {
            family: "kicad-pcb".into(),
            old_format_version: "20240108".into(),
            new_format_version: "20250101".into(),
            projection_policy: "kicad-native-v1".into(),
        }
    }

    fn snapshot(x: i64, layer: &str) -> ObjectSnapshot {
        ObjectSnapshot {
            position_nm: Some(PointNm { x, y: 0 }),
            angle_udeg: Some(0),
            side: Some(BoardSide::Front),
            layer_ids: vec![layer.into()],
            layer_span: None,
            net_label: None,
            bounds_nm: None,
            properties: BTreeMap::new(),
        }
    }

    fn change(kind: ObjectKind, reference: &str, x: i64) -> ObjectChange {
        ObjectChange {
            kind,
            status: ObjectChangeStatus::Modified,
            flags: vec![ObjectChangeFlag::Modified],
            identity: ObjectIdentity {
                reference: Some(reference.into()),
                pad_number: None,
                old_id: Some(format!("old-{reference}")),
                new_id: Some(format!("new-{reference}")),
            },
            old: Some(snapshot(0, "F.Cu")),
            new: Some(snapshot(x, "F.Cu")),
            changed_fields: vec!["position".into()],
        }
    }

    #[test]
    fn empty_report_has_no_changes() {
        let r = DiffReport::new(vec![], vec![]);
        assert!(!r.any_changes());
        assert_eq!(r.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn markdown_summary_reflects_changes() {
        // No changes → the clean "no differences" line, no table.
        let none = DiffReport::new(vec![], vec![]);
        let md = none.to_markdown_summary();
        assert!(md.contains("No differences found"));
        assert!(!md.contains("| layer |"));

        // A changed layer → a totals line + a table row for it; warnings listed.
        let changed = LayerReport::new(
            LayerKind::TopCopper,
            Some("f".into()),
            Some("f".into()),
            LayerStatus::Changed,
            &LayerChange {
                added_area_nm2: 1_000_000_000_000,
                removed_area_nm2: 0,
                added_region_count: 2,
                removed_region_count: 0,
                has_added: true,
                has_removed: false,
            },
        );
        let r = DiffReport::new(vec![changed], vec!["heads up".into()]);
        let md = r.to_markdown_summary();
        assert!(md.contains("1 of 1 layer(s) changed"));
        assert!(md.contains("| layer |"));
        assert!(md.contains("top-copper"));
        assert!(md.contains("heads up"));
    }

    #[test]
    fn markdown_warnings_are_inert_code_spans() {
        // #298: a hostile filename inside a warning must not become a heading,
        // link or @-mention in the bot-posted PR comment.
        let hostile = "cc @user [link](https://x)\n## heading";
        let r = DiffReport::new(vec![], vec![hostile.into()]);
        let md = r.to_markdown_summary();
        assert!(md.contains("- `cc @user [link](https://x) ## heading`\n"));
        for line in md.lines() {
            assert!(
                !line.starts_with('#') || line.starts_with("## etchy"),
                "{line}"
            );
            assert!(!line.starts_with("- ["), "{line}");
        }
        // The newline is collapsed: the warning is one bullet, not two lines.
        assert_eq!(md.lines().filter(|l| l.starts_with("- ")).count(), 1);

        // Backticks in the content get a longer fence, and an edge backtick is padded.
        let r = DiffReport::new(vec![], vec!["a ``b`` c".into(), "`edge".into()]);
        let md = r.to_markdown_summary();
        assert!(md.contains("- ```a ``b`` c```\n"));
        assert!(md.contains("- `` `edge ``\n"));

        // A plain warning is just itself in a code span.
        assert_eq!(md_code_span("heads up"), "`heads up`");
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

    #[test]
    fn v1_json_snapshot_is_byte_for_byte_stable() {
        let actual = DiffReport::new(vec![], vec![]).to_json();
        assert_eq!(
            actual,
            include_str!("../tests/expected/report-v1.json").trim_end_matches('\n')
        );
    }

    #[test]
    fn object_change_report_round_trips() {
        let report = NativeDiffReport::with_object_changes(
            native_input(),
            vec![change(ObjectKind::Footprint, "U1", 1_000_000)],
            NativeDiagnosticsReport {
                partial: false,
                warnings: vec![],
                record_accounting: vec![],
            },
        );
        let json = report.to_json();
        let decoded: NativeDiffReport = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, report);
    }

    #[test]
    fn object_changes_have_stable_order() {
        let report = NativeDiffReport::with_object_changes(
            native_input(),
            vec![
                change(ObjectKind::Via, "", 10),
                change(ObjectKind::Footprint, "U2", 0),
                change(ObjectKind::Footprint, "U1", 20),
            ],
            NativeDiagnosticsReport {
                partial: false,
                warnings: vec![],
                record_accounting: vec![],
            },
        );
        let keys: Vec<_> = report
            .object_changes
            .iter()
            .map(|c| (c.kind, c.identity.reference.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            keys,
            vec![
                (ObjectKind::Footprint, "U1"),
                (ObjectKind::Footprint, "U2"),
                (ObjectKind::Via, ""),
            ]
        );
    }

    #[test]
    fn native_v2_json_matches_example() {
        let report = NativeDiffReport::with_object_changes(
            native_input(),
            vec![change(ObjectKind::Footprint, "U1", 1_000_000)],
            NativeDiagnosticsReport {
                partial: false,
                warnings: vec![],
                record_accounting: vec![],
            },
        );
        assert_eq!(
            report.to_json_pretty(),
            include_str!("../tests/expected/native-report-v2.json").trim_end_matches('\n')
        );
    }
}
