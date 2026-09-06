# ⚠ Before this repo goes public

etchy is intended to be open-source, but during development it accumulated some
data and internal references that **must be dealt with before the repository is
made public**. This is the last gate before flipping visibility — work through
it top to bottom. Grouped by severity; each item notes whether it's an owner
decision (the owner) or a mechanical fix.

Status of this list is from the 2026-07-13 go-public audit (four-dimension
sweep: licence, confidential data/secrets, README/docs, repo hygiene). The good
news up front: **no secrets in the working tree or the full git history, and no
private-board feedback screenshots are committed** — `.gitignore` correctly
excludes `deploy/feedback/` screenshots and uploads.

---

## 1. Blockers — must resolve before flipping visibility

- [x] **Confidential customer board identifiers — working tree ✅, history ✅.**
      The board *files* were never committed, but docs prose and a few code
      comments named a confidential customer board, its revisions, a part number,
      a second internal board, and absolute local paths. All of that is now
      generic in the current tree ("the real board", "rev A"/"rev B",
      `<local-path>/…`), and the one board-specific review spec was deleted
      (see #264 — the term list is deliberately kept there, not in this file).
      History was rewritten with `git filter-repo` on 2026-07-13 (content, commit
      messages, and the board-named file purged from every commit) and
      force-pushed across all branches and tags. Verified from a fresh clone:
      zero hits in blobs, commit messages, paths and ref names. GitHub PR/issue
      titles, bodies and comments were scrubbed through the API in the same pass.
      To re-check, take the term list from #264 as `$TERMS` and run
      `grep -rinE "$TERMS" docs/ crates/ deploy/ *.md`.
- [x] **Committed feedback PII — working tree ✅, history ✅.**
      `deploy/feedback/m1-seed.jsonl` (git-tracked, whitelisted past the `*.jsonl`
      ignore) carried internal LAN IPs, User-Agent strings and tester names. Those
      fields are removed and the demo URL is genericised; the feedback text is
      kept. No board images were ever committed, and live feedback logs stay
      gitignored. The original values were removed from past commits by the same
      history rewrite.
- [ ] **Ask GitHub to garbage-collect the pre-rewrite objects — OWNER ACTION,
      still open.** A history rewrite plus force-push makes the old commits
      unreachable, but it does **not** delete them from GitHub: they remain
      retrievable by SHA through the REST API (verified — a pre-rewrite commit
      still served its old `docs/HANDOFF.md` with the identifiers in it) until
      GitHub garbage-collects the repository. Old SHAs are discoverable from PR
      timelines and the events API, so this matters at the moment of going public,
      not before. Open a GitHub Support request asking them to run `gc` on
      `Cimos/etchy` (only the owner can), and confirm an old SHA 404s before
      flipping visibility. The alternative, if Support is slow, is to publish from
      a fresh repository containing only the rewritten history.

## 2. Standard OSS files — done this pass ✅ / one decision left

- [x] **SECURITY.md** — added (private reporting via GitHub advisories; notes
      etchy parses untrusted input). Review the wording.
- [x] **THIRD-PARTY-LICENSES.md** — added at root, summarising bundled fonts,
      the demo board, PDF fixtures, and the cargo-deny-enforced dep policy.
- [x] **Demo-board provenance** — `crates/etchy-gui/assets/demo/README.md`
      added, citing Cimos/Mad_RP2040 and the redistribution basis.
- [ ] **CODE_OF_CONDUCT.md** — not added (needs your enforcement contact).
      Recommend the Contributor Covenant; pick a contact (GitHub handle or an
      address you're happy to publish) and drop it in.

## 3. Owner decisions

- [ ] **Fixture licence — `corpus/pdf/{old,new}.pdf`.** Derived from KiCad's
      bundled `demos/ecc83` (GPL-2+). Deferred to go-public per the 2026-07-12
      review. Choose: (a) replace with an original/permissive schematic pair
      (recommended for a clean permissive story), (b) keep with an explicit
      per-file GPL-2+ notice, or (c) generate the pair at test time. See
      `corpus/pdf/README.md`.
- [ ] **Internal "handoff" docs.** `docs/HANDOFF.md`, `docs/FEEDBACK_LOG.md`,
      `docs/feedback-m1.md`, `docs/PAGES_LAUNCH_CHECKLIST.md`,
      `docs/review/CODE_REVIEW_2026-06-25.md`, and the whole
      `docs/superpowers/{plans,specs}/` tree read as private dev/session
      artifacts (name a person, reference "this box"/localhost/the feedback-loop
      tooling). None are linked from the README, but all become public. Decide
      per doc: keep, move out of the published tree, or prune. (HANDOFF.md still
      walks through staging a private board locally — the identifiers are gone,
      but the procedure is internal-facing.)
- [ ] **Confirm demo board publish rights.** Mad_RP2040 fab pack is your own
      already-public board — confirm the source repo's licence covers
      redistributing the bundled copy (provenance note now in place).
- [ ] **README release claims vs reality.** README status line and Install
      section point users at [Releases] + `SHA256SUMS`. CI is green and
      `v0.1.0-rc1` is published as a pre-release with four archives plus
      `SHA256SUMS`; public `v0.1.0` is not cut until this checklist is complete.
      Keep README §Install explicit about the pre-release status until then.

## 4. Hardening — should-do

- [ ] **SHA-pin third-party Actions in `ci.yml` and `release.yml`.** They use
      moving refs (`actions/checkout@v5`, `dtolnay/rust-toolchain@stable`,
      `upload/download-artifact@v4`); `pages.yml` already SHA-pins. Matches the
      standing audit-third-party-CI rule.
- [ ] **README + recipe Action example pins `@main`.** Once a release tag
      exists, change `uses: Cimos/etchy@main` (README, `docs/ci-recipes/`) to a
      tag/SHA (`@v0.1.0`).
- [ ] **HANDOFF.md stale demo-dir status** (if the doc is kept): it says the
      demo dir is gitignored/"never commit", but the public Mad_RP2040 board is
      now committed there.
- [ ] **THIRD-PARTY-LICENSES / attribution** — done, but re-check it after the
      fixture-licence decision.

## 5. crates.io publishing — only if that's a goal (not needed for a public repo)

- [ ] Internal path deps lack versions (`etchy-core = { path = "..." }`) —
      crates.io needs `path` + `version`. Add `version = "0.1.0"` alongside.
- [ ] Add `keywords`/`categories` (gerber, pcb, diff, excellon, kicad …) and a
      per-crate `readme` for discoverability.

## 6. At the flip — repo settings (owner only)

- [ ] Set the GitHub description and topics (e.g. rust, gerber, pcb, diff,
      kicad, ci).
- [ ] Add branch protection on `main`.
- [ ] Optionally add `.github/ISSUE_TEMPLATE/` + a PR template.
- [ ] Flip visibility to public — **after** §1 is done and history is rewritten.

## Final sweep commands

```sh
# secrets / private data
git grep -nE 'BEGIN [A-Z ]*PRIVATE KEY|ghp_|github_pat_|AKIA[0-9A-Z]{16}'
# grep for the owner's personal names (first and last) and any personal email
# board identifiers: TERMS='term1|term2|…' from #264 (not listed here on purpose)
grep -rinE "$TERMS" docs/ crates/ deploy/ *.md
# private-range LAN IPs / user paths (a few version-string false positives are normal)
git grep -nE '\b(10\.|172\.(1[6-9]|2[0-9]|3[01])\.|192\.168\.)[0-9]{1,3}\.[0-9]{1,3}\b|/home/[a-z]+/'
# confirm no board/feedback data is tracked
git ls-files | grep -iE 'deploy/feedback/(screenshots|uploads)|\.(gbr|drl)$' || echo "clean"
cargo deny check licenses
```

> Keep this at the repo root as the obvious last gate. Update it as the project
> grows.
