# Design — GUI brand tokens + theme system (sub-project A)

**Date:** 2026-06-17 · **Status:** approved direction, ready for implementation plan
**Scope:** the `etchy-gui` viewer only. Part of the broader "brand across all surfaces"
effort, decomposed as: **A. brand tokens + GUI theme (this doc)** → B. CLI terminal
colours → C. README/docs polish → D. website (deferred). B/C/D are out of scope here.

## Goal

The GUI currently uses stock egui chrome (grey panels, default font); only the
diff colours, window icon, and logo are on-brand. Apply the etchy brand
(`assets/brand/`) to the whole app as a small, switchable **dark/light theme
system** driven by a single token source of truth.

## Architecture

New module `crates/etchy-gui/src/theme.rs` (presentation-only; the engine stays
pure and untouched):

- `enum Theme { Board, Paper }` — `Default = Board`; `fn toggle(self) -> Theme`.
- `struct Tokens { bg, panel, text, text_muted, accent, accent_dim, added, removed, hairline, selection }` — all `egui::Color32`. The single source of truth.
- `fn tokens(Theme) -> Tokens` — the two palettes (below).
- `fn visuals(Theme) -> egui::Visuals` — maps tokens onto egui: `window_fill`/`panel_fill` = bg/panel, widget bg/fg + text = text/muted, `selection.bg_fill` = selection tint, `selection.stroke`/`hyperlink_color` = accent, tuned `rounding` (~6px). Built from `Visuals::dark()`/`light()` as the base.
- `fn install_fonts(&egui::Context)` — registers the bundled brand fonts (once).
- `fn apply(&egui::Context, Theme)` — `set_visuals(visuals(theme))` and sets `TextStyle`s (Heading → Zilla Slab; Monospace → JetBrains Mono).

`main.rs` changes:
- Delete hardcoded `C_ADDED`/`C_REMOVED`/`C_BASE`/`C_CANVAS`; read these from the
  active theme's `Tokens` instead (canvas bg = `tokens.bg` — **canvas follows the
  theme**, confirmed; diff fills = `tokens.added`/`removed`).
- `ViewApp` gains `theme: Theme`. In the eframe creation closure, call
  `install_fonts` + `apply(default)`. In `ui()`, a Board/Paper toggle in the top
  bar calls `apply` when flipped. Theme persists across runs via `eframe`'s
  storage if available with trivial effort; otherwise in-session default `Board`.
- Header: decode `assets/brand/png/etchy-icon-256.png` → texture once → draw the
  mark (~18–20px) beside the `etchy` title.

## Palette tokens (hex)

| token | Board (dark) | Paper (light) |
|---|---|---|
| bg (window + canvas) | `#0b0f0e` | `#f4f1e8` |
| panel | `#12171a` | `#e7e2d4` |
| text | `#e8e6df` | `#0b0f0e` |
| text_muted | `#8b9499` | `#6b6f63` |
| accent (copper) | `#e8a33d` | `#b9762a` |
| accent_dim | `#b9762a` | `#8a5a20` |
| added | `#46d18a` | `#2fae71` |
| removed | `#ff5d73` | `#e23b54` |
| hairline | `#2a2f33` | `#d6d0c2` |
| selection | `#241d12` | `#f0e3c6` |

Copper is the **single accent**: selection tint, active-tab underline, hyperlinks,
header mark, canvas border, theme pill. Diff stays green/red (brand accents),
nudged darker in Paper for contrast on the light background.

## Typography

Bundle both brand fonts under `assets/brand/fonts/` with their licenses:
- **Zilla Slab** (OFL) → registered as the **proportional** family (UI text + the
  `etchy`/`Layers` headings).
- **JetBrains Mono** (OFL) → registered as the **monospace** family; the layer
  list's area/region figures use the monospace `TextStyle` so columns align.

Fonts are embedded via `include_bytes!` (no system-font dependency — fits the
static-binary distribution goal). License files (`OFL.txt`) ship alongside; a
short attribution line is added to the GUI's about/usage text or README.

## Testing

`tokens()`, `visuals()`, `toggle()` are pure → unit tests:
- the two themes' `Tokens` differ (e.g. `bg` not equal);
- `accent` equals the spec copper for each theme;
- `visuals(t).hyperlink_color == tokens(t).accent` and `selection.bg_fill == tokens(t).selection`;
- `Theme::Board.toggle() == Paper` and round-trips.

Rendering itself is visual-verified in the live GUI (Board/Paper on real
revA/revC boards), the channel we've used throughout.

## Out of scope (YAGNI)

Custom widget painting/animation; OS dark/light auto-detection; font subsetting;
and surfaces B/C/D (CLI colours, docs, website) — each its own later cycle.

## Risks / notes

- **Font sourcing:** Zilla Slab + JetBrains Mono TTFs must be added to the repo
  (both freely available, OFL). Verify the exact weights (Zilla Slab Medium/SemiBold
  for headings; JetBrains Mono Regular) at implementation time.
- **egui coverage:** `Visuals` covers chrome comprehensively without custom
  widgets; if a specific element resists theming, note it rather than reaching for
  bespoke painting (that's a deliberate later increment).
- **Reference mockup:** `docs/superpowers/specs/` is text; the approved visual is
  the PIL mockup generated during brainstorming (Board vs Paper side-by-side).
