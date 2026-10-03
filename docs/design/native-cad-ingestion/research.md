# Native CAD ingestion v2: changed and new research

Research date: 2026-10-03. This is a delta from the v1 research. It replaces
the KiCad export-bridge recommendation with direct parsing and records the
additional evidence needed for that decision. The full product contract is in
`spec.md`.

## 1. Decision change

The owner rejected a runtime `kicad-cli` bridge. etchy will read
`.kicad_pcb` bytes itself, build a native board/object model, project that model
to the existing fixed-point layer geometry, and diff both geometry and objects.
KiCad, Altium Designer, Python, `kicad-cli`, and `altium-monkey` are not runtime
dependencies. Producer tools may be test oracles in development and CI.

This deliberately does not reproduce a fabrication pack. A board file records
design geometry and some last-used plotting settings, but it does not prove
which settings produced a pack sent to a fabricator. Native projection therefore
means “what is stored in these two board files under etchy's documented rules.”
Differences from Gerber export are expected and must be classified, not hidden.

The existing same-board rule remains: absolute coordinates, no registration,
and a loud failure when physical extents differ beyond 1 mm or 2% of board span.

## 2. S-expression parser recommendation

### Recommendation: an etchy-owned small parser

Implement a bounded lexer and generic S-expression tree in a new
`etchy-kicad` crate, followed by version-aware decoding into etchy-owned PCB
types. Preserve every list head, atom, string, source span, and unknown child.
Do not build a writer. Do not copy KiCad implementation code.

Reasons:

1. The syntax is small; the hard part is audited PCB meaning. KiCad's format
   uses lists, atoms, quoted strings and numbers. Board values are millimetres,
   and PCB resolution is 1 nm. The official format documentation covers board
   files from KiCad 6 onward and uses a date-valued format version.
2. “No silent misses” requires a complete record inventory. A generic tree lets
   etchy count every top-level and nested record before typed decoding, retain
   source locations, and reject or warn on unknown tokens by policy. A typed
   dependency that ignores fields internally cannot provide that proof.
3. etchy needs only board reading. A round-trip parser/writer adds API surface,
   dependencies, wasm size, and update risk without solving polygonization.
4. Version support must be driven by fixtures from KiCad 6, 7, 8, 9 and 10,
   not by a dependency's claim that a file parsed.

The parser must have byte, nesting, atom/string length, node-count, coordinate,
object-count and generated-point limits. Invalid UTF-8, malformed escapes,
non-finite numbers, excessive precision, duplicate required fields, and integer
overflow are typed errors.

### Candidate findings

| Candidate | Evidence checked | Decision |
|---|---|---|
| `kiutils-rs` 0.2 | MIT. Its repository describes a lossless S-expression tree plus typed document APIs, but names KiCad 10 as primary and 9 as secondary compatibility targets. It is aimed at read/edit/write, not fabrication geometry. | Do not adopt for v2. Re-evaluate later if its 6–10 fixture coverage and unknown-field contract become a fit. |
| `kicad_parse_gen` 7.0.2 | MIT/Apache-2.0, last published in 2018, before the supported KiCad 6 format generation. | Reject for current boards. |
| `kiparse` / KiParse | MIT and useful for extraction experiments, but its published scope does not establish full pad, zone, font, layer and version semantics. | Reject as the production parser. |
| etchy-owned parser | No new licence or runtime dependency; exact limits and unknown-token accounting can be designed around etchy's trust contract. | Adopt. |

Sources: [KiCad board format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/),
[KiCad shared S-expression format](https://dev-docs.kicad.org/en/file-formats/sexpr-intro/),
[`kiutils-rs`](https://github.com/Milind220/kiutils-rs), and
[`kicad_parse_gen` manifest](https://docs.rs/crate/kicad_parse_gen/latest/source/Cargo.toml.orig).

## 3. Native board model and projection

The current `etchy-core::Board` contains only resolved layer polygons. Native
ingestion also needs an immutable `NativeBoard` holding the source format and
version, declared layer table, nets, objects, warnings, and record accounting.
Projection produces the existing `Board` plus a placement layer. Object diff
operates on the two `NativeBoard` values. This keeps existing Gerber behavior
unchanged and lets every output derive from one paired result.

### Copper objects

- Straight tracks become stroked paths at their declared width.
- Track arcs use their stored geometry and width, with deterministic adaptive
  flattening bounded by a documented maximum radial error and point cap.
- A through via contributes its annular disk to every enabled copper layer. A
  blind/buried or microvia contributes only to copper layers in its declared
  inclusive span. With `remove_unused_layers`, retain the two end layers and
  only intermediate layers on which saved connectivity is explicitly
  represented; if the file does not provide enough information to determine
  that set, fail rather than invent copper. `keep_end_layers` is honored.
- Pads support circle, rectangle, oval, trapezoid, rounded rectangle,
  chamfered rectangle, and custom primitives. The order is pad-local shape,
  pad offset/rotation, footprint rotation, footprint translation, then bottom
  footprint mirroring. Wildcard layer sets are expanded against the declared
  layer table. Through-hole annular geometry follows the pad's copper-layer set
  and unused-layer flags.
- Custom-pad primitives include line, arc, circle, rectangle, polygon and cubic
  Bezier forms. Their anchor and outline/convex-hull rule are applied. Unknown
  primitive or boolean behavior is an error.

The official format documents pad shapes, custom primitives, transforms, via
spans, unused-layer flags, net names, and 1 nm board resolution in the two
KiCad format references above. Some unused-layer semantics remain under-
documented; they need producer-oracle fixtures before support is claimed.

### Graphics, outline, drills, placement

- Footprint and board graphics support lines, arcs, circles, rectangles,
  polygons and cubic Beziers with stroke, fill and transforms. Items on copper,
  mask, paste, silk and fabrication layers are projected to those layers.
- `Edge.Cuts` strokes are assembled into closed rings using 1 nm endpoints
  after curve flattening. Multiple disjoint outlines and inner cut-outs are
  allowed. Open, branching or ambiguous contours are errors, not guessed joins.
- Round pad/via holes become drill circles. Oval drills become true slots: a
  stroked centreline with round ends. PTH and NPTH are separated when pad type
  establishes plating; ambiguous plating is `Drill(Unspecified)` with a visible
  warning. Blind/buried/microvia holes are recorded with their layer span and
  projected into a separate native drill identity so they are not confused with
  through drilling.
- Each footprint reference, position, orientation and side produces placement
  geometry and a structured footprint object. Excluded-from-position flags are
  not applied: they are export settings, while native comparison reports board
  objects actually present.
- Keepout zones constrain editing/filling but are not board material. They are
  inventory-accounted and deliberately ignored as geometry; changing one is an
  object change and is visibly labelled `keepout`.

### Zones

Use saved `filled_polygon` data, including its layer assignment and holes. Never
use a zone outline as copper and do not implement refill in the first release.
The official format states that saved fill polygons do not exist when a zone
has not been filled.

Recommended policy:

- A non-keepout copper zone with no saved fill for an affected enabled layer is
  a typed error by default. `--allow-unfilled-zones` permits the run, omits that
  zone from geometry, emits a high-severity warning in every output, and lists
  the zone as `unprojected`; this mode is unsuitable for a clean CI gate.
- The file has no reliable freshness proof. Always report that saved zone fills
  were used and their freshness was not independently verified. Cheap internal
  contradictions—fill on an undeclared layer, malformed rings, fill wholly
  outside its zone bounds, or a referenced layer missing from a multilayer
  zone—are errors. Do not claim that passing these checks proves freshness.
- `kicad-cli --check-zones` is an oracle test, not runtime repair. A discrepancy
  between saved projection and freshly exported Gerber is recorded as stale-fill
  corpus evidence and must produce the documented warning/error behavior.

### Mask and paste

Native projection applies design-level expansion in this precedence order:
pad override, footprint override, then board setup. For paste, combine the
selected absolute margin and ratio as KiCad defines them; reject a result whose
inset collapses or changes topology unless an oracle fixture establishes the
producer behavior. Direct graphics on mask or paste are projected as drawn.
Pad layer membership and footprint flipping select front/back output.

Apply `solder_mask_min_width` by merging neighboring mask apertures separated
by less than the configured web width, after individual expansions. This must
ship only with exact oracle cases around the threshold.

Do not apply last-used plot options such as subtracting mask from silk,
reference/value visibility, mirroring, auxiliary origin, drill marks or selected
output layers. Do not infer fabrication-house compensation. The report states
that native design rules, not plot/export settings, were used.

### Layer mapping and renamed layers

Map by the internal layer identity/ordinal, never by the optional user display
name. User names are labels only.

| KiCad layer | etchy kind |
|---|---|
| `F.Cu` | `TopCopper` |
| enabled inner copper in stack order (`In1.Cu` …) | `InnerCopper(1…)` |
| `B.Cu` | `BottomCopper` |
| `F.Mask`, `B.Mask` | `TopMask`, `BottomMask` |
| `F.Paste`, `B.Paste` | `TopPaste`, `BottomPaste` |
| `F.SilkS` or accepted version alias `F.Silkscreen`; bottom equivalents | `TopSilk`, `BottomSilk` |
| `Edge.Cuts` | `Outline` |
| `F.Fab`, `B.Fab` | `Documentation`, with side in native metadata |
| `User.*`, comments, drawings and eco layers | `Documentation`, kept as separately labelled layers where needed |

An unknown internal layer identity containing material is an error. A known
non-material layer is documented and counted as ignored. Two revisions pair
copper by physical stack position, so a user rename does not create a layer
remove/add. A changed copper count or irreconcilable stack is an error.

## 4. Text and fonts

KiCad's built-in newstroke data may be embedded. KiCad's source history and
font README state that the author permitted relicensing it under CC0. Checked 2026-10-03: the 4.0.1 font `README.txt` says "Released under CC0 licence", while KiCad's current compiled `common/newstroke_font.cpp` carries a GPL-2+ header plus an MIT notice for 2019 CJK glyphs. Generate etchy's data from the CC0 sources only. Preserve
the attribution/provenance file even though CC0 does not require attribution:
[KiCad newstroke source history](https://gitlab.com/kicad/code/develop/-/tree/4.0.1/helpers/tools_to_build_newstroke-font?ref_type=tags).

Implement stroke text for board and footprint text/text boxes on silk, copper,
fabrication and other supported graphic layers, including size, thickness,
justification, rotation, mirroring, multiline layout and footprint transforms.
Pin the exact CC0 data revision and test glyph outlines against KiCad output.

KiCad 7 added a TrueType `face` field, but a board normally names a font rather
than embedding its bytes. A bundled fallback would silently change geometry.
Therefore unresolved TrueType text is not projected: it is retained and diffed
as an object, and every affected layer receives a visible `text-not-rendered`
warning. A future `--font-dir` is not in the first release because it weakens
reproducibility and browser parity.

Resolve variables whose values are in the board itself or intrinsic to the
object, including reference/value fields. A `${NAME}` requiring a project file,
environment or worksheet remains literal in object data, is not projected, and
gets the same visible warning. No `.kicad_pro` input is required in v0.2.0.

## 5. Version spread: KiCad 6–10

Support is by tested format-version range, not merely by major label. Important
known transitions include:

- KiCad 6 is the baseline documented board generation; older files had quoting
  differences and are outside scope.
- KiCad 7 introduced stroke blocks in places that previously used `width`, the
  optional TrueType font face, annotation boxes in custom pads, and broad UUID
  naming changes from older `tstamp` forms.
- KiCad 8/9/10 add tokens and enum values over time. The current documentation
  describes the latest format, not a complete per-release changelog.
- Layer spelling aliases and token renames must be fixture-derived. Accepting an
  unknown date or token because the surrounding list is familiar is forbidden.

Maintain a table in code and docs: producer major, exact header date(s), accepted
aliases, newly understood tokens, fixture hashes, and oracle KiCad version.
Newer files may parse generically but fail with `UnsupportedBoardVersion` until
their manufacturing-affecting records pass review. Harmless metadata may be
warned and counted under an explicit allow-list.

## 6. Object matching and performance

Object changes cover footprints, tracks/arcs, vias, zones, pads and keepouts.
Net names are labels; they do not drive geometry or matching.

- Footprints: match unique reference designator first. Use UUID/timestamp as
  corroboration and for duplicate/missing-reference cases. Same reference with
  changed position, angle or side becomes moved, rotated or flipped, not a
  remove/add. Report combined changes in one record.
- Pads: match within a paired footprint by pad number plus UUID when stable;
  repeated pad numbers use UUID then a deterministic local geometry assignment.
- Tracks/arcs, vias, zones and free pads: exact UUID match when present on both
  sides. Because copy/save and older files can change IDs, unmatched objects are
  assigned within `(kind, layer/span, net-label)` buckets by a spatial index and
  normalized geometry signature. Only unambiguous best matches within documented
  tolerances become modifications; otherwise report remove/add.
- A track move is a matched centreline whose endpoints/arc/width/layers changed.
  Footprint rotation is normalized modulo 360; flip is a side change. Coordinate
  comparisons use etchy's 1 nm integers and angles use fixed integer units.
- Build hash maps for stable IDs and references, then R-trees for unmatched
  geometry. Avoid all-pairs scans. Expected complexity is linear for stable IDs
  plus approximately `O(n log n)` for spatial candidates. Apply per-bucket and
  total candidate caps; ambiguity is reported, never resolved by encounter order.

The object list is explicitly not a connectivity or BOM feature. The precise
scope line is: **“etchy reports changes to stored PCB objects and displays their
stored net names as labels; it does not infer electrical connectivity, compare
net topology, validate routing, or compare a bill of materials, values, variants,
or fitted-component intent.”**

## 7. Trust and oracle verification

For native input, “no silent misses” means every parsed record kind and nested
manufacturing-affecting token is one of:

1. projected to named geometry and, where applicable, an object;
2. deliberately ignored by a documented rule and counted in the report; or
3. rejected with a typed error, or retained with a visible warning and explicit
   `unprojected` status where the spec permits a partial run.

Unknown list heads and unknown children are counted by source path and token.
Unknown manufacturing-affecting data is an error. The summary, JSON, Markdown,
HTML and viewer expose warning/error counts. There is no quiet fallback.

Verification compares native projection with Gerber, Excellon and position
outputs from a pinned `kicad-cli pcb export gerbers` oracle. CI may install a
pinned KiCad; shipped binaries do not. Compare per-layer symmetric-difference
area and regions, drill shapes, outline and placements. Every non-zero mismatch
needs a checked-in classification: intentional native-policy difference,
unavailable font/variable, stale fill, unsupported plot option, or defect.
“Close enough” without a reason is not a passing test.

The public [Cimos/Mad_RP2040 repository](https://github.com/Cimos/Mad_RP2040)
does contain `Mad_RP2040.kicad_pcb`, archives Rev A under `resources/rev_a/`,
and is MIT licensed. It is therefore suitable for provenance-reviewed old/new
native fixtures corresponding to the demo family. Exact commits must be matched
to the existing `crates/etchy-gui/assets/demo/old` and `new` packs before any
claim that they are the same revisions.

## 8. wasm implications

The parser and projector are pure Rust and must compile for
`wasm32-unknown-unknown`; no filesystem, subprocess, threads, or native font
lookup is required. Browser limits need lower configurable ceilings for input
bytes, nodes, objects, polygon points and object-match candidates. Parsing and
projection should yield between phases in the viewer to keep the UI responsive.
Large boards that exceed the browser budget fail with a specific limit message;
the desktop limit is not silently substituted.

## 9. Altium delta

Altium follows only after the shared native board/object model is stable. A new
`etchy-altium` adapter will use a pure-Rust `.PcbDoc` reader, with
`altium-format` remaining the leading candidate subject to an exact version,
dependency-licence audit, unknown-record audit, CFB resource-limit review and
corpus proof. It feeds the same layer projection and object-diff interfaces but
has format-specific transforms, layer stack, pours, masks and text handling.

The local `altium_monkey` checkout is AGPL Python. It cannot be linked, vendored,
translated into etchy, or shipped with etchy. It may be used only as a separately
run black-box test oracle on fixtures etchy is licensed to use. Stable Altium
support also depends on independently generated Altium Gerber/drill/position
outputs, redistributable `.PcbDoc` fixtures across producer versions, and a pure-
Rust reader that reports every record rather than dropping unknown streams.

