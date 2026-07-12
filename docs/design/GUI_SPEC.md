# etchy viewer — GUI specification

The locked, as-built specification of the etchy viewer (native egui + wasm),
consolidating the 2026-06/07 design reviews. Requirement IDs refer to
[`../REQUIREMENTS.md`](../REQUIREMENTS.md); the implementation-planning history
is `shell-implementation-plan.md` (PR #181).

As of writing, the shell described here lives in **PR #202** (stacked on
#167 → #175 → #182); the pre-shell layout is what `main` still shows.

## 1. Shell layout

```
┌───┬──────────────────────────────────────────────────────────────┐
│ E │ Mad_RP2040 v0.0.0 → v0.0.1   [Overlay|Old|New|Split|Swipe]    │
├───┼───────────────┬──────────────────────────────────────────────┤
│ ▤ │ Layers        │                                    ┌────────┐│
│ ⌖ │ focus ───●─── │                                    │ added  ││
│ ⤓ │  ▸ Copper     │                                    │ removed││
│   │  ▸ Soldermask │            board canvas            └────────┘│
│   │  ▸ Mechanical │                                              │
│   │      …        │  ┌────────────────────┐                      │
│ ⚙ │               │  │ x 62.50 y 41.00 ·grid│  ← bottom-left stack│
└───┴───────────────┴──┴────────────────────┴──────────────────────┘
```

- **Activity rail** (VS Code style): slim vertical strip of painter-drawn icons
  (GUI-1). Panel tabs top-down: **Layers, Export**; between them sits the
  **Measure ruler — a plain tool toggle, not a tab** (it arms/disarms the tool,
  highlights while armed, and opens no panel — #211); a **Settings gear pinned
  at the bottom** (GUI-3). Clicking an inactive tab opens its panel; clicking
  the active tab collapses the panel to the rail. No brand mark on the rail.
- **Rail side** flips left/right via Settings › Display and persists (GUI-2).
  The docked panel follows the rail.
- **Top bar** (GUI-4): the real etchy brand icon (the pad-built E from
  `assets/brand/`) + `old → new` board labels; the centre-left **mode segment**
  Overlay / Old / New / Split / Swipe; right-aligned segmented **Help · Fit ·
  Open** styled to match the mode segment. Nothing else.
- Labels derive identically on web and native (GUI-7): when a folder's basename
  is a generic rev marker (`old`, `new`, `a`, `b`, a bare version), the parent
  directory name is borrowed — e.g. `boards/Mad_RP2040/old` → "Mad_RP2040 old".

## 2. Panels

### Layers
- **Focus slider** at the top (VIEW-6, #224 owner-locked): 0–100%, percent
  readout that accepts typed values. Every non-selected VISIBLE layer renders
  at `1 − focus` — its whole render, base AND diff geometry; 0% = all visible
  layers equal, 100% = only the selected layer visible. The selected layer is
  always full strength; the outline orientation reference never dims. Default
  25%; persisted. (The base-opacity slider that used to sit here moved to
  Settings › Diff, VIEW-5.)
- **Eyes are the ONLY visibility control** (VIEW-6): per-row eye toggles plus a
  group-header eye per family. There is **no view segment** (the old
  single/highlight/all/none is deleted, and with it the #158 diff-only "all
  view" trick — PERF-2 superseded) and **no show/hide-all buttons**. Selection
  stays separate from visibility: clicking a row highlights it, only its eye
  shows/hides it. Hiding every eye leaves a **truly blank canvas** ("no
  geometry in this view"); Split/Swipe still force the selected layer visible.
- **Layer rows**, grouped by family (Copper, Soldermask, Silkscreen, Paste,
  Drill, Mechanical, Other), changed-first within groups: eye toggle · small
  **square** colour swatch · name · **copper Δ%** (changed area as share of the
  layer's new-rev area; mm² split on hover) (VIEW-11). Visible non-selected
  rows **ghost** — name and Δ% fade with the canvas focus dim (floored so rows
  stay legible) — so the panel mirrors what's drawn.
- **The board edge is just the Mechanical › outline row** (VIEW-7): a normal
  eye toggle, on by default. No special row anywhere, no legend entry. In
  Split/Swipe the outline still draws into both halves for orientation.
- Selecting a row highlights it and **never moves the camera** (VIEW-1).

### Measure (a tool, not a panel — #211)
- The rail ruler icon is a **plain tool toggle**: click arms measure mode
  (icon highlights), click again disarms — it opens **no panel** (MEAS-1).
  **Ctrl+M** toggles it too. While armed, the bottom-left measure chip is the
  how-to: the click gesture plus the live units / clear / exit keys.
- Two clicks make a measurement. Completed measurements **persist drawn on the
  board** (ruler + distance with ΔX/ΔY/angle beneath, MEAS-6) until cleared by
  key — there is **no measurements list UI** (MEAS-2). The in-progress point
  shows only while armed; disarming drops it but keeps completed rulers.
- The tool's options — snap-to-grid, crosshair, units **mm / mil / inch** —
  live in **Settings › Measure** (MEAS-3), one home, no duplicates.
- **Clearing by key follows the input preset** (MEAS-4): Altium → exact
  **Shift+C** (no Ctrl/Alt supersets); KiCad → **Esc**, in the cascade
  *in-progress point → completed rulers → exit tool*. An Esc aimed at an open
  menu/popup never reaches the cascade. An explicit Clear rebind (Hotkeys)
  replaces the preset defaults.

### Export
- Surfaces the existing exporter — one code path with the CLI. Two actions:
  **Export current layer** and **Export all changed layers**, each preceded by
  the exact file list it writes (index-prefixed SVG per layer + `areas.csv`).
  No format prose, no nested scroll box (#196). In PDF mode the tab offers the
  changed pages' **diff-overlay PNGs** instead (`page-N.png`; unpaired pages
  are named as having nothing to diff against).
- **Where it lands** (#222, VIEW-16): native writes `etchy-export/` next to
  the last opened input (falling back to the cwd) and the toast reports the
  **absolute** path. Web downloads one file directly; a multi-file set ships
  as a single `etchy-export.zip` — browsers block the 2nd+ automatic download
  from one click, so a per-file loop would silently drop most of the set.

### Settings (a rail panel — no floating window)
Stacked collapsible sections, Display open by default (GUI-6). The panel is
**resizable like the others and holds its width** — opening/closing a section
never re-sizes it (#212):
- **Display** — theme dark/light · **activity rail left/right**.
- **Diff** — **base copper opacity** (0–100% slider with typed entry, moved
  here from the Layers panel by #224; `S` still cycles 0/40/80%, VIEW-5) ·
  min-area noise filter (suppression always surfaced, TRUST-3) · **PDF
  resolution** (DPI chips 150/200/300, default 200, VIEW-15): changing it
  re-rasterizes a loaded PDF pair from the retained bytes; a DPI over the
  raster caps fails loud into the load error and the setting reverts. The CLI
  default stays 150 (`--dpi` covers it) — a deliberate divergence: on-screen
  zooming wants more pixels than a 1:1 overlay PNG.
- **Grid** — spacing, colours (display pitch adapts per VIEW-9; snap uses this).
- **Measure** — units **mm / mil / inch** · snap-to-grid · crosshair + readout
  (the measure tool's one home, #211).
- **Input** — preset **Altium / KiCad** (pan buttons + clear/units keys).
- **Colours** — canvas/added/removed/per-layer colours, theme presets.
- **Layers** — per-layer colour rows (square swatches).
- **Hotkeys** — see §4.

## 3. Canvas

- **Colour semantics** (TRUST-6): green = added, red = removed, reserved for
  diff geometry only. Unchanged base copper renders as the layer colour blended
  toward canvas at the base-opacity setting. Copper/cream chrome; board-dark
  canvas.
- **Camera** (VIEW-1..4): Fit (`F`/button) frames the whole board — the union
  of every layer's extent; it is the only thing that reframes. Wheel zooms at
  the cursor; Ctrl+wheel pans Y, Shift+wheel pans X. Drag-to-pan: left in every
  preset, plus middle/right per preset (Altium: right; KiCad: middle or right).
- **Swipe** (VIEW-4): a curtain over one board. Both halves project through the
  same full-canvas camera; only the clip differs, so the divider bisects a
  single board — left old, right new. The divider travels edge-to-edge, has a
  16 px grab band + visible handle, and dragging it never pans (the grab
  latches at press for the whole gesture). Split (contrast) shows the two
  whole boards side by side.
- **Grid** (VIEW-9): drawn pitch = the configured spacing scaled by a 1-2-5
  sequence until lines are ≥ ~24 px apart, so a grid is visible at any zoom;
  zoomed in, the true pitch draws. **Snap always uses the configured pitch.**
- **Crosshair + readout** (VIEW-8): always-on (toggleable), grid-snapped
  cursor cross with a live coordinate chip; the readout appends "· grid" while
  snapping so displayed precision is honest.
- **Bottom-left chip stack** (VIEW-10), bottom-up: coordinate readout → hidden
  count → layer hint → measure hint. Chips appear only when meaningful:
  - `N hidden < X mm²` — the noise filter's suppression count (Overlay mode;
    TRUST-3). Nothing hidden → no chip.
  - `1 / N layers` — only when exactly one of several layers is visible (the
    board-edge outline doesn't count toward the tally).
  - In Split/Swipe the stack starts above the old/new identity labels.
- **LOD / markers** (TRUST-2): to-scale geometry fades with on-screen size;
  below the opacity floor a real diff renders as a fixed marker dot. Genuine
  noise (below the min-area filter) is culled *and counted* in the hidden chip.

## 4. Hotkeys

Defaults (KEY-4); all rebindable in Settings › Hotkeys except the fixed aliases:

| Action | Default | Notes |
|---|---|---|
| Toggle measure | **Ctrl+M** | also the rail ruler toggle |
| Clear measurements | *preset:* Altium **Shift+C** / KiCad **Esc** | rebind overrides preset |
| Fit view | F | frames the whole board |
| Cycle base opacity | S | 0 → 40 → 80% |
| Cycle measure units | *preset:* Altium **Q** / KiCad **Ctrl+U** | each tool's own units key (#211); rebind overrides preset; cycles mm → inch → mil |
| Toggle grid | G | |
| Modes | 1–5 | fixed aliases: O=Overlay, B=Old, A=New (bare key only) |
| Step layer | J/K or ↓/↑ | fixed |
| Escape | Esc | cascade: point → (KiCad) list → exit tool |

Editor rules (KEY-1..3): press-to-capture with Esc cancel; a capture **refuses
a taken key** (any other action, the fixed aliases, or the active preset clear
and units keys) with an inline "taken by …" note; captures cancel when the
editor leaves the screen; focused text fields keep their keystrokes;
exact-modifier matching everywhere; Reset restores every default. Preset-driven
defaults display as e.g. "Q (preset)" until explicitly rebound.

## 5. Branding

- The **pad-built E icon** (`assets/brand/etchy-icon*`) is the product mark —
  top bar and window/app icons. Never a substitute drawing (GUI-5).
- Palette: copper-gold `#e8a33d` on board-dark `#0b0f0e`, paper-cream text;
  green/red reserved per TRUST-6.
- All rail/panel icons are painter-drawn (fonts lack the glyphs on target
  platforms).

## 6. Platform notes

- The GUI compiles for native and `wasm32-unknown-unknown`; every GUI change
  must keep both building (PROC-2).
- **Build stamp** (GUI-10, #213): the git short sha is baked in at compile time
  and shown subtly — a `build <sha>` line in the Help menu and the brand icon's
  hover tooltip — identical on native and wasm, so a browser tab can prove
  which build it runs (the stale-wasm case).
- Native on WSLg: Help links open via `explorer.exe`; file dialogs need
  `xdg-desktop-portal`. Known open defect #203 (pointer offset suspected —
  clicks miss widgets while drags work).
- The web demo embeds the Mad_RP2040 demo pair and serves the feedback widget
  (Ctrl+Enter send, Ctrl+V screenshot paste — PROC-4).
