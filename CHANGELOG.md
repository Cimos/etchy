# Changelog

All notable changes to etchy are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Cut a new section here as part of tagging a release — see "Cutting a release" in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## [Unreleased]

## [0.1.1] — 2026-10-03

### Added
- **Desktop viewer installers:** each release now ships `etchy-viewer-…` as a
  Windows `.msi`, a macOS `.dmg` (Apple Silicon) and a Linux `.AppImage` and
  `.deb`, built by cargo-packager in the release workflow. A Homebrew cask is
  available from the `cimos/etchy` tap.
- `README-CLI.txt` in every command-line archive, explaining that it is a
  terminal tool and how to run it.

### Changed
- Command-line archives are renamed `etchy-cli-<version>-<os>` so they are not
  mistaken for the viewer.
- The Windows viewer no longer opens a console window behind it.
- The release workflow can be run by hand to build every asset without
  publishing.

### Fixed
- The viewer resolves relative folder arguments correctly when launched from
  an AppImage.

## [0.1.0] — 2026-10-02

### Added
- **Schematic-PDF diff, end to end (#63):** `etchy old.pdf new.pdf` runs a
  page-by-page pixel diff in the CLI (summary / JSON / Markdown, `--dpi`,
  `--out` overlay PNGs, a per-page pixel cap) and the viewer opens the same pair
  with the full mode set on native and web. Pages pair by content, so an
  inserted sheet is one extra row instead of a desynced document.
- **Raster page-diff engine** (`etchy_core::imagediff`) — the pure-Rust foundation
  for schematic-PDF diff (#63). Diffs two same-size RGBA pages into added / removed
  / changed pixel tallies, a connected-region count (4-connectivity, with a
  noise-size filter), and a brand-coloured overlay (green added / red removed /
  amber changed on a ghosted page). Fails loud on a page-size or buffer mismatch —
  etchy never rescales or realigns a raster.
- **Schematic-PDF page diff** (`etchy-pdf`, behind the `pdf` feature) — a pure-Rust
  rasterizer (hayro, no C++/PDFium) renders each page and feeds the raster engine.
  `diff_pdfs` returns per-page tallies + a PNG-encodable overlay; `available()`
  reports whether the backend was compiled in. The heavy PDF stack (and its higher
  MSRV) only builds with `--features pdf`, so a default build stays lean.
- **Viewer shell redesign** (#167 → #175 → #182 → #202, plus #210/#214/#226/#229/
  #232/#233): a VS Code-style activity rail — flippable to either side — opening
  **Layers** and **Export** panels, with **Measure** as a plain tool toggle
  between them (no panel) and **Settings** as a gear pinned at the bottom. The
  top bar keeps only the board labels, the Overlay / Old / New / Split / Swipe
  segment, and Help / Fit / Open. Settings gained a hotkey editor where every
  action rebinds, and the build's git short sha is baked in and shown in Help so
  a browser tab can prove which build it runs.
- **Eyes-only layer visibility with a Focus slider (#224):** the old
  single/highlight/all/none view segment is gone. Eyes are the only visibility
  control, selection no longer hides anything, and a Focus slider renders every
  non-selected visible layer at `1 − focus` — base and diff geometry alike — so a
  busy board can be buried without hiding it. Hiding every eye now leaves a
  genuinely blank canvas.
- **Five-line CI adoption (#241):** adding the GitHub Action to a repo takes a
  five-line `uses:` block. The Action supplies its own token and posts a sticky
  per-layer table on the pull request, editing it in place on each push.
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
- **Settable PDF resolution in the viewer (#223):** 150 / 200 / 300 DPI chips in
  Settings › Diff, default 200. Changing it re-rasterizes the loaded pair from the
  retained source bytes; a DPI that breaches the raster caps fails loud and the
  setting reverts.

### Changed
- **Release binaries and the container ship the PDF feature (#252):** both are
  built with `--features pdf`, so a downloaded etchy diffs schematic PDFs without
  a rebuild. hayro is pure Rust and links static on musl.
- **Supply-chain hardening of CI (#278):** third-party actions are pinned by
  commit SHA, the release token is scoped to the job that publishes, wasm
  tarballs are checksummed, `cargo-deny` is pinned, and the golden-corpus gate
  and demo-asset guard were repaired.
- **Export always lands somewhere findable (#222):** native writes an
  `etchy-export/` folder next to the last opened input and reports the absolute
  path; the web build downloads a single file directly and bundles a multi-file
  set into one zip, because browsers block the second and later automatic
  downloads from one click.

### Fixed
- **The demo server no longer serves the feedback log or tester screenshots over
  GET** (#334). The block compared the raw request string, so `/feedback%2Ejsonl`,
  `/./feedback.jsonl` and `//feedback.jsonl` fetched the file, and the
  `screenshots/` dir beside it was never blocked at all — anyone on the LAN could
  list and pull every tester's IP, name, text and screenshots. The decision is now
  made on the resolved real path (feedback file, script, anything under
  `screenshots/`), directory listings return 404, and an offline unittest covers
  the bypasses (`deploy/demo/test_etchy_server.py`).
- **A schematic page whose paper size changed is now a diff, not an error**
  (#262). A resized sheet used to fail the whole run with `ImageSizeMismatch` and
  exit 2 — which CI treats as infrastructure failure to retry, not a review gate
  to block. A resized sheet is a legitimate revision diff, so it is now reported
  as a fully-changed page and the run exits 1; exit 2 is reserved for input etchy
  cannot render at all. The rest of the document still diffs normally. Both pixel
  sizes are named in the summary, the Markdown, the JSON (`size_change`), and the
  viewer, so a 100%-changed page with zero changed pixels never looks like a tool
  bug. Two rasters of different sizes have no pixel correspondence, so such a page
  has no diff overlay: the viewer shows both sheets side by side at their true
  scale (etchy still never rescales a raster) and the Export tab names the page
  and the reason instead of quietly omitting its PNG.
  Sizes are compared with a **2 px per-axis tolerance**: flooring
  `points × dpi / 72` puts two exports of the same paper a pixel apart (an A4
  landscape `MediaBox [0 0 842 595]` gives 1754×1239 px at 150 DPI, the exact
  `[0 0 841.89 595.276]` gives 1753×1240), and calling that a resize would throw
  the sheet's whole pixel diff away. Within the tolerance it is the same sheet:
  both rasters are cropped to the region they share, the pair is diffed in full,
  and the crop is reported (`rounding_crop` in the JSON, a "page size rounding"
  line in the summary/Markdown, a chip in the viewer).
- **A PDF sheet inserted mid-document no longer desyncs every later page**
  (#249). Pages used to pair strictly by index, so inserting one sheet made every
  following pair compare the wrong sheets — all of them reported as heavily
  changed, drowning the real edit. Pages now pair by **content**: each rasterized
  page is fingerprinted (a 16×16 grid of per-cell ink coverage, taken off the
  raster already rendered) and the two revisions' page sequences are aligned, so an
  inserted or removed sheet becomes an explicit `new-only` / `old-only` row and the
  sheets around it keep pairing with themselves. The alignment is always stated —
  in the summary, the Markdown table, the JSON (`alignment`), and the viewer —
  except when it is the plain index pairing, which says nothing extra. The
  fingerprint only decides *which* pages pair: every paired sheet still gets the
  full pixel diff (a lossy digest must never stand in for one), and every page of
  both revisions appears exactly once. Per-page rows now carry `old_page` /
  `new_page` so the sheet each row compares is explicit.
  Each cell's value is the fraction of it darker than a **fixed absolute**
  luminance, quantized on a log ladder, and two digests are compared by L1 distance
  over those per-cell values. That keeps the digest local: ink added anywhere moves
  only the cells it touches, which is what sparse schematic line art needs. Two
  ceilings sit on top of it. A re-pairing is adopted only when it beats plain index
  pairing by a clear margin *and* every pair it chooses is more alike than chance;
  otherwise the alignment keeps index pairing and says "page alignment was
  ambiguous, paired by index" (the JSON gains `alignment.basis`). And the page
  count is capped at **1024 per side** before the alignment matrix is allocated —
  it is quadratic in the page count, so two 10 000-page PDFs (small files, under
  every other cap) would otherwise ask for 800 MB and abort the process with no
  message; over the cap etchy exits 2 naming the input, the count and the limit.
- **The PDF noise floor can no longer hide a change silently** (#276): regions
  under `--min-region-px` are kept out of the overlay but still counted, still
  reported, and still make the run a difference. The default is 1, which hides
  nothing.
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
- **Engine correctness on real exports:** `G74`/`G75` arc quadrant mode is
  honoured and tiny-arc tessellation is floored (#274); drill files are classified
  by plating (PTH / NPTH) so they pair like with like (#275); positional layer
  pairing is guarded against matching unrelated files (#273); and X2
  `.FileFunction` attributes are cross-checked against the filename, with the
  core's test-coverage gaps closed (#239, #240).
- **Viewer trust and fidelity:** four trust bugs in the diff canvas (#277),
  export colour fidelity and input size guards (#244, #246, #247), and the app
  icon now sets the native window icon (#282). MSAA is dropped under software
  rendering so the viewer doesn't peg a WSL machine (#216).
- **CLI robustness (#279):** a broken pipe (piping into `head`) exits quietly
  instead of panicking, the gate verdict wording is unambiguous, and a PDF
  mismatch reports properly.

## [0.1.0-rc1] — 2026-07-01

First tagged pre-release: the engine, CLI and native viewer diffing real
production fab packs, with the release pipeline proven across four platforms.

### Added
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
  - Validated end-to-end on a second real **Altium** board (revs A↔B):
    16 layers, all features exercised; `diff(A,A)=∅`; A↔B yields a full diff.
- **Excellon / NC drill input** — hits and slots parsed into a drill layer, routed
  through the same front-end in the CLI and the viewer, so drilled features diff
  alongside copper.
- **Exports:** per-layer SVG overlays of the diff, plus a copper-area CSV.
- **The viewer grew into a usable tool:** a multi-layer view with highlight/dim
  and group show/hide, a **swipe/curtain** compare and a side-by-side split, a
  measure tool with a grid overlay, an Open revision A / B loader (folder, `.zip`
  or drag-and-drop, remembering the last directory), per-layer change percentages,
  a unified Settings panel with persisted settings and configurable canvas and
  grid colours, per-ECAD input presets, mode hotkeys, and a brand splash on
  startup.
- **Trust in the render:** zoom-stable diff level-of-detail with marker dots so a
  real change never fades to nothing, tessellation-robust region counts, and
  edge-stroked compact regions so adjacent added and removed geometry reads apart.

### Changed
- **Same-board guard tightened to spec** — 1 mm / 2 % of span rather than 10 %, so
  a genuinely different board is caught while a revision's edge changes still pass.
- **Layer classification** types Altium fab-documentation Gerbers as their own kind
  instead of lumping them in with the rest.
- **Performance:** the per-layer diff runs in parallel (rayon), layer geometry is
  shared by `Arc` instead of deep-cloned, render-only base-mesh simplification and
  pre-reserved per-frame buffers cut all-layers pan cost, and the renderer moved
  from wgpu to glow with a release profile to shrink the binary.

### Security
- **Resource caps** on total input size, per-layer object count, and per-layer
  point and polarity-span counts, so a hostile or malformed file fails loud
  instead of exhausting memory.
- `cargo-deny` runs in CI, with a fuzz target and a parser panic boundary.

### Notes
- Crates are std-only at Phase 0 so the workspace builds without a local
  toolchain; the verified real dependencies (clap, i_overlay, eframe,
  pdfium-render, askama, …) are added per-module starting in Milestone 1.

[Unreleased]: https://github.com/Cimos/etchy/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/Cimos/etchy/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Cimos/etchy/compare/v0.1.0-rc1...v0.1.0
[0.1.0-rc1]: https://github.com/Cimos/etchy/releases/tag/v0.1.0-rc1
