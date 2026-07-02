# Trust model & limitations

etchy's core promise: **a missed change is the worst possible outcome.** A tool
that quietly shows the wrong diff is worse than no tool. So etchy is built to
**fail loud** — if it can't render something faithfully, it says so and stops,
rather than emitting a wrong-but-quiet result.

This page states what that guarantee rests on, and — just as importantly — what
etchy deliberately does **not** do.

## What backs the guarantee

- **Fail-loud, never wrong-but-quiet.** Every feature the engine can't render
  faithfully becomes a typed error, not a silent drop: unsupported Gerber
  constructs, ambiguous Excellon headers, parse failures. The third-party Gerber
  parser runs inside a `catch_unwind` boundary, so even a panic on hostile input
  becomes a clean typed error, never a crash mid-batch.
- **Same-board guard.** etchy diffs *revisions of one board* using absolute
  coordinates — it does **not** auto-align. If the two inputs' extents differ by
  more than a small tolerance (1 mm, or 2 % of span), it refuses with a "not the
  same board" error instead of producing a garbage overlay. The guard compares
  **physical board layers only** — documentation drawings (whose legend tables
  legitimately move/grow between revisions) and placement markers are excluded,
  so a regenerated drawing template doesn't read as a different board.
- **Deterministic integer geometry.** All geometry is quantized to a 1 nm
  fixed-point grid and diffed with a validated integer polygon-boolean engine, so
  a given input pair always yields byte-identical results.
- **A golden corpus + property + fuzz tests, in CI.** Synthetic board pairs with
  exact known deltas are checked against ground truth; property tests assert the
  invariants that make a diff trustworthy — `diff(A, A) = ∅`, add/remove symmetry
  (`removed(A,B) = added(B,A)`), non-negative finite areas, and determinism; and a
  fuzz-lite pass asserts the parser never panics on arbitrary bytes.
- **Amplification limits.** Per-layer ceilings on emitted contours, total points,
  and polarity spans mean a tiny crafted file fails loud instead of exhausting a
  CI runner's CPU/RAM.

## Limitations (by design or not-yet)

- **Same-board revisions only.** No registration/auto-alignment. Diffing two
  genuinely different boards is out of scope and is rejected, not attempted.
- **Coordinate-format mismatch is flagged, not reconciled.** If the two revisions
  were exported with different `%FS`/unit settings, identical geometry quantizes
  onto different grids and produces spurious sub-µm "rim" differences; etchy
  **warns** about this rather than silently massaging it.
- **Excellon scope.** Drill hits (circles), `G85` canned slots, routed
  (`M15`…`M16` / `G01`) slots, `R` repeat codes, headerless files, inline digit
  formats, and feed/speed tool fields are supported. **Arc routing** (`G02`/`G03`
  rout) and **incremental coordinates** fail loud. Coordinates with
  zero-suppression but no declared `LZ`/`TZ` mode fail loud rather than guess
  hole positions.
- **Pick-and-place is placement geometry, not a BOM.** Each component is rendered
  as a marker at its centroid + rotation and diffed geometrically (moved / rotated
  / added / removed parts show up); etchy does **not** compare values, footprints,
  or nets. Coordinates are assumed millimetres, and all parts land on one
  `placement` layer (top/bottom sides aren't split yet).
- **Region counts use a small noise floor** so sub-nanometre tessellation slivers
  aren't counted as changes; the changed *area* is always exact.
- **Schematic-PDF diff is not implemented yet** (the `etchy-pdf` crate is a
  feature-gated stub; see the roadmap).
- **Explicit non-goals:** net/connectivity diff, BOM/component diff, DRC, and
  native Altium / IPC-2581 / ODB++ ingestion. Native KiCad `.kicad_pcb` ingestion
  is an accepted *post-1.0* goal.

## Found a silent miss?

That's the bug etchy most wants to hear about. Please open an issue with the two
fab packs (or a minimal reproduction) and what you expected — a change that etchy
shows as unchanged is treated as a correctness defect, not a feature request.
