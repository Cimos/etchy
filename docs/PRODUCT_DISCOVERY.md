# gerber-diff — product discovery

Living capture of the assume-nothing requirements interview (Simon × Claude),
the basis for `ROADMAP.md` and `DEVELOPER_GUIDE.md`. Answers are authoritative;
where they contradict the current implementation, the answer wins and the gap
becomes roadmap work.

Started: 2026-06-15. Current shipped version at interview start: **v0.11.0**.

---

## Round 1 — Foundations

**Primary audience: Open-source community.**
Public adoption is the goal → broad format support, strong docs/onboarding,
cross-platform, low-friction install (think `pip`/`pipx`/package managers, not
only a Windows installer) all become important. Implication: the current
Windows-installer-first posture is now secondary.

**Core job to nail: Fast "what changed?".**
A quick, clear visual answer between two revisions with minimal ceremony. Speed
and clarity rank above rigor/formality. Implication: keep ceremony low; don't
over-build review/sign-off workflow at the expense of speed.

**Diff depth (first-class): Visual/pixel + Geometric/vector.**
- Visual/pixel = today's raster overlay (keep).
- Geometric/vector = NEW: true shape-level diff (added/removed/moved features as
  vectors with coordinates & sizes), not just pixels. Major new capability.
- Explicitly NOT first-class (for now): net/connectivity diff, component/BOM diff.
  Bounds scope — guard against creep into a full ECAD-review tool.

**Primary platform: CLI / CI-first, with cross-platform desktop a close second.**
- Headless automation (gate PRs, pipelines) is the primary surface.
- Cross-platform desktop (Win/mac/Linux) is a strong second — the GUI stays, but
  must run cross-platform, not Windows-only.
- Implication: invest in CLI ergonomics, machine-readable output, CI integration,
  and cross-platform packaging; Windows-exe is no longer the headline.

---
## Round 2 — Comparison semantics, formats, CI

**Geometry source: leaning "Both" (Gerber universal + native CAD when present) — pending deeper discussion (Round 3/4).**

**Geometric diff outputs (first-class): Crisp vector overlay + Change heatmap/regions.**
NOT prioritized as user-facing output: structured change-lists, quantified deltas.
→ Visual-first, "where to look" emphasis. (But see measurement tension below.)

**Input formats (all first-class): Gerber+Excellon, KiCad native (.kicad_pcb), Schematic PDF, ODB++/IPC-2581.**
Broad-format ambition. Implies an architecture that doesn't write a diff engine
per format.

**CI surface (all matter): thresholds & exit codes, PR comments w/ visuals, stable JSON contract, multi-CI + offline.**

## Round 3 — Architecture direction + workflow

**Invocation (all four first-class): CLI two paths/zips · CLI two git refs · desktop pick/drag · CI on every PR.**
Note: "CLI two git refs" + "KiCad first-class" + open-source ⇒ the canonical
workflow "diff my .kicad_pcb across commits" is important; KiCad repos commit
source, not Gerbers — bridged via `kicad-cli` plotting (see Round 4 discussion).

**Alignment: same-board revisions only.** Assume a shared coordinate frame; detect
& clearly error on mismatched boards. NO auto-align magic. Big simplification:
Gerber coords are absolute, so vector diff needs no registration.

**To discuss (deferred to Round 4):** geometry-source phasing; how/what to measure
for CI thresholds vs visual-first outputs.

### Round 4 working theses (expert recommendation, to confirm)
- **One diff engine, many front-ends.** Gerber/Excellon polygon-diff is the
  universal engine (overlay = difference polygons → SVG; heatmap = clustered
  difference regions; magnitude = polygon area — all from one computation).
  "Format support" = front-ends that normalize into the engine's geometry model
  (IR), NOT separate diff engines.
- **KiCad git-refs via `kicad-cli` plot** at each ref → Gerbers → engine. Serves
  the dominant open-source workflow without a bespoke .kicad_pcb geometry parser.
- **Native object-level parsing (identity, components, nets) deferred** — it only
  powers the outputs not prioritized (structured lists). Revisit later.
- **Measure-internally reconciliation:** the polygon-diff yields changed area
  (mm²) + region count for free → GUI shows visual, CLI/JSON/CI expose the
  numbers; thresholds gate on area / region-count, scopable per layer.

## Round 4 — Architecture & implementation foundations

**SCOPE NARROWED (supersedes Round 2 "all formats first-class"): Pure Gerber/Excellon + schematic PDF.**
KiCad-native, IPC-2581, ODB++ ingestion are DROPPED from near/mid-term scope. Be
excellent at the universal fab formats + PDF. (`kicad-cli`-plot bridging may
return later as a convenience, but is not core.) This also right-sizes the
"two git refs" workflow to Gerber-in-git (+ optional later plot bridge).

**Measurement model: CONFIRMED.** One polygon diff → overlay + heatmap + magnitude
(changed area mm², region count). GUI shows visual; CLI/JSON/CI expose numbers;
thresholds gate on area / region-count, scopable per layer.

**Distribution: standalone per-OS binaries + a container image.** NOT pip/pipx,
NOT OS package managers. Implication: self-contained artifacts are the product;
the Python-package path is de-prioritized (notable — interacts with language).

**Implementation language: OPEN — discussing rewrite-vs-iterate (Round 5).**
Key reframe: polygon booleans in Python run in C-backed libs (pyclipper/shapely),
so raw compute is NOT the deciding factor — distribution artifacts (binary size,
cold-start, container size) are, and that's where Python is weakest and the
binaries+container choice bites.

## Round 5 — GUI, scale, appetite

**GUI: secondary, minimal upkeep.** CLI/CI is the product; keep a GUI working but
don't pour effort in.
**Scale target: dense / many-layer.** 8–16+ layers, large dense pours, fine pitch
must stay smooth — the heavy-polygon-ops case (informs the language decision).
**Appetite: Ambitious v2.** Major version; bigger architectural moves are on the
table; longer runway acceptable.

## Round 6 — Language, trust, scope, version (the v2 decision)

**Language: full RUST rewrite (big-bang).** Decided over the strangler suggestion.
v2 is a ground-up Rust implementation. Implications: best artifacts (tiny static
binaries + distroless container, matching the distribution choice), best fit for
dense-board performance + CLI/CI-first; highest risk + longest time-to-first
value (the roadmap must manage this — MVP slice early, golden corpus from day 1,
keep v0.11 Python available so users have no capability gap during the rewrite).

**Trust bar: must be trustworthy — no silent misses.** A missed change is the
worst outcome. Golden-test corpus, validated polygon ops, fail-loud ("couldn't
process this layer") over wrong-but-quiet. This is a top-tier requirement
despite "fast" being the core job.

**Confirmed NON-goals for v2:** net/connectivity diff, BOM/component diff,
DRC/rule-checking. **Schematic-PDF pixel diff is KEPT** as a supported feature
(separate from the geometric engine).

**Name: RENAME (broader scope).** The tool is now a PCB visual + geometric diff
engine (Gerber/Excellon + PDF), not just "gerber". New name to be chosen
(Round 7); verify availability on crates.io / GitHub / PyPI / domain before
committing.

### Resolved v2 shape so far
- Ground-up **Rust**; CLI/CI-first; cross-platform static binaries + container.
- Engine: Gerber/Excellon → per-layer polygons → boolean diff → **crisp SVG
  overlay + change heatmap + magnitudes (area, region count)**; same-board
  revisions only (fail clearly on mismatch).
- Keep **schematic-PDF pixel diff**. CI: thresholds on area/region-count
  (per-layer scopable), exit codes, stable JSON, PR comments w/ visuals, offline.
- GUI secondary. Trustworthy-first (golden corpus, fail-loud). Non-goals:
  net/BOM/DRC.

## Round 7 — Transition, GUI form, output

**Name: needs catchier options** (descriptive fabdiff/boardiff rejected; brainstorm
brandable names — Round 8).
**v0.11 Python: FREEZE as-is.** No bugfixes, no features; clean break, all effort
to Rust. (Roadmap note: users stay on frozen v0.11 until the Rust MVP is usable —
so the MVP must reach core parity reasonably quickly to avoid a long gap.)
**v2 GUI: native Rust (egui/iced).** Not browser-only — a single-binary native
viewer. Still secondary in priority (engine/CLI first), but the intended GUI is
native, not a Tk shell or web app.
**Outputs: HTML + SVG + JSON all first-class** (equal weight).

## Round 8–9 — Name, license, MVP, corpus, repo (finalized)

- **Name: `etchy`.** (`etch` is taken on crates.io — a text formatter, 480k dl;
  `etchr`/`sketch` also taken. `etchy` verified free on crates.io.) Reads as
  `etchy old new`. GitHub `Cimos/etchy` (Simon's namespace, free).
- **License: dual `MIT OR Apache-2.0`** (Rust-ecosystem convention; adds patent grant).
- **Milestone-1 MVP includes the egui GUI** — engine + CLI + native viewer + the
  HTML/SVG/JSON outputs all in the first usable cut (bigger M1, but the GUI ships
  with it rather than later).
- **Trust corpus = all sources:** synthesized known-delta board pairs (ground
  truth), public KiCad demo boards (realism), Simon's own non-confidential
  boards, plus fuzzing/property tests (invariants: diff(A,A)=∅, symmetry, etc.).
- **Repo: fresh `Cimos/etchy`.** Clean start; `Cimos/Gerber-Diff-Tool` stays
  frozen at v0.11 with its existing releases.

---

## CONSOLIDATED v2 SPEC (the brief for ROADMAP.md + DEVELOPER_GUIDE.md)

**etchy** — a fast, trustworthy, open-source PCB **visual + geometric diff** tool.

- **Language/impl:** ground-up **Rust** (big-bang rewrite). Fresh repo. Dual MIT/Apache-2.0.
- **Audience:** open-source community. **Core job:** fast "what changed?" between
  two revisions. **Surfaces:** CLI/CI-first; native egui GUI second; both M1.
- **Inputs (scope):** Gerber RS-274X/X2 + Excellon drill (universal fab core) and
  schematic **PDF** (pixel page-diff, kept). NON-goals: KiCad/IPC/ODB++ ingestion,
  net/connectivity diff, BOM/component diff, DRC.
- **Diff model:** per-layer polygons from Gerber → boolean diff (`added=B−A`,
  `removed=A−B`). **Same-board revisions only** (absolute coords, no registration;
  fail clearly on mismatch). One computation → three views:
  - **Crisp SVG vector overlay** (zoom-forever),
  - **change heatmap / ranked regions** (where to look),
  - **magnitudes**: changed area (mm²), region count (for CI thresholds).
- **Outputs (all first-class):** self-contained HTML report, standalone SVG/PNG,
  machine-readable JSON (stable, versioned schema).
- **CI:** exit codes + per-layer thresholds (area / region-count), PR comments
  with visuals (GitHub + ideally GitLab), fully offline/self-hosted.
- **Distribution:** per-OS standalone static binaries + a distroless container.
  NOT pip, NOT OS package managers.
- **Scale target:** dense / many-layer (8–16+ layers, large pours, fine pitch)
  must stay smooth.
- **Trust bar (top-tier):** no silent misses — golden-test corpus (synthesized +
  real), validated polygon ops, fail-loud over wrong-but-quiet; fuzz/property tests.
- **GUI:** native egui/iced single-binary viewer (overlay / before / after /
  split / swipe / onion + heatmap, pan-zoom). Secondary priority but in M1.

