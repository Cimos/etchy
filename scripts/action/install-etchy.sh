#!/usr/bin/env bash
# Install the etchy CLI for the GitHub Action (action.yml): download the
# release archive for this runner's OS and CPU, check it against the release's
# SHA256SUMS, unpack it, and print the path of the etchy binary on stdout.
# Everything else goes to stderr. Exit 2 on any failure, so the Action treats
# a failed install as an error, never as "no change".
#
# Environment:
#   ETCHY_VERSION  release tag (vX.Y.Z), "latest", or empty
#   ETCHY_REF      the ref the Action was used at (github.action_ref). When
#                  ETCHY_VERSION is empty and this is a release tag it is used,
#                  so `uses: Cimos/etchy@v0.2.0` runs the v0.2.0 binary.
#                  Otherwise (a branch or SHA) the latest release is used.
#   RUNNER_OS, RUNNER_ARCH, RUNNER_TEMP   set by the runner
#   ETCHY_RELEASES optional releases base URL (default: the GitHub repo). Point
#                  it at a mirror laid out the same way for runners without
#                  github.com access.
set -euo pipefail

fail() { echo "etchy install: $*" >&2; exit 2; }

base="${ETCHY_RELEASES:-https://github.com/Cimos/etchy/releases}"
tag_re='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'

tag="${ETCHY_VERSION:-}"
if [ -z "$tag" ] && [[ "${ETCHY_REF:-}" =~ $tag_re ]]; then
  tag="$ETCHY_REF"
fi
if [ -z "$tag" ] || [ "$tag" = "latest" ]; then
  # releases/latest redirects to releases/tag/<tag>; read the tag off the final URL.
  url="$(curl -fsSL --retry 3 -o /dev/null -w '%{url_effective}' "$base/latest")" \
    || fail "could not look up the latest release at $base/latest"
  tag="${url##*/}"
fi
[[ "$tag" =~ $tag_re ]] || fail "version '$tag' is not a release tag like v0.2.0"

case "${RUNNER_OS:-}/${RUNNER_ARCH:-}" in
  Linux/X64)   plat=linux-x86_64;   ext=tar.gz; exe=etchy ;;
  macOS/ARM64) plat=macos-aarch64;  ext=tar.gz; exe=etchy ;;
  macOS/X64)   plat=macos-x86_64;   ext=tar.gz; exe=etchy ;;
  Windows/X64) plat=windows-x86_64; ext=zip;    exe=etchy.exe ;;
  *) fail "no release binary for ${RUNNER_OS:-?}/${RUNNER_ARCH:-?}; set build-from-source: true" ;;
esac

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

dest="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/etchy-$tag-$plat"
rm -rf "$dest"
mkdir -p "$dest"
cd "$dest"

echo "etchy install: $tag for $plat" >&2
curl -fsSL --retry 3 -o SHA256SUMS "$base/download/$tag/SHA256SUMS" \
  || fail "could not download SHA256SUMS for $tag"

# The CLI archive is etchy-cli-<tag>-<plat> from v0.1.1 on; v0.1.0 used
# etchy-<tag>-<plat>. Take whichever name SHA256SUMS lists.
asset=""
expected=""
for name in "etchy-cli-$tag-$plat.$ext" "etchy-$tag-$plat.$ext"; do
  expected="$(awk -v f="$name" '$2 == f || $2 == "*" f { print $1; exit }' SHA256SUMS)"
  if [ -n "$expected" ]; then asset="$name"; break; fi
done
[ -n "$asset" ] || fail "SHA256SUMS for $tag lists no CLI archive for $plat"

curl -fsSL --retry 3 -o "$asset" "$base/download/$tag/$asset" \
  || fail "could not download $asset"
actual="$(sha256 "$asset")"
[ "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" = "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ] \
  || fail "checksum mismatch for $asset: expected $expected, got $actual"
echo "etchy install: $asset checksum OK" >&2

mkdir unpacked
case "$ext" in
  tar.gz) tar -xzf "$asset" -C unpacked ;;
  zip)
    if command -v unzip >/dev/null 2>&1; then
      unzip -q "$asset" -d unpacked
    else
      7z x -bd -y -ounpacked "$asset" >/dev/null
    fi ;;
esac

bin="$(find unpacked -type f -name "$exe" | head -n 1)"
[ -n "$bin" ] || fail "$asset has no $exe inside"
chmod +x "$bin"
"$bin" --version >&2 || fail "the downloaded binary does not run on this runner"
printf '%s\n' "$dest/$bin"
