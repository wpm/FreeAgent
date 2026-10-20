#!/usr/bin/env bash
# Points the website's download links at each product's newest full
# release.
#
#   gh api repos/{owner}/{repo}/releases --paginate | site-releases.sh <site dir>
#
# Reads the repository's releases, as one or more JSON arrays, and finds
# each product named by a {{release:<product>}} placeholder in a .html file
# under <site dir>. A product's release is the one tagged
# <product>-v<version> with the highest semver <version>, among those that
# are neither drafts nor prereleases. Every placeholder becomes
#
#   https://github.com/wpm/FreeAgent/releases/download/<tag>
#
# Prints a "<product> <tag>" line for each product, sorted. Fails, changing
# nothing, when a placeholder's product has no such release, naming it, or
# when a page links through releases/latest, naming the page.
set -euo pipefail

site=${1:?usage: site-releases.sh <site dir> < releases.json}
releases=$(jq -s 'add // []')
download=https://github.com/wpm/FreeAgent/releases/download

pages=()
while IFS= read -r -d '' page; do pages+=("$page"); done < <(find "$site" -name '*.html' -type f -print0)

# releases/latest may be another product's release, so a page links to a
# release only through a placeholder.
if latest=$(grep -l 'releases/latest/' "${pages[@]}" /dev/null); then
  while IFS= read -r page; do
    echo "$page links through releases/latest, which may be another product's release; use {{release:<product>}}" >&2
  done <<< "$latest"
  exit 1
fi
products=$( (grep -ohE '\{\{release:[A-Za-z0-9_-]+\}\}' "${pages[@]}" /dev/null || true) | sed -E 's/^\{\{release:(.*)\}\}$/\1/' | LC_ALL=C sort -u)

resolved=()
missing=0
for product in $products; do
  # A version's precedence: its numbers, then a release above its own
  # prereleases (whose identifiers compare as text, close enough here).
  tag=$(jq -r --arg product "$product" '
    [ .[]
      | select((.draft | not) and (.prerelease | not))
      | .tag_name as $tag
      | ($tag | capture("^" + $product + "-v(?<major>0|[1-9][0-9]*)[.](?<minor>0|[1-9][0-9]*)[.](?<patch>0|[1-9][0-9]*)(-(?<pre>[0-9A-Za-z.-]+))?([+][0-9A-Za-z.-]+)?$")) as $v
      | { tag: $tag,
          key: [($v.major | tonumber), ($v.minor | tonumber), ($v.patch | tonumber),
                ($v.pre == null), ($v.pre // "")] } ]
    | max_by(.key) | .tag // empty' <<< "$releases")
  if [ -z "$tag" ]; then
    echo "no full release of $product, tagged $product-v<version>, for the site's {{release:$product}} links" >&2
    missing=1
  else
    resolved+=("$product $tag")
  fi
done
[ "$missing" -eq 0 ] || exit 1

for line in "${resolved[@]+"${resolved[@]}"}"; do
  product=${line% *}
  tag=${line#* }
  for page in "${pages[@]}"; do
    sed "s|{{release:$product}}|$download/$tag|g" "$page" > "$page.resolved"
    mv "$page.resolved" "$page"
  done
  echo "$line"
done
