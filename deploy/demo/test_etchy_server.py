"""Offline tests for etchy-server.py (#334): no socket, no network.

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


if __name__ == "__main__":
    unittest.main()
