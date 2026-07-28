# deploy/feedback/ — collected demo feedback (development stage)

One `*.jsonl` file per host that ran the demo. Each line is one submission: server
timestamp plus the widget payload (`sentiment`, `category`, `severity`, `name`,
`text`, and `ctx` = screen/viewport/url/app-state at submit time). The server also
records the client IP and User-Agent in the files it writes locally.

- **`<hostname>.jsonl`** — written live by `etchy-server.py` when launched via
  `deploy/setup.sh` (which sets `$ETCHY_FEEDBACK` to point here). These stay
  **local and gitignored**.
- **`m1-seed.jsonl`** — the first round of feedback (M1 demo, 18 Jun 2026). The
  only tracked log; its identifying fields (IP, User-Agent, tester name) have been
  removed and the demo URL genericised. The feedback text is intact.
- Merge/triage everything with **`python3 deploy/collect-feedback.py`**.

> ⚠ **Live logs contain IPs, User-Agents and tester names (PII), so they are
> gitignored — don't commit one.** If a log has to be shared, strip those fields
> first, the way `m1-seed.jsonl` is stripped.
