# Future work — adaptive flash tessellation (#94, geometry variant)

**Status:** FUTURE / deferred. **Prerequisite:** tessellation-robust region count
(PR #96 / `region_count_above`) must be merged first. **Researched:** 2026-06-25 (a
throwaway spike on branch `research/adaptive-tess`; findings on issue #94).

## What & why

Every circular flash is tessellated to a **fixed 64-gon** (`geom.rs` `CIRCLE_SEGMENTS`)
regardless of radius, so a via-heavy board carries far more vertices than needed — the
dominant cost in the per-frame transform, the GPU upload, and the boolean diff. Making
the segment count **adaptive** (scale with radius under the existing ~1µm sagitta
tolerance, like arcs already do) cuts that.

Two variants exist; this doc is the **geometry** one (changes the resolved geometry,
so it speeds the *diff* too). The **render-only** variant already shipped (PR #95, the
faint base mesh only, no diff impact, ~2.2× pan) and does NOT need any of this.

## Measured benefit (real 26-layer board, vs fixed 64-gon)

| | Result |
|---|---|
| Diff wall-time | 3.50s → **2.61s (−26%)** |
| Peak memory | 122MB → **91MB (−25%)** |
| Total board area | −0.002% (negligible) |
| Region count (with PR #96 floor) | stable, ~2% (without #96 it swung ~40%) |

## The tradeoff (why it's deferred, not shipped)

Adaptive uses a coarser circle approximation for **small** features: a 0.5mm pad
becomes a ~36-gon instead of a 64-gon, so its area is **~0.3–0.5% lower** (further from
the true circle than the 64-gon's ~0.16%). Aggregate board area barely moves (−0.002%),
but individual small-feature areas shift ~0.3–0.5%. For a trust-bar-first tool that's a
real (if small) accuracy reduction to a reported magnitude, so it needs an explicit
decision — it is **not** a free optimization.

## Exact work to do

1. **Prerequisite:** merge PR #96 (robust region count). Without it the reported region
   count swings ~40% with tessellation; with it, ~2%. Adaptive is only transparent on
   top of #96.
2. **`crates/etchy-core/src/geom.rs`** — add `circle_segments(r)` (segment count from
   `r` under `SAG_TOL_NM`, clamped `[MIN, CIRCLE_SEGMENTS]`); use it in `ngon` and
   `stadium` (the spike's diff is ~15 lines). Decide `MIN` (spike used 8; consider 12–16
   for a rounder fidelity floor on tiny features — re-measure the speed/area effect).
3. **`CIRCLE_SEGMENTS` doc comment** — it currently says "64 … matches the golden-corpus
   ground-truth maths"; reword to "maximum/ cap" and drop the stale claim.
4. **Property test `disjoint_additions_recovered`** (`tests/properties.rs`) — it asserts
   the diff area equals `pad_area_mm2() * ngon_factor()`, where `ngon_factor()` hardcodes
   the 64-gon ratio. Make it **tessellation-agnostic**: compute the expected per-pad area
   from the *actual* built pad (`area_mm2` of one `pad_layer` pad) × count, so it still
   verifies exact recovery without baking in 64. (This is a correction, not a weakening —
   it still fails if recovery is wrong.) Check `ngon_area_close_to_circle` /
   `stadium_area_matches_rect_plus_disk` still pass (spike: yes, <1%).
5. **Golden corpus** — re-validate (spike: passed 4/4, because the generator uses the
   same builders so ground-truth + result move together). If any golden asserts exact
   64-gon areas, regenerate against the documented v0.11 tolerance and note it.
6. **Document the accepted accuracy change** — add a short note (here + DEVELOPER_GUIDE)
   that small-feature areas are a ~0.3–0.5% coarser circle approximation, deliberately
   traded for ~25% speed/memory. This is the trust-bar sign-off.
7. **Re-measure** speed/memory + area/region deltas on the golden board and a dense real
   board; confirm region count stays stable (~2%) and aggregate area within ~0.01%.

## Acceptance criteria

- `cargo test` green (golden + properties), clippy + fmt clean.
- Region count stable across tessellation (≤ a few %), area within ~0.01% aggregate.
- The accuracy tradeoff documented and explicitly accepted.
- Stacks cleanly with PR #77 (parallel diff) for a combined speedup.

## Effort

~0.5 day (the code is small + spiked; the work is the test update, golden
re-validation, the accuracy sign-off, and re-measurement). Reserved until the diff-time
need justifies the accuracy cost — note PR #77 (parallel diff, −20%, zero accuracy cost)
already covers much of the diff-speed need.
