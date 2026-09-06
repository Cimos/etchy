# deploy/ — the hosted web demo

The web build of `etchy-gui` (WASM) served on a LAN so others can try the diff
viewer in a browser. This folder is the **page + server**, version-controlled so
they can't be lost; the WASM/JS bundle and the demo board are build artifacts and
are **not** committed.

> **New machine?** Use the runbook + one-command setup: [`SETUP.md`](SETUP.md)
> (`deploy/setup.sh` builds, stages, and serves). Feedback is collected in
> [`feedback/`](feedback/); triage with `python3 deploy/collect-feedback.py`.

## Contents

- `demo/index.html` — the curated page served to users. Loads the WASM with a
  **streaming progress bar** (download → indeterminate "starting engine" during
  compile), and overlays the in-app **feedback widget** (sentiment love/issue/idea,
  category, severity, optional name, auto-context incl. `window.__etchyState` =
  layer/mode/zoom from the Rust hook). References `/etchy-gui.js` +
  `/etchy-gui_bg.wasm` with a `?v=` cache-bust.
- `demo/etchy-server.py` — static file server **+ `POST /feedback`** → appends one
  JSON line per submission to `feedback.jsonl` (server ts + IP + UA + payload).
  Refuses to serve the feedback file, the `screenshots/` dir beside it, itself,
  or any directory listing (#334).

## Build + deploy (current host: Windows, served from `C:\Users\<user>\etchy-demo\`)

```sh
# 1. Build the web bundle with STABLE filenames (so index.html refs never change):
trunk build --release --filehash false   # in crates/etchy-gui  -> dist/

# 2. Copy artifacts + this folder's files next to each other in the serve dir:
#    dist/etchy-gui.js, dist/etchy-gui_bg.wasm, deploy/demo/index.html, deploy/demo/etchy-server.py
#    (bump the ?v= in index.html when the bundle changes, to bust browser caches)
```

## Launch / stop (LAN exposure — owner's explicit call)

```powershell
# launch (detached, hidden, no console window):
Start-Process -WindowStyle Hidden -FilePath python.exe -ArgumentList `
  'etchy-server.py','8080','<serve-dir>'
# stop: kill whatever listens on 8080
Get-NetTCPConnection -LocalPort 8080 -State Listen | %{ Stop-Process -Id $_.OwningProcess -Force }
```

Reachable at `http://<host-LAN-ip>:8080/`. No auth (trusted LAN). The feedback
file lives next to the bundle on the host; read it to review submissions.
