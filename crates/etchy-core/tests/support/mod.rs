//! Golden-corpus harness — shared test support (the trust backbone).
//!
//! ROADMAP Phase 0: "built first so every later feature is validated against
//! ground truth." This module holds the *durable* harness machinery — synthetic
//! board generation, the parse→polygonize→diff→measure pipeline, and area/region
//! measurement — used by both `golden_corpus.rs` and `properties.rs`.
//!
//! ## Why this lives in `tests/`, not the library
//!
//! `etchy-core` stays std-only through Phase 0 (CLAUDE.md); the boolean engine
//! (`i_overlay`) and Gerber front-end (`gerber-parser`) are **dev-dependencies**,
//! available to tests/examples but not to the library. The pipeline below is the
//! tidied form of what Spike 1/2 proved. When the real engine lands in
//! `etchy-core` (M1), these tests retarget the library API (`etchy_core::diff`)
//! and this module shrinks to just the synthetic generator + ground truth — the
//! invariants and golden contracts it encodes are the point, and they carry over.

#![allow(dead_code)] // not every helper is used by every test binary

use std::collections::BTreeMap;
use std::f64::consts::PI;

use gerber_parser::gerber_types::{Aperture, Command, DCode, FunctionCode, GCode, Operation, Unit};
use gerber_parser::parse;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// Circle→polygon tessellation. Matches Spike 1 so areas are comparable.
pub const CIRCLE_SEGMENTS: usize = 64;

/// n-gon area as a fraction of the true circle area, for ground-truth tolerances.
pub fn ngon_factor() -> f64 {
    (CIRCLE_SEGMENTS as f64 / (2.0 * PI)) * (2.0 * PI / CIRCLE_SEGMENTS as f64).sin()
}

/// A filled layer: contours in millimetres, ready for `i_overlay`.
pub type Contours = Vec<Vec<[f64; 2]>>;

// ---------------------------------------------------------------------------
// Parse + reduced polygonize (circle/rect flashes) — proven in Spike 1
// ---------------------------------------------------------------------------

/// Parse Gerber text → filled contours (mm). Fails loud on anything the reduced
/// polygonizer can't render faithfully — the harness must never silently drop
/// geometry (the trust bar). Returns `Err` rather than panicking so property
/// tests can assert "no panic, clean error" on arbitrary input.
pub fn polygonize_gerber(text: &str) -> Result<Contours, String> {
    let reader = std::io::BufReader::new(std::io::Cursor::new(text.as_bytes().to_vec()));
    let doc = parse(reader).map_err(|(_, e)| format!("parse error: {e:?}"))?;

    let errs = doc.errors();
    if !errs.is_empty() {
        return Err(format!("{} per-command parse error(s)", errs.len()));
    }

    let scale = match doc.units {
        Some(Unit::Millimeters) => 1.0,
        Some(Unit::Inches) => 25.4,
        None => return Err("no unit (%MO)".into()),
    };

    let apertures = &doc.apertures;
    let mut current_ap: Option<i32> = None;
    let mut cur = [0.0_f64, 0.0];
    let mut contours: Contours = Vec::new();

    for cmd in doc.commands() {
        match cmd {
            Command::FunctionCode(FunctionCode::DCode(DCode::SelectAperture(code))) => {
                current_ap = Some(*code);
            }
            Command::FunctionCode(FunctionCode::DCode(DCode::Operation(op))) => match op {
                Operation::Move(coords) => cur = resolve(coords, cur),
                Operation::Flash(coords) => {
                    let at = resolve(coords, cur);
                    cur = at;
                    let code = current_ap.ok_or("flash with no aperture selected")?;
                    let ap = apertures
                        .get(&code)
                        .ok_or_else(|| format!("flash references undefined aperture D{code}"))?;
                    contours.push(flash_polygon(ap, [at[0] * scale, at[1] * scale], scale)?);
                }
                Operation::Interpolate(..) => {
                    return Err(
                        "draw/interpolate (D01) unsupported by the reduced polygonizer".into(),
                    )
                }
            },
            Command::FunctionCode(FunctionCode::GCode(GCode::RegionMode(true))) => {
                return Err("region mode (G36) unsupported by the reduced polygonizer".into())
            }
            _ => {}
        }
    }
    Ok(contours)
}

fn resolve(coords: &Option<gerber_parser::gerber_types::Coordinates>, cur: [f64; 2]) -> [f64; 2] {
    match coords {
        None => cur,
        Some(c) => [
            c.x.map(f64::from).unwrap_or(cur[0]),
            c.y.map(f64::from).unwrap_or(cur[1]),
        ],
    }
}

fn flash_polygon(ap: &Aperture, at: [f64; 2], scale: f64) -> Result<Vec<[f64; 2]>, String> {
    match ap {
        Aperture::Circle(c) if c.hole_diameter.is_none() => {
            Ok(circle_ngon(at, 0.5 * c.diameter * scale))
        }
        Aperture::Rectangle(r) if r.hole_diameter.is_none() => {
            let (hw, hh) = (0.5 * r.x * scale, 0.5 * r.y * scale);
            Ok(vec![
                [at[0] - hw, at[1] - hh],
                [at[0] + hw, at[1] - hh],
                [at[0] + hw, at[1] + hh],
                [at[0] - hw, at[1] + hh],
            ])
        }
        _ => Err("aperture shape unsupported by the reduced polygonizer".into()),
    }
}

fn circle_ngon(center: [f64; 2], r: f64) -> Vec<[f64; 2]> {
    (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let a = 2.0 * PI * (k as f64) / (CIRCLE_SEGMENTS as f64);
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect()
}

/// Build a layer of 0.5 mm circle pads directly from integer grid cells (one pad
/// per cell, placed at `cell * pitch_mm`). Used by property tests: cells on a
/// grid with `pitch_mm` ≥ pad diameter give pads that are either identical or
/// disjoint, so set-algebra reasoning about areas is exact.
pub fn pad_layer(cells: &[(i32, i32)], pitch_mm: f64) -> Contours {
    cells
        .iter()
        .map(|&(cx, cy)| {
            circle_ngon(
                [cx as f64 * pitch_mm, cy as f64 * pitch_mm],
                PAD_DIA_MM / 2.0,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Diff + measure
// ---------------------------------------------------------------------------

/// One layer's diff result: `added = B−A`, `removed = A−B`, with magnitudes.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerDiff {
    pub added: Shapes,
    pub removed: Shapes,
}

/// `i_overlay` output: `Vec<Shape>`, `Shape = Vec<Contour>` (CCW outer + CW holes).
pub type Shapes = Vec<Vec<Vec<[f64; 2]>>>;

impl LayerDiff {
    pub fn added_area(&self) -> f64 {
        shapes_area(&self.added)
    }
    pub fn removed_area(&self) -> f64 {
        shapes_area(&self.removed)
    }
    pub fn added_regions(&self) -> usize {
        self.added.len()
    }
    pub fn removed_regions(&self) -> usize {
        self.removed.len()
    }
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// The core diff: one boolean computation each direction. This is exactly the M1
/// engine's contract (`added`, `removed` from a single per-layer pair).
pub fn diff_layer(a: &Contours, b: &Contours) -> LayerDiff {
    LayerDiff {
        removed: a.overlay(b, OverlayRule::Difference, FillRule::NonZero),
        added: b.overlay(a, OverlayRule::Difference, FillRule::NonZero),
    }
}

/// Signed-area-correct total area (mm²): outer contours add, holes subtract.
pub fn shapes_area(shapes: &Shapes) -> f64 {
    shapes
        .iter()
        .map(|shape| {
            shape
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let a = shoelace(c).abs();
                    if i == 0 {
                        a
                    } else {
                        -a
                    } // contour 0 = outer, rest = holes
                })
                .sum::<f64>()
        })
        .sum()
}

fn shoelace(c: &[[f64; 2]]) -> f64 {
    let n = c.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let p = c[i];
        let q = c[(i + 1) % n];
        s += p[0] * q[1] - q[0] * p[1];
    }
    0.5 * s
}

// ---------------------------------------------------------------------------
// Synthetic board generation (in-Rust port of corpus/tools/gen_synth_board.py)
// ---------------------------------------------------------------------------
//
// Self-contained so the golden test runs in CI with no Python and no on-disk
// corpus. Mirrors the generator exactly: 4.6 mm format, 0.5 mm circle pads,
// an N×N base grid common to every layer, a 10×10 added block on F_Cu and a
// 10×10 removed block on In1_Cu.

pub const PAD_DIA_MM: f64 = 0.5;
pub const BLOCK: usize = 10;

/// True (circle) area of one pad, mm².
pub fn pad_area_mm2() -> f64 {
    PI * (PAD_DIA_MM / 2.0).powi(2)
}

fn header() -> String {
    format!("G04 etchy synthetic board*\n%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,{PAD_DIA_MM:.6}*%\nD10*\n")
}

fn flash_grid(nx: usize, ny: usize, pitch: f64, ox: f64, oy: f64) -> String {
    let mut s = String::new();
    for j in 0..ny {
        for i in 0..nx {
            let x = ((ox + i as f64 * pitch) * 1e6).round() as i64;
            let y = ((oy + j as f64 * pitch) * 1e6).round() as i64;
            s.push_str(&format!("X{x}Y{y}D03*\n"));
        }
    }
    s
}

/// A synthetic revision pair. `grid` = N (N×N base pads/layer), `pitch` mm.
pub struct SynthBoard {
    pub rev_a: BTreeMap<String, String>, // layer name → Gerber text
    pub rev_b: BTreeMap<String, String>,
    pub copper_layers: usize,
}

/// Generate the same board pair as `gen_synth_board.py --copper-layers k --grid n`.
pub fn gen_synth_board(copper_layers: usize, grid: usize, pitch: f64) -> SynthBoard {
    let base = flash_grid(grid, grid, pitch, 5.0, 5.0);
    let edge = 5.0 + grid as f64 * pitch + 5.0;
    let add_block = flash_grid(BLOCK, BLOCK, 1.0, edge, 5.0);
    let rm_block = flash_grid(BLOCK, BLOCK, 1.0, edge, 30.0);

    let mut cu = vec!["F_Cu".to_string()];
    for i in 1..=copper_layers.saturating_sub(2) {
        cu.push(format!("In{i}_Cu"));
    }
    cu.push("B_Cu".to_string());
    let others = ["F_Mask", "B_Mask", "F_Silk", "B_Silk", "F_Paste", "B_Paste"];
    let layers: Vec<String> = cu
        .iter()
        .cloned()
        .chain(others.iter().map(|s| s.to_string()))
        .collect();

    let mut rev_a = BTreeMap::new();
    let mut rev_b = BTreeMap::new();
    for layer in &layers {
        let mut a = format!("{}{}", header(), base);
        let mut b = a.clone();
        match layer.as_str() {
            "F_Cu" => b.push_str(&add_block),  // revB gains a block → "added"
            "In1_Cu" => a.push_str(&rm_block), // present in A only → "removed"
            _ => {}
        }
        a.push_str("M02*\n");
        b.push_str("M02*\n");
        rev_a.insert(layer.clone(), a);
        rev_b.insert(layer.clone(), b);
    }
    SynthBoard {
        rev_a,
        rev_b,
        copper_layers: cu.len(),
    }
}
