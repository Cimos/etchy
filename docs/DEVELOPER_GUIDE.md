# etchy — Developer Guide & v2 Architecture

> Planning artifact for **etchy** (the Rust successor to gerber-diff). Seeds the
> fresh `Cimos/etchy` repo. Grounded in [`PRODUCT_DISCOVERY.md`](PRODUCT_DISCOVERY.md)
> and [`ROADMAP.md`](ROADMAP.md). Every crate below was verified to exist, be
> actively maintained, and be permissively licensed (a 2026-06-15 web pass);
> versions are a snapshot — pin and update deliberately.

## Design tenets (what every design choice answers to)

- **No silent misses.** The engine fails loud ("couldn't resolve this layer")
  rather than emitting a wrong-but-quiet diff. Determinism (fixed-point geometry)
  + a golden corpus + property/fuzz tests enforce it.
- **One computation, three views.** A single per-layer boolean diff produces the
  SVG overlay, the heatmap regions, and the magnitudes. No parallel pipelines to
  drift apart.
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
│   ├── etchy-cli/      # binary `etchy`: arg parsing, orchestration, exit codes, terminal + file output.
│   ├── etchy-gui/      # binary `etchy-gui` (or `etchy gui`): egui viewer; depends on etchy-core only.
│   └── etchy-pdf/      # schematic-PDF pixel-diff; feature-gated (pulls the heavy pdfium dep) so the core stays tiny.
├── corpus/             # golden test corpus (synthesized + real board pairs) + expected outputs.
├── fuzz/               # cargo-fuzz targets for the parsers.
└── .github/workflows/  # CI: fmt/clippy/nextest/fuzz-smoke + cargo-dist release.
```

Rationale: the **PDF path is a separate, feature-gated crate** because `pdfium`
adds a multi-MB C++ dependency — keep it out of the default geometric-diff binary
so the common case stays small. The **GUI is a separate binary** depending only on
`etchy-core`, so headless/CI builds never compile egui/winit/wgpu.

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
 (A,B)     parse AST      resolve to         (stroke/flash/   (i_overlay   (area,    (SVG / PNG /
                          Primitives          fill → PolygonSet) A−B, B−A)   regions)  HTML / JSON)
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
5. **Measure.** `changed_area_mm2 = area(added)+area(removed)`; cluster difference
   polygons into ranked **regions** (heatmap) via `rstar` proximity + `petgraph`
   connected-components, ranked by `geo` area/centroid.
6. **Render.** SVG (canonical, resolution-independent) for the overlay; PNG derived
   from the SVG (single source of truth) or drawn directly; assemble HTML + JSON.

### Coordinate model (a deliberate correctness decision)
`i_overlay` offers fixed-point integer engines (i32/i64) — **use i64 on a defined
grid (e.g. 1 nm)**. Snapping Gerber float coordinates to this grid is a
*correctness* choice (precision vs i64 overflow headroom), so it is explicit,
documented, and covered by tests — not an implicit float artifact. Fixed-point
also makes diffs **deterministic** (stable CI output), which the trust bar requires.

## Subsystem tech stack (verified)

| Subsystem | Pick | Ver | License | Notes |
|---|---|---|---|---|
| Gerber parse | `gerber-parser` + `gerber-types` | 0.5 / 0.7 | MIT/Apache | MakerPnP; full 2024.05 spec; AST not polygons (we resolve). Pin (pre-1.0). |
| Excellon parse | **hand-rolled** | — | — | No crate exists. Model on gerbonara: M48 header, zero-suppression + coord-format detection (never guess), tool table, hits→Flash, slots→Line/Arc. |
| Polygon booleans | `i_overlay` (+`i_float`) | 7.0 | MIT/Apache | Pure-Rust; handles self-intersection/holes/degenerate; fixed-point; Rayon; ~5–80× Clipper2 on stress (vendor bench — re-verify on real pours). Alt: `geo` BooleanOps (more mature, slower). |
| SVG overlay | `svg` crate or hand-written XML | 0.18 | MIT/Apache | SVG is resolution-independent → "crisp+zoomable" for free. Hand-writing gives full control over layer toggling/styling. |
| Raster preview | `tiny-skia` (+`png`) or `resvg`/`usvg` | 0.12 / 0.47 | BSD-3 / MIT-Apache | `resvg` rasterizes the *same* SVG (single source of truth); `tiny-skia` draws directly (faster). Both pure-Rust, no system libs. |
| CLI | `clap` (derive) + `anstream`/`owo-colors` + `indicatif` | 4.x | MIT/Apache | Progress → **stderr**, suppressed when `--format json` / non-TTY. Hand-roll exit codes (avoid abandoned `exitcode`). |
| Errors | `anyhow` (CLI) + `thiserror` (core) | 1.x / 2.x | MIT/Apache | Typed errors in core; context-wrapped at the binary boundary → exit-code enum. |
| GUI | `eframe`/`egui` (+`egui_extras`, `egui-wgpu`) | 0.34 | MIT/Apache | Immediate-mode `Painter` ideal for the canvas; `load_texture` for GPU; wgpu paint-callback for huge boards; `egui_extras::Table` for the layer list. Pre-1.0 — pin. Tile/downsample layers over the GPU max texture size. (iced rejected: MIT-only, heavier for a one-canvas app.) |
| PDF raster | `pdfium-render` (`static` feature), `etchy-pdf` crate | 0.9 | MIT/Apache (engine Apache/BSD) | Chrome-grade fidelity (trust). Static-link `libpdfium.a` per target → no runtime file. Ship PDFium NOTICE. **Watch `hayro`** (pure-Rust, dual, experimental) behind `--pure-rust-pdf` to one day drop the C++ dep. **Reject** `mupdf` (AGPL), `pdfium` crate (GPL), `pdf2image` (Poppler GPL). |
| HTML report | `askama` (or `maud`) | 0.16 | MIT/Apache | Compile-time templates baked into the binary (self-contained). **Mark base64/SVG `\|safe`** or auto-escaping corrupts the payload. |
| JSON | `serde_json` + `schemars` | 1.x | MIT/Apache | Top-level `schema_version`; emit a committed JSON Schema for CI validation by consumers. |
| Clustering (heatmap) | `rstar` + `petgraph` (+`geo`) | 0.13 / 0.8 | MIT/Apache | R-tree proximity → connected-components/UnionFind; rank regions by `geo` area/centroid. Avoid the `linfa` ML stack (weight). |

## Output contracts

- **HTML** — one self-contained file (compile-time `askama` template; base64 images
  + inline SVG). The shareable/PR-attachable artifact.
- **SVG** — per-layer overlay files, canonical & resolution-independent.
- **JSON** — versioned (`schema_version`), `schemars`-validated; the integration
  backbone. Stable fields: per-layer `{kind, added_area_mm2, removed_area_mm2,
  region_count, regions:[{bbox,area,centroid}]}`, board totals, tool version,
  effective settings. **CI thresholds read this.**

All three derive from one in-memory `DiffReport` struct (`#[derive(Serialize)]`),
so HTML/SVG/JSON cannot disagree.

## GUI architecture (etchy-gui, egui)

- One `eframe` app; the diff is computed by `etchy-core` (same code as the CLI),
  results held in memory.
- Decoded layer rasters → `Context::load_texture` (GPU textures, cheap-clone
  handles). Pan/zoom = a pixels-per-point + offset transform on the `Painter`.
- For a layer that exceeds the GPU max texture dimension (commonly 8–16k px),
  **tile or render via the `egui-wgpu` paint callback** — don't push giant
  `Shape::Path` lists every frame.
- Carries forward v0.11's UX: layer tree (changed-first), modes (overlay / before /
  after / split / swipe / onion) + heatmap, keyboard-driven, with a focus guard so
  shortcuts don't fire while typing.

## Trust & testing strategy (the top-tier requirement)

- **Golden corpus** (`corpus/`): synthesized board pairs with *precisely injected*
  deltas (exact ground truth) + public KiCad demo boards + Simon's non-confidential
  boards. Each has committed expected outputs.
- **Snapshot tests** (`insta`, or `goldenfile`): the SVG/heatmap/JSON outputs are
  snapshotted; CI fails on drift; `cargo insta review` to accept intended changes.
- **Property tests** (`proptest`): invariants — `diff(A,A)=∅`, add/remove symmetry
  (`removed(A,B)==added(B,A)`), idempotence, area conservation bounds.
- **Fuzzing** (`cargo-fuzz` + `arbitrary`): the Gerber and (especially) hand-rolled
  Excellon parsers **must not panic/OOM/hang** on malformed input — non-negotiable
  given "no silent misses". Nightly CI target.
- **Runner:** `cargo-nextest` (parallel, per-test timeouts for the dense-board perf
  tests). Validation parity check against gerbonara on a shared file set during the
  parser bring-up.

## Build, release & distribution

- **Cross-compile:** `cargo-zigbuild` (Zig linker — robust musl + macОS-from-Linux,
  no Docker). Targets incl. `x86_64-` / `aarch64-unknown-linux-musl` → fully static.
- **Release matrix:** `cargo-dist` (one tag → per-OS static binaries + checksums +
  GitHub Release). **Risk:** cargo-dist governance has been turbulent (axodotdev
  wound down; astral fork archived Dec 2025; 0.32 currently alive) — pin it and keep
  `taiki-e/upload-rust-binary-action` as a drop-in fallback.
- **Container:** the musl static binary drops into a `scratch`/distroless image
  (~10–20 MB) — no glibc, no shared libs. Ideal for CI/self-hosted.
- **Allocator:** benchmark musl's allocator on dense boards; swap in `mimalloc` if
  allocation cost shows up.
- **Licensing:** dual `MIT OR Apache-2.0`; bundle third-party NOTICEs (notably
  PDFium's). CI `cargo-deny` to block any copyleft creeping into the dep tree.

## Conventions

- `etchy-core` is pure logic (no `println!`, no `process::exit`, no file-path
  policy) — returns typed errors/values; the CLI/GUI own I/O and presentation.
- Exit codes (etchy's CI contract): `0` no diff · `1` diff found · `2` error.
- `#[repr(i32)]` exit-code enum → `std::process::ExitCode`.
- Every public engine result is reproducible (fixed-point) and serializable.

## Open decisions to settle in Phase 0

1. **Fixed-point grid** (1 nm vs 10 nm) + i64 overflow headroom on largest boards.
2. **`i_overlay` vs `geo` BooleanOps** — bench both on a real dense pour before
   locking (speed vs maturity; `i_overlay`'s vendor benchmarks need independent
   confirmation).
3. **SVG-canonical + `resvg`-derived PNG** (single source of truth) vs direct
   `tiny-skia` raster (speed) — pick after measuring.
4. **`askama` vs `maud`** for the report (template files vs templates-as-Rust).
5. **`hayro` watch** — revisit pure-Rust PDF when its fidelity is proven, to drop
   the pdfium C++ dependency.
