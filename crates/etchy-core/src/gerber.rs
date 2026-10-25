//! Gerber RS-274X front-end: bytes → resolved filled geometry ([`PolygonSet`]).
//!
//! Takes **bytes** (never a path — the library owns no I/O policy). A
//! normalization pass handles compact (`*`-packed) lines and deprecated G-codes
//! (Spike 2), then the graphics-state machine walks the command AST resolving
//! aperture flashes (circle/rect/obround/macro), stroked draws (`D01` lines and
//! `G02/G03` arcs), `G36/G37` region fills, and `LPD/LPC` polarity. The layer's
//! copper is `dark − clear`, computed in one boolean pass.
//!
//! Anything it cannot render faithfully is a loud [`EngineError`] — never a
//! silently dropped or wrong-but-quiet result (the trust bar).

use std::collections::HashMap;
use std::io::{BufReader, Cursor};

use gerber_parser::gerber_types::{
    Aperture as GtAperture, ApertureMacro, Command, CoordinateMode, CoordinateOffset, Coordinates,
    DCode, ExtendedCode, FunctionCode, GCode, InterpolationMode, MacroBoolean, MacroContent,
    MacroDecimal, MacroInteger, Operation, Polarity as GtPolarity, QuadrantMode, Unit,
};
use gerber_parser::gerber_types::{AxisSelect, ImageMirroring, ImagePolarity, StepAndRepeat};
use gerber_parser::parse;

use crate::boolean;
use crate::error::{EngineError, Result};
use crate::geo::{quantize_mm, snap_nm, Contour, PolygonSet, Pt};
use crate::geom;

/// Per-layer contour ceiling. A tiny file with many large arcs or region loops
/// can amplify into millions of contours and exhaust CPU/RAM before the boolean
/// pass even runs (#83). We fail loud at this bound instead of grinding or
/// OOM-ing. Set generously so legitimate dense/curvy boards never trip it;
/// lowered under test so the guard can be exercised without huge allocations.
/// Shared with the Excellon front-end (`R` repeat codes, #308) so one layer means
/// one budget regardless of format.
#[cfg(not(test))]
pub(crate) const MAX_CONTOURS_PER_LAYER: usize = 5_000_000;
#[cfg(test)]
pub(crate) const MAX_CONTOURS_PER_LAYER: usize = 5_000;

/// Per-layer total-point ceiling (#83). The contour count alone doesn't bound
/// this — one region loop or a near-full arc can carry thousands of points, so a
/// handful of contours can still amplify into a huge point set (and a slow/greedy
/// boolean pass). Fail loud once the summed vertex count crosses the bound.
#[cfg(not(test))]
pub(crate) const MAX_POINTS_PER_LAYER: usize = 20_000_000;
#[cfg(test)]
pub(crate) const MAX_POINTS_PER_LAYER: usize = 30_000;

/// Per-layer polarity-span ceiling (#83). Each span is one boolean pass over the
/// accumulated geometry, so a file that toggles `%LP` on every object turns the
/// resolve into an O(N²) grind. Real layers have a handful of polarity groups;
/// this bound is far above any of them.
#[cfg(not(test))]
const MAX_SPANS_PER_LAYER: usize = 100_000;
#[cfg(test)]
const MAX_SPANS_PER_LAYER: usize = 2_000;

/// Parse + resolve one Gerber layer's bytes into its filled copper geometry.
pub fn resolve_layer(bytes: &[u8]) -> Result<PolygonSet> {
    let normalized = normalize(bytes)?;
    // The third-party parser is the first code to touch attacker-controlled bytes.
    // Contain a panic in it as a loud typed error instead of unwinding through the
    // caller / aborting a batch (#85). Relies on panic=unwind (kept that way in the
    // release profile precisely so this works — see #87/#100).
    let doc = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parse(BufReader::new(Cursor::new(normalized.as_bytes())))
            .map_err(|(_, e)| EngineError::Parse(format!("{e:?}")))
    }))
    .map_err(|_| EngineError::Parse("gerber parser panicked on this input".into()))??;

    let errs = doc.errors();
    if !errs.is_empty() {
        return Err(EngineError::CommandErrors {
            count: errs.len(),
            first: format!("{:?}", errs[0]),
        });
    }

    let unit_to_mm = match doc.units {
        Some(Unit::Millimeters) => 1.0,
        Some(Unit::Inches) => 25.4,
        None => return Err(EngineError::UnitsUnresolved),
    };
    let nm_per_unit = unit_to_mm * 1.0e6; // document unit → nm

    // Coordinate grid step in nm, for the G74 single-quadrant tolerance (#234/#274).
    // Re-sniff the %FS decimals from the bytes (cheap, independent of the geometry
    // parse); 0 if unresolved, so single_quadrant_center falls back to relative-only.
    let grid_step_nm = gerber_format(bytes)
        .ok()
        .map(|f| nm_per_unit * 10f64.powi(-(f.dec_digits as i32)))
        .unwrap_or(0.0);

    // Collect aperture-macro definitions (referenced by macro apertures).
    let mut macros: HashMap<&str, &ApertureMacro> = HashMap::new();
    for cmd in doc.commands() {
        if let Command::ExtendedCode(ExtendedCode::ApertureMacro(am)) = cmd {
            macros.insert(am.name.as_str(), am);
        }
    }

    let mut m = Machine::new(
        &doc.apertures,
        &macros,
        unit_to_mm,
        nm_per_unit,
        grid_step_nm,
    );
    for cmd in doc.commands() {
        m.step(cmd)?;
    }
    // Sequential polarity: paint the spans in order — dark unions copper on, clear
    // subtracts it — so a later dark span correctly repaints over an earlier clear
    // (an order-independent dark − clear erases such repaints).
    let mut acc: Vec<Contour> = Vec::new();
    let mut result = PolygonSet::default();
    for (is_dark, contours) in &m.spans {
        result = if *is_dark {
            boolean::union(&acc, contours)
        } else {
            boolean::difference(&acc, contours)
        };
        acc = boolean::flatten(&result);
    }
    Ok(result)
}

/// Coordinate units a Gerber declares via `%MO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    Inches,
    Millimeters,
}

/// A layer's coordinate system: units (`%MO`) + the `%FS` integer/decimal digit
/// counts. Two revisions exported with different `GerberFormat` quantize identical
/// geometry onto different grids, producing spurious sub-µm "rim" differences
/// around every edge — the dominant noise when diffing same-design re-exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GerberFormat {
    pub units: Units,
    pub int_digits: u8,
    pub dec_digits: u8,
}

impl GerberFormat {
    /// Compact human form, e.g. `in@2.5` or `mm@4.4`.
    pub fn describe(&self) -> String {
        let u = match self.units {
            Units::Inches => "in",
            Units::Millimeters => "mm",
        };
        format!("{u}@{}.{}", self.int_digits, self.dec_digits)
    }
}

/// Extract a layer's coordinate format (`%MO` units + `%FS` digit counts) straight
/// from its bytes — independent of the geometry parse, so callers can compare two
/// revisions' grids cheaply.
pub fn gerber_format(bytes: &[u8]) -> Result<GerberFormat> {
    let text = String::from_utf8_lossy(bytes);
    let units = if text.contains("%MOIN") {
        Units::Inches
    } else if text.contains("%MOMM") {
        Units::Millimeters
    } else {
        return Err(EngineError::UnitsUnresolved);
    };
    // %FSLAX<int><dec>Y<int><dec>*%  — digits after the first 'X' in the %FS block.
    let fs = text
        .find("%FS")
        .ok_or_else(|| EngineError::Parse("missing %FS format spec".into()))?;
    let xpos = text[fs..]
        .find('X')
        .ok_or_else(|| EngineError::Parse("no X in %FS".into()))?;
    let digits: Vec<u8> = text[fs + xpos + 1..]
        .chars()
        .take(2)
        .filter_map(|c| c.to_digit(10).map(|d| d as u8))
        .collect();
    if digits.len() != 2 {
        return Err(EngineError::Parse("malformed %FS coordinate digits".into()));
    }
    Ok(GerberFormat {
        units,
        int_digits: digits[0],
        dec_digits: digits[1],
    })
}

/// If two revisions' coordinate formats differ, a human-facing warning explaining
/// the spurious sub-µm "rim" diffs that mismatch causes; otherwise `None`.
pub fn coordinate_mismatch_warning(old: &GerberFormat, new: &GerberFormat) -> Option<String> {
    if old == new {
        return None;
    }
    Some(format!(
        "Revisions were exported with different coordinate systems (old {}, new {}). \
         Identical geometry then quantizes onto different grids, so the diff shows spurious \
         sub-µm rounding differences (\"rims\") around every feature and the changed-area \
         totals are inflated — they may not reflect real design changes. Re-export both fab \
         packs with the same units and coordinate format for a clean diff.",
        old.describe(),
        new.describe()
    ))
}

// ---------------------------------------------------------------------------
// Normalization: compact-line split + deprecated G-code handling (Spike 2)
// ---------------------------------------------------------------------------

/// Re-emit one command per line, splitting compact (`*`-packed, gEDA-style) lines
/// that the line-oriented parser would silently truncate; drop deprecated mode
/// codes `G70/G71/G90` (units come from `%MO`); strip the deprecated `G54`
/// aperture-select prefix. Fails loud on `G91` (incremental coordinates), which
/// would otherwise be a silent misread. `%…%` extended blocks pass through intact.
/// A command that never gets its `*` — before a `%` block or at end of file — is a
/// parse error, not a silent drop (#313): losing it would delete a flash or draw.
fn normalize(bytes: &[u8]) -> Result<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut buf = String::new();
    let mut in_ext = false;

    for ch in text.chars() {
        if in_ext {
            buf.push(ch);
            if ch == '%' {
                let block = buf.trim();
                // `%LN` only names the layer; it carries no geometry (#316).
                if !block.starts_with("%LN") || !block.ends_with("*%") {
                    if identity_repeat(block) {
                        out.push_str("%SRX1Y1I0J0*%");
                    } else {
                        out.push_str(block);
                    }
                    out.push('\n');
                }
                buf.clear();
                in_ext = false;
            }
            continue;
        }
        match ch {
            '%' => {
                check_terminated(&buf, "before '%'")?;
                buf.clear();
                buf.push('%');
                in_ext = true;
            }
            '*' => {
                let tok = buf.trim();
                if !tok.is_empty() {
                    emit_command(tok, &mut out)?;
                }
                buf.clear();
            }
            '\n' | '\r' => {} // newlines are insignificant outside a command
            _ => buf.push(ch),
        }
    }
    if in_ext {
        return Err(EngineError::Parse(format!(
            "unterminated extended command block at end of file: {}",
            excerpt(buf.trim())
        )));
    }
    check_terminated(&buf, "at end of file")?;
    Ok(out)
}

/// True for a `%SR` that repeats once with zero step, in any spelling that
/// omits the zero `I`/`J` fields (`%SRX1Y1*%`). The parser only reads the full
/// form, so these are rewritten to `%SRX1Y1I0J0*%` and accepted downstream.
fn identity_repeat(block: &str) -> bool {
    let Some(body) = block.strip_prefix("%SR").and_then(|s| s.strip_suffix("*%")) else {
        return false;
    };
    let mut rest = body;
    for (tag, default) in [('X', 1.0), ('Y', 1.0), ('I', 0.0), ('J', 0.0)] {
        if let Some(tail) = rest.strip_prefix(tag) {
            let end = tail.find(['X', 'Y', 'I', 'J']).unwrap_or(tail.len());
            let Ok(v) = tail[..end].parse::<f64>() else {
                return false;
            };
            if v != default {
                return false;
            }
            rest = &tail[end..];
        } else if tag == 'X' || tag == 'Y' {
            return false;
        }
    }
    rest.is_empty()
}

/// Fail loud if `buf` still holds a command that never received its `*` (#313).
/// Whitespace alone is fine — blank lines between commands are legitimate.
fn check_terminated(buf: &str, place: &str) -> Result<()> {
    let tok = buf.trim();
    if tok.is_empty() {
        return Ok(());
    }
    Err(EngineError::Parse(format!(
        "unterminated command {place}: {}",
        excerpt(tok)
    )))
}

/// First 40 chars of `s` for an error message (char-boundary safe).
fn excerpt(s: &str) -> String {
    const MAX: usize = 40;
    let mut e: String = s.chars().take(MAX).collect();
    if s.chars().count() > MAX {
        e.push('…');
    }
    e
}

fn emit_command(tok: &str, out: &mut String) -> Result<()> {
    match tok {
        "G70" | "G71" | "G90" | "G54" => return Ok(()), // deprecated mode/select — drop
        "G91" => {
            return Err(EngineError::Unsupported {
                feature: "incremental coordinates (G91)".into(),
            })
        }
        _ => {}
    }
    // Deprecated combined select, e.g. "G54D10" → "D10".
    if let Some(rest) = tok.strip_prefix("G54").filter(|r| r.starts_with('D')) {
        return emit_command(rest, out);
    }
    // Split a leading modal G-code from a following operation in the same block,
    // e.g. `G02X..Y..I..J..D01` → `G02*` + `X..Y..I..J..D01*`. The line-oriented
    // parser drops the I/J (and the mode) otherwise — a silent geometry loss.
    if let Some(rest) = tok.strip_prefix('G') {
        let ndigits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if ndigits > 0 {
            let after = &rest[ndigits..];
            if after.starts_with(['X', 'Y', 'I', 'J', 'D']) {
                emit_command(&format!("G{}", &rest[..ndigits]), out)?;
                return emit_command(after, out);
            }
        }
    }
    out.push_str(tok);
    out.push_str("*\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Graphics-state machine
// ---------------------------------------------------------------------------

struct Machine<'a> {
    apertures: &'a HashMap<i32, GtAperture>,
    macros: &'a HashMap<&'a str, &'a ApertureMacro>,
    unit_to_mm: f64,
    nm_per_unit: f64,
    /// Source coordinate grid step in nm (`nm_per_unit × 10^-dec_digits`), used to
    /// size the G74 single-quadrant tolerance to the real quantization. 0 when the
    /// format couldn't be resolved (fall back to a radius-relative tolerance).
    grid_step_nm: f64,

    /// Objects in paint order, batched into runs of one polarity. Gerber polarity
    /// is sequential — a later dark run repaints over an earlier clear — so these
    /// are resolved in order (union for dark, subtract for clear), not as one
    /// order-independent `dark − clear` pass.
    spans: Vec<(bool, Vec<Contour>)>,

    cur: Pt,
    ap: Option<i32>,
    interp: InterpolationMode,
    /// Arc quadrant mode (`G74` single / `G75` multi), `None` until the file
    /// declares one. A `G74` switches the I/J interpretation to single-quadrant
    /// (#234). There is no default: pre-2012 RS-274X made single-quadrant the
    /// implicit mode while modern exporters always emit `G75`, so an arc drawn
    /// before either code is ambiguous and fails loud rather than being read as
    /// multi-quadrant with a possibly wrong centre (#312). Linear-only files
    /// never consult it.
    quadrant: Option<QuadrantMode>,
    polarity_dark: bool,

    in_region: bool,
    region_loops: Vec<Contour>,
    cur_loop: Contour,

    /// Running count of contours pushed this layer, checked against
    /// `MAX_CONTOURS_PER_LAYER` once per command (#83).
    emitted: usize,
    /// Running sum of vertices across all pushed contours, checked against
    /// `MAX_POINTS_PER_LAYER` once per command (#83).
    total_points: usize,
    /// Vertices buffered in the open region (`region_loops` + `cur_loop`), not yet
    /// pushed. Charged to the point ceiling alongside `total_points` every command,
    /// so a region that never reaches `G37` can't grow without bound (#315). Reset
    /// when the region closes: the filled contours are then counted by `push`.
    region_points: usize,
    /// Boolean passes spent resolving mixed-exposure macro images (#306). Each is
    /// the same kind of pass as a polarity span, so they share `MAX_SPANS_PER_LAYER`.
    macro_passes: usize,
}

impl<'a> Machine<'a> {
    fn new(
        apertures: &'a HashMap<i32, GtAperture>,
        macros: &'a HashMap<&'a str, &'a ApertureMacro>,
        unit_to_mm: f64,
        nm_per_unit: f64,
        grid_step_nm: f64,
    ) -> Self {
        Self {
            apertures,
            macros,
            unit_to_mm,
            nm_per_unit,
            grid_step_nm,
            spans: Vec::new(),
            cur: Pt::new(0, 0),
            ap: None,
            interp: InterpolationMode::Linear,
            quadrant: None,
            polarity_dark: true,
            in_region: false,
            region_loops: Vec::new(),
            cur_loop: Vec::new(),
            emitted: 0,
            total_points: 0,
            region_points: 0,
            macro_passes: 0,
        }
    }

    fn step(&mut self, cmd: &Command) -> Result<()> {
        match cmd {
            Command::FunctionCode(FunctionCode::DCode(DCode::SelectAperture(code))) => {
                self.ap = Some(*code);
            }
            Command::FunctionCode(FunctionCode::DCode(DCode::Operation(op))) => {
                self.operation(op)?
            }
            Command::FunctionCode(FunctionCode::GCode(g)) => match g {
                GCode::InterpolationMode(m) => self.interp = *m,
                GCode::QuadrantMode(q) => self.quadrant = Some(*q),
                GCode::RegionMode(true) => self.begin_region()?,
                GCode::RegionMode(false) => self.end_region()?,
                GCode::Comment(_)
                | GCode::Unit(_)
                | GCode::CoordinateMode(_)
                | GCode::SelectAperture => {}
            },
            Command::FunctionCode(FunctionCode::MCode(_)) => {}
            Command::ExtendedCode(ec) => self.extended(ec)?,
        }
        // Bound per-layer object count: a tiny file with many arcs / region loops
        // must fail loud, not amplify into a CPU/RAM blow-up (#83).
        if self.emitted > MAX_CONTOURS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "contours",
                count: self.emitted,
                limit: MAX_CONTOURS_PER_LAYER,
            });
        }
        // …and the total vertex count (a few huge contours) and polarity-span count
        // (O(N²) boolean passes), which the contour count alone doesn't bound (#83).
        // Points still buffered in an open region count too (#315).
        let points = self.total_points + self.region_points;
        if points > MAX_POINTS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "points",
                count: points,
                limit: MAX_POINTS_PER_LAYER,
            });
        }
        let passes = self.spans.len() + self.macro_passes;
        if passes > MAX_SPANS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "polarity spans",
                count: passes,
                limit: MAX_SPANS_PER_LAYER,
            });
        }
        Ok(())
    }

    fn extended(&mut self, ec: &ExtendedCode) -> Result<()> {
        match ec {
            ExtendedCode::LoadPolarity(p) => self.polarity_dark = matches!(p, GtPolarity::Dark),
            // #305: `%FS…I…` declares incremental notation — every coordinate is an
            // offset from the previous point. `resolve()` reads coordinates as
            // absolute, so accepting the file would render wrong geometry silently.
            // (The deprecated `G91` form is caught earlier, in `normalize`.)
            ExtendedCode::CoordinateFormat(fs) => {
                if fs.coordinate_mode == CoordinateMode::Incremental {
                    return Err(unsupported(
                        "incremental coordinate notation (%FS…I) is not supported",
                    ));
                }
            }
            ExtendedCode::Unit(_)
            | ExtendedCode::ApertureDefinition(_)
            | ExtendedCode::ApertureMacro(_)
            | ExtendedCode::FileAttribute(_)
            | ExtendedCode::ObjectAttribute(_)
            | ExtendedCode::ApertureAttribute(_)
            | ExtendedCode::DeleteAttribute(_)
            | ExtendedCode::ImageName(_) => {}
            ExtendedCode::ImagePolarity(ImagePolarity::Positive)
            | ExtendedCode::MirrorImage(ImageMirroring::None)
            | ExtendedCode::AxisSelect(AxisSelect::AXBY) => {}
            ExtendedCode::OffsetImage(v) if v.a == 0.0 && v.b == 0.0 => {}
            ExtendedCode::ScaleImage(v) if v.a == 1.0 && v.b == 1.0 => {}
            ExtendedCode::StepAndRepeat(StepAndRepeat::Open {
                repeat_x: 1,
                repeat_y: 1,
                distance_x,
                distance_y,
            }) if *distance_x == 0.0 && *distance_y == 0.0 => {}
            ExtendedCode::StepAndRepeat(StepAndRepeat::Close) => {}
            other => {
                let name = format!("{other:?}");
                let tag = name.split([' ', '(']).next().unwrap_or("extended code");
                return Err(unsupported(&format!("extended code {tag}")));
            }
        }
        Ok(())
    }

    fn operation(&mut self, op: &Operation) -> Result<()> {
        match op {
            Operation::Move(coords) => {
                let to = self.resolve(coords)?;
                if self.in_region && !self.cur_loop.is_empty() {
                    self.region_loops.push(std::mem::take(&mut self.cur_loop));
                }
                if self.in_region {
                    self.cur_loop.push(to);
                    self.region_points += 1;
                }
                self.cur = to;
            }
            Operation::Interpolate(coords, offset) => {
                let to = self.resolve(coords)?;
                if self.in_region {
                    self.region_segment(to, offset)?;
                } else {
                    self.draw_segment(to, offset)?;
                }
                self.cur = to;
            }
            Operation::Flash(coords) => {
                let at = self.resolve(coords)?;
                self.cur = at;
                self.flash(at)?;
            }
        }
        Ok(())
    }

    // --- regions ---

    fn begin_region(&mut self) -> Result<()> {
        // A second `G36` before the `G37` would silently discard every loop
        // collected so far (#314): fail loud instead.
        if self.in_region {
            return Err(unsupported("G36 inside an open region"));
        }
        self.in_region = true;
        self.region_loops.clear();
        self.cur_loop.clear();
        self.region_points = 0;
        Ok(())
    }

    fn end_region(&mut self) -> Result<()> {
        // `G37` with no region open has nothing to close; the file is malformed
        // and used to pass as a no-op (#314).
        if !self.in_region {
            return Err(unsupported("G37 outside a region"));
        }
        if !self.cur_loop.is_empty() {
            self.region_loops.push(std::mem::take(&mut self.cur_loop));
        }
        if !self.region_loops.is_empty() {
            // Even-odd fill normalizes winding/holes; result joins the layer set.
            let filled = boolean::fill_even_odd(&self.region_loops);
            for c in filled {
                self.push(c, true); // region exposure is on; polarity applied in push
            }
            self.region_loops.clear();
        }
        // The buffered loops are gone; what survived the fill was charged by `push`.
        self.region_points = 0;
        self.in_region = false;
        Ok(())
    }

    fn region_segment(&mut self, to: Pt, offset: &Option<CoordinateOffset>) -> Result<()> {
        // A contour starts at the current point (RS-274X 4.10: the first `D01`
        // after the `D02` begins it there). A `D02` inside the region seeds the
        // loop itself; when the region opens straight into `D01`s the start vertex
        // was never recorded and the contour lost its first corner (#314). The arc
        // path below relies on this too — it skips the first arc point as `cur`.
        if self.cur_loop.is_empty() {
            self.cur_loop.push(self.cur);
            self.region_points += 1;
        }
        if self.is_arc(offset) {
            let ccw = matches!(self.interp, InterpolationMode::CounterclockwiseCircular);
            let center = self.arc_center(to, offset, ccw)?;
            let pts = geom::arc_points(
                self.cur.x as f64,
                self.cur.y as f64,
                to.x as f64,
                to.y as f64,
                center.0,
                center.1,
                ccw,
            );
            let before = self.cur_loop.len();
            self.cur_loop.extend(pts.into_iter().skip(1)); // first == cur (already present)
            self.region_points += self.cur_loop.len() - before;
        } else {
            self.cur_loop.push(to);
            self.region_points += 1;
        }
        Ok(())
    }

    /// A draw is an arc only if the circular mode is active **and** an I/J offset
    /// is present. Real exporters (e.g. Altium) leave circular mode set after a
    /// region's arc and then emit plain `D01` line segments with no I/J; every
    /// Gerber viewer treats those as linear. Rendering them as lines is the
    /// universal interpretation — not a silent drop.
    fn is_arc(&self, offset: &Option<CoordinateOffset>) -> bool {
        !matches!(self.interp, InterpolationMode::Linear) && offset.is_some()
    }

    // --- stroked draws ---

    fn draw_segment(&mut self, to: Pt, offset: &Option<CoordinateOffset>) -> Result<()> {
        let r = self.stroke_radius_nm()?;
        if self.is_arc(offset) {
            let ccw = matches!(self.interp, InterpolationMode::CounterclockwiseCircular);
            let center = self.arc_center(to, offset, ccw)?;
            let pts = geom::arc_points(
                self.cur.x as f64,
                self.cur.y as f64,
                to.x as f64,
                to.y as f64,
                center.0,
                center.1,
                ccw,
            );
            for w in pts.windows(2) {
                let c = geom::stadium(
                    w[0].x as f64,
                    w[0].y as f64,
                    w[1].x as f64,
                    w[1].y as f64,
                    r,
                );
                self.push(c, true);
            }
        } else {
            let c = geom::stadium(
                self.cur.x as f64,
                self.cur.y as f64,
                to.x as f64,
                to.y as f64,
                r,
            );
            self.push(c, true);
        }
        Ok(())
    }

    /// Stroke half-width (nm). Only circle apertures may stroke a path.
    fn stroke_radius_nm(&self) -> Result<f64> {
        let ap = self.current_aperture()?;
        match ap {
            GtAperture::Circle(c) if c.hole_diameter.is_none() => Ok(self.dim(c.diameter)? / 2.0),
            _ => Err(unsupported("stroking a path with a non-circle aperture")),
        }
    }

    /// Resolve an arc's centre from its I/J offset. `to`/`ccw` are only consulted
    /// in single-quadrant mode, where I/J are unsigned and the centre is chosen so
    /// the arc stays <=90° through the end point (#234).
    fn arc_center(
        &self,
        to: Pt,
        offset: &Option<CoordinateOffset>,
        ccw: bool,
    ) -> Result<(f64, f64)> {
        let o = offset
            .as_ref()
            .ok_or_else(|| unsupported("arc without I/J offset"))?;
        // I/J are relative to the current point, in document units → nm.
        let i = o.x.map(f64::from).unwrap_or(0.0) * self.nm_per_unit;
        let j = o.y.map(f64::from).unwrap_or(0.0) * self.nm_per_unit;
        let (fx, fy) = (self.cur.x as f64, self.cur.y as f64);
        match self.quadrant {
            // No G74/G75 yet: the I/J reading is ambiguous (legacy default was
            // single, modern files always declare G75) — fail loud, not guess (#312).
            None => Err(unsupported(
                "arc before any G74/G75 quadrant mode declaration — add G75 \
                 (multi-quadrant) to the file",
            )),
            // Multi-quadrant (G75): I/J are signed offsets to the centre.
            Some(QuadrantMode::Multi) => Ok((fx + i, fy + j)),
            // Single-quadrant (G74): I/J are magnitudes; pick the valid <=90° corner
            // or fail loud on an inconsistent arc rather than render a wrong centre.
            // A coincident start/end is a 0° or 360° sweep — neither fits the ≤90°
            // mode, and `arc_points` would turn it into a full ring (#321).
            Some(QuadrantMode::Single) if to == self.cur => Err(unsupported(
                "G74 single-quadrant arc with coincident start and end points \
                 (ambiguous zero-length vs full circle; use G75 for a full circle)",
            )),
            Some(QuadrantMode::Single) => geom::single_quadrant_center(
                fx,
                fy,
                to.x as f64,
                to.y as f64,
                i.abs(),
                j.abs(),
                ccw,
                self.grid_step_nm,
            )
            .ok_or_else(|| {
                unsupported(
                    "G74 single-quadrant arc with no valid <=90° centre \
                             (inconsistent endpoints/offsets)",
                )
            }),
        }
    }

    // --- flashes ---

    fn flash(&mut self, at: Pt) -> Result<()> {
        let ap = self.current_aperture()?;
        let (ax, ay) = (at.x as f64, at.y as f64);
        match ap {
            GtAperture::Circle(c) => {
                if c.hole_diameter.is_some() {
                    return Err(unsupported("drilled (hole) circle aperture"));
                }
                let c = geom::ngon(ax, ay, self.dim(c.diameter)? / 2.0);
                self.push(c, true);
            }
            GtAperture::Rectangle(r) => {
                if r.hole_diameter.is_some() {
                    return Err(unsupported("drilled (hole) rectangle aperture"));
                }
                let c = geom::rect(ax, ay, self.dim(r.x)?, self.dim(r.y)?);
                self.push(c, true);
            }
            GtAperture::Obround(r) => {
                if r.hole_diameter.is_some() {
                    return Err(unsupported("drilled (hole) obround aperture"));
                }
                let c = geom::obround(ax, ay, self.dim(r.x)?, self.dim(r.y)?);
                self.push(c, true);
            }
            GtAperture::Polygon(p) => {
                if p.hole_diameter.is_some() {
                    return Err(unsupported("drilled (hole) polygon aperture"));
                }
                let n = p.vertices as usize;
                if !(3..=64).contains(&n) {
                    return Err(unsupported(
                        "polygon aperture with out-of-range vertex count",
                    ));
                }
                let c = polygon_ngon(
                    ax,
                    ay,
                    self.dim(p.diameter)? / 2.0,
                    n,
                    p.rotation.unwrap_or(0.0),
                );
                self.push(c, true);
            }
            GtAperture::Macro(name, args) => {
                let am = *self
                    .macros
                    .get(name.as_str())
                    .ok_or_else(|| unsupported(&format!("undefined macro {name}")))?;
                self.flash_macro(am, args.as_deref().unwrap_or(&[]), at)?;
            }
        }
        Ok(())
    }

    /// Flash an aperture macro at `at`.
    ///
    /// RS-274X §4.5.1.2: a primitive with exposure **off** erases only from the
    /// macro's *own* image drawn so far, and the erased part is transparent when
    /// the aperture is flashed. So a macro that contains any exposure-off primitive
    /// is resolved into its own polygon set first (sequential union / subtract in
    /// primitive order) and the result is pushed with the layer polarity; pushing
    /// the off-primitive as a layer-wide clear would erase copper drawn *before* the
    /// flash (a track under a ring's hole) and depend on draw order (#306). A
    /// macro with only exposure-on primitives is pushed primitive by primitive —
    /// the layer union merges them identically, without the extra boolean pass.
    fn flash_macro(&mut self, am: &ApertureMacro, args: &[MacroDecimal], at: Pt) -> Result<()> {
        let (ax, ay) = (at.x as f64, at.y as f64);
        let mut prims: Vec<(bool, Contour)> = Vec::with_capacity(am.content.len());
        let mut vars = HashMap::new();
        for (i, arg) in args.iter().enumerate() {
            vars.insert((i + 1) as u32, md(arg, &vars)?);
        }
        for content in &am.content {
            if let MacroContent::VariableDefinition(v) = content {
                let value = expression(&v.expression, &vars)?;
                if v.number == 0 {
                    return Err(unsupported("macro variable $0"));
                }
                vars.insert(v.number, value);
            } else if let MacroContent::Thermal(t) = content {
                for c in self.macro_thermal(t, ax, ay, &vars)? {
                    prims.push((true, c));
                }
            } else if let Some(p) = self.macro_primitive(content, ax, ay, &vars)? {
                prims.push(p);
            }
        }
        if prims.iter().all(|(exp, _)| *exp) {
            for (_, c) in prims {
                self.push(c, true);
            }
            return Ok(());
        }
        // Resolve the aperture image locally: runs of same-exposure primitives are
        // one boolean pass each, in order (like the layer's polarity spans).
        //
        // Amplification (#83): the primitives never reach `push`, and each run is a
        // boolean pass over the growing macro image — the same O(runs·N) grind the
        // span ceiling bounds at layer level. So the primitives are charged to the
        // contour/point ceilings up front, and every pass is charged to the span
        // ceiling *before* it runs, so a crafted 100k-primitive macro fails loud
        // instead of computing for hours inside a single flash.
        self.emitted += prims.len();
        self.total_points += prims.iter().map(|(_, c)| c.len()).sum::<usize>();
        if self.emitted > MAX_CONTOURS_PER_LAYER {
            return Err(EngineError::ObjectLimit {
                what: "contours",
                count: self.emitted,
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
        let mut acc: Vec<Contour> = Vec::new();
        let mut i = 0;
        while i < prims.len() {
            let exp = prims[i].0;
            let mut run: Vec<Contour> = Vec::new();
            while i < prims.len() && prims[i].0 == exp {
                run.push(std::mem::take(&mut prims[i].1));
                i += 1;
            }
            self.macro_passes += 1;
            let passes = self.spans.len() + self.macro_passes;
            if passes > MAX_SPANS_PER_LAYER {
                return Err(EngineError::ObjectLimit {
                    what: "polarity spans",
                    count: passes,
                    limit: MAX_SPANS_PER_LAYER,
                });
            }
            let set = if exp {
                boolean::union(&acc, &run)
            } else {
                boolean::difference(&acc, &run)
            };
            acc = boolean::flatten(&set);
        }
        // Outers arrive CCW and holes CW, so pushing them flat keeps the holes
        // transparent under the layer's NonZero union/difference (a track under
        // the hole keeps winding +1 in a dark span; nothing is subtracted there in
        // a clear span).
        for c in acc {
            self.push(c, true);
        }
        Ok(())
    }

    /// One macro primitive → its exposure and contour in absolute nm (`None` for
    /// comments / variable definitions). Every contour is wound CCW (a solid), so
    /// the caller can union / subtract them uniformly; an Outline's winding is the
    /// exporter's choice and is normalized here (the track→pad notch, #13).
    fn macro_primitive(
        &self,
        content: &MacroContent,
        ax: f64,
        ay: f64,
        vars: &HashMap<u32, f64>,
    ) -> Result<Option<(bool, Contour)>> {
        Ok(Some(match content {
            MacroContent::Comment(_) | MacroContent::VariableDefinition(_) => return Ok(None),
            MacroContent::Circle(c) => {
                let exp = mbool(&c.exposure, vars)?;
                let d = self.dim(md(&c.diameter, vars)?)?;
                let (cx, cy) = self.macro_pt(&c.center, c.angle.as_ref(), ax, ay, vars)?;
                (exp, geom::ngon(cx, cy, d / 2.0))
            }
            MacroContent::CenterLine(l) => {
                let exp = mbool(&l.exposure, vars)?;
                let (w, h) = (
                    self.dim(md(&l.dimensions.0, vars)?)?,
                    self.dim(md(&l.dimensions.1, vars)?)?,
                );
                let ang = md(&l.angle, vars)?;
                let (cx, cy) = self.macro_offset(&l.center, ang, ax, ay, vars)?;
                (exp, geom::rect_rot(cx, cy, w, h, ang))
            }
            MacroContent::Outline(o) => {
                let exp = mbool(&o.exposure, vars)?;
                let ang = md(&o.angle, vars)?;
                let mut c: Contour = Vec::with_capacity(o.points.len());
                for (px, py) in &o.points {
                    let (lx, ly) = (self.dim(md(px, vars)?)?, self.dim(md(py, vars)?)?);
                    let (rx, ry) = geom::rotate(lx, ly, ang);
                    c.push(Pt::new(snap_nm(ax + rx)?, snap_nm(ay + ry)?));
                }
                // A CW dark pad would cancel against an overlapping CCW track under
                // the NonZero union (#13); every other primitive is already CCW.
                (exp, crate::geo::wind(c, true))
            }
            MacroContent::VectorLine(v) => {
                let exp = mbool(&v.exposure, vars)?;
                let ang = md(&v.angle, vars)?;
                let w = self.dim(md(&v.width, vars)?)?;
                let (sx, sy) = self.macro_rot_pt(&v.start, ang, ax, ay, vars)?;
                let (ex, ey) = self.macro_rot_pt(&v.end, ang, ax, ay, vars)?;
                (exp, geom::stadium(sx, sy, ex, ey, w / 2.0))
            }
            MacroContent::Polygon(p) => {
                let exp = mbool(&p.exposure, vars)?;
                let n = match p.vertices {
                    MacroInteger::Value(v) => v as usize,
                    MacroInteger::Variable(v) => macro_integer(macro_var(vars, v)?)?,
                    MacroInteger::Expression(ref e) => macro_integer(expression(e, vars)?)?,
                };
                if !(3..=64).contains(&n) {
                    return Err(unsupported("macro polygon vertex count out of range"));
                }
                let ang = md(&p.angle, vars)?;
                let (cx, cy) = self.macro_offset(&p.center, ang, ax, ay, vars)?;
                (
                    exp,
                    polygon_ngon(cx, cy, self.dim(md(&p.diameter, vars)?)? / 2.0, n, ang),
                )
            }
            MacroContent::Moire(_) => return Err(unsupported("macro moiré primitive")),
            MacroContent::Thermal(_) => return Err(unsupported("macro thermal primitive")),
        }))
    }

    /// Thermal primitive 7: an outer disc minus the inner disc and the two
    /// cross gaps, rotated about the macro origin. It has no exposure field.
    fn macro_thermal(
        &self,
        t: &gerber_parser::gerber_types::ThermalPrimitive,
        ax: f64,
        ay: f64,
        vars: &HashMap<u32, f64>,
    ) -> Result<Vec<Contour>> {
        let angle = md(&t.angle, vars)?;
        let (cx, cy) = self.macro_offset(&t.center, angle, ax, ay, vars)?;
        let outer = self.dim(md(&t.outer_diameter, vars)?)?;
        let inner = self.dim(md(&t.inner_diameter, vars)?)?;
        let gap = self.dim(md(&t.gap, vars)?)?;
        if outer <= inner || inner < 0.0 || gap <= 0.0 || gap >= outer / 2.0_f64.sqrt() {
            return Err(unsupported("invalid macro thermal dimensions"));
        }
        let mut cuts = vec![
            geom::rect_rot(cx, cy, outer * 2.0, gap, angle),
            geom::rect_rot(cx, cy, gap, outer * 2.0, angle),
        ];
        if inner > 0.0 {
            cuts.push(geom::ngon(cx, cy, inner / 2.0));
        }
        Ok(boolean::flatten(&boolean::difference(
            &[geom::ngon(cx, cy, outer / 2.0)],
            &cuts,
        )))
    }

    /// A macro-local centre `(x, y)` (doc units) rotated by `angle` about the macro
    /// origin and translated to the flash point — returns absolute nm.
    fn macro_offset(
        &self,
        center: &(MacroDecimal, MacroDecimal),
        angle: f64,
        ax: f64,
        ay: f64,
        vars: &HashMap<u32, f64>,
    ) -> Result<(f64, f64)> {
        let (lx, ly) = (
            self.dim(md(&center.0, vars)?)?,
            self.dim(md(&center.1, vars)?)?,
        );
        let (rx, ry) = geom::rotate(lx, ly, angle);
        // Guard the absolute centre (catches a non-finite rotation/offset too).
        snap_nm(ax + rx)?;
        snap_nm(ay + ry)?;
        Ok((ax + rx, ay + ry))
    }

    fn macro_pt(
        &self,
        center: &(MacroDecimal, MacroDecimal),
        angle: Option<&MacroDecimal>,
        ax: f64,
        ay: f64,
        vars: &HashMap<u32, f64>,
    ) -> Result<(f64, f64)> {
        let ang = match angle {
            Some(a) => md(a, vars)?,
            None => 0.0,
        };
        self.macro_offset(center, ang, ax, ay, vars)
    }

    fn macro_rot_pt(
        &self,
        point: &(MacroDecimal, MacroDecimal),
        angle: f64,
        ax: f64,
        ay: f64,
        vars: &HashMap<u32, f64>,
    ) -> Result<(f64, f64)> {
        self.macro_offset(point, angle, ax, ay, vars)
    }

    // --- helpers ---

    fn current_aperture(&self) -> Result<&'a GtAperture> {
        let code = self.ap.ok_or(EngineError::NoApertureSelected)?;
        self.apertures
            .get(&code)
            .ok_or(EngineError::UndefinedAperture { code })
    }

    /// A document-unit dimension → nm, **range-guarded** like [`quantize_mm`]: an
    /// absurd/out-of-range or non-finite value fails loud instead of silently
    /// saturating a later `f64 -> i64` cast. This is the chokepoint that keeps every
    /// aperture- and macro-derived coordinate bounded (a macro coord is a guarded
    /// flash point plus guarded dim-scaled offsets), so nothing downstream saturates.
    fn dim(&self, v: f64) -> Result<f64> {
        let nm = v * self.nm_per_unit;
        snap_nm(nm)?;
        Ok(nm)
    }

    /// Resolve modal coordinates → quantized nm point.
    fn resolve(&self, coords: &Option<Coordinates>) -> Result<Pt> {
        let Some(c) = coords else { return Ok(self.cur) };
        let x = match c.x {
            Some(v) => quantize_mm(f64::from(v) * self.unit_to_mm)?,
            None => self.cur.x,
        };
        let y = match c.y {
            Some(v) => quantize_mm(f64::from(v) * self.unit_to_mm)?,
            None => self.cur.y,
        };
        Ok(Pt::new(x, y))
    }

    /// Append a contour in paint order. Its effective polarity is dark iff
    /// `polarity_dark == exposure`; consecutive same-polarity contours extend the
    /// current span so they're resolved together (see [`Machine::spans`]).
    fn push(&mut self, c: Contour, exposure: bool) {
        if c.len() < 3 {
            return;
        }
        self.emitted += 1; // checked against the per-layer ceilings in step() (#83)
        self.total_points += c.len();
        let is_dark = self.polarity_dark == exposure;
        // NOTE: contours arrive correctly wound — our geom builders emit CCW solids,
        // and region fills come CCW-outer/CW-holes from fill_even_odd. Do NOT
        // re-wind here: forcing every contour to the polarity flips a region's holes
        // solid (filling pour clearances — the pour-render regression). The one
        // exception (a macro Outline, whose winding is the exporter's choice) is
        // normalized at its own call site, not here.
        match self.spans.last_mut() {
            Some((d, run)) if *d == is_dark => run.push(c),
            _ => self.spans.push((is_dark, vec![c])),
        }
    }
}

fn unsupported(feature: &str) -> EngineError {
    EngineError::Unsupported {
        feature: feature.to_string(),
    }
}

/// A regular `n`-gon (radius `r`) at `(cx, cy)`, rotated `deg` degrees — for the
/// standard polygon aperture (`P`) and the macro polygon primitive.
fn polygon_ngon(cx: f64, cy: f64, r: f64, n: usize, deg: f64) -> Contour {
    use std::f64::consts::PI;
    (0..n)
        .map(|k| {
            let a = deg * PI / 180.0 + 2.0 * PI * (k as f64) / n as f64;
            Pt::new(
                (cx + r * a.cos()).round() as i64,
                (cy + r * a.sin()).round() as i64,
            )
        })
        .collect()
}

/// A macro variable's value. A variable that was never given (no `%AD`
/// parameter, no `$n=` assignment) fails loud rather than reading as 0, so a
/// pad drawn with a missing size never renders as a quietly wrong shape.
fn macro_var(vars: &HashMap<u32, f64>, n: u32) -> Result<f64> {
    vars.get(&n)
        .copied()
        .ok_or_else(|| unsupported(&format!("undefined macro variable ${n}")))
}

/// Evaluate a macro decimal: a literal, a `$n` variable, or an arithmetic
/// expression over variables (RS-274X §4.5.4.3).
fn md(d: &MacroDecimal, vars: &HashMap<u32, f64>) -> Result<f64> {
    let v = match d {
        MacroDecimal::Value(v) => *v,
        MacroDecimal::Variable(n) => macro_var(vars, *n)?,
        MacroDecimal::Expression(e) => expression(e, vars)?,
    };
    if !v.is_finite() {
        return Err(unsupported("non-finite macro value"));
    }
    Ok(v)
}

fn mbool(b: &MacroBoolean, vars: &HashMap<u32, f64>) -> Result<bool> {
    let v = match b {
        MacroBoolean::Value(v) => return Ok(*v),
        MacroBoolean::Variable(n) => macro_var(vars, *n)?,
        MacroBoolean::Expression(e) => expression(e, vars)?,
    };
    match v {
        0.0 => Ok(false),
        1.0 => Ok(true),
        _ => Err(unsupported("macro exposure outside 0 or 1")),
    }
}

fn macro_integer(v: f64) -> Result<usize> {
    if v < 0.0 || v > u32::MAX as f64 || v.fract() != 0.0 {
        return Err(unsupported("invalid macro integer"));
    }
    Ok(v as usize)
}

/// Evaluate a macro arithmetic expression: numbers, `$n` variables, unary
/// `+`/`-`, parentheses, and `+ - x /` with `x` and `/` binding tighter than
/// `+` and `-` (`x` is multiply in the Gerber spec). Division by zero, an
/// undefined variable or a malformed expression fails loud.
fn expression(src: &str, vars: &HashMap<u32, f64>) -> Result<f64> {
    struct Parser<'a> {
        bytes: &'a [u8],
        pos: usize,
        vars: &'a HashMap<u32, f64>,
        depth: usize,
    }
    impl Parser<'_> {
        fn space(&mut self) {
            while self
                .bytes
                .get(self.pos)
                .is_some_and(|c| c.is_ascii_whitespace())
            {
                self.pos += 1;
            }
        }
        fn take(&mut self, c: u8) -> bool {
            self.space();
            if self.bytes.get(self.pos) == Some(&c) {
                self.pos += 1;
                true
            } else {
                false
            }
        }
        fn sum(&mut self) -> Result<f64> {
            let mut v = self.product()?;
            loop {
                v = if self.take(b'+') {
                    v + self.product()?
                } else if self.take(b'-') {
                    v - self.product()?
                } else {
                    break;
                };
            }
            Ok(v)
        }
        fn product(&mut self) -> Result<f64> {
            let mut v = self.atom()?;
            loop {
                v = if self.take(b'x') || self.take(b'X') {
                    v * self.atom()?
                } else if self.take(b'/') {
                    let divisor = self.atom()?;
                    if divisor == 0.0 {
                        return Err(unsupported("macro division by zero"));
                    }
                    v / divisor
                } else {
                    break;
                };
            }
            Ok(v)
        }
        fn atom(&mut self) -> Result<f64> {
            if self.depth >= 64 {
                return Err(unsupported("macro expression nesting limit"));
            }
            self.depth += 1;
            let result = self.atom_inner();
            self.depth -= 1;
            result
        }
        fn atom_inner(&mut self) -> Result<f64> {
            self.space();
            if self.take(b'+') {
                return self.atom();
            }
            if self.take(b'-') {
                return Ok(-self.atom()?);
            }
            if self.take(b'(') {
                let v = self.sum()?;
                if !self.take(b')') {
                    return Err(unsupported("unclosed macro expression"));
                }
                return Ok(v);
            }
            if self.take(b'$') {
                let start = self.pos;
                while self.bytes.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
                let n = std::str::from_utf8(&self.bytes[start..self.pos])
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| unsupported("invalid macro variable"))?;
                return macro_var(self.vars, n);
            }
            let start = self.pos;
            while self
                .bytes
                .get(self.pos)
                .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
            {
                self.pos += 1;
            }
            if start == self.pos {
                return Err(unsupported("invalid macro expression"));
            }
            std::str::from_utf8(&self.bytes[start..self.pos])
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .ok_or_else(|| unsupported("invalid macro number"))
        }
    }
    let mut p = Parser {
        bytes: src.as_bytes(),
        pos: 0,
        vars,
        depth: 0,
    };
    let v = p.sum()?;
    p.space();
    if p.pos != p.bytes.len() || !v.is_finite() {
        return Err(unsupported("invalid macro expression"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDR: &str = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";

    fn area_mm2(g: &str) -> f64 {
        resolve_layer(g.as_bytes()).unwrap().area_mm2()
    }

    #[test]
    fn out_of_range_dimension_fails_loud() {
        // An absurd aperture (1e9 mm dia → 1e15 nm, past the ±1e14 nm cap) must fail
        // loud rather than silently saturate the f64 -> i64 cast. Guards the whole
        // dim() chokepoint that aperture- and macro-derived coordinates flow through.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,1000000000*%\nD10*\nX0Y0D03*\nM02*\n";
        assert!(resolve_layer(g.as_bytes()).is_err());
    }

    #[test]
    fn parser_never_panics_on_garbage() {
        // The catch_unwind boundary (#85) must turn any parser panic into a typed
        // error: resolve_layer always returns for ANY bytes, never unwinds into the
        // caller. The assertion is simply that each call returns (the test would
        // abort if a panic escaped). The fuzz target covers the broader space.
        let inputs: &[&[u8]] = &[
            b"",
            b"\xff\xfe\x00\x01 not gerber at all",
            b"%FSLAX46Y46*%",                        // truncated header
            b"%FSLAX46Y46*%\n%MOMM*%\nG36*\nM02*\n", // region opened, never closed
            b"%MOMM*%\nX0Y0D03*\n",                  // flash, no format/aperture
            b"%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,*%\n", // malformed aperture
        ];
        for inp in inputs {
            let _ = resolve_layer(inp); // Ok or Err — must not panic.
        }
    }

    #[test]
    fn object_count_cap_fails_loud() {
        // A tiny file can still emit a huge object count (here, many flashes). Past
        // the per-layer ceiling the engine must fail loud, not grind or OOM (#83).
        // The cap is lowered under cfg(test), so a few thousand flashes trip it.
        // Rect flashes (4 pts each) keep the point total under MAX_POINTS_PER_LAYER
        // so the *contour* ceiling is what trips here, not the point ceiling.
        let mut g = String::from("%FSLAX46Y46*%\n%MOMM*%\n%ADD10R,0.1X0.1*%\nD10*\n");
        for i in 0..(MAX_CONTOURS_PER_LAYER + 1) {
            g.push_str(&format!("X{}Y0D03*\n", i * 1000));
        }
        g.push_str("M02*\n");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, count }) => {
                assert_eq!(what, "contours");
                assert_eq!(limit, MAX_CONTOURS_PER_LAYER);
                assert!(count > limit, "count {count} should exceed limit {limit}");
            }
            other => panic!("expected ObjectLimit, got {other:?}"),
        }
    }

    #[test]
    fn point_count_cap_fails_loud() {
        // Circle flashes are CIRCLE_SEGMENTS (64) points each, so a few hundred of
        // them cross the point ceiling while staying well under the contour ceiling
        // — exactly the "few contours, many points" vector the contour cap misses
        // (#83).
        let per = crate::geom::CIRCLE_SEGMENTS;
        let n = MAX_POINTS_PER_LAYER / per + 10;
        assert!(
            n < MAX_CONTOURS_PER_LAYER,
            "test must trip points, not contours"
        );
        let mut g = String::from("%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,1.0*%\nD10*\n");
        for i in 0..n {
            g.push_str(&format!("X{}Y0D03*\n", i * 3000));
        }
        g.push_str("M02*\n");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, count }) => {
                assert_eq!(what, "points");
                assert_eq!(limit, MAX_POINTS_PER_LAYER);
                assert!(count > limit, "count {count} should exceed limit {limit}");
            }
            other => panic!("expected ObjectLimit(points), got {other:?}"),
        }
    }

    fn expect_point_limit(g: &str) {
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, count }) => {
                assert_eq!(what, "points");
                assert_eq!(limit, MAX_POINTS_PER_LAYER);
                assert!(count > limit, "count {count} should exceed limit {limit}");
            }
            other => panic!("expected ObjectLimit(points), got {other:?}"),
        }
    }

    /// A region whose top edge zigzags through `n` vertices (none collinear, so
    /// the even-odd fill keeps them all), with its base at `y0` nm.
    fn zigzag_loop(g: &mut String, y0: usize, n: usize) {
        g.push_str(&format!("X0Y{y0}D02*\n"));
        for i in 0..n {
            let y = y0 + if i % 2 == 0 { 1_000_000 } else { 1_100_000 };
            g.push_str(&format!("X{}Y{y}D01*\n", i * 1000));
        }
        g.push_str(&format!("X{}Y{y0}D01*\nX0Y{y0}D01*\n", (n - 1) * 1000));
    }

    #[test]
    fn open_region_arcs_hit_point_cap_without_g37() {
        // #315: arc points buffered in a region that never closes used to bypass
        // the point ceiling (only `push` at `G37` counted them). Repeated full
        // circles (>= CIRCLE_SEGMENTS points each) must trip it mid-region.
        let n = MAX_POINTS_PER_LAYER / crate::geom::CIRCLE_SEGMENTS + 10;
        let mut g = String::from("%FSLAX46Y46*%\n%MOMM*%\nG75*\nG36*\nX1000000Y0D02*\nG03*\n");
        for _ in 0..n {
            g.push_str("X1000000Y0I-1000000J0D01*\n");
        }
        g.push_str("M02*\n"); // never closed
        expect_point_limit(&g);
    }

    #[test]
    fn open_region_loops_across_d02_restarts_hit_point_cap() {
        // #315: loops already moved into `region_loops` by a `D02` restart still
        // count, not just the loop under construction.
        let per = MAX_POINTS_PER_LAYER / 3;
        let mut g = String::from("%FSLAX46Y46*%\n%MOMM*%\nG01*\nG36*\n");
        for k in 0..4 {
            zigzag_loop(&mut g, k * 2_000_000, per);
        }
        g.push_str("M02*\n"); // never closed: only buffered points can trip the cap
        expect_point_limit(&g);
    }

    #[test]
    fn closed_region_points_are_not_double_counted() {
        // #315: buffered region points are released at `G37` and the filled result
        // is charged by `push` instead. Two disjoint loops totalling ~60% of the
        // ceiling must resolve: double-counting would put the layer at ~120%.
        let per = MAX_POINTS_PER_LAYER * 3 / 10;
        let mut g = String::from("%FSLAX46Y46*%\n%MOMM*%\nG01*\nG36*\n");
        zigzag_loop(&mut g, 0, per);
        zigzag_loop(&mut g, 2_000_000, per);
        g.push_str("G37*\nM02*\n");
        let set = resolve_layer(g.as_bytes()).expect("region under the ceiling resolves");
        let pts: usize = set.shapes.iter().flatten().map(|c| c.len()).sum();
        assert!(
            pts > 2 * per,
            "fill should keep the zigzag vertices ({pts})"
        );
    }

    #[test]
    fn span_count_cap_fails_loud() {
        // Toggling polarity on every flash makes each object its own span — an
        // O(N²) boolean grind the contour/point ceilings don't bound (#83).
        let mut g = String::from("%FSLAX46Y46*%\n%MOMM*%\n%ADD10R,0.1X0.1*%\nD10*\n");
        for i in 0..(MAX_SPANS_PER_LAYER + 100) {
            let lp = if i % 2 == 0 { "D" } else { "C" };
            g.push_str(&format!("%LP{lp}*%\nX{}Y0D03*\n", i * 1000));
        }
        g.push_str("M02*\n");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, count }) => {
                assert_eq!(what, "polarity spans");
                assert_eq!(limit, MAX_SPANS_PER_LAYER);
                assert!(count > limit, "count {count} should exceed limit {limit}");
            }
            other => panic!("expected ObjectLimit(polarity spans), got {other:?}"),
        }
    }

    #[test]
    fn flash_circle_area() {
        let g = format!("{HDR}X5000000Y5000000D03*\nM02*\n");
        let a = area_mm2(&g);
        let ideal = std::f64::consts::PI * 0.25 * 0.25;
        assert!(
            (a / ideal - 1.0).abs() < 0.01,
            "circle flash area {a} vs {ideal}"
        );
    }

    #[test]
    fn drilled_apertures_fail_loud() {
        // #240: an aperture with a hole (annular pad) must fail loud, not silently
        // report the solid-disk area — otherwise a copper-annulus change diffs as if
        // the whole pad were solid (a silent miss). Pin circle / rectangle / obround
        // and (#307) polygon.
        // Each is flashed once; the SAME aperture without the hole is checked to
        // resolve, so the failure is provably the hole, not a parse problem.
        let cases = [
            ("C,0.5X0.2", "C,0.5"),         // drilled circle vs solid circle
            ("R,0.6X0.4X0.2", "R,0.6X0.4"), // drilled rect vs solid rect
            ("O,0.6X0.4X0.2", "O,0.6X0.4"), // drilled obround vs solid obround
            ("P,1.0X6X0X0.4", "P,1.0X6"),   // drilled polygon vs solid polygon (#307)
        ];
        for (drilled, solid) in cases {
            let g = format!("%FSLAX46Y46*%\n%MOMM*%\n%ADD10{drilled}*%\nD10*\nX0Y0D03*\nM02*\n");
            let err = resolve_layer(g.as_bytes());
            assert!(
                matches!(err, Err(EngineError::Unsupported { .. })),
                "drilled aperture {drilled} must fail loud, got {err:?}"
            );
            let ok = format!("%FSLAX46Y46*%\n%MOMM*%\n%ADD10{solid}*%\nD10*\nX0Y0D03*\nM02*\n");
            assert!(
                resolve_layer(ok.as_bytes()).is_ok(),
                "the same aperture without a hole ({solid}) must resolve"
            );
        }
    }

    #[test]
    fn deprecated_g71_is_normalized_away() {
        // Altium emits G71 alongside %MO; it must not break parsing.
        let g = "%FSLAX46Y46*%\n%MOMM*%\nG71*\n%ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n";
        assert!(resolve_layer(g.as_bytes()).is_ok());
    }

    #[test]
    fn compact_line_is_split_not_dropped() {
        // Three flashes packed on one line must all be rendered (3 disjoint pads).
        let g = format!("{HDR}X0Y0D03*X2000000Y0D03*X4000000Y0D03*\nM02*\n");
        let ps = resolve_layer(g.as_bytes()).unwrap();
        assert_eq!(
            ps.region_count(),
            3,
            "all three compact-line flashes must render"
        );
    }

    #[test]
    fn unterminated_command_before_extended_block_fails_loud() {
        // A flash missing its `*` right before a `%LP` block used to be silently
        // discarded (#313): the layer resolved Ok with the pad gone. Must fail loud.
        let g = format!("{HDR}X0Y0D03%LPC*%\nM02*\n");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::Parse(msg)) => {
                assert!(
                    msg.contains("unterminated") && msg.contains("X0Y0D03"),
                    "{msg}"
                );
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_command_at_eof_fails_loud() {
        // Same silent drop at end of file: a final flash with no `*` (#313).
        let g = format!("{HDR}X0Y0D03*\nX5Y5D03");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::Parse(msg)) => {
                assert!(
                    msg.contains("unterminated") && msg.contains("X5Y5D03"),
                    "{msg}"
                );
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn unclosed_extended_block_at_eof_fails_loud() {
        // An extended block opened with `%` and never closed was dropped too (#313).
        let g = format!("{HDR}X0Y0D03*\n%LPC*");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::Parse(msg)) => {
                assert!(
                    msg.contains("unterminated") && msg.contains("%LPC"),
                    "{msg}"
                );
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn trailing_whitespace_after_m02_still_resolves() {
        // Blank lines, spaces and tabs after the final `M02*` are not a command (#313).
        let g = format!("{HDR}X0Y0D03*\nM02*\n\n  \n\t\n   ");
        let ps = resolve_layer(g.as_bytes()).expect("trailing whitespace must be fine");
        assert_eq!(ps.region_count(), 1);
    }

    #[test]
    fn crlf_line_endings_still_resolve() {
        // Windows exports: CRLF between every command, including after `M02*` (#313).
        let g = "%FSLAX46Y46*%\r\n%MOMM*%\r\n%ADD10C,0.5*%\r\nD10*\r\nX0Y0D03*\r\nM02*\r\n";
        let ps = resolve_layer(g.as_bytes()).expect("CRLF file must resolve");
        assert_eq!(ps.region_count(), 1);
    }

    #[test]
    fn stroked_line_has_expected_area() {
        // 1 mm draw with a 0.2 mm circle aperture: 0.2×1.0 + π·0.1² mm².
        let g =
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.2*%\nD10*\nX0Y0D02*\nG01*\nX1000000Y0D01*\nM02*\n";
        let a = area_mm2(g);
        let ideal = 0.2 * 1.0 + std::f64::consts::PI * 0.1 * 0.1;
        assert!((a / ideal - 1.0).abs() < 0.02, "stroke area {a} vs {ideal}");
    }

    #[test]
    fn cw_macro_outline_pad_unions_solid_with_track() {
        // A custom pad defined as a macro Outline wound CLOCKWISE (exporters are free
        // to), 1×1 mm, with a 0.2 mm track drawn straight through it. The dark pad and
        // dark track overlap; under a NonZero union the CW pad (winding −1) and CCW
        // track (winding +1) cancel to 0 in the overlap and punch a hole — the notch
        // at a track→pad junction (#13). After winding-normalization the union is one
        // solid region with NO hole.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n\
                 %AMSQ*\n4,1,4,-0.5,-0.5,-0.5,0.5,0.5,0.5,0.5,-0.5,-0.5,-0.5,0*%\n\
                 %ADD10SQ*%\n%ADD11C,0.2*%\n\
                 D10*\nX0Y0D03*\n\
                 D11*\nX-2000000Y0D02*\nG01*\nX2000000Y0D01*\n\
                 M02*\n";
        let ps = resolve_layer(g.as_bytes()).unwrap();
        // One connected solid (pad + track), and crucially NO holes (no notch).
        assert_eq!(
            ps.shapes.len(),
            1,
            "pad+track should be one connected region"
        );
        assert_eq!(
            ps.shapes[0].len(),
            1,
            "track→pad junction must be solid copper, not a notched hole"
        );
        // Sanity: area exceeds the 1 mm² pad alone (the track adds copper).
        assert!(ps.area_mm2() > 1.2, "area {} too small", ps.area_mm2());
    }

    // ---- #306: macro exposure-off cuts the aperture image, not the layer ----

    /// Area of a strip `|y| <= half` clipped to a disk of radius `r` (both mm).
    fn strip_in_disk(r: f64, half: f64) -> f64 {
        let seg = |y: f64| y * (r * r - y * y).sqrt() + r * r * (y / r).asin();
        seg(half) - seg(-half)
    }

    /// `RING`: a 2.0 mm disk with a 1.0 mm exposure-off disk cut out of it.
    const RING: &str = "%AMRING*\n1,1,2.0,0,0*\n1,0,1.0,0,0*%\n";

    #[test]
    fn macro_exposure_off_leaves_copper_under_the_hole() {
        // #306: a 0.3 mm track through the origin, then RING flashed at the origin.
        // Exposure-off erases from the MACRO image only, and the hole is transparent
        // when flashed, so the track must run unbroken through the ring's hole.
        // Copper = annulus + track − (track ∩ annulus). The old layer-wide clear
        // also severed the track inside r < 0.5 (−0.295 mm²).
        let g = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n{RING}%ADD10RING*%\n%ADD11C,0.3*%\n\
             D11*\nX-3000000Y0D02*\nG01*\nX3000000Y0D01*\n\
             D10*\nX0Y0D03*\nM02*\n"
        );
        let ps = resolve_layer(g.as_bytes()).unwrap();
        let annulus = std::f64::consts::PI * (1.0 - 0.25);
        // A D01 stroke has round end caps: 6 mm × 0.3 mm plus a 0.15 mm-radius disk.
        let track = 0.3 * 6.0 + std::f64::consts::PI * 0.15 * 0.15;
        let overlap = strip_in_disk(1.0, 0.15) - strip_in_disk(0.5, 0.15);
        let ideal = annulus + track - overlap;
        let a = ps.area_mm2();
        assert!((a - ideal).abs() < 0.02, "area {a} vs {ideal}");
        // One connected region, and the hole is split by the track into two
        // half-moon holes rather than one severed track.
        assert_eq!(ps.shapes.len(), 1, "ring + track are one connected region");
        assert_eq!(
            ps.shapes[0].len(),
            3,
            "two half-moon holes either side of the track"
        );
    }

    #[test]
    fn macro_exposure_off_draw_order_independent() {
        // The same geometry with the flash BEFORE the track must give the same
        // copper: the fix removes the draw-order dependence, not just one ordering.
        let hdr = format!("%FSLAX46Y46*%\n%MOMM*%\n{RING}%ADD10RING*%\n%ADD11C,0.3*%\n");
        let track = "D11*\nX-3000000Y0D02*\nG01*\nX3000000Y0D01*\n";
        let flash = "D10*\nX0Y0D03*\n";
        let a = area_mm2(&format!("{hdr}{track}{flash}M02*\n"));
        let b = area_mm2(&format!("{hdr}{flash}{track}M02*\n"));
        assert!(
            (a - b).abs() < 1e-6,
            "flash-then-track {b} vs track-then-flash {a}"
        );
    }

    #[test]
    fn macro_exposure_off_in_clear_polarity_adds_no_copper() {
        // In %LPC the whole aperture image is a clearance and its hole is still
        // transparent: flashing RING far from any copper must change nothing. The
        // old code turned the exposure-off disk into a DARK span (clear × off =
        // dark) and conjured a 0.785 mm² disk of copper out of nothing.
        let g = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n{RING}%ADD10RING*%\n%ADD11R,10X10*%\n\
             D11*\nX5000000Y5000000D03*\n\
             %LPC*%\nD10*\nX20000000Y20000000D03*\n%LPD*%\nM02*\n"
        );
        let a = area_mm2(&g);
        assert!(
            (a - 100.0).abs() < 1e-6,
            "area {a} vs the 100 mm² pad alone"
        );
    }

    #[test]
    fn macro_exposure_off_ring_alone_is_an_annulus_with_a_hole() {
        let g = format!("%FSLAX46Y46*%\n%MOMM*%\n{RING}%ADD10RING*%\nD10*\nX0Y0D03*\nM02*\n");
        let ps = resolve_layer(g.as_bytes()).unwrap();
        let ideal = std::f64::consts::PI * (1.0 - 0.25);
        let a = ps.area_mm2();
        assert!(
            (a / ideal - 1.0).abs() < 0.01,
            "annulus area {a} vs {ideal}"
        );
        assert_eq!(ps.shapes.len(), 1);
        assert_eq!(ps.shapes[0].len(), 2, "outer ring + one hole");
    }

    #[test]
    fn mixed_exposure_macro_is_bounded_by_the_span_ceiling() {
        // #83/#306: a macro alternating exposure on/off per primitive costs one
        // boolean pass per primitive inside a single flash. That must be charged
        // to the span ceiling as it runs — not computed to completion — so a
        // crafted file fails loud in bounded time. The cap is lowered under
        // cfg(test); each on-square is disjoint so the image keeps growing. Squares
        // (4 points) keep the point and contour totals under their own ceilings so
        // the pass ceiling is provably what trips.
        let n = MAX_SPANS_PER_LAYER + 10;
        assert!(n < MAX_CONTOURS_PER_LAYER && n * 4 < MAX_POINTS_PER_LAYER);
        let mut am = String::from("%AMBIG*\n");
        for k in 0..n {
            let exp = if k % 2 == 0 { 1 } else { 0 };
            am.push_str(&format!("21,{exp},0.010,0.010,{},0,0*\n", k as f64 * 0.02));
        }
        am.push_str("%\n");
        let g = format!("%FSLAX46Y46*%\n%MOMM*%\n{am}%ADD10BIG*%\nD10*\nX0Y0D03*\nM02*\n");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::ObjectLimit { what, limit, count }) => {
                assert_eq!(what, "polarity spans");
                assert_eq!(limit, MAX_SPANS_PER_LAYER);
                assert!(count > limit);
            }
            other => panic!("expected ObjectLimit(polarity spans), got {other:?}"),
        }
    }

    #[test]
    fn mixed_exposure_macro_primitives_count_toward_the_contour_ceiling() {
        // The primitives of a resolved macro image never reach `push`, so they are
        // charged to the contour ceiling directly: flashing a 3-primitive mixed
        // macro more than MAX_CONTOURS_PER_LAYER / 3 times must trip it even though
        // each flash emits only two contours.
        let flashes = MAX_CONTOURS_PER_LAYER / 3 + 10;
        let mut g = String::from(
            "%FSLAX46Y46*%\n%MOMM*%\n%AMR3*\n1,1,0.2,0,0*\n1,0,0.1,0,0*\n1,1,0.05,0,0*%\n\
             %ADD10R3*%\nD10*\n",
        );
        for i in 0..flashes {
            g.push_str(&format!("X{}Y0D03*\n", i * 300_000));
        }
        g.push_str("M02*\n");
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::ObjectLimit { what, .. }) => {
                assert!(what == "contours" || what == "points", "got {what}");
            }
            other => panic!("expected ObjectLimit, got {other:?}"),
        }
    }

    #[test]
    fn clear_polarity_cw_outline_still_clears_where_it_overlaps_a_clear_circle() {
        // A macro Outline is now always wound CCW like every other primitive. In
        // %LPC the old polarity-dependent CW winding made an outline overlapping a
        // CCW clear circle sum to winding 0 in the overlap, so that overlap was
        // NOT cleared. Both clearances must land: 10×10 pad minus (1×1 square ∪
        // circle d=1 centred on its corner).
        let g = "%FSLAX46Y46*%\n%MOMM*%\n\
                 %AMSQ*\n4,1,4,-0.5,-0.5,-0.5,0.5,0.5,0.5,0.5,-0.5,-0.5,-0.5,0*%\n\
                 %ADD10R,10X10*%\n%ADD11SQ*%\n%ADD12C,1.0*%\n\
                 D10*\nX5000000Y5000000D03*\n\
                 %LPC*%\nD11*\nX5000000Y5000000D03*\nD12*\nX5500000Y5500000D03*\n%LPD*%\nM02*\n";
        let a = area_mm2(g);
        // Square ∪ circle: 1 + π/4 − (quarter of the circle inside the square) =
        // 1 + π/4 − π/16.
        let pi = std::f64::consts::PI;
        let cleared = 1.0 + pi / 4.0 - pi / 16.0;
        let ideal = 100.0 - cleared;
        assert!((a - ideal).abs() < 0.02, "area {a} vs {ideal}");
    }

    // ---- #90: aperture-macro primitive geometry (area/shape + rotation/offset) ----
    // flash_macro handles Circle/CenterLine/VectorLine/Outline/Polygon with offset
    // and rotation; only Outline had a test. These pin each primitive's geometry —
    // all common on real fab packs and silent if the maths is wrong.

    #[test]
    fn macro_circle_primitive_area_is_offset_invariant() {
        // Circle (code 1): dia 0.5 mm at macro-local offset (0.3, 0.3); area = π·0.25²
        // wherever the local centre sits.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMCIR*\n1,1,0.5,0.3,0.3*%\n%ADD10CIR*%\nD10*\nX0Y0D03*\nM02*\n";
        let a = area_mm2(g);
        let ideal = std::f64::consts::PI * 0.25 * 0.25;
        assert!(
            (a / ideal - 1.0).abs() < 0.02,
            "macro circle area {a} vs {ideal}"
        );
    }

    #[test]
    fn macro_centerline_rect_area_is_rotation_invariant() {
        // CenterLine (code 21): 1.0×0.5 mm rect rotated 30°. Area = 0.5 mm² regardless.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMCL*\n21,1,1.0,0.5,0,0,30*%\n%ADD10CL*%\nD10*\nX0Y0D03*\nM02*\n";
        let a = area_mm2(g);
        assert!((a - 0.5).abs() < 0.01, "macro centerline area {a} vs 0.5");
    }

    #[test]
    fn macro_vectorline_stroke_area() {
        // VectorLine (code 20): width 0.2 mm from (0,0) to (1,0). Stadium stroke
        // area = 0.2·1.0 + π·0.1².
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMVL*\n20,1,0.2,0,0,1.0,0,0*%\n%ADD10VL*%\nD10*\nX0Y0D03*\nM02*\n";
        let a = area_mm2(g);
        let ideal = 0.2 * 1.0 + std::f64::consts::PI * 0.1 * 0.1;
        assert!(
            (a / ideal - 1.0).abs() < 0.03,
            "macro vectorline area {a} vs {ideal}"
        );
    }

    #[test]
    fn macro_polygon_hexagon_area() {
        // Polygon (code 5): regular 6-gon, centre (0,0), diameter 1.0 mm
        // (circumradius 0.5). Area = (3√3/2)·R². Exercises polygon_ngon.
        let g =
            "%FSLAX46Y46*%\n%MOMM*%\n%AMPG*\n5,1,6,0,0,1.0,0*%\n%ADD10PG*%\nD10*\nX0Y0D03*\nM02*\n";
        let a = area_mm2(g);
        let r = 0.5;
        let ideal = 1.5 * 3.0_f64.sqrt() * r * r;
        assert!(
            (a / ideal - 1.0).abs() < 0.02,
            "macro polygon area {a} vs {ideal}"
        );
    }

    #[test]
    fn region_fills_its_outline() {
        // A 2×2 mm square region → 4 mm².
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\nX0Y0D02*\nG01*\nX2000000Y0D01*\nX2000000Y2000000D01*\nX0Y2000000D01*\nX0Y0D01*\nG37*\nM02*\n";
        let a = area_mm2(g);
        assert!((a - 4.0).abs() < 0.001, "region area {a} vs 4.0");
    }

    #[test]
    fn region_without_leading_d02_keeps_its_start_vertex() {
        // #314: the contour starts at the current point. `X0Y0D02*` BEFORE `G36*`
        // then two `D01`s is a right triangle (0,0)-(2,0)-(2,2) = 2 mm². Without
        // the start vertex the loop was 2 points, dropped by the `len < 3` guard,
        // and the region vanished with no error — a silent miss.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D02*\nG36*\nX2000000Y0D01*\nX2000000Y2000000D01*\nG37*\nM02*\n";
        let a = area_mm2(g);
        assert!((a - 2.0).abs() < 0.001, "triangle region area {a} vs 2.0");
    }

    #[test]
    fn region_with_leading_d02_is_unchanged_by_the_seed() {
        // #314 guard: a region that opens with its own `D02` must render exactly as
        // before — the seed only fires when no `D02` has started the loop. Same
        // 2×2 square as `region_fills_its_outline`, with and without a stale
        // current point set before `G36*`; both must be 4 mm² and identical.
        let with_d02 = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\nX0Y0D02*\nG01*\nX2000000Y0D01*\nX2000000Y2000000D01*\nX0Y2000000D01*\nX0Y0D01*\nG37*\nM02*\n";
        let stale_cur = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX9000000Y9000000D02*\nG36*\nX0Y0D02*\nG01*\nX2000000Y0D01*\nX2000000Y2000000D01*\nX0Y2000000D01*\nX0Y0D01*\nG37*\nM02*\n";
        let a = area_mm2(with_d02);
        let b = area_mm2(stale_cur);
        assert!((a - 4.0).abs() < 0.001, "region area {a} vs 4.0");
        assert!(
            (a - b).abs() < 1e-9,
            "a stale current point before G36 must not leak into the region: {a} vs {b}"
        );
    }

    #[test]
    fn nested_g36_fails_loud() {
        // #314: a second `G36` while a region is open used to reset the collected
        // loops silently. It is a malformed file — fail loud.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\nX0Y0D02*\nX2000000Y0D01*\nX2000000Y2000000D01*\nX0Y0D01*\nG36*\nX5000000Y5000000D02*\nX6000000Y5000000D01*\nX6000000Y6000000D01*\nX5000000Y5000000D01*\nG37*\nM02*\n";
        let err = resolve_layer(g.as_bytes());
        assert!(
            matches!(err, Err(EngineError::Unsupported { .. })),
            "nested G36 must fail loud, got {err:?}"
        );
    }

    #[test]
    fn g37_outside_a_region_fails_loud() {
        // #314: `G37` with no open region used to be a silent no-op.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nG37*\nM02*\n";
        let err = resolve_layer(g.as_bytes());
        assert!(
            matches!(err, Err(EngineError::Unsupported { .. })),
            "stray G37 must fail loud, got {err:?}"
        );
    }

    #[test]
    fn region_with_inner_loop_keeps_the_hole() {
        // A 4×4 mm filled region with a 2×2 mm inner loop (a pour clearance) → the
        // even-odd fill makes the inner loop a hole: 16 − 4 = 12 mm². Regression guard
        // for the pour-render bug: winding normalization must NOT flip a region's hole
        // solid (which filled every pour clearance → solid copper). See #13 follow-up.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\n\
                 X0Y0D02*\nG01*\nX4000000Y0D01*\nX4000000Y4000000D01*\nX0Y4000000D01*\nX0Y0D01*\n\
                 X1000000Y1000000D02*\nX3000000Y1000000D01*\nX3000000Y3000000D01*\nX1000000Y3000000D01*\nX1000000Y1000000D01*\n\
                 G37*\nM02*\n";
        let a = area_mm2(g);
        assert!(
            (a - 12.0).abs() < 0.01,
            "region-with-hole area {a}, expected 12 (4×4 minus 2×2 hole) — hole was filled solid?"
        );
    }

    #[test]
    fn polarity_clear_subtracts() {
        // Big 4 mm² square pad, then a clear 2×2 region punches a 4-... hole.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10R,2X2*%\nD10*\nX1000000Y1000000D03*\n%LPC*%\nG36*\nX1000000Y1000000D02*\nG01*\nX2000000Y1000000D01*\nX2000000Y2000000D01*\nX1000000Y2000000D01*\nX1000000Y1000000D01*\nG37*\n%LPD*%\nM02*\n";
        let a = area_mm2(g);
        // 2×2 pad (4) minus a 1×1 clear (1) = 3 mm².
        assert!((a - 3.0).abs() < 0.01, "after clear area {a} vs 3.0");
    }

    #[test]
    fn polarity_is_sequential_later_dark_repaints() {
        // Gerber polarity paints in ORDER: dark pad → clear punches a hole → a
        // later dark pad inside the hole must REPAINT (survive). A single
        // all-dark − all-clear pass erases that later pad (it's lumped into "dark"
        // and subtracted by the clear) — the real-board "trace-shaped voids" bug.
        // 10×10 (100) − 4×4 clear (16) + 2×2 dark inside (4) = 88 mm².
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10R,10X10*%\n%ADD11R,4X4*%\n%ADD12R,2X2*%\n\
                 D10*\nX5000000Y5000000D03*\n\
                 %LPC*%\nD11*\nX5000000Y5000000D03*\n\
                 %LPD*%\nD12*\nX5000000Y5000000D03*\nM02*\n";
        let a = area_mm2(g);
        assert!(
            (a - 88.0).abs() < 0.05,
            "sequential polarity area {a} vs 88.0"
        );
    }

    #[test]
    fn incremental_format_spec_fails_loud() {
        // #305: `%FSLIX24Y24*%` declares incremental notation (the 'I' after the
        // zero-omission letter). The parser stored the mode but the engine resolved
        // every coordinate as absolute — wrong geometry, no error. The identical
        // file with `%FSLAX24Y24*%` must still resolve, so the failure is provably
        // the mode, not the rest of the file.
        let body = "%MOMM*%\n%ADD10C,0.5*%\nD10*\nX1000Y1000D03*\nX1000Y0D03*\nM02*\n";
        let inc = format!("%FSLIX24Y24*%\n{body}");
        let err = resolve_layer(inc.as_bytes());
        match err {
            Err(EngineError::Unsupported { feature }) => assert!(
                feature.contains("incremental coordinate notation"),
                "unexpected feature text: {feature}"
            ),
            other => panic!("incremental %FS must fail loud, got {other:?}"),
        }
        let abs = format!("%FSLAX24Y24*%\n{body}");
        assert!(
            resolve_layer(abs.as_bytes()).is_ok(),
            "the same file in absolute notation must resolve"
        );
    }

    #[test]
    fn g74_single_quadrant_arc_honoured() {
        // #234: a quarter-disk region — centre (0,0) → out to (1mm,0) → G74 90° CCW
        // arc back up to (0,1mm) → return to centre. Area = π r²/4 ≈ 0.7854 mm².
        // In single-quadrant the I/J are UNSIGNED (I=1mm, J=0); the pre-fix code
        // ignored G74 and read them as signed multi-quadrant offsets, putting the
        // centre at (2mm,0) and sweeping a garbage ~333° arc — wrong, silent copper.
        let r = 1_000_000; // 1.000000 mm at %FSLAX46
        let g = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG74*\nG36*\n\
             X0Y0D02*\nG01*\nX{r}Y0D01*\nG03*\nX0Y{r}I{r}J0D01*\nG01*\nX0Y0D01*\nG37*\nM02*\n"
        );
        let a = area_mm2(&g);
        let ideal = std::f64::consts::PI / 4.0;
        assert!(
            (a - ideal).abs() < 0.02,
            "G74 quarter-disk area {a} vs {ideal}"
        );
    }

    #[test]
    fn g74_arc_differs_from_g75_for_same_tokens() {
        // The same arc tokens must render DIFFERENTLY under G74 vs G75 — proof the
        // quadrant mode is honoured, not discarded. With I=1mm J=0 the multi-quadrant
        // (G75) reading centres the arc at (2mm,0), the single-quadrant (G74) reading
        // at (0,0); their pie-slice areas can't coincide.
        let r = 1_000_000;
        let body = format!(
            "%MOMM*%\n%ADD10C,0.1*%\nD10*\n{{mode}}\nG36*\n\
             X0Y0D02*\nG01*\nX{r}Y0D01*\nG03*\nX0Y{r}I{r}J0D01*\nG01*\nX0Y0D01*\nG37*\nM02*\n"
        );
        let g74 = format!("%FSLAX46Y46*%\n{}", body.replace("{mode}", "G74*"));
        let g75 = format!("%FSLAX46Y46*%\n{}", body.replace("{mode}", "G75*"));
        let a74 = area_mm2(&g74);
        let a75 = area_mm2(&g75);
        assert!(
            (a74 - a75).abs() > 0.1,
            "G74 area {a74} and G75 area {a75} must differ (mode was ignored?)"
        );
    }

    #[test]
    fn g74_coincident_arc_fails_loud() {
        // #321: a G74 arc whose end equals its start is a 0° or 360° sweep — neither
        // fits the ≤90° mode. Pre-fix it was accepted (sweep 0 ≤ max) and then
        // `arc_points` turned 0 into 2π, silently adding a full ring of copper.
        let r = 1_000_000;
        let stroked = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG74*\n\
             X{r}Y0D02*\nG03*\nX{r}Y0I{r}J0D01*\nM02*\n"
        );
        match resolve_layer(stroked.as_bytes()) {
            Err(EngineError::Unsupported { feature }) => assert!(
                feature.contains("coincident start and end"),
                "message must name the coincident endpoints: {feature}"
            ),
            other => panic!("coincident G74 stroked arc must fail loud, got {other:?}"),
        }
        // The same inside a region (G36/G37) takes the region path and must too.
        let region = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG74*\nG36*\n\
             X{r}Y0D02*\nG02*\nX{r}Y0I{r}J0D01*\nG37*\nM02*\n"
        );
        assert!(
            matches!(
                resolve_layer(region.as_bytes()),
                Err(EngineError::Unsupported { feature }) if feature.contains("coincident")
            ),
            "coincident G74 region arc must fail loud"
        );
    }

    #[test]
    fn g75_coincident_arc_is_full_circle() {
        // #321 guard: under G75 a coincident start/end is a legitimate 360° arc.
        let r = 1_000_000;
        let region = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG75*\nG36*\n\
             X{r}Y0D02*\nG03*\nX{r}Y0I-{r}J0D01*\nG37*\nM02*\n"
        );
        let a = area_mm2(&region);
        let disk = std::f64::consts::PI;
        assert!(
            (a - disk).abs() < 0.02,
            "G75 full-circle region {a} vs {disk}"
        );
        let stroked = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG75*\n\
             X{r}Y0D02*\nG03*\nX{r}Y0I-{r}J0D01*\nM02*\n"
        );
        // 0.1 mm wide ring of mean radius 1 mm: 2π·1·0.1 ≈ 0.628 mm².
        let ring = area_mm2(&stroked);
        assert!(
            (ring - 0.2 * disk).abs() < 0.02,
            "G75 full-circle ring {ring}"
        );
    }

    #[test]
    fn arc_before_any_quadrant_mode_fails_loud() {
        // #312: the g74_single_quadrant_arc_honoured input with its `G74*` removed.
        // With no G74/G75 the I/J reading is ambiguous (legacy default single,
        // modern files always declare G75); the pre-fix code silently assumed
        // multi-quadrant and put the centre at (2mm,0). It must fail loud instead.
        let r = 1_000_000;
        let g = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\n\
             X0Y0D02*\nG01*\nX{r}Y0D01*\nG03*\nX0Y{r}I{r}J0D01*\nG01*\nX0Y0D01*\nG37*\nM02*\n"
        );
        match resolve_layer(g.as_bytes()) {
            Err(EngineError::Unsupported { feature }) => assert!(
                feature.contains("arc before any G74/G75 quadrant mode declaration")
                    && feature.contains("add G75"),
                "message must name the missing declaration and the fix: {feature}"
            ),
            other => panic!("undeclared-mode arc must fail loud, got {other:?}"),
        }
        // A stroked (non-region) arc takes the same path and must fail the same way.
        let stroked = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\n\
             X{r}Y0D02*\nG03*\nX0Y{r}I{r}J0D01*\nM02*\n"
        );
        assert!(
            matches!(
                resolve_layer(stroked.as_bytes()),
                Err(EngineError::Unsupported { .. })
            ),
            "undeclared-mode stroked arc must fail loud"
        );
    }

    #[test]
    fn arc_after_g75_declaration_resolves() {
        // #312: the same input with `G75*` declared where the `G74*` was resolves,
        // and keeps the multi-quadrant reading (centre at (2mm,0), not (0,0)).
        let r = 1_000_000;
        let g = format!(
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG75*\nG36*\n\
             X0Y0D02*\nG01*\nX{r}Y0D01*\nG03*\nX0Y{r}I{r}J0D01*\nG01*\nX0Y0D01*\nG37*\nM02*\n"
        );
        let a = area_mm2(&g);
        let quarter_disk = std::f64::consts::PI / 4.0;
        assert!(a > 0.0, "G75 arc must resolve to copper, got {a}");
        assert!(
            (a - quarter_disk).abs() > 0.1,
            "G75 must keep the multi-quadrant centre, not the G74 one ({a})"
        );
    }

    #[test]
    fn linear_only_file_needs_no_quadrant_mode() {
        // #312: a file with no G74/G75 and no arcs must be unaffected — the mode
        // is only consulted when an arc is actually drawn.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n\
                 X0Y0D02*\nG01*\nX2000000Y0D01*\nX2000000Y1000000D03*\nM02*\n";
        let a = area_mm2(g);
        // 2 mm × 0.5 mm stroke with round caps (2×0.5 + π·0.25² ≈ 1.196) plus a
        // separate 0.5 mm pad (≈ 0.196) 1 mm away from the stroke.
        assert!(a > 1.0, "linear-only file must resolve to copper, got {a}");
    }

    #[test]
    fn gerber_format_parses_units_and_digits() {
        let inch = gerber_format(b"%FSLAX25Y25*%\n%MOIN*%\nM02*\n").unwrap();
        assert_eq!(
            (inch.units, inch.int_digits, inch.dec_digits),
            (Units::Inches, 2, 5)
        );
        let mm = gerber_format(b"%MOMM*%\n%FSLAX44Y44*%\nM02*\n").unwrap();
        assert_eq!(
            (mm.units, mm.int_digits, mm.dec_digits),
            (Units::Millimeters, 4, 4)
        );
    }

    #[test]
    fn coordinate_mismatch_warns_only_on_difference() {
        // The real-board case: rev A inches@2.5 vs rev B mm@4.4.
        let a = GerberFormat {
            units: Units::Inches,
            int_digits: 2,
            dec_digits: 5,
        };
        let b = GerberFormat {
            units: Units::Millimeters,
            int_digits: 4,
            dec_digits: 4,
        };
        assert!(coordinate_mismatch_warning(&a, &a).is_none());
        let w = coordinate_mismatch_warning(&a, &b).expect("mismatch must warn");
        assert!(w.contains("in@2.5") && w.contains("mm@4.4"), "warning: {w}");
    }

    #[test]
    fn unsupported_extended_fails_loud() {
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%SRX2Y1I5J0*%\n%ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n";
        assert!(matches!(
            resolve_layer(g.as_bytes()),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn identity_extended_codes_preserve_flash() {
        let codes = [
            "%IPPOS*%",
            "%OFA0B0*%",
            "%OFA0*%",
            "%OFB0*%",
            "%OF*%",
            "%SFA1B1*%",
            "%SFA1*%",
            "%SFB1*%",
            "%SF*%",
            "%MIA0B0*%",
            "%MIA0*%",
            "%MIB0*%",
            "%MI*%",
            "%ASAXBY*%",
            "%SRX1Y1I0J0*%",
            "%SRX1Y1*%",
            "%SRX1Y1I0*%",
        ];
        let ideal = std::f64::consts::PI * 0.25 * 0.25;
        for code in codes {
            let g =
                format!("%FSLAX46Y46*%\n%MOMM*%\n{code}\n%ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n");
            assert!((area_mm2(&g) - ideal).abs() < 0.01, "{code}");
        }
    }

    #[test]
    fn nonidentity_extended_codes_fail_loud() {
        for code in [
            "%IPNEG*%",
            "%OFA1B0*%",
            "%SFA1B2*%",
            "%MIA1B0*%",
            "%ASAYBX*%",
            "%SRX2Y1I0J0*%",
        ] {
            let g =
                format!("%FSLAX46Y46*%\n%MOMM*%\n{code}\n%ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n");
            assert!(resolve_layer(g.as_bytes()).is_err(), "{code}");
        }
    }

    #[test]
    fn layer_name_is_ignored() {
        let g = format!("{HDR}%LNTop Copper*%\nX0Y0D03*\nM02*\n");
        assert!(area_mm2(&g) > 0.19);
    }

    #[test]
    fn kicad_roundrect_macro_area() {
        let g = include_str!("../../../corpus/gerber/roundrect.gbr");
        let area = area_mm2(g);
        let r = 0.1143_f64;
        let ideal = 0.3302 * 0.2286 + 2.0 * (0.3302 + 0.2286) * r + std::f64::consts::PI * r * r;
        assert!((area - ideal).abs() < 0.005, "area {area} vs {ideal}");
    }

    #[test]
    fn macro_assignments_and_arithmetic() {
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMCALC*\n$2=($1+1)x2/2-1*\n21,1,$2,$1,0,0,0*%\n%ADD10CALC,0.5*%\nD10*\nX0Y0D03*\nM02*\n";
        assert!((area_mm2(g) - 0.25).abs() < 0.001);
        for expr in ["$1/0", "$0+1", "1+", "(1+2"] {
            assert!(expression(expr, &HashMap::new()).is_err(), "{expr}");
        }
    }

    #[test]
    fn parameterised_clear_circle_cuts_macro_only() {
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMRING*\n1,1,$1,0,0*\n1,0,$1/2,0,0*%\n%ADD10RING,1*%\nD10*\nX0Y0D03*\nM02*\n";
        let ideal = std::f64::consts::PI * (0.5_f64.powi(2) - 0.25_f64.powi(2));
        assert!((area_mm2(g) - ideal).abs() < 0.01);
    }

    #[test]
    fn parameterised_thermal_has_four_spokes() {
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMTH*\n7,0,0,$1,$2,$3,0*%\n%ADD10TH,2X1X0.2*%\nD10*\nX0Y0D03*\nM02*\n";
        let ps = resolve_layer(g.as_bytes()).unwrap();
        assert_eq!(ps.region_count(), 4);
        assert!(ps.area_mm2() > 1.5 && ps.area_mm2() < 2.4);
    }

    #[test]
    fn parameterised_polygon_vertex_count() {
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMPOLY*\n5,1,$1+2,0,0,$2,0*%\n%ADD10POLY,4X2*%\nD10*\nX0Y0D03*\nM02*\n";
        let ideal = 1.5 * 3.0_f64.sqrt();
        assert!((area_mm2(g) - ideal).abs() < 0.02);
    }

    #[test]
    fn unsupported_macro_line_codes_fail_loud() {
        for primitive in ["2,1,0.2,0,0,1,0,0", "22,1,0.2,0,0,1,0,0"] {
            let g = format!("%FSLAX46Y46*%\n%MOMM*%\n%AMLINE*\n{primitive}*%\n%ADD10LINE*%\nD10*\nX0Y0D03*\nM02*\n");
            assert!(resolve_layer(g.as_bytes()).is_err(), "{primitive}");
        }
    }

    #[test]
    fn macro_expression_precedence() {
        let vars = HashMap::from([(1, 2.0), (2, 3.0)]);
        for (src, want) in [
            ("1+2x3", 7.0),
            ("(1+2)x3", 9.0),
            ("1+6/2", 4.0),
            ("8/2/2", 2.0),
            ("2-3-4", -5.0),
            ("-2x3", -6.0),
            ("2x-3", -6.0),
            ("-(1+2)", -3.0),
            ("$1+$2x$1", 8.0),
            ("$1X$2", 6.0),
            (".5+0.25", 0.75),
        ] {
            let got = expression(src, &vars).unwrap();
            assert!((got - want).abs() < 1e-12, "{src} = {got}, want {want}");
        }
    }

    #[test]
    fn undefined_macro_variable_fails_loud() {
        assert!(expression("$3+1", &HashMap::from([(1, 1.0)])).is_err());
        // RoundRect given only the radius: the corner variables are undefined.
        let g = include_str!("../../../corpus/gerber/roundrect.gbr").replace(
            "%ADD10RoundRect,0.114300X0.165100X-0.114300X0.165100X0.114300X-0.165100X0.114300X-0.165100X-0.114300X0*%",
            "%ADD10RoundRect,0.114300*%",
        );
        assert!(matches!(
            resolve_layer(g.as_bytes()),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn assignment_feeds_later_primitives() {
        // $3 is assigned from $1 and $2, then used as the circle diameter.
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%AMASG*\n$3=$1+$2x2*\n1,1,$3,0,0*%\n%ADD10ASG,0.2X0.4*%\nD10*\nX0Y0D03*\nM02*\n";
        let ideal = std::f64::consts::PI * 0.5 * 0.5;
        assert!((area_mm2(g) - ideal).abs() < 0.01);
    }

    #[test]
    fn nonidentity_short_repeat_fails_loud() {
        for code in ["%SRX1Y1I0.5*%", "%SRX3Y1*%"] {
            let g =
                format!("%FSLAX46Y46*%\n%MOMM*%\n{code}\n%ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n");
            assert!(resolve_layer(g.as_bytes()).is_err(), "{code}");
        }
    }
}
