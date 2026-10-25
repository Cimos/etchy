#!/usr/bin/env python3
"""Checks for the published demo comparison parser."""
import unittest
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

spec = spec_from_file_location("check_landing", Path(__file__).with_name("check-landing.py"))
checker = module_from_spec(spec)
spec.loader.exec_module(checker)
demo_rows = checker.demo_rows
demo_totals = checker.demo_totals


class DemoComparisonTests(unittest.TestCase):
    def test_totals_accept_site_markup_and_readme_text(self):
        site = '10 of 13 layer(s) changed; total <span>+213.29638 mm²</span> / <span>−448.70036 mm²</span> (967+/863− regions)'
        readme = '10 of 13 layers changed; +213.29638 / -448.70036 mm² (967+/863- regions)'
        self.assertEqual(demo_totals(checker.plain(site)), demo_totals(readme))

    def test_totals_reject_missing_region_counts(self):
        self.assertIsNone(demo_totals('10 of 13 layers changed; +213.29638 / -448.70036 mm²'))

    def test_site_table_rows_include_region_counts(self):
        index = '<table class="extable"><tr><th>Layer</th></tr><tr><td>top-copper</td><td>changed</td><td>80.16495</td><td>127.98536</td><td>133</td><td>121</td></tr></table>'
        docs = '<pre><span>$</span> etchy crates/etchy-gui/assets/demo/old crates/etchy-gui/assets/demo/new\nlayer status added removed +regions -regions\ntop-copper changed 80.16495 127.98536 133 121\n</pre>'
        self.assertEqual(demo_rows(index, 'index'), demo_rows(docs, 'docs'))


if __name__ == '__main__':
    unittest.main()
