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
# and the product's downloads, the lines from <!-- downloads:<product> -->
# to <!-- /downloads:<product> -->, stay as they are. Until a product has
# a full release, its downloads say instead that the first release is on
# its way.
#
# Prints a "<product> <tag>" line for each product with a release, sorted.
# Fails, changing nothing, when a placeholder outside its product's
# downloads has no release, naming the product, or when a page links
# through releases/latest, naming the page.
set -euo pipefail

site=${1:?usage: site-releases.sh <site dir> < releases.json}
releases=$(jq -s 'add // []')
download=https://github.com/wpm/FreeAgent/releases/download
soon='<p class="text">The first release is on its way.</p>'

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
unreleased=()
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
  if [ -z "$tag" ]; then unreleased+=("$product"); else resolved+=("$product $tag"); fi
done

# Each page is resolved into a scratch copy first, so that a failure
# leaves the site as it was.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
for i in "${!pages[@]}"; do
  copy=$scratch/$i.html
  cp "${pages[$i]}" "$copy"
  for product in "${unreleased[@]+"${unreleased[@]}"}"; do
    if ! awk -v start="<!-- downloads:$product -->" -v end="<!-- /downloads:$product -->" -v soon="$soon" '
        { line = $0; sub(/^[ \t]+/, "", line) }
        line == start { inside = 1; print substr($0, 1, index($0, start) - 1) soon; next }
        inside { if (line == end) inside = 0; next }
        { print }
        END { exit inside }' "$copy" > "$copy.soon"; then
      echo "${pages[$i]} has <!-- downloads:$product --> with no <!-- /downloads:$product --> after it" >&2
      exit 1
    fi
    mv "$copy.soon" "$copy"
  done
  for line in "${resolved[@]+"${resolved[@]}"}"; do
    sed "s|{{release:${line% *}}}|$download/${line#* }|g" "$copy" > "$copy.resolved"
    mv "$copy.resolved" "$copy"
  done
done

missing=0
for product in "${unreleased[@]+"${unreleased[@]}"}"; do
  if grep -qF "{{release:$product}}" "$scratch"/*.html; then
    echo "no full release of $product, tagged $product-v<version>, for the site's {{release:$product}} links outside its downloads" >&2
    missing=1
  else
    echo "no full release of $product yet: its downloads say the first release is on its way" >&2
  fi
done
[ "$missing" -eq 0 ] || exit 1

for i in "${!pages[@]}"; do
  mv "$scratch/$i.html" "${pages[$i]}"
done
for line in "${resolved[@]+"${resolved[@]}"}"; do
  echo "$line"
done
