# etchy — FMU review issues: plan

**Date:** 2026-06-20 · **Status:** approved (via in-app feedback review)
**Source:** 17 issues filed through the web demo's in-app feedback widget while
diffing **CubeOrange+ FMU REV 4 ↔ REV 67** (real Altium fab packs).

## Decisions (from review)

- **Order:** work top-down through the workstreams below.
- **G1 (phantom diffs):** first step is **detect + warn on a global offset** between
  the two exports — not auto-align (a non-goal), not a silent tolerance.
- **Tracking:** this doc (one section per workstream; check items off as done).

## Workstreams (in priority order)

### G7a · Quick UI wins — **do first** (#7, #11)
- [ ] **#7** Remove the area/region mm² figures from the left layer list (declutter).
- [ ] **#11** Drop the `□` glyph shown before layer names.
- Viewer-only (`etchy-gui`); low risk, immediately visible.

### G2 · Rendering gaps — features not drawn (#3, #4) — trust-critical
- [ ] **#3** Some tracks not rendering. **#4** Missing footprints.
- **Hypothesis:** traces/region fills not polygonized for some layers, or a layer
  that fails to parse is *silently skipped* by the web build (`board_from_files`
  swallows per-layer errors → a whole layer vanishes quietly).
- **First step:** instrument a per-layer parse over the FMU pack; list what fails or
  is skipped **loudly** (no silent misses), then fix the offending handling.

### G1 · Diff accuracy — phantom / hairline diffs (#1, #5, #6) — confirmed
- [ ] Detect a global offset / coordinate-precision mismatch between the two boards
  and **warn** ("revisions appear offset by ~N µm — diff may be registration").
- **Root cause (confirmed via screenshots):** features are ~identical between revs
  but ringed by a hairline red/green rim; summed over thousands of features this
  inflates the mm² totals while being "nothing" zoomed in.
- Detect-and-warn first; a *surfaced* sub-tolerance threshold is a possible later
  step (never silent). Full auto-align stays a non-goal.

### G3 · Overlap visualization (#2, #8)
- [ ] Added (green) + removed (red) overlap renders black/occluded; unchanged copper
  invisible without "show base".
- [ ] Per-layer colors + **user-configurable colors** (Altium-compare style); a
  distinct overlap treatment; an always-available faint base. Ties to brand-theme.

### G6 · Native viewer performance (#15, #17)
- [ ] Native drag/scroll laggy; the wasm build is smooth.
- **Likely cause:** native runs software GL on WSL (`LIBGL_ALWAYS_SOFTWARE=1`) and
  rebuilds the full ~10k-triangle mesh every frame. **Fix:** cache the tessellated
  mesh (rebuild only on layer/zoom change); allow hardware GL where present; profile.

### G4 · Side-by-side comparison view (#9)
- [ ] A Before | After split (and/or swipe), in addition to the overlay.

### G5 · Layer parsing & grouping, KiCad + Altium (#10)
- [ ] Group the layer list into sections (copper / mask / silk / paste / drill / mech);
  robust classification for both Altium (extension) and KiCad (suffix). Drill needs
  Excellon — still unsupported (separate).

### G7b · UI controls (#12, #13, #16)
- [ ] **#12** Pan X/Y with Ctrl / Shift + scroll. **#13** Hotkeys (e.g. `S` toggles
  base). **#16** Golden etchy mark/wordmark top-left (brand-theme).

### G8 · Feedback-widget polish (#14)
- [ ] Overlay/thumbnail doesn't settle while the user keeps interacting.
  (Multi-screenshot paste + Ctrl+Enter already shipped.)

## Notes

- Work builds on the `gui-web-wasm` viewer; that branch still needs its own
  pre-PR fixes (public demo committed + staged by `setup.sh`, web CI).
- The FMU boards are confidential — kept local only (gitignored `assets/demo/`),
  never committed.
