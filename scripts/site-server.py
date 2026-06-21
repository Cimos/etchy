#!/usr/bin/env python3
"""etchy site + feedback server for LAN review.

Serves the landing/docs site (../site, no-store) AND accepts the widget's
POST /submit on the SAME origin, so anyone on the private network can open
http://<this-host>:8000/ and submit feedback (text + screenshots) to this machine.

Feedback is written to the local feedback-loop store: appends to feedback.jsonl,
decodes screenshots to uploads/<stamp>/, and bumps state/last_submission-<KEY>.json
so a watcher can wake. Binds 0.0.0.0 on purpose (LAN review) — private network only.

Env: FEEDBACK_PORT (default 8000), FEEDBACK_DIR (default ~/UbuntuProjects/feedback-loop),
     FEEDBACK_KEY (default landing).
"""
import http.server, socketserver, json, os, re, base64, datetime

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.normpath(os.path.join(HERE, "..", "site"))
FBDIR = os.environ.get("FEEDBACK_DIR") or os.path.expanduser("~/UbuntuProjects/feedback-loop")
KEY = os.environ.get("FEEDBACK_KEY", "landing")
PORT = int(os.environ.get("FEEDBACK_PORT", "8000"))
FEEDBACK = os.path.join(FBDIR, "feedback.jsonl")
UPLOADS = os.path.join(FBDIR, "uploads")
SIGNAL = os.path.join(FBDIR, "state", "last_submission-%s.json" % KEY)


def _safe(name):
    name = os.path.basename(name or "file")
    return (re.sub(r"[^A-Za-z0-9._-]", "_", name).strip("._") or "file")[:120]


def save_attachments(atts, stamp):
    saved = []
    if not atts:
        return saved
    folder = os.path.join(UPLOADS, stamp.replace(":", "").replace("-", ""))
    os.makedirs(folder, exist_ok=True)
    for i, a in enumerate(atts):
        m = re.match(r"^data:([^;,]*)(;base64)?,(.*)$", a.get("data", ""), re.DOTALL)
        if not m:
            continue
        try:
            blob = base64.b64decode(m.group(3)) if m.group(2) else m.group(3).encode("utf-8")
        except Exception:
            continue
        p = os.path.join(folder, "%02d_%s" % (i, _safe(a.get("name"))))
        with open(p, "wb") as fh:
            fh.write(blob)
        saved.append({"name": a.get("name"), "type": a.get("type"), "size": a.get("size"), "path": p})
    return saved


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=SITE, **k)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        super().end_headers()

    def do_POST(self):
        if self.path.split("?")[0] != "/submit":
            self.send_response(404); self.end_headers(); return
        n = int(self.headers.get("Content-Length", 0) or 0)
        try:
            data = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            data = {}
        data["_received_at"] = datetime.datetime.now().isoformat(timespec="seconds")
        data["key"] = KEY
        if data.get("attachments"):
            data["attachments"] = save_attachments(data["attachments"], data["_received_at"])
        os.makedirs(os.path.dirname(SIGNAL), exist_ok=True)
        with open(FEEDBACK, "a", encoding="utf-8") as f:
            f.write(json.dumps(data, ensure_ascii=False) + "\n")
        tmp = SIGNAL + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False)
        os.replace(tmp, SIGNAL)
        body = json.dumps({"ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("0.0.0.0", PORT), Handler) as httpd:
        print("etchy site+feedback on 0.0.0.0:%d (LAN) · serving %s · key=%s" % (PORT, SITE, KEY))
        httpd.serve_forever()
