"""Checks the default shell and staged demo feedback assets."""

import pathlib
import unittest
from html.parser import HTMLParser

ROOT = pathlib.Path(__file__).resolve().parents[2]


class Tags(HTMLParser):
    def __init__(self):
        super().__init__()
        self.tags = []

    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, dict(attrs)))


class FeedbackWidgetTests(unittest.TestCase):
    def test_default_shell_has_no_feedback_collection(self):
        shell = (ROOT / "crates/etchy-gui/index.html").read_text()
        self.assertNotIn("etfb-", shell)
        self.assertNotIn("/feedback", shell)
        self.assertNotIn("toDataURL", shell)
        self.assertNotIn("userAgent", shell)
        page = Tags()
        page.feed(shell)
        self.assertTrue(any(tag == "canvas" for tag, _ in page.tags))

    def test_demo_loads_one_widget_script_and_stages_it(self):
        shell = (ROOT / "deploy/demo/index.html").read_text()
        page = Tags()
        page.feed(shell)
        scripts = [attrs.get("src") for tag, attrs in page.tags if tag == "script" and "src" in attrs]
        self.assertEqual(scripts, ["/feedback-widget.js"])
        self.assertIn('id="etfb-panel"', shell)
        self.assertIn("Ctrl/Cmd+Enter", shell)
        self.assertIn("Ctrl+V", shell)
        self.assertIn('"$REPO/deploy/demo/feedback-widget.js"',
                      (ROOT / "deploy/setup.sh").read_text())

    def test_widget_keeps_attachment_paths_without_user_agent(self):
        script = (ROOT / "deploy/demo/feedback-widget.js").read_text()
        for event in ("paste", "drop", "dragover", "change", "keydown"):
            self.assertIn("addEventListener('" + event + "'", script)
        self.assertIn("shots.splice(i,1)", script)
        self.assertIn("screenshots:sent", script)
        self.assertIn("e.metaKey", script)
        self.assertNotIn("userAgent", script)
        self.assertNotIn("clientIp", script)
        self.assertNotIn("c.ua", script)


if __name__ == "__main__":
    unittest.main()
