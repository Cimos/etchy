//! Golden-corpus tests — validate the engine against *exact, known* ground truth.
//!
//! Two layers of validation:
//!  1. `synthetic_*` — self-contained: generate the known-delta board pair in
//!     Rust, run the full pipeline, assert the recovered delta matches the
//!     contract. Always runs in CI (no Python, no on-disk corpus needed).
//!  2. `committed_corpus_*` — if `corpus/synthetic/{revA,revB}` + `ground_truth.json`
//!     are present on disk, validate against them too (the files verified against
//!     the frozen v0.11 tool). Skipped with a notice when absent (they're
//!     gitignored — regenerate via `corpus/tools/gen_synth_board.py`).
//!
//! The contract (from `gen_synth_board.py` / `ground_truth.json`): `F_Cu` gains a
//! 10×10 pad block (added), `In1_Cu` loses one (removed), every other layer is
//! byte-identical and must diff to ∅.

mod support;

use std::path::Path;
use support::*;

/// Per-pad expected area through our 64-gon tessellation.
fn expected_block_area() -> f64 {
    pad_area_mm2() * (BLOCK * BLOCK) as f64 * ngon_factor()
}

#[test]
fn synthetic_recovers_known_delta() {
    let board = gen_synth_board(16, 8, 2.0);
    let tol = 0.01 * expected_block_area(); // 1%

    let mut checked_added = false;
    let mut checked_removed = false;

    for (name, a_text) in &board.rev_a {
        let b_text = &board.rev_b[name];
        let a = polygonize_gerber(a_text).expect("revA polygonize");
        let b = polygonize_gerber(b_text).expect("revB polygonize");
        let d = diff_layer(&a, &b);

        match name.as_str() {
            "F_Cu" => {
                assert_eq!(d.added_regions(), BLOCK * BLOCK, "F_Cu added region count");
                assert!(
                    (d.added_area() - expected_block_area()).abs() <= tol,
                    "F_Cu added area {} vs expected {} ±{}",
                    d.added_area(),
                    expected_block_area(),
                    tol
                );
                assert_eq!(d.removed_regions(), 0, "F_Cu must have nothing removed");
                checked_added = true;
            }
            "In1_Cu" => {
                assert_eq!(
                    d.removed_regions(),
                    BLOCK * BLOCK,
                    "In1_Cu removed region count"
                );
                assert!(
                    (d.removed_area() - expected_block_area()).abs() <= tol,
                    "In1_Cu removed area {} vs expected {} ±{}",
                    d.removed_area(),
                    expected_block_area(),
                    tol
                );
                assert_eq!(d.added_regions(), 0, "In1_Cu must have nothing added");
                checked_removed = true;
            }
            _ => {
                assert!(
                    d.is_empty(),
                    "unchanged layer {name} diffed non-empty: +{} / -{} regions",
                    d.added_regions(),
                    d.removed_regions()
                );
            }
        }
    }
    assert!(
        checked_added && checked_removed,
        "both delta layers must be present"
    );
}

#[test]
fn synthetic_self_diff_is_empty() {
    // diff(A, A) = ∅ on every layer — the sharpest no-false-positive guard.
    let board = gen_synth_board(16, 8, 2.0);
    for (name, a_text) in &board.rev_a {
        let a = polygonize_gerber(a_text).expect("polygonize");
        let d = diff_layer(&a, &a);
        assert!(d.is_empty(), "diff(A,A) non-empty on {name}");
    }
}

#[test]
fn synthetic_diff_is_deterministic() {
    // Same inputs → byte-identical contours (the trust bar requires stable CI output).
    let board = gen_synth_board(8, 6, 2.0);
    for (name, a_text) in &board.rev_a {
        let b_text = &board.rev_b[name];
        let a = polygonize_gerber(a_text).expect("a");
        let b = polygonize_gerber(b_text).expect("b");
        assert_eq!(
            diff_layer(&a, &b),
            diff_layer(&a, &b),
            "non-deterministic diff on {name}"
        );
    }
}

#[test]
fn committed_corpus_matches_ground_truth() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/synthetic");
    let (rev_a, rev_b, gt) = (
        root.join("revA"),
        root.join("revB"),
        root.join("ground_truth.json"),
    );
    if !(rev_a.is_dir() && rev_b.is_dir() && gt.is_file()) {
        eprintln!(
            "SKIP committed_corpus_matches_ground_truth: {} not populated \
             (gitignored — run corpus/tools/gen_synth_board.py to enable)",
            root.display()
        );
        return;
    }

    let ground: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&gt).unwrap()).expect("parse ground_truth");
    let pad_area = ground["pad_area_mm2"].as_f64().unwrap();
    let block_pads = ground["block_pads"].as_u64().unwrap() as usize;
    let expect_changed_area = pad_area * block_pads as f64 * ngon_factor();
    let tol = 0.01 * expect_changed_area;

    // Build name → geometry for both revs from the on-disk .gbr files.
    let load = |dir: &Path| -> std::collections::BTreeMap<String, PolygonSet> {
        let mut m = std::collections::BTreeMap::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "gbr").unwrap_or(false) {
                let stem = p.file_stem().unwrap().to_str().unwrap();
                let name = stem.strip_prefix("synth-").unwrap_or(stem).to_string();
                let text = std::fs::read_to_string(&p).unwrap();
                m.insert(
                    name,
                    polygonize_gerber(&text).expect("polygonize committed corpus"),
                );
            }
        }
        m
    };
    let a = load(&rev_a);
    let b = load(&rev_b);
    assert_eq!(a.len(), b.len(), "rev layer-set mismatch");

    let changed = ground["expected_changed"].as_object().unwrap();
    let unchanged: Vec<&str> = ground["expected_unchanged"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    for (name, ca) in &a {
        let cb = &b[name];
        let d = diff_layer(ca, cb);
        if let Some(spec) = changed.get(name) {
            let kind = spec["kind"].as_str().unwrap();
            let (area, regions) = match kind {
                "added" => (d.added_area(), d.added_regions()),
                "removed" => (d.removed_area(), d.removed_regions()),
                other => panic!("unknown kind {other}"),
            };
            assert_eq!(regions, block_pads, "{name} {kind} region count");
            assert!(
                (area - expect_changed_area).abs() <= tol,
                "{name} {kind} area {area} vs {expect_changed_area} ±{tol}"
            );
        } else if unchanged.contains(&name.as_str()) {
            assert!(d.is_empty(), "unchanged {name} diffed non-empty");
        }
    }
}
