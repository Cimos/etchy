# etchy — feedback log

Every piece of product feedback, backdated to its source, with what became of
it. Feedback is the origin of most requirements in
[`REQUIREMENTS.md`](REQUIREMENTS.md) — this log is the traceability between the
two: raw item → requirement ID and/or issue → outcome.

**Sources:** the in-app feedback widget (`deploy/feedback/*.jsonl`, gitignored —
timestamps + severities quoted from it), the hosted M1 demo round
(`docs/feedback-m1.md`), review-page decision submissions, and the GitHub issue
tracker. Reporter is the owner unless noted. Repeated reports of one problem are
merged into a single row (count noted).

**Outcome legend:** ✅ shipped · 🔶 built, in an open PR · 🔜 open (issue) ·
📋 deferred/future · ↔ superseded by a later decision (see §9).

---

## 1 · 2026-06-18 — hosted M1 demo (3 testers, 6 submissions)

| Feedback | Became | Outcome |
|---|---|---|
| Mode buttons (overlay/before/after) too small to click | larger hit-targets; later the segmented top bar (GUI-4) | ✅ |
| Layer sidebar icons read as meaningless boxes; mm² clutters the sidebar | layer-list redesign #114 → VIEW-11 (Δ% inline, mm² on hover, square swatches) | ✅ |
| Red/green overlap hard to see — needs a distinct treatment | overlap/polarity render fixes; base blending (VIEW-5) | ✅ |
| "Some traces missing on layers" (possible correctness bug) | the missing-traces investigations → polarity fixes on the real-board corpus (#145); guarded by TRUST-5 | ✅ |
| Ingest drill + pick-and-place files | scope decision → Excellon (#62) + P&P (#115) shipped (SCOPE-2, IN-2/IN-3) | ✅ |
| Keyboard shortcuts for modes (big shared screens) | mode hotkeys 1–5/O/B/A → KEY-4 | ✅ |
| Per-layer change % in the sidebar | #114 → VIEW-11 | ✅ |

## 2 · 2026-06-20 — widget round 1 (native + web, first deep session)

| Feedback | Became | Outcome |
|---|---|---|
| Phantom objects when zoomed out; large "copper added" that vanishes on zoom; tiny points reading as big changes (3 reports) | the LOD rework: fade + fixed marker dots, no phantom blobs → TRUST-2 | ✅ |
| Overlapping copper shows black / looks unchanged with base off (3 reports incl. 06-21) | polarity + base display fixes; base opacity blend (VIEW-5) | ✅ |
| Some tracks not rendering; missing footprints (Blocking) | rendering correctness passes + real-board corpus (#145) | ✅ |
| Remove the numeric/analytic figures from the left sidebar | #114 → mm² to hover (VIEW-11) | ✅ |
| Colour per layer would help distinguish overlaps ("like Altium's compare") | per-layer colours + presets → VIEW-12 | ✅ |
| Side-by-side comparison view | Split + Swipe modes (#91, #61) → VIEW-4 | ✅ |
| Group layers Copper/Mask/Silk/Drill; support KiCad and Altium naming | layer grouping (#58) + classification (IN-4) | ✅ |
| "This symbol is used a lot… doesn't help" (square icon) | painter-drawn icon overhaul → GUI-5 | ✅ |
| Ctrl/Shift + scroll to pan X/Y | wheel-axis panning (Ctrl=Y, Shift=X) — spec'd in GUI_SPEC §3 | ✅ |
| Hotkeys, e.g. S for base | S base-cycle → KEY-4 | ✅ |
| Feedback widget: screenshot doesn't scale/move with the window | widget iterations; Ctrl+V/Ctrl+Enter parity → PROC-4 | ✅ |
| Native app laggy dragging/scrolling; web fine (2 reports) | perf passes: cached tessellation, wheel-unit parity (#56) → PERF-1 | ✅ |
| "etchy" top-left could be the golden logo | branding → GUI-5 (evolved 07-05/07-06, see §9) | ✅ |
| Measure tool + "look into other helpful tools" | the measure system → MEAS-1..5 | ✅ |
| Unclear which revision you're viewing in Before/After | per-half labels, then old/new naming (GUI-8) + label parity (GUI-7) | ✅ |
| "Info up the top serves no purpose there" | progressive top-bar declutter → GUI-4 | ✅ |

## 3 · 2026-06-21 — widget round 2

| Feedback | Became | Outcome |
|---|---|---|
| Zoomed out everything reads all-green; artifacts change size drastically between zoom levels (2 reports) | LOD fade bands (TRUST-2) — the all-green bug fix | ✅ |
| "What is all this?" (debug-ish caption clutter) | canvas caption removal (#178) → VIEW-10 | ✅ |
| Board edge/cutouts visible on all layers? | outline drawn as context on both halves; evolved to board-edge-as-layer (VIEW-7, see §9) | ✅ |
| Heads-up/warning box: annoying, needs title, must be concise, should auto-fade to an icon, hover must not shift the layout (5 reports) | the warnings rework (named, concise, phased auto-fade, layout-stable) | ✅ |
| Top bar: default sizes too small, no etchy branding | top-bar redo (#57/#166) + brand icon (GUI-5) | ✅ |
| "demo — rev A → rev B" doesn't reflect the boards | real labels from paths → GUI-7 | ✅ |
| "Before shows as added — seems backwards?" | naming/legend clarity → old/new everywhere (GUI-8) | ✅ |
| Per-layer colour change; Altium/KiCad-like default palettes; dark/light modes; light-mode contrast off (4 reports) | Settings › Colours + presets + themes → VIEW-12 | ✅ |
| Background colours differ between web and native | shared canvas colour settings | ✅ |
| Web page tab icon | favicon (brand set) | ✅ |
| Noise filter: wider user-chosen range ("not silly"), slider felt non-linear | surfaced min-area filter with sane range → TRUST-3 | ✅ |
| Base off/faint/strong does nothing in Split; board edge missing in Split; text-over-text in Split (3 reports) | Split-view fixes (#44/#45/#48) | ✅ |
| Shift-scroll works, Ctrl-scroll doesn't | the raw-MouseWheel modifier fix (GUI_SPEC §3) | ✅ |
| Scroll artifacts appear/disappear while panning | transform-cache determinism fixes | ✅ |
| "Square missing in identical footprints — error on this end" | corpus investigation (#145 real-board fixes) | ✅ |
| Colours window should close when clicking the app behind it | click-outside dismiss (later: Settings became a panel, #199) | ✅ |

## 4 · 2026-07-04 — widget (two batches) + wireframe approvals

| Feedback | Became | Outcome |
|---|---|---|
| Add/remove artifacts showing (diagonal streaks) | #153 → edge-stroke feature removed entirely | ✅ |
| Noise filter belongs in Settings | #154 → Settings › Diff | ✅ |
| Top bar mustn't wrap; scale with window | responsive tiers (#57) | ✅ |
| Altium-style eye icon for visibility | eye toggles (#58/#114) | ✅ |
| Swipe divider really hard to grab | #61 → wide grab band + handle | ✅ |
| Wireframe picks: left-rail Settings style; "yes" to #57 group B, #59, #58/#114 | drove the Settings restyle (#152) and the approved builds | ✅ |
| Colour options list liked | #155 presets | ✅ |
| Drills disappear in a mid-zoom band, return when zoomed further | #156 → LOD marker floor (TRUST-2) | ✅ |
| Board edge should live in the Layers window, not the top bar | #157 (evolved → VIEW-7, see §9) | ✅ |
| Native laggy with all layers, fine with one | #158 → all-view base cut (PERF-2) | ✅ |
| Open A/Open B + Help do nothing on native | #159 → WSL-aware Help/open fixes | ✅ |
| Standardise old/new naming; fold Open A/B into a menu | #160 → GUI-8 | ✅ |

Page decisions the same day: build everything in clusters, bugs first; close the
done backlog (#105 waiver).

## 5 · 2026-07-05 — three widget rounds + the shell direction

| Feedback | Became | Outcome |
|---|---|---|
| "Visual artifacts still showing" (10:56) | stale-wasm false alarm — verified clean post-#153; lesson: verify the served build | ✅ |
| "Other" layers are actually copper — misparsed | #176 → IN-4 (`.gl<n>` ordinal fix, #180) | ✅ |
| Hide-all keeps one layer visible | #173 → VIEW-6 | ✅ |
| Mechanical outline **is** the board edge — remove the concept + legend icon | #3 → VIEW-7 (final scope set 07-05 quiz: normal layer, on by default) | ✅ |
| Clicking a layer refits the view — camera must stay put | #169 → VIEW-1 | ✅ |
| Colour icon smaller — small square | #5 → VIEW-11 | ✅ |
| Mode/base on different snap points; move base to Layers; tidy top buttons | #6 → GUI-4 + VIEW-5 | ✅ |
| Remove "changed first" wording | #7 → VIEW-11 | ✅ |
| Fit fits the changes, not the whole board | #170 → VIEW-2 | ✅ |
| Etchy should be the E icon, not the word | #9 → GUI-5 | ✅ |
| Layer-state text can leave the viewer | #178 → VIEW-10 | ✅ |
| VS Code-style toolbar: side rail, hideable, cog at bottom | #11 → GUI-1..3 (the shell) | ✅ |
| Base off/faint/strong should be a slider | #12 → VIEW-5 | ✅ |
| Swipe moves the PCB while dragging the bar | #171 → VIEW-4 (took three fixes — see the #175 history) | ✅ |
| Swipe bar limited to part of the width | #171 → VIEW-4 full-range | ✅ |
| Always snap cursor to grid, even when not measuring | #16 → VIEW-8 | ✅ |
| Always-on crosshair | #17 → VIEW-8 | ✅ |
| Left-click should pan too | #172 → VIEW-3 | ✅ |
| Web and native show different old→new labels | #177 → GUI-7 | ✅ |
| Δ% should be the copper colour | #20 → VIEW-11 | ✅ |
| Page: "more realistic wireframes"; then "Measure into its own tab? spitball"; answers: Measure **and Export** as rail tabs, rail **flippable** | MEAS-1, GUI-2/GUI-3 | ✅ |
| Page (20:55): grid-snap default **on**; board-edge = **normal layer, on by default**; units = **keep mm/mil/inch**; trust-count relocation OK | VIEW-7/VIEW-8, MEAS-3, TRUST-3 | ✅ |

## 6 · 2026-07-06 — shell review (12 notes) + overnight decisions

| Feedback | Became | Outcome |
|---|---|---|
| Remove the E above the Layers icon (rail) | #190 → GUI-3/GUI-5 | ✅ |
| Grid/cursor location → bottom-left overlay | #193 → VIEW-10 | ✅ |
| Use the actual etchy branding E, not this one | #191 → GUI-5 | ✅ |
| Remove the scroll window (Export list) — space to make it large | #196 | ✅ |
| Hidden-count chip → same bottom-left overlay | #194 → VIEW-10 | ✅ |
| Formats info block unnecessary | #196 | ✅ |
| Remove Armed button; note "Ctrl+M to measure" | #197 → MEAS-1 (hotkey set to Ctrl+M by quiz) | ✅ |
| Grid should scale with zoom like Altium/KiCad | #195 → VIEW-9 | ✅ |
| Keyboard shortcut to clear measurements ("Ctrl+C") | #198 → MEAS-4 (evolved to preset keys, see §9) | ✅ |
| Settings icon: a proper gear | #192 → GUI-5 | ✅ |
| Move Settings into the side panel like the rest | #199 → GUI-6 | ✅ |
| Open/Fit/Help look different from the mode segment | #200 → GUI-4 | ✅ |
| **Native: no buttons click; board drag works** (22:46) | #203 → BUG-1 (WSLg pointer-offset suspected; diagnostic pending) | 🔜 |
| Page: shell "rework needed" (the 12 above); **squash the 6 slices into 1–2 PRs**; more GUI polish next | the overnight round + squashed PR #202 | ✅ |
| Quiz: **Ctrl+M**; Settings panel = **stacked collapsible sections**; clear key **preset-aware** (Altium Shift+C / KiCad Esc); **Hotkeys editor** wanted | MEAS-4, GUI-6, KEY-1..4 | ✅ |

## 7 · 2026-07-07 — docs + current review round

| Feedback | Became | Outcome |
|---|---|---|
| Page: requirements docs — merge #204 as-is; nothing missing; review the shell chain next | #204 merged; chain #167→#175→#182→#202 merged | ✅ |
| Settings rows: hard to tell the setting name ("Theme") from the clickable values (dark/light) | #205 → new requirement GUI-9 (shipped in #210) | ✅ |
| (chat) All feedback must live in documentation as requirements, backdated | this log + REQUIREMENTS.md | ✅ |
| (chat) Support other EDA tool formats — X1/X2 etc. | IN-6 | ✅ X1/X2 · 🔜 more schemes |
| (chat) Report generating as a requirement | CLI-7 | ✅ |
| (chat) GitHub Action: 5-line adoption; every Gerber-touching PR gets a layer-by-layer diff comment | CLI-8 | 🔜 validate |
| Show/hide-all and single/highlight/all overlap — merge into one control group (22:32) | #207 → VIEW-6 refinement (shipped in #210) | ✅ |
| Measurements need ΔX/ΔY offsets + angle, not just distance (22:33) | #208 → MEAS-6 (shipped in #210) | ✅ |
| Measure tab must not auto-arm the tool; move its settings into Settings; drop the Measurements list — the tab can go (23:08) | #211 → MEAS-1/2/3 rework (rail ruler = plain tool toggle, no panel; Settings › Measure; rulers persist on canvas, cleared by key) | 🔶 |
| Settings panel: others resize, it doesn't — and it re-sizes itself when sections open/close (23:09) | #212 → GUI-6 refinement (resizable, stable width) | 🔶 |
| Swipe still pans (web, 100%, post-merge) — fixes verified on main, so likely a stale wasm tab; the running build must be provable (23:10) | #213 → GUI-10 build stamp (git sha in Help + brand tooltip) | 🔶 stamp · 🔜 re-test |

## 7b · 2026-07-12 — PDF-diff review round

| Feedback | Became | Outcome |
|---|---|---|
| Export does nothing on the PDF diff (17:16) | #222 → VIEW-16 (native: absolute export dir next to the inputs; web: DOM-attached anchor + multi-file sets bundled into one zip) | 🔶 |
| PDF render resolution needs to be higher (17:16) | #223 → VIEW-15 (Settings › Diff DPI chips 150/200/300, GUI default 200, re-rasterize on change; CLI keeps 150 + `--dpi`) | 🔶 |
| Layers visibility model (segment + eyes) feels off — deep-dive + quiz wanted (17:17) | #224 → VIEW-6 rework, owner-locked variant B (view segment deleted; eyes-only visibility + Focus slider; base slider → Settings › Diff; PERF-2 superseded) | 🔶 |

## 8 · Decision record (owner quiz answers, dated)

| Date | Decision |
|---|---|
| 2026-07-04 | Build all feedback in clusters, bugs first; one big review after. Close the done backlog without per-issue review (the #105 waiver). Old/new naming. |
| 2026-07-05 | VS Code shell direction locked. Measure **and** Export become rail tabs; rail side flippable. Grid-snap on by default. Board edge = a normal layer, on by default. Units: keep mm/mil/inch. Bugs before shell; then "keep going now, stacked"; shell as incremental PRs. |
| 2026-07-06 | Shell rework per the 12 notes. Squash the slices into one review PR. Ctrl+M measure. Settings panel with stacked collapsible sections. Clear-measurements preset-aware (Altium Shift+C, KiCad Esc). Hotkeys editor wanted. |
| 2026-07-07 | Requirements docs merged as-is. Merge chain executed (authorised: merges + branch cleanup). All feedback → documented requirements, backdated (this log). |

## 9 · Superseded / evolved items (the conflicts, resolved)

Where later feedback changed an earlier ask — kept here so the history reads
honestly. None are open disputes; flag anything you want reopened.

| Earlier ask | Later decision that superseded it |
|---|---|
| mm² figures removed from the sidebar entirely (06-18/06-20) | Δ% inline + mm² on hover (#114) — data kept, clutter gone |
| Board edge "on all layers?" (06-21), then "board edge into the Layers panel" (07-04, #157), then "remove the concept" (07-05) | Final: the outline **is** a normal layer, on by default (VIEW-7) |
| Clear measurements = **Ctrl+C** (07-06 note) | Preset-aware: Altium **Shift+C**, KiCad **Esc** (07-06 quiz) |
| Plain **M** to measure (the shell plan) | **Ctrl+M** (07-06 note + quiz) |
| Painter-drawn E monogram as branding (shell PR A) | The real pad-built brand icon; no rail monogram (07-06, #190/#191) |
| Drill + P&P ingestion flagged as possible scope creep (06-18 triage) | Accepted into scope and shipped (SCOPE-2) |
| Measure as a rail **tab** whose icon arms the tool + opens the panel (MEAS-1 as built, #197); the tab held the **measurements list** (MEAS-2) and the **units picker** (MEAS-3) | 07-07 23:08 reversal (#211): the ruler icon is a **plain tool toggle** — no panel, no auto-arm side effects beyond the toggle itself; the **list UI is gone** (rulers persist on canvas, cleared by key); snap/crosshair/units live in **Settings › Measure**; the units hotkey went preset-aware (Altium Q / KiCad Ctrl+U) |
| Native lag → GPU transform path (#106) | GPU path stays off — it breaks marker-LOD trust (PERF-3); lag fixed by the all-view base cut instead (#158) |
| Show/hide-all + view segment merged into **one visibility control group** single/highlight/all/none (07-07, #207 → VIEW-6 as then built) — itself the first reversal of the visibility controls | 07-12 second reversal (#224): the segment and the eyes interacted weirdly, so the **segment is deleted entirely** — per-row/group **eyes are the only visibility control**, a **Focus slider** replaces highlight-dimming (non-selected visible layers at `1 − focus`, base and diff), the base slider moved to Settings › Diff, and the #158 all-view diff-only trick (PERF-2) retired with the segment |

---

*Maintenance: when new feedback arrives, add the dated row here, open the
tracking issue, and add/adjust the requirement in REQUIREMENTS.md — in the same
change.*
