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
- **PDF path (`etchy-pdf`, pdfium-render — M3):** a prebuilt `libpdfium.a` per
  target (vendored from bblanchon/paulocoutinhox); a C++ toolchain for static
  linking. Feature-gated, so it doesn't affect the core build.

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

See [`docs/ROADMAP.md`](docs/ROADMAP.md) for what to build next and
[`docs/DEVELOPER_GUIDE.md`](docs/DEVELOPER_GUIDE.md) for the architecture +
verified crate stack.
