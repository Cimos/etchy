# Top-bar rework — design scope

Scope for reworking the etchy-gui viewer top bar. Covers feedback **#57**
(segmented controls), **#154** (noise filter → Settings), **#157** (board edge →
Layers panel), **#160** (old/new naming + Open A/B clutter), and the
responsive/no-wrap requirement. Wireframe reference:
`docs/design/ui-wireframes.html` Part B, "Top-bar & mode controls" (#57) —
Simon approved that direction.

All code anchors are in `crates/etchy-gui/src/main.rs`. Line numbers shift;
anchor by the markers given (e.g. `egui::Panel::top("top")`).

---

## 1. Target layout

### Wide (>= ~1080 px available width)

```
+------------------------------------------------------------------------------------------------+
| etchy   boardA -> boardB                                    3/7 layers changed  +138.3 −58.5 mm²|   <- title row (unchanged)
+------------------------------------------------------------------------------------------------+
| ( Overlay | Old | New | Split | Swipe )   base ( off | faint | strong )   [!] warn  exported... |
|                                              [Open v] [Fit] [Measure] [Export v] [Settings] [Help v] |
+------------------------------------------------------------------------------------------------+
```

Rendered as ONE controls row (the two lines above are only for page width here):

```
| (Overlay|Old|New|Split|Swipe)  base (off|faint|strong)  [!]warn ...flex... [Open v][Fit][Measure][Export v][Settings][Help v] |
```

- `( a | b | c )` = a **segmented control**: one rounded pill-group, zero gap
  between segments, 8 px corner radius on the group, selected segment filled
  copper `#e8a33d` with board-dark text.
- Two separate segments: **mode** picker and **base** intensity (with a small
  muted `base` label before it).
- Actions float **right**, in this LTR order: `Open v · Fit · Measure ·
  Export v · Settings · Help v`. `v` = menu indicator (menu_button chevron).
- Flexible middle space carries the transient bits: warnings chip
  (`warnings_ui`, stays in-row per #49) and the transient export status label.

Gone from the bar entirely: `board edge` checkbox (→ Layers panel, #157),
`noise filter` slider + DragValue (→ Settings, #154), `Open A…`/`Open B…`
buttons (→ `Open v` menu, #160b), `GPU` checkbox (already duplicated in
Settings > Display — remove the bar copy).

### Medium (~880–1080 px)

Drop the muted `base` prefix label and the transient export-status text
(export feedback still visible via the Export menu / canvas); everything else
unchanged.

```
| (Overlay|Old|New|Split|Swipe) (off|faint|strong) [!]warn ...flex... [Open v][Fit][Measure][Export v][Settings][Help v] |
```

### Narrow (~640–880 px)

Right-hand actions collapse into a single **`More v`** menu (plain ASCII label
— egui's default font tofus on many glyphs, see #30). Menu contains, in order:
Open old revision… / Open new revision… / separator / Fit / Measure (checkable)
/ Export current layer / Export all changed layers / separator / Settings /
Help submenu.

```
| (Overlay|Old|New|Split|Swipe) (off|faint|strong) [!]warn ...flex... [More v] |
```

### Very narrow (< ~640 px)

The base segment also folds into `More v` (as three checkable items or a
submenu). The mode segment NEVER collapses — it is the core control and its
five short labels fit any plausible window.

```
| (Overlay|Old|New|Split|Swipe) [!]warn ...flex... [More v] |
```

The bar never wraps at any width: `horizontal_wrapped` is replaced by a plain
`horizontal` row whose contents are chosen per-frame from the width tier.

---

## 2. Component inventory

Every element currently in the controls row (`ui.horizontal_wrapped` inside
`egui::Panel::top("top")`), and its fate:

| Current element | Widget today | Fate |
|---|---|---|
| Overlay / Before / After / Split / Swipe | 5x `selectable_value` | **Keep in bar** — mode segmented control; rename Before→Old, After→New (#160a) |
| `base:` label + off / faint / strong | label + 3x `selectable_value` | **Keep in bar** — second segmented control |
| GPU checkbox (`gpu-transform` feature) | `checkbox` | **Remove from bar** — already lives in Settings > Display (`settings_display`); the bar copy is a duplicate |
| Settings toggle | `selectable_label` | **Keep in bar** — right cluster |
| `board edge` checkbox | `checkbox` (only when `self.outline.is_some()`) | **Move to Layers panel** (#157) — visibility entry, see below |
| Fit button | `button` | **Keep in bar** — right cluster |
| Export menu (Current layer / All changed layers) | `menu_button` | **Keep in bar** — right cluster |
| Export status label (`self.export_msg`) | weak small `label` | **Keep in bar** — flexible middle, next to warnings chip; dropped at medium tier |
| Measure toggle | `selectable_label` | **Keep in bar** — right cluster |
| noise filter slider + exact DragValue (`min_area_mm2`) | `Slider` + `DragValue` | **Move to Settings** (#154) — new **Diff** tab, see below |
| Open A… / Open B… buttons | 2x `button` | **Fold into menu** (#160b) — single `Open v` `menu_button`, items "Old revision…" / "New revision…" calling `open_primary(RevSide::Old/New)` |
| Help menu (links + version) | `menu_button` | **Keep in bar** — right cluster, rightmost |
| Warnings chip | `warnings_ui(ui, now)` | **Keep in bar** — flexible middle (per #49: never a separate row that reflows the canvas) |

Title row (wordmark, `{old_label} -> {new_label}`, right-aligned totals):
unchanged except naming (below).

### Landing spots for the moved items

- **Noise filter → Settings.** Add a `SettingsTab::Diff` variant (between
  `Display` and `Grid` in `SettingsTab::ALL`) with a copper `settings_header`
  "Diff", the existing linear `Slider` 0.0..=0.1 mm² + the unbounded
  `DragValue`, and the existing hover text. `min_area_mm2` is already a field
  and already persisted in the `Settings` struct — no storage change. Update
  the canvas hidden-count caption to say where the control went, e.g.
  `"3 regions hidden by noise filter (Settings > Diff)"`, and fix the
  Split/Swipe caption strings that mention the filter.
- **Board edge → Layers panel.** In the left `egui::Panel::left("layers")`,
  render (when `self.outline.is_some()`) a visibility row after the layer
  list — checkbox bound to `self.show_outline`, label `board edge`, a small
  swatch in `C_OUTLINE_FAINT`, keeping today's hover text. `show_outline`
  state and the `outline_legend_visible` logic are untouched.

---

## 3. Naming decision — one scheme: **old / new**

Standardize on **old / new** everywhere the two revisions are named.
Rationale: the CLI is already `etchy old/ new/`, the internal enum is
`RevSide::Old/New`, the split/swipe captions already say OLD/NEW, and the diff
semantics are "old revision vs new revision". "A/B" and "before/after" are
dropped as user-facing terms.

Exact labels per location:

| Place | Today | Becomes |
|---|---|---|
| Mode buttons | `Before`, `After` | `Old`, `New` |
| Open controls | `Open A…`, `Open B…` (hover "revision A (old)") | `Open v` menu → `Old revision…`, `New revision…` |
| Welcome-screen rows (`side_open_row`) | `Revision A (old)`, `Revision B (new)` | `Old revision`, `New revision` |
| File-picker dialog titles (`pick_folder`/`pick_zip`) | `Open revision A (old) — folder` etc. | `Open old revision — folder` / `Open new revision — .zip fab pack` |
| Title-row header | `{old} -> {new}` | unchanged (already old→new order); hover text "old revision -> new revision" |
| Split-mode canvas labels | `{label} (old)` / `{label} (new)` | unchanged |
| Mode captions | `showing OLD board (before)` / `showing NEW board (after)` | `showing OLD board` / `showing NEW board` |
| Legend | `added` / `removed` (+ `board edge`) | unchanged — added/removed describe the diff, not the revisions |
| `publish_state` mode string (web bridge) | `Before`/`After` | `Old`/`New` (check the web/demo side for consumers of this string before renaming) |

Keyboard shortcuts: keep `1–5` for modes. Keep the existing `B`/`A` letter
aliases working (unlabelled legacy mnemonics) so muscle memory survives —
`O` is taken by Overlay so no new letter mnemonics are added. (Default picked;
see Risks.)

---

## 4. Responsive behaviour — concrete rule

Replace `ui.horizontal_wrapped` with `ui.horizontal` and pick a **width tier**
per frame from `ui.available_width()` measured at the top of the controls row:

```
w >= 1080        -> Tier 0: everything, full labels
880 <= w < 1080  -> Tier 1: drop "base" prefix label + export-status text
640 <= w < 880   -> Tier 2: right cluster (Open/Fit/Measure/Export/Settings/Help)
                            collapses into one "More v" menu_button
w < 640          -> Tier 3: base segment also moves into the More menu;
                            only mode segment + warnings chip + More remain
```

Rules:

- Collapse order is fixed: labels first, then actions, then the base segment.
  The **mode segment never collapses** and the **warnings chip never hides**.
- Breakpoints are `const` values in `main.rs`, tuned once against the real
  rendered widths (button padding is 14x10, item spacing 10 — the numbers
  above are estimates to verify with a screenshot pass, see memory note
  "verify GUI visuals").
- Tier is derived per frame — no persisted state, no hysteresis needed
  (egui repaints on resize; if flicker at a boundary shows up in testing, add
  a +/-16 px hysteresis band).
- Everything in the More menu is the same state/actions as the wide bar —
  no behavior forks, only presentation.

---

## 5. egui implementation notes (egui/eframe 0.34)

All changes land in `crates/etchy-gui/src/main.rs`.

- **Segmented control helper.** No built-in widget; add
  `fn segmented<T: PartialEq + Copy>(ui, value: &mut T, options: &[(T, &str)])`:
  an `egui::Frame` with `corner_radius(8)` (0.34 renamed `Rounding` →
  `CornerRadius`) and a 1 px stroke, containing a `ui.horizontal` with
  `item_spacing.x = 0.0` of `SelectableLabel`s. Selected segment: fill
  `C_COPPER`, text board-dark; unselected: transparent fill, cream text. Use
  it for both mode and base groups. (Per-segment end-cap rounding can be
  faked by rounding only the group frame and letting inner selection fills be
  square-ish — acceptable v1; pixel-perfect caps are a polish item.)
- **Bar layout.** Inside `egui::Panel::top("top")`, replace the
  `ui.horizontal_wrapped` controls row with `ui.horizontal`. Left-to-right:
  mode segment, base segment (tier-gated), warnings chip + export-status
  (tier-gated). Then the right cluster via
  `ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), ...)` —
  the title row already uses this pattern; remember RTL adds widgets in
  reverse visual order (add Help first).
- **Open menu.** `ui.menu_button("Open", ...)` with two `button`s calling
  `self.open_primary(RevSide::Old, ...)` / `RevSide::New` then `ui.close()`
  (same set-flag-act-after pattern the Export menu uses to avoid borrowing
  `self` in the closure).
- **More menu (tiers 2–3).** One `menu_button("More", ...)` reusing the same
  actions; Export and Help become flat items / a nested `ui.menu_button`
  submenu inside it. Measure and the base levels render as checkable items
  (`ui.selectable_label` inside the menu).
- **Settings Diff tab.** Add `SettingsTab::Diff` + entry in
  `SettingsTab::ALL`, a `settings_diff(&mut self, ui)` pane, and its match arm
  next to `settings_display`/`settings_grid`. Move the Slider + DragValue
  code there verbatim. The typing-guard comment near the hotkey handling
  ("noise-filter / grid-spacing DragValue") stays valid — the guard covers
  focused text edits wherever they live, but re-check the guard isn't
  bar-specific.
- **Layers panel row.** In `egui::Panel::left("layers")`, after the list:
  gated `checkbox(&mut self.show_outline, "board edge")` + swatch.
- **Naming.** Mode display strings appear twice (the bar and the
  `publish_state` match) — update both. Update `pick_folder`/`pick_zip`
  titles, `side_open_row` labels, and the mode captions.
- **State fields:** none added beyond the `SettingsTab::Diff` enum variant.
  Nothing new persists; `Settings` struct unchanged. Fields removed from the
  bar (`min_area_mm2`, `show_outline`, `use_gpu`) keep their existing
  storage/behavior.
- **Hotkeys:** unchanged set (1–5, O/B/A, S, F, E, Esc, U, G, arrows/J/K,
  Ctrl+M).

Heads-up for the implementer: `main.rs` is ~4.9k lines and under concurrent
change on other branches — anchor edits by the markers above, not line
numbers, and rebase before starting.

---

## 6. Risks / open questions

Defaults are picked; Simon can veto.

1. **Mode label rename `Before/After` → `Old/New`** (default: rename). The
   `publish_state` string feeds the web/demo status bridge — confirm no
   consumer string-matches "Before"/"After" before renaming, or keep the wire
   value stable and rename only the display label.
2. **Hotkey mnemonics** (default: keep `B`/`A` as hidden legacy aliases for
   Old/New). Alternative: retire them and leave `1–5` only.
3. **Noise filter home** (default: new Settings > Diff tab). Alternative per
   #154: fold under Grid — rejected because it isn't grid-related, but cheap
   to change.
4. **GPU checkbox removal from the bar** (default: remove; Settings > Display
   copy already exists). Not in the approved feedback list — flag in the PR.
5. **Breakpoint values** are estimates; tune with real screenshots (web build
   via the Playwright rig) before merge.

---

## 7. Build checklist (one PR)

1. Rebase on latest `main`; locate the controls row (`horizontal_wrapped` in
   `Panel::top("top")`).
2. Add the `segmented()` helper widget + a smoke usage; verify rounded chrome
   and copper selection in both themes.
3. Add `SettingsTab::Diff` + `settings_diff` pane; move the noise-filter
   Slider/DragValue into it; update the hidden-count and Split/Swipe caption
   strings to point at Settings > Diff.
4. Move the `board edge` checkbox into the Layers panel (gated on
   `outline.is_some()`, swatch + hover text).
5. Remove the bar's GPU checkbox (Settings > Display copy remains).
6. Replace Open A…/Open B… with the `Open v` menu_button.
7. Rebuild the controls row: `horizontal` (not wrapped), mode segment + base
   segment left, warnings/export-status middle, right-to-left action cluster
   (Help, Settings, Export, Measure, Fit, Open).
8. Add width tiers + the `More v` collapse menu; define breakpoint consts.
9. Naming sweep: mode labels, `publish_state`, picker titles, welcome rows,
   captions, hover texts (grep for `Before`, `After`, `Revision A`,
   `Revision B`, `Open A`, `Open B`).
10. Verify: `cargo clippy` + tests; run native + web builds; screenshot the
    bar at ~1400 / 1000 / 800 / 600 px widths (Playwright rig for web) and
    confirm no wrap at any width; check both themes.
11. PR referencing `Refs #57, #154, #157, #160` (never `Closes`), with the
    width-tier screenshots in the description.
