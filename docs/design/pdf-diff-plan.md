# Schematic-PDF diff — implementation plan (#63)

Wiring the already-built PDF engine into the CLI. Requirement **CLI-6**
([`../REQUIREMENTS.md`](../REQUIREMENTS.md)); scope **SCOPE-2**. This plan is
grounded in a full read of the current code (paths + line refs below).

## 1. Where we are

**The engine is done and unit-tested — only the CLI wiring is missing.**

- `etchy-core::imagediff` (`crates/etchy-core/src/imagediff.rs`): `diff_images(old, new, opts) -> ImageDiffResult { stats, overlay }`. `Image { width, height, rgba }`. Per-pixel classify (added/removed/changed) + connected-component region count + a brand-coloured overlay `Image`. Fails loud on size mismatch. `ImageDiffOptions { ink_threshold, change_threshold, min_region_px }`.
- `etchy-pdf` (`crates/etchy-pdf/src/lib.rs`, feature `pdf`, off by default, pure-Rust **hayro** — *not* pdfium): `rasterize(bytes, dpi) -> Vec<Image>` (all pages), `diff_pdfs(old, new, dpi, opts) -> PdfDiff`, `encode_png(&Image) -> Vec<u8>`, `available() -> bool`, `DEFAULT_DPI = 150.0`. `PdfDiff { old_pages, new_pages, pages: Vec<PageDiff{page, old_page, new_page, diff: Option<ImageDiffResult>}>, alignment }` with `any_changes()`. Pages pair **by content** (#249, see §3.4); a sheet on one side only is a row with no diff.

**Missing (the whole task):**
- `etchy-cli` has **no `etchy-pdf` dependency, no `pdf` feature, and no `.pdf` detection**. Its pipeline is Gerber/Excellon/placement-only and everything funnels through `board_from_files()` → `(Board, GerberFormat)` → `DiffReport`. PDF produces none of those types, so it needs a **parallel branch**, not a slot in the existing path.
- No PDF test fixtures exist anywhere; the engine tests synthesise PDFs in code.
- Docs still say "pdfium" in three places (stale — it's hayro).

## 2. Scope for v0.1.0

- **CLI-first.** `etchy old.pdf new.pdf` → page-by-page pixel diff → summary + JSON + per-page overlay PNGs → exit 0/1/2. Behind the `pdf` cargo feature.
- **GUI: deferred (post-1.0).** No PDF viewing in the egui shell for v0.1.0 — the wasm/GUI build already excludes `etchy-pdf` by omission. (Decision Q6.)
- **Same-format inputs only.** Both inputs must be PDF; mixing `old.pdf` with a gerber folder is a loud error, not a guess.

## 3. Design

### 3.1 Input detection & dispatch
- New early branch in `run()` (`crates/etchy-cli/src/main.rs`, ~line 267, before the git/dir board-loading): if **both** positional inputs are PDFs — by `.pdf` extension **and** `%PDF` magic (first bytes) — dispatch to a new `run_pdf(&cli)`. If exactly one is a PDF → loud "both inputs must be PDF" error (exit 2).
- Behind `#[cfg(feature = "pdf")]`. When the binary is built without `pdf` and a `.pdf` is passed, print a clear "this build lacks PDF support — rebuild with `--features pdf`" and exit 2 (mirror `etchy_pdf::available()`), rather than a confusing "not a gerber" skip.

### 3.2 Output shape — a parallel `PdfReport` (not `DiffReport`)
PDF has no layers/mm²/copper regions, so it cannot reuse `DiffReport`. Add a small,
separately schema-versioned report in `etchy-pdf` (or `etchy-cli`):

```
PdfReport {
  schema_version: u32,        // pdf schema v1, independent of the gerber JSON v1
  tool_version: String,
  any_changes: bool,
  dpi: f32,
  old_pages: usize, new_pages: usize,
  pages: [ PdfPageReport {
    page: usize,              // 1-based
    present: "both" | "old-only" | "new-only",
    added_px, removed_px, changed_px, total_px: u64,
    changed_fraction: f64,
    regions: u32,
    overlay_png: Option<String>,   // written filename, when --out given
  } ],
}
```
- Reuse the existing `--format {summary,json,md}` flag. `summary` = a per-page table + totals + a `result:` line; `json` = `PdfReport`; `md` = a Markdown table (for the PR-comment path, CLI-8 later).
- **Overlays are PNG, not SVG.** Written one-per-page via `encode_png` into an output directory (Decision Q3 on the flag). SVG/`--html` don't apply to raster PDF diffs (v0.1.0).

### 3.3 Exit codes & gating
- Exit contract unchanged: `PdfError` → 2; else `PdfDiff::any_changes()` (page-count change **or** any page with `changed_fraction > 0`) → 1, else 0.
- The copper gates `--fail-on-area` / `--fail-on-regions` / `--gate-layers` are mm²/layer concepts — **not applicable to pixels**. For v0.1.0: passing them alongside a PDF input is a loud "not valid for PDF" error (don't silently ignore). A pixel-fraction gate (`--fail-on-changed-fraction <f>`) is a clean later addition (Decision Q2).

### 3.4 Page-count mismatch (a trust concern)
- Surface prominently in every format: "old 4 page(s), new 5 — 4 sheet(s) paired and diffed; **page 5 is new-only**".
- Treat any old-only/new-only page as a **change** for the exit code (a page appearing/disappearing is a diff). `any_changes()` already returns true when page counts differ — good; the summary must make it legible.

**Superseded 2026-07-13 (#249):** pages no longer pair by index. Each rasterized
page is fingerprinted (`etchy_core::pagealign`, a 16×16 **ink-coverage** digest of
the raster already rendered) and the two sequences are sequence-aligned, so a sheet
inserted or removed mid-document is an explicit row and the sheets around it keep
pairing with themselves. The chosen alignment is reported in every format unless
it is the identity. Invariants: the fingerprint decides only *which* pages pair
(every pair still gets the full pixel diff), every page appears exactly once, and
the alignment is deterministic.

The digest must be **local and absolute** — each cell records the fraction of
itself darker than a fixed luminance, quantized on a log ladder, and two digests
are compared by L1 distance over those per-cell values. A first attempt compared
each cell against the *page mean*; on sparse line art (i.e. every schematic sheet)
adding one small part lowered the mean and flipped dozens of untouched cells, so
an edited sheet scored further from itself than from a different sheet — the
alignment then unpaired it and its real change was never located. Any digest here
must satisfy: ink added or removed anywhere changes the digest only near that ink.

Two ceilings sit on top of it:
- **Confidence net.** A re-pairing is adopted only when it beats plain index
  pairing by `ALIGN_MARGIN` *and* every pair it chooses is more alike than chance.
  Otherwise the alignment falls back to index pairing and says so ("page alignment
  was ambiguous, paired by index"). Index pairing is the well-understood baseline;
  content alignment must be a strict improvement on it, never a regression.
- **Page-count cap.** The alignment is a DP matrix quadratic in the page count, so
  `MAX_ALIGN_PAGES` (1024 per side, ≈ 8.4 MB of matrix — safe on 32-bit wasm too)
  is checked *before* allocating and fails loud, naming the input and the limit.
  Two 10 000-page PDFs, small files under every other cap, otherwise asked for
  800 MB and aborted the process with no message at all.

### 3.5 DPI
- `--dpi <f32>` (default `DEFAULT_DPI` = 150). Higher DPI = crisper diff but more memory/time. Note a **DoS/size ceiling**: a large schematic at high DPI is a big raster — cap the rendered pixel area (or DPI) and fail loud if exceeded, consistent with the gerber per-file caps (CORE-7). (Decision Q5 — cap value.)

## 4. Implementation steps (TDD, each verified)

1. **CLI feature wiring.** `crates/etchy-cli/Cargo.toml`: add `etchy-pdf = { path = "../etchy-pdf", optional = true }` and `[features] pdf = ["dep:etchy-pdf", "etchy-pdf/pdf"]`. Confirm a default `cargo build -p etchy-cli` still excludes hayro.
2. **`PdfReport` type + serialization** (pure, TDD): construct from a `PdfDiff`, incl. `present` classification and 1-based page numbers; JSON/summary/md renderers. Unit-test the mapping (identical, changed, page-added, page-removed).
3. **`run_pdf()` + detection** in `main.rs`: `%PDF`-magic + extension sniff, both-must-be-PDF guard, the `#[cfg]` + not-built error, read files (reuse the `MAX_LAYER_FILE_BYTES` read guard), call `diff_pdfs`, render output, write per-page PNGs, map exit codes.
4. **`--dpi` + output-dir flag** (Q3) + the "gates not valid for PDF" guard.
5. **Fixtures + CLI integration test.** Commit a tiny **non-confidential** schematic PDF pair (a KiCad demo schematic exported to PDF, one small edit between them) under `corpus/pdf/`. Integration test: run the CLI on the pair, assert exit 1 + expected page/pixel deltas + PNG written; run identical-vs-identical → exit 0. (Decision Q4 — fixture source.)
6. **Docs.** Update `REQUIREMENTS.md` CLI-6 → ✅, `ROADMAP.md` M3; **fix the stale "pdfium" → "hayro"** in `DEVELOPER_GUIDE.md:31`, `README.md:104`, `CLAUDE.md:42`.
7. **Local-verify** (CI billing-blocked): `cargo test -p etchy-pdf --features pdf`, `cargo test -p etchy-cli --features pdf`, `cargo clippy` (both), `cargo fmt --check`, `cargo deny check licenses` (hayro tree), and confirm the **default** (no-pdf) build + the wasm GUI build are untouched.

## 5. Release packaging (Decision Q1 — the big one)
`etchy-pdf` needs **rustc 1.85** (workspace MSRV is 1.75; the crate carries its own
`rust-version`). Enabling `pdf` in the shipped release raises the effective
toolchain floor for the release build and pulls the hayro tree into the binary/
container. Options:
- **(A, recommended)** Ship `pdf` **on** in the release binaries + distroless container — it's a documented supported input (SCOPE-2), and 1.85 is old enough to be a non-issue for a from-source builder. Keep the *library* MSRV at 1.75 for `etchy-core`/`etchy-cli` default; only the pdf-enabled build needs 1.85.
- **(B)** Ship a separate `etchy` (no pdf) + `etchy-pdf`-enabled artifact. More release complexity; avoid unless the MSRV bump is a hard objection.

## 6. Risks
- **Real-world PDF variety.** The engine is tested only on synthetic single-square PDFs. Real KiCad/Altium schematic exports (fonts, vector strokes, embedded rasters, multiple page sizes) may expose hayro gaps or rasterisation differences. Mitigate with a real fixture (step 5) and a "couldn't rasterise → fail loud" path (TRUST-4).
- **Same-size requirement.** `diff_images` fails on size mismatch; two PDFs whose same-index pages differ in point-size (e.g. a page resized A4→A3) would error. Decide: fail loud (trust) vs letterbox/scale to match. Recommend **fail loud** for v0.1.0 with a clear message (a resized sheet is itself a meaningful change to flag). **Settled 2026-07-13 (#262):** a *genuine* resize is reported as a fully-changed page (exit 1), never rescaled and never an error; a difference within `SIZE_TOLERANCE_PX` is rasterization rounding, so both sides are cropped to their shared region and diffed normally — see §7.7.
- **Memory at high DPI** — the cap in §3.5.

## 7. Decisions — LOCKED (owner, 2026-07-12)
1. **Release packaging:** ship `pdf` **ON** in the release binaries + container (release build MSRV becomes 1.85; library default stays 1.75).
2. **Gate semantics:** any-change → exit 1. The deliverable is the **viewable diff file** — the tool's job is surfacing that a change exists and making it easy to see. A `--fail-on-changed-fraction` flag can come later.
3. **Overlay output:** new **`--out DIR`** for the per-page PNGs.
4. **Fixture:** a **KiCad demo schematic** exported to PDF twice (one small edit) via the installed KiCad — public-safe, reproducible.
5. **DPI:** **one DPI for all sheet sizes** ("same detail everywhere") — text renders equally crisp; larger sheets naturally produce more pixels. Default 150; `--dpi` overrides; ~50 MP/page hard cap fails loud.
6. **GUI:** **full modes** (Old/New/Overlay/Split/Swipe on raster pages), **native AND wasm from day one**. This makes a **hayro-on-wasm spike a prerequisite** — prove `etchy-pdf` compiles + renders on wasm32 before the GUI build; if wasm is blocked, come back with findings before descoping.
7. **Page-size mismatch:** fail loud — a resized sheet is itself a change to flag.
   **Revised 2026-07-13 (owner, #262):** a resized sheet is a **diff, not an
   error** — reported as a fully-changed page with both sizes named, exit 1. The
   pair is not pixel-diffed (no pixel correspondence) and has no overlay; the
   viewer shows both sheets at true scale. Exit 2 stays for unrenderable input,
   because CI treats it as infrastructure failure rather than a review gate.

   "Resized" means resized *beyond rasterization rounding*. Flooring
   `points × dpi / 72` to whole pixels puts two exports of the same paper up to a
   pixel apart per axis (`MediaBox [0 0 842 595]` → 1754×1239 px at 150 DPI; the
   exact `[0 0 841.89 595.276]` → 1753×1240 px), and treating that as a resize
   discards the sheet's whole pixel diff. Within `SIZE_TOLERANCE_PX` (2 px per
   axis) it is the **same** sheet: both rasters are cropped to the region they
   share, the pair is diffed normally, and the crop is reported in every format.

## 8. Execution order
1. **Spike (prerequisite):** hayro on wasm32 + a real KiCad schematic PDF render (the two flagged risks) + generate the fixture pair.
2. **CLI wiring** per §4 (its own PR).
3. **GUI full modes** (its own PR, on the CLI one): PDF pair loads into the viewer; pages listed in the Layers panel (one row per page); the mode segment drives Old/New/Overlay/Split/Swipe over the rasterized pages; measure/grid disabled or adapted for raster space (pixels→mm via DPI is known, so measuring can stay in mm).

