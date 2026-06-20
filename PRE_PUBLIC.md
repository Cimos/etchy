# ⚠ Before this repo goes public

etchy is intended to be open-source, but during development it carries data that
**must be removed before the repository is made public**. Work through this list
first.

## Must remove / scrub

- [ ] **Demo feedback PII.** `deploy/feedback/*.jsonl` records contain client
      **IP addresses, User-Agent strings, and tester names**. Either delete these
      files, or scrub the `ip`, `ua`, and `name` fields, before publishing. (They
      are kept in-repo on purpose during development so the team shares feedback —
      see `deploy/feedback/README.md`.) Remember git history retains them: scrub
      *and* rewrite history (e.g. `git filter-repo`) if they were ever committed.

## Check before publishing

- [ ] Search history for any other private data (hostnames, internal IPs, LAN
      URLs in `deploy/feedback/`, paths with usernames).
- [ ] Confirm no confidential boards / fab data are committed.

> Add to this list as the project grows. Keep it at the repo root so it's the
> obvious last gate before flipping visibility.
