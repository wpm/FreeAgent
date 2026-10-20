#!/usr/bin/env bash
# The files the website links to on a release, and whether the release
# has them.
#
#   site-downloads.sh <site dir> <tag> [<assets>]
#
# Prints the name of each file that a .html page under <site dir> links to
# through releases/download/<tag>/, one per line, sorted: the site as
# site-releases.sh leaves it. Only links count, so a command shown on a
# page that fetches a release file is not a download. Given <assets>, a
# file of the release's asset names one per line, it also fails, naming
# each one, when a download is not among them. A site with no downloads
# from the release fails.
set -euo pipefail

usage="usage: site-downloads.sh <site dir> <tag> [<assets>]"
site=${1:?$usage}
tag=${2:?$usage}
assets=${3:-}

downloads=$(find "$site" -name '*.html' -type f \
  -exec grep -ohE "href=\"[^\"]*/releases/download/${tag//./[.]}/[^\"#?<[:space:]]*" {} + \
  | sed 's|.*/||' | LC_ALL=C sort -u || true)
[ -n "$downloads" ] || { echo "no downloads from $tag under $site" >&2; exit 1; }
printf '%s\n' "$downloads"

if [ -n "$assets" ]; then
  missing=$(LC_ALL=C comm -23 <(printf '%s\n' "$downloads") <(LC_ALL=C sort -u "$assets"))
  if [ -n "$missing" ]; then
    while IFS= read -r name; do echo "not among the release's assets: $name" >&2; done <<< "$missing"
    exit 1
  fi
fi
