#!/usr/bin/env bash
# etchy demo — one-command dev setup: toolchain -> build (WASM) -> stage -> serve.
#
# Onboards a new dev machine (WSL/Linux) end to end and launches the hosted demo
# with the feedback widget, writing feedback straight into the repo at
# deploy/feedback/<hostname>.jsonl (preserved + collectable).
#
# Usage:
#   deploy/setup.sh [--port N] [--serve-dir DIR] [--build-only] [--serve-only]
#                   [--no-install] [-h|--help]
#
#   --port N        port to serve on (default 8080)
#   --serve-dir DIR where to stage the bundle (default: deploy/.serve, gitignored)
#   --build-only    build + stage, do not launch the server
#   --serve-only    skip build; serve an already-staged --serve-dir
#   --no-install    do not auto-install missing toolchain bits (just report)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT=8080
SERVE_DIR="$REPO/deploy/.serve"
BUILD=1; SERVE=1; INSTALL=1

while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2;;
    --serve-dir) SERVE_DIR="$2"; shift 2;;
    --build-only) SERVE=0; shift;;
    --serve-only) BUILD=0; shift;;
    --no-install) INSTALL=0; shift;;
    -h|--help) sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

say() { printf '\033[36m• %s\033[0m\n' "$*"; }
die() { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

check_toolchain() {
  have python3 || die "python3 not found (needed to serve). Install it and re-run."
  if [ "$BUILD" = 1 ]; then
    if ! have cargo || ! have rustup; then
      die "Rust toolchain not found. Install rustup first:
       curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
     then re-run this script."
    fi
    if ! rustup target list --installed 2>/dev/null | grep -q '^wasm32-unknown-unknown$'; then
      if [ "$INSTALL" = 1 ]; then say "adding wasm32-unknown-unknown target…"; rustup target add wasm32-unknown-unknown
      else die "missing target wasm32-unknown-unknown (run: rustup target add wasm32-unknown-unknown)"; fi
    fi
    if ! have trunk; then
      if [ "$INSTALL" = 1 ]; then say "installing trunk (one-time, builds from source)…"; cargo install trunk --locked
      else die "trunk not found (run: cargo install trunk --locked)"; fi
    fi
  fi
}

build() {
  say "building etchy-gui for the browser (WASM, stable filenames)…"
  ( cd "$REPO/crates/etchy-gui" && trunk build --release --filehash false )
  local dist="$REPO/crates/etchy-gui/dist"
  [ -f "$dist/etchy-gui.js" ] && [ -f "$dist/etchy-gui_bg.wasm" ] \
    || die "build did not produce dist/etchy-gui.js + _bg.wasm in $dist"
  # Size-optimize the bundle ourselves (#99). trunk's own wasm-opt is disabled in
  # index.html because its bundled binaryen rejects Rust's bulk-memory output; we
  # run wasm-opt here with all features enabled instead. Best-effort: skip with a
  # note if binaryen isn't installed, so a plain dev build still works.
  if have wasm-opt; then
    say "optimizing wasm bundle (wasm-opt -Oz)…"
    wasm-opt -all -Oz --strip-debug "$dist/etchy-gui_bg.wasm" -o "$dist/etchy-gui_bg.wasm"
  else
    say "wasm-opt not found — shipping unoptimized bundle (install binaryen for a smaller one)"
  fi
  say "staging serve dir: $SERVE_DIR"
  mkdir -p "$SERVE_DIR"
  cp "$dist/etchy-gui.js" "$dist/etchy-gui_bg.wasm" "$SERVE_DIR/"
  cp "$REPO/deploy/demo/index.html" "$REPO/deploy/demo/etchy-server.py" "$SERVE_DIR/"
}

serve() {
  [ -f "$SERVE_DIR/etchy-server.py" ] || die "nothing staged in $SERVE_DIR (run without --serve-only first)"
  local fb="$REPO/deploy/feedback/$(hostname).jsonl"
  local ip; ip="$( (hostname -I 2>/dev/null || echo) | awk '{print $1}')"
  say "feedback -> $fb"
  say "serving on 0.0.0.0:$PORT  ->  http://${ip:-<this-host-ip>}:$PORT/"
  say "stop with Ctrl+C; commit deploy/feedback/$(hostname).jsonl to share feedback."
  ETCHY_FEEDBACK="$fb" exec python3 "$SERVE_DIR/etchy-server.py" "$PORT" "$SERVE_DIR"
}

check_toolchain
[ "$BUILD" = 1 ] && build
if [ "$SERVE" = 1 ]; then serve; else say "build-only: staged in $SERVE_DIR"; fi
