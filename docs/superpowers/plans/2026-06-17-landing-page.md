# etchy Landing Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the single-page etchy landing site in `site/` and a GitHub Actions workflow that deploys it to GitHub Pages, launch-ready for when `Cimos/etchy` is made public.

**Architecture:** One static `site/index.html` with inline `<style>` (no JS, no build step, no SSG). Fonts and brand assets are vendored locally so there is no third-party CDN dependency at runtime. A `.github/workflows/pages.yml` workflow uploads `site/` as the Pages artifact and deploys it using only official, SHA-pinned Actions with least-privilege permissions. The page presents the **M1 MVP** feature set in present tense and an undated "On the roadmap" section.

**Tech Stack:** HTML5 + CSS (inline), self-hosted woff2 fonts (Zilla Slab 700; JetBrains Mono 400/500/700), inline SVG brand mark, GitHub Pages via GitHub Actions. A `python3` checker script serves as the test harness.

**Reference artifact:** the approved mockup `etchy-landing-final-v2.html` (delivered during brainstorming). Task 5 reproduces it with self-hosted fonts + a real `<head>`.

**Source of brand assets:** they live on the `gui-brand-theme` branch under `assets/brand/` (not on `main` yet). This plan vendors copies into `site/assets/`.

---

## File Structure

| Path | Responsibility |
|---|---|
| `site/index.html` | The entire page: markup + inline `<style>` + inline brand SVG. |
| `site/assets/fonts/*.woff2` | Vendored Zilla Slab 700, JetBrains Mono 400/500/700. |
| `site/assets/fonts/fonts.css` | `@font-face` declarations (imported by `index.html`). |
| `site/assets/fonts/OFL.txt` | SIL Open Font License text for the vendored fonts. |
| `site/assets/brand/` | Favicons (svg + 16/32/48 png), apple-touch (180 png), social card (1280×640 png). |
| `site/.nojekyll` | Disables any Jekyll processing of the artifact (belt-and-suspenders). |
| `.github/workflows/pages.yml` | Build/deploy `site/` to GitHub Pages (official, SHA-pinned Actions). |
| `scripts/check-landing.py` | Requirements checker / test harness (NOT deployed — lives outside `site/`). |

Note: `scripts/check-landing.py` is intentionally outside `site/` so it is not published in the Pages artifact.

---

## Task 1: Scaffold `site/` and vendor brand assets

**Files:**
- Create: `site/.nojekyll`
- Create: `site/assets/brand/` (copied files)

- [ ] **Step 1: Create the directory skeleton**

Run:
```bash
mkdir -p site/assets/fonts site/assets/brand scripts
touch site/.nojekyll
```

- [ ] **Step 2: Vendor the brand assets from the `gui-brand-theme` branch**

The branch holds them under `assets/brand/`. Copy only the files the site needs into `site/assets/brand/` using `git show` (does not disturb the working tree's tracked paths):

```bash
for f in etchy-favicon.svg etchy-icon.svg; do
  git show gui-brand-theme:assets/brand/$f > site/assets/brand/$f
done
for f in etchy-favicon-16.png etchy-favicon-32.png etchy-favicon-48.png \
         etchy-app-icon-180.png etchy-social-1280x640.png; do
  git show gui-brand-theme:assets/brand/png/$f > site/assets/brand/$f
done
```

- [ ] **Step 3: Verify the assets landed and are non-empty**

Run:
```bash
ls -l site/assets/brand/ && file site/assets/brand/*.png
```
Expected: 7 files; the `.png` files report as PNG image data, the `.svg` files are text/SVG.

- [ ] **Step 4: Commit**

```bash
git add site/.nojekyll site/assets/brand
git commit -m "feat(site): scaffold site/ and vendor brand assets"
```

---

## Task 2: Vendor fonts and write `@font-face`

**Files:**
- Create: `site/assets/fonts/zilla-slab-700.woff2`
- Create: `site/assets/fonts/jetbrains-mono-400.woff2`
- Create: `site/assets/fonts/jetbrains-mono-500.woff2`
- Create: `site/assets/fonts/jetbrains-mono-700.woff2`
- Create: `site/assets/fonts/fonts.css`
- Create: `site/assets/fonts/OFL.txt`

- [ ] **Step 1: Download the woff2 files (latin subset) from the Fontsource CDN**

Fontsource serves stable, direct woff2 files (both fonts are OFL-licensed):
```bash
cd site/assets/fonts
curl -fL -o zilla-slab-700.woff2      "https://cdn.jsdelivr.net/fontsource/fonts/zilla-slab@latest/latin-700-normal.woff2"
curl -fL -o jetbrains-mono-400.woff2  "https://cdn.jsdelivr.net/fontsource/fonts/jetbrains-mono@latest/latin-400-normal.woff2"
curl -fL -o jetbrains-mono-500.woff2  "https://cdn.jsdelivr.net/fontsource/fonts/jetbrains-mono@latest/latin-500-normal.woff2"
curl -fL -o jetbrains-mono-700.woff2  "https://cdn.jsdelivr.net/fontsource/fonts/jetbrains-mono@latest/latin-700-normal.woff2"
cd -
```
If the sandbox blocks network access, fetch these on a connected machine and copy them in; the four filenames above are the contract the rest of the plan relies on.

- [ ] **Step 2: Verify the files are real woff2 (not HTML error pages)**

Run:
```bash
file site/assets/fonts/*.woff2
```
Expected: each reports `Web Open Font Format (Version 2)`. If any reports `HTML document`, the download failed — re-fetch.

- [ ] **Step 3: Write the `@font-face` stylesheet**

Create `site/assets/fonts/fonts.css`:
```css
/* Self-hosted fonts — no third-party CDN at runtime. Both OFL-licensed (see OFL.txt). */
@font-face{
  font-family:"Zilla Slab";
  font-style:normal; font-weight:700; font-display:swap;
  src:url("./zilla-slab-700.woff2") format("woff2");
}
@font-face{
  font-family:"JetBrains Mono";
  font-style:normal; font-weight:400; font-display:swap;
  src:url("./jetbrains-mono-400.woff2") format("woff2");
}
@font-face{
  font-family:"JetBrains Mono";
  font-style:normal; font-weight:500; font-display:swap;
  src:url("./jetbrains-mono-500.woff2") format("woff2");
}
@font-face{
  font-family:"JetBrains Mono";
  font-style:normal; font-weight:700; font-display:swap;
  src:url("./jetbrains-mono-700.woff2") format("woff2");
}
```

- [ ] **Step 4: Add the OFL license text**

Run:
```bash
curl -fL -o site/assets/fonts/OFL.txt "https://raw.githubusercontent.com/google/fonts/main/ofl/zillaslab/OFL.txt"
```
(If blocked, copy the SIL OFL 1.1 text in manually — it must be present for redistribution.)

- [ ] **Step 5: Commit**

```bash
git add site/assets/fonts
git commit -m "feat(site): vendor self-hosted fonts (Zilla Slab, JetBrains Mono)"
```

---

## Task 3: Write the requirements checker (the failing test)

**Files:**
- Create: `scripts/check-landing.py`

- [ ] **Step 1: Write the checker**

Create `scripts/check-landing.py`:
```python
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
        (">etchy<", "etchy wordmark"),
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
```

- [ ] **Step 2: Run it to confirm it FAILS (index.html not built yet)**

Run:
```bash
python3 scripts/check-landing.py
```
Expected: `LANDING CHECK: FAIL` with `site/index.html is missing` (exit code 1).

- [ ] **Step 3: Commit**

```bash
git add scripts/check-landing.py
git commit -m "test(site): add landing-page requirements checker"
```

---

## Task 4: Build `site/index.html`

**Files:**
- Create: `site/index.html`

- [ ] **Step 1: Write the page**

Create `site/index.html` with the following exact content (this is the approved mockup with the Google Fonts `<link>` replaced by the local `fonts.css`, plus a real `<head>` with meta/OG/favicons):

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>etchy — PCB visual & geometric diff</title>
<meta name="description" content="etchy is a fast, trustworthy, open-source PCB visual and geometric diff tool. Point it at two fab revisions and it shows — and measures — exactly what changed.">
<link rel="icon" href="assets/brand/etchy-favicon.svg" type="image/svg+xml">
<link rel="icon" href="assets/brand/etchy-favicon-32.png" sizes="32x32" type="image/png">
<link rel="apple-touch-icon" href="assets/brand/etchy-app-icon-180.png">
<meta property="og:type" content="website">
<meta property="og:title" content="etchy — PCB visual & geometric diff">
<meta property="og:description" content="Point etchy at two revisions of a board's fab output. It shows — and measures — exactly what changed.">
<meta property="og:image" content="assets/brand/etchy-social-1280x640.png">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:image" content="assets/brand/etchy-social-1280x640.png">
<link rel="stylesheet" href="assets/fonts/fonts.css">
<style>
  :root{
    --copper-1:#f6c873; --copper-2:#e8a33d; --copper-3:#b9762a;
    --board:#0b0f0e; --board-2:#121817; --paper:#f4f1e8;
    --added:#46d18a; --removed:#ff5d73;
    --ink:#e7e3d6; --muted:#8b9491; --line:#23302d;
  }
  *{box-sizing:border-box}
  html,body{margin:0;overflow-x:hidden}
  body{background:var(--board);color:var(--ink);font-family:"JetBrains Mono",ui-monospace,monospace;line-height:1.6;-webkit-font-smoothing:antialiased}
  .copper{background:linear-gradient(135deg,var(--copper-1),var(--copper-2) 50%,var(--copper-3));-webkit-background-clip:text;background-clip:text;color:transparent}
  .wrap{max-width:1080px;margin:0 auto;padding:0 24px}
  h1,h2,h3{font-family:"Zilla Slab",Georgia,serif;font-weight:700;letter-spacing:-.01em}
  nav{display:flex;align-items:center;justify-content:space-between;padding:22px 0;border-bottom:1px solid var(--line);gap:16px}
  .brand svg{width:38px;height:38px;display:block}
  .navlinks{display:flex;gap:26px;font-size:14px;color:var(--muted);flex-wrap:wrap;justify-content:flex-end}
  .navlinks a{color:var(--muted);text-decoration:none;white-space:nowrap}
  .navlinks a:hover{color:var(--copper-1)}
  .navlinks a.gh{color:var(--copper-1)}
  .hero{padding:80px 0 56px;display:grid;grid-template-columns:1.05fr .95fr;gap:48px;align-items:center}
  .badge{display:inline-flex;align-items:center;gap:8px;font-size:12px;letter-spacing:.06em;text-transform:uppercase;color:var(--copper-2);border:1px solid var(--copper-3);border-radius:999px;padding:5px 12px}
  .wordmark{font-family:"Zilla Slab";font-weight:700;font-size:76px;line-height:1;margin:22px 0 6px;letter-spacing:-.02em}
  .hero h1{font-size:26px;font-weight:700;margin:0 0 16px;color:var(--ink)}
  .hero p.lede{font-size:17px;color:var(--muted);max-width:46ch;margin:0 0 28px}
  .cta{display:flex;gap:14px;flex-wrap:wrap}
  .btn{font-family:"JetBrains Mono";font-size:14px;font-weight:500;text-decoration:none;padding:12px 20px;border-radius:8px}
  .btn-primary{background:linear-gradient(135deg,var(--copper-1),var(--copper-3));color:#1a1206;font-weight:700}
  .btn-ghost{border:1px solid var(--line);color:var(--ink)}
  .btn-ghost:hover{border-color:var(--copper-3)}
  .term{background:var(--board-2);border:1px solid var(--line);border-radius:12px;overflow:hidden;box-shadow:0 30px 60px -30px #000}
  .term-bar{display:flex;gap:7px;padding:12px 14px;border-bottom:1px solid var(--line)}
  .term-bar i{width:11px;height:11px;border-radius:50%;background:#2c3a37;display:block}
  .term-body{padding:18px;font-size:13.5px;overflow-x:auto}
  .term-body .pr{color:var(--copper-2)} .term-body .cmd{color:var(--ink)} .term-body .out{color:var(--muted)}
  .term-body .add{color:var(--added)} .term-body .rem{color:var(--removed)}
  section{padding:56px 0;border-top:1px solid var(--line)}
  .label{font-size:12px;letter-spacing:.14em;text-transform:uppercase;color:var(--copper-3)}
  .cards{display:grid;grid-template-columns:repeat(3,1fr);gap:20px;margin-top:26px}
  .card{background:var(--board-2);border:1px solid var(--line);border-radius:12px;padding:22px}
  .card h3{margin:6px 0 8px;font-size:19px}
  .card p{margin:0;color:var(--muted);font-size:14px}
  .card .ic{font-size:22px}
  .diffart{width:100%;height:auto;border-radius:12px;border:1px solid var(--line);background:var(--board-2)}
  .roadmap-head{display:flex;align-items:baseline;gap:14px;flex-wrap:wrap;margin:10px 0 6px}
  .roadmap-head h2{font-size:28px;margin:0}
  .roadmap-head .sub{color:var(--muted);font-size:14px}
  .road{display:grid;grid-template-columns:repeat(3,1fr);gap:16px;margin-top:24px}
  .road .item{border:1px solid var(--line);border-left:3px solid var(--copper-3);border-radius:10px;padding:18px;background:rgba(18,24,23,.5)}
  .road .item h3{font-size:16px;margin:0 0 6px;color:var(--copper-1);font-family:"Zilla Slab"}
  .road .item p{margin:0;color:var(--muted);font-size:13.5px}
  footer{padding:40px 0 60px;border-top:1px solid var(--line);color:var(--muted);font-size:13px;display:flex;justify-content:space-between;flex-wrap:wrap;gap:12px;align-items:center}
  footer a{color:var(--muted);text-decoration:none} footer a:hover{color:var(--copper-1)}
  @media(max-width:840px){
    .hero{grid-template-columns:1fr;padding-top:52px}
    .wordmark{font-size:60px}
    .cards{grid-template-columns:1fr}
    .road{grid-template-columns:1fr}
  }
  @media(max-width:560px){ .navlinks a.collapse{display:none} }
</style>
</head>
<body>
<div class="wrap">
  <nav>
    <div class="brand" aria-label="etchy">
      <svg viewBox="0 0 120 120" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="etchy logo"><defs><linearGradient id="cu" gradientUnits="userSpaceOnUse" x1="20" y1="18" x2="100" y2="102"><stop offset="0" stop-color="#f6c873"/><stop offset=".5" stop-color="#e8a33d"/><stop offset="1" stop-color="#b9762a"/></linearGradient></defs><g fill="url(#cu)"><rect x="24" y="26" width="16" height="17" rx="1.5"/><rect x="24" y="48" width="16" height="21" rx="1.5"/><rect x="24" y="74" width="16" height="20" rx="1.5"/><rect x="24" y="26" width="26" height="16" rx="1.5"/><rect x="55" y="26" width="19" height="16" rx="1.5"/><rect x="24" y="52" width="40" height="14" rx="1.5"/><rect x="24" y="78" width="26" height="16" rx="1.5"/><rect x="55" y="78" width="19" height="16" rx="1.5"/></g><circle cx="92" cy="86" r="9" fill="none" stroke="url(#cu)" stroke-width="1.8"/><circle cx="92" cy="86" r="3.6" fill="url(#cu)"/></svg>
    </div>
    <nav class="navlinks" aria-label="primary">
      <a class="collapse" href="#what">What it does</a>
      <a class="collapse" href="#roadmap">Roadmap</a>
      <a class="collapse" href="#">Docs</a>
      <a class="gh" href="#">GitHub ↗</a>
    </nav>
  </nav>

  <main>
  <div class="hero">
    <div>
      <span class="badge">Gerber · Excellon · same-board diff</span>
      <div class="wordmark copper">etchy</div>
      <h1>PCB visual &amp; geometric diff.</h1>
      <p class="lede">Point etchy at two revisions of a board's fab output. It shows — and measures — exactly what changed. Fast, trustworthy, open source, written in Rust.</p>
      <div class="cta">
        <a class="btn btn-primary" href="#">View on GitHub</a>
        <a class="btn btn-ghost" href="#">Read the docs</a>
      </div>
    </div>
    <div class="term">
      <div class="term-bar"><i></i><i></i><i></i></div>
      <div class="term-body">
        <div><span class="pr">$</span> <span class="cmd">etchy revA/ revB/</span></div>
        <div class="out">&nbsp;</div>
        <div class="out">resolving 16 copper layers … ok</div>
        <div class="out">diffing geometry … done in 0.9s</div>
        <div class="out">&nbsp;</div>
        <div><span class="add">+ 4 regions added</span>&nbsp;&nbsp;<span class="rem">- 1 region removed</span></div>
        <div class="out">→ report.html · overlay.svg · diff.json</div>
      </div>
    </div>
  </div>

  <section id="what">
    <span class="label">What it does</span>
    <div class="cards">
      <div class="card"><div class="ic copper">◳</div><h3>Visual + geometric</h3><p>Per-layer polygon boolean diff → a resolution-independent SVG overlay, a change heatmap, and measured magnitudes (changed area, region count) — all from one computation.</p></div>
      <div class="card"><div class="ic copper">⎙</div><h3>See it &amp; export it</h3><p>Native viewer with overlay / before / after / split / swipe / onion modes + heatmap. Exports a self-contained HTML report, standalone SVGs, and machine-readable JSON.</p></div>
      <div class="card"><div class="ic copper">✓</div><h3>Trustworthy</h3><p>Same-board revisions only. Fails loud on mismatch, never a garbage diff. Golden corpus + property + fuzz tests. No silent misses.</p></div>
    </div>
  </section>

  <section>
    <span class="label">The diff, visualized</span>
    <h2 style="font-size:28px;margin:10px 0 22px">Added in <span style="color:var(--added)">green</span>, removed in <span style="color:var(--removed)">red</span>.</h2>
    <svg class="diffart" viewBox="0 0 1000 280" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="Example diff overlay: traces added in green, removed in red.">
      <rect width="1000" height="280" fill="#0e1413"/>
      <g stroke="#33403d" stroke-width="6" fill="none" stroke-linecap="round">
        <path d="M60 60 H400 V150 H700"/><path d="M60 120 H300 V220 H620"/>
        <path d="M120 250 H500 V90 H880"/><circle cx="880" cy="90" r="14"/>
        <circle cx="60" cy="60" r="10"/><circle cx="700" cy="150" r="10"/>
      </g>
      <g stroke="#46d18a" stroke-width="7" fill="none" stroke-linecap="round">
        <path d="M700 150 H840 V210 H940"/><circle cx="940" cy="210" r="13"/>
      </g>
      <g stroke="#ff5d73" stroke-width="7" fill="none" stroke-linecap="round">
        <path d="M300 220 H620"/>
      </g>
      <text x="30" y="30" fill="#8b9491" font-family="JetBrains Mono" font-size="14">overlay.svg — top copper</text>
    </svg>
  </section>

  <section id="roadmap">
    <span class="label">On the roadmap</span>
    <div class="roadmap-head">
      <h2>Where etchy is headed.</h2>
      <span class="sub">Direction, not promises — shipped when it's ready.</span>
    </div>
    <div class="road">
      <div class="item"><h3>CI gating &amp; PR comments</h3><p>Per-layer thresholds, exit codes, and an inline visual diff posted on pull requests.</p></div>
      <div class="item"><h3>Schematic-PDF diff</h3><p>Page-by-page pixel diff of schematic PDFs, as a supported secondary mode.</p></div>
      <div class="item"><h3>Dense-board speed</h3><p>Parallel per-layer work and memory ceilings for big multi-layer boards.</p></div>
      <div class="item"><h3>Static binaries + container</h3><p>Per-OS static builds and a distroless container — no runtime to install.</p></div>
      <div class="item"><h3>Docs site</h3><p>Install, CLI reference, CI recipes, JSON schema, and how the diff works.</p></div>
      <div class="item"><h3>Stable JSON schema</h3><p>A versioned, documented output contract for downstream integrations.</p></div>
    </div>
  </section>
  </main>

  <footer>
    <svg width="22" height="22" viewBox="0 0 120 120" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><defs><linearGradient id="cf" gradientUnits="userSpaceOnUse" x1="20" y1="18" x2="100" y2="102"><stop offset="0" stop-color="#f6c873"/><stop offset=".5" stop-color="#e8a33d"/><stop offset="1" stop-color="#b9762a"/></linearGradient></defs><g fill="url(#cf)"><rect x="24" y="26" width="16" height="17" rx="1.5"/><rect x="24" y="48" width="16" height="21" rx="1.5"/><rect x="24" y="74" width="16" height="20" rx="1.5"/><rect x="24" y="26" width="26" height="16" rx="1.5"/><rect x="55" y="26" width="19" height="16" rx="1.5"/><rect x="24" y="52" width="40" height="14" rx="1.5"/><rect x="24" y="78" width="26" height="16" rx="1.5"/><rect x="55" y="78" width="19" height="16" rx="1.5"/></g><circle cx="92" cy="86" r="9" fill="none" stroke="url(#cf)" stroke-width="1.8"/><circle cx="92" cy="86" r="3.6" fill="url(#cf)"/></svg>
    <div>MIT / Apache-2.0 · open source · © Cimos</div>
    <div><a href="#">GitHub</a> · <a href="#roadmap">Roadmap</a> · <a href="#">Docs</a> · <a href="#">gerber-diff</a></div>
  </footer>
</div>
</body>
</html>
```

- [ ] **Step 2: Run the checker to verify it PASSES**

Run:
```bash
python3 scripts/check-landing.py
```
Expected: `LANDING CHECK: PASS` (exit code 0).

- [ ] **Step 3: Eyeball it in a browser**

Open `site/index.html` in a browser. Confirm: copper wordmark + icon render, terminal block shows green/red lines, three cards, the diff SVG, the roadmap grid, footer. No console 404s for fonts or images.

- [ ] **Step 4: Commit**

```bash
git add site/index.html
git commit -m "feat(site): build landing page (M1 framing + roadmap)"
```

---

## Task 5: GitHub Actions Pages deploy workflow

**Files:**
- Create: `.github/workflows/pages.yml`

- [ ] **Step 1: Write the workflow (official Actions, pinned by tag for now)**

Create `.github/workflows/pages.yml`:
```yaml
name: Deploy landing page

on:
  push:
    branches: [main]
    paths:
      - "site/**"
      - ".github/workflows/pages.yml"
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: false

jobs:
  deploy:
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: site
      - id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 2: Pin every Action to a commit SHA (per the third-party-CI rule)**

Resolve the current commit SHA for each tag and rewrite the file in place. This satisfies the SHA-pin requirement without hand-copying SHAs:
```bash
pin() { # pin <owner/repo> <tag>
  sha=$(gh api "repos/$1/commits/$2" --jq .sha)
  sed -i "s#uses: $1@$2#uses: $1@${sha} # $2#" .github/workflows/pages.yml
}
pin actions/checkout v4
pin actions/configure-pages v5
pin actions/upload-pages-artifact v3
pin actions/deploy-pages v4
```

- [ ] **Step 3: Verify the workflow is valid YAML and is fully pinned**

Run:
```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/pages.yml')); print('yaml ok')"
grep -E 'uses:' .github/workflows/pages.yml
```
Expected: `yaml ok`, and every `uses:` line ends with a 40-char SHA followed by a `# vX` comment (no bare `@vN` tags remain).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/pages.yml
git commit -m "ci(site): add SHA-pinned GitHub Pages deploy workflow"
```

---

## Task 6: Final verification and PR

- [ ] **Step 1: Re-run the checker from a clean state**

Run:
```bash
python3 scripts/check-landing.py
```
Expected: `LANDING CHECK: PASS`.

- [ ] **Step 2: Verify fonts load offline (self-hosted requirement)**

In the browser devtools Network tab, reload `site/index.html` and confirm the four `*.woff2` requests resolve from the local `assets/fonts/` path and there are **zero** requests to `fonts.googleapis.com` / `fonts.gstatic.com`.

- [ ] **Step 3: Verify mobile layout**

In devtools device mode at 375px width: no horizontal scroll; nav shows only the **GitHub ↗** link; hero, cards, and roadmap stack to one column.

- [ ] **Step 4: Confirm no test artifacts are in the deploy folder**

Run:
```bash
test ! -e site/test_site.py && find site -name "*.py" -print
```
Expected: no Python files printed under `site/` (the checker lives in `scripts/`).

- [ ] **Step 5: Push the branch and open a PR**

```bash
git push -u origin landing-site
gh pr create --title "Landing page (GitHub Pages)" \
  --body "Implements docs/superpowers/plans/2026-06-17-landing-page.md. Static site/ + SHA-pinned Pages workflow. Dormant until the repo is public and Pages is enabled."
```

---

## Launch-day checklist (NOT part of this build — do when going public)

These are the spec's open items; they require the repo to be public and decisions made:

- [ ] Decide custom domain vs `cimos.github.io/etchy/`. If a path-prefixed Pages URL is used, confirm the relative asset paths in `index.html` resolve (they are already relative, so `/etchy/` works). If a custom domain is chosen, add `site/CNAME` + DNS.
- [ ] Replace the placeholder `href="#"` links (View on GitHub, Docs, gerber-diff, GitHub footer) with real URLs.
- [ ] Land `assets/brand/` on `main` (from `gui-brand-theme`) — or keep the vendored `site/assets/brand/` copies as the source of truth.
- [ ] Enable Pages: Settings → Pages → Source: **GitHub Actions**.
- [ ] Set the repo Social preview image (Settings → General) to `etchy-social-1280x640.png`.
- [ ] Re-check the roadmap/status copy reflects the actual shipped feature set at launch.

---

## Self-review notes

- **Spec coverage:** in-repo `/site` (T1), self-hosted fonts (T2), no-JS single page with all six sections + M1 framing + undated roadmap (T4), GitHub Actions deploy with official SHA-pinned least-privilege Actions (T5), favicons/OG/social wired (T4), launch open-items captured (checklist). Verification items map to the spec's Verification section (T6).
- **Placeholder scan:** the only `href="#"` values are intentional and explicitly tracked in the launch checklist; all code/content is complete.
- **Type/name consistency:** font filenames in T2 (`zilla-slab-700.woff2`, `jetbrains-mono-{400,500,700}.woff2`) match `fonts.css` and the T3 checker's woff2 existence check; `fonts.css` path matches the `<link>` in T4; brand filenames in T1 match the `<head>` references in T4; `scripts/check-landing.py` path is consistent across T3/T4/T6.
