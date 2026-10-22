#!/usr/bin/env bash
# Copies a desktop app's installers to the names that stay the same
# from release to release, which the website links to.
#
#   stable-names.sh <product> <directory> < artifact-paths.json
#
# Reads tauri-action's artifactPaths output, a JSON array of paths, and
# copies each installer into <directory> under its stable name, here
# Social Deduction's:
#
#   .dmg          Social-Deduction.dmg
#   -setup.exe    Social-Deduction-setup.exe
#   .msi          Social-Deduction.msi
#   .AppImage     Social-Deduction-x86_64.AppImage, Social-Deduction-aarch64.AppImage
#   .deb          social-deduction-amd64.deb, social-deduction-arm64.deb
#   .rpm          social-deduction-x86_64.rpm, social-deduction-aarch64.rpm
#
# A Linux package's architecture comes from the end of tauri-action's
# name for it, and its stable name spells it as its format does.
#
# Everything else, the .app bundle and the updater archives among them,
# is left out. Prints each copy, and fails when no path is an installer
# or a package's architecture is not one of these.
set -euo pipefail

usage="usage: stable-names.sh <product> <directory> < artifact-paths.json"
product=${1:?$usage}
out=${2:?$usage}
# shellcheck source=SCRIPTDIR/product.sh
. "$(dirname "$0")/product.sh"
product_names "$product"
mkdir -p "$out"

# A Linux package's architecture, read from the end of its name, which
# tauri-action spells amd64 or arm64 in some formats, and written as given.
arch() { # <path> <x86_64 spelling> <aarch64 spelling>
  case ${1%.*} in
    *[_.]amd64 | *[_.]x86_64) echo "$2" ;;
    *[_.]arm64 | *[_.]aarch64) echo "$3" ;;
    *) echo "no stable name for the architecture of $1" >&2; return 1 ;;
  esac
}

copied=0
while IFS= read -r path; do
  # jq on Windows ends its lines with a carriage return.
  path=${path%$'\r'}
  case $path in
    *.dmg) name=$stem.dmg ;;
    *-setup.exe) name=$stem-setup.exe ;;
    *.msi) name=$stem.msi ;;
    *.AppImage) name=$stem-$(arch "$path" x86_64 aarch64).AppImage ;;
    *.deb) name=$product-$(arch "$path" amd64 arm64).deb ;;
    *.rpm) name=$product-$(arch "$path" x86_64 aarch64).rpm ;;
    *) continue ;;
  esac
  # Windows runners hand over Windows paths, which Git Bash wants as Unix ones.
  if command -v cygpath > /dev/null; then path=$(cygpath -u "$path"); fi
  cp "$path" "$out/$name"
  echo "$path -> $out/$name"
  copied=$((copied + 1))
done < <(jq -r '.[]')

[ "$copied" -gt 0 ] || { echo "no installer among the artifacts" >&2; exit 1; }
