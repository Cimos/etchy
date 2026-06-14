# etchy — Roadmap

> Planning artifact for **etchy**, the Rust successor to gerber-diff. Lives here
> temporarily; seeds the fresh `Cimos/etchy` repo. Decisions are grounded in
> [`PRODUCT_DISCOVERY.md`](PRODUCT_DISCOVERY.md) (the assume-nothing interview).
> Companion: [`DEVELOPER_GUIDE.md`](DEVELOPER_GUIDE.md) (architecture + stack).

## What etchy is

A fast, trustworthy, open-source **PCB visual + geometric diff** tool. Point it at
two revisions of a board's fab output and it shows — and measures — exactly what
changed. CLI/CI-first, with a native desktop viewer; built ground-up in Rust for
small static binaries and dense-board speed.

**One-liner:** `etchy old/ new/` → a crisp overlay, a "where to look" heatmap,
and machine-readable magnitudes — on any Gerber/Excellon fab pack.

## Guiding principles (from the interview)

1. **Trustworthy over clever.** A *missed* change is the worst outcome. Validated
   polygon ops, a golden corpus, property/fuzz tests, and **fail-loud** ("couldn't
   process this layer") rather than wrong-but-quiet. This gates every milestone.
2. **Fast "what changed?"** Low ceremony; instant cold-start; smooth on dense
   8–16+ layer boards.
3. **CLI/CI-first, GUI second** — but the GUI ships in the MVP.
4. **One computation, three views.** A single per-layer polygon diff yields the
   overlay, the heatmap, and the magnitudes; humans see visuals, machines get numbers.
5. **Same-board revisions only.** Absolute coordinates, no registration; detect and
   clearly error on genuinely different boards — never emit a garbage diff.
6. **Self-contained distribution.** Per-OS static binaries + a distroless container.
   No runtime to install.

## Scope

**In:** Gerber RS-274X/X2, Excellon drill, schematic-PDF (pixel page-diff, kept).
**Out (explicit non-goals):** native CAD ingestion (KiCad/Altium/IPC-2581/ODB++),
net/connectivity diff, BOM/component diff, DRC/rule-checking. Anything here is
"Beyond 1.0", not forgotten — see the bottom.

---

## Phases & milestones

Time estimates are deliberately omitted (solo, open-source cadence); milestones
are ordered by dependency and each is independently shippable.

### Phase 0 — Foundations & de-risking
The big-bang rewrite's main risk is time-to-first-value; Phase 0 buys down the
highest-uncertainty pieces before committing to the full build.

- **Repo scaffold:** `Cimos/etchy`, Cargo workspace, dual `MIT OR Apache-2.0`,
  CI (fmt/clippy/test/build matrix), `CHANGELOG`, contributor docs.
- **Spike 1 — polygon engine:** prove the chosen boolean-ops crate on a real dense
  board (16-layer pour): correctness + performance + memory. This is the
  technical crux; if it fails, the whole approach changes. (See DEVELOPER_GUIDE
  "Polygon booleans".)
- **Spike 2 — Gerber/Excellon parsing:** confirm a Rust parser covers real fab
  output (or scope the hand-rolled parser). Validate against gerbonara output on
  the same files.
- **Golden-corpus harness:** the trust backbone, built first so every later
  feature is validated against ground truth. Sources: synthesized known-delta
  board pairs (exact ground truth) + public KiCad demo boards + Simon's
  non-confidential boards. Property tests (`diff(A,A)=∅`, symmetry of add/remove)
  + fuzzing wired into CI.

**Exit:** the engine spike diffs a real dense board correctly and fast enough; the
corpus harness runs in CI.

### Milestone 1 — MVP (engine + CLI + GUI)
The first genuinely useful etchy. A user can diff two Gerber/Excellon revisions
and *see* + *export* the result.

- **Engine:** parse → per-layer polygons (stroke/flash/fill) → boolean diff
  (`added`, `removed`) → magnitudes (changed area mm², region count) → render.
- **Layer pairing** (rename-tolerant, by type) + **same-board guard** (clear error
  on mismatched extents).
- **CLI:** `etchy <old> <new>` → writes outputs; rich terminal summary; `--json`.
- **Outputs (all first-class):** self-contained **HTML** report, standalone
  **SVG** overlays, machine-readable **JSON**.
- **Native GUI (egui):** the single-window viewer — layer list (changed-first),
  modes (overlay / before / after / split / swipe / onion) + **heatmap**,
  pan/zoom/fit, keyboard-driven. (Carries forward the v0.11 UX, natively.)
- Validated end-to-end against the golden corpus.

**Exit:** "drop two fab packs, see and export the diff" works on real boards,
trustworthy on the corpus.

### Milestone 2 — CI surface
Make etchy a first-class pipeline gate (the primary surface).

- **Thresholds + exit codes:** gate on changed-area / region-count, **scopable per
  layer** (e.g. fail on copper, ignore silkscreen).
- **Git-refs invocation:** `etchy <refA> <refB> [subdir]` over a repo of committed
  Gerbers (no checkout).
- **PR comments with visuals:** GitHub Action (and GitLab CI) that posts the
  summary + an image/heatmap inline; fully offline/self-hosted.
- **Stable, versioned JSON schema** published as the integration contract.

**Exit:** a PR that changes a board shows an inline visual diff and can fail CI on
a threshold.

### Milestone 3 — Schematic PDF + dense-board hardening
- **Schematic-PDF pixel diff** ported (page-by-page), as a supported secondary mode.
- **Performance pass** for dense/many-layer boards: parallelism across layers,
  memory ceilings, adaptive work; benchmarked against targets.
- **Heatmap polish:** region ranking, "jump to biggest change", thresholded views.

**Exit:** smooth on a 16-layer dense board; PDF diff usable.

### Milestone 4 — 1.0 (distribution, docs, trust complete)
- **Distribution:** per-OS static binaries (incl. Linux musl) + **distroless
  container**; reproducible release pipeline.
- **Docs site:** install, CLI reference, CI recipes, JSON schema, "how the diff
  works", trust/limitations statement.
- **Trust corpus complete** + coverage/fuzz gates green; limitations documented.
- **Polish:** error messages, `--help`, examples, sample boards.

**Exit:** a stranger can install etchy, diff a board, and gate a PR in minutes —
and trust the result.

---

## Beyond 1.0 (parked, not forgotten)
Revisit only if demand appears; each was explicitly deferred in discovery:
- Native CAD ingestion (KiCad `.kicad_pcb` via parser or `kicad-cli` plot bridge;
  Altium) → unlocks object identity + git-native rev-to-rev for KiCad users.
- Structured change-lists & per-object deltas ("via moved 0.3 mm") — needs object
  identity from native CAD.
- IPC-2581 / ODB++ ingestion (carry net + component data).
- Net/connectivity diff, BOM/component diff. (DRC remains out — different tool.)

## Risks & mitigations
| Risk | Mitigation |
|---|---|
| Big-bang rewrite → long gap with no usable tool | Phase 0 spikes de-risk the crux early; v0.11 Python stays available (frozen); M1 is scoped to "usable", not "complete". |
| Polygon booleans slow/incorrect on dense pours | Spike 1 proves the crate before committing; fail-loud fallback; benchmark gates. |
| Gerber parser gaps on real-world fab quirks | Validate against gerbonara on a wide corpus; hand-roll only what's needed. |
| PDF rendering licensing (mupdf = AGPL) | Use a permissively-licensed path (pdfium BSD bundling or pure-Rust); decided in DEVELOPER_GUIDE. |
| "Trustworthy" is hard to prove | Golden corpus + property/fuzz from Phase 0; fail-loud; documented limitations. |
| Solo maintenance load | Keep scope tight (non-goals enforced); permissive license + good docs to invite contributors. |

## Success metrics (1.0)
- Correctly diffs the full golden corpus with zero silent misses.
- Smooth (interactive) on a 16-layer dense board.
- `cargo install`-free: a colleague installs a binary / runs the container and gets
  a diff in < 2 minutes.
- Adopted in at least one real CI pipeline (dogfooded on a CubePilot board repo).
