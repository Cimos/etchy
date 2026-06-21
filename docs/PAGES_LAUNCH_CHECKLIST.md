# etchy Pages — launch checklist

The landing site lives in `site/` and deploys to GitHub Pages via
`.github/workflows/pages.yml`. It is on `main` but **dormant**: the repo is
private and Pages is not enabled, so the deploy workflow fails on pushes that
touch `site/**` (expected — it goes green once Pages is enabled).

## Go-public steps (do these when launching)

- [ ] Make the repo public.
- [ ] Enable Pages: **Settings → Pages → Source: GitHub Actions**.
- [ ] Set the repo **Social preview** image to `site/assets/brand/etchy-social-1280x640.png` (Settings → General).
- [ ] Re-check the page copy matches the **actually-shipped** feature set at launch
      (shipped GUI viewer modes today: **Overlay / Before / After** — everything else stays on the roadmap).
- [ ] If a custom domain is used instead of `cimos.github.io/etchy/`: add `site/CNAME`
      and update the absolute `og:url` / `og:image` base in `site/index.html`.

## Decided (no action needed)

- **URL:** default `https://cimos.github.io/etchy/` (no custom domain).
- **Brand assets:** vendored in `site/assets/brand/` as the source of truth — no separate merge required.
- **Fonts:** self-hosted in `site/assets/fonts/` (no CDN at runtime).
- **GUI demo** (`site/gui-demo.js`): shipped as a real feature; it adds JS to the otherwise-static page.

## Dev-only tooling — NOT for the public site

- The in-page **feedback widget** (`feedback-widget.js` + its `<script>` include) is a
  dev/review tool that posts to a local feedback server (`localhost`). It lives only on the
  `feedback-widget` branch and is intentionally **not** on `main`. Do not add it to the public site.

## Verify before any deploy

- `python3 scripts/check-landing.py` → `LANDING CHECK: PASS`.
