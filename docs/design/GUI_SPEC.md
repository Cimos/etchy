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
│ ⌖ │  base ───●─── │                                    │ added  ││
│ ⤓ │  view s/h/all │                                    │ removed││
│   │  ▸ Copper     │            board canvas            └────────┘│
│   │  ▸ Soldermask │                                              │
│   │  ▸ Mechanical │                                              │
│   │      …        │  ┌────────────────────┐                      │
│ ⚙ │               │  │ x 62.50 y 41.00 ·grid│  ← bottom-left stack│
└───┴───────────────┴──┴────────────────────┴──────────────────────┘
```

- **Activity rail** (VS Code style): slim vertical strip of painter-drawn icons
  (GUI-1). Tabs top-down: **Layers, Measure, Export**; a **Settings gear pinned
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
- **Base opacity slider** at the top (VIEW-5): 0–100%, percent readout that
  accepts typed values; replaces the old off/faint/strong segment. `S` cycles
  0% → 40% → 80% (the familiar stops).
- **View segment** single / highlight / all (VIEW-6): single shows the active
  layer; highlight shows all, dimming non-selected; all shows all equally
  (non-selected layers draw diff-only for speed, PERF-2). Per-row eyes
  fine-tune afterwards. **Hide all clears every layer** (#173).
- **Layer rows**, grouped by family (Copper, Soldermask, Silkscreen, Paste,
  Drill, Mechanical, Other), changed-first within groups: eye toggle · small
  **square** colour swatch · name · **copper Δ%** (changed area as share of the
  layer's new-rev area; mm² split on hover) (VIEW-11).
- **The board edge is just the Mechanical › outline row** (VIEW-7): a normal
  eye toggle, on by default. No special row anywhere, no legend entry. In
  Split/Swipe the outline still draws into both halves for orientation.
- Selecting a row highlights it and **never moves the camera** (VIEW-1).

### Measure
- The rail icon **arms the tool and opens this tab in one click** — including
  while the panel is collapsed; re-click disarms and collapses (MEAS-1).
  A hint line names the shortcut (**Ctrl+M**); there is no Armed widget.
- Contents: snap-to-grid toggle · crosshair toggle · units **mm / mil / inch**
  · the running **measurements list** — each row shows its distance in the
  active unit with a per-row remove; plus clear-all (MEAS-2/3).
- Completed measurements persist drawn on the board (ruler + distance label)
  until deleted; the in-progress point shows only while armed.
- **Clearing by key follows the input preset** (MEAS-4): Altium → exact
  **Shift+C** (no Ctrl/Alt supersets); KiCad → **Esc**, in the cascade
  *in-progress point → completed list → exit tool*. An Esc aimed at an open
  menu/popup never reaches the cascade. An explicit Clear rebind (Hotkeys)
  replaces the preset defaults.

### Export
- Surfaces the existing exporter — one code path with the CLI. Two actions:
  **Export current layer** and **Export all changed layers**, each preceded by
  the exact file list it writes (index-prefixed SVG per layer + `areas.csv`).
  No format prose, no nested scroll box (#196). Native writes
  `./etchy-export/`; wasm downloads.

### Settings (a rail panel — no floating window)
Stacked collapsible sections, Display open by default (GUI-6):
- **Display** — theme dark/light · measure units · **activity rail left/right**.
- **Diff** — min-area noise filter (suppression always surfaced, TRUST-3).
- **Grid** — spacing, colours (display pitch adapts per VIEW-9; snap uses this).
- **Input** — preset **Altium / KiCad** (pan buttons + clear-measure key).
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
| Toggle measure | **Ctrl+M** | also the rail Measure icon |
| Clear measurements | *preset:* Altium **Shift+C** / KiCad **Esc** | rebind overrides preset |
| Fit view | F | frames the whole board |
| Cycle base opacity | S | 0 → 40 → 80% |
| Cycle measure units | U | mm → mil → inch |
| Toggle grid | G | |
| Modes | 1–5 | fixed aliases: O=Overlay, B=Old, A=New (bare key only) |
| Step layer | J/K or ↓/↑ | fixed |
| Escape | Esc | cascade: point → (KiCad) list → exit tool |

Editor rules (KEY-1..3): press-to-capture with Esc cancel; a capture **refuses
a taken key** (any other action, the fixed aliases, or the active preset clear
key) with an inline "taken by …" note; captures cancel when the editor leaves
the screen; focused text fields keep their keystrokes; exact-modifier matching
everywhere; Reset restores every default.

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
- Native on WSLg: Help links open via `explorer.exe`; file dialogs need
  `xdg-desktop-portal`. Known open defect #203 (pointer offset suspected —
  clicks miss widgets while drags work).
- The web demo embeds the Mad_RP2040 demo pair and serves the feedback widget
  (Ctrl+Enter send, Ctrl+V screenshot paste — PROC-4).
