# Spike 2 — Gerber + Excellon parse validation on real fab output

> ROADMAP Phase 0. Goal: confirm a Rust parser covers **real, multi-vendor** fab
> output (or scope the hand-roll), and **validate against gerbonara** on a shared
> file set — before committing to the M1 front-ends. Throwaway code; the output is
> a go/no-go + this findings note.
>
> Companion to [`SPIKE_1.md`](SPIKE_1.md) (the polygon engine, already **GO**).
> Spike code: [`crates/etchy-core/examples/spike2.rs`](../crates/etchy-core/examples/spike2.rs)
> (throwaway, dev-only deps — `etchy-core` stays std-only).

## Method

Parse a large real-world corpus and report, per format, what the candidate parser
handles, what it rejects (and **why**, with sample offending lines), and — for
Excellon — validate structural correctness against an embedded ground-truth oracle.

**Corpus (520 files, 14 EDA tools).** The [gerbonara](https://github.com/jaseg/gerbonara)
test suite (KiCad, Altium, Eagle, OrCAD, Allegro, EasyEDA, DipTrace, PADS, p-cad,
gEDA, geda, Fritzing, Target3001, Upverter, …) + the Ucamco spec example files
from [gerber-parser](https://github.com/MakerPnP/gerber-parser) + our
`corpus/synthetic`. gerbonara's code is Apache-2.0; **the board files are
third-party and are NOT vendored into etchy** — the spike points at a clone.

```bash
git clone --depth 1 https://github.com/jaseg/gerbonara /tmp/gn
git clone --depth 1 https://github.com/MakerPnP/gerber-parser /tmp/gp
cargo run --release --example spike2 -p etchy-core -- \
    /tmp/gn/tests/resources /tmp/gp/assets corpus/synthetic
```

Files are routed Gerber vs Excellon by **content sniff** (fab tools use dozens of
extensions); 90 non-fab files (READMEs, `.py`, aperture/tool sidecars, PDFs) were
correctly skipped and counted.

> **On the "validate against gerbonara" requirement:** the environment has no
> `pip`/network-Python, so gerbonara could not be *run* as a live oracle. Instead
> we used gerbonara's own curated corpus and two independent oracles: (1) the
> `Quantity = N` ground-truth comments OrCAD/Allegro embed per drill tool, and
> (2) gerber-parser's published round-trip tests against the Ucamco files. A live
> gerbonara parity diff is folded into the M1 parser bring-up (DEVELOPER_GUIDE
> already lists it as the CI parity check).

---

## A. Gerber — `gerber-parser` 0.5: **GO (modern output), with a documented shim**

**389 Gerber files. 0 panics, 0 hangs, 0 hard-fails — 100% parsed without
crashing.** This clears the non-negotiable robustness/fuzz bar: the parser is safe
to point at untrusted input. **190 files parsed perfectly clean**; 199 surfaced ≥1
per-command error — but errors are exposed via `doc.errors()`, **never silently
dropped**, so etchy's fail-loud trust bar is *satisfiable on top of this parser*.

**Feature coverage is complete for modern fab output.** Every geometry primitive
the engine needs is parsed across real boards:

| feature | files | occurrences |
|---|---:|---:|
| circle aperture | 357 | 2 914 |
| rect aperture | 138 | 2 021 |
| obround aperture | 104 | 581 |
| polygon aperture | 7 | 16 |
| **macro** aperture (`%AM`) | 104 def in 154 | 2 180 |
| flash `D03` | 267 | 440 522 |
| draw/interpolate `D01` | 228 | 1 028 733 |
| **arc** `G02/G03` | 108 | 55 705 |
| **region** `G36/G37` | 179 | 34 747 |
| **polarity-clear** `LPC` | 62 | 419 |
| step-and-repeat `SR` | 1 | 1 |

KiCad/Altium/EasyEDA X2 exports land in the clean 190. The synthetic corpus and the
Ucamco `polarities_and_apertures.gbr` (all aperture types + arcs + regions +
polarity + a thermal macro) parse cleanly.

### What the 199 reject — categorized (occurrences, with sample lines)

**Geometry-affecting (must address or fail-loud):**

| error kind | occ | cause | sample |
|---|---:|---|---|
| `CoordinateDataWithoutOperationCode` | 611 | **gerber-parser is line-oriented, but Gerber is `*`-delimited.** gEDA/geda pack a whole layer onto one line (`…D02*G75*G03…D01*G01*…`) | `X270000D02*G75*G03X270000Y155000I0J-5000D01*G01*` |
| `OperationBeforeFormat` | 201 | same single-line files: ops seen before the parser locks `%FS` | `X0Y0D02*` |
| `NoRegexMatch(%AD)` | 34 | non-standard named apertures (not `R,1.65X1.65` but `Rect-W…-H…-RO…`) | `%ADD13Rect-W1650000-H1650000-RO1.500*%` |
| `CoordinateFormatMismatch` | 18 | coordinate digit count exceeds the declared `%FS` format | `X1220000Y2570000D02*` |
| `UnsupportedMacroDefinition(%AM)` | 7 | a macro syntax the parser can't build (e.g. fiducial `%AMTARGET*`) | `%AMTARGET*` |
| `NoRegexMatch(%IR)` | 37 | deprecated image-rotation/transform block (pre-2012) | `%IR0*IPPOS*OFA0.00000B0.00000*MIA0B0*…` |

The big two (611 + 201, both from the **single-line / deprecated `G54Dnn` style**)
are concentrated in gEDA-family exports and are the **most impactful gap**. They
are *not* a fundamental limitation — a trivial **pre-normalization shim** (split
the byte stream on `*`, re-emit one command per line) feeds gerber-parser input it
handles. No fork required.

**Metadata-only (geometry intact — safe to ignore or warn):**
`UnknownCommand` 830 (mostly deprecated mode codes `G70*`/`G71*`/`G90*` and
vendor blocks), `UnknownCommand(%LN)` 140 (layer name), `(%IC)` 34, `(%TJ)` 4,
`UnsupportedFileAttribute(%TF)` 1, `InvalidDateTime` 1 (fractional-second
timestamp), `NoEndOfFile` 25 (missing `M02`).

> Note `G70`/`G71` (deprecated inch/mm) and `G90`/`G91` (abs/incremental) parse as
> `UnknownCommand`. They're metadata **unless** a file sets units *only* via
> `G70/G71` (no `%MO`) — then dropping them mis-scales geometry. The shim should
> translate these deprecated codes too; etchy must **fail loud if units are
> unresolved** rather than assume mm.

### Verdict A — **GO** on `gerber-parser` 0.5 for the M1 Gerber front-end, provided:

1. **Pre-normalization shim** before `parse()`: re-split on `*` → one command per
   line, and translate deprecated `G54Dnn` / `G70/71/90/91` to their modern forms.
   This converts the 611+201+ gEDA failures into clean parses. Cheapest, highest-leverage M1 task.
2. **etchy inspects `doc.errors()` and fails loud** on any *geometry-affecting*
   residual (non-standard `%AD`, unsupported `%AM`, `CoordinateFormatMismatch`,
   image transforms `%IR/%MI/%SF/%OF`). Metadata errors (`%LN`, `%TF`, …) → warn.
3. **Upstream the fixes** to MakerPnP (`*`-tokenization, deprecated G-codes,
   `%AMTARGET`) — same permissive org as our whole stack; benefits everyone and
   shrinks our shim over time. Pin `=0.5` until then (pre-1.0).

`gerber-parser` produces a typed command AST, **not** filled polygons — etchy owns
the graphics-state → `Primitive` resolution either way (DEVELOPER_GUIDE), so this
parser remains the right choice; the gaps are at the lexer, not the model.

---

## B. Excellon — hand-rolled parser **confirmed necessary; scope is bounded**

No Excellon crate exists (DEVELOPER_GUIDE). The spike's minimal prototype parsed
**41 drill files → 11 066 hits**, and validated structure against the embedded
oracle: on `orcad/arena…`, the per-tool hit counts the parser recovered **matched
the file's own `Quantity = N` comments exactly** (5 tools, 859 hits). Because hit
and tool counts are **independent of coordinate format**, this is a clean
structural correctness check even where coordinate decoding is ambiguous.

### Dialect variance the front-end must absorb (observed across 14 tools)

- **`M48` header optional** — PADS and some Allegro files have none; the prototype
  must infer header vs body without it.
- **Units:** `INCH` / `METRIC`, with **all** of `LZ` / `TZ` / none seen
  (INCH, INCH·LZ, INCH·TZ, METRIC, METRIC·LZ, METRIC·TZ all present), and some
  files with **no units keyword at all** (Allegro `ncdrill` — units are external).
- **Coordinate format — the crux ("never guess"):**
  - *Decimal* coords (`X2.1142`) — unambiguous (KiCad, Upverter, some Fritzing).
  - *Integer + explicit format* — from `;FILE_FORMAT=2:4` (EasyEDA), `METRIC,…,000.000`
    (p-cad 3:3), or the `INCH/METRIC` trailer.
  - *Integer, NO declared format* — **18 files** (gEDA `.cnc`, DipTrace, several
    `.ncd`, PADS). These are **genuinely ambiguous** and the parser **flags** them
    rather than guessing. **Decision needed (see below).**
- **Tool defs vary in field order:** `T1C0.016` (KiCad) vs `T1F00S00C0.300`
  (Target3001 — feed/speed *before* `C`) vs `T1C.015F095S3` (PADS — trailing
  feed/speed). The prototype now keys on "digits after `T`" + "digits after `C`",
  which handles all three.
- **Signed coordinates** (`X+026814`, DipTrace), `G85` slots, plated/non-plated
  `;TYPE=` sections, `FMAT,2`, mode codes `G90/G05/M71/M72/M30`.

### Open decision (resolve before M1 Excellon work)

**Ambiguous integer-coordinate files (no declared format): policy?** Options:
1. **Fail loud** — strictest, matches the trust bar, but rejects ~18/41 real files.
2. **Documented heuristic** (gerbonara-style: infer from coordinate digit-width +
   value ranges) **with a loud warning** in output, overridable by a CLI flag.

Recommendation: **(2) heuristic + warning + override flag.** Pure fail-loud is too
brittle for a "diff any fab pack" tool; but the inference must be explicit,
surfaced, and user-overridable — never silent. (This mirrors gerbonara's pragmatic
stance while keeping etchy's "no silent misses" promise.)

### Verdict B — hand-roll is justified and **well-scoped**. M1 Excellon front-end:

M48(optional) header → units+zero-suppression+format (explicit, else flagged
heuristic) → tool table (`T<n>…C<dia>`, any field order) → tool-select → `X..Y..`
hits→`Flash`, `G85`/routed→`Line`/`Arc` → emit the same `Primitive`s as Gerber.
Validate per-tool counts against `Quantity=` comments where present; **fuzz it**
(DEVELOPER_GUIDE flags the hand-rolled parser as the top fuzz priority).

---

## Combined go/no-go

| | verdict |
|---|---|
| **Gerber parser** (`gerber-parser` 0.5) | **GO** — robust (0 panic/389), full modern-feature coverage; add a `*`-tokenization + deprecated-G-code shim and fail-loud on `doc.errors()`. Upstream fixes. |
| **Excellon parser** (hand-rolled) | **GO to build** — necessity confirmed, scope bounded, structural correctness oracle-validated; resolve the ambiguous-format policy first. |
| **Trust bar** | satisfiable — every parser gap is *surfaced*, not silently dropped; the only judgement call (ambiguous Excellon format) gets an explicit, overridable, warned heuristic. |

## Recommended next steps

1. **Golden-corpus harness** (ROADMAP Phase 0, the remaining Phase-0 item): wire the
   property tests (`diff(A,A)=∅`, add/remove symmetry) + the synthetic ground truth
   + a curated slice of this real corpus into CI; add a live **gerbonara parity**
   check once Python tooling is available in CI.
2. **M1 Gerber front-end**: the normalization shim + graphics-state→`Primitive`
   resolution (reference `gerber-viewer`), fail-loud on geometry-affecting errors.
3. **M1 Excellon front-end** per the bounded scope above; fuzz both parsers.
4. **Upstream** the gerber-parser fixes (`*`-tokenization, deprecated G-codes,
   `%AMTARGET`) to MakerPnP.
