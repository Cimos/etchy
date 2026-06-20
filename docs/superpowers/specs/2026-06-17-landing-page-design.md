# etchy landing page — design

**Date:** 2026-06-17
**Status:** Approved design, pending implementation plan
**Owner:** Cimos (Simon)

## Goal

A single-page marketing/landing site for **etchy**, hosted on GitHub Pages,
built and committed now so it is **launch-ready when `Cimos/etchy` is made
public**. The repo stays private until etchy is genuinely user-friendly (at least
the **Milestone 1 MVP**, likely more); GitHub Pages on a free plan only publishes
from a public repo, so the site sits dormant in-repo and goes live with the
public flip — site and code launch as one event.

**The page describes the first public release (M1 MVP) as a working tool —
present tense, no "in development" framing.** By the time it's public, M1 is done,
so the old Phase-0 / "building in the open" status framing would be stale and
would undersell. A separate "On the roadmap" section lists what's coming next as
**direction, with no dates and no version numbers**.

Tone: this is free, open-source developer tooling, not a product launch. Plain,
descriptive copy — no slogans or growth-hacky CTAs. The design follows the
existing brand identity rather than inventing a new look.

## Scope

**In scope**
- One page: nav → hero → "what it does" → diff visualization → **roadmap** → footer.
- Copy describes **M1 MVP capabilities** in present tense (see First-release feature set).
- Static HTML + CSS, **no JavaScript**, **no build step / no SSG**.
- Self-hosted fonts and brand assets (no third-party CDN at runtime).
- GitHub Actions workflow to deploy `/site` to GitHub Pages.

**Out of scope (deferred, structure not built now)**
- Rendered docs pages (the `docs/*.md` planning docs stay as-is).
- A live diff demo / SVG gallery (real tool output, not illustrative art).
- Any analytics, cookies, or tracking.

## First-release feature set (what the page presents as available)

The first public release targets **Milestone 1 (MVP)**. Present-tense capabilities:
- **Visual + geometric diff** of Gerber RS-274X/X2 and Excellon — per-layer
  polygon boolean diff → SVG overlay, change heatmap, magnitudes (changed area,
  region count).
- **Native viewer** (egui) with overlay / before / after / split / swipe / onion
  modes + heatmap, changed-layers-first, pan/zoom/fit.
- **CLI** `etchy <old> <new>` with a rich terminal summary and `--json`.
- **Outputs:** self-contained **HTML report**, standalone **SVG** overlays,
  machine-readable **JSON**.
- **Trustworthy:** same-board guard (fail-loud on mismatch), rename-tolerant layer
  pairing, golden corpus + property + fuzz tests.

**Roadmap section (direction only — no dates, no versions):** CI gating & PR
comments · schematic-PDF diff · dense-board performance · static binaries +
distroless container · docs site · stable versioned JSON schema. Tone: intent,
not promises ("where etchy is headed", "shipped when it's ready").

## Decisions locked during brainstorming

| Decision | Outcome |
|---|---|
| Direction | **A — "Dark board"**: board-dark background, developer-tool feel, terminal hero. (Rejected B — light/paper.) |
| Nav logo | **Icon only** (the stencil-"E" mark). The lowercase "etchy" wordmark appears large in the hero instead — avoids the icon+wordmark "double-letter" stutter. |
| Wordmark casing | **Lowercase "etchy"** per brand guidelines; matches the CLI command `etchy old/ new/`. No capital "Etchy" variant. |
| Headline | Plain descriptive: **"PCB visual & geometric diff."** No slogan. |
| Hero badge | Factual capability tag **"Gerber · Excellon · same-board diff"** — no "in development" / Phase-0 status. |
| First-release target | **M1 MVP**, presented in present tense as a working tool. |
| Status framing | **No "building in the open" status section.** Replaced by an **"On the roadmap"** section — direction only, **no dates, no version numbers**. |
| CTAs | Two neutral buttons: **View on GitHub**, **Read the docs**. No "★ Star". |
| gerber-diff | Demoted to a single **footer link** (predecessor); no longer a "use it today" message anywhere. |
| Diff art strokes | Added = solid green, removed = **solid red** (matched). Semantic dashing belongs in the real tool's output, not marketing art. |
| Site location | **Inside `Cimos/etchy`**, in a top-level `/site` directory. |
| Deploy | **GitHub Actions** → Pages (legacy root/`docs` source can't serve `/site`). |

## Look & feel

From `assets/brand/` (see Assets section for the merge note):

- **Palette:** copper gradient `#f6c873 → #e8a33d → #b9762a`; board dark `#0b0f0e`;
  panel `#121817`; ink `#e7e3d6`; muted `#8b9491`; hairline `#23302d`.
  Diff accents: added `#46d18a`, removed `#ff5d73`.
- **Type:** Zilla Slab 700 (wordmark + headings), JetBrains Mono (body, code, nav).
- **Mark:** stencil capital "E" + fiducial dot, on the copper gradient. Inlined as
  SVG in the nav, hero-adjacent, and footer.

**Page sections (top to bottom):**
1. **Nav** — icon-only mark left; right links (What it does · Roadmap · Docs · GitHub).
   On phones (≤560px) only GitHub shows, to prevent horizontal overflow.
2. **Hero** — two columns (stacks on mobile). Left: "Gerber · Excellon · same-board
   diff" badge, large **etchy** wordmark, "PCB visual & geometric diff." subhead,
   lede paragraph, two CTAs. Right: a faux terminal showing `etchy revA/ revB/` and
   its added/removed summary + output files (`report.html · overlay.svg · diff.json`).
3. **What it does** — three cards: Visual + geometric · See it & export it (native
   viewer modes + heatmap; HTML/SVG/JSON) · Trustworthy.
4. **The diff, visualized** — an inline SVG of copper traces with added-green and
   removed-red segments. (Illustrative, not real tool output.)
5. **On the roadmap** — heading "Where etchy is headed." + "Direction, not promises
   — shipped when it's ready." A grid of items (CI gating & PR comments ·
   schematic-PDF diff · dense-board speed · static binaries + container · docs site
   · stable JSON schema). **No dates, no version numbers**; muted/secondary visual
   weight so it reads as intent, not commitment.
6. **Footer** — small mark, license (MIT / Apache-2.0 · © Cimos), links incl. a
   subtle gerber-diff (predecessor) link.

The approved reference mockup is `etchy-landing-final-v2.html` (delivered during
brainstorming). The implementation reproduces it, swapping the Google Fonts CDN
`<link>` for self-hosted `@font-face`.

## Architecture / file layout

```
etchy/
  site/
    index.html              # the page — styles stay inline in <style> (single file, no separate CSS)
    assets/
      fonts/                # vendored woff2: Zilla Slab 700, JetBrains Mono 400/500/700
      brand/                # favicons, social card, app icons, the mark SVG
    favicon.ico             # + linked PNG/SVG favicons in <head>
    CNAME                   # only if a custom domain is chosen (see open items)
  .github/workflows/
    pages.yml               # build/deploy workflow (new)
```

Self-contained: a reviewer can open `site/index.html` locally and see the page
(fonts and assets resolve via relative paths). The unit is one HTML document plus
a flat asset folder — easy to understand and change without side effects.

## Build & deploy

GitHub Actions, because Pages' legacy source only serves the repo root or
`/docs`, and we want `/site`.

`.github/workflows/pages.yml`:
- **Trigger:** push to `main` touching `site/**` or the workflow; plus
  `workflow_dispatch`.
- **Permissions (least privilege):** `pages: write`, `id-token: write`,
  `contents: read`. Nothing else.
- **Concurrency:** one `pages` deployment at a time.
- **Steps (official actions only, SHA-pinned):**
  `actions/checkout` → `actions/configure-pages` →
  `actions/upload-pages-artifact` (path `site/`) → `actions/deploy-pages`.
- No third-party Actions. Each pinned to a reviewed commit SHA with a `# vX.Y.Z`
  comment, per the project's third-party-CI rule.

The workflow is committed now but only produces a live site once the repo is
public and Pages is enabled (Settings → Pages → Source: GitHub Actions).

## Assets

Brand assets currently live on the **`gui-brand-theme`** branch
(`assets/brand/`), **not on `main`**. Before launch they must be on `main` (or
the site must vendor a snapshot). The site copies the files it needs into
`site/assets/brand/`:
- Favicons (16/32/48/64 PNG + scalable SVG), apple-touch (180), app icons.
- `etchy-social-1280x640.png` wired as `og:image` / `twitter:image`, and set as
  the repo's Social preview.
- The mark SVG, inlined into the page.

**Fonts:** vendor `woff2` for Zilla Slab 700 and JetBrains Mono 400/500/700 into
`site/assets/fonts/` and declare `@font-face` (both fonts are OFL-licensed, so
redistribution is fine — include the license file). This removes the runtime
dependency on Google Fonts (privacy + offline resilience).

## `<head>` / SEO / a11y

- `<title>`, meta description, Open Graph + Twitter card tags using the social PNG.
- Favicon link set.
- Semantic landmarks (`<nav> <main> <footer>`), one `<h1>`, the logo SVG carries
  an accessible label, decorative SVGs `aria-hidden`. Color contrast meets WCAG AA
  on the dark background.
- No JS, so no client-side failure modes; fully functional with scripting off.

## Open items (resolve before / at launch — not blockers for building)

1. **Custom domain?** Default URL will be `https://cimos.github.io/etchy/`. If a
   custom domain is wanted, add a `CNAME` file + DNS. (Affects asset paths — use
   root-relative `/` only with a custom domain or a user/org Pages repo; otherwise
   use relative paths because of the `/etchy/` path prefix.)
2. **Brand-asset merge:** land `assets/brand/` on `main` (from `gui-brand-theme`)
   before the public flip.
3. **Real GitHub/Docs link targets** once public (repo URL, docs destination).
4. **Repo Social preview** image set in Settings after public.

## Verification (at implementation time)

- Open `site/index.html` locally → renders correctly, fonts load from local files
  (verify with network throttled / offline).
- Mobile widths (≤375px): no horizontal scroll; nav shows only GitHub.
- HTML validates; links resolve; Lighthouse a11y/SEO sane.
- `pages.yml` runs green on a public test (or dry-run via `workflow_dispatch`).
