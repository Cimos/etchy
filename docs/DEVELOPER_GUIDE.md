# etchy — Developer Guide & v2 Architecture

> Architecture notes for **etchy** (the Rust successor to gerber-diff), grounded
> in [`PRODUCT_DISCOVERY.md`](PRODUCT_DISCOVERY.md) and [`ROADMAP.md`](ROADMAP.md).
> `Cargo.lock` records the dependencies used by the current workspace.

## Design tenets (what every design choice answers to)

- **No silent misses.** The engine fails loud ("couldn't resolve this layer")
  rather than emitting a wrong-but-quiet diff. Determinism (fixed-point geometry)
  + a golden corpus + property/fuzz tests enforce it.
- **One computation, shared outputs.** A single per-layer boolean diff produces
  the SVG overlay and the magnitudes used by the reports.
- **Format-agnostic core.** Gerber and Excellon front-ends both emit the *same*
  primitive type; the diff engine never knows which format it came from.
- **Small, static, permissive.** Pure-Rust where possible; no copyleft anywhere in
  the tree; everything links into one static binary + a distroless container.
- **CLI/CI is the product; the GUI is a client of the same core.**

## Workspace layout (Cargo workspace)

```
etchy/
├── crates/
│   ├── etchy-core/     # the engine: parse → resolve → polygonize → diff → measure → render. No I/O policy, no CLI.
│   ├── etchy-cli/      # binary `etchy`: arg parsing, command flow, exit codes, terminal + file output.
│   ├── etchy-gui/      # binary `etchy-gui`: egui viewer, using the core and optional PDF path.
│   ├── etchy-pdf/      # schematic-PDF pixel-diff; feature-gated pure-Rust hayro path.
│   └── etchy-kicad/    # bounded .kicad_pcb parser (0.2.0 work in progress).
├── corpus/             # golden test corpus (synthesized + real board pairs) + expected outputs.
├── fuzz/               # cargo-fuzz targets for the parsers.
└── .github/workflows/  # CI: build/test/fmt/clippy/deny/wasm; hand-built release matrix.
```

Rationale: the **PDF path is a separate, feature-gated crate** because the
`hayro` PDF stack adds a sizeable dep tree (and a higher MSRV) — keep it out of
the default geometric-diff binary so the common case stays small. The **GUI is a
separate binary**, so CLI builds do not compile egui/winit/wgpu.

## The core data model (IR)

The whole engine pivots on one format-agnostic primitive set. Both front-ends
produce it; the diff consumes it.

```rust
// etchy-core::geo
pub enum Primitive {            // emitted by BOTH Gerber and Excellon front-ends
    Line  { from: Pt, to: Pt, aperture: Aperture },   // traces, routed slots
    Arc   { from: Pt, to: Pt, center: Pt, cw: bool, aperture: Aperture },
    Flash { at: Pt, aperture: Aperture },             // pads, drill hits
    Region{ outline: Vec<Pt>, holes: Vec<Vec<Pt>> },  // G36/G37 poured copper
}

pub struct Layer {
    pub kind: LayerKind,        // TopCopper, BottomCopper, Drill, …  (rename-tolerant pairing key)
    pub polarity_resolved: PolygonSet,   // the FILLED geometry after graphics-state resolution
    pub source: PathBuf,
}
```

**Key insight from the research:** `gerber-parser` gives a typed *command AST*, not
filled polygons. **etchy-core owns the graphics-state machine** that turns that AST
into `polarity_resolved` polygons: current aperture, dark/clear polarity, region
winding, arc tessellation, and unit/coordinate-format resolution. `gerber-viewer`
(same org, MIT/Apache) is reference prior-art for this resolution step. The
Excellon front-end (hand-rolled — no crate exists) emits the same `Primitive`s
(drill hits → `Flash`, slots → `Line`/`Arc`), so the diff treats copper and drill
layers identically.

## Pipeline

```
inputs ──▶ front-end ──▶ graphics-state ──▶ polygonize ──▶ DIFF ──▶ measure ──▶ render
 (A,B)     parse AST      resolve to         (stroke/flash/   (i_overlay   (area,    (SVG / HTML /
                          Primitives          fill → PolygonSet) A−B, B−A)   regions)  JSON)
```

1. **Front-end / parse.** Gerber via `gerber-parser`; Excellon via the hand-rolled
   parser. Both → `Vec<Primitive>` per layer. **Fail loud** on unknown
   commands / ambiguous coordinate format — never silently drop.
2. **Graphics-state resolution → polygonize.** Resolve polarity/regions; stroke
   lines by aperture (Minkowski-style buffer), flash pads, fill regions, tessellate
   arcs → one `PolygonSet` per layer in a **fixed-point integer grid** (see
   "Coordinate model").
3. **Layer pairing.** Match A↔B layers by `LayerKind` (rename-tolerant). **Same-board
   guard:** if extents/datum differ beyond tolerance → hard error (no auto-align).
4. **Diff.** Per layer: `added = B − A`, `removed = A − B` via `i_overlay` boolean
   difference. `unchanged = A ∩ B` only if needed for display.
5. **Measure.** Report added and removed polygon area and region counts from the
   boolean result.
6. **Render.** Write SVG overlays and a self-contained HTML report from the same
   diff result; serialize the measured values as JSON.

### Coordinate model (a deliberate correctness decision)
The engine uses i64 nanometre coordinates. Quantizing parsed coordinates to this
grid is a correctness choice, with explicit bounds and tests. Fixed-point
also makes diffs **deterministic** (stable CI output), which the trust bar requires.

## Subsystem tech stack (in `Cargo.lock`)

| Subsystem | Current crates | Notes |
|---|---|---|
| Gerber parse | `gerber_parser`, `gerber-types` | The engine resolves the parsed commands to geometry. |
| Excellon parse | in-tree parser | No separate parser dependency. |
| Polygon booleans and triangulation | `i_overlay`, `i_triangle` | Integer geometry; `i_triangle` is declared in `etchy-core`. |
| CLI and reports | `clap`, `anyhow`, `serde_json` | HTML and SVG are written by the workspace code. |
| GUI | `eframe`, `egui` | The native and web viewers share the app code. |
| PDF raster | `hayro`, `png` | Feature-gated pure-Rust path. |
| Parallel diff and property tests | `rayon`, `proptest` | Rayon is used on native targets. |

See the crate manifests and `Cargo.lock` for the complete dependency list and
exact versions. Release archives are built by `.github/workflows/release.yml`;
`cargo-packager` builds desktop installers in that workflow.

## Output contracts

- **HTML** — a self-contained report with inline SVG overlays.
- **SVG** — per-layer overlays with resolution-independent geometry.
- **JSON** — a versioned report with per-layer and total magnitudes and gate results.

The CLI derives these outputs from one diff result.

## GUI architecture (etchy-gui, egui)

- One `eframe` app; the diff is computed by `etchy-core`, as in the CLI.
- The viewer caches triangulated geometry and transforms it for the canvas.
- The shipped view modes are Overlay, Old, New, Split and Swipe.

## Trust & testing strategy

- **Golden corpus:** generated synthetic boards with exact ground truth, checked
  under `cargo test`; KiCad fixtures live under `corpus/native/kicad/` and the
  public Mad_RP2040 demo board under `crates/etchy-gui/assets/demo/`.
- **Property tests:** `proptest` checks diff invariants and shape winding.
- **Fuzzing:** targets in `fuzz/` exercise parser and resolver boundaries.
- **CI:** `.github/workflows/ci.yml` runs builds, tests, fmt, clippy, wasm checks
  and cargo-deny. The release workflow builds archives and desktop installers.
- **PDF:** `hayro` is the rasterizer; no PDFium dependency is shipped.

## Conventions

- `etchy-core` is pure logic (no `println!`, no `process::exit`, no file-path
  policy) — returns typed errors/values; the CLI/GUI own I/O and presentation.
- Exit codes (etchy's CI contract): `0` no diff · `1` diff found · `2` error.
- `#[repr(i32)]` exit-code enum → `std::process::ExitCode`.
- Every public engine result is reproducible (fixed-point) and serializable.
