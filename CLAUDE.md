# CLAUDE.md — etchy

Project context for Claude Code sessions (incl. WSL). Read the three planning
docs in `docs/` before substantive work — they are authoritative.

## What etchy is

A fast, trustworthy, open-source **PCB visual + geometric diff** tool, written
ground-up in **Rust**. Successor to the Python tool gerber-diff (frozen at
`Cimos/Gerber-Diff-Tool` v0.11). `etchy old/ new/` → crisp SVG overlay + change
heatmap + JSON magnitudes, on any Gerber/Excellon fab pack.

**Status: Phase 0 (scaffold).** Crates are std-only so the workspace builds
without a local toolchain; real dependencies land per-module from Milestone 1.

## Decisions that constrain the work (from `docs/PRODUCT_DISCOVERY.md`)

- **Audience:** open-source community. **Core job:** fast "what changed?".
- **Surfaces:** CLI/CI-first; native **egui** GUI second (both in M1).
- **Diff:** per-layer polygon boolean (`added=B−A`, `removed=A−B`) → SVG overlay +
  heatmap + magnitudes (area, region count) from ONE computation.
- **Same-board revisions only** — fixed-point absolute coords, no auto-align; fail
  loud on mismatch.
- **Trust bar is top-tier: no silent misses.** Golden corpus + property + fuzz
  tests; fail-loud over wrong-but-quiet.
- **Inputs:** Gerber RS-274X/X2, Excellon, schematic PDF (pixel). **Non-goals:**
  native CAD ingestion (KiCad/Altium/IPC-2581/ODB++), net/connectivity diff,
  BOM/component diff, DRC. Hold this line — don't drift into an ECAD-review tool.
- **Distribution:** static binaries + distroless container. Not pip, not OS pkg mgrs.

## Layout

| Crate | Role |
|---|---|
| `etchy-core` | engine: parse → resolve → polygonize → diff → measure → render. **Pure logic** (no I/O policy). |
| `etchy-cli` | binary `etchy` — primary CLI/CI surface. Exit codes: 0 no diff · 1 diff · 2 error. |
| `etchy-gui` | native egui viewer (separate binary; never compiled headless). |
| `etchy-pdf` | schematic-PDF pixel diff (feature-gated; keeps the heavy PDFium dep out of core). |

## Conventions

- `etchy-core` stays pure: no `println!`, no `process::exit`, no path policy.
- Geometry is fixed-point (deterministic diffs).
- Permissive deps only — `deny.toml` blocks copyleft; `cargo deny check` before adding one.
- The verified crate stack (i_overlay, gerber-parser, eframe, pdfium-render,
  askama, serde_json, rstar/petgraph, tiny-skia, cargo-dist/zigbuild, proptest/
  insta/cargo-fuzz) is in `docs/DEVELOPER_GUIDE.md` — use those, don't reinvent.
- Build/test/run + WSL setup: `CONTRIBUTING.md`.

## Next work

`docs/ROADMAP.md` Phase 0 → M1. Immediate: **Spike 1** — see
[`docs/SPIKE_1.md`](docs/SPIKE_1.md) for the precise spec with docs.rs-verified
`i_overlay` 7.0 + `gerber-parser` 0.5 APIs, success criteria, test-board steps,
and the open decisions to resolve. Then Spike 2 (Gerber/Excellon parse
validation) and the golden-corpus harness.
