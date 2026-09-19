# etchy demo — dev setup & feedback runbook

How any developer gets the **hosted web demo + feedback widget** running on a new
machine, and how tester feedback is preserved during development. The demo is the
WASM build of `etchy-gui` served over a LAN so others can try the diff viewer in a
browser and submit structured feedback.

> **TL;DR:** on WSL/Linux run **`deploy/setup.sh`** — it installs what's missing,
> builds, stages, and serves.

---

## 1. Prerequisites

| Need | Why | Install |
|---|---|---|
| **Rust + rustup** | build the WASM bundle | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **`wasm32-unknown-unknown`** target | compile to WebAssembly | `rustup target add wasm32-unknown-unknown` *(setup.sh auto-adds)* |
| **trunk** | WASM bundler for `etchy-gui` | `cargo install trunk --locked` *(setup.sh auto-installs)* |
| **Python 3** | the static + `/feedback` server | usually present; else your package manager |

`setup.sh` checks all of these and installs the two cheap ones automatically
(pass `--no-install` to only report).

## 2. One-command path (WSL/Linux — build + serve)

```bash
deploy/setup.sh                 # toolchain check -> build -> stage -> serve on :8080
deploy/setup.sh --port 9000     # different port
deploy/setup.sh --build-only    # build + stage into deploy/.serve, don't serve
deploy/setup.sh --serve-only    # serve an already-staged bundle
```

It builds `crates/etchy-gui` with `trunk build --release --filehash false`
(stable filenames so `index.html` refs never change), stages the bundle +
`index.html` + `etchy-server.py` into `deploy/.serve/` (gitignored), then serves
on `0.0.0.0:<port>` and prints the LAN URL. Stop with Ctrl+C.

## 3. Serving from Windows (build on WSL, serve on Windows)

Build/stage once on WSL (`deploy/setup.sh --build-only`), then on Windows:

```powershell
deploy\setup.ps1 -Port 8080
```

(or, raw, from the staged dir: `python etchy-server.py 8080 .`, which binds to
loopback by default; set `$env:ETCHY_BIND = '0.0.0.0'` first to expose it).
LAN-reachable at `http://<host-ip>:8080/`. No auth — trusted LAN only; exposure is
the owner's call.

## 4. Manual build (if you'd rather not use the script)

```bash
cd crates/etchy-gui && trunk build --release --filehash false   # -> dist/
# copy next to each other into a serve dir:
#   dist/etchy-gui.js, dist/etchy-gui_bg.wasm,
#   deploy/demo/index.html, deploy/demo/etchy-server.py
ETCHY_FEEDBACK=../../deploy/feedback/$(hostname).jsonl \
  python3 etchy-server.py 8080 <serve-dir>
```

The raw command binds to loopback by default. Prefix it with
`ETCHY_BIND=0.0.0.0` to expose it to the LAN.

Bump the `?v=` query in `index.html` when the bundle changes (busts browser cache).

## 5. Feedback — collected & preserved

See [`feedback/README.md`](feedback/README.md) for the local-storage, triage, and safe-sharing rules.
