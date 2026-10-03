# Native board ingestion specification

Full replacement specification. Status legend follows `docs/REQUIREMENTS.md`:
✅ shipped · 🔶 open PR · 🔜 accepted, not built · 📋 later.

## 1. Scope and product contract

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-SCOPE-1 | etchy shall read two revisions of the same native PCB board directly in Rust. No runtime KiCad, Altium Designer, Python, `kicad-cli`, or `altium-monkey` is permitted. Producer tools may run only as development/CI test oracles. | owner 2026-10-03 | 🔜 |
| NCAD-SCOPE-2 | KiCad `.kicad_pcb` in KiCad 6–10 S-expression generations is first. Altium `.PcbDoc` follows through a pure-Rust reader. `.kicad_sch` and `.SchDoc` are out of scope. | owner 2026-10-03 | 🔜 |
| NCAD-SCOPE-3 | Native comparison shall produce both the existing per-layer polygon diff and a structured object change list. Geometry covers copper, mask, paste, silk, drill, outline, documentation and placement as specified below. | owner 2026-10-03 | 🔜 |
| NCAD-SCOPE-4 | Native projection describes board-file content under etchy's published rules. It shall not claim identity with a fabrication pack because plot/export settings and external project/font data may differ. | owner decision; research | 🔜 |
| NCAD-SCOPE-5 | Existing fab-pack and PDF behavior shall not change. Mixed native/fab-pack comparison is rejected in the first release because their projection policies are not equivalent. | compatibility | 🔜 |
| NCAD-SCOPE-6 | Same-board revisions only. Native inputs use the existing physical-extent guard: mismatch beyond 1 mm or 2% of span fails; `--force` remains an explicit bypass. No auto-alignment. | owner; `CORE-4` | 🔜 |
| NCAD-SCOPE-7 | etchy remains `MIT OR Apache-2.0`; all linked data and dependencies must pass `cargo deny`. | owner; `SCOPE-7` | 🔜 |

Replace the old non-goal wording with:

> etchy reports changes to stored PCB objects and displays their stored net names
> as labels; it does not infer electrical connectivity, compare net topology,
> validate routing, or compare a bill of materials, values, variants, or fitted-
> component intent. DRC, native schematics, IPC-2581 and ODB++ remain out of
> scope. Native Altium board ingestion is planned after KiCad.

An object change list is therefore neither a connectivity diff nor a BOM. A
footprint row says that a placed board object changed; it does not assert that a
part number, value, sourcing choice, fitted variant or circuit function changed.

## 2. Input selection and CLI

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-CLI-1 | `etchy old.kicad_pcb new.kicad_pcb` shall auto-detect native KiCad by suffix and root token. Both checks must agree. Git-ref invocation may resolve two board blobs at the same repository-relative path. | existing CLI; trust | 🔜 |
| NCAD-CLI-2 | A directory or zip containing exactly one `.kicad_pcb` may be accepted; zero or multiple candidates is a typed ambiguity error. Schematics and project files are ignored and counted, not selected. | input behavior | 🔜 |
| NCAD-CLI-3 | Add `--objects <all|changed|none>`, default `changed`. `all` includes unchanged paired objects in JSON only; human formats still show changed rows plus counts. | owner object list | 🔜 |
| NCAD-CLI-4 | Add repeatable `--object-kind <footprint|pad|track|arc|via|zone|keepout>` as a presentation filter. It shall not change geometry, exit status, totals or JSON source data unless JSON is explicitly requested with `--filtered-json`. | trust | 🔜 |
| NCAD-CLI-5 | Add `--allow-unfilled-zones`. Without it, a material zone lacking saved fill is exit 2. With it, the zone is omitted from geometry, marked `unprojected`, and produces a high-severity warning in every format. | research recommendation | 🔜 |
| NCAD-CLI-6 | Add `--native-warnings <error|report>`, default `report`. `error` upgrades any permitted native warning to exit 2. It does not downgrade errors. | CI control | 🔜 |
| NCAD-CLI-7 | Existing `--format summary|json|md`, `--html`, `--out`, threshold flags and exit codes remain. Exit 0 means no geometry or object change; exit 1 means either changed geometry or a changed object; exit 2 means input/projection failure. Warnings alone do not change 0/1 unless upgraded. | `CLI-1`; owner | 🔜 |
| NCAD-CLI-8 | `--fail-on-area`, `--fail-on-regions` and layer gates continue to apply to geometry. Add `--fail-on-objects <N>` and repeatable `--gate-object-kind`; exceeding the object count is a diff result, not an infrastructure error. | CI behavior | 🔜 |
| NCAD-CLI-9 | Native reports shall identify input family, board header version, parser/projector version, projection policy version, zone-fill policy, warning count and whether `--force` or partial projection was used. | reproducibility | 🔜 |

Examples:

```text
etchy rev-a.kicad_pcb rev-b.kicad_pcb --format summary
etchy old.kicad_pcb new.kicad_pcb --format json --objects all
etchy old.kicad_pcb new.kicad_pcb --html --fail-on-objects 0
```

No CLI option accepts a KiCad executable, Altium executable, Python interpreter,
font search path, plot profile or export directory in v0.2.0.

## 3. Parser and record accounting

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-PARSE-1 | `etchy-kicad` shall contain a bounded Rust lexer and generic S-expression tree plus version-aware decoding. It is read-only and preserves source spans and all unknown list heads/children for accounting. | research | 🔜 |
| NCAD-PARSE-2 | Supported files start with `kicad_pcb`, declare a tested date-format version, and use a supported KiCad 6–10 token profile. A newer/unknown version is `UnsupportedBoardVersion`, even if generic parsing succeeds. | trust | 🔜 |
| NCAD-PARSE-3 | Apply explicit ceilings to bytes, nesting, nodes, string/atom length, objects, layers, polygon points, curve output and coordinates before amplification. Native and wasm limits are reported in effective settings. | `CORE-7` | 🔜 |
| NCAD-PARSE-4 | Invalid syntax/UTF-8/number, non-finite value, precision beyond supported conversion, duplicate required field, unresolved required reference and overflow are typed errors with source location. | `TRUST-4` | 🔜 |
| NCAD-PARSE-5 | Every decoded record has disposition `projected`, `object-only`, `ignored-by-rule`, `warned-unprojected`, or `error`. Counts by token path and disposition are serializable. | owner; `TRUST-1` | 🔜 |
| NCAD-PARSE-6 | Unknown manufacturing-affecting records or children are errors. Harmless metadata is ignored only through a reviewed allow-list and appears in accounting. Unknown tokens are never silently dropped. | owner | 🔜 |

## 4. Shared native model

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-MODEL-1 | Add a pure `NativeBoard` model separate from the current resolved `Board`. It holds format/version, layer stack, net-number-to-name table, typed objects, warnings and accounting. | architecture | 🔜 |
| NCAD-MODEL-2 | Projection consumes `NativeBoard` and produces existing fixed-point `Board` layer geometry plus provenance. Object matching consumes the two native models. Existing boolean diff and same-board guard remain the single geometry path. | current `model.rs` | 🔜 |
| NCAD-MODEL-3 | Object coordinates use checked 1 nm integers; angles use a deterministic fixed integer representation. UUID/timestamp strings remain opaque IDs. Net names are display labels only. | `CORE-2`; owner | 🔜 |
| NCAD-MODEL-4 | Geometry, object changes, warnings and accounting shall be held in one serializable report so summary, JSON, Markdown, HTML and viewer cannot disagree. | existing report principle | 🔜 |

## 5. KiCad geometry projection

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-GEO-1 | Project track segments and arcs on their declared copper layer with declared width. Curves use deterministic bounded flattening and publish the maximum error. | owner | 🔜 |
| NCAD-GEO-2 | Project through, blind/buried and microvias on their inclusive copper-layer spans. Honor `remove_unused_layers` and `keep_end_layers` only where saved data determines used layers; otherwise fail. Record drill span separately from through drills. | owner; format docs | 🔜 |
| NCAD-GEO-3 | Project circle, rectangle, oval, trapezoid, rounded rectangle, chamfered rectangle and custom pads. Support pad rotation/offset, footprint translation/rotation, bottom-side mirroring and layer-set expansion. Unknown shape semantics fail. | owner; format docs | 🔜 |
| NCAD-GEO-4 | Custom pads support line, arc, circle, rectangle, polygon and cubic-Bezier primitives plus anchor and outline/convex-hull behavior. Generated contours are bounded. | owner; format docs | 🔜 |
| NCAD-GEO-5 | Through-hole pad annular material is emitted only on applicable copper layers. Round and oval pad drills, including offset drills, go to plated/non-plated drill layers as declared. Ambiguous plating uses `Drill(Unspecified)` and warns. | owner | 🔜 |
| NCAD-GEO-6 | Project footprint and board lines, arcs, circles, polygons, rectangles and cubic Beziers with stored stroke/fill onto copper, mask, paste, silk, fabrication and supported user layers. Apply footprint transforms exactly once. | owner | 🔜 |
| NCAD-GEO-7 | Use saved zone `filled_polygon` geometry. Do not refill and never substitute a zone outline. Missing fill is an error unless `--allow-unfilled-zones`; malformed or internally contradictory fill is always an error. Report that fill freshness cannot be proved from the board alone. | owner; format docs | 🔜 |
| NCAD-GEO-8 | Keepouts are not material geometry. They are counted as deliberately ignored for projection and remain object-change candidates labelled `keepout`. | owner | 🔜 |
| NCAD-GEO-9 | Apply pad mask/paste expansion precedence: pad, footprint, board setup. Apply absolute and ratio paste adjustments and `solder_mask_min_width`; verify threshold/topology behavior against pinned KiCad output before enabling each rule. Direct mask/paste graphics are projected as stored. | owner; format docs | 🔜 |
| NCAD-GEO-10 | Do not apply last-used plot/export settings: selected layers, mask subtraction from silk, reference/value plot toggles, output origin, output mirroring, drill marks, format precision or producer-specific Gerber options. State this in every native report. | owner | 🔜 |
| NCAD-GEO-11 | Assemble `Edge.Cuts` strokes into closed outer rings and cut-outs. Multiple disjoint boards/cut-outs are supported; open, branching, self-ambiguous or unjoinable contours fail. | owner | 🔜 |
| NCAD-GEO-12 | Emit footprint placement geometry and structured footprint objects for every placed footprint, regardless of export-exclusion flags. Position, rotation, side and reference are retained. | owner | 🔜 |

## 6. Text contract

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-TEXT-1 | Embed an exact, provenance-recorded KiCad newstroke data revision only after verifying its CC0 notice. Take glyphs from the CC0 font sources (`helpers/tools_to_build_newstroke-font/`, whose README says "Released under CC0 licence"), not from KiCad's compiled `common/newstroke_font.cpp`, which carries a GPL-2+ header; its 2019 CJK additions are MIT and need attribution if used. Render built-in stroke text on supported graphic layers. | KiCad source licence | 🔜 |
| NCAD-TEXT-2 | Stroke text supports size, line thickness, justification, rotation, mirroring, multiline layout, text boxes and footprint transforms. Glyph and total generated-point limits apply. | geometry | 🔜 |
| NCAD-TEXT-3 | A TrueType face whose bytes are not embedded in the board is not replaced with a fallback. The text is object-only and every affected output shows `text-not-rendered`, face name, object ID and layer. | owner choice | 🔜 |
| NCAD-TEXT-4 | Resolve intrinsic board/object variables such as footprint reference/value where values are present. A variable requiring `.kicad_pro`, environment, worksheet or other external state is object-only and visibly warned; it is never silently expanded or rendered literally as trusted geometry. | owner choice | 🔜 |
| NCAD-TEXT-5 | Hidden text is stored and object-diffed but not projected. This follows board visibility, not last-used plot toggles. | native semantics | 🔜 |

## 7. Layer mapping

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-LAYER-1 | Map `F.Cu`, physical inner copper stack order and `B.Cu` to top, numbered inner and bottom copper. Pair revisions by physical stack position. Changed copper-layer count/stack ambiguity fails. | owner | 🔜 |
| NCAD-LAYER-2 | Map front/back mask, paste and silk spellings accepted by the tested version profile to matching `LayerKind`; map `Edge.Cuts` to outline and front/back fabrication to documentation with side metadata. | owner | 🔜 |
| NCAD-LAYER-3 | Optional user-renamed layer names are labels only; they do not alter physical mapping or cause remove/add. Known `User.*`, comment/drawing/eco layers are separate documentation labels. | owner | 🔜 |
| NCAD-LAYER-4 | Material on an unknown internal layer is an error. Known non-material layers may be ignored only by a documented, counted rule. | trust | 🔜 |

## 8. Object change list

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-OBJ-1 | Report added, removed and modified footprints, pads, tracks, arcs, vias, zones and keepouts. Footprint modifications split into moved, rotated, flipped and other property/geometry changes and may carry several flags. | owner | 🔜 |
| NCAD-OBJ-2 | Match footprints by unique reference, corroborated by UUID/timestamp. Duplicate or absent references use stable IDs only when unambiguous. Same reference with position/angle/side change is one modified object. | owner | 🔜 |
| NCAD-OBJ-3 | Match pads inside paired footprints by pad number and stable ID, with deterministic geometry assignment for repeated numbers. Free objects use stable ID first, then kind/layer/net-label buckets and indexed geometry candidates. | owner | 🔜 |
| NCAD-OBJ-4 | Geometry fallback matching accepts only a unique best candidate within published coordinate/angle/shape tolerances. Ambiguity produces remove/add plus a warning; encounter order never decides identity. | trust | 🔜 |
| NCAD-OBJ-5 | Use hash maps and spatial indexes so stable-ID work is linear and fallback work is approximately `O(n log n)`. Candidate counts are capped per bucket and board. | big-board performance | 🔜 |
| NCAD-OBJ-6 | Each change records kind, status/flags, old/new IDs, reference/pad number where applicable, old/new layer/span, stored net-name labels, old/new position/angle/side, bounding boxes and changed fields. | owner | 🔜 |
| NCAD-OBJ-7 | Net-name change is a stored object-property change. It is displayed as a label and shall not trigger connectivity inference or topology claims. | owner non-goal | 🔜 |

## 9. Output behavior

### Terminal summary

After geometry totals, print one compact line:

```text
Objects: 2 added, 1 removed, 4 modified (2 footprints, 3 tracks, 1 via, 1 pad)
Warnings: 1 text not rendered; saved zone-fill freshness not verified
```

With `--objects changed`, follow with bounded rows containing status, kind,
identity, movement/rotation/flip, layer and net label. If rows are truncated,
print the omitted count and point to JSON/HTML. Never truncate totals.

### JSON

Native input requires `schema_version: 2`; existing fab-pack JSON v1 remains
available for non-native runs. Required additions:

```json
{
  "input": {
    "family": "kicad-pcb",
    "old_format_version": "YYYYMMDD",
    "new_format_version": "YYYYMMDD",
    "projection_policy": "kicad-native-v1"
  },
  "object_summary": {
    "added": 0,
    "removed": 0,
    "modified": 1,
    "by_kind": {"footprint": 1}
  },
  "object_changes": [{
    "kind": "footprint",
    "status": "modified",
    "flags": ["moved", "rotated"],
    "identity": {"reference": "U1", "old_id": "...", "new_id": "..."},
    "net_labels": [],
    "old": {"position_nm": [0, 0], "angle_udeg": 0, "side": "front"},
    "new": {"position_nm": [1000000, 0], "angle_udeg": 90000000, "side": "front"},
    "changed_fields": ["position", "angle"]
  }],
  "native_diagnostics": {
    "partial": false,
    "warnings": [],
    "record_accounting": []
  }
}
```

Arrays use stable ordering by kind, identity, layer and coordinates. Schema v2
defines enums, nullability and old/new snapshots fully; IDs are opaque strings.

### Markdown

Add an “Object changes” heading with totals by kind and a bounded table:
`Change | Kind | Identity | Details | Layer/span | Net`. Add “Native input
warnings” and “Projection rules” sections even when linked from a PR comment.
If the table is shortened, show the omitted count and artifact link.

### HTML

Add filterable object totals and table, old/new field expansion, click-to-frame
links, warning details, record-accounting download and native projection policy.
The file stays self-contained. Geometry remains the visual source; selecting an
object highlights its old/new bounding boxes without recoloring unchanged board
material as added/removed.

### Viewer

Add an Objects rail panel grouped by kind and status. Search matches reference,
ID and net label. Selecting a row selects its layer and highlights old/new
bounds; it does not move the camera. A separate “Frame object” action may move
the camera. Filters do not alter totals. Warning badges open the exact affected
objects. Native parsing and projection run off the UI frame where supported and
are cancellable.

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-OUT-1 | All formats report the same geometry, object totals, warnings, partial state and effective policy from one result. | trust | 🔜 |
| NCAD-OUT-2 | Human output may bound rows but never totals; machine JSON contains all requested rows or fails on a configured resource limit. | trust | 🔜 |
| NCAD-OUT-3 | Added/removed green/red remain reserved for diff geometry. Object highlights use neutral selection styling plus explicit status text/icons. | `TRUST-6` | 🔜 |

## 10. Errors and warnings

Errors stop the run and exit 2: malformed/unsupported version; unknown material
record; unsupported pad/via/graphic semantics; overflow/resource limit; missing
zone fill without opt-in; contradictory fill; ambiguous layer stack; invalid
outline; indeterminate unused-layer copper; and same-board mismatch.

Warnings permit a result but are visible in every requested output: saved fill
freshness unverifiable; opted-in unfilled zone omitted; unresolved TrueType or
external variable text object-only; ambiguous object identity reported as
remove/add; ambiguous drill plating; known ignored metadata; and intentional
native-vs-export policy difference. Each warning has a stable code, severity,
count, affected IDs/layers and plain remediation.

`native_diagnostics.partial` is true whenever any material-bearing object was
not projected. A clean report cannot have `partial: true`. The viewer keeps a
persistent warning badge; CLI summary prints warnings after totals. JSON never
represents a warning only as free text.

## 11. Trust and verification

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-TRUST-1 | For every record and manufacturing-affecting child token, disposition is projected, object-only, ignored by a documented rule, visibly warned/unprojected, or error. Unknown counts and paths are reported. | owner | 🔜 |
| NCAD-TRUST-2 | Add known-answer geometry tests, `diff(A,A)=empty`, add/remove symmetry, object-match symmetry, deterministic serialization, parser/projector fuzzing and amplification tests. | `TRUST-5` | 🔜 |
| NCAD-TRUST-3 | Compare native geometry with Gerber/Excellon/position output from pinned KiCad 6–10 oracle installations. Every mismatch has a checked-in classification; unexplained mismatch fails CI. | owner | 🔜 |
| NCAD-TRUST-4 | The corpus covers every supported shape/transform/layer/text/zone/version behavior and both expected errors and warnings. A parser success alone is not support evidence. | `TRUST-7` | 🔜 |
| NCAD-TRUST-5 | Use public MIT `Cimos/Mad_RP2040` native revisions after matching exact commits to current demo packs. Record URL, commit, licence, KiCad version and oracle command. | owner | 🔜 |
| NCAD-TRUST-6 | Run an adversarial review before stable release, focused on unknown records, transforms, layer spans, zone fills, curves, fonts, object ambiguity, integer bounds and wasm memory. | `TRUST-7` | 🔜 |

## 12. wasm

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-WASM-1 | KiCad direct parsing, projection, geometry diff and object list shall work in the web viewer without a server or external executable. | owner | 🔜 |
| NCAD-WASM-2 | Publish browser byte/node/object/point/candidate and report-size limits. Exceeding one is a typed, named error; no reduced-detail diff is silently substituted. | trust | 🔜 |
| NCAD-WASM-3 | Avoid native filesystem/font/thread assumptions. Yield between parse, project, boolean and match stages and expose progress/cancellation. | UX | 🔜 |

## 13. Altium follow-on

| ID | Requirement | Source | Status |
|---|---|---|---|
| NCAD-ALTIUM-1 | A later `etchy-altium` crate shall read `.PcbDoc` through a pure-Rust reader and populate the shared `NativeBoard`; no Python or Altium runtime. | owner | 📋 |
| NCAD-ALTIUM-2 | Evaluate `altium-format` through an exact-version API audit, full dependency/licence check, unknown-stream/record accounting, CFB resource audit, native/wasm build and corpus spike before adoption. | research | 📋 |
| NCAD-ALTIUM-3 | Altium-specific layer, transform, pad/via, saved-pour, mask/paste, outline, drill, placement and text adapters must meet the same disposition and object-list contracts. | trust | 📋 |
| NCAD-ALTIUM-4 | `altium_monkey` is an AGPL Python test oracle only. It shall not be linked, bundled, translated or used at runtime. Stable support depends on redistributable `.PcbDoc` fixtures and independent Altium-produced exports. | owner; licence | 📋 |

## 14. Explicit non-goals

1. Native schematic input or schematic-object diff.
2. Electrical connectivity/topology inference, ratsnest comparison or DRC.
3. BOM, value, sourcing, fitted-variant or assembly-intent comparison.
4. Rewriting or upgrading board files.
5. Recomputing KiCad zone fills or Altium pours in the first releases.
6. Reproducing an unknown fabrication export or applying last-used plot settings.
7. Runtime producer tools, Python, remote conversion or font downloads.
8. Mixed native/fab-pack comparisons in the first release.
9. IPC-2581 or ODB++ input.
10. Auto-registration or comparing different boards.

## 15. Owner decisions (2026-10-03)

1. Object changes ship with KiCad geometry in one 0.2.0 release.
2. A zone with no saved fill is an error by default; `--allow-unfilled-zones`
   continues with a high-severity warning and lists the zone as unprojected.
3. Undrawn TrueType or project-variable text warns only and does not change
   the exit code; CI can promote warnings to exit 2.
4. `F.Fab`/`B.Fab` geometry is present but hidden by default, like other
   documentation layers.
5. Blind, buried and micro via holes get separate native drill layers carrying
   their span.
6. Native runs use JSON schema v2; Gerber and PDF runs keep v1 unchanged.
7. Altium: spike on `altium-format` 0.1.x now, pinned to an exact version.
8. Still open: browser input and projected-point budgets for the public demo
   host. Measure first, then lock the numbers before release.
