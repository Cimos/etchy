#!/usr/bin/env python3
"""Requirements checker for the etchy landing page.

Run from repo root: python3 scripts/check-landing.py
Exit 0 = all checks pass; exit 1 = at least one failure.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
INDEX = SITE / "index.html"

failures = []


def check(cond, msg):
    if not cond:
        failures.append(msg)


def main():
    check(INDEX.is_file(), "site/index.html is missing")
    if not INDEX.is_file():
        report()
        return
    html = INDEX.read_text(encoding="utf-8")

    # No third-party font CDN at runtime (self-hosted requirement).
    check("fonts.googleapis.com" not in html, "index.html still references fonts.googleapis.com")
    check("fonts.gstatic.com" not in html, "index.html still references fonts.gstatic.com")

    # Local fonts wired in.
    check("assets/fonts/fonts.css" in html, "index.html does not link assets/fonts/fonts.css")

    # Required content / sections.
    for needle, label in [
        ('class="wordmark"', "etchy wordmark"),
        ("PCB visual", "headline 'PCB visual & geometric diff'"),
        ('id="what"', "'What it does' section"),
        ('id="roadmap"', "'On the roadmap' section"),
        ("View on GitHub", "GitHub CTA"),
    ]:
        check(needle in html, f"missing {label}")

    # Honest framing: no Phase-0 / in-development status, no dates/versions in copy.
    check("in development" not in html.lower(), "page still says 'in development'")
    check("Phase 0" not in html, "page still references 'Phase 0'")
    check(not re.search(r"\bv\d+\.\d+", html), "page contains a version number (roadmap must be undated/unversioned)")

    # Every locally-referenced asset must exist on disk.
    for m in re.finditer(r'(?:href|src)="((?!https?:|#|data:)[^"]+)"', html):
        rel = m.group(1)
        check((SITE / rel).exists(), f"referenced asset missing on disk: {rel}")

    # Social card must exist, and og:image/twitter:image must be ABSOLUTE URLs
    # (OGP/Twitter crawlers do not resolve relative paths — relative = broken preview).
    check((SITE / "assets" / "brand" / "etchy-social-1280x640.png").is_file(),
          "social card site/assets/brand/etchy-social-1280x640.png is missing")
    for attr, label in [('property="og:image"', "og:image"), ('name="twitter:image"', "twitter:image")]:
        m = re.search(attr + r'\s+content="([^"]+)"', html)
        check(m is not None, f"missing meta {label}")
        if m:
            check(m.group(1).startswith("https://"),
                  f"meta {label} must be an absolute https URL (got '{m.group(1)}')")

    # fonts.css must reference woff2 files that exist.
    fonts_css = SITE / "assets" / "fonts" / "fonts.css"
    check(fonts_css.is_file(), "site/assets/fonts/fonts.css is missing")
    if fonts_css.is_file():
        css = fonts_css.read_text(encoding="utf-8")
        for m in re.finditer(r'url\("\./([^"]+\.woff2)"\)', css):
            woff = SITE / "assets" / "fonts" / m.group(1)
            check(woff.is_file(), f"font file missing on disk: {m.group(1)}")

    report()


def report():
    if failures:
        print("LANDING CHECK: FAIL")
        for f in failures:
            print("  - " + f)
        sys.exit(1)
    print("LANDING CHECK: PASS")


if __name__ == "__main__":
    main()
