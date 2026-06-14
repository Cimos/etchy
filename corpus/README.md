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

> Empty at Phase 0 — populated as the engine front-ends land in Milestone 1.
