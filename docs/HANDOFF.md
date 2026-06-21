# etchy — handoff (resume on another machine)

**Branch:** `gui-web-wasm` (local) → pushed to **`gui-web-wasm-v2`** = **PR #10** (supersedes #7).
**As of:** 2026-06-21. Pick up by `git fetch && git checkout gui-web-wasm-v2`.

## How to build / run / test
- `cargo test` (workspace), `cargo clippy`, `cargo fmt` — keep all green/clean.
- Native viewer: `cargo run --release -p etchy-gui -- <old-dir> <new-dir>` (WSL: `DISPLAY=:0 LIBGL_ALWAYS_SOFTWARE=1`).
- Web demo: `bash deploy/setup.sh` → http://localhost:8080 (needs a demo board staged in the gitignored `crates/etchy-gui/assets/demo/{old,new}`).
- CLI: `etchy --format <summary|json|md> old/ new/` (exit 0 no-diff / 1 diff / 2 error).
- **Confidential:** the CubeOrange+ FMU board lives only in the gitignored `assets/demo/` — never commit it. Local review feedback (`deploy/feedback/*.jsonl` + screenshots) is gitignored too.

## Done (M1 viewer + M2 start)
G1 units-mismatch warning · G2 sequential polarity · G7a/G5 layer list + grouping · G7b hotkeys + Ctrl/Shift-scroll pan · G9 zoom true-scale fade + noise slider · G7c branding · G1b warning chip · G10 board outline · G3 base levels + configurable colours · per-layer colours (all kinds) · dark/light mode · G6 world-space tessellation cache · **perf: one merged mesh/frame + off-screen cull** · M2: `--format md` + `action.yml` GitHub Action (script-injection-hardened). Landing site salvaged from #7; tabbed install from #9.

## OUTSTANDING — finish these (root-caused; see also docs/superpowers/specs/)
Each touches `crates/etchy-gui/src/main.rs`; pure logic → unit-tested kernels (pattern in `lod.rs`). Verify geometry by rasterising the mesh to PNG (no live-window screenshot available here).

### A. Diff-render artifacts (BLOCKERS) — NOT a geometry bug (triangulation proven exact)
- **"track+pad shows as no-copper" (#55) & "missing square in identical footprint" (#47):** software-GL aliasing of thin slivers/shared edges. Fix: enable **MSAA** in `native::run` — `eframe::NativeOptions { multisampling: 4, ..}`. (The merged-mesh change already removed per-shape seams.)
- **Pan makes added/removed flicker by position (#46):** thin diff crescents have large bbox extent but sub-pixel thickness, so the LOD metric (extent-based) mis-rates them. Fix: in `push_diff_items` also store a **thickness proxy** (e.g. `4*area/perimeter`, or min-bbox-side) and feed THAT (not extent) to `geometry_alpha`. + MSAA.

### B. Split view (#44/#45/#48)
- **#44** base off/faint/strong inert in Split: the Split loop hardcodes `color: base_color`. Make it honour `base_level` (gate base pushes on `key.base_on` in `build_cache` Split arm; apply `base_display_color`).
- **#45** outline missing in Split: the Split loop does `Side::Full => continue`, dropping the outline. Draw `Side::Full` items into BOTH halves.
- **#48** label overlap: Split side-labels and the per-layer caption both draw at top-left. Move Split side-labels to each half's bottom-left.

### C. Warning chip (#49/#51/#53)
- **#53** tofu glyphs: no custom font is installed, so `⚠` / `☾☀` render as tofu in egui's default font. Use **ASCII + copper colour/weight** only (no symbol glyphs) — or install a font with the glyphs.
- **#51** concise + a proper name: `warning_label` emits a bare count; give it the warning's subject (the single coordinate-mismatch case → name it).
- **#49** still reflows/obtrusive: reserve the chip's slot **unconditionally** (or move it into the controls row) so the top panel height never changes whether warnings exist or not.

### D. Theme / controls polish (#52/#57)
- **#57** light mode hard to read: retune `chrome(Theme::Light)` for contrast; layer-type colours (cream silk on a cream board) need theme-aware variants.
- **#52** noise slider feels non-linear: it's `.logarithmic(true)` — try linear, or a gentler curve.

## Remaining roadmap
- **M2** finish: release binaries (cargo-dist) so `action.yml` doesn't build from source; docs.
- **Finalize PR #10:** commit a PUBLIC non-confidential demo board (replace the gitignored FMU embed in `demo_diff()`), have `setup.sh` stage it, add web CI; then it's mergeable. Pre-launch: trim `site/docs.html` viewer-modes copy (split/swipe/onion not all shipped). Close #7 and #9 (superseded/salvaged).

## Conventions (also ~/.claude/CLAUDE.md)
Commits/PRs: clean, plain, **no AI attribution, no emoji**. etchy-core stays pure (no I/O). Permissive deps only (`cargo deny`). Brand: copper `#e8a33d`, board `#0b0f0e`, surface `#141a18`, cream `#f4f1e8`; green `#46d18a`/red `#ff5d73` reserved for added/removed.
