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
    Aperture as GtAperture, ApertureMacro, Command, CoordinateOffset, Coordinates, DCode,
    ExtendedCode, FunctionCode, GCode, InterpolationMode, MacroBoolean, MacroContent, MacroDecimal,
    MacroInteger, Operation, Polarity as GtPolarity, Unit,
};
use gerber_parser::parse;

use crate::boolean;
use crate::error::{EngineError, Result};
use crate::geo::{quantize_mm, snap_nm, Contour, PolygonSet, Pt};
use crate::geom;

/// Parse + resolve one Gerber layer's bytes into its filled copper geometry.
pub fn resolve_layer(bytes: &[u8]) -> Result<PolygonSet> {
    let normalized = normalize(bytes)?;
    let doc = parse(BufReader::new(Cursor::new(normalized.as_bytes())))
        .map_err(|(_, e)| EngineError::Parse(format!("{e:?}")))?;

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

    // Collect aperture-macro definitions (referenced by macro apertures).
    let mut macros: HashMap<&str, &ApertureMacro> = HashMap::new();
    for cmd in doc.commands() {
        if let Command::ExtendedCode(ExtendedCode::ApertureMacro(am)) = cmd {
            macros.insert(am.name.as_str(), am);
        }
    }

    let mut m = Machine::new(&doc.apertures, &macros, unit_to_mm, nm_per_unit);
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
fn normalize(bytes: &[u8]) -> Result<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut buf = String::new();
    let mut in_ext = false;

    for ch in text.chars() {
        if in_ext {
            buf.push(ch);
            if ch == '%' {
                out.push_str(buf.trim());
                out.push('\n');
                buf.clear();
                in_ext = false;
            }
            continue;
        }
        match ch {
            '%' => {
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
    Ok(out)
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

    /// Objects in paint order, batched into runs of one polarity. Gerber polarity
    /// is sequential — a later dark run repaints over an earlier clear — so these
    /// are resolved in order (union for dark, subtract for clear), not as one
    /// order-independent `dark − clear` pass.
    spans: Vec<(bool, Vec<Contour>)>,

    cur: Pt,
    ap: Option<i32>,
    interp: InterpolationMode,
    polarity_dark: bool,

    in_region: bool,
    region_loops: Vec<Contour>,
    cur_loop: Contour,
}

impl<'a> Machine<'a> {
    fn new(
        apertures: &'a HashMap<i32, GtAperture>,
        macros: &'a HashMap<&'a str, &'a ApertureMacro>,
        unit_to_mm: f64,
        nm_per_unit: f64,
    ) -> Self {
        Self {
            apertures,
            macros,
            unit_to_mm,
            nm_per_unit,
            spans: Vec::new(),
            cur: Pt::new(0, 0),
            ap: None,
            interp: InterpolationMode::Linear,
            polarity_dark: true,
            in_region: false,
            region_loops: Vec::new(),
            cur_loop: Vec::new(),
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
                GCode::RegionMode(true) => self.begin_region(),
                GCode::RegionMode(false) => self.end_region(),
                GCode::Comment(_)
                | GCode::QuadrantMode(_)
                | GCode::Unit(_)
                | GCode::CoordinateMode(_)
                | GCode::SelectAperture => {}
            },
            Command::FunctionCode(FunctionCode::MCode(_)) => {}
            Command::ExtendedCode(ec) => self.extended(ec)?,
        }
        Ok(())
    }

    fn extended(&mut self, ec: &ExtendedCode) -> Result<()> {
        match ec {
            ExtendedCode::LoadPolarity(p) => self.polarity_dark = matches!(p, GtPolarity::Dark),
            ExtendedCode::CoordinateFormat(_)
            | ExtendedCode::Unit(_)
            | ExtendedCode::ApertureDefinition(_)
            | ExtendedCode::ApertureMacro(_)
            | ExtendedCode::FileAttribute(_)
            | ExtendedCode::ObjectAttribute(_)
            | ExtendedCode::ApertureAttribute(_)
            | ExtendedCode::DeleteAttribute(_)
            | ExtendedCode::ImageName(_) => {}
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

    fn begin_region(&mut self) {
        self.in_region = true;
        self.region_loops.clear();
        self.cur_loop.clear();
    }

    fn end_region(&mut self) {
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
        self.in_region = false;
    }

    fn region_segment(&mut self, to: Pt, offset: &Option<CoordinateOffset>) -> Result<()> {
        if self.is_arc(offset) {
            let center = self.arc_center(offset)?;
            let ccw = matches!(self.interp, InterpolationMode::CounterclockwiseCircular);
            let pts = geom::arc_points(
                self.cur.x as f64,
                self.cur.y as f64,
                to.x as f64,
                to.y as f64,
                center.0,
                center.1,
                ccw,
            );
            self.cur_loop.extend(pts.into_iter().skip(1)); // first == cur (already present)
        } else {
            self.cur_loop.push(to);
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
            let center = self.arc_center(offset)?;
            let ccw = matches!(self.interp, InterpolationMode::CounterclockwiseCircular);
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

    fn arc_center(&self, offset: &Option<CoordinateOffset>) -> Result<(f64, f64)> {
        let o = offset
            .as_ref()
            .ok_or_else(|| unsupported("arc without I/J offset"))?;
        let i = o.x.map(f64::from).unwrap_or(0.0);
        let j = o.y.map(f64::from).unwrap_or(0.0);
        // I/J are relative to the current point, in document units.
        Ok((
            self.cur.x as f64 + i * self.nm_per_unit,
            self.cur.y as f64 + j * self.nm_per_unit,
        ))
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
                if args.as_ref().is_some_and(|a| !a.is_empty()) {
                    return Err(unsupported(&format!(
                        "macro aperture {name} with parameters"
                    )));
                }
                let am = *self
                    .macros
                    .get(name.as_str())
                    .ok_or_else(|| unsupported(&format!("undefined macro {name}")))?;
                self.flash_macro(am, at)?;
            }
        }
        Ok(())
    }

    fn flash_macro(&mut self, am: &ApertureMacro, at: Pt) -> Result<()> {
        let (ax, ay) = (at.x as f64, at.y as f64);
        for content in &am.content {
            match content {
                MacroContent::Comment(_) | MacroContent::VariableDefinition(_) => {}
                MacroContent::Circle(c) => {
                    let exp = mbool(&c.exposure)?;
                    let d = self.dim(md(&c.diameter)?)?;
                    let (cx, cy) = self.macro_pt(&c.center, c.angle.as_ref(), ax, ay)?;
                    self.push(geom::ngon(cx, cy, d / 2.0), exp);
                }
                MacroContent::CenterLine(l) => {
                    let exp = mbool(&l.exposure)?;
                    let (w, h) = (
                        self.dim(md(&l.dimensions.0)?)?,
                        self.dim(md(&l.dimensions.1)?)?,
                    );
                    let ang = md(&l.angle)?;
                    let (cx, cy) = self.macro_offset(&l.center, ang, ax, ay)?;
                    self.push(geom::rect_rot(cx, cy, w, h, ang), exp);
                }
                MacroContent::Outline(o) => {
                    let exp = mbool(&o.exposure)?;
                    let ang = md(&o.angle)?;
                    let mut c: Contour = Vec::with_capacity(o.points.len());
                    for (px, py) in &o.points {
                        let (lx, ly) = (self.dim(md(px)?)?, self.dim(md(py)?)?);
                        let (rx, ry) = geom::rotate(lx, ly, ang);
                        c.push(Pt::new(snap_nm(ax + rx)?, snap_nm(ay + ry)?));
                    }
                    self.push(c, exp);
                }
                MacroContent::VectorLine(v) => {
                    let exp = mbool(&v.exposure)?;
                    let ang = md(&v.angle)?;
                    let w = self.dim(md(&v.width)?)?;
                    let (sx, sy) = self.macro_rot_pt(&v.start, ang, ax, ay)?;
                    let (ex, ey) = self.macro_rot_pt(&v.end, ang, ax, ay)?;
                    self.push(geom::stadium(sx, sy, ex, ey, w / 2.0), exp);
                }
                MacroContent::Polygon(p) => {
                    let exp = mbool(&p.exposure)?;
                    let n = match p.vertices {
                        MacroInteger::Value(v) => v as usize,
                        _ => return Err(unsupported("macro polygon with variable vertex count")),
                    };
                    if !(3..=64).contains(&n) {
                        return Err(unsupported("macro polygon vertex count out of range"));
                    }
                    let ang = md(&p.angle)?;
                    let (cx, cy) = self.macro_offset(&p.center, ang, ax, ay)?;
                    self.push(
                        polygon_ngon(cx, cy, self.dim(md(&p.diameter)?)? / 2.0, n, ang),
                        exp,
                    );
                }
                MacroContent::Moire(_) => return Err(unsupported("macro moiré primitive")),
                MacroContent::Thermal(_) => return Err(unsupported("macro thermal primitive")),
            }
        }
        Ok(())
    }

    /// A macro-local centre `(x, y)` (doc units) rotated by `angle` about the macro
    /// origin and translated to the flash point — returns absolute nm.
    fn macro_offset(
        &self,
        center: &(MacroDecimal, MacroDecimal),
        angle: f64,
        ax: f64,
        ay: f64,
    ) -> Result<(f64, f64)> {
        let (lx, ly) = (self.dim(md(&center.0)?)?, self.dim(md(&center.1)?)?);
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
    ) -> Result<(f64, f64)> {
        let ang = match angle {
            Some(a) => md(a)?,
            None => 0.0,
        };
        self.macro_offset(center, ang, ax, ay)
    }

    fn macro_rot_pt(
        &self,
        point: &(MacroDecimal, MacroDecimal),
        angle: f64,
        ax: f64,
        ay: f64,
    ) -> Result<(f64, f64)> {
        self.macro_offset(point, angle, ax, ay)
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
        let is_dark = self.polarity_dark == exposure;
        // Normalize winding by polarity (dark→CCW, clear→CW) so the NonZero union
        // adds overlapping same-polarity primitives instead of cancelling them. An
        // exporter is free to wind a macro Outline/Polygon either way; a CW dark pad
        // would otherwise punch a hole where a CCW track overlaps it (the notch at a
        // track→pad junction — issue #13).
        let c = crate::geo::wind(c, is_dark);
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

/// Read a literal macro decimal; variables/expressions are unsupported (the
/// boards we target use literals only).
fn md(d: &MacroDecimal) -> Result<f64> {
    match d {
        MacroDecimal::Value(v) => Ok(*v),
        _ => Err(unsupported("aperture macro with variables/expressions")),
    }
}

fn mbool(b: &MacroBoolean) -> Result<bool> {
    match b {
        MacroBoolean::Value(v) => Ok(*v),
        _ => Err(unsupported(
            "aperture macro exposure with variable/expression",
        )),
    }
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
        assert_eq!(ps.shapes.len(), 1, "pad+track should be one connected region");
        assert_eq!(
            ps.shapes[0].len(),
            1,
            "track→pad junction must be solid copper, not a notched hole"
        );
        // Sanity: area exceeds the 1 mm² pad alone (the track adds copper).
        assert!(ps.area_mm2() > 1.2, "area {} too small", ps.area_mm2());
    }

    #[test]
    fn region_fills_its_outline() {
        // A 2×2 mm square region → 4 mm².
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\nX0Y0D02*\nG01*\nX2000000Y0D01*\nX2000000Y2000000D01*\nX0Y2000000D01*\nX0Y0D01*\nG37*\nM02*\n";
        let a = area_mm2(g);
        assert!((a - 4.0).abs() < 0.001, "region area {a} vs 4.0");
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
        // and subtracted by the clear) — the FMU "trace-shaped voids" bug.
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
        // The real FMU case: REV4 inches@2.5 vs REV67 mm@4.4.
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
}
