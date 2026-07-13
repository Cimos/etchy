#!/usr/bin/env bash
# Guard: the tracked demo fab pack must stay exactly the public Mad_RP2040 board.
#
# The demo dir uses a skip-worktree swap so a confidential board can be viewed
# locally without staging it. That relies on discipline — one `git add -f` after
# a swap silently commits a private board. This check makes that loud: it compares
# the set + content hashes of the tracked demo files against a committed manifest
# and fails if anything is added, removed, or changed.
#
#   check-demo-assets.sh            verify tracked files match the manifest
#   check-demo-assets.sh --update   rewrite the manifest from the current tracked
#                                   files (run deliberately when the public demo
#                                   board legitimately changes)
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

DEMO_DIR="crates/etchy-gui/assets/demo"
MANIFEST="scripts/demo-assets.sha256"

# Hash of the index (staged/committed) blob for a path — this is what would be
# published, and it catches a force-added swap even before it is committed.
blob_sha() { git cat-file blob ":$1" | sha256sum | cut -d' ' -f1; }

mapfile -t tracked < <(git ls-files "$DEMO_DIR/" | sort)

if [ "${1:-}" = "--update" ]; then
  : > "$MANIFEST"
  for f in "${tracked[@]}"; do
    printf '%s  %s\n' "$(blob_sha "$f")" "$f" >> "$MANIFEST"
  done
  echo "Wrote $MANIFEST (${#tracked[@]} files)."
  exit 0
fi

if [ ! -f "$MANIFEST" ]; then
  echo "FAIL: manifest $MANIFEST missing" >&2
  exit 1
fi

mapfile -t expected < <(awk '{print $2}' "$MANIFEST" | sort)

# 1. The set of tracked demo files must match the allow-list exactly.
extra=$(comm -23 <(printf '%s\n' "${tracked[@]}") <(printf '%s\n' "${expected[@]}"))
missing=$(comm -13 <(printf '%s\n' "${tracked[@]}") <(printf '%s\n' "${expected[@]}"))
fail=0
if [ -n "$extra" ]; then
  echo "FAIL: unexpected file(s) tracked under $DEMO_DIR (possible private board):" >&2
  printf '  %s\n' $extra >&2
  fail=1
fi
if [ -n "$missing" ]; then
  echo "FAIL: allow-listed demo file(s) missing from the tree:" >&2
  printf '  %s\n' $missing >&2
  fail=1
fi

# 2. Every tracked file's content must match its recorded hash.
while read -r want path; do
  [ -n "$path" ] || continue
  got=$(blob_sha "$path")
  if [ "$got" != "$want" ]; then
    echo "FAIL: content of $path does not match the manifest (swapped board?)." >&2
    echo "  expected $want, got $got" >&2
    fail=1
  fi
done < "$MANIFEST"

if [ "$fail" -ne 0 ]; then
  echo "" >&2
  echo "The tracked demo assets diverged from the public Mad_RP2040 fab pack." >&2
  echo "If this change is intentional and the board is PUBLIC, run:" >&2
  echo "  scripts/check-demo-assets.sh --update && git add $MANIFEST" >&2
  exit 1
fi

echo "OK: demo assets match the public manifest (${#expected[@]} files)."
