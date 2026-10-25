"""Tests for collect-feedback.py de-duplication (#251).

Run from the repo root:  python3 -m unittest deploy/test_collect_feedback.py
"""
import importlib.util
import json
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "collect_feedback", os.path.join(HERE, "collect-feedback.py"))
cf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cf)

BASE = {"ts": "2026-10-19T01:00:00+11:00", "ip": "10.0.0.5", "sentiment": "positive",
        "category": "Layout & sizing", "text": "looks good"}


class DedupeTests(unittest.TestCase):
    def load(self, files):
        with tempfile.TemporaryDirectory() as d:
            for name, recs in files.items():
                with open(os.path.join(d, name), "w", encoding="utf-8") as f:
                    for r in recs:
                        f.write((r if isinstance(r, str) else json.dumps(r)) + "\n")
            old = cf.FEEDBACK_DIR
            cf.FEEDBACK_DIR = d
            try:
                return cf.load()
            finally:
                cf.FEEDBACK_DIR = old

    def test_same_second_same_text_different_category_both_kept(self):
        b = dict(BASE, category="Clarity")
        self.assertEqual(len(self.load({"h.jsonl": [BASE, b]})), 2)

    def test_same_second_same_text_different_screenshots_both_kept(self):
        a = dict(BASE, screenshots=["screenshots/a.png"])
        b = dict(BASE, screenshots=["screenshots/b.png"])
        recs = self.load({"h.jsonl": [a, b]})
        self.assertEqual(sorted(r["screenshots"][0] for r in recs),
                         ["screenshots/a.png", "screenshots/b.png"])

    def test_identical_content_with_distinct_ids_both_kept(self):
        recs = self.load({"h.jsonl": [dict(BASE, id="1"), dict(BASE, id="2")]})
        self.assertEqual(len(recs), 2)

    def test_copies_across_files_collapse(self):
        with_id = dict(BASE, id="1")
        legacy = dict(BASE, text="older, no id")
        recs = self.load({"a.jsonl": [with_id, legacy], "b.jsonl": [legacy, with_id]})
        self.assertEqual(len(recs), 2)

    def test_legacy_records_and_junk_lines_load(self):
        recs = self.load({"h.jsonl": [BASE, "not json", "[1, 2]", ""]})
        self.assertEqual(len(recs), 1)
        self.assertEqual(recs[0]["text"], "looks good")


if __name__ == "__main__":
    unittest.main()
