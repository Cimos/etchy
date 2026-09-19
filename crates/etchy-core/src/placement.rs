//! Pick-and-place (component centroid) front-end: bytes → placement-marker
//! geometry.
//!
//! P&P files list each component's centroid + rotation. etchy renders each as a
//! small **rotated-rectangle marker** at its position, so the ordinary polygon
//! diff surfaces added / removed / moved / rotated parts in the overlay, the HTML
//! report, the magnitudes, and the CI gate — no separate component-diff model.
//! This keeps P&P as *geometry*, deliberately not a BOM/component-list tool (a
//! stated non-goal): we diff where parts sit, not what they are.
//!
//! Two input shapes are handled: KiCad-style whitespace `.pos` and comma-separated
//! centroid CSVs (Altium/JLC/generic). Coordinates are converted to millimetres
//! from the unit the file **declares**: the bracketed suffix on the Altium position
//! columns (`Center-X(mil)`, `(in)`, `(mm)`) or the KiCad banner (`## Unit = inches`,
//! `## Unit = mm`). A declared unit we don't recognise fails loud, as do X and Y
//! columns declaring different units — scaling a board by 39.37 must never happen
//! quietly. A file that declares no unit at all is assumed millimetres (the
//! overwhelming default). A file we can't parse into any placements fails loud
//! rather than silently contributing nothing.

use crate::error::{EngineError, Result};
use crate::geo::{Contour, PolygonSet};
use crate::geom;

/// Marker rectangle (mm). Asymmetric (wider than tall) so a **rotation** change
/// moves its corners and thus shows in the diff, not just a position change.
const MARKER_W_MM: f64 = 1.4;
const MARKER_H_MM: f64 = 0.6;
const NM_PER_MM: f64 = 1.0e6;

/// Header wording that names the X / Y position column (Altium, JLC, KiCad,
/// generic centroid CSVs). Matched case-insensitively by substring.
const X_COL_KEYS: &[&str] = &["posx", "pos x", "center-x", "centerx", "mid x", "ref-x"];
const Y_COL_KEYS: &[&str] = &["posy", "pos y", "center-y", "centery", "mid y", "ref-y"];

/// A length unit a pick-and-place file can declare for its coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Mm,
    Mil,
    Inch,
}

impl Unit {
    /// Recognise a declared unit token; anything else is the caller's error.
    fn parse(token: &str) -> Option<Unit> {
        match token.trim().to_ascii_lowercase().as_str() {
            "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" => Some(Unit::Mm),
            "mil" | "mils" => Some(Unit::Mil),
            "in" | "inch" | "inches" => Some(Unit::Inch),
            _ => None,
        }
    }

    /// Recognise a declared unit token or fail loud naming it.
    fn parse_loud(token: &str, where_: &str) -> Result<Unit> {
        Unit::parse(token).ok_or_else(|| {
            EngineError::Parse(format!(
                "unrecognised pick-and-place coordinate unit `{token}` in {where_} \
                 (expected mm, mil or in)"
            ))
        })
    }

    fn to_mm(self, v: f64) -> f64 {
        match self {
            Unit::Mm => v,
            Unit::Mil => v * 0.0254,
            Unit::Inch => v * 25.4,
        }
    }
}

/// One parsed component placement (millimetres, degrees).
#[derive(Debug, Clone, PartialEq)]
struct Placement {
    reference: String,
    x_mm: f64,
    y_mm: f64,
    rot_deg: f64,
}

/// Content sniff for a pick-and-place file, distinct from Gerber/Excellon. Looks
/// for the KiCad `.pos` banner or a CSV/columned header naming a reference and a
/// position column.
pub fn looks_like_placement(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    if lower.contains("%fs") || lower.contains("%mo") || text.lines().any(|l| l.trim() == "M48") {
        return false; // Gerber / Excellon
    }
    // KiCad .pos banners.
    if lower.contains("footprint positions") || lower.contains("## unit") {
        return true;
    }
    // A header naming a reference column AND a position column (CSV or columned).
    lower.lines().take(20).any(|l| {
        let has_ref = l.contains("designator") || l.contains("ref");
        let has_pos = l.contains("posx")
            || l.contains("pos x")
            || l.contains("center-x")
            || l.contains("centerx")
            || l.contains("mid x")
            || l.contains("ref-x");
        has_ref && has_pos
    })
}

/// Parse + render a P&P file into placement-marker geometry.
pub fn resolve_placement(bytes: &[u8]) -> Result<PolygonSet> {
    let text = String::from_utf8_lossy(bytes);
    let placements = parse_placements(&text)?;
    if placements.is_empty() {
        return Err(EngineError::Parse(
            "no component placements found in the pick-and-place file".into(),
        ));
    }
    let markers: Vec<Contour> = placements
        .iter()
        .map(|p| {
            geom::rect_rot(
                p.x_mm * NM_PER_MM,
                p.y_mm * NM_PER_MM,
                MARKER_W_MM * NM_PER_MM,
                MARKER_H_MM * NM_PER_MM,
                p.rot_deg,
            )
        })
        .collect();
    // Disjoint markers come back as separate shapes; coincident ones merge.
    Ok(crate::boolean::union(&markers, &[]))
}

/// Dispatch on shape: decide CSV vs whitespace-columned from the **header** line
/// (the one naming the reference + position columns) when there is one — data
/// rows can legally contain commas inside quoted descriptions, and banner text
/// above the header is arbitrary. Fall back to the first content line.
fn parse_placements(text: &str) -> Result<Vec<Placement>> {
    let header = text.lines().find(|l| {
        let low = l.to_ascii_lowercase();
        let has_ref = low.contains("designator") || low.contains("ref");
        has_ref && X_COL_KEYS.iter().any(|k| low.contains(k))
    });
    let unit = declared_unit(text, header)?.unwrap_or(Unit::Mm);
    let probe = header.or_else(|| {
        text.lines()
            .find(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
    });
    let mut placements = if probe.map(|l| l.contains(',')).unwrap_or(false) {
        parse_csv(text)?
    } else {
        parse_whitespace(text)?
    };
    if unit != Unit::Mm {
        for p in &mut placements {
            p.x_mm = unit.to_mm(p.x_mm);
            p.y_mm = unit.to_mm(p.y_mm);
        }
    }
    Ok(placements)
}

/// The coordinate unit the file declares, or `None` when it declares nothing (the
/// caller then assumes mm). Two places are read: the `(unit)` suffix on the X and
/// Y position columns of the header (Altium writes `Center-X(mil)` on an imperial
/// board) and the KiCad `## Unit = inches` banner. An unrecognised token, X and Y
/// disagreeing, or header and banner disagreeing all fail loud.
fn declared_unit(text: &str, header: Option<&str>) -> Result<Option<Unit>> {
    let mut from_header = None;
    if let Some(header) = header {
        let cols: Vec<String> = if header.contains(',') {
            header
                .split(',')
                .map(|c| c.trim().trim_matches('"').to_ascii_lowercase())
                .collect()
        } else {
            header
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect()
        };
        let col_unit = |keys: &[&str]| -> Result<Option<Unit>> {
            let col = cols.iter().find(|c| keys.iter().any(|k| c.contains(k)));
            match col.and_then(|c| bracket_suffix(c).map(|u| (c, u))) {
                Some((col, tok)) => Unit::parse_loud(tok, &format!("column `{col}`")).map(Some),
                None => Ok(None),
            }
        };
        let (x, y) = (col_unit(X_COL_KEYS)?, col_unit(Y_COL_KEYS)?);
        if let (Some(x), Some(y)) = (x, y) {
            if x != y {
                return Err(EngineError::Parse(format!(
                    "pick-and-place X and Y columns declare different units ({x:?} vs {y:?})"
                )));
            }
        }
        from_header = x.or(y);
    }

    // KiCad banner: `## Unit = mm, Angle = deg.`
    let mut from_banner = None;
    for line in text.lines() {
        let low = line.trim().to_ascii_lowercase();
        if !low.starts_with("##") || !low.contains("unit") {
            continue;
        }
        if let Some(rhs) = low.split_once('=').map(|(_, r)| r) {
            let tok = rhs.split(',').next().unwrap_or("").trim();
            from_banner = Some(Unit::parse_loud(tok, "the `## Unit` banner")?);
            break;
        }
    }

    match (from_header, from_banner) {
        (Some(h), Some(b)) if h != b => Err(EngineError::Parse(format!(
            "pick-and-place header declares {h:?} but the `## Unit` banner declares {b:?}"
        ))),
        (h, b) => Ok(h.or(b)),
    }
}

/// The text inside a trailing `(…)` of a column name: `center-x(mil)` → `mil`.
fn bracket_suffix(col: &str) -> Option<&str> {
    let col = col.trim_end();
    let inner = col.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    Some(inner[open + 1..].trim())
}

/// Whitespace-columned rows: KiCad `Ref Val Package PosX PosY Rot [Side]`, and
/// Altium `Designator Comment Layer Footprint X Y Rot "Description"`. Parsed from
/// the right so multi-token middle columns don't shift the numerics. A trailing
/// **quoted** description (Altium; may be multi-word, may contain commas) is
/// stripped first — leaving it in play silently dropped every row whose
/// description had spaces, a partial parse the trust bar forbids.
fn parse_whitespace(text: &str) -> Result<Vec<Placement>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip a trailing `"…"` run (the Altium Description column).
        if line.ends_with('"') {
            if let Some(open) = line[..line.len() - 1].rfind('"') {
                line = line[..open].trim_end();
            }
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        // If the last field is non-numeric it's the Side column; the three numbers
        // before it are PosX PosY Rot. Otherwise the last three are PosX PosY Rot.
        let last_is_num = f[f.len() - 1].parse::<f64>().is_ok();
        let base = if last_is_num { f.len() } else { f.len() - 1 };
        if base < 4 {
            continue;
        }
        let (x, y, rot) = (f[base - 3], f[base - 2], f[base - 1]);
        match (x.parse::<f64>(), y.parse::<f64>(), rot.parse::<f64>()) {
            (Ok(x_mm), Ok(y_mm), Ok(rot_deg)) => out.push(Placement {
                reference: f[0].to_string(),
                x_mm,
                y_mm,
                rot_deg,
            }),
            _ => continue, // a header/legend row — skip, don't fail the file
        }
    }
    Ok(out)
}

/// Comma-separated centroid CSV. Locate the reference / X / Y / rotation columns
/// from the header by name (tolerant of Altium/JLC/generic wording), then read rows.
fn parse_csv(text: &str) -> Result<Vec<Placement>> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let header = lines
        .next()
        .ok_or_else(|| EngineError::Parse("empty pick-and-place CSV".into()))?;
    let cols: Vec<String> = header
        .split(',')
        .map(|c| c.trim().trim_matches('"').to_ascii_lowercase())
        .collect();
    let find = |preds: &[&str]| -> Option<usize> {
        cols.iter()
            .position(|c| preds.iter().any(|p| c.contains(p)))
    };
    let ref_i = find(&["designator", "ref"])
        .ok_or_else(|| EngineError::Parse("no reference/designator column in CSV".into()))?;
    let x_i =
        find(X_COL_KEYS).ok_or_else(|| EngineError::Parse("no X-position column in CSV".into()))?;
    let y_i =
        find(Y_COL_KEYS).ok_or_else(|| EngineError::Parse("no Y-position column in CSV".into()))?;
    let rot_i = find(&["rotation", "rot"]);

    let mut out = Vec::new();
    for line in lines {
        let f: Vec<&str> = line
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .collect();
        let get = |i: usize| f.get(i).copied().unwrap_or("");
        let (x, y) = (strip_unit(get(x_i)), strip_unit(get(y_i)));
        match (x.parse::<f64>(), y.parse::<f64>()) {
            (Ok(x_mm), Ok(y_mm)) => out.push(Placement {
                reference: get(ref_i).to_string(),
                x_mm,
                y_mm,
                rot_deg: rot_i
                    .and_then(|i| strip_unit(get(i)).parse::<f64>().ok())
                    .unwrap_or(0.0),
            }),
            _ => continue,
        }
    }
    Ok(out)
}

/// Trim a trailing unit like `mm` (some exporters write `10.0mm`).
fn strip_unit(s: &str) -> &str {
    s.trim().trim_end_matches("mm").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KICAD_POS: &str = "### Footprint positions - created on 2026\n\
## Unit = mm, Angle = deg.\n\
# Ref     Val    Package   PosX      PosY      Rot     Side\n\
C1        100nF  C_0402    10.0000   20.0000   90.0000 top\n\
R1        1k     R_0402    15.0000   20.0000    0.0000 top\n\
## End\n";

    const CSV_POS: &str = "Designator,Comment,Layer,Center-X(mm),Center-Y(mm),Rotation\n\
C1,100nF,top,10.0,20.0,90\n\
R1,1k,top,15.0,20.0,0\n";

    #[test]
    fn parses_kicad_whitespace_pos() {
        let p = parse_placements(KICAD_POS).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].reference, "C1");
        assert!((p[0].x_mm - 10.0).abs() < 1e-9 && (p[0].y_mm - 20.0).abs() < 1e-9);
        assert!((p[0].rot_deg - 90.0).abs() < 1e-9);
    }

    #[test]
    fn parses_centroid_csv() {
        let p = parse_placements(CSV_POS).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].reference, "R1");
        assert!((p[1].x_mm - 15.0).abs() < 1e-9);
    }

    // Mimics a real Altium "Pick Place for X.txt": banner text, a columned header,
    // and rows ending in a QUOTED multi-word Description (CRLF endings). Rows with
    // multi-word descriptions were silently skipped — a partial parse (~17 of ~140
    // components on a production board) with no error.
    const ALTIUM_POS: &str = "Altium Designer Pick and Place Locations\r\n\
D:\\git\\proj\\Pick Place for X.txt\r\n\
\r\n\
Units used: mm\r\n\
\r\n\
Designator   Comment        Layer       Footprint   Center-X(mm) Center-Y(mm) Rotation Description\r\n\
R1           1K             TopLayer    0402-RES    20.6248      44.8056      0        \"RES 1K OHM 1/16W 1% 0402\"\r\n\
U405         ICM20649       TopLayer    ICM20649    56.2356      36.0934      0        \"\"\r\n\
SW1          FPF1320        BottomLayer FPF1320     29.9516      38.9507      270      \"LOAD SWITCH, 1.5A\"\r\n";

    #[test]
    fn altium_columned_pos_parses_every_row() {
        let p = parse_placements(ALTIUM_POS).unwrap();
        assert_eq!(p.len(), 3, "all rows incl. multi-word quoted descriptions");
        assert_eq!(p[0].reference, "R1");
        assert!((p[0].x_mm - 20.6248).abs() < 1e-6);
        assert!((p[0].y_mm - 44.8056).abs() < 1e-6);
        assert!((p[2].rot_deg - 270.0).abs() < 1e-9);
        assert!(looks_like_placement(ALTIUM_POS.as_bytes()));
    }

    #[test]
    fn resolves_markers_one_shape_per_component() {
        let ps = resolve_placement(KICAD_POS.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 2, "one marker per component");
        assert!(ps.area_mm2() > 0.0);
    }

    #[test]
    fn sniff_distinguishes_from_gerber_and_excellon() {
        assert!(looks_like_placement(KICAD_POS.as_bytes()));
        assert!(looks_like_placement(CSV_POS.as_bytes()));
        assert!(!looks_like_placement(b"%FSLAX46Y46*%\n%MOMM*%\n"));
        assert!(!looks_like_placement(b"M48\nMETRIC,TZ\nT1C0.5\n"));
    }

    #[test]
    fn empty_or_headerless_fails_loud() {
        assert!(resolve_placement(b"# just a comment\n").is_err());
    }

    /// An Altium columned export of one part, with the unit in the column
    /// brackets the way Altium writes it (the banner alone is not what we key on).
    fn altium(unit: &str, x: &str, y: &str) -> String {
        format!(
            "Altium Designer Pick and Place Locations\r\n\
             \r\n\
             Units used: {unit}\r\n\
             \r\n\
             Designator Comment Layer    Footprint Center-X({unit}) Center-Y({unit}) Rotation Description\r\n\
             R1         1K      TopLayer 0402-RES  {x} {y} 90 \"RES 1K OHM\"\r\n"
        )
    }

    /// An Altium CSV export of the same part.
    fn altium_csv(unit: &str, x: &str, y: &str) -> String {
        format!(
            "Designator,Comment,Layer,Center-X({unit}),Center-Y({unit}),Rotation\n\
             R1,1K,top,{x},{y},90\n"
        )
    }

    /// A KiCad `.pos` of one part with the given banner unit.
    fn kicad(unit: &str, x: &str, y: &str) -> String {
        format!(
            "### Footprint positions - created on 2026\n\
             ## Unit = {unit}, Angle = deg.\n\
             # Ref     Val    Package   PosX      PosY      Rot     Side\n\
             R1        1k     R_0402    {x}   {y}    90.0000 top\n\
             ## End\n"
        )
    }

    fn nm(text: &str) -> Vec<(i64, i64)> {
        parse_placements(text)
            .unwrap()
            .iter()
            .map(|p| {
                (
                    (p.x_mm * NM_PER_MM).round() as i64,
                    (p.y_mm * NM_PER_MM).round() as i64,
                )
            })
            .collect()
    }

    // #311: an imperial Altium export was read as mm, placing every marker
    // 39.37x too far out. 1000 mil = 1 in = 25.4 mm exactly.
    #[test]
    fn altium_mil_and_inch_land_on_the_same_nm_as_mm() {
        let mm = nm(&altium("mm", "25.4", "50.8"));
        assert_eq!(mm, vec![(25_400_000, 50_800_000)]);
        assert_eq!(nm(&altium("mil", "1000", "2000")), mm);
        assert_eq!(nm(&altium("in", "1", "2")), mm);
        assert_eq!(nm(&altium_csv("mil", "1000", "2000")), mm);
        // End to end: identical marker geometry, so a mm rev vs a mil rev diffs clean.
        let a = resolve_placement(altium("mm", "25.4", "50.8").as_bytes()).unwrap();
        let b = resolve_placement(altium("mil", "1000", "2000").as_bytes()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn kicad_inches_banner_lands_on_the_same_nm_as_mm() {
        let mm = nm(&kicad("mm", "25.4000", "50.8000"));
        assert_eq!(mm, vec![(25_400_000, 50_800_000)]);
        assert_eq!(nm(&kicad("inches", "1.0000", "2.0000")), mm);
    }

    #[test]
    fn unknown_declared_unit_fails_loud() {
        let err = parse_placements(&altium("furlong", "1", "2")).unwrap_err();
        assert!(
            matches!(err, EngineError::Parse(ref m) if m.contains("furlong")),
            "{err}"
        );
        let err = parse_placements(&kicad("furlongs", "1", "2")).unwrap_err();
        assert!(
            matches!(err, EngineError::Parse(ref m) if m.contains("furlongs")),
            "{err}"
        );
    }

    #[test]
    fn altium_x_and_y_in_different_units_fails_loud() {
        let mixed = altium("mil", "1000", "2000").replace("Center-Y(mil)", "Center-Y(mm)");
        let err = parse_placements(&mixed).unwrap_err();
        assert!(
            matches!(err, EngineError::Parse(ref m) if m.contains("different units")),
            "{err}"
        );
    }

    #[test]
    fn undeclared_unit_is_assumed_mm() {
        let text = "Designator,Comment,Layer,Center-X,Center-Y,Rotation\nR1,1K,top,25.4,50.8,90\n";
        assert_eq!(nm(text), vec![(25_400_000, 50_800_000)]);
    }
}
