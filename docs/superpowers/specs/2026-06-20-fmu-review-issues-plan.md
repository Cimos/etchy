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

### G2 · Rendering gaps — features not drawn (#3, #4) — **ROOT CAUSE FOUND + FIXED**
- [x] **#3** Some tracks not rendering. **#4** Missing footprints.
- **Root cause (confirmed):** `resolve_layer` resolved polarity as one
  order-independent `dark − clear` set difference, but Gerber polarity is
  **sequential** (a later `LPD` repaints over an earlier `LPC`). The FMU copper
  layers go pour-dark → clear-anti-pads → traces-dark-again; the single pass
  subtracted those clears from the later traces too → trace-shaped voids.
- **Fix:** process objects in paint order as polarity **spans** — dark unions
  copper on, clear subtracts it (gerber.rs `spans` + boolean.rs `union`). Verified:
  new test `polarity_is_sequential_later_dark_repaints`; existing polarity/golden/
  property tests still green; FMU GTL restored +28 mm² / +27 shapes (the erased
  traces), confirmed by rasterizing etchy's own mesh.

### G1 · Diff accuracy — phantom / hairline diffs (#1, #5, #6) — **DONE (detect + warn)**
- [x] Warn when the two revisions were exported with mismatched **units/precision**.
- **Root cause (confirmed):** not a positional offset — the FMU revs were exported
  by different Altium versions with different coordinate systems (REV4 `in@2.5`,
  REV67 `mm@4.4`). Identical geometry quantizes onto different grids → a hairline
  rim around every feature → inflated totals that are "nothing" zoomed in.
- **Implemented:** `etchy_core::gerber_format` (parses `%MO`/`%FS`) +
  `coordinate_mismatch_warning`; surfaced in `DiffReport.warnings`, attached by the
  CLI + GUI loaders, shown in the CLI output and a GUI top-bar "heads-up:" banner.
  Tests `gerber_format_parses_units_and_digits`, `coordinate_mismatch_warns_only_on_difference`;
  verified live on the FMU pack. A surfaced sub-tolerance merge stays a possible
  later step; auto-align remains a non-goal.

### G3 · Overlap visualization (#2, #8) — **DONE (base + colors)**
- [x] Always-available faint base (Off/Faint/Strong, `S` cycles, default Faint) so
  unchanged copper stays visible instead of being lost in black — the real #8 fix.
- [x] User-configurable added/removed colors via a "Colors" popover (Altium-compare
  style), default brand green/red. `base_alpha`/`cycle_base` pure + unit-tested.
- Dropped from the design: an "overlap = added ∩ removed" treatment. The engine's
  `added = B−A` / `removed = A−B` are disjoint by construction, so that set is always
  empty — it was a non-problem. Per-layer color overrides deferred (global for now).

### G1b · Auto-hide the units warning (#27) — **DONE**
- [x] The warning shows briefly then collapses to a clickable copper "heads-up" chip;
  click re-expands it as a floating overlay (no canvas reflow); auto-hides after
  `AUTO_HIDE_SECS`. Never silently gone. Pure `warning_phase` kernel, wasm-safe timing.

### G10 · Board outline on all layers (#24) — **DONE**
- [x] Edge.Cuts/GKO outline drawn faintly under every layer for orientation; "board
  edge" checkbox / `E` toggle; skipped when the outline layer itself is selected.
  Pure `pick_outline_index`/`outline_legend_visible` + unit tests.

### G6 · Native viewer performance (#15, #17)
- [ ] Native drag/scroll laggy; the wasm build is smooth.
- **Likely cause:** native runs software GL on WSL (`LIBGL_ALWAYS_SOFTWARE=1`) and
  rebuilds the full ~10k-triangle mesh every frame. **Fix:** cache the tessellated
  mesh (rebuild only on layer/zoom change); allow hardware GL where present; profile.

### G4 · Side-by-side comparison view (#9)
- [ ] A Before | After split (and/or swipe), in addition to the overlay.

### G5 · Layer parsing & grouping, KiCad + Altium (#10) — **DONE**
- [x] Left panel grouped into Copper / Soldermask / Silkscreen / Paste / Drill /
  Mechanical / Other (fixed order, changed-first within each). Classification keys
  off the engine's normalized `LayerKind`, so KiCad + Altium both work. Pure
  `layer_group`/`group_layers` with unit tests. Drill still needs Excellon (separate).

### G9 · Zoom level-of-detail (#22, #29) — **DONE**
- [x] Replaced the fixed-3px marker clamp (the cause of the zoomed-out green blob /
  blob-vs-blurb). Diff features draw true-to-scale and fade to nothing as they go
  sub-pixel. A heatmap variant was tried and rejected; pure fade is what shipped.
- [x] Surfaced min-area noise threshold as a top-bar slider (0..0.002 mm², 0 = off);
  the caption reports how many regions are hidden. Pure `geometry_alpha`/
  `ring_area_nm2` kernels with unit tests.

### G7b · UI controls (#12, #13, #16)
- [x] **#13** Hotkeys: `↑/↓` (or `J/K`) cycle layers · `O/B/A` switch view ·
  `S` toggle base · `F` fit. Pure `step_in_order()` for the cycling (unit-tested),
  egui input glue for the rest. (The on-canvas hint line was later dropped in G7c
  as clutter — #23.)
- [ ] **#12** Pan X/Y with Ctrl / Shift + scroll.

### G7c · Top-bar layout + branding (#28, #16, #23) — **DONE**
- [x] etchy egui theme (board-dark panels, copper accents on selection/hover).
- [x] Logo + copper "etchy" wordmark top-left (#16). Logo image native-only
  (`eframe::icon_data` isn't on wasm); web shows the wordmark.
- [x] Title row = wordmark · revisions · changed-area totals; control row given
  larger hit targets (#28). Dropped the cluttered keyboard-hint line (#23).
- Backlog (from review): move the noise-filter slider into a future VSCode-style
  settings menu (not a File/Edit menubar). Optional wasm logo via an image loader.

### G8 · Feedback-widget polish (#14)
- [ ] Overlay/thumbnail doesn't settle while the user keeps interacting.
  (Multi-screenshot paste + Ctrl+Enter already shipped.)

## Next GUI batch (designed via workflow 2026-06-21) — build order: G6 → G4 → per-layer colors

- **G6 · native perf** — cache the tessellation in world space (the expensive,
  camera-independent step); re-run only the cheap world→screen transform per frame,
  so pan/zoom and color edits never re-triangulate. Build first (the lag complaint).
- **G4 · side-by-side** — Before | After split, one shared camera, draggable divider.
- **Per-layer colors (RE-SCOPED per review)** — NOT per-layer added/removed overrides.
  Give the **base/context layer its own layer-type colour** (copper→copper-gold,
  silk→cream, mask→green-ish, etc.); added/removed stay green/red globally. The global
  add/removed colour pickers (G3) stay — Simon likes those.
- **Settings/help menu — DEFERRED ("down the track").** Eventually the noise slider,
  colour pickers, base level, toggles all move into a proper VSCode-style settings/help
  menu. Not now; keep them on the bar for the moment.

## Notes

- Work builds on the `gui-web-wasm` viewer; that branch still needs its own
  pre-PR fixes (public demo committed + staged by `setup.sh`, web CI).
- The FMU boards are confidential — kept local only (gitignored `assets/demo/`),
  never committed.
