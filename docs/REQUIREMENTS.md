# etchy — Software Requirements

The single consolidated requirements document for etchy: everything decided in
product discovery, design reviews, and the 2026-06/07 feedback rounds, stated as
numbered requirements with their source and current status.

- **Companions:** [`PRODUCT_DISCOVERY.md`](PRODUCT_DISCOVERY.md) (why these
  decisions), [`design/GUI_SPEC.md`](design/GUI_SPEC.md) (the full viewer
  behaviour spec), [`TRUST.md`](TRUST.md) (trust model), [`ROADMAP.md`](ROADMAP.md)
  (sequencing), [`DEVELOPER_GUIDE.md`](DEVELOPER_GUIDE.md) (architecture).
- **Status legend:** ✅ shipped on `main` · 🔶 built, in an open PR · 🔜 accepted,
  not built · 📋 future (post-1.0).
- **Source** is the GitHub issue, PR, or the decision record that produced the
  requirement. "Owner" decisions were made by Simon in review sessions.

---

## 1. Product scope

| ID | Requirement | Source | Status |
|---|---|---|---|
| SCOPE-1 | etchy compares two revisions of the **same board's** fab output and shows + measures exactly what changed. | discovery | ✅ |
| SCOPE-2 | Inputs are **Gerber RS-274X/X2**, **Excellon** drill, **pick-and-place** centroid files, and **schematic PDF** (pixel page-diff). | discovery, #115, #63 | ✅ (PDF CLI wiring 🔜 #63) |
| SCOPE-3 | Surfaces are **CLI/CI first**, native **egui** GUI second; both ship. The GUI also builds to wasm for the web demo. | discovery | ✅ |
| SCOPE-4 | **Non-goals** (hold the line): native CAD ingestion (Altium/IPC-2581/ODB++), net/connectivity diff, BOM/component diff, DRC. | discovery | standing |
| SCOPE-5 | Native KiCad `.kicad_pcb` ingestion is an **accepted future goal**, post-1.0. | #122 | 📋 |
| SCOPE-6 | Distribution is per-OS **static binaries** + a **distroless container** — never pip/OS package managers. | discovery, #135 | ✅ (container publish to ghcr 🔜) |
| SCOPE-7 | Licensing stays permissive (`MIT OR Apache-2.0`); `cargo deny` blocks copyleft deps. | discovery | ✅ |

## 2. Diff engine (etchy-core)

| ID | Requirement | Source | Status |
|---|---|---|---|
| CORE-1 | Per-layer **polygon boolean** diff: `added = new − old`, `removed = old − new`. One computation feeds every output (overlay, magnitudes, exports). | discovery | ✅ |
| CORE-2 | Geometry is **fixed-point (nm)** for deterministic, reproducible diffs. | discovery | ✅ |
| CORE-3 | `etchy-core` stays **pure**: no I/O policy, no `println!`, no `process::exit`. | CLAUDE.md | standing |
| CORE-4 | **Same-board guard:** mismatched extents (>1 mm / 2%) fail loud with a clear error — never a garbage diff. No auto-registration. | discovery, #92/#127 | ✅ |
| CORE-5 | Layer pairing is rename-tolerant, keyed by classified layer type; both revisions must resolve to comparable layer sets. | M1 | ✅ |
| CORE-6 | Magnitudes per layer: changed **area (mm²)** split into added/removed, and **region count**, from the same boolean result. | discovery | ✅ |
| CORE-7 | Per-layer input **DoS ceilings** (points/spans caps) so hostile files can't exhaust memory. | #83/#128 | ✅ |

## 3. Inputs & layer classification

| ID | Requirement | Source | Status |
|---|---|---|---|
| IN-1 | Gerber RS-274X and X2 parse on real fab output (Altium + KiCad exports validated; real-board real-board fixes in the corpus). | Spike 2, #145 | ✅ |
| IN-2 | Excellon drill parse, including headerless files, inline formats, R codes; phantom-hole and tool-line traps covered by tests. | #62, #144 | ✅ |
| IN-3 | Pick-and-place centroid diff: moved/rotated/added/removed parts as geometry (`LayerKind::Placement`). | #115/#141 | ✅ |
| IN-4 | Filename classification types every copper layer as **Copper** — including KiCad `.gl<n>` inner copper, mapped **ordinally** (`.gl2`→Inner 1) so cross-scheme pairing (`.gl2` ≡ `In1_Cu` ≡ `.g1`) works. Genuinely unknown files fall to Other. | #176 | 🔶 PR #180 |
| IN-5 | Loaders accept folders, zips, and drag-and-drop (GUI); the same classification path serves CLI and GUI. | #120/#93 | ✅ |

## 4. Trust (top-tier requirement — gates everything)

| ID | Requirement | Source | Status |
|---|---|---|---|
| TRUST-1 | **No silent misses.** A change the tool saw must never be invisible without an on-screen accounting. | discovery | standing |
| TRUST-2 | Real-but-tiny diff regions render as fixed-size **marker dots** when too small to draw to scale — they fade only into markers, never into nothing (covers the mid-zoom dropout band). | #14, #156/#162 | ✅ |
| TRUST-3 | The noise filter (min-area threshold) always **surfaces its hidden-region count** in the viewer; suppression is visible ("N hidden < X mm²"), never quiet. | G9, #178 review | 🔶 PR #182/#202 |
| TRUST-4 | Fail-loud beats wrong-but-quiet: unprocessable layers/files produce clear errors, not partial silent output. | discovery | standing |
| TRUST-5 | A **golden corpus** (synthetic known-answer boards + real boards) plus property tests (`diff(A,A)=∅`, add/remove symmetry) and fuzzing gate correctness in CI. | Phase 0 | ✅ |
| TRUST-6 | Green `#46d18a` / red `#ff5d73` are **reserved for added/removed diff geometry** — never used for decoration in any surface. | brand theme | standing |
| TRUST-7 | Fast-built parsers get an **adversarial review pass** before release (the 2026-07-01 pass found 10 real trust defects). | #144 lesson | standing |

## 5. CLI & CI surface

| ID | Requirement | Source | Status |
|---|---|---|---|
| CLI-1 | `etchy old/ new/` runs the whole pipeline; exit codes **0 = no diff, 1 = diff, 2 = error** (the CI contract). | discovery | ✅ |
| CLI-2 | Outputs: SVG overlays, versioned **JSON v1** magnitudes, Markdown summary, self-contained **HTML report** (`--html`). | M1/M2, #139 | ✅ |
| CLI-3 | Threshold gates: `--fail-on-area`, `--fail-on-regions`, `--gate-layers` (validated tokens; misnamed layers must not silently skip the gate). | #137, #144 | ✅ |
| CLI-4 | **Git-refs invocation**: `etchy refA refB [subdir]` diffs committed fab packs without a checkout; ref names are never guessed from typos. | #138, #144 | ✅ |
| CLI-5 | GitHub Action (composite) posts the Markdown summary on PRs; runs fully offline/self-hosted. | M2 | ✅ |
| CLI-6 | PDF inputs: `.pdf` old/new → page-by-page pixel diff summary + overlay PNGs, same exit-code contract. | #63 | 🔜 (engine ✅ #150/#151) |

## 6. GUI — shell (the locked 2026-07 redesign)

The full behaviour spec is [`design/GUI_SPEC.md`](design/GUI_SPEC.md). Everything
below is owner-locked and built in **PR #202** unless noted.

| ID | Requirement | Source | Status |
|---|---|---|---|
| GUI-1 | **VS Code-style shell**: a slim activity rail of painter-drawn icons opens docked side panels (tabs); clicking the active tab collapses its panel. | #11/#57, owner | 🔶 #202 |
| GUI-2 | Rail side is **flippable left/right** in Settings; the choice persists. | owner | 🔶 #202 |
| GUI-3 | Rail tabs: **Layers, Measure, Export**, with **Settings as a gear pinned at the bottom**. No monogram on the rail. | owner, #190 | 🔶 #202 |
| GUI-4 | The top bar holds only: brand icon + board label, the **mode segment** (Overlay/Old/New/Split/Swipe), and segmented **Help/Fit/Open**. Everything else lives in the rail. | #6/#200 | 🔶 #202 |
| GUI-5 | Branding uses the real **etchy pad-built E** (`assets/brand/etchy-icon`) — never a hand-drawn substitute. Rail/panel icons are painter-drawn, never font glyphs. | #191, #16/#30 | 🔶 #202 |
| GUI-6 | **Settings is a rail panel** (not a floating window) with stacked collapsible sections: Display, Diff, Grid, Input, Colours, Layers, Hotkeys. | #199, owner | 🔶 #202 |
| GUI-7 | Web and native present the **same old→new labels**, derived consistently from meaningful path parts. | #177 | 🔶 #182 |
| GUI-8 | Old/new naming everywhere (never A/B or before/after). | #160, owner | ✅ |

## 7. GUI — viewer behaviour

| ID | Requirement | Source | Status |
|---|---|---|---|
| VIEW-1 | **Selecting a layer never moves the camera.** Only Fit reframes. | #169 | 🔶 #175 |
| VIEW-2 | **Fit frames the whole board** (union of every layer's extent), not the changed region or one layer. | #170 | 🔶 #175 |
| VIEW-3 | **Left-drag pans** in every input preset (right/middle per preset still work). | #172/#18 | 🔶 #175 |
| VIEW-4 | **Swipe is a curtain over one board**: the divider bisects a single board (left = old, right = new), travels the full canvas width, and dragging it never pans the board. | #171/#183, owner | 🔶 #175 |
| VIEW-5 | Base copper (unchanged geometry) renders at a user-set **opacity slider** (0–100%, in the Layers panel); the S key cycles the familiar off/faint/strong stops. | #12, owner | 🔶 #202 |
| VIEW-6 | View modes **single / highlight / all** control multi-layer display; hide-all clears **every** layer. | #59, #173 | ✅ / 🔶 #175 |
| VIEW-7 | The **board edge is a normal layer** (Mechanical › outline): plain eye toggle, visible by default; no special row, control, or legend entry. | #3/#157, owner clarified | 🔶 #202 |
| VIEW-8 | **Always-on crosshair + grid-snapped cursor** with a live coordinate readout, independent of measure mode (toggles live in the Measure tab; snap defaults on and the readout says "· grid" so precision is honest). | #16/#17/#179, owner | 🔶 #182/#202 |
| VIEW-9 | The **drawn grid adapts to zoom** (1-2-5 pitch selection) so a grid is visible at any zoom; **snapping stays at the configured pitch** — the display never changes what snap does. | #195 | 🔶 #202 |
| VIEW-10 | All trust/status chips (coordinate readout, hidden count, "1 / N layers" hint, measure hint) form one **bottom-left stack**, clear of the Split/Swipe identity labels. No status caption clutters the canvas top. | #10/#193/#194, #48 | 🔶 #202 |
| VIEW-11 | Layer rows: small **square** colour swatches, **copper-coloured Δ%**, no "changed first" caption; mm² detail on hover. | #5/#20/#7, #114 | 🔶 #175/#202 |
| VIEW-12 | Colour/theme presets selectable in Settings; per-layer colours editable. | #155 | ✅ |

## 8. Measure system

| ID | Requirement | Source | Status |
|---|---|---|---|
| MEAS-1 | The rail's Measure icon **arms the tool and opens the tab in one click** (works with the panel collapsed); re-click disarms. **Ctrl+M** toggles it too. No "Armed" widget — a hint names the shortcut. | #197, owner | 🔶 #202 |
| MEAS-2 | Two clicks make a measurement; completed measurements **accumulate in a list** (per-row delete, clear-all) and persist drawn on the board. | owner spitball | 🔶 #202 |
| MEAS-3 | Units **mm / mil / inch** ("keep what we have"), selectable in the Measure tab. | owner | 🔶 #202 |
| MEAS-4 | **Clear-measurements follows the input preset**: Altium → exact **Shift+C**, KiCad → **Esc** (after the in-progress point clears). A custom rebind stands the preset defaults down. Esc aimed at an open popup never clears the list. | #198, owner | 🔶 #202 |
| MEAS-5 | Measure clicks snap to the grid when snap is on; placement follows the visible snapped cursor. | #51 | ✅ |

## 9. Hotkeys

| ID | Requirement | Source | Status |
|---|---|---|---|
| KEY-1 | A **Settings › Hotkeys editor** lists every rebindable action with its binding; rebinding is press-to-capture, Esc cancels, with **reset to defaults**. Bindings persist. | #201, owner | 🔶 #202 |
| KEY-2 | A capture **refuses a key another action owns** (including fixed aliases and the preset clear key) — one press must never dispatch two actions. Bindings match their **exact modifier set**. | #201 review | 🔶 #202 |
| KEY-3 | A capture armed while its editor is hidden cancels; a focused text field keeps its keystrokes. | #201 review | 🔶 #202 |
| KEY-4 | Defaults: Ctrl+M measure · F fit · S base cycle · U units · G grid · 1–5 modes (O/B/A legacy aliases) · J/K/arrows step layers · preset clear per MEAS-4. | as built | 🔶 #202 |

## 10. Rendering & performance

| ID | Requirement | Source | Status |
|---|---|---|---|
| PERF-1 | Smooth pan/zoom on a dense 16-layer board: tessellate once (cached by geometry key), transform per frame, cull off-screen, one merged mesh. | discovery, G6 | ✅ |
| PERF-2 | In **all-layers** view, non-selected layers draw diff-only (base copper dropped) — the ~70% vertex cut that keeps dense pan smooth without losing any diff. | #158 | ✅ |
| PERF-3 | The optional GPU transform path stays **off by default**: it draws true-scale with no marker LOD, which violates TRUST-2 for tiny diffs. Do not enable it as a perf fix. | #106/#117 decision | standing |
| PERF-4 | Per-layer parallel diff (rayon); render never re-triangulates on camera or colour changes. | M3 | ✅ |
| PERF-5 | Adaptive flash tessellation (fewer segments for tiny pads) is accepted future work. | #94 | 📋 |
| PERF-6 | The polarity-span accumulation stays O(spans·N) — the known-safe form. The proposed batch rewrite reintroduced the real-board trace-voids bug; quadratic is inherent here. Post-1.0 design task. | #79 | 📋 |

## 11. Known open defects

| ID | Requirement | Source | Status |
|---|---|---|---|
| BUG-1 | Native (WSLg): buttons must respond to clicks — currently suspected fractional-scale pointer offset (drag works, absolute hit-testing misses). Diagnostic: `WINIT_X11_SCALE_FACTOR=1`. | #203 | 🔜 |

## 12. Process requirements (how this repo works)

| ID | Requirement | Source | Status |
|---|---|---|---|
| PROC-1 | Every piece of incoming feedback is categorised and gets a tracking issue; fixing PRs use **Refs #N, never Closes** — the owner reviews and closes issues himself. | working agreement | standing |
| PROC-2 | While CI is billing-blocked, every PR is locally verified: `cargo test`, `clippy --all-targets` warning-clean, `fmt --check`, and the wasm build for GUI changes. | working agreement | standing |
| PROC-3 | Feedback screenshots may show confidential boards: reference local paths in issues; never commit `deploy/feedback/`. | working agreement | standing |
| PROC-4 | Every feedback widget ships **Ctrl+Enter to send** and **Ctrl+V screenshot paste** with removable thumbnails. | working agreement | ✅ (dev template parity 🔜 #168) |

---

*Sources of record: `docs/PRODUCT_DISCOVERY.md` for the founding decisions; issue
tracker for every numbered feedback item; `docs/design/GUI_SPEC.md` for the full
viewer behaviour this document summarises.*
