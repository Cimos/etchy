#!/usr/bin/env python3
"""etchy demo server: static files (GET) + POST /feedback -> feedback.jsonl.

Usage:  python etchy-server.py [PORT] [ROOT]
Serves ROOT (default: this script's dir) on 0.0.0.0:PORT (default 8080).
Feedback records are appended as one JSON object per line to feedback.jsonl,
stamped with the server time, client IP and User-Agent. The feedback file and
this script are never served over GET.
"""
import datetime
import json
import os
import sys
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
ROOT = sys.argv[2] if len(sys.argv) > 2 else os.path.dirname(os.path.abspath(__file__))
FEEDBACK = os.path.join(ROOT, "feedback.jsonl")
BLOCKED = {"/feedback.jsonl", "/etchy-server.py"}
_lock = threading.Lock()


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=ROOT, **k)

    def _blocked(self):
        p = self.path.split("?", 1)[0].rstrip("/").lower()
        return p in BLOCKED

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
