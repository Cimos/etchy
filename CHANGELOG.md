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

### Notes
- Crates are std-only at Phase 0 so the workspace builds without a local
  toolchain; the verified real dependencies (clap, i_overlay, eframe,
  pdfium-render, askama, …) are added per-module starting in Milestone 1.
