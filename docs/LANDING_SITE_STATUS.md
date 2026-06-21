# etchy landing + docs site — status & handoff

_Snapshot for picking this back up later. All review **feedback** has been actioned;
what remains below is **deferred work, tooling, and open decisions** — not unfinished feedback._

## Done (on `main`)

The public site is complete and on `main` (PRs #3–#6, #8, #9, #11):

- **`site/index.html`** — landing page (Direction A, dark board). Icon-only header, stencil-E
  mark-as-"e" wordmark, plain-language cards, CSS-only **CLI/GUI tabs** with an interactive
  **GUI demo** (`site/gui-demo.js`), repo badges, **Get-it** section (framed "at first release"),
  a **real Mad_RP2040 diff example** (real etchy numbers, Rev_A→v0.0.1), undated roadmap.
- **`site/docs.html`** — styled docs page: overview · install (**tabbed**, Windows default) ·
  CLI usage + exit codes · how-the-diff-works · real `--json` schema · CI (roadmap) · viewer ·
  trust/limitations. Shares the landing shell (same header/width/footer); nav = plain **Home** + GitHub.
- **Self-hosted fonts** (`site/assets/fonts/`), **vendored brand** (`site/assets/brand/`),
  **SHA-pinned Pages deploy workflow** (`.github/workflows/pages.yml`), and a checker
  (`scripts/check-landing.py` → `LANDING CHECK: PASS`).
- Copy is matched to the **real CLI** (summary + `--json`; viewer modes Overlay/Before/After only).

## Outstanding / deferred

1. **Go-public checklist** (`docs/PAGES_LAUNCH_CHECKLIST.md` on `main`) — intentionally deferred:
   enable Pages (Settings → Pages → Source: GitHub Actions), set the repo Social-preview image,
   re-check copy vs shipped features at launch. The Pages workflow is **dormant** and shows a red
   run on `main` (no Pages site yet) — expected until enabled. URL decided: `cimos.github.io/etchy/`.
2. **Dev feedback widget + LAN tooling live only on the `feedback-widget` branch (NOT `main`):**
   `site/feedback-widget.js` (the one shared widget: sentiment/category/severity + comment + name +
   screenshots, copper-themed), its `<script>` include in index/docs, `scripts/site-server.py`
   (serves `site/` + same-origin `POST /submit`, binds 0.0.0.0 for LAN), and `scripts/lan-share.sh`
   (up/down/check — adds a Windows portproxy + firewall rule so LAN teammates can submit).
   **Open decision:** does the widget ever ship on the public site? Today it's dev/LAN-only and is
   stripped from every `main` PR.
3. **Widget consolidation is half-done.** The landing/docs use the shared `feedback-widget.js`. The
   **app demo on `gui-web-wasm` (`deploy/demo/index.html`) still has its own inline widget** — it was
   NOT migrated to the shared file. Finishing consolidation = point the app demo at `feedback-widget.js`.
4. **Real example is static numbers,** not a live SVG overlay — the CLI emits a summary + `--json`,
   no SVG file yet. Upgrade to an embedded real overlay once the CLI/engine exposes SVG output.
   (Mad_RP2040 carries a kept `v0.0.0` tag at 650652b, created to build that revision's datapack.)
5. **Branch cleanup:** these merged branches can be deleted — `landing-site`, `landing-rev`,
   `landing-rev2`, `docs-updates`, `docs-homelink`, `docs-insttabs`, `docs-match`.

## Not ours — parallel session (do not touch without checking)

- Open PRs **#7** and **#10** (`gui-web-wasm` / `gui-web-wasm-v2`) are the etchy GUI/web-demo
  workstream. **#10's title mentions "landing site"** — possible overlap with this work; reconcile
  before merging either side.
- Feedback from other agent keys (`2726bf47`, `7b4a9031`, …) in `feedback-loop/feedback.jsonl`
  (the `G5/G7c/G9/G10`, heatmap, zoom items) belongs to that workstream, not the landing page.

## How to resume the LAN review setup (ephemeral — dies with the session)

```bash
# from this worktree on the feedback-widget branch:
scripts/lan-share.sh up      # starts site+feedback server (:8000) + LAN portproxy/firewall
scripts/lan-share.sh check   # preview (IPs, URL) without changes
scripts/lan-share.sh down    # stop + remove portproxy/firewall
```
- The review feedback server `feedback-loop/` (:8765) auto-starts via the `~/.claude/settings.json`
  SessionStart hook. The old per-project instances (`feedback-loop-etchy` :8770, `feedback-loop-landing`
  :8771) are **superseded** — ignore them; the canonical install is `feedback-loop/` (:8765).
- Widget submissions land in `feedback-loop/feedback.jsonl` + `uploads/`, keyed `landing-2f998112`.

## Branch map

- **`main`** — public site (no dev widget).
- **`feedback-widget`** — `main` content + dev widget + LAN tooling + this doc. Diverged from `main`
  (missing the squash-merged `PAGES_LAUNCH_CHECKLIST.md`); reconcile if ever folding the widget in.
