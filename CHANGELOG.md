# Changelog

All notable changes to etchy are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Raster page-diff engine** (`etchy_core::imagediff`) — the pure-Rust foundation
  for schematic-PDF diff (#63). Diffs two same-size RGBA pages into added / removed
  / changed pixel tallies, a connected-region count (4-connectivity, with a
  noise-size filter), and a brand-coloured overlay (green added / red removed /
  amber changed on a ghosted page). Fails loud on a page-size or buffer mismatch —
  etchy never rescales or realigns a raster. PDF rasterization wiring lands next.
- **Schematic-PDF page diff** (`etchy-pdf`, behind the `pdf` feature) — a pure-Rust
  rasterizer (hayro, no C++/PDFium) renders each page and feeds the raster engine,
  pairing revisions' pages by index. `diff_pdfs` returns per-page tallies + a PNG-
  encodable overlay; `available()` reports whether the backend was compiled in. The
  heavy PDF stack (and its higher MSRV) only builds with `--features pdf`, so the
  default binaries stay lean. CLI wiring lands next.

### Fixed
- **A PDF sheet inserted mid-document no longer desyncs every later page**
  (#249). Pages used to pair strictly by index, so inserting one sheet made every
  following pair compare the wrong sheets — all of them reported as heavily
  changed, drowning the real edit. Pages now pair by **content**: each rasterized
  page is fingerprinted (a 16×16 luminance digest of the raster already rendered)
  and the two revisions' page sequences are aligned, so an inserted or removed
  sheet becomes an explicit `new-only` / `old-only` row and the sheets around it
  keep pairing with themselves. The alignment is always stated — in the summary,
  the Markdown table, the JSON (`alignment`), and the viewer — except when it is
  the plain index pairing, which says nothing extra. The fingerprint only decides
  *which* pages pair: every paired sheet still gets the full pixel diff (a lossy
  digest must never stand in for one), and every page of both revisions appears
  exactly once. Per-page rows now carry `old_page` / `new_page` so the sheet each
  row compares is explicit.
- **Real-board (Altium) validation fixes** — found by running a full production
  fab pack end-to-end: tool definitions with feed/speed *before* the diameter
  (`T1F00S00C0.00787`) now parse (the whole pack was rejected); Altium's columned
  pick-and-place format (trailing quoted multi-word descriptions) now parses
  every row (rows with spaces in the description were silently skipped — a
  partial placement diff); and the same-board guard now compares **physical
  layers only**, so a drill drawing regenerated with a legend table no longer
  false-positives as "not the same board".
- **Ten trust defects from an adversarial code review** (all reproduced by
  failing tests first): Excellon headerless files no longer resolve to an empty
  drill layer; inline digit formats (`METRIC,LZ,0000.00`) now scale coordinates
  correctly; `R` repeat codes emit the repeated hits instead of one phantom hole;
  `G00`/`G01` tool-up moves no longer paint phantom holes; tool lines with
  feed/speed fields (`T1C0.5F200S65`, `T01F200S65`) parse instead of rejecting
  the file or being ignored. A misnamed drill file (e.g. Altium `Board.TXT`) now
  lands on the Drill layer by content, so `--gate-layers drill` sees it; unknown
  `--gate-layers` tokens are a loud exit 2 instead of a silently disarmed gate;
  git mode is explicit (`--git`/`[SUBDIR]`) so a typo'd folder can't be
  reinterpreted as a ref, and whole-repo git diffs print a scoping note.

### Added
- **Help menu (viewer):** a toolbar **Help** menu with links to the GitHub repo,
  the website, "report an issue", and **Sponsor / fund etchy** (GitHub Sponsors),
  plus the version. Opens links in the browser on native and web. A repo
  `.github/FUNDING.yml` adds the Sponsor button too.
- **Pick-and-place input (#115):** centroid files (KiCad `.pos`, and Altium/JLC/
  generic centroid CSVs) are parsed and each component rendered as a rotated
  marker at its position, so **moved / rotated / added / removed parts diff** in
  the overlay, HTML, magnitudes, and CI gate — as geometry, not a BOM list. New
  `LayerKind::Placement`; routed by all loaders (CLI + GUI + web).
- **HTML report (M1):** `--html <file>` writes a single self-contained HTML report
  — totals, warnings, and each changed layer's overlay (the per-layer SVG, inlined)
  with its magnitudes. No external assets; shareable and printable.
- **Git-refs invocation (M2):** `etchy <refA> <refB> [subdir]` diffs two committed
  revisions of a repo's Gerbers with no checkout (reads the blobs via `git`).
  Auto-detected when the OLD argument isn't a directory, or forced with `--git`.
- **CI gate thresholds (M2):** `--fail-on-area <mm²>` and `--fail-on-regions <n>`
  gate the exit code on the *magnitude* of change, and `--gate-layers <spec>`
  (e.g. `copper`) scopes the gate to specific layers — so a pipeline can fail on
  copper changes and ignore silkscreen churn. With no flags the exit contract is
  unchanged (any change ⇒ exit 1). The verdict prints to stderr.
- **Distribution (1.0 track):** a **distroless container** (`Dockerfile` — a
  static musl `etchy` CLI in `gcr.io/distroless/static`, ~11 MB, no shell/libc)
  and a lean, tag-triggered **release workflow** (`.github/workflows/release.yml`)
  that builds per-OS CLI binaries (Linux musl static, macOS x86_64 + arm64,
  Windows) and attaches the archives + `SHA256SUMS` to the GitHub Release.
- Phase-0 scaffold: Cargo workspace (`etchy-core`, `etchy-cli`, `etchy-gui`,
  `etchy-pdf`), dual MIT/Apache-2.0 license, CI (build + test matrix on
  Linux/macOS/Windows), and the planning docs (roadmap, developer guide,
  product discovery).
- Core IR types: `geo::{Pt, Aperture, Primitive}`, `model::{Layer, LayerKind}`,
  `diff::LayerChange`, `report::DiffReport` (+ `SCHEMA_VERSION`).
- `etchy` CLI exit-code contract (0 = no diff, 1 = diff found, 2 = error).
- Phase-0 de-risking spikes (throwaway, dev-deps only):
  - **Spike 1** (`examples/spike1.rs`): proves the `i_overlay` 7.0 polygon-diff
    engine — correctness vs synthetic ground truth, ~2.1 s / ~373 MB on a dense
    16-layer board, deterministic. **GO.** See `docs/SPIKE_1.md`.
  - **Spike 2** (`examples/spike2.rs`): validates `gerber-parser` 0.5 + scopes the
    hand-rolled Excellon parser over a 520-file multi-vendor corpus (0 panics;
    full modern-feature coverage; gaps catalogued). **GO.** See `docs/SPIKE_2.md`.
- **Golden-corpus harness** (the trust backbone, `crates/etchy-core/tests/`):
  self-contained synthetic golden tests vs exact ground truth, `proptest`
  invariants (`diff(A,A)=∅`, add/remove symmetry, area bounds, determinism),
  and fuzz-lite "never panics on arbitrary input" — wired into CI via `cargo test`.
- **Milestone 1 — engine + CLI (first vertical slice).** The spike-proven
  pipeline promoted into a real `etchy-core` library and a working `etchy` CLI
  (design: `docs/M1_ENGINE_DESIGN.md`):
  - Engine: Gerber bytes → `parse_gerber` (graphics-state resolve, **flash of
    circle/rect apertures**) → `polygonize` → `diff_layer` (i_overlay **i64
    integer** engine; `added = B−A`, `removed = A−B`) → measure (exact `i128` nm²
    + region count) → `compare` (same-board guard, pair-by-kind, `DiffReport`).
    Coordinates quantized mm → i64 nm via a guarded `quantize_mm`. Core stays
    pure (no I/O / path / `println!` / `exit`).
  - **Fail-loud trust bar:** every unrendered feature (traces/regions/arcs/macros/
    obround/polarity/drilled apertures, compact multi-command-per-line Gerber,
    non-positive apertures, parse errors) is a typed `EngineError`, never a
    silent or wrong-but-quiet diff.
  - CLI `etchy <old> <new>`: dir walk + content-sniff + filename→`LayerKind`
    classify (I/O lives in the CLI), terminal summary, `--json` report
    (`schema_version = 1`, exact nm² as strings), exit codes 0/1/2.
  - The Phase-0 golden/property harness retargeted onto the shipping library;
    new `etchy-cli/tests/cli.rs` pins the exit-code + JSON contract.
  - Hardened against an adversarial review pass (overflow cap, duplicate-layer-kind
    rejection, compact-line detection, sniff robustness, empty-layer status).
  - Scope deferred to later increments: line/region/arc/macro polygonization, the
    Gerber `*`-tokenization shim, Excellon, SVG/HTML/heatmap render, the egui GUI.
- **Full Gerber polygonizer + native GUI — diffs real production boards.** The
  engine grew from flash-only to a complete RS-274X graphics-state machine:
  - **Normalization shim** (`gerber::normalize`): splits compact/`*`-packed and
    combined `Gnn…Dnn` blocks into one command per line (recovering geometry the
    line-oriented parser would drop), drops deprecated `G70/G71/G90/G54`, fails
    loud on `G91`.
  - **Geometry** (`geom`): stroked `D01` lines and `G02/G03` arcs (capsule
    Minkowski stroke; sagitta-bounded tessellation), `G36/G37` region fills
    (even-odd normalized), circle/rect/**obround**/polygon flashes, and an
    **aperture-macro evaluator** (circle/center-line/outline/vector-line/polygon
    primitives, rotation, per-primitive exposure).
  - **Polarity:** `LPD/LPC` resolved as `dark − clear` in one boolean pass per
    layer (`boolean` module, shared with the diff). i_overlay's types no longer
    leak past it.
  - Layer **pairing is now by filename** (exact for same-board revisions; never
    collapses multiple same-kind layers); the same-board guard tolerance is
    generous (catches gross mismatches, allows revision edge changes).
  - **Native egui GUI** (`etchy-gui <old> <new>`): changed-first layer list +
    pan/zoom canvas (added green / removed red, base toggle, Overlay/Before/After),
    with a WSL software-GL/X11 fallback so it launches out-of-the-box.
  - Validated end-to-end on a real **Altium** board (a second real board A↔B):
    16 layers, all features exercised; `diff(A,A)=∅`; A↔B yields a full diff.

### Notes
- Crates are std-only at Phase 0 so the workspace builds without a local
  toolchain; the verified real dependencies (clap, i_overlay, eframe,
  pdfium-render, askama, …) are added per-module starting in Milestone 1.
