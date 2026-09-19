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
use crate::geo::{snap_nm, Contour, PolygonSet};
use crate::geom;
use crate::gerber::{MAX_CONTOURS_PER_LAYER, MAX_POINTS_PER_LAYER};

/// Most digits accepted on either side of a declared coordinate format
/// (`;FILE_FORMAT=i:d`, `METRIC,LZ,000.000`). Suppressed coordinates are zero-padded
/// to `int + dec` digits and parsed as an `i64`: 9 + 9 = 18 digits always fits
/// (i64::MAX has 19), and `10^dec` with `dec <= 9` is exact in f64, so the decode
/// stays exact. Real exporters use 2..6 per side; a larger declaration is either a
/// typo or a crafted header (`3:99999999999` asked `format!` for ~100 GB of padding
/// per coordinate and aborted the process, #308). Fail loud instead.
const MAX_FORMAT_DIGITS: usize = 9;

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

/// Parse a tool definition → (tool number, diameter text). Returns `None` for a
/// bare tool select (`T<n>`) or any non-tool line. Feed/speed fields sit on
/// either side of the diameter across dialects — `T1C0.020F200S65` (trailing)
/// AND Altium's `T1F00S00C0.00787` (leading) — so the tool number is the leading
/// digit run after `T`, and the diameter is the numeric run after the `C`
/// wherever it appears.
fn is_tool_def(line: &str) -> Option<(u32, &str)> {
    let rest = line.strip_prefix('T')?;
    let nend = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if nend == 0 {
        return None;
    }
    let num: u32 = rest[..nend].parse().ok()?;
    let cpos = rest.find('C')?;
    let dia = rest[cpos + 1..].trim();
    let end = dia
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(dia.len());
    let dia = &dia[..end];
    if dia.is_empty() {
        return None;
    }
    Some((num, dia))
}

/// A tool select `T<n>` in the body, returning the tool number. Tolerates
/// trailing feed/speed/retract fields (`T01F200S65`) — but a `C` tail is a tool
/// *definition*, not a select.
fn tool_select(line: &str) -> Option<u32> {
    let rest = line.strip_prefix('T')?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let tail = &rest[end..];
    if tail.is_empty() || tail.starts_with(['F', 'S', 'B', 'H', 'Z']) {
        rest[..end].parse().ok()
    } else {
        None
    }
}

/// Parse + resolve one Excellon drill file's bytes into filled hole/slot geometry.
///
/// Two passes so a **headerless** file (no `M48`, a form the sniff deliberately
/// accepts) still resolves its body: pass 1 collects header facts (units, zero
/// suppression, digit format, tool diameters) from their unambiguous line forms
/// wherever they appear; pass 2 interprets the body — every line after the `M48`
/// header's `%`/`M95` terminator, or the whole file when there is no header
/// (header-form lines don't match any body form and are ignored there).
pub fn resolve_excellon(bytes: &[u8]) -> Result<PolygonSet> {
    // Contain a panic anywhere in the header/body walk as a loud typed error
    // instead of unwinding through the caller / aborting a batch — the same
    // boundary `resolve_layer` puts around the Gerber parser (#85, #308). Relies
    // on panic=unwind (kept that way in the release profile for this reason).
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resolve_excellon_inner(bytes)
    }))
    .map_err(|_| EngineError::Parse("excellon parser panicked on this input".into()))?
}

fn resolve_excellon_inner(bytes: &[u8]) -> Result<PolygonSet> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();

    // ---- Pass 1: header facts, from all lines ----
    let mut units: Option<Units> = None;
    let mut zeros: Option<Zeros> = None;
    let mut fmt_digits: Option<(usize, usize)> = None;
    let mut tools: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    for line in &lines {
        parse_header_line(line, &mut units, &mut zeros, &mut fmt_digits, &mut tools)?;
    }

    let units = units.ok_or_else(|| {
        EngineError::Parse("Excellon file declares no units (INCH/METRIC)".into())
    })?;
    let (int_digits, dec_digits) = fmt_digits.unwrap_or_else(|| units.default_format());
    if int_digits > MAX_FORMAT_DIGITS || dec_digits > MAX_FORMAT_DIGITS {
        return Err(EngineError::Parse(format!(
            "drill coordinate format {int_digits}:{dec_digits} declares more than \
             {MAX_FORMAT_DIGITS} digits on one side — refusing an absurd format"
        )));
    }
    let fmt = Format {
        zeros,
        int_digits,
        dec_digits,
    };
    let unit_nm = units.to_nm();

    // ---- Pass 2: body ----
    let body_start = if lines.contains(&"M48") {
        lines
            .iter()
            .position(|l| *l == "%" || *l == "M95")
            .map(|i| i + 1)
            .ok_or_else(|| {
                EngineError::Parse("M48 header never terminated (missing % or M95)".into())
            })?
    } else {
        0
    };
    let mut m = Body::new(fmt, unit_nm, tools);
    for line in &lines[body_start..] {
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
        apply_inline_format(rest, fmt_digits);
        return Ok(());
    }
    if let Some(rest) = line.strip_prefix("METRIC") {
        *units = Some(Units::Metric);
        apply_zero_mode(rest, zeros);
        apply_inline_format(rest, fmt_digits);
        return Ok(());
    }
    if let Some((num, dia)) = is_tool_def(line) {
        let d: f64 = dia
            .parse()
            .map_err(|_| EngineError::Parse(format!("bad tool diameter in '{line}'")))?;
        // A digit run long enough to overflow f64 parses as `inf`; it would reach
        // the geometry builders and saturate the nm cast (#308). Units aren't
        // known yet here, so the range check happens at select time.
        if !d.is_finite() {
            return Err(EngineError::Parse(format!(
                "tool diameter in '{line}' is not a finite number"
            )));
        }
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

/// An inline digit-format token on the `INCH`/`METRIC` line, e.g.
/// `METRIC,LZ,0000.00` (4 integer / 2 decimal digits). Decoding suppressed
/// coordinates with the wrong split silently mis-scales every hole position, so
/// this must win over the per-units default.
fn apply_inline_format(rest: &str, fmt_digits: &mut Option<(usize, usize)>) {
    for tok in rest.split(',') {
        let tok = tok.trim();
        if let Some((i, d)) = tok.split_once('.') {
            if !i.is_empty()
                && !d.is_empty()
                && i.chars().all(|c| c == '0')
                && d.chars().all(|c| c == '0')
            {
                *fmt_digits = Some((i.len(), d.len()));
                return;
            }
        }
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
    /// Running sum of vertices across all pushed contours, checked against the
    /// shared `MAX_POINTS_PER_LAYER` on every push (#308). The contour count is
    /// `contours.len()`, checked against `MAX_CONTOURS_PER_LAYER` the same way.
    total_points: usize,
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
            total_points: 0,
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
        // Repeat code `R<n>X<off>Y<off>`: repeat the previous hit n times, each
        // stepping by the (relative) offset. Must be handled before the generic
        // coordinate dispatch, which would misread the offset as an absolute hit.
        if let Some(rest) = line.strip_prefix('R') {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                return self.repeat_hits(rest);
            }
        }
        // A coordinate-bearing line: drill hit, G85 slot, or routed move.
        if line.contains('X') || line.contains('Y') {
            return self.coordinate_line(line);
        }
        // G00/G01 mode words with no coordinates, feed/speed, etc. — ignore.
        Ok(())
    }

    /// `R<n>X<off>Y<off>` (the text after `R`): n additional hits, each offset
    /// from the previous position by the given (relative) step.
    fn repeat_hits(&mut self, rest: &str) -> Result<()> {
        let nend = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let n: u32 = rest[..nend]
            .parse()
            .map_err(|_| EngineError::Parse(format!("bad repeat count in 'R{rest}'")))?;
        let frag = &rest[nend..];
        if !frag.contains('X') && !frag.contains('Y') {
            return Err(EngineError::Parse(format!(
                "repeat code 'R{rest}' carries no X/Y offset"
            )));
        }
        // A repeat count is pure amplification (`R4000000000X0.001` asks for 4e9
        // holes from one line, #308): refuse up front against the shared per-layer
        // contour budget rather than grinding towards it one hit at a time.
        let requested = self.contours.len().saturating_add(n as usize);
        if requested > MAX_CONTOURS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "contours",
                count: requested,
                limit: MAX_CONTOURS_PER_LAYER,
            });
        }
        // Offsets are relative steps in the file's format/units; a missing axis
        // steps by zero.
        let (dx, dy) = self.read_xy(frag, 0.0, 0.0)?;
        for _ in 0..n {
            self.x += dx;
            self.y += dy;
            self.emit_hit(self.x, self.y)?;
        }
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
        // Range-guard the nm diameter like the Gerber `dim()` chokepoint: an absurd
        // value must fail loud here, not saturate inside geom::ngon (#308).
        self.dia_nm = self.nm(d * self.unit_nm)?;
        Ok(())
    }

    /// Range/finiteness guard for a value already in nm — the Excellon
    /// equivalent of the Gerber `dim()` chokepoint. Returns the value unchanged
    /// (rounding to the grid happens in the geometry builders) so a guarded
    /// coordinate is exactly what the unguarded path would have used.
    fn nm(&self, v: f64) -> Result<f64> {
        snap_nm(v)?;
        Ok(v)
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
        // A G00/G01 word is a rout-mode MOVE: with the tool down (M15…M16) it cuts
        // a slot; with the tool up it only repositions — emitting a hit for a
        // tool-up rapid painted phantom holes at retract targets. A bare
        // coordinate line (no G word) in drill mode is a hit.
        let (coord, is_move) = if let Some(r) = line.strip_prefix("G00") {
            (r, true)
        } else if let Some(r) = line.strip_prefix("G01") {
            (r, true)
        } else {
            (line, false)
        };
        let (nx, ny) = self.read_xy(coord, self.x, self.y)?;
        if self.routing {
            // Tool down: any move cuts a slot segment from here to there.
            self.emit_slot(self.x, self.y, nx, ny)?;
        } else if !is_move {
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
        // Explicit-decimal fields accept anything `f64` parses (`1.0e300`,
        // `1.0e999` = inf); guard every decoded coordinate so it fails loud instead
        // of saturating the nm cast in the geometry builders (#308).
        let x = match extract_axis(frag, 'X') {
            Some(f) => self.nm(self.fmt.decode(f)? * self.unit_nm)?,
            None => dx,
        };
        let y = match extract_axis(frag, 'Y') {
            Some(f) => self.nm(self.fmt.decode(f)? * self.unit_nm)?,
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
        // Repeat codes accumulate `x += dx`, so guard the final position too.
        let (x, y) = (self.nm(x)?, self.nm(y)?);
        self.push(geom::ngon(x, y, self.dia_nm / 2.0))
    }

    fn emit_slot(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<()> {
        if self.dia_nm <= 0.0 {
            return Err(EngineError::Parse(
                "drill slot before a tool with a positive diameter was selected".into(),
            ));
        }
        let (x1, y1) = (self.nm(x1)?, self.nm(y1)?);
        let (x2, y2) = (self.nm(x2)?, self.nm(y2)?);
        self.push(geom::stadium(x1, y1, x2, y2, self.dia_nm / 2.0))
    }

    /// Append one contour, then enforce the per-layer contour and point ceilings
    /// shared with the Gerber front-end (#83, #308). Checked on every push — a
    /// single `R` line can emit thousands of hits, so a per-line check would let
    /// the loop run far past the budget before failing.
    fn push(&mut self, c: Contour) -> Result<()> {
        self.total_points += c.len();
        self.contours.push(c);
        if self.contours.len() > MAX_CONTOURS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "contours",
                count: self.contours.len(),
                limit: MAX_CONTOURS_PER_LAYER,
            });
        }
        if self.total_points > MAX_POINTS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "points",
                count: self.total_points,
                limit: MAX_POINTS_PER_LAYER,
            });
        }
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
    fn headerless_file_still_resolves_hits() {
        // No M48 and no %/M95 terminator — a form looks_like_excellon claims via
        // units + tool defs. The body must not be eaten by header scanning
        // (review finding: silently returned an empty set).
        let src = "INCH,LZ\nT1C0.032\nT1\nX0100Y0100\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 1, "the one drill hit must survive");
        // 2.4 LZ: "0100" right-pads to 010000 → 1.0 in = 25.4 mm.
        let bb = ps.bbox_nm().unwrap();
        let cx = (bb[0] + bb[2]) as f64 / 2.0 / 1e6;
        assert!(
            (cx - 25.4).abs() < 0.1,
            "hole at 1.0 in = 25.4 mm, got {cx}"
        );
    }

    #[test]
    fn inline_digit_format_overrides_default() {
        // METRIC,LZ,0000.00 declares a 4.2 split; decoding as the default 3.3
        // put every hole at one-tenth scale (review finding).
        let src = "M48\nMETRIC,LZ,0000.00\nT1C1.000\n%\nT1\nX010000Y010000\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        let bb = ps.bbox_nm().unwrap();
        let cx = (bb[0] + bb[2]) as f64 / 2.0 / 1e6;
        // 4.2 LZ: "010000" → 0100.00 → 100.0 mm (not 10.0 mm under 3.3).
        assert!(
            (cx - 100.0).abs() < 0.1,
            "hole at 100 mm under 4.2, got {cx}"
        );
    }

    #[test]
    fn repeat_code_emits_the_repeated_hits() {
        // R2X5.0 = repeat the previous hit twice, stepping +5 mm in X each time.
        // Was rendered as ONE phantom hole at the offset itself (review finding).
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX10.0Y10.0\nR2X5.0\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 3, "original + 2 repeats");
        let bb = ps.bbox_nm().unwrap();
        assert!(
            (bb[2] as f64 / 1e6 - 20.25).abs() < 0.1,
            "last hole at x=20 mm"
        );
        assert!(
            (bb[0] as f64 / 1e6 - 9.75).abs() < 0.1,
            "first hole at x=10 mm"
        );
    }

    #[test]
    fn g00_rapid_does_not_emit_a_hit() {
        // A tool-up rapid (G00) outside routing repositions only; it was emitting
        // a phantom hole at the move target (review finding).
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nG00X10.0Y10.0\nM15\nG01X15.0Y10.0\nM16\nG00X50.0Y50.0\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 1, "just the slot — no hole at (50,50)");
        let bb = ps.bbox_nm().unwrap();
        assert!(
            bb[2] as f64 / 1e6 < 16.0,
            "nothing near x=50, got max x {}",
            bb[2]
        );
    }

    #[test]
    fn tool_lines_with_feed_speed_fields_parse() {
        // Header def `T1C0.500F200S65` (diameter + feed/speed) and body select
        // `T01F200S65` are a common dialect; both were mishandled (review
        // findings: def rejected the file, select was silently ignored).
        let src = "M48\nMETRIC,TZ\nT1C0.500F200S65\n%\nT01F200S65\nX10.0Y10.0\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 1);
        // Diameter 0.5 mm → area ≈ π·0.25² ≈ 0.196 mm².
        assert!(
            (ps.area_mm2() - 0.196).abs() < 0.01,
            "got {}",
            ps.area_mm2()
        );
    }

    #[test]
    fn altium_tool_def_with_feed_speed_before_c_parses() {
        // Real Altium NC drill (validated against a production fab pack) writes
        // tool defs as `T1F00S00C0.00787` — feed/speed BEFORE the C field, with a
        // `;FILE_FORMAT=2:5` comment and INCH,LZ. This exact form failed to parse
        // ("selects tool T1 not defined in the header").
        let src = "M48\n;FILE_FORMAT=2:5\nINCH,LZ\n;TYPE=PLATED\nT1F00S00C0.00787\n%\nT1\nX0100000Y0100000\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 1);
        // 2.5 LZ: "0100000" pads right to 7 digits → 01.00000 in = 25.4 mm.
        let bb = ps.bbox_nm().unwrap();
        let cx = (bb[0] + bb[2]) as f64 / 2.0 / 1e6;
        assert!((cx - 25.4).abs() < 0.1, "hole at 1.0 in, got {cx} mm");
        // Diameter 0.00787 in = 0.2 mm → area ≈ π·0.1² ≈ 0.0314 mm².
        assert!(
            (ps.area_mm2() - 0.0314).abs() < 0.002,
            "got {}",
            ps.area_mm2()
        );
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

    #[test]
    fn huge_repeat_count_fails_loud_with_object_limit() {
        // `R4000000000X0.001` asks one line to emit 4e9 holes (#308). It must trip
        // the shared per-layer contour cap up front — not grind through the loop.
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX10.0Y10.0\nR4000000000X0.001\nM30\n";
        match resolve_excellon(src.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, count }) => {
                assert_eq!(what, "contours");
                assert_eq!(limit, MAX_CONTOURS_PER_LAYER);
                assert!(count > limit, "count {count} must exceed limit {limit}");
            }
            other => panic!("expected ObjectLimit, got {other:?}"),
        }
    }

    #[test]
    fn many_small_repeats_trip_the_point_cap() {
        // Each repeat line stays under the contour cap, but the summed vertices
        // (64 per hit) cross the shared point ceiling; the per-push check must
        // catch it (#308). Caps are lowered under cfg(test).
        let per_line = 100;
        let lines = MAX_POINTS_PER_LAYER / (per_line * geom::CIRCLE_SEGMENTS) + 2;
        assert!(
            lines * per_line < MAX_CONTOURS_PER_LAYER,
            "test must trip the point cap, not the contour cap"
        );
        let mut src = String::from("M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX0.0Y0.0\n");
        for _ in 0..lines {
            src.push_str(&format!("R{per_line}X0.001\n"));
        }
        src.push_str("M30\n");
        match resolve_excellon(src.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, .. }) => {
                assert_eq!(what, "points");
                assert_eq!(limit, MAX_POINTS_PER_LAYER);
            }
            other => panic!("expected ObjectLimit(points), got {other:?}"),
        }
    }

    #[test]
    fn absurd_format_digits_fail_loud() {
        // `;FILE_FORMAT=3:99999999999` made decode() zero-pad every suppressed
        // coordinate to ~1e11 characters and abort the process (#308). Both the
        // comment form and the inline `METRIC,LZ,000.0000000000` form must be a
        // parse error before any coordinate is decoded.
        let src =
            "M48\n;FILE_FORMAT=3:99999999999\nMETRIC,LZ\nT1C0.5\n%\nT1\nX010000Y010000\nM30\n";
        assert!(matches!(
            resolve_excellon(src.as_bytes()),
            Err(EngineError::Parse(_))
        ));
        let src = "M48\nMETRIC,LZ,0000000000.000\nT1C0.5\n%\nT1\nX010000Y010000\nM30\n";
        assert!(matches!(
            resolve_excellon(src.as_bytes()),
            Err(EngineError::Parse(_))
        ));
        // 9:9 is the largest accepted split and still decodes exactly.
        let src = "M48\n;FILE_FORMAT=9:9\nMETRIC,LZ\nT1C0.5\n%\nT1\nX000000010Y000000010\nM30\n";
        let ps = resolve_excellon(src.as_bytes()).unwrap();
        assert_eq!(ps.shapes.len(), 1);
    }

    #[test]
    fn non_finite_and_out_of_range_coordinates_fail_loud() {
        // Explicit-decimal coordinates accept anything f64 parses; each of these
        // used to reach geom::ngon and saturate `round() as i64` silently (#308).
        for coord in [
            "Xinf",
            "XNaN",
            "X1e300",
            "X1.0e300",
            "X1.0e999",
            "X-1.0e999",
        ] {
            let src = format!("M48\nMETRIC,TZ\nT1C0.500\n%\nT1\n{coord}Y10.0\nM30\n");
            let r = resolve_excellon(src.as_bytes());
            assert!(
                matches!(
                    r,
                    Err(EngineError::Parse(_)) | Err(EngineError::Geometry(_))
                ),
                "{coord}: expected a loud error, got {r:?}"
            );
        }
        // A tool-up rapid to an absurd point must fail at the move, not later.
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nG00X1.0e300Y0.0\nM30\n";
        assert!(matches!(
            resolve_excellon(src.as_bytes()),
            Err(EngineError::Geometry(_))
        ));
        // Repeats accumulating past the range fail at the hit that crosses it.
        let src = "M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX0.0Y0.0\nR3X60000000.0\nM30\n";
        assert!(matches!(
            resolve_excellon(src.as_bytes()),
            Err(EngineError::Geometry(_))
        ));
    }

    #[test]
    fn non_finite_tool_diameter_fails_loud() {
        // The tool-def scanner only admits digits and '.', so `inf` arrives as a
        // digit run too long for f64 (parses to +inf). It used to reach
        // geom::ngon as an infinite radius (#308).
        let inf_digits = "9".repeat(400);
        let src = format!("M48\nMETRIC,TZ\nT1C{inf_digits}\n%\nT1\nX10.0Y10.0\nM30\n");
        assert!(matches!(
            resolve_excellon(src.as_bytes()),
            Err(EngineError::Parse(_))
        ));
        // Finite but past the nm range (1e300 mm) must fail at tool select.
        let big = format!("1{}", "0".repeat(300));
        let src = format!("M48\nMETRIC,TZ\nT1C{big}\n%\nT1\nX10.0Y10.0\nM30\n");
        assert!(matches!(
            resolve_excellon(src.as_bytes()),
            Err(EngineError::Geometry(_))
        ));
    }

    #[test]
    fn resolver_never_panics_on_garbage() {
        // The catch_unwind boundary (#308) must turn any panic in the header/body
        // walk into a typed error: resolve_excellon returns for ANY bytes. The
        // fuzz target covers the broader space.
        let inputs: &[&[u8]] = &[
            b"",
            b"\xff\xfe\x00\x01 not a drill file",
            b"M48\n",                           // header never terminated
            b"M48\nMETRIC\n%\nT1\nX\nY\n",      // empty coordinate fields
            b"M48\nMETRIC,TZ\nT1C\n%\nR\nRX\n", // malformed tool def and repeats
            b"M48\nMETRIC,TZ\nT1C0.5\n%\nT1\nX1.0Y1.0G85\n",
        ];
        for inp in inputs {
            let _ = resolve_excellon(inp); // Ok or Err — must not panic.
        }
    }
}
