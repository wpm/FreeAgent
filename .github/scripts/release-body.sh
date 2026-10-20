#!/usr/bin/env bash
# A release's body with the desktop app's notes from release-notes.sh on
# top.
#
#   gh release view <tag> --json body --jq .body | release-body.sh <version>
#
# Reads the body the release has, the notes GitHub generated, and
# prints it with the desktop app's notes above it, between the markers
# <!-- desktop-app --> and <!-- /desktop-app -->. Notes already between
# those markers are replaced, so rerunning the workflow keeps one copy,
# the latest.
#
# Environment: as release-notes.sh, which says how the macOS build is signed.
set -euo pipefail

version=${1:?usage: release-body.sh <version>}
start='<!-- desktop-app -->'
end='<!-- /desktop-app -->'
section=$(printf '%s\n%s\n%s' "$start" "$("$(dirname "$0")/release-notes.sh" "$version")" "$end")

# The body without its marked notes and the blank line after them.
rest=$(awk -v start="$start" -v end="$end" '
  { line = $0; sub(/\r$/, "", line) }
  line == start { inside = 1; next }
  inside { if (line == end) { inside = 0; after = 1 } next }
  after && line == "" { after = 0; next }
  { after = 0; print }
')

if [ -z "$rest" ]; then
  printf '%s\n' "$section"
else
  printf '%s\n\n%s\n' "$section" "$rest"
fi
