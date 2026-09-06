# etchy — Software Requirements

The single consolidated requirements document for etchy: everything decided in
product discovery, design reviews, and the 2026-06/07 feedback rounds, stated as
numbered requirements with their source and current status.

- **Companions:** [`PRODUCT_DISCOVERY.md`](PRODUCT_DISCOVERY.md) (why these
  decisions), [`design/GUI_SPEC.md`](design/GUI_SPEC.md) (the full viewer
  behaviour spec), [`TRUST.md`](TRUST.md) (trust model), [`FEEDBACK_LOG.md`](FEEDBACK_LOG.md)
  (dated feedback → requirement traceability), [`ROADMAP.md`](ROADMAP.md)
  (sequencing), [`DEVELOPER_GUIDE.md`](DEVELOPER_GUIDE.md) (architecture).
- **Status legend:** ✅ shipped on `main` · 🔶 built, in an open PR · 🔜 accepted,
  not built · 📋 future (post-1.0) · ↔ superseded by a later decision.
- **Source** is the GitHub issue, PR, or the decision record that produced the
  requirement. "Owner" decisions were made by Simon in review sessions.

---

## 1. Product scope

| ID | Requirement | Source | Status |
|---|---|---|---|
| SCOPE-1 | etchy compares two revisions of the **same board's** fab output and shows + measures exactly what changed. | discovery | ✅ |
| SCOPE-2 | Inputs are **Gerber RS-274X/X2**, **Excellon** drill, **pick-and-place** centroid files, and **schematic PDF** (pixel page-diff, CLI + GUI on both platforms). | discovery, #115, #63 | ✅ |
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
| IN-1 | Gerber RS-274X and X2 parse on real fab output (Altium + KiCad exports validated; real-board fixes in the corpus). | Spike 2, #145 | ✅ |
| IN-2 | Excellon drill parse, including headerless files, inline formats, R codes; phantom-hole and tool-line traps covered by tests. | #62, #144 | ✅ |
| IN-3 | Pick-and-place centroid diff: moved/rotated/added/removed parts as geometry (`LayerKind::Placement`). | #115/#141 | ✅ |
| IN-4 | Filename classification types every copper layer as **Copper** — including KiCad `.gl<n>` inner copper, mapped **ordinally** (`.gl2`→Inner 1) so cross-scheme pairing (`.gl2` ≡ `In1_Cu` ≡ `.g1`) works. Genuinely unknown files fall to Other. | #176 | ✅ |
| IN-5 | Loaders accept folders, zips, and drag-and-drop (GUI); the same classification path serves CLI and GUI. | #120/#93 | ✅ |
| IN-6 | Gerber dialect coverage spans **X1 (legacy RS-274X) and X2 (attributes)**, and filename/naming schemes from the major EDA tools. Altium + KiCad shipped; additional EDA-tool schemes (Eagle, OrCAD, …) are accepted — each lands with corpus samples + classification tests. | owner 2026-07-07 | ✅ X1/X2 · 🔜 more EDA schemes |

## 4. Trust (top-tier requirement — gates everything)

| ID | Requirement | Source | Status |
|---|---|---|---|
| TRUST-1 | **No silent misses.** A change the tool saw must never be invisible without an on-screen accounting. | discovery | standing |
| TRUST-2 | Real-but-tiny diff regions render as fixed-size **marker dots** when too small to draw to scale — they fade only into markers, never into nothing (covers the mid-zoom dropout band). | #14, #156/#162 | ✅ |
| TRUST-3 | The noise filter (min-area threshold) always **surfaces its hidden-region count** in the viewer; suppression is visible ("N hidden < X mm²"), never quiet. | G9, #178 review | ✅ |
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
| CLI-6 | PDF inputs: `.pdf` old/new → page-by-page pixel diff summary + overlay PNGs (`--out`), same exit-code contract. Behind the `pdf` feature; `--dpi` sets resolution. | #63 | ✅ (verified end-to-end on the corpus pair; release binaries ship the feature on) |
| CLI-7 | **Report generation** is first-class: one run produces a shareable, self-contained **HTML report** plus SVG overlays, JSON v1, and a Markdown summary — no separate tooling. | owner 2026-07-07, #139 | ✅ |
| CLI-8 | **Five-line CI adoption:** adding the GitHub Action to a repo takes ≤ 5 workflow lines, and every PR that touches Gerbers gets a **layer-by-layer diff posted in its comments**. | owner 2026-07-07 | ✅ 5-line `uses:` block (recipe in `docs/ci-recipes/`); the Action supplies its own token and posts a sticky per-layer `--format md` table. The current Action design builds from source; `v0.1.0-rc1` is published as a pre-release, and public v0.1.0 is gated on `PRE_PUBLIC.md`. |
| CLI-9 | **PDF pages pair by content, not by index**: each rasterized page is fingerprinted (a 16×16 grid of per-cell **ink coverage** against a fixed absolute luminance, compared by L1 distance) and the two revisions' page sequences are aligned, so an inserted or removed sheet becomes an explicit `new-only` / `old-only` row instead of desynchronising every later pair. The digest must be **local**: ink added anywhere may move only the cells it touches, so a small edit never makes a sheet look like a different document. The alignment is **always reported** (summary, Markdown, JSON `alignment`, and the viewer) unless it is the plain index pairing, which says nothing extra. The fingerprint decides *which* pages pair and nothing else: every paired sheet still gets the full pixel diff, and no page may be dropped. | #249 | ✅ |
| CLI-9a | **Content alignment must never do worse than index pairing**: a re-pairing is adopted only when it beats index pairing by a clear margin *and* every pair it chooses is more alike than chance; otherwise index pairing is kept and the ambiguity is stated ("page alignment was ambiguous, paired by index", JSON `alignment.basis`). The page count is capped at 1024 per side, checked and failed loud **before** the quadratic alignment matrix is allocated. | #249 | ✅ |
| CLI-10 | **A PDF page-size change is a diff, not an error**: a paired sheet whose two sides rasterize to genuinely different pixel sizes is reported as a fully-changed page (`changed_fraction` 1.0) with both sizes named in every format, and the run **exits 1**. Exit 2 stays reserved for input etchy cannot render at all (a CI gate treats exit 2 as infrastructure failure, not a review block). The pair is never pixel-diffed or rescaled, so it has no overlay: the viewer shows both sheets at true scale and the Export tab names the page and the reason. Sizes within 2 px per axis are rasterization rounding, **not** a resize: both rasters are cropped to the region they share and the pair is diffed in full, with the crop reported — discarding a sheet's pixel diff over a rounding artefact is not acceptable. | #262 | ✅ |

## 6. GUI — shell (the locked 2026-07 redesign)

The full behaviour spec is [`design/GUI_SPEC.md`](design/GUI_SPEC.md). Everything
below is owner-locked; the shell merged to `main` 2026-07-07 (PR chain
#167→#175→#182→#202).

| ID | Requirement | Source | Status |
|---|---|---|---|
| GUI-1 | **VS Code-style shell**: a slim activity rail of painter-drawn icons opens docked side panels (tabs); clicking the active tab collapses its panel. | #11/#57, owner | ✅ |
| GUI-2 | Rail side is **flippable left/right** in Settings; the choice persists. | owner | ✅ |
| GUI-3 | Rail: **Layers and Export tabs** with the **Measure ruler as a plain tool toggle between them** (arms/disarms, no panel — see MEAS-1) and **Settings as a gear pinned at the bottom**. No monogram on the rail. | owner, #190, #211 | ✅ |
| GUI-4 | The top bar holds only: brand icon + board label, the **mode segment** (Overlay/Old/New/Split/Swipe), and segmented **Help/Fit/Open**. Everything else lives in the rail. | #6/#200 | ✅ |
| GUI-5 | Branding uses the real **etchy pad-built E** (`assets/brand/etchy-icon`) — never a hand-drawn substitute. Rail/panel icons are painter-drawn, never font glyphs. | #191, #16/#30 | ✅ |
| GUI-6 | **Settings is a rail panel** (not a floating window) with stacked collapsible sections: Display, Diff, Grid, Measure, Input, Colours, Layers, Hotkeys. **Resizable like the other panels, and its width holds steady** when sections open/close. | #199, #212, owner | ✅ |
| GUI-7 | Web and native present the **same old→new labels**, derived consistently from meaningful path parts. | #177 | ✅ |
| GUI-8 | Old/new naming everywhere (never A/B or before/after). | #160, owner | ✅ |
| GUI-9 | Settings rows make the selectable value chips visually distinct from the setting label. | #205, #210 | ✅ |
| GUI-10 | The build's **git short sha is baked in at compile time** and shown subtly — a `build <sha>` line in the Help menu and the brand icon's hover tooltip — identically on native and wasm, so a user can prove which build a browser tab runs. | #213 | ✅ |

## 7. GUI — viewer behaviour

| ID | Requirement | Source | Status |
|---|---|---|---|
| VIEW-1 | **Selecting a layer never moves the camera.** Only Fit reframes. | #169 | ✅ |
| VIEW-2 | **Fit frames the whole board** (union of every layer's extent), not the changed region or one layer. | #170 | ✅ |
| VIEW-3 | **Left-drag pans** in every input preset (right/middle per preset still work). | #172/#18 | ✅ |
| VIEW-4 | **Swipe is a curtain over one board**: the divider bisects a single board (left = old, right = new), travels the full canvas width, and dragging it never pans the board. | #171/#183, owner | ✅ |
| VIEW-5 | Base copper (unchanged geometry) renders at a user-set **opacity slider** (0–100% with typed entry), in **Settings › Diff** (moved out of the Layers panel — the Focus slider took its spot, #224); the S key cycles the familiar off/faint/strong stops. | #12, owner; #224 | ✅ |
| VIEW-6 | **Eyes are the only visibility control** (per-row + group-header toggles); selection stays separate from visibility. The view segment (single/highlight/all/none) is **deleted**. A **Focus slider** (0–100%, top of the Layers panel, persisted, default 25%) renders every non-selected VISIBLE layer at `1 − focus` — base and diff geometry alike; 100% shows only the selected layer. Panel rows ghost to mirror the canvas. Hiding every eye leaves a **truly blank canvas**; Split/Swipe still force the selected layer visible. | #224 owner-locked (variant B); supersedes #59/#173/#207/#210 | ✅ |
| VIEW-7 | The **board edge is a normal layer** (Mechanical › outline): plain eye toggle, visible by default; no special row, control, or legend entry. | #3/#157, owner clarified | ✅ |
| VIEW-8 | **Always-on crosshair + grid-snapped cursor** with a live coordinate readout, independent of measure mode (toggles live in Settings › Measure; snap defaults on and the readout says "· grid" so precision is honest). | #16/#17/#179, owner | ✅ |
| VIEW-9 | The **drawn grid adapts to zoom** (1-2-5 pitch selection) so a grid is visible at any zoom; **snapping stays at the configured pitch** — the display never changes what snap does. | #195 | ✅ |
| VIEW-10 | All trust/status chips (coordinate readout, hidden count, "1 / N layers" hint, measure hint) form one **bottom-left stack**, clear of the Split/Swipe identity labels. No status caption clutters the canvas top. | #10/#193/#194, #48 | ✅ |
| VIEW-11 | Layer rows: small **square** colour swatches, **copper-coloured Δ%**, no "changed first" caption; mm² detail on hover. | #5/#20/#7, #114 | ✅ |
| VIEW-12 | Colour/theme presets selectable in Settings; per-layer colours editable; dark/light themes with readable contrast in both. | #155, 2026-06-21 feedback | ✅ |
| VIEW-13 | Wheel semantics: plain wheel zooms at the cursor; Ctrl+wheel pans Y; Shift+wheel pans X; a physical notch feels the same on web and native. | 2026-06-20 feedback, #56 | ✅ |
| VIEW-14 | Warnings are named, concise, auto-fade to an icon, and never shift the layout on hover. | 2026-06-21 feedback | ✅ |
| VIEW-15 | **PDF rasterization DPI is user-settable** in Settings › Diff (chips 150/200/300, default **200** — deliberately higher than the CLI's 150 default, which keeps its `--dpi` flag). Changing it **re-rasterizes the loaded pair** from the retained source bytes; a DPI that breaches the raster caps (50 MP/page, 8192 px/side, 250 MP/doc) fails loud and the setting reverts. Persisted. | #223 | ✅ |
| VIEW-16 | **Export always produces findable output**: native writes `etchy-export/` next to the last opened input (absolute path in the toast); web downloads a single file directly and bundles a multi-file set into one zip (browsers block the 2nd+ automatic download). PDF mode exports the changed pages' overlay PNGs. | #222 | ✅ |

## 8. Measure system

| ID | Requirement | Source | Status |
|---|---|---|---|
| MEAS-1 | The rail's ruler icon is a **plain tool toggle**: click arms measure mode (icon highlighted while armed), click again disarms — it opens **no panel**. **Ctrl+M** toggles it too. No "Armed" widget — the bottom-left measure chip names the keys while armed. | #197, #211, owner | ✅ |
| MEAS-2 | Two clicks make a measurement; completed measurements **persist drawn on the board** (ruler + labels) until cleared by key per MEAS-4. **No measurements list UI** — the canvas is their only home. | owner spitball, #211 | ✅ |
| MEAS-3 | Units **mm / mil / inch** ("keep what we have"), selectable in **Settings › Measure** (with snap and crosshair — the tool's one home). The **units-cycle hotkey is preset-aware**: Altium **Q**, KiCad **Ctrl+U** (each tool's own units key); an explicit rebind overrides. | owner, #211 | ✅ |
| MEAS-4 | **Clear-measurements follows the input preset**: Altium → exact **Shift+C**, KiCad → **Esc** (after the in-progress point clears). A custom rebind stands the preset defaults down. Esc aimed at an open popup never clears the rulers. | #198, owner | ✅ |
| MEAS-5 | Measure clicks snap to the grid when snap is on; placement follows the visible snapped cursor. | #51 | ✅ |
| MEAS-6 | A measurement reports **ΔX, ΔY, and its angle** as well as the straight-line distance. | #208, #210 | ✅ |

## 9. Hotkeys

| ID | Requirement | Source | Status |
|---|---|---|---|
| KEY-1 | A **Settings › Hotkeys editor** lists every rebindable action with its binding; rebinding is press-to-capture, Esc cancels, with **reset to defaults**. Bindings persist. | #201, owner | ✅ |
| KEY-2 | A capture **refuses a key another action owns** (including fixed aliases and the preset clear/units keys) — one press must never dispatch two actions. Bindings match their **exact modifier set**. | #201 review | ✅ |
| KEY-3 | A capture armed while its editor is hidden cancels; a focused text field keeps its keystrokes. | #201 review | ✅ |
| KEY-4 | Defaults: Ctrl+M measure · F fit · S base cycle · preset units per MEAS-3 (Altium Q / KiCad Ctrl+U) · G grid · 1–5 modes (O/B/A legacy aliases) · J/K/arrows step layers · preset clear per MEAS-4. | as built, #211 | ✅ |

## 10. Rendering & performance

| ID | Requirement | Source | Status |
|---|---|---|---|
| PERF-1 | Smooth pan/zoom on a dense 16-layer board: tessellate once (cached by geometry key), transform per frame, cull off-screen, one merged mesh. | discovery, G6 | ✅ |
| PERF-2 | ~~In **all-layers** view, non-selected layers draw diff-only (base copper dropped).~~ **Superseded by #224:** the view segment is gone and the Focus model always draws every visible layer's base (dimmed by `1 − focus`); the #158 diff-only trick is retired — GPU-era surfaces carry the cost, and PERF-1's caching remains the perf backbone. | #158 → #224 | ↔ superseded |
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
| PROC-2 | CI is the merge gate. Local verification remains good practice before pushing: `cargo test`, `clippy --all-targets` warning-clean, `fmt --check`, and the wasm build for GUI changes. | working agreement | standing |
| PROC-3 | Feedback screenshots may show confidential boards: reference local paths in issues; never commit `deploy/feedback/`. | working agreement | standing |
| PROC-4 | Every feedback widget ships **Ctrl+Enter to send** and **Ctrl+V screenshot paste** with removable thumbnails. | working agreement | ✅ |

---

*Sources of record: `docs/PRODUCT_DISCOVERY.md` for the founding decisions; issue
tracker for every numbered feedback item; `docs/design/GUI_SPEC.md` for the full
viewer behaviour this document summarises.*
