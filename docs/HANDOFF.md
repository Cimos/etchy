# etchy — handoff (resume on another machine)

**As of:** 2026-06-25. Everything is on **`main`**; `git fetch && git checkout main`.

## Latest (2026-06-25) — #49 Phase 3 merged (multi-layer + swipe) + review feedback; perf design doc open

**State of `main`** (HEAD `1c0c30c`): builds + tests green locally (47 gui tests, clippy
clean, fmt clean; native + wasm). Merged this session, in order:
- **#71** viewer feedback fixes: ASCII `->` arrow (was tofu, #30), per-theme canvas/grid
  colours (#31), collapsible layer-group headers (#36), measure Esc cascade (#50),
  input presets trimmed to Altium/KiCad with Altium default (#54/#55).
- **#73** Phase 3 multi-layer (#58/#59): render multiple layers at once, per-layer +
  per-group visibility checkboxes, Show all / Hide all, selected layer highlighted /
  others dimmed (0.4α). Group headers collapse (combined with #36 via `CollapsingState`).
- **#74** Phase 3 swipe/curtain compare (#61): draggable vertical divider, old left /
  new right. **Also folds in the round of review feedback** (11 items, below).
  Superseded the auto-closed #72 (its base branch was deleted on the #73 merge).

**Review feedback (11 items) — all addressed in #74, verified green + screenshot:**
swipe freeze fixed (the outline `NO_LAYER = usize::MAX` sentinel was indexing the layer
list in the split/swipe colour lookup → panic, wedging wasm); select no longer
auto-ticks visibility; "Show changed" hidden (API kept); group rows indented under the
header; top controls wrap when narrow; colour-list scrollbar to far right; inline layer
swatch opens its colour picker; Esc backs out of the Colours window; dropped the
"input:" dropdown prefix; adaptive base-copper LOD when zoomed out. **Simon chose "PR/
commit trail is enough"** — no separate tracking issues opened for these.

**⏳ Performance (#9/#10) — DESIGN STAGE, no code yet.** Dense multi-layer boards (the
real board, 24+ layers) are slow to pan/zoom. The base-LOD shipped in #74 **did not help
at working zoom** (Simon confirmed "no real change") — LOD only removes already-sub-pixel
features.
Root cause: `transform_cache` re-runs the world→screen transform of every visible
triangle on the CPU every frame (scales with layers, not zoom). **Decision: deeper
engine work — GPU-side transform.** Design doc **`docs/PERF_GPU_TRANSFORM.md`** + **PR
#75** are open and **awaiting Simon's review**. Hybrid plan: GPU-transform base copper
via an egui glow `PaintCallback`, keep diff items + their LOD on CPU; local-origin
precision fix; CPU fallback. **Four open decisions** for Simon (spike-first vs full
build; base-copper-only scope; bake-dimming vs uniform; keep CPU fallback). **Do not
start coding until those are answered.**

**Issues still open for Simon to verify + close** (PRs used `Refs`, not `Closes`): **#58,
#59, #61** (Phase 3 features). The #71-era issues (#30/#31/#36/#50/#54/#55) were closed
this session after his sign-off.

**Remaining roadmap:** implement perf per the #75 review → **Phase 4** (#60 export
SVG/PNG + copper-area · #57 UI polish/top-bar) → **Phase 5** (#62 drill parsing).
Follow-up: #68 (feedback widget → pre-filled GitHub issue).

**Live demo (this box):** the web viewer is served on **`localhost:8080`** showing a
**confidential real board, rev A → rev B** (24/26 layers changed). That board is
staged into the gitignored `crates/etchy-gui/assets/demo/{old,new}` and **must never be
committed** — to rebuild: copy `<local-path>/rev-a` and `<local-path>/rev-b` into
`assets/demo/{old,new}`, set the labels in `main.rs` (~line 191) to `rev A`/`rev B`,
`bash deploy/setup.sh --build-only --serve-dir <dir>`, serve
with `ETCHY_BIND=127.0.0.1 python3 <dir>/etchy-server.py 8080 <dir>`, then **restore**:
`git checkout -- crates/etchy-gui/assets/demo && git clean -fd crates/etchy-gui/assets/demo`
and revert the label edit. The committed demo board is the public Mad_RP2040.

**Branches:** `main` has everything. `docs/perf-gpu-transform` (PR #75) open. Earlier
working/agent branches are merged or superseded — safe to ignore.

**CI:** GitHub Actions on `main` has been green since 2026-08-01. `v0.1.0-rc1`
is published as a pre-release with four archives plus `SHA256SUMS`; public
`v0.1.0` is gated on completing `PRE_PUBLIC.md`.

---

## Latest (2026-06-24) — feedback round + #49 Phase 1–2 merged; main is integrated & green

**State of `main`:** builds + tests green locally (native + wasm, clippy clean,
fmt clean). Integrates, this session:
- **Render correctness:** #46 (zoom-stable LOD + marker dots), #48 (track→pad notch
  fix), and **#66 (pour-clearances regression fix** — #48's winding normalization had
  over-applied in `push()` and filled filled-region holes solid; now `wind()` is
  applied only to the macro Outline primitive, regions keep their holes). Visually
  verified on the real board (junctions solid; pours render with clearances).
- **#49 Phase 1 (#64):** measure-tool upgrades (crosshairs, sticky, off-line copper-chip
  label, mm/inch/mil units, Ctrl+M, Esc-clear, right/middle-drag pan in measure mode)
  + grid overlay (toggle, spacing, snap-to-grid with live cursor snap).
- **#49 Phase 2 (#65 + #69/#54):** persistent settings via eframe (native RON + web
  localStorage, no new deps); configurable canvas/grid colour; mode hotkeys 1/2/3/4;
  centered Colors window; per-ECAD input presets (EtchyDefault/KiCad/Altium → pan button).

**Merged PRs this session:** #45, #46, #47, #48, #43, #10 (earlier) → then #66, #65, #64,
#69 (=#54, superseded the auto-closed #67). All squash-merged; branches deleted.

**Historical CI note:** At the time of this 2026-06-24 entry, GitHub Actions runs
failed before starting jobs and merges were verified locally. That incident is
resolved: CI on `main` has been green since 2026-08-01 and is the merge gate.

**Headless visual verification now works** (this box, no sudo): Playwright + Chromium in
`~/.cache/etchy-shot-venv`; `~/.cache/etchy-shot-venv/bin/python ~/.cache/etchy-shot-venv/shot.py <url> <out.png> [wait_ms] [w] [h]`
screenshots the web viewer (serve a build, shot localhost). Used it to catch + confirm
the pour regression and verify the measure/grid features. Native egui window still can't
be captured. (See `~/.claude` memory `verify-native-gui-visuals`.)

### For Simon (your workflow: you verify + close issues)
- **Verify & close** the implemented issues — render: #12/#14 (diagnosed/fixed), #13
  (notch, verified); the fixed-pending set #28–#42; and the #49 Phase 1–2 issues now
  shipped: #50 #51 #52 #53 #55 #56 #54. (Fixing PRs used `Refs`, so they stayed open.)
- **Measure feedback** Ctrl+M / Esc / right-click-pan are implemented but only
  `pans_on`/`snap` kernels are unit-tested — **confirm those interactions live**.
- The earlier #15–#27/#44 backlog issues were already closed.

### Remaining #49 plan (NOT done — next up)
- **Phase 3:** #58 (hide a layer group — quick win) · #61 (swipe/curtain compare view) ·
  #59 (multi-layer view + highlight — large/future).
- **Phase 4:** #60 (export SVG/PNG for current/all layers + copper-area; PNG needs a
  deny.toml-cleared raster crate e.g. tiny-skia) · #57 (UI polish + top-bar wireframes).
- **Phase 5:** #62 (drill Excellon/NC parsing).
- **Follow-ups:** #54 full per-key remapper; #68 (feedback widget → pre-filled GitHub issue).

---

## Earlier — PR #10 era (history)

**Branch:** `gui-web-wasm-v2` = PR #10 (merged). Original resume notes below.

## Latest (2026-06-22) — #10 gate cleared: public demo board + web CI, all green

- **PR #10's last gate is done — CLEAN + MERGEABLE, all CI green.** Committed a
  public, non-confidential demo board (`crates/etchy-gui/assets/demo/{old,new}` =
  **Mad_RP2040 v0.0.0 → v0.0.1**, the PCBWay gerbers from the board's own GitHub
  release `pcb-datapack`s), un-ignored that path, and added a **web-build (wasm)
  CI job** (`trunk build`, trunk installed from its pinned prebuilt release). All
  4 jobs pass: build+test on ubuntu/macos/windows + the new wasm job. Site example
  updated to the real v0.0.0 → v0.0.1 figures (+235.3 / −395.3 mm², 8/11 layers).
- **Mad_RP2040 release pipeline fixed + exercised** (PR #18 there): `release.yaml`
  is now idempotent (create-or-clobber publish — fixes the failed v0.0.1 run) and
  `workflow_dispatch(tag)`-able for any tag. Dispatched clean release builds for
  **v0.0.0** (first ever) and **v0.0.1** (rebuilt) — both now carry fresh fab packs.
- **Known debug item:** etchy classifies the inner-copper files `.gl2/.gl3` as
  `other` (the layer classifier knows `.G1/.G2` but not KiCad's `.glN`). Demo works;
  small etchy-core classifier fix worth doing.
- **Private-board override:** `assets/demo` is now committed; to review a confidential
  board locally, swap files in and `git update-index --skip-worktree …` so they never
  stage (see `.gitignore` note). Never commit a private board.

## Earlier (2026-06-22) — #10 unblocked, branches cleaned, private review demo

- **PR #10 is now MERGEABLE.** Merged `main` into the branch and resolved the two
  add/add conflicts (`docs/PAGES_LAUNCH_CHECKLIST.md`, `site/docs.html`) to main's
  finished versions; `scripts/check-landing.py` passes. The "trim viewer-modes copy"
  task was already satisfied — `docs.html` lists only the shipped modes
  (Overlay/Before/After); the landing's split/swipe/onion mention sits in the roadmap
  section, which is fine.
- **Branches cleaned up.** Closed #7 (superseded). Deleted 8 merged/superseded remote
  branches (the 7 squash-merged docs/landing PRs + `gui-web-wasm`) and `gui-brand-theme`
  — its one unique commit (the GUI brand/theme design spec) was salvaged into this
  branch first (`de859e7`). Remotes left: `main`, `gui-web-wasm-v2`, `feedback-widget`.
- **Private LAN review demo (NOT committed) — a real production board, rev A vs rev B.**
  Built the web viewer for a co-worker to review on the office LAN. How it was made, for
  next time:
  - Source packs: `<local-path>/rev-a.zip` and `<local-path>/rev-b.zip` (both pack their
    files under the same base name, so layers pair by name).
  - **Stage only the electrical layers** into `assets/demo/{old,new}` —
    `GTL GBL G1..G6 GTS GBS GTO GBO GTP GBP TXT`. A rev B drawing/dimension layer spans
    to ~131mm (vs the ~34mm board) and trips the same-board guard, so the full pack
    won't diff; the 14 electrical layers do.
  - Set the web labels locally to `rev A` / `rev B` (**kept local — not
    committed**; the repo keeps the generic non-confidential labels). `trunk build
    --release --filehash false`, deploy to the Windows serve dir, serve with
    `deploy/demo/etchy-server.py` on `0.0.0.0:8080`.
  - **Feedback given:** Simon confirmed rev A vs rev B is the intended comparison — it
    is a near-total redesign (all 14 layers changed, ~+1120 / −638 mm², ~5,900/6,580
    regions; expected for a revision gap that wide). He had the demo **stopped** after
    review; relaunch on request.
- **Env note:** the office machine now drives reviews through the `feedback-loop` tool
  (`~/UbuntuProjects/feedback-loop`, SessionStart hook) and the merged `~/.claude/CLAUDE.md`
  agreements (Cimos identity, no AI attribution, never the word "canonical").

## How to build / run / test
- `cargo test` (workspace), `cargo clippy`, `cargo fmt` — keep all green/clean.
- Native viewer: `cargo run --release -p etchy-gui -- <old-dir> <new-dir>` (WSL: `DISPLAY=:0 LIBGL_ALWAYS_SOFTWARE=1`).
- Web demo: `bash deploy/setup.sh` → http://localhost:8080 (needs a demo board staged in the gitignored `crates/etchy-gui/assets/demo/{old,new}`).
- CLI: `etchy --format <summary|json|md> old/ new/` (exit 0 no-diff / 1 diff / 2 error).
- **Confidential:** the real production board lives only in the gitignored `assets/demo/` — never commit it. Local review feedback (`deploy/feedback/*.jsonl` + screenshots) is gitignored too.

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
- **Finalize PR #10 (now MERGEABLE — conflicts resolved, #7/#9 closed).** The one gate
  left is the PUBLIC non-confidential demo board: commit one (replace the gitignored
  real-board embed in `demo_diff()`), have `setup.sh` stage it, then add **web CI** — CI
  can't build the wasm until a committable board exists, so the two are coupled. Mad_RP2040 is the
  likely board (already the site's public example). Then merge. Native CI (`ci.yml`,
  build+test+clippy on 3 OSes) already passes and is board-independent.
- **M2** finish: release binaries (cargo-dist) so `action.yml` doesn't build from source; docs.

## Conventions (also ~/.claude/CLAUDE.md)
Commits/PRs: clean, plain, **no AI attribution, no emoji**. etchy-core stays pure (no I/O). Permissive deps only (`cargo deny`). Brand: copper `#e8a33d`, board `#0b0f0e`, surface `#141a18`, cream `#f4f1e8`; green `#46d18a`/red `#ff5d73` reserved for added/removed.
