# Third-party licenses and bundled assets

etchy is licensed **MIT OR Apache-2.0** (see `LICENSE-MIT` and `LICENSE-APACHE`).
This file records third-party material redistributed in this repository or in
the binaries it produces.

## Rust dependencies

All crate dependencies resolve to permissive licenses (MIT, Apache-2.0,
BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0, OFL-1.1, Ubuntu-font-1.0).
This is enforced in CI by `cargo deny` against the allow-list in `deny.toml`;
copyleft licenses (GPL/AGPL/LGPL/MPL) are blocked. Run `cargo deny check
licenses` to reproduce.

## Bundled fonts

- **JetBrains Mono** (400/500/700) and **Zilla Slab** (700) — shipped as
  `.woff2` under `site/assets/fonts/`, each with its SIL Open Font License 1.1
  text (`OFL-JetBrainsMono.txt`, `OFL-ZillaSlab.txt`). Names unmodified.
- The native/wasm GUI embeds **eframe/egui**'s default fonts (Ubuntu-Light,
  Hack, Noto Emoji, emoji-icon-font), covered by OFL-1.1 / Ubuntu-font-1.0 via
  the upstream crate.

## Bundled sample data

- **Demo board** — `crates/etchy-gui/assets/demo/{old,new}/`: the author's own
  Mad_RP2040 fab pack (<https://github.com/Cimos/Mad_RP2040>), used with
  permission. See that directory's `README.md`.
- **Synthetic corpus** — `corpus/synthetic/`: generated test fixtures, etchy's
  own (see `corpus/tools/`).
- **Schematic PDF fixtures** — `corpus/pdf/{old,new}.pdf`: see
  `corpus/pdf/README.md` for provenance and the open licence question tracked
  in `PRE_PUBLIC.md`.

## Brand

`assets/brand/` and `site/assets/brand/` are etchy's own identity — no
third-party licence applies.
