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

### Notes
- Crates are std-only at Phase 0 so the workspace builds without a local
  toolchain; the verified real dependencies (clap, i_overlay, eframe,
  pdfium-render, askama, …) are added per-module starting in Milestone 1.
