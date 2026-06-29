# M1 engine-core design — the first vertical slice

> Synthesized from a 3-proposal judge-panel design workflow (trust-first vs
> extensibility-first vs pragmatic-mvp). Spine = trust-first; grafts from the
> other two noted inline. This is the implementable spec for the first Milestone-1
> increment: promote the spike-proven pipeline into the real `etchy-core` library
> + a working CLI, validated against the Phase-0 golden corpus.

## Goal & shape

A **complete thin vertical slice**: `etchy <oldDir> <newDir>` → parse Gerber →
polygonize → per-layer boolean diff → measure → JSON/terminal report, exit 0/1/2.
**Flash-only geometry** (circle + rect apertures) — the only geometry with an
exact, independently-verified oracle today (the synthetic corpus + 64-gon area
math). Everything else **fails loud** (typed error), never silently dropped.

## Coordinate model (the deliberate correctness decision)

Quantize Gerber f64 mm → **i64 nanometres** once, at the front-end boundary
(`GRID_NM = 1`, `NM_PER_MM = 1_000_000`), via a **guarded** `quantize_mm(mm) ->
Result<i64, GeoError>` that rejects non-finite / out-of-range *before* the
`as i64` cast (a saturating cast is a silent-miss hazard). Everything downstream
is integer. Diff via i_overlay's **integer** engine (not the bbox-scaling float
adapter the spike used), so determinism is a property of our code, independent of
board size:

```rust
Overlay::<i64>::with_contours(&subj, &clip).overlay(OverlayRule::Difference, FillRule::NonZero)
// removed = A−B (subj=A, clip=B); added = B−A (subj=B, clip=A)
```

**Overflow:** i64 nm spans ±9.2e9 m (coords trivially safe). Area via integer
shoelace overflows i64 on the cross-product sum → accumulate the doubled signed
area in **i128** (exact), halve at the end. Report `area_nm2: i128` (exact,
canonical) + `area_mm2: f64` (derived, lossy, display/threshold convenience).

## Module layout (`crates/etchy-core/src/`)

| file | responsibility |
|---|---|
| `error.rs` | `EngineError` (thiserror) + `GeoError`. No I/O/path variants. |
| `geo.rs` | Kept `Pt`/`Aperture`/`Primitive` (now nm); `quantize_mm`; `Contour=Vec<Pt>`, `Shape=Vec<Contour>`; **owned `PolygonSet`** (insulates the API from i_overlay types — graft P2) with `area_nm2()/area_mm2()/region_count()/is_empty()/bbox_nm()`. |
| `gerber.rs` | `parse_gerber(&[u8]) -> Result<Vec<Primitive>>`: gerber-parser + fail-loud on `doc.errors()` and unsupported AST; maps apertures→nm. Bytes in, **no path**. |
| `polygonize.rs` | `polygonize(&[Primitive]) -> Result<PolygonSet>`: Flash→contour (circle 64-gon / rect 4-corner); `Line`/`Arc`/`Region` arms return `Unsupported` (the documented growth seam). |
| `diff.rs` | `diff_layer(&PolygonSet,&PolygonSet) -> LayerDiff{added,removed: PolygonSet}` via the i_overlay int engine; `LayerChange{added/removed_area_nm2:i128, *_region_count:u32}`. |
| `model.rs` | Kept `LayerKind`; `Layer{kind, label:String, geometry:PolygonSet}` (**no `PathBuf`** — graft P1 purity; `label` is an opaque diagnostic string — graft P3); `Board`; pair-by-kind; whole-board-union same-board guard. |
| `report.rs` | `#[derive(Serialize)] DiffReport` (kept `SCHEMA_VERSION`), `to_json()`. The one struct all outputs derive from. |
| `lib.rs` | `compare(&Board,&Board) -> Result<DiffReport>` (guard → pair → diff → measure → build); re-exports; `version()`. |

The `Primitive` IR boundary (kept from the scaffold) is the format-agnostic seam
the Excellon front-end + the lines/regions polygonizer plug into next — fixing
the IR-collapse weakness the judge flagged in the trust-first proposal.

## Purity (CLAUDE.md: no `println!`, no `process::exit`, no path policy)

Core takes **bytes + a pre-classified `Board`**, returns `Result<_, EngineError>`
and `String` JSON. The CLI owns *all* I/O: dir walk, file read, `classify(filename)
-> LayerKind` (naming policy), rendering, and the `Exit{0,1,2}` mapping (anyhow at
the boundary). No `PathBuf`, `std::fs`, `println!`, or `ExitCode` anywhere in core.

## Polygonizer scope

**IN:** Gerber `Flash` (D03) of circle (→64-gon) + rect (→4 corners) apertures;
modal position (`Move`/D02); aperture select; benign no-geometry codes.
**FAIL LOUD** (typed `EngineError::Unsupported{feature}`, never silent): D01
draw/interpolate, G36/G37 regions, arcs, aperture macros, obround/polygon
apertures, drilled (hole) flashes, polarity-clear, mirror/rotate/scale,
step-and-repeat, blocks, the gEDA single-line style, any geometry-affecting
`doc.errors()`. Lines + regions + arcs are the next increment (with their own
injected-delta corpus), behind this same fail-loud contract.

## JSON report (schema_version = 1)

`schema_version`, `tool_version`, `any_changes`, `totals{added/removed_area_mm2,
added/removed_regions, layers_changed, layers_total}`, `layers[]{kind (kebab),
inner_index, label_old, label_new, status (unchanged|changed|added-layer|
removed-layer), added/removed_area_mm2, added/removed_area_nm2 (i128 as **string**
— JSON has no i128), added/removed_regions}`, `warnings[]` (forward-compat;
metadata-only parse issues land here in a later increment — empty this slice).

## Same-board guard (the corpus-tripping risk all three flagged)

Compare each board's **whole-board union bbox** (not per-layer — F_Cu's added
block legitimately grows its own bbox) with a **generous** tolerance
(`max(2 mm, 10% of span)`, matching `model.rs`). The synthetic revs share identical
union extents, so it passes; grossly different boards fail loud ("not the same
board; etchy will not auto-align"). A coarse plausibility check, not registration:
it catches grossly mis-sized boards (wrong files), but extent alone cannot tell two
*same-size* different boards apart — tightening the percentage wouldn't fix that and
would only false-positive on legit revisions, so the bound stays loose (#92).

## Test retargeting

`tests/support/mod.rs` shrinks to its durable core: KEEP the synthetic generator +
ground-truth constants (`gen_synth_board`, `PAD_DIA_MM`, `BLOCK`, `pad_area_mm2`,
`ngon_factor`, `CIRCLE_SEGMENTS=64`); DELETE the inline pipeline; add thin
adapters (`polygonize_gerber`, `diff_layer`, `pad_layer` built via the library) so
`golden_corpus.rs` and `properties.rs` pass **unchanged** — now exercising the
shipping library. Move i_overlay/gerber-parser/serde to real `[dependencies]`
(examples still compile); proptest stays a dev-dep. Add `tests/cli.rs` pinning the
0/1/2 exit contract + `--json` parses with `schema_version == 1`.

## Out of scope (next increments)

Lines/regions/arcs/macros/polarity polygonization; the SPIKE_2 `*`-tokenization
normalization shim; Excellon front-end; SVG/HTML/heatmap render; egui GUI;
schemars JSON-Schema emission (M2). Each lands behind the fail-loud guarantee
proven here.
