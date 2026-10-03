# Native board ingestion implementation and release plan

Full replacement plan. Each pull request is independently reviewable, preserves
existing Gerber/PDF behavior, and adds its own trust tests. No runtime producer
tool is introduced.

## Release proposal

| Release | Content |
|---|---|
| 0.2.0 | Stable direct KiCad 6–10 `.kicad_pcb` parsing, native geometry projection, object change list, CLI/JSON v2/Markdown/HTML/native viewer/wasm viewer, pinned-oracle evidence. Recommended single release. |
| 0.2.1 | Corpus expansion and corrections only; no new format promise. If the owner stages UI work, 0.2.0 ships geometry plus JSON object data and 0.2.1 completes human/viewer object surfaces. |
| 0.3.0 | Experimental Altium `.PcbDoc` behind `native-altium-experimental`, only after the pure-Rust reader spike passes. |
| Later minor | Stable Altium only after parity, licence, format-span, resource and adversarial gates. The version is not promised in advance. |

Feature flags:

- `native-kicad`: parser, typed model and projector; enabled in normal CLI,
  desktop and wasm release builds once stable.
- `native-altium-experimental`: off by default; exact-pinned pure-Rust reader and
  adapter. It must not affect default builds.
- No `kicad-cli`, Python, Altium or `altium-monkey` integration feature.

## Phase 0: policy and evidence

### PR 1 — accept requirements and schema direction

Files: documentation and JSON schema drafts only.

1. Resolve `spec.md` open questions.
2. Update product scope, trust language, roadmap and non-goals.
3. Record supported KiCad majors as a target pending fixture gates, not an
   implementation claim.
4. Approve JSON v2 and stable warning/error codes.

Tests: documentation links, schema examples, existing schema v1 unchanged.

### PR 2 — corpus provenance and oracle recipes

Files: `corpus/native/kicad/`, test tooling, no runtime crates.

1. Add tiny synthetic KiCad 6/7/8/9/10 boards, each with exact producer/header
   version and one behavior per fixture.
2. Add old/new MIT Mad_RP2040 board files only after matching commits to current
   demo packs; record that existing exports may use plot settings native mode
   intentionally ignores.
3. Add manifests with source URL, commit, licence, expected record inventory,
   producer version, oracle command, export settings and classified differences.
4. Pin one KiCad baseline in ordinary relevant CI and the full matrix in
   scheduled/manual CI. Keep generated oracle packs where licensing permits.

Tests: manifest schema/hashes, licence/provenance checks, inventory checks, and
reproduction documentation. Existing demo assets are not modified in this PR.

## Phase 1: bounded syntax and typed board model

### PR 3 — `etchy-kicad` S-expression tree

Crates: new `crates/etchy-kicad`; workspace plumbing only.

1. Implement byte lexer, strings/escapes, atoms, lists and source spans.
2. Preserve all nodes and provide child-consumption accounting.
3. Add native/wasm resource-limit profiles and typed parse errors.
4. Parse root/header/version only; no geometry.

Tests: fixture syntax, whitespace/comments/escaping, invalid UTF-8, malformed
lists, depth/node/string/byte limits, numeric traps, integer overflow, fuzz-lite,
deterministic diagnostics, wasm compile. Run `cargo deny` despite no intended new
runtime dependency.

### PR 4 — version profiles and record inventory

Crate: `etchy-kicad`.

1. Add explicit KiCad 6–10 header-date profiles and accepted token aliases.
2. Decode layer/setup/net tables and inventory every top-level/nested record.
3. Add harmless-metadata allow-list; reject unknown material-bearing data.
4. Emit disposition/accounting without projection.

Tests: one fixture per accepted header date, cross-version token tables, unknown
top-level and nested fields, duplicate fields, future version, renamed layers,
record counts, fuzzed headers. Review gate: every fixture node is consumed once
or appears in accounting.

### PR 5 — shared native model

Crates: `etchy-core`, `etchy-kicad`.

1. Add `NativeBoard`, native layer identity, net labels, source IDs, typed object
   enums, diagnostics and projection provenance.
2. Keep existing resolved `Board` and diff APIs intact.
3. Add report types for object changes and native diagnostics behind no CLI yet.
4. Define checked coordinate/angle conversion.

Tests: serialization/schema v2, 1 nm boundaries, angle normalization, source ID
round-trip, warnings/accounting, v1 regression snapshots, native/wasm builds.

## Phase 2: KiCad objects and geometry

### PR 6 — layer stack, transforms and simple graphics

Crates: `etchy-kicad`, `etchy-core` geometry helpers.

1. Map copper stack, technical/documentation layers and user names.
2. Implement footprint front/back transforms as tested affine operations.
3. Project board/footprint line, rectangle, circle and polygon strokes/fills.
4. Assemble simple `Edge.Cuts`; reject bad topology.

Tests: every layer mapping and alias, renamed layers, flip/rotation/translation
compositions, exact-area shapes, open/branching/self-ambiguous outlines, cut-outs,
property tests that inverse transforms restore points, oracle per-layer deltas.

### PR 7 — tracks, arcs and curves

Crates: `etchy-kicad`, `etchy-core` reusable stroke/curve helpers.

1. Add track segments/arcs with width.
2. Add board and footprint arcs and cubic Beziers.
3. Lock deterministic flattening error and point caps.
4. Retain UUID/timestamp, net label and normalized centreline for object diff.

Tests: clockwise/counter-clockwise/full/degenerate arcs, widths, Bezier extrema,
quantization, cap failures, exact known bounds/areas, KiCad Gerber comparison,
`diff(A,A)` and old/new symmetry.

### PR 8 — pads, drills and placement

Crates: `etchy-kicad`, `etchy-core`.

1. Add basic, rounded, chamfered and trapezoid pads.
2. Add custom primitives and anchor/hull modes.
3. Apply pad plus footprint transforms and wildcard layer sets.
4. Add round/offset/oval drills, plating identities and placement objects/layer.

Tests: every shape; rotated/flipped/custom pads; repeated numbers; front/back
layers; PTH/NPTH; slots and offsets; exact annular areas; duplicate ownership;
excluded-position flags; oracle Gerber/drill/position comparisons; point caps.

### PR 9 — vias and unused copper layers

Crates: `etchy-kicad`, `etchy-core` model additions if approved.

1. Add through, blind/buried and microvia spans.
2. Add drill-span representation that cannot pair with a through drill by error.
3. Implement `remove_unused_layers`/`keep_end_layers` only for cases proven by
   saved data and oracle fixtures; reject indeterminate cases.

Tests: 2/4/16-layer stacks, all via types/spans, changed stack errors, used and
unused intermediate layers, end retention, drill identity, KiCad copper/drill
oracle comparisons. This PR cannot merge on undocumented guessed behavior.

### PR 10 — saved zones and keepouts

Crates: `etchy-kicad`, CLI flag plumbing limited to test harness.

1. Decode saved filled polygons and holes per layer.
2. Validate declared layer, topology and cheap consistency checks.
3. Implement missing-fill hard error and explicit partial opt-in.
4. Account for keepouts as non-material object-only data.

Tests: solid/hatched/multilayer fills, holes/islands, unfilled, deliberately stale,
malformed, outside-bounds, missing-layer and keepout cases. Compare saved native
geometry with normal export and `--check-zones` oracle output; classify every
mismatch. Assert partial status/warnings on opt-in.

### PR 11 — mask and paste rules

Crates: `etchy-kicad`.

1. Implement board/footprint/pad margin precedence.
2. Implement paste absolute plus ratio sizing and collapse policy.
3. Implement mask minimum-web merge after fixture proof.
4. Project direct mask/paste graphics; apply front/back transform.

Tests: precedence matrix, positive/negative/zero margins, ratio combinations,
topology collapse, minimum-web values immediately below/at/above threshold,
flipped footprints and direct graphics. KiCad Gerber symmetric differences must
be zero unless a checked-in native-policy classification explains them.

### PR 12 — CC0 stroke text and object-only text

Crates/files: `etchy-kicad`, a minimal font-data module, third-party notices.

1. Vendor the exact CC0 newstroke data with upstream URL/hash/licence evidence, generated from the CC0 font sources rather than KiCad's GPL-headed `newstroke_font.cpp` (see NCAD-TEXT-1).
2. Render stroke text/text boxes with transforms and limits.
3. Resolve only intrinsic variables available in the board.
4. Mark TrueType and external-variable text object-only with structured warning.

Tests: licence/NOTICE check, glyph outline snapshots, Unicode coverage/failure,
alignment/mirror/rotation/multiline/text-box cases, hidden text, reference/value,
project variables, TrueType names, output point limits, KiCad silk/copper/fab
oracle comparisons.

## Phase 3: object diff

### PR 13 — stable-ID and footprint/pad matching

Crates: `etchy-core` or a new small `etchy-native-diff` if compile boundaries
justify it; `etchy-kicad` only supplies objects.

1. Pair footprints by reference plus stable-ID corroboration.
2. Detect moved/rotated/flipped/combined changes.
3. Pair pads within footprints, including repeated pad numbers.
4. Produce deterministic structured changes and totals.

Tests: reference rename, regenerated IDs, duplicate/missing references, modulo
angles, flip plus rotation, pad edits/repeats, symmetry, deterministic order,
large hash-map benchmark and candidate caps.

### PR 14 — free geometry matching and spatial index

Crates: same object-diff boundary; use existing permissive spatial dependency if
appropriate rather than adding another.

1. Pair tracks/arcs/vias/zones/keepouts by stable ID.
2. Bucket unmatched objects by kind/layer span/net label and use an R-tree for
   nearby normalized-geometry candidates.
3. Accept only unique best matches; ambiguous cases become remove/add plus warning.
4. Add stored net-name labels without connectivity analysis.

Tests: moved/reshaped/re-netted objects, split/merged tracks, copied IDs,
ambiguous parallel tracks, zone outline/fill changes, large dense boards,
`O(n log n)` benchmark guard, candidate-limit errors, match symmetry.

## Phase 4: outputs and applications

### PR 15 — CLI dispatch, exit/gate behavior and JSON v2

Crates: `etchy-cli`, `etchy-kicad`, `etchy-core` reporting.

1. Detect two native inputs and reject mixed families/ambiguous containers.
2. Add object/zone/warning/gate flags from the spec.
3. Serialize complete JSON v2; retain v1 snapshots for old inputs.
4. Make any geometry or object change exit 1; errors remain 2.

Tests: direct files, git blobs, directory/zip selection, spoofed suffix/root,
mixed inputs, every flag conflict, warning upgrade, partial projection, object-
only diff, gate thresholds, exact exit codes, JSON Schema validation.

### PR 16 — summary, Markdown and HTML

Crates: `etchy-cli` reporting/templates.

1. Add bounded human rows and unbounded totals.
2. Add object tables, filters/details, warnings, accounting and policy statement.
3. Add click-to-frame metadata in HTML without changing geometry colors.

Tests: snapshots for every object status/kind, truncation, escaping hostile names,
empty changes, warning-only/partial runs, large list size cap, self-contained HTML,
cross-format total equality.

### PR 17 — desktop viewer Objects panel

Crates: `etchy-gui`.

1. Load native files on a cancellable background task with stage progress.
2. Add Objects panel, grouping/search/filter/detail and warning navigation.
3. Selection highlights bounds and layer without moving camera; explicit frame
   action moves it.
4. Preserve all locked viewer interactions and colors.

Tests: loader states/cancel, filtering, selection/camera invariant, frame action,
warning navigation, focus/visibility interaction, large virtualized lists,
existing GUI regression tests.

### PR 18 — wasm parser and viewer

Crates: `etchy-kicad`, `etchy-gui` wasm.

1. Enable direct browser load and tune measured browser limits.
2. Yield between stages and support cancellation.
3. Show exact resource-limit and partial/warning messages.
4. Measure binary-size and peak-memory changes on Mad_RP2040 and a dense board.

Tests: wasm build, real-browser smoke, synthetic limit boundaries, cancellation,
no filesystem/process/font lookup, native/wasm deterministic report equivalence,
memory/time budget checks on supported browsers.

## Phase 5: KiCad release gate

### PR 19 — parity matrix and adversarial audit fixes

Crates: all affected; corpus and CI.

1. Run the complete KiCad 6–10 fixture matrix using pinned producer oracles.
2. Generate a per-fixture native-vs-export difference ledger. No unexplained
   polygon, drill, outline or placement delta passes.
3. Audit unknown tokens, accounting, transforms, fill state, unused-layer pads/
   vias, font behavior, object ambiguity, overflow and browser amplification.
4. Fix findings in small follow-up PRs, never by broadening warning allow-lists.

Tests: full matrix plus extended fuzz/property/resource jobs. Exit criterion:
zero unexplained oracle differences and zero unresolved silent-miss findings.

### PR 20 — documentation and 0.2.0

Files: README, CLI help/reference, requirements, trust, developer guide, website,
Action docs, schema, notices, changelog and release workflow.

Publish supported header dates and producer versions, native-vs-fab distinction,
zone/text limitations, warning/error catalog, wasm limits, object non-goal line,
oracle reproduction and Mad_RP2040 provenance. Release only after static native
targets, wasm, `cargo deny`, corpus, Action and report-schema checks pass.

## Phase 6: Altium experimental

### PR 21 — pure-Rust reader decision spike

Crate: new `etchy-altium`, feature `native-altium-experimental`.

1. Pin the candidate `altium-format` version and audit its API, dependency tree,
   licence metadata, unsafe code, CFB bounds and unknown stream/record behavior.
2. Parse inventory only into the shared accounting model.
3. Compile native and wasm; measure representative boards.
4. Compare inventories with separately run `altium_monkey` and Altium where
   licensed fixtures permit. Do not copy their implementation or bundle them.

Tests: corrupt/cyclic/truncated CFB, stream/record bombs, unknown records,
licence CI, native target matrix, wasm parse caps. Go/no-go requires that every
record can be surfaced; otherwise choose or build another pure-Rust reader.

### PR 22 onward — incremental Altium adapters

Land separate PRs for layer stack/outline/simple primitives; pads/vias/component
transforms/drills/placement; saved pours/planes; mask/paste; text; object IDs;
CLI/GUI experimental surface. Each supports only oracle-proved cases and errors
on the rest. Reuse shared object diff/output/viewer code without weakening its
identity rules.

Release 0.3.0 experimental only after a redistributable corpus, explicit format-
era matrix, Altium-produced Gerber/drill/position comparisons, resource tests,
and adversarial review. Stable status waits for zero unexplained differences and
a supportable pure-Rust dependency/API. wasm is advertised only if browser
tests pass; feature-gated absence must fail clearly.

## CI layout

| Job | Relevant pull requests | Scheduled/manual |
|---|---|---|
| Existing fmt/clippy/test/deny | every PR | full matrix |
| Parser/model unit + fuzz-lite | native code | extended fuzz |
| Known-answer projection/object properties | native code | dense corpus |
| Checked-in oracle-pack comparison | native code | full corpus |
| Real pinned KiCad baseline | KiCad behavior changes | KiCad 6–10 matrix |
| wasm build + browser smoke | native parser/GUI changes | memory/perf matrix |
| JSON schemas/report snapshots | report changes | compatibility audit |
| Altium/`altium_monkey` oracle | never required at runtime; controlled relevant CI only | before experimental/stable releases |

## Release gates and risks

| Risk | Required response |
|---|---|
| Parsed syntax mistaken for supported geometry | Support table is token/behavior/fixture based; unknown material data errors. |
| Saved zone fill absent or stale | No outline substitution; hard error by default; visible partial opt-in; oracle freshness comparison. |
| Native result mistaken for fab output | Every report states projection policy and unapplied export settings; corpus classifies expected differences. |
| TrueType/project variable unavailable | Object-only plus visible warning; no fallback font or quiet literal expansion. |
| Via/pad unused-layer semantics unclear | Ship only oracle-proved cases; fail indeterminate boards. |
| Object matching mislabels remove/add as move | Stable IDs/references first; unique indexed fallback only; ambiguity remains remove/add with warning. |
| Large board or hostile file exhausts memory | Pre-amplification ceilings, point/candidate caps, fuzzing, separate measured wasm budgets. |
| Version 10 or later adds a token | Exact header/token profiles; newer data remains unsupported until reviewed. |
| Licence contamination | CC0 font provenance, permissive dependencies, `cargo deny`; AGPL oracle stays separate and test-only. |
| Altium reader is incomplete or unstable | Experimental exact pin, complete record accounting, independent exports, no stable deadline. |

## Definition of done for KiCad 0.2.0

1. All numbered stable requirements selected by the owner are implemented.
2. Every supported record/token has a disposition and every unknown-material
   fixture fails with a source-located typed error.
3. KiCad 6–10 corpus and Mad_RP2040 revisions pass known-answer and oracle tests;
   every non-zero oracle delta is documented and user-visible where relevant.
4. Geometry and object reports agree across summary, JSON, Markdown, HTML,
   desktop and wasm.
5. Same-board guard, fixed-point determinism, exit codes and existing inputs have
   regression coverage.
6. Native and wasm resource limits are published and tested.
7. Adversarial review has no unresolved silent-miss finding.
8. Licence/NOTICE review and `cargo deny` pass.

