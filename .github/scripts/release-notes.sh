#!/usr/bin/env bash
# The top of a release's notes: the desktop app's installers, one row per
# platform, so that the list of assets under it needs no explaining.
# The notes GitHub generated follow it.
#
#   release-notes.sh <version>
#
# Environment:
#   MACOS_SIGNED      "true" when the macOS build will be signed
#   MACOS_NOTARIZED   "true" when it will be notarized as well
set -euo pipefail

version=${1:?usage: release-notes.sh <version>}

cat <<NOTES
## Desktop app

Social Deduction $version for the desktop plays one game of Werewolf or Avalon and shows it as a group chat beside a live board.

| Platform | Download | Then |
| --- | --- | --- |
| macOS (Apple silicon and Intel) | \`Social-Deduction.dmg\` | Open it, drag Social Deduction to Applications, then choose **Social Deduction ▸ Install Command-Line Tool…** |
| Windows (x64) | \`Social-Deduction-setup.exe\` | Run it, and terminals opened afterward have \`social-deduction\` on the PATH. |
| Linux x86_64 | \`social-deduction-amd64.deb\` or \`social-deduction-x86_64.rpm\` | Install the package, which puts the command line in \`/usr/bin\`. |
| Linux arm64 | \`social-deduction-arm64.deb\` or \`social-deduction-aarch64.rpm\` | Install the package, which puts the command line in \`/usr/bin\`. |

Each download gives you the app and the \`social-deduction\` command line. \`Social-Deduction.msi\` installs the app alone.

Command line only, for servers and CI: see the [README](https://github.com/wpm/FreeAgent/tree/main/apps/social-deduction#servers-and-ci).
NOTES

if [ "${MACOS_NOTARIZED:-}" = true ]; then
  :
elif [ "${MACOS_SIGNED:-}" = true ]; then
  cat <<'NOTES'

The macOS app is signed but not notarized, so the first time it opens, macOS asks to confirm.
NOTES
else
  cat <<'NOTES'

The macOS app is unsigned, so macOS says it is damaged. Clear the quarantine mark once and it opens normally:

```sh
xattr -d com.apple.quarantine "/Applications/Social Deduction.app"
```
NOTES
fi

cat <<'NOTES'

Windows may show a SmartScreen warning for a new installer; choose "More info" and "Run anyway".
NOTES
