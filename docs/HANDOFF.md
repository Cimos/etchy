# etchy — handoff (resume on another machine)

**Branch:** `gui-web-wasm` (local) → pushed to **`gui-web-wasm-v2`** = **PR #10** (supersedes #7).
**As of:** 2026-06-22. Pick up by `git fetch && git checkout gui-web-wasm-v2`.

## How to build / run / test
- `cargo test` (workspace), `cargo clippy`, `cargo fmt` — keep all green/clean.
- Native viewer: `cargo run --release -p etchy-gui -- <old-dir> <new-dir>` (WSL: `DISPLAY=:0 LIBGL_ALWAYS_SOFTWARE=1`).
- Web demo: `bash deploy/setup.sh` → http://localhost:8080 (needs a demo board staged in the gitignored `crates/etchy-gui/assets/demo/{old,new}`).
- CLI: `etchy --format <summary|json|md> old/ new/` (exit 0 no-diff / 1 diff / 2 error).
- **Confidential:** the CubeOrange+ FMU board lives only in the gitignored `assets/demo/` — never commit it. Local review feedback (`deploy/feedback/*.jsonl` + screenshots) is gitignored too.

## Done (M1 viewer + M2 start)
G1 units-mismatch warning · G2 sequential polarity · G7a/G5 layer list + grouping · G7b hotkeys + Ctrl/Shift-scroll pan · G9 zoom true-scale fade + noise slider · G7c branding · G1b warning chip · G10 board outline · G3 base levels + configurable colours · per-layer colours (all kinds) · dark/light mode · G6 world-space tessellation cache · **perf: one merged mesh/frame + off-screen cull** · M2: `--format md` + `action.yml` GitHub Action (script-injection-hardened). Landing site salvaged from #7; tabbed install from #9.

## DONE — A–D landed this run (gui-web-wasm-v2)
All four clusters from the autonomous finishing run are implemented, build green
(native + wasm), 23 unit tests pass, clippy + fmt clean. The visual items (C/D
appearance, and the A/B render look) are **pending Simon's eyes** — no live-window
screenshot is available on this box (llvmpipe software GL), so correctness was
build- and test-verified, not visually confirmed. Some native lag is inherent to
llvmpipe and won't fully match the GPU web build.

### A. Diff-render artifacts — DONE (53d4863)
- MSAA enabled in `native::run` (`NativeOptions { multisampling: 4, .. }`) for #55/#47.
- New unit-tested kernel `feature_thickness_nm(area, extent)`; `transform_cache`
  now feeds a **thickness proxy** (not bbox extent) into `lod::geometry_alpha`, so
  thin diff crescents stop flickering on pan (#46).

### B. Split view — DONE (57e7630)
- #44: `build_cache` Split arm gates base pushes on `key.base_on`; draw applies
  `base_display_color` per `base_level` (off/faint/strong now live in Split).
- #45: `Side::Full` (outline) appended to BOTH halves with `C_OUTLINE_FAINT`.
- #48: Split side-labels moved to each half's bottom-left (no overlap with caption).

### C. Warning chip — DONE (7016a32)
- #53: dropped tofu glyphs (`⚠`/`☾☀`) for ASCII markers (`[!]`, `theme: dark|light`).
- #51: `warning_label` now names the warning subject, not a bare count.
- #49: chip folded into the controls row so the top-panel height never reflows.

### D. Theme / controls polish — DONE (7016a32)
- #57: `layer_type_color` gained theme-aware dark/light variants (silk/mask/paste/
  drill read on the cream board); `resolve_base_color` now takes the active theme.
- #52: noise slider switched from `.logarithmic(true)` to linear.

## Remaining roadmap
- **M2** finish: release binaries (cargo-dist) so `action.yml` doesn't build from source; docs.
- **Finalize PR #10:** commit a PUBLIC non-confidential demo board (replace the gitignored FMU embed in `demo_diff()`), have `setup.sh` stage it, add web CI; then it's mergeable. Pre-launch: trim `site/docs.html` viewer-modes copy (split/swipe/onion not all shipped). Close #7 and #9 (superseded/salvaged).

## Conventions (also ~/.claude/CLAUDE.md)
Commits/PRs: clean, plain, **no AI attribution, no emoji**. etchy-core stays pure (no I/O). Permissive deps only (`cargo deny`). Brand: copper `#e8a33d`, board `#0b0f0e`, surface `#141a18`, cream `#f4f1e8`; green `#46d18a`/red `#ff5d73` reserved for added/removed.
