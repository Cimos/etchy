//! Spike 1 — prove the polygon-diff engine (`i_overlay`) on a dense board.
//!
//! THROWAWAY de-risking code (ROADMAP Phase 0). It is deliberately self-contained
//! and lives as an example so the `i_overlay` / `gerber_parser` / `gerber-types`
//! deps stay out of `etchy-core`'s real dependency set (they're dev-only).
//!
//! Pipeline proven here: parse → reduced polygonize → per-layer boolean diff
//! (`added = B−A`, `removed = A−B`) → measure (area mm² + region count) → check
//! against `corpus/synthetic/ground_truth.json`. Also checks `diff(A,A)=∅` and
//! determinism (same diff twice → byte-identical contours).
//!
//! Run:  cargo run --release --example spike1 -p etchy-core [-- <corpus_dir>]
//! Mem:  /usr/bin/time -v cargo run --release --example spike1 -p etchy-core
//!
//! Trust bar: the reduced polygonizer FAILS LOUD on anything it can't faithfully
//! render (lines, regions, arcs, non-circle/rect apertures, parse errors,
//! non-mm/inch units). It never silently skips geometry.

use std::collections::HashMap;
use std::error::Error;
use std::f64::consts::PI;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gerber_parser::gerber_types::{
    Aperture, Command, DCode, ExtendedCode, FunctionCode, GCode, Operation, Unit,
};
use gerber_parser::parse;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// Segments used to approximate a circular aperture as a polygon. 64 keeps the
/// area error under ~0.13% (n-gon vs circle), well within the spike's tolerance.
const CIRCLE_SEGMENTS: usize = 64;

/// A filled layer: a set of contours in millimetres, ready for `i_overlay`.
type Contours = Vec<Vec<[f64; 2]>>;

fn main() -> Result<(), Box<dyn Error>> {
    let corpus = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "corpus/synthetic".to_string());
    let corpus = PathBuf::from(corpus);

    println!("=== etchy Spike 1 — i_overlay polygon-diff engine ===");
    println!("corpus: {}", corpus.display());

    let rev_a = polygonize_rev(&corpus.join("revA"))?;
    let rev_b = polygonize_rev(&corpus.join("revB"))?;

    let total_contours: usize = rev_a.values().map(|c| c.len()).sum();
    println!(
        "parsed+polygonized revA: {} layers, {} contours (revB matches)\n",
        rev_a.len(),
        total_contours
    );

    // Pair layers by name; fail loud if the two revisions disagree on layer set.
    let mut layers: Vec<&String> = rev_a.keys().collect();
    layers.sort();
    if rev_a.len() != rev_b.len() {
        return Err(format!(
            "layer-set mismatch: revA {} vs revB {} layers",
            rev_a.len(),
            rev_b.len()
        )
        .into());
    }

    let ground = load_ground_truth(&corpus.join("ground_truth.json"))?;

    let mut report: Vec<LayerDiff> = Vec::new();
    let t_all = Instant::now();
    for name in &layers {
        let a = &rev_a[*name];
        let b = rev_b
            .get(*name)
            .ok_or_else(|| format!("layer {name} missing in revB"))?;

        let t = Instant::now();
        let removed = a.overlay(b, OverlayRule::Difference, FillRule::NonZero); // A − B
        let added = b.overlay(a, OverlayRule::Difference, FillRule::NonZero); // B − A
        let dt = t.elapsed();

        // diff(A,A) must be empty — the cheapest, sharpest correctness guard.
        let self_diff = a.overlay(a, OverlayRule::Difference, FillRule::NonZero);
        if !self_diff.is_empty() {
            return Err(format!(
                "diff(A,A) non-empty on layer {name}: {} shapes",
                self_diff.len()
            )
            .into());
        }

        // Determinism: recompute and require byte-identical contours.
        let added2 = b.overlay(a, OverlayRule::Difference, FillRule::NonZero);
        let deterministic = added == added2;

        report.push(LayerDiff {
            name: (*name).clone(),
            added_area: shapes_area(&added),
            added_regions: added.len(),
            removed_area: shapes_area(&removed),
            removed_regions: removed.len(),
            micros: dt.as_micros(),
            deterministic,
        });
    }
    let total = t_all.elapsed();

    print_report(&report);
    // Production diff = added + removed per layer (2 overlays). The loop also runs
    // diff(A,A) and a determinism re-run (2 more overlays/layer) — excluded here.
    let prod_ms: f64 = report.iter().map(|r| r.micros as f64).sum::<f64>() / 1e3;
    println!(
        "\nproduction diff time (added+removed, all {} layers): {:.3} ms",
        layers.len(),
        prod_ms
    );
    println!(
        "loop wall time incl. diff(A,A)+determinism re-run (4 overlays/layer): {:.3} ms",
        total.as_secs_f64() * 1e3
    );

    let ok = check_correctness(&report, &ground);
    println!(
        "\nGO/NO-GO: {}",
        if ok {
            "GO ✓  i_overlay reproduces the known delta, diff(A,A)=∅, deterministic"
        } else {
            "NO-GO ✗  see failures above"
        }
    );
    if !ok {
        return Err("correctness check failed".into());
    }
    Ok(())
}

struct LayerDiff {
    name: String,
    added_area: f64,
    added_regions: usize,
    removed_area: f64,
    removed_regions: usize,
    micros: u128,
    deterministic: bool,
}

fn print_report(report: &[LayerDiff]) {
    println!(
        "{:<10} {:>12} {:>9} {:>12} {:>9} {:>9} {:>6}",
        "layer", "added_mm2", "+regions", "removed_mm2", "-regions", "µs", "det?"
    );
    for r in report {
        println!(
            "{:<10} {:>12.5} {:>9} {:>12.5} {:>9} {:>9} {:>6}",
            r.name,
            r.added_area,
            r.added_regions,
            r.removed_area,
            r.removed_regions,
            r.micros,
            if r.deterministic { "yes" } else { "NO" }
        );
    }
}

// ---------------------------------------------------------------------------
// Parse + reduced polygonize
// ---------------------------------------------------------------------------

/// Polygonize every `*.gbr` in a revision directory into named filled contours.
fn polygonize_rev(dir: &Path) -> Result<HashMap<String, Contours>, Box<dyn Error>> {
    let mut out = HashMap::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "gbr").unwrap_or(false))
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Err(format!("no .gbr files in {}", dir.display()).into());
    }

    for path in entries {
        let name = layer_name(&path);
        let contours =
            polygonize_file(&path).map_err(|e| format!("polygonizing {}: {e}", path.display()))?;
        out.insert(name, contours);
    }
    Ok(out)
}

/// `corpus/synthetic/revA/synth-F_Cu.gbr` -> `F_Cu`.
fn layer_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
    stem.strip_prefix("synth-").unwrap_or(stem).to_string()
}

/// Parse one Gerber file and emit filled contours (mm). FAILS LOUD on any
/// feature the reduced polygonizer cannot render faithfully.
fn polygonize_file(path: &Path) -> Result<Contours, Box<dyn Error>> {
    let reader = BufReader::new(File::open(path)?);
    // parse() -> Result<GerberDoc, (GerberDoc, ParseError)> in gerber_parser 0.5.
    let doc = parse(reader).map_err(|(_, e)| format!("parse error: {e:?}"))?;

    // Per-command errors are collected inside the doc even on the Ok path.
    let errs = doc.errors();
    if !errs.is_empty() {
        return Err(format!(
            "{} per-command parse error(s); first: {:?}",
            errs.len(),
            errs[0]
        )
        .into());
    }

    // Units → mm scale factor. Fail loud if unspecified.
    let scale = match doc.units {
        Some(Unit::Millimeters) => 1.0,
        Some(Unit::Inches) => 25.4,
        None => return Err("no unit (%MO%) in file".into()),
    };

    let apertures = &doc.apertures;
    let mut current_ap: Option<i32> = None;
    let mut cur = [0.0_f64, 0.0]; // modal current point (mm, pre-scale handled below)
    let mut contours: Contours = Vec::new();

    for cmd in doc.commands() {
        match cmd {
            Command::FunctionCode(FunctionCode::DCode(DCode::SelectAperture(code))) => {
                current_ap = Some(*code);
            }
            Command::FunctionCode(FunctionCode::DCode(DCode::Operation(op))) => {
                match op {
                    Operation::Move(coords) => {
                        cur = resolve(coords, cur);
                    }
                    Operation::Flash(coords) => {
                        let at = resolve(coords, cur);
                        cur = at;
                        let code = current_ap.ok_or("flash (D03) with no aperture selected")?;
                        let ap = apertures.get(&code).ok_or_else(|| {
                            format!("flash references undefined aperture D{code}")
                        })?;
                        contours.push(flash_polygon(ap, [at[0] * scale, at[1] * scale], scale)?);
                    }
                    Operation::Interpolate(_, _) => {
                        return Err("draw/interpolate (D01) not supported by the spike's reduced polygonizer".into());
                    }
                }
            }
            Command::FunctionCode(FunctionCode::GCode(g)) => match g {
                // Benign modal/no-geometry codes.
                GCode::Comment(_)
                | GCode::InterpolationMode(_)
                | GCode::QuadrantMode(_)
                | GCode::Unit(_)
                | GCode::CoordinateMode(_)
                | GCode::SelectAperture => {}
                GCode::RegionMode(true) => {
                    return Err(
                        "region mode (G36) not supported by the spike's reduced polygonizer".into(),
                    );
                }
                GCode::RegionMode(false) => {}
            },
            Command::FunctionCode(FunctionCode::MCode(_)) => {}
            // Allow-list: only codes with no geometry effect in the reduced model
            // are accepted. Everything else (polarity-clear, mirror/rotate/scale,
            // step-repeat, blocks, macros) FAILS LOUD — never silently ignored.
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
                    return Err(format!(
                        "extended code not supported by the spike's reduced polygonizer: {other:?}"
                    )
                    .into());
                }
            },
        }
    }

    Ok(contours)
}

/// Resolve modal coordinates: an omitted axis keeps the current value. Returns mm
/// (pre-scale; caller applies unit scale).
fn resolve(coords: &Option<gerber_parser::gerber_types::Coordinates>, cur: [f64; 2]) -> [f64; 2] {
    match coords {
        None => cur,
        Some(c) => {
            let x = c.x.map(f64::from).unwrap_or(cur[0]);
            let y = c.y.map(f64::from).unwrap_or(cur[1]);
            [x, y]
        }
    }
}

/// A flash at `at` (mm) → a filled polygon. Circle → n-gon, Rect → 4 corners.
/// Fails loud on shapes the spike doesn't render.
fn flash_polygon(ap: &Aperture, at: [f64; 2], scale: f64) -> Result<Vec<[f64; 2]>, Box<dyn Error>> {
    match ap {
        Aperture::Circle(c) => {
            if c.hole_diameter.is_some() {
                return Err("aperture hole (drilled flash) not supported by the spike".into());
            }
            Ok(circle_ngon(at, 0.5 * c.diameter * scale))
        }
        Aperture::Rectangle(r) => {
            if r.hole_diameter.is_some() {
                return Err("aperture hole (drilled flash) not supported by the spike".into());
            }
            let (hw, hh) = (0.5 * r.x * scale, 0.5 * r.y * scale);
            // CCW from bottom-left.
            Ok(vec![
                [at[0] - hw, at[1] - hh],
                [at[0] + hw, at[1] - hh],
                [at[0] + hw, at[1] + hh],
                [at[0] - hw, at[1] + hh],
            ])
        }
        Aperture::Obround(_) => Err("obround aperture not supported by the spike".into()),
        Aperture::Polygon(_) => Err("regular-polygon aperture not supported by the spike".into()),
        Aperture::Macro(name, _) => {
            Err(format!("macro aperture {name} not supported by the spike").into())
        }
    }
}

/// CCW regular n-gon inscribed in radius `r` at `center` (mm).
fn circle_ngon(center: [f64; 2], r: f64) -> Vec<[f64; 2]> {
    (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let a = 2.0 * PI * (k as f64) / (CIRCLE_SEGMENTS as f64);
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Measure
// ---------------------------------------------------------------------------

/// Signed-area-correct total area (mm²) of an `i_overlay` result: outer contours
/// add (CCW, positive), holes subtract (CW, negative). Sum |signed area| works
/// because the overlay output is already correctly wound.
fn shapes_area(shapes: &[Vec<Vec<[f64; 2]>>]) -> f64 {
    shapes
        .iter()
        .flat_map(|shape| shape.iter())
        .map(|contour| shoelace(contour).abs())
        .sum::<f64>()
        - 2.0
            * shapes
                .iter()
                .flat_map(|shape| shape.iter().skip(1)) // holes
                .map(|contour| shoelace(contour).abs())
                .sum::<f64>()
}

/// Signed area via the shoelace formula (mm²). CCW > 0, CW < 0.
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
// Ground-truth correctness check
// ---------------------------------------------------------------------------

struct Ground {
    pad_area_mm2: f64,
    block_pads: u64,
    changed: HashMap<String, (String, u64)>, // layer -> (kind, pads)
    unchanged: Vec<String>,
}

fn load_ground_truth(path: &Path) -> Result<Ground, Box<dyn Error>> {
    let v: serde_json::Value = serde_json::from_reader(BufReader::new(File::open(path)?))?;
    let mut changed = HashMap::new();
    for (k, c) in v["expected_changed"]
        .as_object()
        .ok_or("bad ground truth")?
    {
        changed.insert(
            k.clone(),
            (
                c["kind"].as_str().unwrap_or("").to_string(),
                c["pads"].as_u64().unwrap_or(0),
            ),
        );
    }
    Ok(Ground {
        pad_area_mm2: v["pad_area_mm2"].as_f64().ok_or("missing pad_area_mm2")?,
        block_pads: v["block_pads"].as_u64().ok_or("missing block_pads")?,
        changed,
        unchanged: v["expected_unchanged"]
            .as_array()
            .ok_or("missing expected_unchanged")?
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
    })
}

fn check_correctness(report: &[LayerDiff], g: &Ground) -> bool {
    let mut ok = true;
    let by_name: HashMap<&str, &LayerDiff> = report.iter().map(|r| (r.name.as_str(), r)).collect();

    // n-gon underestimates the circle area; allow a small tolerance band.
    let expected_block_area = g.pad_area_mm2 * g.block_pads as f64;
    let tol = 0.01 * expected_block_area; // 1%
    let ngon_factor =
        (CIRCLE_SEGMENTS as f64 / (2.0 * PI)) * (2.0 * PI / CIRCLE_SEGMENTS as f64).sin();

    println!(
        "\nexpected block: {} pads × {:.5} mm² = {:.5} mm² (n-gon factor {:.5})",
        g.block_pads, g.pad_area_mm2, expected_block_area, ngon_factor
    );

    for (layer, (kind, pads)) in &g.changed {
        let Some(r) = by_name.get(layer.as_str()) else {
            println!("  FAIL: changed layer {layer} not in report");
            ok = false;
            continue;
        };
        let (area, regions, other_area) = match kind.as_str() {
            "added" => (r.added_area, r.added_regions, r.removed_area),
            "removed" => (r.removed_area, r.removed_regions, r.added_area),
            other => {
                println!("  FAIL: {layer} unknown kind {other}");
                ok = false;
                continue;
            }
        };
        let expect_area = g.pad_area_mm2 * *pads as f64 * ngon_factor;
        let area_ok = (area - expect_area).abs() <= tol;
        let regions_ok = regions as u64 == *pads;
        let other_empty = other_area.abs() < 1e-9;
        if !(area_ok && regions_ok && other_empty) {
            ok = false;
        }
        println!(
            "  {} {layer}: area {:.5} (expect {:.5} ±{:.5}) [{}], regions {} (expect {}) [{}], opposite-dir empty [{}]",
            kind, area, expect_area, tol,
            if area_ok { "ok" } else { "FAIL" },
            regions, pads,
            if regions_ok { "ok" } else { "FAIL" },
            if other_empty { "ok" } else { "FAIL" },
        );
    }

    for layer in &g.unchanged {
        let Some(r) = by_name.get(layer.as_str()) else {
            println!("  FAIL: unchanged layer {layer} not in report");
            ok = false;
            continue;
        };
        let empty = r.added_regions == 0 && r.removed_regions == 0;
        if !empty {
            ok = false;
            println!(
                "  FAIL: unchanged {layer} diffed non-empty: +{} regions / -{} regions",
                r.added_regions, r.removed_regions
            );
        }
    }
    if g.unchanged.iter().all(|l| {
        by_name
            .get(l.as_str())
            .map(|r| r.added_regions == 0 && r.removed_regions == 0)
            .unwrap_or(false)
    }) {
        println!(
            "  all {} unchanged layers diffed to ∅  [ok]",
            g.unchanged.len()
        );
    }

    if report.iter().all(|r| r.deterministic) {
        println!("  determinism: all layers byte-identical on re-run  [ok]");
    } else {
        ok = false;
        println!("  FAIL: non-deterministic layers present");
    }

    ok
}
