#!/usr/bin/env bash
# The installers the website links to, and whether a release has them.
#
#   site-downloads.sh <page> [<assets>]
#
# Prints the name of each file <page> downloads through
# releases/latest/download/, one per line, sorted. Given <assets>, a file
# of a release's asset names one per line, it also fails, naming each
# one, when a download is not among them. A page with no downloads fails.
set -euo pipefail

page=${1:?usage: site-downloads.sh <page> [<assets>]}
assets=${2:-}

downloads=$(grep -o 'releases/latest/download/[^"#?<[:space:]]*' "$page" | sed 's|.*/||' | LC_ALL=C sort -u || true)
[ -n "$downloads" ] || { echo "no downloads on $page" >&2; exit 1; }
printf '%s\n' "$downloads"

if [ -n "$assets" ]; then
  missing=$(LC_ALL=C comm -23 <(printf '%s\n' "$downloads") <(LC_ALL=C sort -u "$assets"))
  if [ -n "$missing" ]; then
    while IFS= read -r name; do echo "not among the release's assets: $name" >&2; done <<< "$missing"
    exit 1
  fi
fi
