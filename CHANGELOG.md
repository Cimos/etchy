# Changelog

All notable changes to etchy are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Phase-0 scaffold: Cargo workspace (`etchy-core`, `etchy-cli`, `etchy-gui`,
  `etchy-pdf`), dual MIT/Apache-2.0 license, CI (build + test matrix on
  Linux/macOS/Windows), and the planning docs (roadmap, developer guide,
  product discovery).
- Core IR types: `geo::{Pt, Aperture, Primitive}`, `model::{Layer, LayerKind}`,
  `diff::LayerChange`, `report::DiffReport` (+ `SCHEMA_VERSION`).
- `etchy` CLI exit-code contract (0 = no diff, 1 = diff found, 2 = error).
- Phase-0 de-risking spikes (throwaway, dev-deps only):
  - **Spike 1** (`examples/spike1.rs`): proves the `i_overlay` 7.0 polygon-diff
    engine — correctness vs synthetic ground truth, ~2.1 s / ~373 MB on a dense
    16-layer board, deterministic. **GO.** See `docs/SPIKE_1.md`.
  - **Spike 2** (`examples/spike2.rs`): validates `gerber-parser` 0.5 + scopes the
    hand-rolled Excellon parser over a 520-file multi-vendor corpus (0 panics;
    full modern-feature coverage; gaps catalogued). **GO.** See `docs/SPIKE_2.md`.
- **Golden-corpus harness** (the trust backbone, `crates/etchy-core/tests/`):
  self-contained synthetic golden tests vs exact ground truth, `proptest`
  invariants (`diff(A,A)=∅`, add/remove symmetry, area bounds, determinism),
  and fuzz-lite "never panics on arbitrary input" — wired into CI via `cargo test`.
- **Milestone 1 — engine + CLI (first vertical slice).** The spike-proven
  pipeline promoted into a real `etchy-core` library and a working `etchy` CLI
  (design: `docs/M1_ENGINE_DESIGN.md`):
  - Engine: Gerber bytes → `parse_gerber` (graphics-state resolve, **flash of
    circle/rect apertures**) → `polygonize` → `diff_layer` (i_overlay **i64
    integer** engine; `added = B−A`, `removed = A−B`) → measure (exact `i128` nm²
    + region count) → `compare` (same-board guard, pair-by-kind, `DiffReport`).
    Coordinates quantized mm → i64 nm via a guarded `quantize_mm`. Core stays
    pure (no I/O / path / `println!` / `exit`).
  - **Fail-loud trust bar:** every unrendered feature (traces/regions/arcs/macros/
    obround/polarity/drilled apertures, compact multi-command-per-line Gerber,
    non-positive apertures, parse errors) is a typed `EngineError`, never a
    silent or wrong-but-quiet diff.
  - CLI `etchy <old> <new>`: dir walk + content-sniff + filename→`LayerKind`
    classify (I/O lives in the CLI), terminal summary, `--json` report
    (`schema_version = 1`, exact nm² as strings), exit codes 0/1/2.
  - The Phase-0 golden/property harness retargeted onto the shipping library;
    new `etchy-cli/tests/cli.rs` pins the exit-code + JSON contract.
  - Hardened against an adversarial review pass (overflow cap, duplicate-layer-kind
    rejection, compact-line detection, sniff robustness, empty-layer status).
  - Scope deferred to later increments: line/region/arc/macro polygonization, the
    Gerber `*`-tokenization shim, Excellon, SVG/HTML/heatmap render, the egui GUI.

### Notes
- Crates are std-only at Phase 0 so the workspace builds without a local
  toolchain; the verified real dependencies (clap, i_overlay, eframe,
  pdfium-render, askama, …) are added per-module starting in Milestone 1.
