//! Golden-corpus harness — shared test support (the trust backbone).
//!
//! ROADMAP Phase 0: "built first so every later feature is validated against
//! ground truth." Now retargeted onto the **shipping library API** (M1): the
//! synthetic generator + ground-truth constants live here; the parse → polygonize
//! → diff → measure pipeline they exercise is `etchy_core`. Thin adapters keep the
//! existing `golden_corpus.rs` / `properties.rs` call sites unchanged.

#![allow(dead_code)] // not every helper is used by every test binary

use std::collections::BTreeMap;
use std::f64::consts::PI;

pub use etchy_core::{diff_layer, PolygonSet};

/// Circle→polygon tessellation — matches the library so areas are comparable to
/// the ground-truth maths.
pub const CIRCLE_SEGMENTS: usize = etchy_core::CIRCLE_SEGMENTS;

/// n-gon area as a fraction of the true circle area, for ground-truth tolerances.
pub fn ngon_factor() -> f64 {
    (CIRCLE_SEGMENTS as f64 / (2.0 * PI)) * (2.0 * PI / CIRCLE_SEGMENTS as f64).sin()
}

// ---------------------------------------------------------------------------
// Adapters onto the library (keep call sites in the test files unchanged)
// ---------------------------------------------------------------------------

/// Parse + polygonize Gerber text via the library; `Err` as a string for tests.
pub fn polygonize_gerber(text: &str) -> Result<PolygonSet, String> {
    etchy_core::polygonize_gerber(text.as_bytes()).map_err(|e| e.to_string())
}

/// Build a layer of 0.5 mm circle pads at integer grid cells (`cell * pitch_mm`),
/// routed through the real Gerber pipeline. Cells on a grid coarser than the pad
/// diameter give pads that are either identical or disjoint.
pub fn pad_layer(cells: &[(i32, i32)], pitch_mm: f64) -> PolygonSet {
    let mut s = String::from("%FSLAX46Y46*%\n%MOMM*%\n");
    s.push_str(&format!("%ADD10C,{PAD_DIA_MM:.6}*%\nD10*\n"));
    for &(cx, cy) in cells {
        let x = (cx as f64 * pitch_mm * 1e6).round() as i64;
        let y = (cy as f64 * pitch_mm * 1e6).round() as i64;
        s.push_str(&format!("X{x}Y{y}D03*\n"));
    }
    s.push_str("M02*\n");
    polygonize_gerber(&s).expect("pad_layer polygonize")
}

// ---------------------------------------------------------------------------
// Synthetic board generation (in-Rust port of corpus/tools/gen_synth_board.py)
// ---------------------------------------------------------------------------

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
