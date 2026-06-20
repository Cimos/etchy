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
- [x] **#13** Hotkeys: `↑/↓` (or `J/K`) cycle layers · `O/B/A` switch view ·
  `S` toggle base · `F` fit. Pure `step_in_order()` for the cycling (unit-tested),
  egui input glue for the rest; hint line lists the keys.
- [ ] **#12** Pan X/Y with Ctrl / Shift + scroll. **#16** Golden etchy mark/wordmark
  top-left (brand-theme).

### G8 · Feedback-widget polish (#14)
- [ ] Overlay/thumbnail doesn't settle while the user keeps interacting.
  (Multi-screenshot paste + Ctrl+Enter already shipped.)

## Notes

- Work builds on the `gui-web-wasm` viewer; that branch still needs its own
  pre-PR fixes (public demo committed + staged by `setup.sh`, web CI).
- The FMU boards are confidential — kept local only (gitignored `assets/demo/`),
  never committed.
