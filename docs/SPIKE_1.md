# Spike 1 — prove the polygon-diff engine (i_overlay) on a dense board

> The engine crux (ROADMAP Phase 0). Goal: de-risk **correctness + performance +
> memory** of per-layer polygon boolean diff on a real dense (8–16 layer) board,
> **before** committing to the full M1 build. Throwaway code is fine; the output
> is a go/no-go + a short findings note appended here.
>
> APIs below were verified on docs.rs (2026-06-15); `i_overlay` is precise,
> `gerber-parser` is only ~9% documented so its `GerberDoc` internals are
> **discovery work** (sub-task A).

## Success criteria (go/no-go)

1. **Correctness:** on a board pair with a *known* injected delta, `added`/`removed`
   polygons recover that delta; and `diff(A, A)` is empty for every layer.
2. **Performance:** full 16-layer dense-board diff completes in a few seconds on a
   dev laptop (target ≲ ~3 s; record the real number).
3. **Memory:** peak stays bounded (record it; flag if it blows past ~1–2 GB).

If correctness fails or perf/memory are wildly off, reconsider `geo`'s BooleanOps
or the architecture before M1.

## Crates (add to a throwaway `crates/etchy-core/examples/spike1.rs` or a temp bin)

```toml
i_overlay     = "7"
gerber-parser = "0.5"
gerber-types  = "0.7"
# timing/mem: std::time::Instant; peak RSS via /usr/bin/time -v on Linux/WSL.
```

## Verified API — the diff step (`i_overlay` 7.0)

```rust
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

// Each layer is a set of contours: Vec<Vec<[f64; 2]>> (mm or nm as f64).
let a: Vec<Vec<[f64; 2]>> = /* layer A filled contours */;
let b: Vec<Vec<[f64; 2]>> = /* layer B filled contours */;

let removed = a.overlay(&b, OverlayRule::Difference, FillRule::NonZero); // A − B
let added   = b.overlay(&a, OverlayRule::Difference, FillRule::NonZero); // B − A
// Output: Vec<Shape> where Shape = Vec<Contour>, CCW outer + CW holes.
// Area of added/removed shapes = the change magnitude (shoelace per contour).
```

`SingleFloatOverlay::overlay` takes f64 and snaps to i_overlay's internal
fixed-point. **Decision to resolve:** trust that internal snap, or pre-quantize to
our own grid (DEVELOPER_GUIDE "Coordinate model")? Test determinism: run the same
diff twice, assert byte-identical output; if stable, the f64 API is enough for M1.

## Verified API — parse (`gerber-parser` 0.5)

```rust
use std::io::BufReader;
use std::fs::File;
// gerber_parser::parse(reader: impl BufRead) -> Result<GerberDoc, GerberParserErrorWithContext>
let doc = gerber_parser::parse(BufReader::new(File::open(path)?))?;
```

**Sub-task A (discovery — undocumented):** map `GerberDoc` → our `Primitive`s →
filled `Vec<Vec<[f64;2]>>` contours. Inspect `GerberDoc`'s fields (it wraps
`gerber_types::Command`s). Reference prior-art: **`gerber-viewer`** (same MakerPnP
org, MIT/Apache) resolves graphics-state → renderable geometry — read it for the
polarity/region/aperture handling. For the spike, a **reduced polygonizer** is
fine to get end-to-end first:
- Flashes: circle aperture → n-gon polygon at position; rect → 4 points.
- Lines: buffer the segment by aperture radius → a capsule polygon (or start with
  butt-cap rectangles; refine later).
- Regions (G36/G37): use the outline directly.
- Defer aperture macros, polarity-clear, arcs to M1 — but **fail loud** on them in
  the spike (don't silently skip — that violates the trust bar).

## Test board

**Ready & exact-ground-truth (recommended — use this for the correctness check).**
A deterministic generator already exists and its output was verified against the
frozen v0.11 tool (top copper +8600px / inner_1 −8600px, every other layer empty):

```bash
python corpus/tools/gen_synth_board.py --out corpus/synthetic --copper-layers 16 --grid 80
# -> corpus/synthetic/{revA,revB}/ (16 copper + mask/silk/paste) + ground_truth.json
```
The delta is precisely known: `F_Cu` gains a 10×10 pad block (added), `In1_Cu`
loses one (removed), every other layer is byte-identical. `ground_truth.json`
lists the expected per-layer outcome — assert the engine reproduces it. Scale
`--grid` up (e.g. 120) for the perf measurement. No KiCad needed.

**Realistic (optional, for extra confidence).** A public KiCad demo plotted via
`kicad-cli` (`sudo apt install kicad`):
```bash
kicad-cli pcb export gerbers -o revA/ <demo>.kicad_pcb
# edit one trace/pad, re-plot to revB/ for a second known-delta pair
kicad-cli pcb export gerbers -o revB/ <demo-edited>.kicad_pcb
```

## What to record (append below when done)

- Per-layer + total diff time on the dense board; peak memory.
- Determinism check result (f64 API stable?).
- Any board feature that broke the reduced polygonizer.
- Go/no-go on `i_overlay`; if no-go, the `geo` BooleanOps comparison.

---

## Findings

_Run 2026-06-15, WSL2 (Linux 6.6), Rust 1.96.0 release build. Spike code:
[`crates/etchy-core/examples/spike1.rs`](../crates/etchy-core/examples/spike1.rs)
(throwaway, dev-only deps — `etchy-core` stays std-only). Reproduce:_

```bash
cargo run --release --example spike1 -p etchy-core            # committed corpus (grid 80)
/usr/bin/time -v cargo run --release --example spike1 -p etchy-core   # + peak RSS
python corpus/tools/gen_synth_board.py --out /tmp/synth120 --copper-layers 16 --grid 120
cargo run --release --example spike1 -p etchy-core -- /tmp/synth120   # denser stress
```

### Result: **GO** ✓ on `i_overlay` 7.0

All three success criteria met. No reason to fall back to `geo`'s BooleanOps.

**1. Correctness — PASS.** On the synthetic 16-copper-layer board (22 layers total),
the engine exactly reproduces `ground_truth.json`:

| layer | expected | engine: regions | engine: area mm² |
|---|---|---|---|
| `F_Cu` | +100 pads | **+100** | 19.60343 |
| `In1_Cu` | −100 pads | **−100** | 19.60343 |
| 20 others | ∅ | **0 / 0** | 0 |

- Area matches the n-gon-approximated expectation `19.60347 mm²` (100 × π·0.25²
  × the 64-gon factor 0.99839) to 5 d.p.; well inside a 1% tolerance.
- The opposite direction is empty for each changed layer (added-only / removed-only).
- `diff(A, A)` is empty for **every** layer (checked inline; build fails loud otherwise).

**2. Performance — PASS** (target ≲ ~3 s for a dense 16-layer diff).
Single-threaded, sequential over layers, on the committed **grid-80** board
(6 400 pads/layer, **140 900** contours total):

- **Production diff (added + removed, all 22 layers): ~2.11 s** ✓ (~96 ms/layer).
- Denser **grid-120** stress (14 400 pads/layer, **316 900** contours): ~6.37 s —
  that's ~230 k copper flashes, denser than essentially any real board, and still
  single-threaded. M3's per-layer parallelism (≈ wall time → slowest single layer,
  ~0.3 s) plus skipping byte-identical layers (see below) collapses this.

**3. Memory — PASS** (flag threshold ~1–2 GB). Peak RSS:
- grid-80: **~373 MB**
- grid-120: **~753 MB**

**Determinism — STABLE.** Re-running each diff in-process gives **byte-identical**
contours on every layer, on both grids. The convenience `.overlay()` uses
`i_overlay`'s default **i32** engine, which adapts f64 → a bbox-relative
fixed-point grid (sub-nm resolution over a ~175 mm board). For the spike the f64
API is sufficient.

### Open decision — resolved direction for M1

- **Coordinate snap (the doc's open question): pre-quantize to our own nm grid and
  feed integers** (DEVELOPER_GUIDE "Coordinate model"), rather than relying on
  `i_overlay`'s bbox-relative float→int adapter. In-process determinism held here,
  but the i32 adapter's grid depends on the input bounding box, so identical
  geometry in differently-sized boards could snap differently. Quantizing
  ourselves makes determinism a property of *our* code, not a side effect of the
  adapter — and matches the fixed-point IR already in `geo.rs`. Cheap to do; do it.
- **`FillRule::NonZero`** is correct for our solid (positive-orientation) contours;
  revisit only if/when polarity-clear regions arrive (M1).

### What broke the reduced polygonizer

Nothing in the synthetic corpus — it's circle-flash-only, which the reduced
polygonizer renders as 64-gons. As designed, the polygonizer **fails loud** (never
silently skips) on everything else: D01 interpolation/lines, G36 regions, aperture
macros, obround/regular-polygon apertures, drilled (hole) flashes, and any
geometry-affecting extended code (polarity-clear, mirror/rotate/scale,
step-and-repeat, aperture blocks). Those are M1 polygonizer work, validated by
Spike 2's wider corpus.

### API discoveries (sub-task A — `gerber_parser` 0.5 / `gerber-types` 0.7)

The spike spec's API sketch was close but off in two ways worth recording:

- **Crate name is `gerber_parser` (underscore)**, not `gerber-parser`.
- **`parse()` signature is** `fn parse<T: Read>(BufReader<T>) -> Result<GerberDoc,
  (GerberDoc, ParseError)>` — the error arm carries the partial doc, not the
  doc-comment's `GerberParserErrorWithContext`. Per-command parse errors live
  **inside** `GerberDoc.commands: Vec<Result<Command, _>>` even on the `Ok` path;
  use `doc.errors()` and fail loud if non-empty.
- `GerberDoc` fields used: `units: Option<Unit>`, `apertures: HashMap<i32,
  gerber_types::Aperture>`, and `doc.commands() -> Vec<&Command>`. Walk
  `Command::FunctionCode(DCode::SelectAperture | DCode::Operation(Flash/Move/
  Interpolate))`; `Aperture::Circle(Circle { diameter, hole_diameter })`;
  `CoordinateNumber: From<_> -> f64` yields **mm** directly (handles the 4.6 format
  spec for us). Coordinates are modal (omitted axis ⇒ keep current).

### Recommended next steps

1. **Spike 2** — Gerber/Excellon parse validation across a wider, realistic corpus
   (KiCad demo via `kicad-cli`); confirms the M1 polygonizer scope. Validate
   `gerber_parser` against gerbonara on the same files.
2. **M1 perf**: skip diffing byte-identical (hash/bbox-equal) layers — the common
   "few layers changed" case becomes near-instant; and parallelize across layers.
3. **M1**: implement the full polygonizer (lines→capsules, regions, macros,
   polarity) behind the same fail-loud guarantee proven here.
