# etchy-gui shell redesign — implementation plan

File-level plan for the VS Code-style shell redesign of `etchy-gui`. The design
is **locked** by the owner; this document maps each locked feature onto concrete
changes in `crates/etchy-gui/src/main.rs` (functions, structs, state fields),
suggests a PR slice per feature, and a test strategy. Refs #57.

All line references are to `crates/etchy-gui/src/main.rs` at the time of writing
and are anchors, not exact addresses — grep the named symbol.

## Where the relevant code lives today

| Concern | Symbol(s) | Approx. location |
|---|---|---|
| App shell / per-frame layout | `impl eframe::App for ViewApp` → `fn ui` | ~2101 |
| Top bar (title + controls) | `egui::Panel::top("top")` | ~2225 |
| Wordmark (top bar) | `RichText::new("etchy").size(24.0)` | ~2231 |
| Wordmark (splash) | `fn splash_ui` | ~2058 |
| Mode segment | `segmented(ui, &mut self.mode, …)` | ~2295 |
| Base off/faint/strong segment | `segmented(ui, &mut self.base_level, …)` | ~2306 |
| Layers panel | `egui::Panel::left("layers")` | ~2385 |
| Per-layer colour swatch | `ui.color_edit_button_srgba(&mut sw)` inside the row | ~2575 |
| Board-edge row (Layers) | `if self.outline.is_some() { … "board edge" … }` | ~2661 |
| Board-edge legend chip | `fn legend` → `rows.push((C_OUTLINE_FAINT, "board edge"))` | ~4022 |
| Settings window + rail | `if self.show_settings { egui::Window::new("Settings") … }` | ~2686 |
| Settings sections | `settings_display/diff/grid/input/colours/layers` | ~2780–2945 |
| Measure state + tool | `measure_mode`, `measure_pts`, `measure_unit`, `snap_grid`, `show_grid`, `grid_mm` | struct ~800; draw ~3399 |
| Measure hotkey (Ctrl+M today) | shortcut block in `fn ui` | ~2155 |
| Export | `build_export` / `do_export` / `export_msg` | ~934 / ~961 |
| Persistence | `struct Settings`, `to_settings`, `apply_settings` | ~1457 / ~1605 / ~1637 |
| `BaseLevel` enum + helpers | `BaseLevel`, `base_display_color`, `cycle_base`, `GeomKey.base_on` | 273 / 352 / 378 / 404 |

## Guardrails

- Do **not** touch swipe / camera / pan / Fit code (`Camera`, `split_rects`,
  `swipe_rects`, `fit`, `scroll_to_camera_action`, `dragging_pans`). Those are
  owned by another branch. The rail work only reshapes chrome around the
  `CentralPanel` that hosts `draw_canvas`.
- `etchy-core` stays pure — every change here is GUI-only.
- Each slice must keep `cargo build -p etchy-gui --target wasm32-unknown-unknown`
  green (the rail is drawn identically on web and native).
- Keyboard shortcuts live in one block in `fn ui`; new ones (M to arm measure)
  go there, and must stay suppressed while `egui_wants_keyboard_input()`.

---

## Feature 1 — VS Code activity rail + side panels (the shell)

The riskiest, highest-churn piece. Everything else slots into the panels this
creates, so land it first but behind a clean seam.

**New state (on `ViewApp`):**

- `active_panel: Option<PanelTab>` — which side panel is expanded; `None` =
  collapsed (rail-only, canvas full width). Runtime-only, not persisted.
- `rail_side: RailSide` — `Left` | `Right`, persisted (see Feature 8).

**New types:**

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelTab { Layers, Measure, Export, Help }

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum RailSide { #[default] Left, Right }
```

`Settings` (cog, bottom) stays its own `egui::Window` for now — it already has a
working left-rail of `SettingsTab`s and a nested colour picker that a docked panel
would complicate. The cog just flips `show_settings`.

**Layout change in `fn ui`:** replace the single `Panel::left("layers")` with:

1. A slim rail panel — `egui::SidePanel::left("rail")` (or `right`, chosen by
   `rail_side`) with a fixed width (~48px), `.resizable(false)`. Contents top to
   bottom: the **E monogram** (Feature 7), one icon button per `PanelTab`, a
   flexible spacer (`ui.allocate_space` / bottom-anchored layout), and the
   **Settings cog** pinned at the bottom.
2. A content panel — a second `SidePanel` on the same side, shown only when
   `active_panel.is_some()`, `.resizable(true).default_width(260.0)`, dispatching
   on the active tab to `layers_panel_ui` / `measure_panel_ui` /
   `export_panel_ui` / (Help = the `help_links` list inline).
3. The existing `CentralPanel` hosting `draw_canvas` stays last so it fills the
   remainder.

**Rail icon behaviour:** clicking an icon that is not active opens that panel;
clicking the active icon collapses it (`active_panel = None`). Selected icon reads
brand copper (`C_COPPER`), matching the Settings rail accent already in use.
Because egui has no built-in icon font, draw glyph-free marks with the painter
(same approach as `eye_toggle`, ~3762) or use short text labels — do **not** rely
on font glyphs (the bundled font renders `●`/arrows as tofu; see the `#16`/`#30`
notes in the code).

**Extract, don't rewrite, the panel bodies:** move the current Layers panel body
into `fn layers_panel_ui(&mut self, ui)` verbatim first (pure cut/paste), so the
diff that introduces the rail is layout-only and reviewable. The Measure and
Export panels start as thin stubs, filled by Features 3 and 4.

**`rail_side` flip:** reading `rail_side` picks `SidePanel::left` vs `right`. A
small helper avoids duplicating the panel-build closure for the two sides.

**Test strategy:** the rail is pure layout — no new pure kernels except maybe a
`rail_panels(rail_side) -> (which side)` trivial mapping, not worth a test. Cover
this slice with a manual/visual pass (screenshot both `rail_side` values, panel
open + collapsed). Keep the extracted panel bodies behaviourally identical so the
existing layer/visibility tests still pass unchanged.

**PR slice:** *PR A — rail + panel extraction.* Layout-only: rail, panel dispatch,
Layers moved into `layers_panel_ui`, Measure/Export as stubs, cog still opens the
Settings window. **Needs visual review.** This is the load-bearing slice; keep it
free of behaviour changes so review is about the shell alone.

---

## Feature 2 — Top bar shrink (remove base control)

**Change in the top bar controls row (~2290):** drop the `base` label + the
`segmented(&mut self.base_level, …)` block entirely. The base control moves into
the Layers panel as a slider (Feature 6).

The bar keeps: board label + revisions (title row, ~2231), the **mode segment**
(`segmented(&mut self.mode, …)`), and the right cluster reduced to **Open / Fit /
Help**. Remove **Measure, Export, Settings** from the top-bar action cluster and
the `More` menu — those are now rail tabs/cog:

- Delete the `toggle_measure`, `exp_current`, `exp_all`, `toggle_settings`
  intents and their buttons from both the wide branch and the `More` branch
  (~2318–2400) and the matching act-on-intent block (~2412–2425). Keep `fit` and
  `open_side`.
- With three controls left, the `collapse_actions` / `More` tier may be
  unneeded; the `TIER_MORE_PX` / `TIER_LABELS_PX` logic (~3730) can be simplified,
  but keep a `More` fallback if the window can still get narrow enough to clip
  Open/Fit/Help. Re-tune the tier constants rather than deleting them blindly.

**Test strategy:** no pure kernels change; visual review that the bar no longer
wraps at narrow widths and that Open/Fit/Help still work. `publish_state`
(bottom of `fn ui`) is unaffected.

**PR slice:** *PR C — top-bar shrink.* Small, but it depends on Measure/Export
living in the rail (PR A) and the base slider existing (PR D), so land it after
those. **Light visual review.**

---

## Feature 3 — Measure tab

Today measure is a single in-progress ruler (`measure_pts`, max 2 points, drawn at
~3399) toggled from the top bar with **Ctrl+M**. The redesign turns it into a
first-class tab with a persistent list of results.

**New state:**

- `measurements: Vec<Measurement>` where
  `struct Measurement { a: [f64; 2], b: [f64; 2] }` (world nm/mm units matching
  `measure_pts`). The completed rulers; `measure_pts` stays as the in-progress
  0/1/2-point buffer.
- `show_crosshair: bool` — crosshair toggle (the crosshair draw already exists at
  ~3399–3433; gate it on this flag). Persisted (Feature 8).
- `snap_grid`, `measure_unit`, `show_grid`/`grid_mm` already exist — the tab
  surfaces them (snap-to-grid toggle, units mm/mil, and the crosshair toggle).
  Note the locked spec says units **mm/mil**; the enum is `Unit { Mm, Inch, Mil }`
  (~4066). Keep `Inch` in the type (used by `format_distance`) but the tab's unit
  control can offer mm/mil per the spec, or all three — confirm; lowest-risk is to
  show all three since the formatter already handles them.

**Arming from the rail (works while the panel is collapsed):** clicking the
Measure **rail icon** must set `measure_mode = true` (arm), independent of whether
the Measure panel is expanded. So the icon's click handler both opens the panel
*and* arms measure; re-clicking the active icon disarms + collapses. This is the
one rail icon with a side effect beyond show/hide — document it inline.

**"M" hotkey:** add `M` (no modifier) to the shortcut block (~2155) to
toggle/arm measure, alongside or replacing the current Ctrl+M. Reuse the existing
`measure_escape` cascade (369) for Esc. Guard with `egui_wants_keyboard_input()`
so typing in a DragValue doesn't arm it.

**Completing a measurement:** in `draw_canvas` where the second click lands
(~3056, `if self.measure_pts.len() >= 2 { clear }`), instead push a finished
`Measurement { a, b }` onto `measurements` and clear `measure_pts` for the next
one. Keep the snap applied at click time.

**Drawing:** the existing single-ruler draw (~3434–3455) becomes a loop over
`measurements` (draw each segment + its distance label via `format_distance` /
`measure_label`) plus the in-progress `measure_pts`. `distance_mm` (~4058) and
`format_distance` are reused unchanged.

**Panel contents (`measure_panel_ui`):**
- snap-to-grid toggle (`snap_grid`), crosshair toggle (`show_crosshair`), grid
  toggle + spacing (`show_grid` / `grid_mm`), unit selector (`measure_unit`).
- a scrollable list of `measurements`: each row shows its distance
  (`format_distance(distance_mm(a,b), unit)`) + endpoints, with a **delete**
  (small ✕/"del" button) that removes that index; a **Clear all** button that
  empties the vec; an **Export** button (Feature 4 plumbing) that writes the list.

**Test strategy (pure kernels — TDD these first):**
- A `measure_push(pts: &mut Vec<[f64;2]>, done: &mut Vec<Measurement>, click)` or
  simpler: a pure helper `finish_measurement(a, b) -> Measurement` plus a test
  that two clicks produce one `Measurement` and reset the buffer. Extract the
  click→state transition into a pure fn so it is unit-testable off-screen.
- `delete`/`clear` are trivial vec ops but a small test that delete removes the
  right index and clear empties guards regressions.
- `distance_mm` / `format_distance` tests already exist — extend
  `format_distance_per_unit` only if the tab changes unit options.
- Visual review for the crosshair toggle, list rendering, and arm-while-collapsed.

**PR slice:** *PR E — Measure tab.* Depends on PR A (needs the panel host).
Pure-logic parts are TDD'd; the arming/drawing is **medium visual review** (the
canvas ruler behaviour changes from single to list).

---

## Feature 4 — Export tab

Today export is two menu actions calling `do_export(all_layers)` (~961), with a
transient `export_msg`. Turn it into a panel that previews *what* will be exported
and *which formats*.

**Panel contents (`export_panel_ui`):**
- A **list of what will be exported**: derive it from `build_export(all_layers)`
  (~934), which already returns the per-layer SVG set + `areas.csv`. Show file
  names/count for both "current layer" and "all changed layers" so the user sees
  the set before committing.
- **Formats**: today it is SVG-per-layer + CSV (copper mm²). List these
  explicitly. If measurement export (Feature 3) is added, list that file here too.
- The two existing actions (Current layer / All changed layers) become buttons in
  the panel; keep `export_msg` as the transient status line, shown in-panel.

**Measurement export plumbing:** add a `build_measurement_export(&self) ->
ExportFile` (CSV of `measurements` with distance + endpoints) reusing the
`exportio` module already used by `build_export`. Wire it to both the Measure
tab's Export button and the Export tab's list.

**Test strategy:** `build_export` is already exercised indirectly; add a pure test
that `build_measurement_export` emits one row per measurement with correct
distances (reuse `distance_mm`). The preview list is a pure function of
`build_export` output → unit-test the *count/name* derivation if extracted; the
rendering is light visual review.

**PR slice:** *PR F — Export tab.* Depends on PR A. Low risk (mostly surfaces
existing `build_export`); **light visual review.** If Feature 3 lands first, fold
the measurement CSV in here; otherwise keep it optional.

---

## Feature 5 — Remove the board-edge concept entirely

The board edge is the Mechanical/`Edge.Cuts`/GKO outline, already drawn as a
reference. The locked design removes it as a *separate concept* (the Mechanical
outline layer already conveys it).

**Delete:**
- The Layers-panel board-edge row (~2661–2678): the `if self.outline.is_some()`
  block with its faint-copper swatch + `checkbox(&mut self.show_outline, "board
  edge")`.
- The legend chip: in `fn legend` (~4022) drop the
  `rows.push((C_OUTLINE_FAINT, "board edge"))` branch and the `outline_row`
  parameter; update the single call site (~3388) accordingly.
- The `toggle_outline` (O) keyboard shortcut in `fn ui` (~2194) and its entry in
  the shortcut tuple.

**Decide the fate of `show_outline` / `outline` / `C_OUTLINE_FAINT`:** the
outline is currently painted faintly *on every layer* for orientation via
`GeomKey.outline_effective` (~417), `build_cache(…, self.outline)` (~3153), and
`push_context_items`. The locked ask is to remove the *board-edge concept* — the
row and the legend chip. Two readings; pick with the owner:
1. **Minimal (recommended):** remove only the user-facing board-edge *control*
   and *legend chip*, and hardwire `show_outline = true` so the Mechanical outline
   still draws as orientation context (it is no longer a toggleable "board edge",
   just always-on context). Keeps `outline`, `pick_outline_index`,
   `outline_legend_visible`, and the cache path intact — smallest, safest diff.
2. **Full removal:** also stop drawing the faint outline entirely — delete
   `show_outline`, `outline`, `pick_outline_index`, `outline_legend_visible`,
   `C_OUTLINE_FAINT`, the `outline_effective` field on `GeomKey`, the
   `build_cache` outline arg, and `push_context_items`' outline handling. Larger,
   touches the tessellation cache key and its tests (`geom_key_*`,
   `pick_outline_index_finds_the_first_outline_layer`,
   `outline_legend_visible_*`).

Recommend #1 unless the owner wants the faint outline gone from the canvas too.
Either way the **row and legend chip go**.

**Test strategy:** if #1, delete `outline_legend_visible_*` only if the fn is
removed; otherwise leave the pure tests. If #2, remove the now-dead pure tests and
adjust `geom_key_tracks_selection_inputs_only` / `geom_cache_dirty_*` for the
dropped `outline_effective` field. Visual review that the Mechanical outline still
reads correctly (or is gone, per the chosen reading).

**PR slice:** *PR B — remove board edge.* Independent of the rail; can land early.
Reading #1 is **low risk / light review**; reading #2 needs a **cache-path review**
because it edits `GeomKey`.

---

## Feature 6 — Base opacity slider in the Layers panel

Replace the off/faint/strong `BaseLevel` segment with a continuous opacity slider
living in the Layers panel.

**Recommended: make base opacity continuous.** Replace `base_level: BaseLevel`
with `base_opacity: f32` (0.0..=1.0):
- `base_display_color` (352) currently maps `BaseLevel` → `t ∈ {0.0, 0.4, 0.8}`;
  change its signature to take `t: f32` directly (the blend math is unchanged).
- `GeomKey.base_on` (~414) becomes `base_opacity > 0.0`.
- Delete `cycle_base` (378) and its test `cycle_base_rotates_off_faint_strong`,
  and remove the `toggle_base` (B) shortcut path (~2155) — or repurpose B to
  nudge opacity; simplest is to drop it.
- Add the slider to `layers_panel_ui` (e.g. a labelled `egui::Slider::new(&mut
  self.base_opacity, 0.0..=1.0)` reading "base"), near the view-mode row.

**Persistence migration:** `Settings.base_level: BaseLevel` (serde string repr)
becomes `base_level` → keep the field name or add `base_opacity: f32`. Because
`#[serde(default)]` is on `Settings`, an old persisted `base_level` string is
simply ignored if the field is renamed — acceptable (users lose one stored
preference once). If you want a clean migration, keep reading the old field via a
`#[serde(alias)]`/manual map: off→0.0, faint→0.4, strong→0.8. Update `to_settings`
/ `apply_settings` and `settings_serde_round_trips`.

**Alternative (lower risk, less true to the ask):** keep `BaseLevel` as a 3-stop
enum but render it as a 3-notch slider. Rejected — the locked design says a
slider replacing the segment, and a continuous value is the natural fit.

**Test strategy (TDD):** update `base_display_color_dims_toward_canvas` to the
`f32` signature; add a case that `base_opacity == 0.0` yields the canvas colour and
`1.0` yields the layer colour. Update `geom_key_tracks_selection_inputs_only`
(`base_on` derivation) and `settings_serde_round_trips`.

**PR slice:** *PR D — base opacity slider.* Depends on nothing structural (can
precede the rail), but the slider lands in the Layers panel body, so sequencing it
after PR A keeps the panel edits in one place. **Light visual review**; the enum→
float change is mechanical and test-covered.

---

## Feature 7 — Small square colour swatches per layer

Per-layer swatches today use `color_edit_button_srgba` (~2575), which renders a
rounded button. The locked design wants **small square swatches**.

**Change:** in the layer row, replace the raw `color_edit_button_srgba` with a
small painted **square** that opens the colour picker on click. Follow the
existing painted-rect pattern (`eye_toggle` at ~3762, and the legend squares at
~4025 use `rect_filled(… vec2(12.0,12.0) …)`). Options:
- Allocate a ~14×14 square with `allocate_exact_size`, `rect_filled` it with the
  swatch colour, and open an egui colour popup on click (egui exposes
  `color_picker` popups; or keep `color_edit_button_srgba` but shrink/square it
  via `ui.spacing_mut().interact_size` + rounding = 0). Simplest that stays
  square: wrap the swatch in a fixed-size cell and set corner rounding to 0.
- Apply the same square treatment to the swatches in `settings_layers` (~2913)
  for consistency.

**Test strategy:** pure visual — the swatch colour still comes from
`resolve_base_color` (tested by `resolve_base_color_prefers_override`), so no
logic change. Screenshot review that swatches are square and the picker still
opens and records a `base_overrides` entry.

**PR slice:** fold into *PR A* (Layers panel is already being touched there) or a
tiny follow-up. **Light visual review.**

---

## Feature 8 — Branding: E monogram replaces the wordmark

**Two sites, both currently the "etchy" wordmark:**
- Top bar (~2231): `RichText::new("etchy").size(24.0).strong().color(C_COPPER)`.
- Splash (`splash_ui`, ~2058): the large wordmark + tagline.

**Change:** replace the top-bar wordmark with an **E monogram** — either a
copper-on-dark painted mark (a small `rect`/glyph drawn with the painter so it
never depends on a font glyph) or a single "E" `RichText` in the brand copper at
the same size. The monogram also becomes the rail's top mark (Feature 1). Keep the
splash as-is or reduce it to the monogram — confirm with the owner; the locked ask
is specifically "E monogram replaces the wordmark", so the top bar + rail are the
required sites, splash is optional.

Update the comment at `run()` (~69) that calls the "etchy" wordmark "the single
logo" if the wordmark is fully retired.

**`RailSide` persistence (the flip):** add `rail_side: RailSide` and (from Feature
3) `show_crosshair: bool` and (Feature 6) `base_opacity` to `struct Settings`
(~1457), `to_settings` (~1605), `apply_settings` (~1637), and `Settings::default`.
Extend `settings_serde_round_trips` (~4852) with the new fields. `RailSide` needs
a stable serde repr like `BaseLevel`/`InputPreset` (string enum). The flip control
itself is a toggle in Settings → Display (`settings_display`, ~2780).

**Test strategy:** `RailSide` serde round-trip (add to the enum round-trip tests
next to `input_preset_serde_round_trips`). Monogram is visual review.

**PR slice:** monogram → fold into *PR A* (rail already needs the top mark). The
`rail_side` flip + Settings toggle can ride with PR A or a small *PR G*. **Light
visual review.**

---

## Recommended PR sequence

| PR | Scope | Depends on | Risk / review |
|---|---|---|---|
| **A** | Activity rail + panel host, Layers moved into `layers_panel_ui`, E monogram (top + rail), square swatches, `rail_side` + flip | — | **High — the riskiest piece.** Layout-only, no behaviour change, so review is about the shell. Needs visual review on native + wasm, both rail sides, panel open/collapsed. |
| **B** | Remove board-edge row + legend chip (+ optional full outline removal) | — (independent) | Low (reading #1) / medium (reading #2, edits `GeomKey`). |
| **D** | Base opacity slider (`BaseLevel` → `base_opacity: f32`) in Layers panel | A (panel body) | Low; mechanical + test-covered. Light visual review. |
| **E** | Measure tab: `measurements` list, M hotkey, arm-from-rail, list draw/delete/clear | A | Medium; TDD the pure transitions, visual review the canvas ruler change. |
| **F** | Export tab: preview list + formats, measurement CSV | A (+ E for the CSV) | Low; surfaces existing `build_export`. Light review. |
| **C** | Top-bar shrink to board label + mode + Open/Fit/Help | A (Measure/Export moved out), D (base moved out) | Low; light visual review, re-tune tier constants. |
| **G** *(optional)* | Splash reduced to monogram; comment cleanup | A | Cosmetic. |

**Land A first** — it defines the seams every other slice plugs into. B and D are
the safest and can be reviewed quickly. C must come last because it removes
controls that only have a home once A/D exist. E is the only slice that changes
canvas behaviour (single ruler → list), so give it the most testing after A.

## Risks & notes

- **The rail/tab restructure (PR A) is the riskiest piece** — it rewrites the
  top-level panel layout in `fn ui` and moves the whole Layers body. Keep it
  strictly layout-only (verbatim body extraction, stub Measure/Export) so a
  reviewer can reason about the shell without behaviour drift. Behaviour changes
  ride in B–G.
- egui 0.34 has no icon font and the bundled font renders many symbols as tofu
  (see `#16`/`#30` in-code). Draw rail icons/monogram/swatches with the painter or
  plain text, never symbol glyphs.
- Measure arming must work with the panel collapsed — the rail icon and the M
  hotkey both set `measure_mode`, decoupled from `active_panel`.
- The `BaseLevel` → float change touches `GeomKey` (the tessellation cache key);
  verify pan/zoom still skip re-triangulation (`geom_cache_dirty`) and that
  `base_on` still gates correctly. Do not otherwise touch cache/camera code.
- Persistence: new fields (`rail_side`, `base_opacity`, `show_crosshair`) go
  through `Settings` + `to_settings`/`apply_settings`; `#[serde(default)]` keeps
  old saved settings loadable. Extend `settings_serde_round_trips`.

## Local verification per PR

- `cargo test -p etchy-gui`
- `cargo clippy -p etchy-gui --all-targets` (warning-clean)
- `cargo fmt --all --check`
- `cargo build -p etchy-gui --target wasm32-unknown-unknown`
