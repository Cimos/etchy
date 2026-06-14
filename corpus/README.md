# Golden test corpus

The trust backbone (see DEVELOPER_GUIDE "Trust & testing"). Built in Phase 0,
before features, so every later change is validated against ground truth.

Planned sources:
- **`synthetic/`** — board pairs with *precisely injected* deltas (exact ground
  truth: known added/removed copper area, known moved features).
- **`kicad-demos/`** — public KiCad demo boards turned into revision pairs (realism).
- **`real/`** — Simon's non-confidential boards (actual use cases).

Each entry carries committed **expected outputs** (JSON magnitudes + SVG/heatmap
snapshots via `insta`). Property tests (`diff(A,A)=∅`, add/remove symmetry,
idempotence) and parser fuzzing (`cargo-fuzz`) complement the corpus.

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
