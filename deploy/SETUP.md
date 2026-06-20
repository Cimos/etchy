# etchy demo — dev setup & feedback runbook

How any developer gets the **hosted web demo + feedback widget** running on a new
machine, and how tester feedback is preserved during development. The demo is the
WASM build of `etchy-gui` served over a LAN so others can try the diff viewer in a
browser and submit structured feedback.

> **TL;DR:** on WSL/Linux run **`deploy/setup.sh`** — it installs what's missing,
> builds, stages, and serves. Feedback lands in `deploy/feedback/<hostname>.jsonl`;
> commit it. Triage with `python3 deploy/collect-feedback.py`.

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

(or, raw, from the staged dir: `python etchy-server.py 8080 .`). LAN-reachable at
`http://<host-ip>:8080/`. No auth — trusted LAN only; exposure is the owner's call.

## 4. Manual build (if you'd rather not use the script)

```bash
cd crates/etchy-gui && trunk build --release --filehash false   # -> dist/
# copy next to each other into a serve dir:
#   dist/etchy-gui.js, dist/etchy-gui_bg.wasm,
#   deploy/demo/index.html, deploy/demo/etchy-server.py
ETCHY_FEEDBACK=../../deploy/feedback/$(hostname).jsonl \
  python3 etchy-server.py 8080 <serve-dir>
```

Bump the `?v=` query in `index.html` when the bundle changes (busts browser cache).

## 5. Feedback — collected & preserved

- The widget POSTs to `/feedback`; the server appends one JSON line per submission
  (server ts + client IP + UA + payload: sentiment / category / severity / name /
  text / `ctx` = screen/viewport/url/app-state).
- `setup.sh`/`setup.ps1` set **`$ETCHY_FEEDBACK`** so writes go straight into the
  repo at **`deploy/feedback/<hostname>.jsonl`** — one file per host, so two
  machines never clash. **Commit your file** to share feedback with the team.
- Triage everything: `python3 deploy/collect-feedback.py` (summary + records);
  `--merged` emits combined JSONL.

> ⚠ Feedback records contain IP/UA/names (PII), kept on purpose during dev. They
> **must be scrubbed before the repo goes public** — see [`/PRE_PUBLIC.md`](../PRE_PUBLIC.md).
