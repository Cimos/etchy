# Golden test corpus

The engine tests use generated synthetic Gerber pairs with known added and
removed areas. The generator and `synthetic/ground_truth.json` are tracked;
`synthetic/revA/` and `synthetic/revB/` are gitignored and regenerated when
needed. Public Mad_RP2040 fab revisions are committed under
`crates/etchy-gui/assets/demo/`. Native KiCad fixtures and exported oracles live
under `corpus/native/kicad/`; PDF fixtures live under `corpus/pdf/`. Small
Gerber parser cases, such as KiCad's stock `RoundRect` macro with a known
flash area, live under `corpus/gerber/`.

## Harness

`crates/etchy-core/tests/golden_corpus.rs` runs self-contained synthetic tests
on every `cargo test`. It also checks the generated on-disk corpus when present.
Set `ETCHY_REQUIRE_CORPUS=1` to fail if those files are missing; Linux CI does
this after generating them. `properties.rs` runs `proptest` invariants, and
`fuzz/` holds the resolver fuzz targets.

To run the on-disk check locally:

```bash
python3 corpus/tools/gen_synth_board.py --out corpus/synthetic --copper-layers 16 --grid 24
ETCHY_REQUIRE_CORPUS=1 cargo test -p etchy-core --test golden_corpus
```

The generator emits a multi-layer RS-274X board pair with an exact delta:
`F_Cu` gains a pad block, `In1_Cu` loses one, and other layers are unchanged.
The expected magnitudes are in `synthetic/ground_truth.json`.
