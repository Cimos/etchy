# ⚠ Before this repo goes public

etchy is intended to be open-source, but during development it accumulated some
data and internal references that **must be dealt with before the repository is
made public**. This is the last gate before flipping visibility — work through
it top to bottom. Grouped by severity; each item notes whether it's an owner
decision (Simon) or a mechanical fix.

Status of this list is from the 2026-07-13 go-public audit (four-dimension
sweep: licence, confidential data/secrets, README/docs, repo hygiene). The good
news up front: **no secrets in the working tree or the full git history, and no
private-board feedback screenshots are committed** — `.gitignore` correctly
excludes `deploy/feedback/` screenshots and uploads.

---

## 1. Blockers — must resolve before flipping visibility

- [ ] **Confidential customer board named in docs prose (history too).** The
      board *files* are not committed, but several docs name an
      employer-confidential CubePilot board and its internals: **FMU
      REV_67A/REV_67B**, **CubeOrange+**, part number **290-00146**,
      **MotionJigController**, plus absolute local paths. Locations:
      `docs/HANDOFF.md`, `CHANGELOG.md`, `docs/PERF_GPU_TRANSFORM.md`,
      `docs/future/ADAPTIVE_TESSELLATION.md`, `docs/review/CODE_REVIEW_2026-06-25.md`,
      `docs/superpowers/specs/2026-06-20-fmu-review-issues-plan.md`,
      `docs/ROADMAP.md`. **Owner decision + action:** scrub/genericise these
      (e.g. "a real 24-layer Altium board"), and because they're in past
      commits, **rewrite history** (`git filter-repo`) so the names don't
      survive. Coordinate the rewrite — the repo has open PRs/branches and is
      worked from multiple worktrees.
      Sweep to confirm clean:
      `grep -rin -E 'fmu|cubeorange|290-00146|motionjig|Desktop/|ProductionFiles' docs/ *.md`
- [ ] **Committed feedback PII (history too).** `deploy/feedback/m1-seed.jsonl`
      (git-tracked, whitelisted past the `*.jsonl` ignore) contains internal LAN
      IPs (`10.10.10.123`, `10.10.10.194`), User-Agent strings, and tester names
      ("Robbie", "SM"). No board images. **Action:** scrub the `ip`/`ua`/`name`
      fields or drop the file, and rewrite history since it's in past commits.
      Decide whether the seed feedback needs to ship at all.

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
      per doc: keep, move out of the published tree, or prune. (HANDOFF.md and
      the fmu-review spec also carry the §1 confidential content.)
- [ ] **Confirm demo board publish rights.** Mad_RP2040 fab pack is your own
      already-public board — confirm the source repo's licence covers
      redistributing the bundled copy (provenance note now in place).
- [ ] **README release claims vs reality.** README status line and Install
      section point users at [Releases] + `SHA256SUMS`. v0.1.0 is not cut
      (Actions billing parked). Either publish the Release with per-OS archives
      + checksums before flipping, or soften README §Install to
      "build from source / container" until binaries ship (both already work).

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
grep -rin -E 'fmu|cubeorange|290-00146|motionjig|Desktop/|ProductionFiles' docs/ *.md
git grep -nE '10\.10\.10\.|192\.168\.|/home/[a-z]+/'   # LAN IPs / user paths
# confirm no board/feedback data is tracked
git ls-files | grep -iE 'deploy/feedback/(screenshots|uploads)|\.(gbr|drl)$' || echo "clean"
cargo deny check licenses
```

> Keep this at the repo root as the obvious last gate. Update it as the project
> grows.
