# M1 demo — feedback triage (18 Jun 2026)

First round of hosted-demo feedback (6 submissions, ~3 testers). Raw records:
`deploy/feedback/m1-seed.jsonl`. Re-run triage with
`python3 deploy/collect-feedback.py`.

## Themes (highest leverage first)

### 1. Layer sidebar UI — needs a redesign  *(the only Blocking item)*
- Icons read as meaningless "square boxes + two circles". *(#2, Blocking)*
- The mm² area data clutters the sidebar — move it onto the diff overlay instead. *(#2)*
- Want a per-layer change **%** in the sidebar, with detailed info on selection. *(#6, idea)*

### 2. Contrast — and a likely bug
- Red/green **overlap** is hard to see; needs a distinct overlap colour. *(#3, Annoying)*
- ⚠ "**Some traces missing on layers**" — triage as a possible rendering/diff
  correctness bug, independent of the colour fix. *(#3)*

### 3. Controls
- Mode buttons (overlay / before / after) are **too small to click**. *(#1, Annoying)*
- Want **keyboard shortcuts** for overlay/before/after — for driving a big shared
  screen with several people. *(#5, idea)*

### 4. Feature request
- Ingest **drill file + pick-and-place file** (diff more than copper). *(#4, idea)*
  Note: native CAD/extra-layer ingestion is currently a stated non-goal — flag for
  roadmap discussion rather than silent scope creep.

## Suggested priority
1. Redesign the layer sidebar (icons, move mm², add % ) — biggest win, Blocking.
2. Investigate "missing traces" — potential correctness bug.
3. Distinct overlap colour; larger hit-targets + keyboard shortcuts for modes.
4. Roadmap call on drill / pick-and-place ingestion.
