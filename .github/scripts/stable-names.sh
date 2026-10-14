#!/usr/bin/env bash
# Copies the desktop app's installers to the names that stay the same
# from release to release, which the website links to.
#
#   stable-names.sh <directory> < artifact-paths.json
#
# Reads tauri-action's artifactPaths output, a JSON array of paths, and
# copies each installer into <directory> under its stable name:
#
#   .dmg          Social-Deduction.dmg
#   -setup.exe    Social-Deduction-setup.exe
#   .msi          Social-Deduction.msi
#   .AppImage     Social-Deduction.AppImage
#   .deb          social-deduction.deb
#   .rpm          social-deduction.rpm
#
# Everything else, the .app bundle and the updater archives among them,
# is left out. Prints each copy, and fails when no path is an installer.
set -euo pipefail

out=${1:?usage: stable-names.sh <directory> < artifact-paths.json}
mkdir -p "$out"

copied=0
while IFS= read -r path; do
  case $path in
    *.dmg) name=Social-Deduction.dmg ;;
    *-setup.exe) name=Social-Deduction-setup.exe ;;
    *.msi) name=Social-Deduction.msi ;;
    *.AppImage) name=Social-Deduction.AppImage ;;
    *.deb) name=social-deduction.deb ;;
    *.rpm) name=social-deduction.rpm ;;
    *) continue ;;
  esac
  # Windows runners hand over Windows paths, which Git Bash reads as Unix ones.
  if command -v cygpath > /dev/null; then path=$(cygpath -u "$path"); fi
  cp "$path" "$out/$name"
  echo "$path -> $out/$name"
  copied=$((copied + 1))
done < <(jq -r '.[]')

[ "$copied" -gt 0 ] || { echo "no installer among the artifacts" >&2; exit 1; }
