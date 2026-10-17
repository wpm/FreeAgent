#!/usr/bin/env bash
# The top of a release's notes: the desktop app's installers, one row per
# platform, so that the list of assets under it needs no explaining.
# dist's notes from CHANGELOG.md follow it.
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
| macOS (Apple silicon and Intel) | \`Social-Deduction.dmg\` | Open it and drag Social Deduction to Applications. |
| Windows | \`Social-Deduction-setup.exe\` | Run it, and terminals opened afterward have \`social-deduction\` on the PATH. \`Social-Deduction.msi\` installs the app alone. |
| Linux x86_64 | \`social-deduction-amd64.deb\` or \`social-deduction-x86_64.rpm\` | Install the package. |
| Linux arm64 | \`social-deduction-arm64.deb\` or \`social-deduction-aarch64.rpm\` | Install the package. |
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
