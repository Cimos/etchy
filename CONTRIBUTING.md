# Contributing / Development setup

etchy is Rust. The recommended dev environment is **Linux or WSL2** (Rust builds
are fastest there and the CI Linux target is the distribution target).

## WSL2 setup (recommended)

1. **Clone inside the WSL filesystem — NOT `/mnt/c`.** Building on the mounted
   Windows drive is slow (cross-filesystem I/O) and causes permission + line-ending
   problems.
   ```bash
   mkdir -p ~/git && cd ~/git
   git clone git@github.com:Cimos/etchy.git   # or https://github.com/Cimos/etchy.git
   cd etchy
   ```
2. **Install Rust** via rustup (the repo pins `stable` + rustfmt + clippy in
   `rust-toolchain.toml`, so the right toolchain is selected automatically):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source "$HOME/.cargo/env"
   ```
3. **Build + test:**
   ```bash
   cargo build --workspace
   cargo test  --workspace
   cargo run -p etchy-cli -- --version
   ```

## System packages (Linux/WSL), by milestone

The Phase-0 scaffold is **pure Rust — no system packages needed**. They come in
as real dependencies land:

- **GUI (`etchy-gui`, eframe/egui — M1):** X11/Wayland + GPU libs, e.g. on
  Debian/Ubuntu: `libxkbcommon-dev libwayland-dev libxcb1-dev libgl1-mesa-dev`.
  Under WSL2 on Windows 11, **WSLg** provides the display — the native viewer runs
  in WSL with no extra X server.
- **Test boards (Spike 1 / corpus):** `kicad-cli` to plot public KiCad demo boards
  to Gerbers, e.g. `sudo apt install kicad`, then
  `kicad-cli pcb export gerbers -o out/ board.kicad_pcb`.
- **PDF path (`etchy-pdf`, hayro):** **pure Rust — no system packages, no C++
  toolchain, no `libpdfium`.** Build it with `--features pdf` (release binaries
  and the container ship it on). It raises that crate's MSRV to 1.85; it's
  feature-gated, so the core build is unaffected.

## Conventions

- `etchy-core` is pure logic — no `println!`, no `process::exit`, no file-path
  policy. CLI/GUI own I/O and presentation.
- Geometry is **fixed-point** (deterministic diffs). See DEVELOPER_GUIDE.
- **No silent misses:** parsers/engine fail loud rather than emit a wrong-but-quiet
  diff. Add property tests (`diff(A,A)=∅`, symmetry) + fuzz the parsers.
- **Permissive-only deps** — `deny.toml` blocks GPL/AGPL/LGPL. Run `cargo deny check`
  before adding a dependency.
- Dual-licensed `MIT OR Apache-2.0`; contributions are under the same.

## Before pushing

```bash
cargo fmt --all
cargo clippy --workspace --all-targets   # tighten to -D warnings once deps land
cargo test  --workspace
```

## Cutting a release

A release *is* a pushed tag: `.github/workflows/release.yml` fires on `v*`, builds
the CLI for four platforms with `--features pdf`, and attaches the archives plus
`SHA256SUMS` to the GitHub Release. A tag with a `-` in it (`v0.1.0-rc1`) publishes
as a pre-release. Before pushing the tag:

1. **Cut the CHANGELOG section.** Move everything under `## [Unreleased]` in
   [`CHANGELOG.md`](CHANGELOG.md) into a new `## [X.Y.Z] — YYYY-MM-DD` heading,
   leave `[Unreleased]` empty for the next cycle, and update the compare links at
   the bottom of the file. A tag with no changelog section of its own is an
   unfinished release — this step is what keeps the two from drifting.
2. Bump `version` under `[workspace.package]` in the root `Cargo.toml` and commit
   the refreshed `Cargo.lock`.
3. Run the gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`,
   `cargo test --workspace`.
4. `git tag vX.Y.Z && git push origin vX.Y.Z`, then check the workflow attached
   all four archives and `SHA256SUMS`.
5. The install commands in `site/docs.html` name a concrete release — point them
   at the new one.

See [`docs/ROADMAP.md`](docs/ROADMAP.md) for what to build next and
[`docs/DEVELOPER_GUIDE.md`](docs/DEVELOPER_GUIDE.md) for the architecture +
verified crate stack.
