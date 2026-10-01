# CLAUDE.md — etchy

Project context for Claude Code sessions (incl. WSL). Before substantive work
read **`docs/REQUIREMENTS.md`** (the consolidated numbered requirements with
status) and, for viewer work, **`docs/design/GUI_SPEC.md`** (the locked UI
behaviour spec). `docs/PRODUCT_DISCOVERY.md` / `docs/ROADMAP.md` /
`docs/DEVELOPER_GUIDE.md` carry the why, the sequencing, and the architecture.

## What etchy is

A fast, trustworthy, open-source **PCB visual + geometric diff** tool, written
ground-up in **Rust**. Successor to the Python tool gerber-diff (frozen at
`Cimos/Gerber-Diff-Tool` v0.11). `etchy old/ new/` → crisp SVG overlay + change
heatmap + JSON magnitudes, on any Gerber/Excellon fab pack.

**Status (2026-09):** engine/CLI/CI surfaces shipped through M2; PDF engine
and CLI wiring are merged; the GUI shell redesign is merged; CI on `main` is
green. `v0.1.0-rc1` is published as a pre-release; public `v0.1.0` is next.

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
- The verified crate stack (i_overlay, gerber-parser, eframe, hayro,
  askama, serde_json, rstar/petgraph, tiny-skia, cargo-dist/zigbuild, proptest/
  insta/cargo-fuzz) is in `docs/DEVELOPER_GUIDE.md` — use those, don't reinvent.
- Build/test/run + WSL setup: `CONTRIBUTING.md`.

## Next work

See `docs/ROADMAP.md` "Where we are" and `docs/REQUIREMENTS.md` for statuses.
Current front: fix native WSLg clicks (#203), then cut
public v0.1.0, and publish the container to ghcr.
