#!/usr/bin/env python3
"""Requirements checker for the etchy landing page.

Run from repo root: python3 scripts/check-landing.py
Exit 0 = all checks pass; exit 1 = at least one failure.
"""
import re
import sys
from html import unescape
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
INDEX = SITE / "index.html"
DOCS = SITE / "docs.html"
README = ROOT / "README.md"

failures = []


def check(cond, msg):
    if not cond:
        failures.append(msg)


def plain(html):
    return unescape(re.sub(r"<[^>]+>", "", html)).replace("−", "-")


def demo_totals(text):
    match = re.search(
        r"(\d+) of (\d+) layers?\b[^\n]*?\+(\d+\.\d+)\s*(?:mm²)?\s*/\s*"
        r"-(\d+\.\d+)\s*mm²[^\n]*?\((\d+)\+/(\d+)-\s*regions\)",
        text,
    )
    return match.groups() if match else None


def demo_rows(html, page):
    if page == "index":
        table = re.search(r'<table class="extable">(.*?)</table>', html, re.DOTALL)
        if not table:
            return None
        rows = re.findall(r"<tr>(.*?)</tr>", table.group(1), re.DOTALL)[1:]
        return [tuple(plain(cell).strip() for cell in re.findall(r"<td[^>]*>(.*?)</td>", row, re.DOTALL)) for row in rows]
    block = re.search(r"\$</span> etchy crates/etchy-gui/assets/demo/old.*?</pre>", html, re.DOTALL)
    if not block:
        return None
    lines = plain(block.group(0)).splitlines()[2:]
    return [tuple(line.split()) for line in lines if re.match(r"^[a-z][a-z0-9-]+\s+(changed|unchanged)\s", line)]


def main():
    check(INDEX.is_file(), "site/index.html is missing")
    if not INDEX.is_file():
        report()
        return
    html = INDEX.read_text(encoding="utf-8")

    check(DOCS.is_file(), "site/docs.html is missing")
    check(README.is_file(), "README.md is missing")
    if DOCS.is_file() and README.is_file():
        docs_html = DOCS.read_text(encoding="utf-8")
        readme = README.read_text(encoding="utf-8")
        index_rows = demo_rows(html, "index")
        docs_rows = demo_rows(docs_html, "docs")
        check(bool(index_rows), "index.html demo table is missing")
        check(bool(docs_rows), "docs.html demo table is missing")
        if index_rows and docs_rows:
            check(index_rows == docs_rows, "demo layer rows differ between index.html and docs.html")
        totals = [demo_totals(plain(html)), demo_totals(plain(docs_html)), demo_totals(readme)]
        check(all(totals), "demo totals are missing from index.html, docs.html or README.md")
        if all(totals):
            check(len(set(totals)) == 1, "demo totals differ between index.html, docs.html and README.md")
            if index_rows:
                changed, count, _, _, added_regions, removed_regions = totals[0]
                check(len(index_rows) == int(count), "index.html demo row count differs from its totals")
                check(sum(row[1] == "changed" for row in index_rows) == int(changed),
                      "index.html changed-row count differs from its totals")
                check(sum(int(row[4]) for row in index_rows) == int(added_regions),
                      "index.html added-region rows differ from its totals")
                check(sum(int(row[5]) for row in index_rows) == int(removed_regions),
                      "index.html removed-region rows differ from its totals")

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

    # Honest framing: no Phase-0 / in-development status.
    check("in development" not in html.lower(), "page still says 'in development'")
    check("Phase 0" not in html, "page still references 'Phase 0'")

    # Every locally-referenced asset must exist on disk.
    for m in re.finditer(r'(?:href|src)="((?!https?:|#|data:)[^"]+)"', html):
        rel = m.group(1).split("#", 1)[0]
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
