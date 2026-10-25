"""Tests for etchy-server.py (#334, #250).

BlockTests are offline; PostLimitTests start the server on 127.0.0.1 only.
Run from the repo root:  python3 -m unittest deploy/demo/test_etchy_server.py
"""
import importlib.util
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


def _load_server(root, feedback):
    """Import etchy-server.py as a module with ROOT/FEEDBACK pointed at ROOT."""
    argv, env = sys.argv, os.environ.get("ETCHY_FEEDBACK")
    sys.argv = ["etchy-server.py", "8080", root]
    os.environ["ETCHY_FEEDBACK"] = feedback
    try:
        spec = importlib.util.spec_from_file_location(
            "etchy_server", os.path.join(HERE, "etchy-server.py"))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
    finally:
        sys.argv = argv
        if env is None:
            os.environ.pop("ETCHY_FEEDBACK", None)
        else:
            os.environ["ETCHY_FEEDBACK"] = env
    return mod


class BlockTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = os.path.realpath(cls.tmp.name)
        cls.feedback = os.path.join(cls.root, "feedback.jsonl")
        cls.shots = os.path.join(cls.root, "screenshots")
        os.makedirs(cls.shots)
        # The default layout: bundle, page, script copy, feedback and screenshots
        # all in the served directory.
        for name in ("index.html", "etchy-gui.js", "etchy-server.py", "feedback.jsonl"):
            with open(os.path.join(cls.root, name), "w") as f:
                f.write("x")
        with open(os.path.join(cls.shots, "x.png"), "wb") as f:
            f.write(b"\x89PNG")
        cls.srv = _load_server(cls.root, cls.feedback)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def blocked(self, request_path, script=None):
        return self.srv.is_blocked(
            request_path, self.root, self.feedback,
            script or os.path.join(self.root, "etchy-server.py"), self.shots)

    def test_bypass_paths_are_blocked(self):
        for p in ("/feedback.jsonl", "/feedback%2Ejsonl", "/./feedback.jsonl",
                  "//feedback.jsonl", "/feedback.jsonl?x=1", "/FEEDBACK.JSONL"
                  if os.path.normcase("A") == "a" else "/feedback.jsonl#frag"):
            with self.subTest(path=p):
                self.assertTrue(self.blocked(p))

    def test_screenshots_are_blocked(self):
        for p in ("/screenshots/", "/screenshots", "/screenshots/x.png",
                  "/screenshots/missing.png", "/screenshots%2Fx.png"):
            with self.subTest(path=p):
                self.assertTrue(self.blocked(p))

    def test_script_is_blocked(self):
        self.assertTrue(self.blocked("/etchy-server.py"))
        # A copy under ROOT is blocked by name even when the running script is
        # the one in the source tree rather than the staged copy.
        self.assertTrue(self.blocked("/etchy-server.py",
                                     script=os.path.join(HERE, "etchy-server.py")))

    def test_bundle_is_served(self):
        for p in ("/index.html", "/etchy-gui.js", "/etchy-gui.js?v=3", "/",
                  "/etchy-gui_bg.wasm", "/missing.txt"):
            with self.subTest(path=p):
                self.assertFalse(self.blocked(p))

    def test_feedback_outside_root_reached_by_symlink(self):
        if not hasattr(os, "symlink"):
            self.skipTest("no symlink")
        with tempfile.TemporaryDirectory() as other:
            fb = os.path.join(other, "host.jsonl")
            open(fb, "w").close()
            link = os.path.join(self.root, "notes.txt")
            try:
                os.symlink(fb, link)
            except OSError:
                self.skipTest("symlink not permitted")
            try:
                self.assertTrue(self.srv.is_blocked(
                    "/notes.txt", self.root, fb,
                    os.path.join(self.root, "etchy-server.py"),
                    os.path.join(other, "screenshots")))
            finally:
                os.unlink(link)

    def test_list_directory_refuses(self):
        h = self.srv.Handler.__new__(self.srv.Handler)
        codes = []
        h.send_error = lambda code, *a, **k: codes.append(code)
        self.assertIsNone(h.list_directory(self.root))
        self.assertEqual(codes, [404])

    def test_handler_uses_resolved_path(self):
        h = self.srv.Handler.__new__(self.srv.Handler)
        h.directory = self.root
        h.path = "/feedback%2Ejsonl"
        self.assertTrue(h._blocked())
        h.path = "/index.html"
        self.assertFalse(h._blocked())


class PostLimitTests(unittest.TestCase):
    """POST /feedback against a live server on 127.0.0.1 (#250, #251)."""

    @classmethod
    def setUpClass(cls):
        import threading
        from http.server import ThreadingHTTPServer
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = os.path.realpath(cls.tmp.name)
        cls.feedback = os.path.join(cls.root, "fb", "feedback.jsonl")
        cls.shots = os.path.join(cls.root, "fb", "screenshots")
        cls.srv = _load_server(cls.root, cls.feedback)
        cls.srv.MAX_BODY = 64 * 1024
        cls.srv.MAX_SHOT_BYTES = 1024
        cls.srv.MAX_SHOTS = 2
        cls.httpd = ThreadingHTTPServer(("127.0.0.1", 0), cls.srv.Handler)
        cls.port = cls.httpd.server_address[1]
        threading.Thread(target=cls.httpd.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown()
        cls.httpd.server_close()
        cls.tmp.cleanup()

    def setUp(self):
        import shutil
        if os.path.exists(self.feedback):
            os.remove(self.feedback)
        shutil.rmtree(self.shots, ignore_errors=True)

    def post(self, body, headers=None):
        import http.client
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        try:
            c.putrequest("POST", "/feedback")
            for k, v in (headers or {}).items():
                c.putheader(k, v)
            c.endheaders()
            if body:
                c.send(body)
            return c.getresponse().status
        finally:
            c.close()

    def post_json(self, obj):
        import json
        body = json.dumps(obj).encode()
        return self.post(body, {"Content-Length": str(len(body))})

    def records(self):
        import json
        if not os.path.exists(self.feedback):
            return []
        with open(self.feedback, encoding="utf-8") as f:
            return [json.loads(line) for line in f if line.strip()]

    def shot_files(self):
        return os.listdir(self.shots) if os.path.isdir(self.shots) else []

    @staticmethod
    def png(n):
        import base64
        return "data:image/png;base64," + base64.b64encode(b"\x89PNG" + b"x" * (n - 4)).decode()

    def test_missing_content_length_is_411(self):
        self.assertEqual(self.post(None), 411)
        self.assertEqual(self.records(), [])

    def test_bad_content_length_is_400(self):
        for v in ("-1", "abc", "1.5"):
            with self.subTest(v=v):
                self.assertEqual(self.post(None, {"Content-Length": v}), 400)
        self.assertEqual(self.records(), [])

    def test_body_over_limit_is_413_without_reading(self):
        # Claims more than MAX_BODY but sends nothing: must answer at once.
        self.assertEqual(self.post(None, {"Content-Length": str(10 ** 12)}), 413)
        self.assertEqual(self.records(), [])

    def test_screenshot_over_limit_is_413_and_writes_nothing(self):
        status = self.post_json({"text": "hi", "screenshots": [self.png(100), self.png(2000)]})
        self.assertEqual(status, 413)
        self.assertEqual(self.records(), [])
        self.assertEqual(self.shot_files(), [])

    def test_legacy_screenshot_over_limit_is_413(self):
        self.assertEqual(self.post_json({"text": "hi", "screenshot": self.png(2000)}), 413)
        self.assertEqual(self.records(), [])

    def test_legacy_screenshot_beside_list_is_checked(self):
        big = {"text": "hi", "screenshots": [], "screenshot": self.png(2000)}
        self.assertEqual(self.post_json(big), 413)
        self.assertEqual(self.records(), [])
        ok = {"text": "hi", "screenshots": [self.png(10)], "screenshot": self.png(10)}
        self.assertEqual(self.post_json(ok), 200)
        (rec,) = self.records()
        self.assertTrue(rec["screenshot"].startswith("screenshots"))
        self.assertEqual(len(rec["screenshots"]), 1)
        self.assertEqual(len(self.shot_files()), 2)

    def test_too_many_screenshots_is_413(self):
        status = self.post_json({"text": "hi", "screenshots": [self.png(10)] * 3})
        self.assertEqual(status, 413)
        self.assertEqual(self.records(), [])
        self.assertEqual(self.shot_files(), [])

    def test_invalid_base64_is_noted_not_written(self):
        status = self.post_json({"text": "hi", "screenshots": [
            "data:image/png;base64,@@not base64@@", {"data": self.png(10)}]})
        self.assertEqual(status, 200)
        (rec,) = self.records()
        self.assertEqual(rec["screenshots"][0], "(screenshot decode failed)")
        self.assertTrue(rec["screenshots"][1].startswith("screenshots"))
        self.assertEqual(len(self.shot_files()), 1)

    def test_valid_submission_is_recorded_with_unique_id(self):
        self.assertEqual(self.post_json({"text": "ok", "screenshot": self.png(10)}), 200)
        self.assertEqual(self.post_json({"text": "ok", "id": "mine"}), 200)
        a, b = self.records()
        self.assertTrue(a["screenshot"].startswith("screenshots"))
        self.assertNotEqual(b["id"], "mine")
        self.assertNotEqual(a["id"], b["id"])


if __name__ == "__main__":
    unittest.main()
