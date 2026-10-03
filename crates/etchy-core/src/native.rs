//! Format-neutral stored board objects and native-input diagnostics.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::model::LayerKind;

pub const NM_PER_MM: i64 = 1_000_000;
pub const MICRODEGREES_PER_TURN: i64 = 360_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UnitConversionError {
    #[error("invalid millimetre value: {0}")]
    InvalidMillimetres(String),
    #[error("millimetre value has precision below 1 nm: {0}")]
    SubNanometrePrecision(String),
    #[error("millimetre value is outside the i64 nanometre range: {0}")]
    CoordinateOverflow(String),
    #[error("angle is not finite")]
    NonFiniteAngle,
    #[error("angle is outside the supported range")]
    AngleOverflow,
}

pub fn millimetres_text_to_nm(text: &str) -> Result<i64, UnitConversionError> {
    let value = text.trim();
    if value.is_empty() {
        return Err(UnitConversionError::InvalidMillimetres(text.into()));
    }
    let (negative, unsigned) = match value.as_bytes()[0] {
        b'-' => (true, &value[1..]),
        b'+' => (false, &value[1..]),
        _ => (false, value),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(UnitConversionError::InvalidMillimetres(text.into()));
    }
    let significant_fraction = fraction.trim_end_matches('0');
    if significant_fraction.len() > 6 {
        return Err(UnitConversionError::SubNanometrePrecision(text.into()));
    }
    let whole: u128 = whole
        .parse()
        .map_err(|_| UnitConversionError::CoordinateOverflow(text.into()))?;
    let fraction_digits = &fraction[..fraction.len().min(6)];
    let fraction_value: u128 = if fraction_digits.is_empty() {
        0
    } else {
        fraction_digits
            .parse()
            .map_err(|_| UnitConversionError::InvalidMillimetres(text.into()))?
    };
    let scale = 10_u128.pow((6 - fraction_digits.len()) as u32);
    let magnitude = whole
        .checked_mul(NM_PER_MM as u128)
        .and_then(|v| v.checked_add(fraction_value * scale))
        .ok_or_else(|| UnitConversionError::CoordinateOverflow(text.into()))?;
    let signed = if negative {
        if magnitude == i64::MAX as u128 + 1 {
            i64::MIN
        } else if magnitude <= i64::MAX as u128 {
            -(magnitude as i64)
        } else {
            return Err(UnitConversionError::CoordinateOverflow(text.into()));
        }
    } else if magnitude <= i64::MAX as u128 {
        magnitude as i64
    } else {
        return Err(UnitConversionError::CoordinateOverflow(text.into()));
    };
    Ok(signed)
}

pub fn millimetres_value_to_nm(value: f64) -> Result<i64, UnitConversionError> {
    if !value.is_finite() {
        return Err(UnitConversionError::InvalidMillimetres(value.to_string()));
    }
    let nm = value * NM_PER_MM as f64;
    if nm < i64::MIN as f64 || nm >= i64::MAX as f64 {
        return Err(UnitConversionError::CoordinateOverflow(value.to_string()));
    }
    Ok(nm.round() as i64)
}

pub fn normalize_angle_udeg(angle_udeg: i64) -> i32 {
    angle_udeg.rem_euclid(MICRODEGREES_PER_TURN) as i32
}

pub fn degrees_value_to_udeg(value: f64) -> Result<i32, UnitConversionError> {
    if !value.is_finite() {
        return Err(UnitConversionError::NonFiniteAngle);
    }
    let scaled = value * 1_000_000.0;
    if scaled < i64::MIN as f64 || scaled >= i64::MAX as f64 {
        return Err(UnitConversionError::AngleOverflow);
    }
    Ok(normalize_angle_udeg(scaled.round() as i64))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeFormat {
    KicadPcb,
    AltiumPcbdoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardSide {
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointNm {
    pub x: i64,
    pub y: i64,
}

impl Serialize for PointNm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        [self.x, self.y].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PointNm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let [x, y] = <[i64; 2]>::deserialize(deserializer)?;
        Ok(Self { x, y })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: u64,
    pub end: u64,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSource {
    pub id: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeLayerKind {
    Standard(LayerKind),
    BlindDrill,
    BuriedDrill,
    MicroDrill,
    Fabrication,
    Documentation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeLayer {
    pub id: String,
    pub name: String,
    pub kind: NativeLayerKind,
    pub side: Option<BoardSide>,
    pub copper_position: Option<u16>,
    pub drill_span: Option<LayerSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSpan {
    pub start_layer_id: String,
    pub end_layer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drill {
    pub diameter_nm: i64,
    pub size_y_nm: Option<i64>,
    pub offset_nm: Option<PointNm>,
    pub plated: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PadShape {
    Circle,
    Rectangle,
    Oval,
    Trapezoid,
    RoundedRectangle,
    ChamferedRectangle,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViaKind {
    Through,
    Blind,
    Buried,
    Micro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontKind {
    Stroke,
    TrueType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footprint {
    pub source: ObjectSource,
    pub reference: String,
    pub value: String,
    pub position_nm: PointNm,
    pub rotation_udeg: i32,
    pub side: BoardSide,
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pad {
    pub source: ObjectSource,
    pub parent_footprint_ref: String,
    pub number: String,
    pub shape: PadShape,
    pub position_nm: PointNm,
    pub size_nm: PointNm,
    pub layer_ids: Vec<String>,
    pub net_label: Option<String>,
    pub drill: Option<Drill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub source: ObjectSource,
    pub layer_id: String,
    pub start_nm: PointNm,
    pub end_nm: PointNm,
    pub arc_mid_nm: Option<PointNm>,
    pub width_nm: i64,
    pub net_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Via {
    pub source: ObjectSource,
    pub kind: ViaKind,
    pub position_nm: PointNm,
    pub span: LayerSpan,
    pub diameter_nm: i64,
    pub drill_nm: i64,
    pub net_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zone {
    pub source: ObjectSource,
    pub layer_ids: Vec<String>,
    pub net_label: Option<String>,
    pub keepout: bool,
    pub has_saved_fill: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub source: ObjectSource,
    pub layer_id: String,
    pub content: String,
    pub font_kind: FontKind,
    pub rendered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "kebab-case")]
pub enum NativeObject {
    Footprint(Footprint),
    Pad(Pad),
    Track(Track),
    Via(Via),
    Zone(Zone),
    Text(Text),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordDisposition {
    Projected,
    ObjectOnly,
    IgnoredByRule,
    WarnedUnprojected,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCount {
    pub token_path: String,
    pub disposition: RecordDisposition,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecordAccounting {
    pub total_records: u64,
    pub records: Vec<RecordCount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSeverity {
    Low,
    Medium,
    High,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeDiagnosticCode {
    SavedFillFreshnessUnverified,
    UnfilledZoneOmitted,
    TextNotRendered,
    ExternalTextVariable,
    AmbiguousObjectIdentity,
    AmbiguousDrillPlating,
    IgnoredMetadata,
    NativeExportPolicyDifference,
    MalformedInput,
    UnsupportedBoardVersion,
    UnknownMaterialRecord,
    UnsupportedObjectSemantics,
    NumericOverflow,
    ResourceLimit,
    MissingZoneFill,
    ContradictoryZoneFill,
    AmbiguousLayerStack,
    InvalidOutline,
    IndeterminateUnusedLayerCopper,
    BoardMismatch,
}

impl NativeDiagnosticCode {
    pub const fn id(self) -> &'static str {
        match self {
            Self::SavedFillFreshnessUnverified => "saved-fill-freshness-unverified",
            Self::UnfilledZoneOmitted => "unfilled-zone-omitted",
            Self::TextNotRendered => "text-not-rendered",
            Self::ExternalTextVariable => "external-text-variable",
            Self::AmbiguousObjectIdentity => "ambiguous-object-identity",
            Self::AmbiguousDrillPlating => "ambiguous-drill-plating",
            Self::IgnoredMetadata => "ignored-metadata",
            Self::NativeExportPolicyDifference => "native-export-policy-difference",
            Self::MalformedInput => "malformed-input",
            Self::UnsupportedBoardVersion => "unsupported-board-version",
            Self::UnknownMaterialRecord => "unknown-material-record",
            Self::UnsupportedObjectSemantics => "unsupported-object-semantics",
            Self::NumericOverflow => "numeric-overflow",
            Self::ResourceLimit => "resource-limit",
            Self::MissingZoneFill => "missing-zone-fill",
            Self::ContradictoryZoneFill => "contradictory-zone-fill",
            Self::AmbiguousLayerStack => "ambiguous-layer-stack",
            Self::InvalidOutline => "invalid-outline",
            Self::IndeterminateUnusedLayerCopper => "indeterminate-unused-layer-copper",
            Self::BoardMismatch => "board-mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDiagnostic {
    pub code: NativeDiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source_path: Option<String>,
    pub affected_ids: Vec<String>,
    pub affected_layers: Vec<String>,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionProvenance {
    pub policy_version: String,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeBoard {
    pub source_format: NativeFormat,
    pub format_version: String,
    pub producer: String,
    pub producer_version: String,
    pub layers: Vec<NativeLayer>,
    pub net_labels: BTreeMap<i64, String>,
    pub objects: Vec<NativeObject>,
    pub diagnostics: Vec<NativeDiagnostic>,
    pub record_accounting: RecordAccounting,
    pub projection_provenance: ProjectionProvenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_coordinates_cover_one_nm_and_integer_limits() {
        assert_eq!(millimetres_text_to_nm("0.000001"), Ok(1));
        assert_eq!(millimetres_text_to_nm("-0.000001"), Ok(-1));
        assert_eq!(millimetres_text_to_nm("9223372036854.775807"), Ok(i64::MAX));
        assert_eq!(
            millimetres_text_to_nm("-9223372036854.775808"),
            Ok(i64::MIN)
        );
        assert!(matches!(
            millimetres_text_to_nm("9223372036854.775808"),
            Err(UnitConversionError::CoordinateOverflow(_))
        ));
        assert!(matches!(
            millimetres_text_to_nm("0.0000001"),
            Err(UnitConversionError::SubNanometrePrecision(_))
        ));
    }

    #[test]
    fn numeric_coordinates_round_to_one_nm_and_reject_overflow() {
        assert_eq!(millimetres_value_to_nm(0.000_001), Ok(1));
        assert_eq!(millimetres_value_to_nm(-0.000_001), Ok(-1));
        assert!(matches!(
            millimetres_value_to_nm(f64::INFINITY),
            Err(UnitConversionError::InvalidMillimetres(_))
        ));
        assert!(matches!(
            millimetres_value_to_nm(i64::MAX as f64),
            Err(UnitConversionError::CoordinateOverflow(_))
        ));
    }

    #[test]
    fn angles_normalize_modulo_one_turn() {
        assert_eq!(normalize_angle_udeg(-1), 359_999_999);
        assert_eq!(normalize_angle_udeg(360_000_001), 1);
        assert_eq!(normalize_angle_udeg(0), 0);
        assert_eq!(normalize_angle_udeg(360_000_000), 0);
        assert_eq!(normalize_angle_udeg(-720_000_000), 0);
        assert_eq!(degrees_value_to_udeg(-90.0), Ok(270_000_000));
        assert_eq!(degrees_value_to_udeg(12.345678), Ok(12_345_678));
    }

    #[test]
    fn board_round_trips_with_source_ids_and_accounting() {
        let board = NativeBoard {
            source_format: NativeFormat::KicadPcb,
            format_version: "20260101".into(),
            producer: "KiCad".into(),
            producer_version: "10.0".into(),
            layers: vec![],
            net_labels: BTreeMap::from([(1, "GND".into())]),
            objects: vec![NativeObject::Text(Text {
                source: ObjectSource {
                    id: "text-1".into(),
                    span: SourceSpan {
                        start: 4,
                        end: 12,
                        line: Some(1),
                        column: Some(5),
                    },
                },
                layer_id: "F.SilkS".into(),
                content: "hello".into(),
                font_kind: FontKind::Stroke,
                rendered: true,
            })],
            diagnostics: vec![],
            record_accounting: RecordAccounting {
                total_records: 1,
                records: vec![RecordCount {
                    token_path: "kicad_pcb/gr_text".into(),
                    disposition: RecordDisposition::Projected,
                    count: 1,
                }],
            },
            projection_provenance: ProjectionProvenance {
                policy_version: "kicad-native-v1".into(),
                rules: vec!["board design rules, not plot settings".into()],
            },
        };
        let json = serde_json::to_string(&board).unwrap();
        let decoded: NativeBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, board);
    }
}
