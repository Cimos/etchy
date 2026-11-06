"""Headless-browser tests for the demo feedback widget (#439).

Loads the real deploy/demo/index.html + feedback-widget.js in headless Chromium,
served by etchy-server.py on 127.0.0.1, with the WASM bundle stubbed out (no
build needed). Checks Ctrl/Cmd+Enter submit, paste with removable thumbnails,
drag-drop, the file picker, the POST payload, that nothing is sent without
a user action, and that the server's screenshot/body caps are refused
client-side with a visible message.

Needs Playwright:  pip install -r deploy/requirements-browser-test.txt
                   python3 -m playwright install chromium
Run from the repo root:  python3 -m unittest deploy/demo/test_feedback_widget_browser.py
Skipped if Playwright is missing, unless ETCHY_BROWSER_TEST=1 (as in CI), where
a missing Playwright is an error.
"""
import base64
import json
import os
import sys
import tempfile
import threading
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from test_etchy_server import _load_server  # noqa: E402

try:
    from playwright.sync_api import sync_playwright
except ImportError:
    if os.environ.get("ETCHY_BROWSER_TEST") == "1":
        raise
    sync_playwright = None

# The only keys the widget may send (no ua, no ip).
PAYLOAD_KEYS = {"sentiment", "category", "severity", "text", "name", "ctx", "screenshots"}
CTX_KEYS = {"screen", "viewport", "dpr", "lang", "url", "app"}

# Fail the test if the page reads the User-Agent or tries to open a new tab.
INIT_SCRIPT = """
window.__uaRead = false;
Object.defineProperty(Navigator.prototype, 'userAgent', {
  get() { window.__uaRead = true; return 'test'; }, configurable: true });
window.open = function () { window.__opened = (window.__opened || 0) + 1; return null; };
"""

# Smallest valid PNGs, distinct so a removed shot can be told apart.
PNG_A = base64.b64encode(bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010806000000"
    "1f15c4890000000d49444154789c6360f8cfc0f01f0005000201b6d6cf"
    "1b0000000049454e44ae426082")).decode()
PNG_B = base64.b64encode(bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010806000000"
    "1f15c4890000000d49444154789c63f8cfc0f01f0005000201b6d6cf1b"
    "0000000049454e44ae426082")).decode()

# Builds image Files in the page and fires a paste on the document or a drop on
# the attach area. Chromium honours clipboardData/dataTransfer in these ctors.
DISPATCH = """([kind, files]) => {
  const dt = new DataTransfer();
  for (const f of files) {
    let u;
    if (f.size !== undefined) {
      u = new Uint8Array(f.size);  // zero-filled; only the size matters
    } else {
      const bin = atob(f.b64);
      u = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
    }
    dt.items.add(new File([u], f.name, { type: f.type }));
  }
  if (kind === 'paste') {
    document.dispatchEvent(new ClipboardEvent('paste',
      { clipboardData: dt, bubbles: true, cancelable: true }));
  } else {
    const target = document.getElementById('etfb-shot');
    target.dispatchEvent(new DragEvent('dragover', { dataTransfer: dt, bubbles: true, cancelable: true }));
    target.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
  }
}"""


@unittest.skipIf(sync_playwright is None, "Playwright not installed (set ETCHY_BROWSER_TEST=1 to require it)")
class FeedbackWidgetBrowserTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.feedback = os.path.join(os.path.realpath(cls.tmp.name), "feedback.jsonl")
        # Serve deploy/demo itself: it holds index.html and feedback-widget.js.
        cls.srv = _load_server(HERE, cls.feedback)
        cls.httpd = cls.srv.ThreadingHTTPServer(("127.0.0.1", 0), cls.srv.Handler)
        threading.Thread(target=cls.httpd.serve_forever, daemon=True).start()
        cls.base = "http://127.0.0.1:%d" % cls.httpd.server_address[1]
        cls.pw = sync_playwright().start()
        cls.browser = cls.pw.chromium.launch()

    @classmethod
    def tearDownClass(cls):
        cls.browser.close()
        cls.pw.stop()
        cls.httpd.shutdown()
        cls.httpd.server_close()
        cls.tmp.cleanup()

    def setUp(self):
        open(self.feedback, "w").close()
        self.ctx = self.browser.new_context()
        self.ctx.add_init_script(INIT_SCRIPT)
        self.page = self.ctx.new_page()
        self.page.set_default_timeout(10000)
        self.posts = []
        self.page.on("request", lambda r: r.method != "GET" and self.posts.append(r))
        self.errors = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        # Stub the WASM bundle so the page loads without a build.
        self.page.route("**/etchy-gui.js*", lambda r: r.fulfill(
            content_type="text/javascript", body="export default async function init() {}"))
        self.page.route("**/etchy-gui_bg.wasm*", lambda r: r.fulfill(
            content_type="application/wasm", body=b"\0asm"))
        self.page.goto(self.base + "/")
        self.page.wait_for_function("typeof window.etfbSend === 'function'")

    def tearDown(self):
        self.ctx.close()

    # --- helpers ---

    def open_panel(self, text="the layer buttons are too small"):
        self.page.click("#etfb-fab")
        self.page.wait_for_selector("#etfb-panel.open")
        if text is not None:
            self.page.fill("#etfb-text", text)

    def attach(self, kind, *files):
        self.page.evaluate(DISPATCH, [kind, [
            {"b64": b64, "name": name, "type": typ} for b64, name, typ in files]])

    def attach_sized(self, kind, *files):
        """Attach zero-filled images of the given byte sizes."""
        self.page.evaluate(DISPATCH, [kind, [
            {"size": size, "name": name, "type": "image/png"} for size, name in files]])

    def error(self, *needles):
        self.page.wait_for_selector("#etfb-err.show")
        msg = self.page.text_content("#etfb-err")
        for needle in needles:
            self.assertIn(needle, msg)
        return msg

    def no_error(self):
        self.assertFalse(self.page.is_visible("#etfb-err"))

    def thumbs(self, n):
        self.page.wait_for_function(
            "n => document.querySelectorAll('#etfb-shots .etfb-thumb').length === n", arg=n)
        return [self.page.get_attribute(sel, "alt") for sel in
                ["#etfb-shots .etfb-thumb:nth-child(%d) img" % (i + 1) for i in range(n)]]

    def submit(self, key="Control+Enter"):
        with self.page.expect_response("**/feedback") as resp:
            self.page.press("#etfb-text", key)
        self.assertEqual(resp.value.status, 200)
        self.page.wait_for_selector("#etfb-toast.show")
        self.assertIn("Thanks", self.page.text_content("#etfb-toast"))
        self.assertEqual(len(self.posts), 1, "exactly one POST per submit")
        return self.check_payload(self.posts[0])

    def check_payload(self, req):
        self.assertEqual(req.url, self.base + "/feedback")
        self.assertEqual(req.headers.get("content-type"), "application/json")
        body = json.loads(req.post_data)
        self.assertEqual(set(body), PAYLOAD_KEYS)
        self.assertLessEqual(set(body["ctx"]), CTX_KEYS)
        for key in ("ua", "ip", "userAgent", "clientIp"):
            self.assertNotIn(key, body)
            self.assertNotIn(key, body["ctx"])
        # Within the server's caps (#250), so it is never refused with 413.
        self.assertLessEqual(len(req.post_data.encode()), self.srv.MAX_BODY)
        self.assertLessEqual(len(body["screenshots"]), self.srv.MAX_SHOTS)
        for shot in body["screenshots"]:
            self.assertEqual(set(shot), {"data", "name"})
            self.assertTrue(shot["data"].startswith("data:image/"))
            kind, img = self.srv.decode_shot(shot["data"])
            self.assertLessEqual(len(img), self.srv.MAX_SHOT_BYTES)
        self.assertFalse(self.page.evaluate("window.__uaRead"), "widget read navigator.userAgent")
        self.assertEqual(self.errors, [])
        return body

    def records(self):
        with open(self.feedback, encoding="utf-8") as f:
            return [json.loads(line) for line in f if line.strip()]

    # --- tests ---

    def test_nothing_sent_without_user_action(self):
        # Past the hint bubble's timers, with the panel opened, typed in and a
        # screenshot pasted: still nothing leaves the page until Send.
        self.page.wait_for_timeout(2500)
        self.open_panel()
        self.attach("paste", (PNG_A, "a.png", "image/png"))
        self.thumbs(1)
        self.page.press("#etfb-text", "Enter")
        self.page.wait_for_timeout(500)
        self.assertEqual(self.posts, [])
        self.assertEqual(self.records(), [])
        # Closing the panel throws the draft away without sending.
        self.page.click(".etfb-x")
        self.page.wait_for_timeout(300)
        self.assertEqual(self.posts, [])
        self.assertEqual(self.errors, [])

    def test_empty_text_is_not_sent(self):
        self.open_panel(text="   ")
        self.page.press("#etfb-text", "Control+Enter")
        self.page.click("#etfb-send")
        self.page.wait_for_timeout(500)
        self.assertEqual(self.posts, [])
        self.assertTrue(self.page.is_visible("#etfb-panel.open"))

    def test_ctrl_enter_submits_and_server_stores_it(self):
        self.open_panel()
        self.page.click(".etfb-sent[data-v='idea']")
        self.page.click("#etfb-pills .etfb-pill:has-text('Speed')")
        self.page.fill("#etfb-name", "SD")
        body = self.submit("Control+Enter")
        self.assertEqual(body["sentiment"], "idea")
        self.assertEqual(body["category"], "Speed")
        self.assertIsNone(body["severity"])
        self.assertEqual(body["text"], "the layer buttons are too small")
        self.assertEqual(body["name"], "SD")
        self.assertEqual(body["ctx"]["url"], self.base + "/")
        # No shot attached: the live canvas is captured on Send.
        self.assertEqual([s["name"] for s in body["screenshots"]], ["canvas.png"])
        # The panel resets after a successful send.
        self.page.wait_for_selector("#etfb-panel:not(.open)", state="attached")
        self.assertEqual(self.page.input_value("#etfb-text"), "")
        [rec] = self.records()
        self.assertEqual(rec["text"], body["text"])
        self.assertNotIn("ua", rec)
        self.assertNotIn("ip", rec)
        self.assertEqual(len(rec["screenshots"]), 1)
        self.assertTrue(rec["screenshots"][0].startswith("screenshots"))

    def test_cmd_enter_submits(self):
        self.open_panel()
        body = self.submit("Meta+Enter")
        self.assertEqual(body["sentiment"], "issue")
        self.assertEqual(body["severity"], "Annoying")

    def test_pasted_shots_become_removable_thumbnails(self):
        self.open_panel()
        self.attach("paste", (PNG_A, "a.png", "image/png"), (PNG_B, "b.png", "image/png"))
        self.assertEqual(self.thumbs(2), ["a.png", "b.png"])
        self.assertIn("2 attached", self.page.text_content("#etfb-shot"))
        # Remove the first thumbnail; only b.png is left and sent.
        self.page.click("#etfb-shots .etfb-thumb:nth-child(1) .rm")
        self.assertEqual(self.thumbs(1), ["b.png"])
        body = self.submit()
        self.assertEqual(body["screenshots"],
                         [{"data": "data:image/png;base64," + PNG_B, "name": "b.png"}])
        self.assertEqual(len(self.records()[0]["screenshots"]), 1)

    def test_paste_ignores_non_images_and_closed_panel(self):
        # Panel closed: a paste is not captured.
        self.attach("paste", (PNG_A, "a.png", "image/png"))
        self.open_panel()
        self.thumbs(0)
        self.attach("paste", (base64.b64encode(b"hi").decode(), "x.txt", "text/plain"))
        self.thumbs(0)
        self.assertEqual(self.posts, [])

    def test_dropped_shot_is_attached(self):
        self.open_panel()
        self.attach("drop", (PNG_A, "dropped.png", "image/png"),
                    (base64.b64encode(b"no").decode(), "notes.txt", "text/plain"))
        self.assertEqual(self.thumbs(1), ["dropped.png"])
        body = self.submit()
        self.assertEqual(body["screenshots"],
                         [{"data": "data:image/png;base64," + PNG_A, "name": "dropped.png"}])

    def test_file_picker_attaches(self):
        self.open_panel()
        self.page.set_input_files("#etfb-file", [
            {"name": "picked.png", "mimeType": "image/png", "buffer": base64.b64decode(PNG_B)}])
        self.assertEqual(self.thumbs(1), ["picked.png"])
        body = self.submit()
        self.assertEqual([s["name"] for s in body["screenshots"]], ["picked.png"])

    def test_github_link_is_opt_in_and_sends_nothing(self):
        self.open_panel()
        self.page.click("#etfb-gh")
        self.assertEqual(self.page.evaluate("window.__opened"), 1)
        self.assertEqual(self.posts, [])

    # --- client-side caps: the server's MAX_* limits (#250), refused visibly ---

    def shots(self, n, prefix="s"):
        return [(PNG_A, "%s%d.png" % (prefix, i), "image/png") for i in range(n)]

    def test_paste_caps_at_max_shots(self):
        cap = self.srv.MAX_SHOTS
        self.open_panel()
        self.no_error()
        self.attach("paste", *self.shots(cap + 1))
        self.assertEqual(len(self.thumbs(cap)), cap)
        self.error("Max %d screenshots" % cap, "s%d.png" % cap)
        self.assertIn("%d attached (max %d)" % (cap, cap), self.page.text_content("#etfb-shot"))
        # Still full: another paste is refused too, and says so.
        self.attach("paste", (PNG_B, "extra.png", "image/png"))
        self.thumbs(cap)
        self.error("Max %d screenshots" % cap, "extra.png")
        # Removing one clears the message and makes room again.
        self.page.click("#etfb-shots .etfb-thumb:nth-child(1) .rm")
        self.thumbs(cap - 1)
        self.no_error()
        self.attach("paste", (PNG_B, "extra.png", "image/png"))
        self.thumbs(cap)
        self.no_error()
        body = self.submit()
        self.assertEqual(len(body["screenshots"]), cap)
        self.assertEqual(len(self.records()[0]["screenshots"]), cap)

    def test_drop_caps_at_max_shots(self):
        cap = self.srv.MAX_SHOTS
        self.open_panel()
        self.attach("paste", *self.shots(cap - 1))
        self.thumbs(cap - 1)
        self.attach("drop", (PNG_B, "d1.png", "image/png"), (PNG_B, "d2.png", "image/png"))
        self.assertEqual(self.thumbs(cap)[-1], "d1.png")
        msg = self.error("Max %d screenshots" % cap, "d2.png")
        self.assertNotIn("d1.png", msg)
        self.assertEqual(len(self.submit()["screenshots"]), cap)

    def test_file_picker_caps_at_max_shots(self):
        cap = self.srv.MAX_SHOTS
        self.open_panel()
        self.page.set_input_files("#etfb-file", [
            {"name": "p%d.png" % i, "mimeType": "image/png", "buffer": base64.b64decode(PNG_B)}
            for i in range(cap + 2)])
        self.thumbs(cap)
        self.error("Max %d screenshots" % cap, "p%d.png" % cap, "p%d.png" % (cap + 1))
        self.assertEqual(len(self.submit()["screenshots"]), cap)

    def test_oversized_shot_is_refused_on_every_path(self):
        over = self.srv.MAX_SHOT_BYTES + 1
        self.open_panel()
        for kind in ("paste", "drop"):
            self.attach_sized(kind, (over, kind + "-big.png"))
            self.error("Image too large", kind + "-big.png")
            self.thumbs(0)
        self.page.set_input_files("#etfb-file", [
            {"name": "picked-big.png", "mimeType": "image/png", "buffer": bytes(over)}])
        self.error("Image too large", "picked-big.png")
        self.thumbs(0)
        # Only the oversized one is refused; the rest of the batch is kept.
        self.attach("drop", (PNG_A, "ok.png", "image/png"))
        self.attach_sized("drop", (over, "big.png"))
        self.thumbs(1)
        self.error("Image too large", "big.png")
        self.assertEqual(self.posts, [])

    def test_shot_at_the_size_limit_is_sent(self):
        self.open_panel()
        self.attach_sized("drop", (self.srv.MAX_SHOT_BYTES, "edge.png"))
        self.assertEqual(self.thumbs(1), ["edge.png"])
        self.no_error()
        body = self.submit()
        self.assertEqual([s["name"] for s in body["screenshots"]], ["edge.png"])

    def test_attachments_over_the_body_budget_are_refused(self):
        # Two 6 MiB images base64-encode to 16 MiB: the second can never fit.
        size = 6 * 1024 * 1024
        self.open_panel()
        self.attach_sized("drop", (size, "one.png"))
        self.thumbs(1)
        self.attach_sized("paste", (size, "two.png"))
        self.error("total limit", "two.png")
        self.assertEqual(self.thumbs(1), ["one.png"])
        self.assertEqual([s["name"] for s in self.submit()["screenshots"]], ["one.png"])

    def test_body_over_the_limit_is_not_sent(self):
        self.open_panel()
        self.attach_sized("drop", (7 * 1024 * 1024, "big.png"))
        self.thumbs(1)
        # Long text pushes the whole body past MAX_BODY.
        self.page.evaluate("n => { document.getElementById('etfb-text').value = 'x'.repeat(n); }",
                           self.srv.MAX_BODY // 2)
        self.page.click("#etfb-send")
        self.error("too large to send", "remove a screenshot")
        self.page.wait_for_timeout(300)
        self.assertEqual(self.posts, [])
        self.assertTrue(self.page.is_visible("#etfb-panel.open"))
        self.thumbs(1)
        # Shortening the text lets it through.
        self.page.fill("#etfb-text", "short now")
        body = self.submit()
        self.assertEqual([s["name"] for s in body["screenshots"]], ["big.png"])

    def test_attach_while_sending_is_refused_visibly(self):
        held = []
        self.page.route("**/feedback", lambda r: held.append(r))
        self.open_panel()
        self.attach("paste", (PNG_A, "a.png", "image/png"))
        self.thumbs(1)
        self.page.click("#etfb-send")
        for _ in range(100):
            if held:
                break
            self.page.wait_for_timeout(50)
        self.assertEqual(len(held), 1)
        # The draft is reset when the send completes, so a late paste is refused.
        self.attach("paste", (PNG_B, "late.png", "image/png"))
        self.error("Sending")
        self.thumbs(1)
        held[0].continue_()
        self.page.wait_for_selector("#etfb-toast.show")
        self.assertIn("Thanks", self.page.text_content("#etfb-toast"))
        self.assertEqual(len(self.records()), 1)

    def test_oversized_canvas_capture_is_skipped_with_notice(self):
        self.page.evaluate("n => { HTMLCanvasElement.prototype.toDataURL ="
                           " () => 'data:image/png;base64,' + 'A'.repeat(n); }",
                           (self.srv.MAX_SHOT_BYTES + 3) // 3 * 4 + 4)
        self.open_panel()
        body = self.submit()
        self.assertEqual(body["screenshots"], [])
        self.assertIn("too large to attach", self.page.text_content("#etfb-toast"))


if __name__ == "__main__":
    unittest.main()
