# PDF fixtures — schematic revision pair

A real KiCad schematic exported to PDF twice with one small edit between them,
for the `.pdf` CLI path (CLI-6) integration tests. Expected diff: page 1 has a
small changed region (the title-block text), all other content identical.

## How these were generated

Source: the **ecc83 demo project bundled with KiCad 7.0.11**
(`/usr/share/kicad/demos/ecc83/ecc83-pp.kicad_sch` on Ubuntu, package
`kicad-demos`), a single-page ECC83 push-pull amplifier schematic.

1. `old.pdf` — the schematic exported as-is:

   ```sh
   kicad-cli sch export pdf /usr/share/kicad/demos/ecc83/ecc83-pp.kicad_sch -o old.pdf
   ```

2. `new.pdf` — the title-block `title` field edited from empty to
   `"ECC Push-Pull rev B"` in a copy of the `.kicad_sch`, then exported the same
   way:

   ```sh
   kicad-cli sch export pdf ecc83-pp.kicad_sch -o new.pdf   # edited copy
   ```

Verified with hayro at the default 150 DPI: both render, and the diff finds the
title text as added pixels (3 regions, ~230 px) on page 1.

To regenerate (e.g. after a KiCad version bump changes the plot output), repeat
the two steps above — any single visible edit works, but keep it small so the
expected numbers in `crates/etchy-cli/tests/pdf_cli.rs` stay meaningful.

## Source and licence

These PDFs are derived from KiCad's bundled demo project. The Debian
`kicad-demos` copyright file marks `demos/*` as **GPL-2+** (copyright KiCad
Developers). etchy itself is MIT/Apache-2.0; these two files are **test fixtures
only** — they are not compiled into or linked with any etchy binary.

**Open licence question (for the owner's review):** whether shipping GPL-2+-derived
fixture PDFs in an MIT/Apache repo needs a per-file licence note or a
replacement fixture (e.g. a schematic drawn from scratch). Flagged in the PR
that added these files.
