# Golden test corpus

The trust backbone (see DEVELOPER_GUIDE "Trust & testing"). Built in Phase 0,
before features, so every later change is validated against ground truth.

Planned sources:
- **`synthetic/`** — board pairs with *precisely injected* deltas (exact ground
  truth: known added/removed copper area, known moved features).
- **`kicad-demos/`** — public KiCad demo boards turned into revision pairs (realism).
- **`real/`** — the owner's non-confidential boards (actual use cases).

Each entry carries committed **expected outputs** (JSON magnitudes + SVG/heatmap
snapshots via `insta`). Property tests (`diff(A,A)=∅`, add/remove symmetry,
idempotence) and parser fuzzing (`cargo-fuzz`) complement the corpus.

## Harness status (Phase 0 — built)

The harness lives in `crates/etchy-core/tests/` and runs under `cargo test`:

- `support/mod.rs` — the synthetic generator (in-Rust port of the Python tool) +
  the proven parse→polygonize→diff→measure pipeline (Spike 1/2), shared by the
  test binaries. It sits in `tests/` because `etchy-core` stays std-only through
  Phase 0; when the real engine lands (M1) the tests retarget the library API.
- `golden_corpus.rs` — exact ground-truth validation: a self-contained synthetic
  pair (always runs in CI, no Python) **plus** validation against the on-disk
  `synthetic/` corpus + `ground_truth.json` when present (skips with a notice if
  not, unless `ETCHY_REQUIRE_CORPUS=1` is set — then a missing corpus fails).
- `properties.rs` — `proptest` invariants and fuzz-lite "never panics on
  arbitrary input" (real `cargo-fuzz` targets land in M1 against the library
  parsers — you fuzz the parser, which moves into the lib then).

CI's ubuntu job regenerates `synthetic/` as a hard step (a generator failure
fails the job) and runs the tests with `ETCHY_REQUIRE_CORPUS=1`, so the deep
test runs on Linux and cannot silently skip; the Windows/macOS legs don't
generate it (no `python3` on the Windows runner) and rely on the self-contained
synthetic tests, which guarantee coverage everywhere. To run the deep test
locally:

```bash
python3 corpus/tools/gen_synth_board.py --out corpus/synthetic --copper-layers 16 --grid 24
ETCHY_REQUIRE_CORPUS=1 cargo test -p etchy-core --test golden_corpus
```

A live **gerbonara
parity** check is folded into the M1 parser bring-up.

## `synthetic/` — ready now

`tools/gen_synth_board.py` deterministically emits a multi-layer RS-274X board
pair with an **exact, known delta** and a `ground_truth.json` contract:

```bash
python tools/gen_synth_board.py --out synthetic --copper-layers 16 --grid 80
```

The generated `revA/` and `revB/` are gitignored (regenerate on demand — the
generator + `ground_truth.json` are the tracked source of truth). Delta: `F_Cu`
gains a pad block (added), `In1_Cu` loses one (removed), all other layers
byte-identical.

**Verified (2026-06-15)** against the frozen v0.11 tool: it reported `top copper`
+8600px / `inner_1 copper` −8600px and **zero** change on the other 12 layers —
matching `ground_truth.json` exactly. This is the ground-truth backbone for
Spike 1's correctness check (see `docs/SPIKE_1.md`).

> `kicad-demos/` and `real/` are populated as the engine front-ends land in M1.
