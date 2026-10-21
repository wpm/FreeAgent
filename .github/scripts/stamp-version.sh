#!/usr/bin/env bash
# Stamps a release's version, from its tag, into the crates of the product
# it releases. On main every crate is at 0.0.0, so no version is ever
# committed.
#
#   stamp-version.sh <product>-v<version>
#
#   stamp-version.sh social-deduction-v0.2.0
#   stamp-version.sh free-agent-v0.3.0-rc.1
#
# <version> is semver, prereleases included. Sets version = "<version>" in
# the manifest of each of the product's crates, in the workspace of the
# current directory, brings Cargo.lock along with cargo update --workspace,
# and checks with cargo metadata that each crate says <version>. Fails,
# naming the tag and changing nothing, when the tag doesn't parse or names
# no product.
#
# Environment:
#   GITHUB_OUTPUT   where the product and version outputs go (stdout without it)
set -euo pipefail

tag=${1-}

# The products, each with its crates: the one place a product's crates are
# named. (.github/labeler.yml finds them by their folders.)
crates() { # <product>
  case $1 in
    free-agent) echo free-agent ;;
    social-deduction) echo social-deduction social-deduction-gui social-deduction-desktop social-deduction-gui-types ;;
    *) return 1 ;;
  esac
}

semver='(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?([+][0-9A-Za-z.-]+)?'
if ! [[ $tag =~ ^([a-z0-9][a-z0-9-]*)-v($semver)$ ]]; then
  echo "the tag '$tag' is not <product>-v<version>, with a semver version" >&2
  exit 1
fi
product=${BASH_REMATCH[1]}
version=${BASH_REMATCH[2]}
names=$(crates "$product") || { echo "the tag '$tag' names no product: $product" >&2; exit 1; }

metadata() { cargo metadata --no-deps --format-version 1; }
manifests=$(metadata | jq -r --arg names "$names" '
  ($names | split(" ")) as $names
  | .packages[] | select(.name as $n | $names | index($n)) | .manifest_path')

# The first top-level version line in each manifest is its [package]'s.
while IFS= read -r manifest; do
  # jq on Windows ends its lines with a carriage return.
  manifest=${manifest%$'\r'}
  awk -v version="$version" '
    !done && /^version = "[^"]*"/ { print "version = \"" version "\""; done = 1; next }
    { print }' "$manifest" > "$manifest.stamped"
  mv "$manifest.stamped" "$manifest"
done <<< "$manifests"
cargo update --workspace --quiet

stamped=$(metadata)
for name in $names; do
  found=$(jq -r --arg name "$name" '.packages[] | select(.name == $name) | .version' <<< "$stamped")
  found=${found%$'\r'}
  if [ "$found" != "$version" ]; then
    echo "$name is ${found:-not in the workspace}, not $version, after stamping $tag" >&2
    exit 1
  fi
done

echo "$names: $version" >&2
printf 'product=%s\nversion=%s\n' "$product" "$version" >> "${GITHUB_OUTPUT:-/dev/stdout}"
