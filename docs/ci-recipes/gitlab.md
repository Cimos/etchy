# etchy in GitLab CI

Gate a merge request on a board diff. The job downloads the etchy release
binary, diffs the two fab packs, fails on any change, keeps the HTML report and
per-layer images as a job artifact, and posts the Markdown summary as a note on
the merge request (edited in place on each push, not re-posted).

The ghcr container is not used as the job image: it is distroless, with no
shell, and GitLab needs one to run `script`. The release binary is static, so a
plain `alpine` image runs it.

## The recipe

Add this to `.gitlab-ci.yml`. Change `OLD_DIR` / `NEW_DIR` to where the
board's Gerber/Excellon output lives, and pin `ETCHY_VERSION`.

```yaml
etchy-diff:
  image: alpine:3.20
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
      changes: ["fab/**/*"]
  variables:
    ETCHY_VERSION: v0.1.2
    OLD_DIR: fab/rev-a
    NEW_DIR: fab/rev-b
  script:
    - apk add --no-cache curl jq
    - |
      set -eu
      # Download the release binary and check it against SHA256SUMS.
      base="https://github.com/Cimos/etchy/releases/download/$ETCHY_VERSION"
      asset="etchy-cli-$ETCHY_VERSION-linux-x86_64.tar.gz"
      curl -fsSLO "$base/$asset"
      curl -fsSLO "$base/SHA256SUMS"
      grep " $asset\$" SHA256SUMS | sha256sum -c -
      tar -xzf "$asset"
      etchy="$PWD/etchy-cli-$ETCHY_VERSION-linux-x86_64/etchy"

      # Run the diff. Exit code: 0 = no diff, 1 = diff, 2 = error.
      mkdir -p etchy-report
      set +e
      "$etchy" --format md --svg etchy-report/svg --html etchy-report/report.html \
        -- "$OLD_DIR" "$NEW_DIR" > etchy-report/summary.md
      code=$?
      set -e

      # Post or refresh the MR note. A failed note is a warning; it never
      # changes the gate.
      if [ "$code" -le 1 ]; then
        marker="<!-- etchy-diff -->"
        {
          echo "$marker"
          cat etchy-report/summary.md
          if [ "$code" -eq 1 ]; then
            echo
            echo "Layer images and the HTML report: $CI_JOB_URL/artifacts/browse/etchy-report/"
          fi
        } > etchy-report/note.md
        jq -Rs '{body: .}' etchy-report/note.md > etchy-report/note.json
        if [ -n "${ETCHY_NOTE_TOKEN:-}" ]; then
          auth="PRIVATE-TOKEN: $ETCHY_NOTE_TOKEN"
        else
          auth="JOB-TOKEN: $CI_JOB_TOKEN"
        fi
        api="$CI_API_V4_URL/projects/$CI_PROJECT_ID/merge_requests/$CI_MERGE_REQUEST_IID/notes"
        id="$(curl -fsS -H "$auth" "$api?per_page=100" \
          | jq -r --arg m "$marker" '[.[] | select(.body | startswith($m))][0].id // empty')" || id=""
        if [ -n "$id" ]; then
          curl -fsS -X PUT -H "$auth" -H "Content-Type: application/json" \
            --data @etchy-report/note.json "$api/$id" > /dev/null
        else
          curl -fsS -X POST -H "$auth" -H "Content-Type: application/json" \
            --data @etchy-report/note.json "$api" > /dev/null
        fi || echo "warning: could not post the MR note (see the token notes)"
      fi
      exit "$code"
  artifacts:
    when: always
    expose_as: etchy report
    paths: [etchy-report/]
```

## What you get

- **The gate.** Any change exits 1 and fails the job; an error exits 2 and
  fails it too. To report without blocking the merge, add
  `allow_failure: { exit_codes: [1] }` to the job. etchy's own gate flags work
  here as on the command line, for example
  `--gate-layers copper --fail-on-area 0.5`.
- **The report.** `etchy-report/` holds `summary.md`, `report.html` and one
  overlay SVG per changed layer in `svg/` (base grey, removed red, added
  green). `expose_as` puts a link to it on the merge request page. The HTML
  opens in the browser when the instance has GitLab Pages turned on; otherwise
  download it.
- **The note.** The same layer-by-layer table as the GitHub Action, with a link
  to the images. Two etchy jobs in one pipeline need different `marker` values,
  or they overwrite each other's note.

## The token for the note

`CI_JOB_TOKEN` can only post notes where the instance allows the job token to
write merge request notes, and many do not. If the note step warns, create a
project access token with the `api` scope and the Reporter role, and store it as
a masked CI/CD variable named `ETCHY_NOTE_TOKEN`. The recipe uses it when it is
set and falls back to `CI_JOB_TOKEN` otherwise.

## Runners without internet access

Mirror the release's CLI archive and `SHA256SUMS` on an internal server and
point `base` at it, or bake the binary into your own runner image and skip the
download lines. The diff itself never talks to a hosted service; only the note
uses the GitLab API.
