# Native board ingestion specification

Status legend follows [`docs/REQUIREMENTS.md`](../../../docs/REQUIREMENTS.md):
✅ shipped · 🔶 built in an open PR · 🔜 accepted, not built · 📋 future · ↔
superseded. All requirements below are proposed and therefore 🔜 unless marked
otherwise.

## 1. Scope and policy

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-SCOPE-1 | etchy shall accept two KiCad board files (`.kicad_pcb`) or two Altium board files (`.PcbDoc`) and project each revision into the existing per-layer geometry diff. Mixed native formats, or one native board and one fab pack, shall fail unless a later requirement explicitly defines that pairing. | owner task 2026-10-03 | 🔜 |
| NCAD-SCOPE-2 | Native board ingestion is a board-manufacturing projection, not an object, net, connectivity, rule, BOM, component-value, or DRC diff. | existing `SCOPE-1`, `CORE-1`; owner task | 🔜 |
| NCAD-SCOPE-3 | `.kicad_sch` and `.SchDoc` are explicitly out of scope for this feature. Existing schematic PDF diff remains the schematic surface. Native schematic support would require a separate product decision because visual schematic rendering and connectivity comparison are different trust problems. | [`docs/ROADMAP.md`](../../../docs/ROADMAP.md); owner task | 🔜 |
| NCAD-SCOPE-4 | Change the `CLAUDE.md` non-goal line from “native CAD ingestion (KiCad/Altium/IPC-2581/ODB++)” to “native CAD ingestion other than supported KiCad `.kicad_pcb` and Altium `.PcbDoc` board input (IPC-2581/ODB++ remain out); net/connectivity diff, BOM/component diff, and DRC remain out.” Make matching changes to `SCOPE-4`, `SCOPE-5`, `TRUST.md`, and `ROADMAP.md` when implementation begins. | owner decision 2026-10-03; issue #122 recorded locally in requirements | 🔜 |
| NCAD-SCOPE-5 | Adding two board formats does not weaken “no silent misses.” The importer must account for every manufacturing-relevant record it encounters, produce complete `Board` geometry, or stop with a typed error. Unknown, unsupported, stale, or ambiguous constructs are errors, not skipped records or warning-only partial output. | `TRUST-1`, `TRUST-4`, [`docs/TRUST.md`](../../../docs/TRUST.md) | 🔜 |
| NCAD-SCOPE-6 | The phrase “same as exported Gerbers” means parity with a named and reported etchy export profile. The tool shall not claim it reconstructed an arbitrary historical fab pack when its KiCad plot settings or Altium OutJob were not supplied. | research finding; owner task | 🔜 |

Why the non-goal changes: the owner has accepted both board formats, so the old
blanket exclusion is false. The revised text remains narrow: it admits only
manufacturing geometry from two board formats and does not turn etchy into a
general ECAD review tool. The trust bar still holds because format admission is
gated on complete record accounting and parity tests; a parser recognizing a
file is not enough to produce a diff.

## 2. Input dispatch and CLI

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-CLI-1 | `etchy old.kicad_pcb new.kicad_pcb` shall select KiCad board mode. `etchy old.PcbDoc new.PcbDoc` shall select Altium board mode. Extension matching is ASCII case-insensitive for `.PcbDoc` and `.kicad_pcb`, but content magic/root shall be checked before parsing. | owner task | 🔜 |
| NCAD-CLI-2 | Detection shall use both the path suffix and content: KiCad must start with a valid `(kicad_pcb ...)` root after permitted leading whitespace; binary Altium must have CFB/OLE magic and required PcbDoc streams. A suffix/content disagreement is exit 2 with both observations named. | `TRUST-4`; Altium format evidence in research | 🔜 |
| NCAD-CLI-3 | Both inputs must resolve to the same input family. Example error: `native input types differ: old is KiCad .kicad_pcb; new is Altium binary .PcbDoc; compare two boards from one producer or export both to Gerber`. | existing PDF mixed-input behavior in CLI | 🔜 |
| NCAD-CLI-4 | Add `--native-profile <saved|etchy-default|PATH>`. For KiCad, default `saved` uses board plot parameters plus locked safety overrides; `etchy-default` ignores saved plot choices and uses the documented etchy layer/output profile. `PATH` is reserved for a versioned JSON profile. Altium initially accepts only `etchy-default`; other values fail. | reproducibility finding | 🔜 |
| NCAD-CLI-5 | Add `--kicad-cli <PATH>` and environment fallback `ETCHY_KICAD_CLI`; otherwise search `PATH`. The resolved executable path and `kicad-cli version` shall be shown with `--verbose` and recorded in JSON/HTML metadata. A missing executable shall say: `KiCad board input requires kicad-cli; install supported KiCad or export a Gerber pack`. | recommended KiCad route | 🔜 |
| NCAD-CLI-6 | Add `--native-keep-temp` only as a diagnostic flag. Default temporary exports shall use a private temporary directory and be deleted after parsing. On retained output, print the absolute directory. | operational safety | 🔜 |
| NCAD-CLI-7 | Existing output options, gates, and exit codes remain: 0 = accepted comparison with no gated diff, 1 = accepted comparison with a gated diff, 2 = ingestion/export/configuration error. Native conversion errors never become exit 0 or 1 with partial layers. | `CLI-1`–`CLI-3`, `TRUST-4` | 🔜 |
| NCAD-CLI-8 | Git-ref mode shall support native files only when each revision resolves to exactly one board file under the selected subdirectory. For KiCad CLI conversion, materialize each blob plus any required project/profile companion files into separate temporary directories. Ambiguous multiple-board discovery fails and lists the candidates. | `CLI-4`; no-guess policy | 🔜 |
| NCAD-CLI-9 | Existing PDF-only flags remain errors on board modes. Native-only flags are errors on Gerber-directory, zip, and PDF modes; they shall never be ignored. | existing CLI dispatch pattern | 🔜 |

### File-pair examples

```text
etchy old.kicad_pcb new.kicad_pcb
etchy --native-profile etchy-default old.kicad_pcb new.kicad_pcb
etchy --kicad-cli /opt/kicad/bin/kicad-cli old.kicad_pcb new.kicad_pcb
etchy old.PcbDoc new.PcbDoc
```

## 3. KiCad adapter

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-KICAD-1 | The supported implementation shall invoke `kicad-cli` as an external process; it shall not link, vendor, translate, or copy KiCad GPL plotting code. | KiCad GPL; etchy `SCOPE-7`; [`deny.toml`](../../../deny.toml) | 🔜 |
| NCAD-KICAD-2 | Before export, probe `version` and each required command's help. Maintain an allow-list of tested major/minor ranges and capability flags. Unknown majors fail by default with an override only if the owner approves a clearly named unsafe/experimental policy. | installed 7.0.11/current-help difference | 🔜 |
| NCAD-KICAD-3 | Minimum supported KiCad shall be 8.x or the first tested release whose `pcb export gerbers` provides `--check-zones`. Installed 7.0.11 is research evidence but shall not be declared supported for boards containing zones because its help lacks the refill check. Zone-free 7.x support is an owner decision, not an implicit fallback. | filled-zone trap; local CLI probe | 🔜 |
| NCAD-KICAD-4 | Export one Gerber per enabled manufacturing layer, Excellon drill, and CSV/ASCII position data using one reported profile. Use internal layer names, X2 attributes, absolute origin unless the profile explicitly says otherwise, millimetres for drill/position, and fixed Gerber precision. | CLI help; existing loader expectations | 🔜 |
| NCAD-KICAD-5 | `saved` profile shall use `--board-plot-params`, but etchy shall still enumerate and report the actual exported files/layers and lock safety-critical settings needed for comparable old/new outputs. If old/new resolved profiles differ in layer selection, origin, precision, zone policy, text inclusion, mask subtraction, or drill separation, fail and print the differences. | `CORE-5`; reproducibility finding | 🔜 |
| NCAD-KICAD-6 | Run zone validation/refill before each Gerber export. A non-zero zone check/export exit, diagnostic indicating failed refill, or absence of expected fill output is exit 2. The source board shall not be modified. | current CLI `--check-zones`; `TRUST-4` | 🔜 |
| NCAD-KICAD-7 | Include footprint pads and graphics according to KiCad plotting semantics; plot arcs, tracks, filled regions, board graphics, Edge.Cuts, mask, paste, and silk text through KiCad. Do not independently reinterpret those objects in etchy. | route choice | 🔜 |
| NCAD-KICAD-8 | Plot project-variable text only when its project context is available and substitution is verified. An unresolved `${...}` on a manufacturing layer, missing required `.kicad_pro`, unavailable font, or substitution diagnostic is exit 2. | KiCad issue 17732; `TRUST-4` | 🔜 |
| NCAD-KICAD-9 | Capture stdout, stderr, exit status, command name, and producer version for each child process. Error output shall identify old/new revision and stage (`gerbers`, `drill`, or `pos`) without exposing unrelated environment values. | diagnostic trust | 🔜 |
| NCAD-KICAD-10 | After export, pass the complete temporary pack through the existing Gerber, Excellon, placement, X2 reconciliation, caps, and `Board` construction paths. Zero recognized manufacturing layers is an error. | existing architecture | 🔜 |

## 4. Altium adapter

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-ALTIUM-1 | Implement Altium as an optional pure-Rust crate, initially using an exact pinned `altium-format` release after `cargo deny`, dependency, security, wasm, and API review. Do not depend on or translate `altium-monkey`; use it only as an independently run validation oracle where licence review permits. | research recommendation; `SCOPE-7` | 🔜 |
| NCAD-ALTIUM-2 | The adapter shall read binary CFB/OLE `.PcbDoc`. ASCII PcbDoc is out of the first release unless the selected dependency documents and tests it. Content limits shall cover file bytes, CFB sectors, directory entries, streams, records, decoded points, curves, text glyphs, and produced polygon points. | `CORE-7`; selected crate limitation | 🔜 |
| NCAD-ALTIUM-3 | Every manufacturing-relevant stream and record type shall be classified as supported or rejected. Unknown stream names may be ignored only from an audited allow-list of non-manufacturing metadata. Unknown records/fields in Board, layer stack, components, pads, vias, tracks, arcs, fills, regions, polygons, text, rules affecting mask/paste, or outline are exit 2. | `TRUST-1`, `TRUST-4`, `TRUST-7` | 🔜 |
| NCAD-ALTIUM-4 | Polygon pours shall use the saved poured primitives/regions linked to their polygon definition. Missing, stale, shelved, or inconsistent saved pour data shall be rejected or explicitly treated according to a tested rule; etchy shall not substitute the polygon boundary and shall not implement repour in the first release. | local `altium-monkey` inspection; silent-miss risk | 🔜 |
| NCAD-ALTIUM-5 | Resolve free and component-owned tracks, arcs, pads, vias, fills, regions, custom pad shapes, and supported text into board coordinates, respecting component side, rotation, mirroring, layer, and ownership. A primitive must be counted exactly once. | local `altium-monkey` object model; parity goal | 🔜 |
| NCAD-ALTIUM-6 | Generate copper, mask, paste, silk, outline/cutouts, PTH drill, NPTH drill, and placement layers. Unsupported pad-stack, via-stack, plane, mask/paste expansion, special-string, or font behavior on any selected layer is exit 2. | requested per-layer parity; existing `LayerKind` | 🔜 |
| NCAD-ALTIUM-7 | The initial renderer shall support only text/font paths proven against Altium Gerber output. Unknown TrueType fonts, embedded-font decoding failures, unresolved special strings, or unsupported stroke styles shall stop the comparison when they occur on a manufacturing layer. | silk fidelity requirement | 🔜 |
| NCAD-ALTIUM-8 | Resolve legacy and newer layer IDs through the parsed layer stack, then map physical order to `TopCopper`, ordinal `InnerCopper(n)`, and `BottomCopper`; map top/bottom mask, silk, and paste explicitly. Do not infer inner order from display names alone. | existing cross-scheme pairing; Altium versioned layer IDs | 🔜 |
| NCAD-ALTIUM-9 | `altium-format` parser success is not sufficient for support. Each accepted construct requires an etchy polygonizer test and an independent Altium-Gerber parity fixture. Until the release gate is met, builds expose Altium only through the `native-altium-experimental` feature and reports label the result experimental. | parser maturity finding; trust bar | 🔜 |
| NCAD-ALTIUM-10 | A `.PcbDoc` without its OutJob shall use the versioned `etchy-default` projection and say that arbitrary OutJob-specific options are unknown. If the owner later accepts `.OutJob`, it must become an explicit profile input; etchy shall not search sibling files and guess. | fidelity boundary; no-guess policy | 🔜 |

## 5. Mapping into `etchy-core`

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-CORE-1 | Add a pure ingestion interface that accepts bytes plus explicit format/profile metadata and returns `Board` plus an `IngestionManifest`. Process spawning, filesystem discovery, temporary directories, and UI messages stay outside `etchy-core`. If dependency weight warrants it, format adapters live in `etchy-kicad` and `etchy-altium` crates and feed `etchy-core` values. | `CORE-3`; current model | 🔜 |
| NCAD-CORE-2 | Reuse the existing `Board`, `Layer`, `LayerKind`, `PolygonSet`, fixed 1 nm coordinates, layer pairing, same-board guard, boolean diff, measurement, and render pipeline. Native input shall not create a parallel diff implementation. | `CORE-1`–`CORE-6` | 🔜 |
| NCAD-CORE-3 | Map layers as follows: F/front copper → `TopCopper`; inner physical order → `InnerCopper(1..)`; B/back copper → `BottomCopper`; front/back mask, silk, paste → matching kinds; plated/non-plated holes → `Drill`; Edge.Cuts/board profile → `Outline`; centroid markers → `Placement`. Non-manufacturing mechanical/documentation layers are excluded unless the profile explicitly maps them to `Documentation` and reports them. | existing `LayerKind`; `IN-4` | 🔜 |
| NCAD-CORE-4 | Native layer labels shall contain stable source names for diagnostics, but pairing identity remains `LayerKind`. Duplicate physical kinds, ambiguous inner order, or multiple outlines that cannot be deterministically composed shall fail before diffing. | `CORE-5`; `TRUST-4` | 🔜 |
| NCAD-CORE-5 | `IngestionManifest` shall include input family, source format version, adapter and dependency version, producer executable version, profile ID/hash, source layers, emitted layers, per-record-type seen/emitted/rejected counts, zone/pour state, warnings, and verification status. It shall be serialized into JSON and shown in HTML/GUI details. | `TRUST-1`; reproducibility | 🔜 |
| NCAD-CORE-6 | Negative-plane semantics shall be converted to material geometry before constructing native `Layer` values, or represented with the existing polarity field only when proven equivalent. Positive/negative ambiguity must stop ingestion. | existing polarity behavior; plane risk | 🔜 |

## 6. GUI, wasm, and Action behavior

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-GUI-1 | Native egui shall allow selecting/dropping two matching `.kicad_pcb` files or two matching `.PcbDoc` files and show conversion progress before the normal viewer. Native errors shall use the same typed causes as CLI errors. | owner task; shared-loader principle | 🔜 |
| NCAD-GUI-2 | Native KiCad GUI support uses the same discovered/configured `kicad-cli` and profile logic as the CLI. It shall never block the UI thread; cancellation terminates the child and removes temporary output. | route choice; GUI responsiveness | 🔜 |
| NCAD-GUI-3 | The wasm viewer cannot execute `kicad-cli`. In the KiCad CLI-based release it shall reject `.kicad_pcb` immediately with: `KiCad board conversion needs kicad-cli and is available in the desktop app/CLI; export a fab-pack zip for this browser viewer`. It shall not upload the board to a service. | platform constraint; offline model | 🔜 |
| NCAD-GUI-4 | The wasm viewer may enable `.PcbDoc` only after the Rust Altium adapter builds on `wasm32-unknown-unknown`, passes browser memory/CPU caps, and meets the same parity corpus as native. Until then it fails with an equally explicit availability message. | recommended Altium route; trust bar | 🔜 |
| NCAD-GUI-5 | The Open dialog filters and drop target shall list only formats available in that build. Help/About shall state native-format feature flags and KiCad executable status. | no misleading UI | 🔜 |
| NCAD-ACTION-1 | The GitHub Action shall add an opt-in KiCad setup mode with a pinned supported major and verified package source/container digest. It shall never silently use whatever `kicad-cli` happens to be first on `PATH`; detected version is printed and included in artifacts. | `CLI-5`; reproducibility | 🔜 |
| NCAD-ACTION-2 | Altium Action support uses the Rust feature and needs no Altium or Python runtime. Experimental status and feature version appear in the PR summary until the release gate is removed. | recommended route | 🔜 |

## 7. Error and warning contract

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-ERR-1 | The following are hard errors: mismatched input families; false extension/magic; unsupported source or producer version; missing external executable; producer export failure; absent/failed zone fill; absent/inconsistent pour fill; unknown manufacturing record; unresolved layer; unsupported pad/via/text/font/rule; profile mismatch; expected output missing; resource cap; and zero recognized layers. | `TRUST-4` | 🔜 |
| NCAD-ERR-2 | Warnings may describe non-geometric metadata, verified substitutions, ignored allow-listed records, or profile limitations. A warning cannot authorize dropping geometry or changing material semantics. | `TRUST-1` | 🔜 |
| NCAD-ERR-3 | Every error names revision (`old`/`new`), file, format/version, layer or object/stream when known, adapter stage, and corrective action. Error text must not imply that a partial result is safe. | usability; trust | 🔜 |
| NCAD-ERR-4 | Child-process timeouts, signals, malformed output, and unexpected output filenames are exit 2. etchy shall reject output written outside its allocated temporary directory. | hostile input/process safety | 🔜 |
| NCAD-ERR-5 | If either revision fails, no diff report, SVG, HTML, or changed/no-change verdict is emitted. Diagnostic manifests may be retained only under the explicit diagnostic option. | no partial output | 🔜 |

Example errors:

```text
etchy: old board requires KiCad zone refill, but kicad-cli 7.0.11 has no
--check-zones support; install a supported KiCad version or export Gerbers

etchy: new Board.PcbDoc: unsupported manufacturing record in Pads6/Data
(record 418, pad-stack mode 7); no diff was produced

etchy: native profiles differ: old uses auxiliary drill origin; new uses
absolute origin; choose one explicit profile for both revisions
```

## 8. Verification and release gates

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-TEST-1 | For each supported producer version/profile, export the native board to Gerber/Excellon/position using the reference producer, ingest both native and exported paths, and assert equal layer sets plus zero per-layer geometric difference after the same fixed-point conversion. | parity promise; `TRUST-5` | 🔜 |
| NCAD-TEST-2 | Seed KiCad tests with the public `Cimos/Mad_RP2040` project already represented by exported demo assets in [`crates/etchy-gui/assets/demo`](../../../crates/etchy-gui/assets/demo). Add licence/provenance metadata and native revisions before copying any board file into the corpus. | owner task; existing demo | 🔜 |
| NCAD-TEST-3 | The KiCad corpus shall cover versions 6, 7, 8, and 9 as inputs even if some require a newer supported CLI to read/upgrade in memory; test tracks, vias, through/SMD/custom pads, footprint rotation/flip, arcs, filled/unfilled/stale zones, keepouts, cutouts, mask/paste rules, stroke and TrueType silk, variables, Edge.Cuts, PTH/NPTH, placement, renamed layers, and 2/4/16-layer stacks. Unsupported combinations shall have golden error tests. | research traps; owner task | 🔜 |
| NCAD-TEST-4 | The Altium corpus shall span disclosed producer eras and cover free/component primitives, rotations/mirroring, custom pads, via spans, internal/negative planes, saved/shelved/unpoured pours, cutouts, mask/paste rules, stroke/TrueType/special text, outline variants, PTH/NPTH, placement, and old/new layer identifiers. Each accepted fixture needs Gerber/NC/position outputs made by a named Altium version and exact OutJob/profile. | research traps; `TRUST-5` | 🔜 |
| NCAD-TEST-5 | Parser-only fixtures are insufficient. Require known-answer synthetic geometry, real-board parity, `diff(A,A)=empty`, old/new symmetry, deterministic bytes, layer-count accounting, unsupported-record rejection, fuzzing, and amplification-limit tests. | `TRUST-5`, `TRUST-7`, `CORE-7` | 🔜 |
| NCAD-TEST-6 | Run a deliberate adversarial review before each format leaves experimental status. Review unknown-record handling, integer overflow, curve tessellation, transformations, duplicate ownership, zone/pour state, font fallback, layer mapping, CFB bounds, child-process paths, and temporary-file cleanup. | `TRUST-7` | 🔜 |
| NCAD-TEST-7 | A supported-version matrix shall be published. New producer majors remain unsupported until corpus parity passes; dependency updates cannot broaden claimed support merely because parsing succeeds. | fail-loud policy | 🔜 |

## 9. Explicit non-goals

1. Native `.kicad_sch`, `.SchDoc`, symbol/library, project-wide, or
   connectivity diff.
2. Net, rule, DRC, impedance, BOM, value, fitted-variant, or component-identity
   reporting.
3. Editing or rewriting native CAD files.
4. Reimplementing KiCad zone fill or Altium polygon repour in the first release.
5. IPC-2581 or ODB++ input.
6. Bundling KiCad, Altium Designer, Python, or `altium-monkey` into etchy's
   static release archives.
7. Browser upload/conversion services.
8. Guessing an absent Altium OutJob or claiming parity with an unknown fab
   export profile.
9. Auto-aligning different boards or relaxing the same-board guard.

## 10. Owner decisions still open

1. Is the simple two-file KiCad command allowed to default to saved board plot
   parameters, or must `etchy-default` be the default for cross-revision
   reproducibility?
2. Must KiCad 7 zone-free boards be supported, or is KiCad 8+ an acceptable
   minimum because the installed 7.0.11 lacks `--check-zones`?
3. Is native `.kicad_pcb` support in the browser a release requirement? If yes,
   the recommended CLI bridge cannot satisfy it; the project must fund a direct
   plotter or approve a local companion process. A remote conversion service is
   not proposed.
4. For Altium, should the command accept an explicit `.OutJob`/versioned profile
   later, or is the documented `etchy-default` projection sufficient?
5. May `altium-format` 0.1.x be adopted while its maintainer describes a breaking
   rewrite, or should etchy wait for the new release/API before implementation?
6. What independent Altium installations and versions are available to create
   legally redistributable parity fixtures and Gerber/NC/position oracle output?
7. Does the owner require Altium wasm in its first stable release, or may wasm
   remain a fail-loud unsupported surface until memory and parity gates pass?
8. Should the release JSON schema add ingestion metadata in v1 as optional
   fields, or should native input trigger a schema v2?
