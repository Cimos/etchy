# deploy/feedback/ — collected demo feedback (development stage)

One `*.jsonl` file per host that ran the demo, committed so tester feedback is
**preserved and shared** across dev machines during development. Each line is one
submission: server timestamp, client IP, User-Agent, plus the widget payload
(`sentiment`, `category`, `severity`, `name`, `text`, and `ctx` = screen/viewport/
url/app-state at submit time).

- **`<hostname>.jsonl`** — written live by `etchy-server.py` when launched via
  `deploy/setup.sh` (which sets `$ETCHY_FEEDBACK` to point here). Commit yours so
  the team sees it.
- **`m1-seed.jsonl`** — the first round of feedback (M1 demo, 18 Jun 2026).
- Merge/triage everything with **`python3 deploy/collect-feedback.py`**.

> ⚠ **These records contain IPs, User-Agents and tester names (PII).** They are
> kept during development on purpose, but **must be scrubbed before this repo goes
> public** — see [`/PRE_PUBLIC.md`](../../PRE_PUBLIC.md).
