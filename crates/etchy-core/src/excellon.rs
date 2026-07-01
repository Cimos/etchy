//! Excellon / NC drill front-end: bytes → filled hole + slot geometry
//! ([`PolygonSet`]).
//!
//! Like the Gerber front-end this takes **bytes** (no path/I-O policy) and emits
//! integer-nm filled geometry: each drill hit becomes a circle of the tool's
//! diameter, each slot (G85 canned, or a routed G01 run between `M15`/`M16`)
//! becomes a stadium of the tool's width. The result is the union of them all.
//!
//! **Trust bar.** Excellon's Achilles heel is coordinate decoding: zero
//! suppression (`LZ`/`TZ`) and the integer/decimal split are declared in the
//! header (or not at all), and getting them wrong silently moves holes. So we
//! fail loud on an ambiguous header (no units, or suppressed coordinates with no
//! declared mode) rather than guess — a wrong hole position is exactly the silent
//! miss etchy exists to prevent. Anything we can't render faithfully (incremental
//! coordinates, arc routing) is a loud [`EngineError`], never a quiet drop.

use crate::error::{EngineError, Result};
use crate::geo::{Contour, PolygonSet};
use crate::geom;

/// Which zeros a coordinate field keeps (the other side is suppressed). Named for
/// what's *present* in the file, matching the header keywords `LZ`/`TZ`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zeros {
    /// `LZ`: leading zeros kept, trailing suppressed → right-pad to full width.
    Leading,
    /// `TZ`: trailing zeros kept, leading suppressed → left-pad to full width.
    Trailing,
}

/// Coordinate units declared by `INCH` / `METRIC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Units {
    Inch,
    Metric,
}

impl Units {
    fn to_nm(self) -> f64 {
        match self {
            // 1 inch = 25.4 mm = 25.4e6 nm; 1 mm = 1e6 nm.
            Units::Inch => 25.4e6,
            Units::Metric => 1.0e6,
        }
    }
    /// Conventional integer.decimal split when a file doesn't state one.
    fn default_format(self) -> (usize, usize) {
        match self {
            Units::Inch => (2, 4),
            Units::Metric => (3, 3),
        }
    }
}

/// The coordinate-decoding context resolved from the header. (Units are applied
/// separately via the nm scale, so they aren't stored here.)
#[derive(Clone, Copy, Debug)]
struct Format {
    zeros: Option<Zeros>,
    int_digits: usize,
    dec_digits: usize,
}

impl Format {
    /// Decode one coordinate field (the text after `X`/`Y`, e.g. `-0150` or `1.5`)
    /// into the file's units. Explicit decimal points win; otherwise apply the
    /// declared zero-suppression against the integer/decimal width.
    fn decode(&self, field: &str) -> Result<f64> {
        let field = field.trim();
        if field.is_empty() {
            return Err(EngineError::Parse("empty drill coordinate".into()));
        }
        let (neg, mag) = match field.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, field.strip_prefix('+').unwrap_or(field)),
        };
        // Explicit decimal point: unambiguous, suppression doesn't apply.
        let val = if mag.contains('.') {
            mag.parse::<f64>()
                .map_err(|_| EngineError::Parse(format!("bad drill coordinate '{field}'")))?
        } else {
            if !mag.chars().all(|c| c.is_ascii_digit()) {
                return Err(EngineError::Parse(format!(
                    "bad drill coordinate '{field}'"
                )));
            }
            let total = self.int_digits + self.dec_digits;
            let zeros = self.zeros.ok_or_else(|| {
                EngineError::Parse(
                    "drill coordinates have no decimal point and the header declares no zero \
                     suppression (LZ/TZ) — refusing to guess hole positions"
                        .into(),
                )
            })?;
            if mag.len() > total {
                return Err(EngineError::Parse(format!(
                    "drill coordinate '{field}' has more digits than the {}.{} format",
                    self.int_digits, self.dec_digits
                )));
            }
            let padded = match zeros {
                // Leading kept → the missing digits are trailing → pad right.
                Zeros::Leading => format!("{mag:0<width$}", width = total),
                // Trailing kept → the missing digits are leading → pad left.
                Zeros::Trailing => format!("{mag:0>width$}", width = total),
            };
            let intval: i64 = padded
                .parse()
                .map_err(|_| EngineError::Parse(format!("bad drill coordinate '{field}'")))?;
            intval as f64 / 10f64.powi(self.dec_digits as i32)
        };
        Ok(if neg { -val } else { val })
    }
}

/// Content sniff: an Excellon drill file, as opposed to Gerber. Gerber carries
/// `%FS`/`%MO` blocks; Excellon opens with `M48` (or, tolerant of headerless
/// files, carries `INCH`/`METRIC` plus `T…C…` tool definitions).
pub fn looks_like_excellon(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    // A `%FS`/`%MO` line means Gerber — never claim it.
    let mut has_m48 = false;
    let mut has_unit = false;
    let mut has_tool = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("%FS") || t.starts_with("%MO") {
            return false;
        }
        if t == "M48" {
            has_m48 = true;
        }
        if t.starts_with("INCH") || t.starts_with("METRIC") {
            has_unit = true;
        }
        if is_tool_def(t).is_some() {
            has_tool = true;
        }
    }
    has_m48 || (has_unit && has_tool)
}

/// Parse `T<n>C<dia>` (a tool definition) → (tool number, diameter text). Returns
/// `None` for a bare tool select (`T<n>`) or any non-tool line.
fn is_tool_def(line: &str) -> Option<(u32, &str)> {
    let rest = line.strip_prefix('T')?;
    let cpos = rest.find('C')?;
    let num: u32 = rest[..cpos].parse().ok()?;
    let dia = &rest[cpos + 1..];
    // Diameter may carry trailing feed/speed fields on some dialects; take the
    // leading numeric run.
    let dia = dia.trim();
    Some((num, dia))
}

/// A bare tool select `T<n>` (no `C`), returning the tool number.
fn tool_select(line: &str) -> Option<u32> {
    let rest = line.strip_prefix('T')?;
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// Parse + resolve one Excellon drill file's bytes into filled hole/slot geometry.
pub fn resolve_excellon(bytes: &[u8]) -> Result<PolygonSet> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().map(|l| l.trim()).peekable();

    // ---- Header (M48 … M95/% ) ----
    let mut units: Option<Units> = None;
    let mut zeros: Option<Zeros> = None;
    let mut fmt_digits: Option<(usize, usize)> = None;
    let mut tools: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();

    // Consume until the header end marker (`%` or `M95`), collecting units,
    // suppression and tool diameters. Files without an M48 header fall through
    // with these still unset; we resolve defaults below.
    let mut in_header = false;
    for line in lines.by_ref() {
        if line == "M48" {
            in_header = true;
            continue;
        }
        if line == "%" || line == "M95" {
            break;
        }
        parse_header_line(line, &mut units, &mut zeros, &mut fmt_digits, &mut tools)?;
        if !in_header && (line == "M30" || line == "M00") {
            break;
        }
    }

    let units = units.ok_or_else(|| {
        EngineError::Parse("Excellon file declares no units (INCH/METRIC)".into())
    })?;
    let (int_digits, dec_digits) = fmt_digits.unwrap_or_else(|| units.default_format());
    let fmt = Format {
        zeros,
        int_digits,
        dec_digits,
    };
    let unit_nm = units.to_nm();

    // ---- Body ----
    let mut m = Body::new(fmt, unit_nm, tools);
    for line in lines {
        m.step(line)?;
    }
    Ok(m.finish())
}

/// Parse one header line, updating the accumulated state. Unknown but harmless
/// header directives (FMAT, VER, comments, blank) are ignored; unsupported ones
/// (incremental coordinates) fail loud.
fn parse_header_line(
    line: &str,
    units: &mut Option<Units>,
    zeros: &mut Option<Zeros>,
    fmt_digits: &mut Option<(usize, usize)>,
    tools: &mut std::collections::HashMap<u32, f64>,
) -> Result<()> {
    if line.is_empty() || line.starts_with(';') {
        // KiCad-style `;FILE_FORMAT=3:3` comment can still carry the digit split.
        if let Some(fmt) = line.strip_prefix(';').and_then(parse_format_comment) {
            *fmt_digits = Some(fmt);
        }
        return Ok(());
    }
    if line == "ICI,ON" || line == "ICI" {
        return Err(EngineError::Unsupported {
            feature: "Excellon incremental coordinates (ICI)".into(),
        });
    }
    if let Some(rest) = line.strip_prefix("INCH") {
        *units = Some(Units::Inch);
        apply_zero_mode(rest, zeros);
        return Ok(());
    }
    if let Some(rest) = line.strip_prefix("METRIC") {
        *units = Some(Units::Metric);
        apply_zero_mode(rest, zeros);
        return Ok(());
    }
    if let Some((num, dia)) = is_tool_def(line) {
        let d: f64 = dia
            .parse()
            .map_err(|_| EngineError::Parse(format!("bad tool diameter in '{line}'")))?;
        tools.insert(num, d);
        return Ok(());
    }
    // FMAT,2 / VER,1 / M-codes etc. — ignored.
    Ok(())
}

/// `,LZ` / `,TZ` suffix on an `INCH`/`METRIC` line.
fn apply_zero_mode(rest: &str, zeros: &mut Option<Zeros>) {
    if rest.contains("LZ") {
        *zeros = Some(Zeros::Leading);
    } else if rest.contains("TZ") {
        *zeros = Some(Zeros::Trailing);
    }
}

/// A `;FILE_FORMAT=3:3` / `;FORMAT={3:3}` style comment → (int, dec) digits.
fn parse_format_comment(comment: &str) -> Option<(usize, usize)> {
    let c = comment.to_ascii_uppercase();
    let idx = c.find("FORMAT")?;
    let tail = &c[idx..];
    let colon = tail.find(':')?;
    let before: String = tail[..colon]
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let after: String = tail[colon + 1..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let i: usize = before.chars().rev().collect::<String>().parse().ok()?;
    let d: usize = after.parse().ok()?;
    Some((i, d))
}

/// Drill-body interpreter: walks tool selects, drill hits, canned (`G85`) slots,
/// and routed (`M15`…`M16`) slot runs into filled contours.
struct Body {
    fmt: Format,
    unit_nm: f64,
    tools: std::collections::HashMap<u32, f64>,
    dia_nm: f64, // current tool diameter (nm)
    x: f64,      // current position (nm), modal
    y: f64,
    routing: bool, // between M15 (down) and M16/M17 (up)
    contours: Vec<Contour>,
}

impl Body {
    fn new(fmt: Format, unit_nm: f64, tools: std::collections::HashMap<u32, f64>) -> Self {
        Self {
            fmt,
            unit_nm,
            tools,
            dia_nm: 0.0,
            x: 0.0,
            y: 0.0,
            routing: false,
            contours: Vec::new(),
        }
    }

    fn step(&mut self, line: &str) -> Result<()> {
        if line.is_empty() || line.starts_with(';') || line == "%" {
            return Ok(());
        }
        match line {
            "M15" => {
                self.routing = true;
                return Ok(());
            }
            "M16" | "M17" => {
                self.routing = false;
                return Ok(());
            }
            "M30" | "M00" | "M95" | "G05" | "G90" | "T0" => return Ok(()),
            _ => {}
        }
        // Rout arcs aren't rendered faithfully yet — fail loud rather than drop.
        if line.starts_with("G02") || line.starts_with("G03") {
            return Err(EngineError::Unsupported {
                feature: "Excellon arc routing (G02/G03)".into(),
            });
        }
        // Bare tool select.
        if let Some(t) = tool_select(line) {
            self.select_tool(t)?;
            return Ok(());
        }
        // A coordinate-bearing line: drill hit, G85 slot, or routed move.
        if line.contains('X') || line.contains('Y') {
            return self.coordinate_line(line);
        }
        // G00/G01 mode words with no coordinates, feed/speed, etc. — ignore.
        Ok(())
    }

    fn select_tool(&mut self, t: u32) -> Result<()> {
        if t == 0 {
            return Ok(()); // T0 = no tool / end
        }
        let d = *self.tools.get(&t).ok_or_else(|| {
            EngineError::Parse(format!(
                "drill body selects tool T{t} not defined in the header"
            ))
        })?;
        self.dia_nm = d * self.unit_nm;
        Ok(())
    }

    fn coordinate_line(&mut self, line: &str) -> Result<()> {
        // G85 canned slot: "X..Y..G85X..Y.." — from the first coord to the second.
        if let Some(gpos) = line.find("G85") {
            let (from, to) = line.split_at(gpos);
            let (x1, y1) = self.read_xy(from, self.x, self.y)?;
            let (x2, y2) = self.read_xy(&to[3..], x1, y1)?;
            self.emit_slot(x1, y1, x2, y2)?;
            self.x = x2;
            self.y = y2;
            return Ok(());
        }
        // Strip a leading G00/G01 so read_xy sees only the coordinate.
        let coord = line
            .strip_prefix("G00")
            .or_else(|| line.strip_prefix("G01"))
            .unwrap_or(line);
        let (nx, ny) = self.read_xy(coord, self.x, self.y)?;
        if self.routing {
            // Routed move with the tool down = a slot segment from here to there.
            self.emit_slot(self.x, self.y, nx, ny)?;
        } else {
            // Drill mode: a hit at the new point.
            self.emit_hit(nx, ny)?;
        }
        self.x = nx;
        self.y = ny;
        Ok(())
    }

    /// Read the X/Y tokens from a coordinate fragment, in nm, modal (a missing
    /// axis keeps its previous value).
    fn read_xy(&self, frag: &str, dx: f64, dy: f64) -> Result<(f64, f64)> {
        let x = match extract_axis(frag, 'X') {
            Some(f) => self.fmt.decode(f)? * self.unit_nm,
            None => dx,
        };
        let y = match extract_axis(frag, 'Y') {
            Some(f) => self.fmt.decode(f)? * self.unit_nm,
            None => dy,
        };
        Ok((x, y))
    }

    fn emit_hit(&mut self, x: f64, y: f64) -> Result<()> {
        if self.dia_nm <= 0.0 {
            return Err(EngineError::Parse(
                "drill hit before a tool with a positive diameter was selected".into(),
            ));
        }
        self.contours.push(geom::ngon(x, y, self.dia_nm / 2.0));
        Ok(())
    }

    fn emit_slot(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<()> {
        if self.dia_nm <= 0.0 {
            return Err(EngineError::Parse(
                "drill slot before a tool with a positive diameter was selected".into(),
            ));
        }
        self.contours
            .push(geom::stadium(x1, y1, x2, y2, self.dia_nm / 2.0));
        Ok(())
    }

    /// Union every hole/slot into one filled set in a single boolean pass —
    /// disjoint holes come back as separate shapes, overlapping ones merge (like
    /// the Gerber path, but all objects are "dark" so no per-object accumulation).
    fn finish(self) -> PolygonSet {
        if self.contours.is_empty() {
            return PolygonSet::default();
        }
        crate::boolean::union(&self.contours, &[])
    }
}

/// Extract the numeric field following `axis` (`X`/`Y`) up to the next axis
/// letter or a `G`/`M` word.
fn extract_axis(frag: &str, axis: char) -> Option<&str> {
    let start = frag.find(axis)? + 1;
    let rest = &frag[start..];
    let end = rest.find(['X', 'Y', 'G', 'M']).unwrap_or(rest.len());
    let field = &rest[..end];
    if field.is_empty() {
        None
    } else {
        Some(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(_units: Units, zeros: Option<Zeros>, i: usize, d: usize) -> Format {
        Format {
            zeros,
            int_digits: i,
            dec_digits: d,
        }
    }

    #[test]
    fn decode_leading_zero_mode_pads_right() {
        // INCH,LZ, format 2.4: "15" → 15.0000, "0500" → 05.0000.
        let f = fmt(Units::Inch, Some(Zeros::Leading), 2, 4);
        assert!((f.decode("15").unwrap() - 15.0).abs() < 1e-9);
        assert!((f.decode("0500").unwrap() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn decode_trailing_zero_mode_pads_left() {
        // INCH,TZ, format 2.4: "15" → 0.0015, "1500" → 0.1500.
        let f = fmt(Units::Inch, Some(Zeros::Trailing), 2, 4);
        assert!((f.decode("15").unwrap() - 0.0015).abs() < 1e-9);
        assert!((f.decode("1500").unwrap() - 0.15).abs() < 1e-9);
    }

    #[test]
    fn decode_explicit_decimal_ignores_suppression() {
        let f = fmt(Units::Metric, None, 3, 3);
        assert!((f.decode("1.5").unwrap() - 1.5).abs() < 1e-9);
        assert!((f.decode("-2.25").unwrap() + 2.25).abs() < 1e-9);
    }

    #[test]
    fn decode_without_suppression_or_decimal_fails_loud() {
        let f = fmt(Units::Metric, None, 3, 3);
        assert!(
            f.decode("150").is_err(),
            "must refuse to guess hole positions"
        );
    }

    #[test]
    fn sniff_distinguishes_excellon_from_gerber() {
        assert!(looks_like_excellon(b"M48\nMETRIC,TZ\nT1C0.5\n%\n"));
        assert!(!looks_like_excellon(b"%FSLAX46Y46*%\n%MOMM*%\n"));
    }

    #[test]
    fn resolves_metric_drill_hits() {
        // Two 0.5 mm holes, explicit decimals.
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX10.0Y10.0\nX20.0Y10.0\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        // Two disjoint holes → two shapes, positive area each.
        assert_eq!(ps.shapes.len(), 2, "two disjoint holes");
        assert!(ps.area_mm2() > 0.0);
        // ~2 circles of 0.5 mm dia: area ≈ 2 * π * 0.25² ≈ 0.393 mm².
        assert!(
            (ps.area_mm2() - 0.393).abs() < 0.02,
            "got {}",
            ps.area_mm2()
        );
    }

    #[test]
    fn resolves_suppressed_coordinates() {
        // METRIC,LZ format 3.3: "010000" → 10.000 mm (leading kept, pad right).
        let src = "M48\nMETRIC,LZ\nT1C1.000\n%\nT1\nX010000Y010000\nX020000Y010000\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 2);
        let bb = ps.bbox_nm().unwrap();
        // Holes at x=10mm and x=20mm, 1mm dia → bbox x spans ~9.5..20.5 mm.
        assert!((bb[0] as f64 / 1e6 - 9.5).abs() < 0.1, "min x {}", bb[0]);
        assert!((bb[2] as f64 / 1e6 - 20.5).abs() < 0.1, "max x {}", bb[2]);
    }

    #[test]
    fn resolves_g85_slot() {
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX10.0Y10.0G85X15.0Y10.0\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert!(ps.area_mm2() > 0.0);
        let bb = ps.bbox_nm().unwrap();
        // Slot from x=10 to x=15, 0.5mm wide → spans ~9.75..15.25 mm.
        assert!((bb[0] as f64 / 1e6 - 9.75).abs() < 0.1, "min x {}", bb[0]);
        assert!((bb[2] as f64 / 1e6 - 15.25).abs() < 0.1, "max x {}", bb[2]);
    }

    #[test]
    fn resolves_routed_slot() {
        // Route mode: position (G00), tool down (M15), linear route (G01), up (M16).
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nG00X10.0Y10.0\nM15\nG01X15.0Y10.0\nM16\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        let bb = ps.bbox_nm().unwrap();
        assert!((bb[0] as f64 / 1e6 - 9.75).abs() < 0.1, "min x {}", bb[0]);
        assert!((bb[2] as f64 / 1e6 - 15.25).abs() < 0.1, "max x {}", bb[2]);
    }

    #[test]
    fn missing_units_fails_loud() {
        let src = "M48\nT1C0.5\n%\nT1\nX1.0Y1.0\nM30\n";
        assert!(resolve_excellon(src.as_bytes()).is_err());
    }

    #[test]
    fn arc_routing_fails_loud() {
        let src =
            "M48\nMETRIC,TZ\nT1C0.5\n%\nT1\nG00X10.0Y10.0\nM15\nG03X15.0Y10.0I2.5J0\nM16\nM30\n";
        assert!(resolve_excellon(src.as_bytes()).is_err());
    }
}
