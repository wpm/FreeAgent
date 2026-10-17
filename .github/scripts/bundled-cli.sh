#!/usr/bin/env bash
# Checks that each of the desktop app's bundles carries the command line,
# and that it is the release's.
#
#   bundled-cli.sh <version> [<sidecar>] < artifact-paths.json
#
# Reads tauri-action's artifactPaths output, a JSON array of paths, opens
# each bundle (.app, .dmg, .deb, .rpm, .AppImage, -setup.exe, .msi), finds
# social-deduction in it and runs `social-deduction --version`, which has
# to print <version>. Given the sidecar the bundle was built with, each
# bundle's binary has to be that very file. Everything else, the updater
# archives among them, is left out. Fails when a bundle has no
# social-deduction, or no path is a bundle.
set -euo pipefail

version=${1:?usage: bundled-cli.sh <version> [<sidecar>] < artifact-paths.json}
sidecar=${2:-}

sha() { # <file>
  if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d ' ' -f 1
}

expected=
if [ -n "$sidecar" ]; then expected=$(sha "$sidecar"); fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# Unpacks a bundle into a directory and prints where to look in it.
open_bundle() { # <bundle> <directory>
  local bundle=$1 dir=$2
  mkdir -p "$dir"
  case $bundle in
    *.app) echo "$bundle" ;;
    *.dmg)
      hdiutil attach -quiet -nobrowse -readonly -mountpoint "$dir/mount" "$bundle"
      cp -R "$dir/mount/." "$dir/files"
      hdiutil detach -quiet "$dir/mount"
      echo "$dir/files" ;;
    *.deb) dpkg-deb -x "$bundle" "$dir" && echo "$dir" ;;
    *.rpm) (cd "$dir" && rpm2cpio "$bundle" | cpio -idm --quiet) && echo "$dir" ;;
    *.AppImage)
      chmod +x "$bundle"
      (cd "$dir" && "$bundle" --appimage-extract > /dev/null) && echo "$dir/squashfs-root" ;;
    *-setup.exe) 7z x -y -o"$dir" "$bundle" > /dev/null && echo "$dir" ;;
    *.msi)
      MSYS2_ARG_CONV_EXCL='*' msiexec /a "$(cygpath -w "$bundle")" /qn TARGETDIR="$(cygpath -w "$dir")"
      echo "$dir" ;;
    *) return 1 ;;
  esac
}

checked=0
failed=0
while IFS= read -r bundle; do
  # jq on Windows ends its lines with a carriage return, and hands over
  # Windows paths, which Git Bash wants as Unix ones.
  bundle=${bundle%$'\r'}
  if command -v cygpath > /dev/null; then bundle=$(cygpath -u "$bundle"); fi
  root=$(open_bundle "$bundle" "$scratch/$((checked + 1))") || continue
  checked=$((checked + 1))
  binary=$(find "$root" -type f \( -name social-deduction -o -name social-deduction.exe \) | head -n 1)
  if [ -z "$binary" ]; then
    echo "FAIL: $bundle has no social-deduction" >&2
    failed=$((failed + 1))
    continue
  fi
  said=$("$binary" --version)
  said=${said%$'\r'}
  if [ "$said" != "social-deduction $version" ]; then
    echo "FAIL: $binary says \"$said\", not \"social-deduction $version\"" >&2
    failed=$((failed + 1))
    continue
  fi
  if [ -n "$expected" ]; then
    actual=$(sha "$binary")
    if [ "$actual" != "$expected" ]; then
      echo "FAIL: $binary is not $sidecar" >&2
      failed=$((failed + 1))
      continue
    fi
    said="$said, sha256 $actual"
  fi
  echo "$bundle: $said"
done < <(jq -r '.[]')

[ "$checked" -gt 0 ] || { echo "no bundle among the artifacts" >&2; exit 1; }
[ "$failed" -eq 0 ]
