# Native KiCad corpus

This directory holds provenance-tracked `.kicad_pcb` fixtures and pinned
manufacturing exports used as test oracles. `manifest.toml` is the inventory.
Its `fixtures` entries describe files that are present; `slots` describe known
gaps and must remain `status = "pending"` until the stated evidence exists.

## What is covered

The checked-in boards cover KiCad 7 syntax only. They retain the source demo's
header `(version 20221018) (generator pcbnew)`. They were made by reducing the
KiCad demo board installed at
`/usr/share/kicad/demos/test_pads_inside_pads/test_pads_inside_pads.kicad_pcb`
from Ubuntu package `kicad-demos 7.0.11+dfsg-1build4`. The source file's SHA-256
is `92ee3bb1660f7f877119850b749e00f11999faae401e2bbcddfdce1aede0694d`.
Ubuntu's package copyright file assigns `demos/*` GPL-2+.

These reduced files are not described as written or saved by KiCad. Each was
loaded and successfully exported by `kicad-cli 7.0.11`. They isolate track and
arc records, basic pad shapes, via spans, saved and absent zone fills, built-in
stroke text, renamed layers, and a bottom-side footprint. KiCad 6, 8 and 10
remain pending (KiCad 9 is covered by the Mad_RP2040 boards below) because no installed producer or licence-compatible public file
with sufficient producer evidence was available when this corpus was built.

## Mad_RP2040 boards

`mad_rp2040/old/` and `mad_rp2040/new/` hold `Mad_RP2040.kicad_pcb` copied
unchanged from the public MIT repository <https://github.com/Cimos/Mad_RP2040>
at tags `v0.0.0` (commit `650652bc4d2d4900496da44eaacbdd3164d45010`) and
`v0.0.1` (commit `e2c21c68a6bec878b70fa6fbbd1b0374dee4804a`). Both were saved by
KiCad 9.0 (header `version 20241229`, `generator_version "9.0"`), so they also
cover the KiCad 9 format.

They are the true sources of the bundled demo packs. Checked 2026-10-03: every
one of the 13 files in `crates/etchy-gui/assets/demo/old` is byte-identical,
apart from date lines, to the PCBWay pack inside the `v0.0.0` release's
`Mad_RP2040-v0.0.0-pcb-datapack.zip` (KiCad 9.0.7+1); `demo/new` matches the
`v0.0.1` release the same way. To repeat the check, download those release
assets, unzip `output_pcb/PCBWay/*_compress.zip`, and compare each file with
the demo pack ignoring lines containing `CreationDate` or `date 20`.

The demo packs are therefore the KiCad 9 oracle for these two boards. They were
made with the PCBWay plot profile, so native projection is expected to differ
where that profile applies plot settings (for example mask subtraction on silk);
each difference must be classified in `manifest.toml`.

## Regenerating the oracles

From this directory, run for example:

```sh
tools/export_oracle.sh fixtures/kicad7/track_arc.kicad_pcb oracles/track_arc
```

The script requires exactly `kicad-cli 7.0.11`, records that version, chooses
all declared manufacturing layers in a fixed order, and invokes:

- Gerber export with explicit layers and precision 6; X2 and netlist attributes
  remain enabled and saved board plot settings are not used.
- Excellon drill export in millimetres, absolute origin, decimal zeros,
  alternate oval representation, with plated and non-plated files separated.
- CSV position export for both sides in millimetres, with absolute coordinates.

KiCad embeds generation timestamps in Gerber output, so byte hashes of a
regenerated oracle pack are not expected to remain stable. Fixture hashes are
stable and enforced by the manifest check.

## Validation

Run:

```sh
python3 tools/test_manifest.py
```

The standard-library-only check verifies every present path and SHA-256, checks
the licence allow-list, requires provenance and oracle fields, and rejects a
pending slot that does not explain what evidence is missing. It is intentionally
not wired into CI in this change; it can be added as a single Python step when
native corpus jobs are introduced.

## Honesty rules

- A producer/version claim needs evidence in the file header or a producer
  record made during export.
- Editing a board derived from a producer file does not make it output written
  by that producer. The production note must describe the derivation.
- A release-family slot stays pending when the exact producer or a suitably
  licensed public fixture is unavailable.
- A revision match needs a commit and file hash or reproducible output match.
  Similar dates, names or geometry are not enough.
- `differences` starts empty. Later projection work must classify every observed
  native-to-oracle mismatch instead of silently accepting it.
