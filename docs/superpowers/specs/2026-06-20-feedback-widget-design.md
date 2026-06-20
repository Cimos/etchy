# etchy feedback widget — design (dev/review tool)

**Date:** 2026-06-20
**Status:** Approved design, lightweight build
**Scope:** A dev/review-only feedback widget embedded in the etchy landing page. Lets Simon leave comments + screenshots while reviewing the page locally; submissions flow into the existing feedback-loop backend and wake the agent.

## Decisions (from brainstorming)

| Decision | Outcome |
|---|---|
| Screenshot capture | **Paste (Ctrl+V) / drag-drop / browse** — no JS dependency, lets him annotate first. |
| Public-site separation | **Not a concern right now** (site isn't live). Widget goes directly into `site/index.html`, flagged DEV-ONLY for removal/gating before going public. |
| Backend | **Reuse** the feedback-loop-etchy server at `localhost:8770/submit` (same `feedback.jsonl` + `uploads/` + wake signal). |

## Components

- **`site/feedback-widget.js`** — the entire widget (button, overlay, capture, submit) as one self-contained IIFE that injects its own DOM + scoped CSS. No external deps.
- **`site/index.html`** — one clearly-marked block near `</body>`:
  ```html
  <!-- DEV-ONLY feedback widget — REMOVE before public launch (points at localhost:8770). -->
  <script src="feedback-widget.js" data-feedback-endpoint="http://localhost:8770/submit"></script>
  ```
  Removal = delete the file + this block.

## UI

- Floating round button, fixed bottom-right ("💬 Feedback"), copper accent on board-dark.
- Click → overlay panel (does not block the page; dismissible via ✕ or Esc):
  - **Comment / suggestion** textarea (required to enable submit).
  - **Your name** text field — persisted in `localStorage` (`etchy_fb_name`) and pre-filled next time.
  - **Screenshot area** — paste, drag-drop, or browse; lists thumbnails with per-item remove. Accepts images.
  - **Submit** button + status line; **Esc** closes, **Ctrl/Cmd+Enter** submits.
- Styled with scoped class names (`efb-*`) and high `z-index` so it never collides with page styles.

## Data flow

On submit, build a payload and POST to the endpoint from `data-feedback-endpoint`:
```json
{
  "source": "etchy-widget",
  "page": { "url": "<location.href>", "title": "<document.title>" },
  "username": "<name>",
  "comment": "<textarea>",
  "attachments": [ { "name": "...", "type": "image/png", "size": 1234, "data": "data:image/png;base64,..." } ],
  "viewport": { "w": <innerWidth>, "h": <innerHeight> },
  "ts": "<ISO string>"
}
```
- Shape matches what `feedback-loop` `server.py` already consumes: it base64-decodes `attachments[].data` into `uploads/<stamp>/`, appends the record to `feedback.jsonl`, and atomically rewrites `state/last_submission.json` (the wake signal).
- **Cross-origin handling:** page is `:8000`, backend `:8770`. The widget sends the body as `text/plain` with `fetch(..., { mode: "no-cors" })` — a CORS "simple request", so no preflight and no change to the shared `server.py`. The response is opaque (fine: fire-and-forget). The widget optimistically shows "Saved ✓"; on a network error (server down) it shows a retry message.

## Error handling

- Submit disabled until the comment is non-empty.
- Files that fail to read are skipped; the rest still submit.
- Network failure → status shows "Couldn't reach the feedback server (is it running on :8770?)"; button re-enabled.

## Out of scope

- Production/public hosting of the widget, on-image annotation, authentication, editing past submissions.

## Testing / verification

- `python3 scripts/check-landing.py` still passes (no font-CDN added).
- Manual: button appears bottom-right; click opens the panel; paste + drag + browse each add a thumbnail; submit with a comment + image lands a new line in `feedback-loop-etchy/feedback.jsonl` and a file under `feedback-loop-etchy/uploads/`, and bumps `state/last_submission.json` (the armed watcher fires).
- Esc closes; name persists across reloads.

## Launch checklist addition

- [ ] **Strip or gate the dev feedback widget** (`feedback-widget.js` + its `<script>` block) before the site goes public — it targets `localhost:8770` and must not ship to visitors.
