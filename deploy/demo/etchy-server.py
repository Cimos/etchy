#!/usr/bin/env python3
"""etchy demo server: static files (GET) + POST /feedback -> feedback.jsonl.

Usage:  python etchy-server.py [PORT] [ROOT]
Serves ROOT (default: this script's dir) on 127.0.0.1:PORT (default 8080).
Set ETCHY_BIND=0.0.0.0 to serve the LAN.
Feedback records are appended as one JSON object per line, stamped with the
server time and a unique server-made "id". The client IP and User-Agent are
not recorded (#287), and any "ip"/"ua" a client sends is dropped. POST bodies
need a Content-Length (411 without, 400 if invalid) of at most MAX_BODY bytes,
with at most MAX_SHOTS screenshots of MAX_SHOT_BYTES each once decoded; over a
limit is 413 and nothing is written. The feedback file, the screenshots/
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
import binascii
import datetime
import json
import os
import sys
import threading
import uuid
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
ROOT = sys.argv[2] if len(sys.argv) > 2 else os.path.dirname(os.path.abspath(__file__))
BIND = os.environ.get("ETCHY_BIND", "127.0.0.1")
FEEDBACK = os.environ.get("ETCHY_FEEDBACK") or os.path.join(ROOT, "feedback.jsonl")
os.makedirs(os.path.dirname(os.path.abspath(FEEDBACK)), exist_ok=True)
# Pasted screenshots are decoded into a screenshots/ dir beside the feedback file.
SHOTS_DIR = os.path.join(os.path.dirname(os.path.abspath(FEEDBACK)), "screenshots")
SCRIPT = os.path.abspath(__file__)
# Never served, by basename, wherever a copy sits under ROOT (the staged serve dir
# holds a copy of this script; the feedback file may have been renamed).
BLOCKED_NAMES = {"feedback.jsonl", "etchy-server.py", os.path.basename(FEEDBACK)}
_lock = threading.Lock()

# POST /feedback limits (#250). The server may face the LAN, so one request must
# not be able to exhaust memory or disk. Over-limit requests get 413.
MAX_BODY = 16 * 1024 * 1024  # whole request body, bytes
MAX_SHOT_BYTES = 8 * 1024 * 1024  # one decoded screenshot, bytes
MAX_SHOTS = 8  # screenshots per submission


class ShotTooLarge(Exception):
    pass


def decode_shot(durl):
    """Decode a data:image/... URL to (ext, bytes).

    Returns None if it is not valid base64. Raises ShotTooLarge if the decoded
    image would exceed MAX_SHOT_BYTES (checked before decoding).
    """
    header, sep, b64 = durl.partition(",")
    if not sep or ";base64" not in header:
        return None
    b64 = "".join(b64.split())  # tolerate wrapped base64
    if len(b64) // 4 * 3 > MAX_SHOT_BYTES + 3:
        raise ShotTooLarge()
    try:
        img = base64.b64decode(b64, validate=True)
    except (binascii.Error, ValueError):
        return None
    if not img:
        return None
    if len(img) > MAX_SHOT_BYTES:
        raise ShotTooLarge()
    ext = "jpg" if "image/jpeg" in header else "webp" if "image/webp" in header else "png"
    return ext, img


def strip_pii(rec):
    """Drop the client IP and User-Agent from a feedback record, in place (#287).

    Neither is used for triage. Removes top-level "ip"/"ua" and "ctx.ua" (sent
    by older widget builds). Returns REC.
    """
    rec.pop("ip", None)
    rec.pop("ua", None)
    if isinstance(rec.get("ctx"), dict):
        rec["ctx"].pop("ua", None)
    return rec


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

    def _reject(self, code, msg):
        # The body was not read, so the connection can't be reused.
        self.close_connection = True
        self.send_error(code, msg)

    def do_POST(self):
        if self.path.split("?", 1)[0] != "/feedback":
            self.send_error(404)
            return
        length = self.headers.get("Content-Length")
        if length is None:
            self._reject(411, "Content-Length required")
            return
        try:
            n = int(length)
        except ValueError:
            n = -1
        if n < 0:
            self._reject(400, "bad Content-Length")
            return
        if n > MAX_BODY:
            self._reject(413, f"body over {MAX_BODY} bytes")
            return
        try:
            raw = self.rfile.read(n) if n > 0 else b"{}"
            data = json.loads(raw.decode("utf-8") or "{}")
            if not isinstance(data, dict):
                data = {"value": data}
        except Exception:
            self.send_error(400, "bad json")
            return
        # Pasted screenshots arrive as data: URLs. Every one is decoded and checked
        # before any file is written, so a rejected request leaves nothing on disk.
        # Accepts a list (`screenshots`) and/or a single legacy string
        # (`screenshot`); both are capped together and neither is logged as base64.
        shots = data.get("screenshots")
        legacy = data.get("screenshot")
        urls = []
        if isinstance(shots, list):
            urls = [s.get("data") if isinstance(s, dict) else s for s in shots]
            urls = [u for u in urls if isinstance(u, str) and u.startswith("data:image/")]
        has_legacy = isinstance(legacy, str) and legacy.startswith("data:image/")
        if has_legacy:
            urls.append(legacy)
        if len(urls) > MAX_SHOTS:
            self._reject(413, f"more than {MAX_SHOTS} screenshots")
            return
        decoded = []
        for durl in urls:
            try:
                d = decode_shot(durl)
            except ShotTooLarge:
                self._reject(413, f"screenshot over {MAX_SHOT_BYTES} bytes")
                return
            decoded.append(d)
        # Decoded images go to files under a sibling screenshots/ dir; the logged
        # record carries the relative path(s) instead of the bulky base64, so the
        # jsonl stays readable. An undecodable image is noted, never written.
        paths = []
        for i, d in enumerate(decoded):
            if d is None:
                paths.append("(screenshot decode failed)")
                continue
            ext, img = d
            os.makedirs(SHOTS_DIR, exist_ok=True)
            stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%S_%f")
            fname = f"{stamp}-{i}-{uuid.uuid4().hex[:8]}.{ext}"
            try:
                with open(os.path.join(SHOTS_DIR, fname), "wb") as imgf:
                    imgf.write(img)
            except Exception:
                paths.append("(screenshot write failed)")
                continue
            paths.append(os.path.join("screenshots", fname))
        if has_legacy:
            data["screenshot"] = paths.pop()
        if isinstance(shots, list):
            data["screenshots"] = paths
        rec = {"ts": datetime.datetime.now().astimezone().isoformat(timespec="seconds")}
        rec.update(strip_pii(data))
        # Server-made id: lets collect-feedback.py tell two genuine submissions
        # with identical content apart (#251). Set last so a client can't pick it.
        rec["id"] = uuid.uuid4().hex
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
    httpd = ThreadingHTTPServer((BIND, PORT), Handler)
    print(f"etchy demo: serving {ROOT} on {BIND}:{PORT}  (POST /feedback -> {FEEDBACK})")
    httpd.serve_forever()
