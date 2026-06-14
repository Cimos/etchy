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

## Test board (public KiCad demo)

```bash
sudo apt install kicad                       # provides kicad-cli
# pick a dense multi-layer KiCad demo (e.g. from /usr/share/kicad/demos),
kicad-cli pcb export gerbers -o revA/ <demo>.kicad_pcb
# make revB: tweak one trace/pad in the demo, re-plot to revB/ (known delta),
kicad-cli pcb export gerbers -o revB/ <demo-edited>.kicad_pcb
```
Fallback for exact ground truth: synthesize a 16-layer dense-pour Gerber set with
a programmatically-known delta (also seeds `corpus/synthetic/`).

## What to record (append below when done)

- Per-layer + total diff time on the dense board; peak memory.
- Determinism check result (f64 API stable?).
- Any board feature that broke the reduced polygonizer.
- Go/no-go on `i_overlay`; if no-go, the `geo` BooleanOps comparison.

---

## Findings

_(to be filled in by the WSL session that runs the spike)_
