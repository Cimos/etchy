//! Gerber RS-274X front-end: bytes → format-agnostic [`Primitive`]s.
//!
//! Takes **bytes** (never a path — the library owns no I/O policy). Resolves the
//! graphics state (units, current aperture, modal position) and emits `Primitive`s
//! for the diff to consume. This increment emits `Flash` of circle/rect apertures;
//! everything else is a loud [`EngineError::Unsupported`] (the trust bar — see
//! `docs/SPIKE_2.md` for the catalogued gaps and the planned widening).

use std::io::{BufReader, Cursor};

use gerber_parser::gerber_types::{
    Aperture as GtAperture, Command, Coordinates, DCode, ExtendedCode, FunctionCode, GCode,
    Operation, Unit,
};
use gerber_parser::parse;

use crate::error::{EngineError, Result};
use crate::geo::{quantize_mm, Aperture, Primitive, Pt};

fn invalid_aperture(detail: &str) -> EngineError {
    EngineError::InvalidAperture {
        detail: detail.to_string(),
    }
}

/// Parse Gerber bytes into the engine's primitive IR. Fails loud on parse errors
/// and on any geometry feature this build cannot render faithfully.
pub fn parse_gerber(bytes: &[u8]) -> Result<Vec<Primitive>> {
    // gerber-parser 0.5 is line-oriented (one command per physical line) and
    // SILENTLY drops extra commands on a compact, `*`-delimited line (gEDA-style,
    // e.g. `X0Y0D03*X1Y1D03*`) — a wrong-but-quiet diff (see docs/SPIKE_2.md). We
    // detect that here and fail loud; the proper `*`-tokenization shim that
    // *recovers* such geometry is a later increment.
    if let Some(line) = first_compact_line(bytes) {
        return Err(unsupported(&format!(
            "multiple commands on one physical line (compact/gEDA-style Gerber): '{line}'"
        )));
    }

    let doc = parse(BufReader::new(Cursor::new(bytes)))
        .map_err(|(_, e)| EngineError::Parse(format!("{e:?}")))?;

    // Per-command parse errors → fail loud (never proceed on partial geometry).
    let errs = doc.errors();
    if !errs.is_empty() {
        return Err(EngineError::CommandErrors {
            count: errs.len(),
            first: format!("{:?}", errs[0]),
        });
    }

    // Document-unit → mm scale factor. Refuse to guess.
    let scale = match doc.units {
        Some(Unit::Millimeters) => 1.0,
        Some(Unit::Inches) => 25.4,
        None => return Err(EngineError::UnitsUnresolved),
    };

    let apertures = &doc.apertures;
    let mut current_ap: Option<i32> = None;
    let mut cur = Pt::new(0, 0);
    let mut out: Vec<Primitive> = Vec::new();

    for cmd in doc.commands() {
        match cmd {
            Command::FunctionCode(FunctionCode::DCode(DCode::SelectAperture(code))) => {
                current_ap = Some(*code);
            }
            Command::FunctionCode(FunctionCode::DCode(DCode::Operation(op))) => match op {
                Operation::Move(coords) => {
                    cur = resolve(coords, cur, scale)?;
                }
                Operation::Flash(coords) => {
                    let at = resolve(coords, cur, scale)?;
                    cur = at;
                    let code = current_ap.ok_or(EngineError::NoApertureSelected)?;
                    let gt_ap = apertures
                        .get(&code)
                        .ok_or(EngineError::UndefinedAperture { code })?;
                    out.push(Primitive::Flash {
                        at,
                        aperture: map_aperture(gt_ap, scale)?,
                    });
                }
                Operation::Interpolate(..) => {
                    return Err(unsupported("draw/interpolate (D01) — traces/lines"));
                }
            },
            Command::FunctionCode(FunctionCode::GCode(g)) => match g {
                // No-geometry modal codes: safe to ignore.
                GCode::Comment(_)
                | GCode::InterpolationMode(_)
                | GCode::QuadrantMode(_)
                | GCode::Unit(_)
                | GCode::CoordinateMode(_)
                | GCode::SelectAperture
                | GCode::RegionMode(false) => {}
                GCode::RegionMode(true) => return Err(unsupported("region fill (G36/G37)")),
            },
            Command::FunctionCode(FunctionCode::MCode(_)) => {}
            // Allow only metadata extended codes; anything geometry-affecting is loud.
            Command::ExtendedCode(ec) => match ec {
                ExtendedCode::CoordinateFormat(_)
                | ExtendedCode::Unit(_)
                | ExtendedCode::ApertureDefinition(_)
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
            },
        }
    }

    Ok(out)
}

fn unsupported(feature: &str) -> EngineError {
    EngineError::Unsupported {
        feature: feature.to_string(),
    }
}

/// Detect a physical line carrying more than one `*`-terminated command outside an
/// extended-code (`%…%`) block — the input gerber-parser silently truncates.
/// Returns the offending line (trimmed, capped) if found. `*` inside `%…%` blocks
/// (format spec, aperture macros) is not a command terminator and is ignored.
fn first_compact_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut in_ext = false;
    for line in text.lines() {
        let mut stars = 0usize;
        for ch in line.chars() {
            match ch {
                '%' => in_ext = !in_ext,
                '*' if !in_ext => stars += 1,
                _ => {}
            }
        }
        if stars > 1 {
            return Some(line.trim().chars().take(60).collect());
        }
    }
    None
}

/// Resolve a modal coordinate set against the current point, in document units,
/// and quantize to nm. An omitted axis keeps the current value.
fn resolve(coords: &Option<Coordinates>, cur: Pt, scale: f64) -> Result<Pt> {
    let Some(c) = coords else { return Ok(cur) };
    let x = match c.x {
        Some(v) => quantize_mm(f64::from(v) * scale)?,
        None => cur.x,
    };
    let y = match c.y {
        Some(v) => quantize_mm(f64::from(v) * scale)?,
        None => cur.y,
    };
    Ok(Pt::new(x, y))
}

/// Map a parsed aperture to the engine's aperture, quantized to nm. Fails loud on
/// shapes this build does not render (obround, polygon, macro) and on drilled
/// (hole) apertures (copper-minus-hole is a silent-miss risk until handled).
fn map_aperture(ap: &GtAperture, scale: f64) -> Result<Aperture> {
    match ap {
        GtAperture::Circle(c) => {
            if c.hole_diameter.is_some() {
                return Err(unsupported("drilled (hole) circle aperture"));
            }
            // (NaN/inf is caught downstream by quantize_mm's finite guard.)
            if c.diameter <= 0.0 {
                return Err(invalid_aperture(&format!(
                    "circle diameter {} must be > 0",
                    c.diameter
                )));
            }
            Ok(Aperture::Circle {
                diameter_nm: quantize_mm(c.diameter * scale)?,
            })
        }
        GtAperture::Rectangle(r) => {
            if r.hole_diameter.is_some() {
                return Err(unsupported("drilled (hole) rectangle aperture"));
            }
            if r.x <= 0.0 || r.y <= 0.0 {
                return Err(invalid_aperture(&format!(
                    "rectangle {}x{} must be positive",
                    r.x, r.y
                )));
            }
            Ok(Aperture::Rect {
                w_nm: quantize_mm(r.x * scale)?,
                h_nm: quantize_mm(r.y * scale)?,
            })
        }
        // (Obround/Polygon/Macro are unsupported below — their positivity is moot.)
        GtAperture::Obround(_) => Err(unsupported("obround aperture")),
        GtAperture::Polygon(_) => Err(unsupported("regular-polygon aperture")),
        GtAperture::Macro(name, _) => Err(unsupported(&format!("macro aperture {name}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDR: &str = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";

    #[test]
    fn compact_multi_command_line_fails_loud() {
        // gEDA-style: three flashes on one physical line — gerber-parser would keep
        // only the first. We must fail loud, never silently drop the other two.
        let g = format!("{HDR}X0Y0D03*X1000000Y0D03*X2000000Y0D03*\nM02*\n");
        let err = parse_gerber(g.as_bytes()).unwrap_err();
        assert!(
            matches!(err, EngineError::Unsupported { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn one_command_per_line_is_fine() {
        let g = format!("{HDR}X0Y0D03*\nX1000000Y0D03*\nM02*\n");
        let prims = parse_gerber(g.as_bytes()).unwrap();
        assert_eq!(prims.len(), 2);
    }

    #[test]
    fn extended_blocks_do_not_trip_compact_detection() {
        // %FS…*% has an internal '*' but is one extended statement — not compact.
        assert!(first_compact_line(HDR.as_bytes()).is_none());
    }

    #[test]
    fn non_positive_aperture_fails_loud() {
        let g = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0*%\nD10*\nX0Y0D03*\nM02*\n";
        let err = parse_gerber(g.as_bytes()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidAperture { .. }),
            "got {err:?}"
        );
    }
}
