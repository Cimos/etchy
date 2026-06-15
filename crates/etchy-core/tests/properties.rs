//! Property tests — the invariants that make the diff *trustworthy* (ROADMAP
//! principle 1, DEVELOPER_GUIDE "Property tests"). These hold for *every* input,
//! not just the golden corpus, so they catch whole classes of silent-miss bugs.
//!
//! Invariants:
//!   * `diff(A,A) = ∅`                          — no false positives
//!   * `removed(A,B) = added(B,A)`              — add/remove symmetry
//!   * areas are non-negative & finite          — measurement sanity
//!   * disjoint additions are recovered exactly — no missed changes
//!   * the diff is deterministic                — stable CI output
//!   * the parser never panics on arbitrary input — fuzz-lite (see note at end)

mod support;

use proptest::prelude::*;
use support::*;

/// Pads on a grid coarser than the pad diameter ⇒ each cell's pad is disjoint
/// from every other, so unions/differences reason exactly on area & count.
const PITCH: f64 = 2.0; // mm, ≫ 0.5 mm pad diameter

/// A layer = a set of distinct grid cells (deduped → no coincident pads).
fn layer_strategy() -> impl Strategy<Value = Vec<(i32, i32)>> {
    prop::collection::hash_set((0i32..16, 0i32..16), 0..24).prop_map(|s| {
        let mut v: Vec<_> = s.into_iter().collect();
        v.sort();
        v
    })
}

proptest! {
    /// diff(A,A) must be empty for any layer A.
    #[test]
    fn self_diff_is_empty(cells in layer_strategy()) {
        let a = pad_layer(&cells, PITCH);
        let d = diff_layer(&a, &a);
        prop_assert!(d.is_empty(), "diff(A,A) not empty for {} pads", cells.len());
    }

    /// Add/remove symmetry: removed(A,B) == added(B,A), byte-for-byte.
    #[test]
    fn add_remove_symmetry(a_cells in layer_strategy(), b_cells in layer_strategy()) {
        let a = pad_layer(&a_cells, PITCH);
        let b = pad_layer(&b_cells, PITCH);
        let ab = diff_layer(&a, &b);
        let ba = diff_layer(&b, &a);
        prop_assert_eq!(&ab.removed, &ba.added, "removed(A,B) != added(B,A)");
        prop_assert_eq!(&ab.added, &ba.removed, "added(A,B) != removed(B,A)");
    }

    /// Areas are always non-negative and finite.
    #[test]
    fn areas_non_negative_finite(a_cells in layer_strategy(), b_cells in layer_strategy()) {
        let d = diff_layer(&pad_layer(&a_cells, PITCH), &pad_layer(&b_cells, PITCH));
        prop_assert!(d.added_area() >= -1e-9 && d.added_area().is_finite());
        prop_assert!(d.removed_area() >= -1e-9 && d.removed_area().is_finite());
    }

    /// Disjoint additions are recovered exactly: if B = A ⊎ extra (extra cells
    /// disjoint from A), then removed = ∅ and added = exactly the extra pads.
    #[test]
    fn disjoint_additions_recovered(
        base in layer_strategy(),
        extra_raw in prop::collection::hash_set((0i32..16, 0i32..16), 0..12),
    ) {
        use std::collections::HashSet;
        let base_set: HashSet<_> = base.iter().copied().collect();
        // Shift `extra` far away so it cannot coincide or overlap with base.
        let extra: Vec<(i32, i32)> = extra_raw.into_iter().map(|(x, y)| (x + 100, y)).collect();
        prop_assume!(extra.iter().all(|c| !base_set.contains(c)));

        let a = pad_layer(&base, PITCH);
        let mut b_cells = base.clone();
        b_cells.extend(extra.iter().copied());
        let b = pad_layer(&b_cells, PITCH);

        let d = diff_layer(&a, &b);
        prop_assert_eq!(d.removed_regions(), 0, "nothing should be removed");
        prop_assert_eq!(d.added_regions(), extra.len(), "added region count != extra pads");

        // Actual is an nm-quantized 64-gon; allow a small band around the ideal.
        let expected = pad_area_mm2() * extra.len() as f64 * ngon_factor();
        prop_assert!(
            (d.added_area() - expected).abs() <= 1e-3 * expected + 1e-6,
            "added area {} vs expected {}", d.added_area(), expected
        );
    }

    /// The diff is deterministic — identical inputs yield identical output.
    #[test]
    fn diff_is_deterministic(a_cells in layer_strategy(), b_cells in layer_strategy()) {
        let a = pad_layer(&a_cells, PITCH);
        let b = pad_layer(&b_cells, PITCH);
        prop_assert_eq!(diff_layer(&a, &b), diff_layer(&a, &b));
    }

    /// Fuzz-lite: the polygonizer/parser must never panic on arbitrary bytes —
    /// only return Ok or a clean Err (the "no silent misses / no crashes" bar).
    #[test]
    fn polygonize_never_panics_on_arbitrary_input(s in ".{0,400}") {
        let _ = polygonize_gerber(&s); // must not panic; result is irrelevant here
    }
}

/// Structured-but-malformed Gerber snippets must also be handled without panic.
#[test]
fn polygonize_handles_malformed_gerber_without_panic() {
    let cases = [
        "",
        "M02*\n",
        "%FSLAX46Y46*%\n",                                // header only
        "%FSLAX46Y46*%\n%MOMM*%\nD10*\nX0Y0D03*\nM02*\n", // flash, no aperture def
        "%MOMM*%\nG36*\nM02*\n",                          // region (unsupported → clean Err)
        "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\nX0Y0D01*\nM02*\n", // draw (unsupported)
        "garbage not a gerber file at all",
    ];
    for c in cases {
        let _ = support::polygonize_gerber(c); // no panic; Ok or clean Err
    }
}
