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
//! centroid CSVs (Altium/JLC/generic). Coordinates are assumed millimetres (the
//! overwhelming default; KiCad states `## Unit = mm`). A file we can't parse into
//! any placements fails loud rather than silently contributing nothing.

use crate::error::{EngineError, Result};
use crate::geo::{Contour, PolygonSet};
use crate::geom;

/// Marker rectangle (mm). Asymmetric (wider than tall) so a **rotation** change
/// moves its corners and thus shows in the diff, not just a position change.
const MARKER_W_MM: f64 = 1.4;
const MARKER_H_MM: f64 = 0.6;
const NM_PER_MM: f64 = 1.0e6;

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

/// Dispatch on shape: a comma-separated file with a recognisable header is CSV;
/// otherwise treat it as whitespace-columned (KiCad `.pos`).
fn parse_placements(text: &str) -> Result<Vec<Placement>> {
    let looks_csv = text
        .lines()
        .find(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .map(|l| l.contains(','))
        .unwrap_or(false);
    if looks_csv {
        parse_csv(text)
    } else {
        parse_whitespace(text)
    }
}

/// KiCad-style whitespace `.pos`: `# ` comments, then `Ref Val Package PosX PosY
/// Rot [Side]`. Parsed from the right so a multi-token Val/Package doesn't shift
/// the numeric columns; the optional trailing textual Side is detected.
fn parse_whitespace(text: &str) -> Result<Vec<Placement>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
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
    let x_i = find(&["posx", "center-x", "centerx", "mid x", "ref-x", "pos x"])
        .ok_or_else(|| EngineError::Parse("no X-position column in CSV".into()))?;
    let y_i = find(&["posy", "center-y", "centery", "mid y", "ref-y", "pos y"])
        .ok_or_else(|| EngineError::Parse("no Y-position column in CSV".into()))?;
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
}
