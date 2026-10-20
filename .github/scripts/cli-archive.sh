#!/usr/bin/env bash
# Packs the social-deduction command line for one platform, for servers
# and CI, with its example configurations, its README and the licenses.
#
#   cli-archive.sh <target> <binary> <directory>
#
# Writes social-deduction-<target>.tar.gz, holding a folder of that name,
# or social-deduction-<target>.zip, holding the files at its top, for a
# Windows target. Beside it goes a .sha256 in sha256sum's format. Prints
# the archive's path and then the checksum's.
set -euo pipefail

usage="usage: cli-archive.sh <target> <binary> <directory>"
target=${1:?$usage}
binary=${2:?$usage}
out=${3:?$usage}
[ -f "$binary" ] || { echo "no binary at $binary" >&2; exit 1; }

root=$(cd "$(dirname "$0")/../.." && pwd)
name=social-deduction-$target
exe=
case $target in *windows*) exe=.exe ;; esac

mkdir -p "$out"
out=$(cd "$out" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
staged=$scratch/$name
mkdir "$staged"
cp "$binary" "$staged/social-deduction$exe"
cp -R "$root/apps/social-deduction/configs" "$staged/configs"
cp "$root/apps/social-deduction/README.md" "$staged/README.md"
cp "$root"/LICENSE-* "$staged/"

if [ -n "$exe" ]; then
  archive=$name.zip
  rm -f "$out/$archive"
  # Windows runners have 7-Zip where they lack zip.
  if command -v zip > /dev/null; then
    (cd "$staged" && zip -qr "$out/$archive" .)
  else
    (cd "$staged" && 7z a -tzip -bd "$out/$archive" . > /dev/null)
  fi
else
  archive=$name.tar.gz
  tar -czf "$out/$archive" -C "$scratch" "$name"
fi

if command -v sha256sum > /dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
(cd "$out" && "${sum[@]}" "$archive" > "$archive.sha256")
echo "$out/$archive"
echo "$out/$archive.sha256"
