#!/usr/bin/env bash
# Report step of the GitHub Action (action.yml): build the PR comment from the
# etchy Markdown summary plus a link to the layer-image artifact, add the link
# to the step summary, and post or refresh the sticky PR comment.
#
# A reporting failure (missing permission, not a PR event) is a warning, never
# a pipeline failure: reporting must not break the gate.
#
# Environment (set by action.yml):
#   REPORT_DIR    folder with summary.md and svg/<layer>.svg
#   KEY           per-input-pair key; names the artifact and the comment marker
#   CHANGED       "true" when etchy found differences
#   ARTIFACT_URL  link to the uploaded artifact, empty when none was uploaded
#   DO_COMMENT    "true" to post the PR comment
#   PR_NUMBER     the pull request number, empty outside pull_request events
#   GH_TOKEN, GITHUB_REPOSITORY, GITHUB_STEP_SUMMARY   from the runner
set -euo pipefail

body="$REPORT_DIR/comment.md"
marker="<!-- etchy-diff:${KEY} -->"

images=""
if [ "$CHANGED" = "true" ]; then
  # Changed layer names, from the one-SVG-per-changed-layer output.
  layers="$(find "$REPORT_DIR/svg" -maxdepth 1 -name '*.svg' -type f 2>/dev/null \
    | sed 's#.*/##; s#\.svg$##' | LC_ALL=C sort | paste -sd, - | sed 's/,/, /g' || true)"
  if [ -n "$ARTIFACT_URL" ]; then
    images="**Layer images:** [etchy-diff-${KEY}](${ARTIFACT_URL}) has one overlay SVG per changed layer (${layers:-none}) and the HTML report. Removed is red, added is green."
  else
    images="**Layer images:** not uploaded for this run. Set \`upload-report: true\` to get one overlay SVG per changed layer and the HTML report."
  fi
fi

{
  printf '%s\n' "$marker"
  cat "$REPORT_DIR/summary.md"
  if [ -n "$images" ]; then
    printf '\n%s\n' "$images"
  fi
} > "$body"

# The step summary already has the table; add only the images line.
if [ -n "$images" ]; then
  printf '\n%s\n' "$images" >> "$GITHUB_STEP_SUMMARY"
fi

if [ "${DO_COMMENT:-}" != "true" ] || [ -z "${PR_NUMBER:-}" ]; then
  exit 0
fi
if [ -z "${GH_TOKEN:-}" ]; then
  echo "::warning::comment:true but no token available — skipping PR comment"
  exit 0
fi

if comment_id="$(
  gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --paginate \
    --jq ".[] | select(.body | startswith(\"$marker\")) | .id" \
    | sed -n '1p'
)"; then
  if [ -n "$comment_id" ]; then
    # -F with @file sends the file's text as the body (-f would send "@file").
    gh api -X PATCH \
      "repos/${GITHUB_REPOSITORY}/issues/comments/${comment_id}" \
      -F body=@"$body" >/dev/null \
      || echo "::warning::could not update PR comment — grant the job pull-requests:write"
  else
    gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
      -F body=@"$body" >/dev/null \
      || echo "::warning::could not post PR comment — grant the job pull-requests:write"
  fi
else
  echo "::warning::could not find existing PR comment — grant the job pull-requests:write"
fi
