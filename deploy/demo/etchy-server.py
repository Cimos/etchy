#!/usr/bin/env python3
"""etchy demo server: static files (GET) + POST /feedback -> feedback.jsonl.

Usage:  python etchy-server.py [PORT] [ROOT]
Serves ROOT (default: this script's dir) on 0.0.0.0:PORT (default 8080).
Feedback records are appended as one JSON object per line, stamped with the
server time, client IP and User-Agent. The feedback file, the screenshots/
directory beside it and this script are never served over GET or HEAD: every
request path is resolved to a real file first (percent-decoding, "." and "//"
included) and compared against those locations, and directory listings are
refused.

Feedback file location (in priority order):
  1. $ETCHY_FEEDBACK  — absolute/relative path (set by deploy/setup.sh to write
     straight into the repo at deploy/feedback/<hostname>.jsonl so feedback is
     preserved + collectable; the dir is created if missing).
  2. ROOT/feedback.jsonl  — default (next to the served bundle).
"""
import base64
import datetime
import json
import os
import sys
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
ROOT = sys.argv[2] if len(sys.argv) > 2 else os.path.dirname(os.path.abspath(__file__))
FEEDBACK = os.environ.get("ETCHY_FEEDBACK") or os.path.join(ROOT, "feedback.jsonl")
os.makedirs(os.path.dirname(os.path.abspath(FEEDBACK)), exist_ok=True)
# Pasted screenshots are decoded into a screenshots/ dir beside the feedback file.
SHOTS_DIR = os.path.join(os.path.dirname(os.path.abspath(FEEDBACK)), "screenshots")
SCRIPT = os.path.abspath(__file__)
# Never served, by basename, wherever a copy sits under ROOT (the staged serve dir
# holds a copy of this script; the feedback file may have been renamed).
BLOCKED_NAMES = {"feedback.jsonl", "etchy-server.py", os.path.basename(FEEDBACK)}
_lock = threading.Lock()


def _real(path):
    return os.path.normcase(os.path.realpath(path))


def _inside(path, directory):
    try:
        return os.path.commonpath([path, directory]) == directory
    except ValueError:  # different drives on Windows
        return False


def is_blocked(request_path, root, feedback_path, script_path, shots_dir):
    """True if REQUEST_PATH, served from ROOT, would reach something private.

    The decision is made on the resolved real path, not the request string, so
    `/feedback%2Ejsonl`, `/./feedback.jsonl` and `//feedback.jsonl` all resolve
    to the same file as `/feedback.jsonl` and are blocked alike. Blocked: the
    feedback file, the script itself, anything inside the screenshots dir, and
    any file whose basename is in BLOCKED_NAMES.
    """

    class _Dir:  # translate_path only reads self.directory
        directory = root

    local = SimpleHTTPRequestHandler.translate_path(_Dir(), request_path)
    real = _real(local)
    if real in (_real(feedback_path), _real(script_path)):
        return True
    if _inside(real, _real(shots_dir)):
        return True
    return os.path.basename(real) in BLOCKED_NAMES


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=ROOT, **k)

    def end_headers(self):
        # The WASM/JS filenames are stable (no hash), so browsers cache them and
        # show a stale build after a rebuild ("the page didn't update"). Tell the
        # browser never to cache — this is a dev demo, freshness beats caching.
        self.send_header("Cache-Control", "no-store, must-revalidate")
        super().end_headers()

    def _blocked(self):
        return is_blocked(self.path, self.directory, FEEDBACK, SCRIPT, SHOTS_DIR)

    def list_directory(self, path):
        # No directory listings: a bare directory URL is a 404 like any missing file.
        self.send_error(404)
        return None

    def do_GET(self):
        if self._blocked():
            self.send_error(404)
            return
        super().do_GET()

    def do_HEAD(self):
        if self._blocked():
            self.send_error(404)
            return
        super().do_HEAD()

    def do_POST(self):
        if self.path.split("?", 1)[0] != "/feedback":
            self.send_error(404)
            return
        try:
            n = int(self.headers.get("Content-Length", 0))
            raw = self.rfile.read(n) if n > 0 else b"{}"
            data = json.loads(raw.decode("utf-8") or "{}")
            if not isinstance(data, dict):
                data = {"value": data}
        except Exception:
            self.send_error(400, "bad json")
            return
        # Pasted screenshots arrive as data: URLs. Decode each to a file under a
        # sibling screenshots/ dir and replace the bulky base64 in the logged record
        # with the relative path(s), so the jsonl stays readable. Accepts a list
        # (`screenshots`) or a single legacy string (`screenshot`).
        def _save_shot(durl, idx):
            header, b64 = durl.split(",", 1)
            ext = "jpg" if "image/jpeg" in header else "webp" if "image/webp" in header else "png"
            sdir = SHOTS_DIR
            os.makedirs(sdir, exist_ok=True)
            stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%S_%f")
            fname = f"{stamp}-{idx}-{self.client_address[0].replace(':', '_')}.{ext}"
            with open(os.path.join(sdir, fname), "wb") as imgf:
                imgf.write(base64.b64decode(b64))
            return os.path.join("screenshots", fname)

        shots = data.get("screenshots")
        if isinstance(shots, list):
            paths = []
            for i, s in enumerate(shots):
                durl = s.get("data") if isinstance(s, dict) else s
                if isinstance(durl, str) and durl.startswith("data:image/"):
                    try:
                        paths.append(_save_shot(durl, i))
                    except Exception:
                        paths.append("(screenshot decode failed)")
            data["screenshots"] = paths
        elif isinstance(data.get("screenshot"), str) and data["screenshot"].startswith("data:image/"):
            try:
                data["screenshot"] = _save_shot(data["screenshot"], 0)
            except Exception:
                data["screenshot"] = "(screenshot decode failed)"
        rec = {
            "ts": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
            "ip": self.client_address[0],
            "ua": self.headers.get("User-Agent", ""),
        }
        rec.update(data)
        try:
            with _lock, open(FEEDBACK, "a", encoding="utf-8") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        except Exception as e:
            self.send_error(500, f"write failed: {e}")
            return
        body = b'{"ok":true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass  # quiet


if __name__ == "__main__":
    httpd = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    print(f"etchy demo: serving {ROOT} on 0.0.0.0:{PORT}  (POST /feedback -> {FEEDBACK})")
    httpd.serve_forever()
