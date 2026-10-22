#!/usr/bin/env bash
# The top of a release's notes: the desktop app's installers, one row per
# platform, so that the list of assets under it needs no explaining.
# The notes GitHub generated follow it.
#
#   release-notes.sh <product> <version>
#
# Environment:
#   MACOS_SIGNED      "true" when the macOS build will be signed
#   MACOS_NOTARIZED   "true" when it will be notarized as well
set -euo pipefail

usage="usage: release-notes.sh <product> <version>"
product=${1:?$usage}
version=${2:?$usage}
# shellcheck source=SCRIPTDIR/product.sh
. "$(dirname "$0")/product.sh"
product_names "$product"

cat <<NOTES
## Desktop app

$title $version for the desktop $plays.

| Platform | Download | Then |
| --- | --- | --- |
| macOS (Apple silicon and Intel) | \`$stem.dmg\` | Open it, drag $title to Applications, then choose **$title ▸ Install Command-Line Tool…** |
| Windows (x64) | \`$stem-setup.exe\` | Run it, and terminals opened afterward have \`$product\` on the PATH. |
| Linux x86_64 | \`$product-amd64.deb\` or \`$product-x86_64.rpm\` | Install the package, which puts the command line in \`/usr/bin\`. |
| Linux arm64 | \`$product-arm64.deb\` or \`$product-aarch64.rpm\` | Install the package, which puts the command line in \`/usr/bin\`. |

Each download gives you the app and the \`$product\` command line. \`$stem.msi\` installs the app alone.

Command line only, for servers and CI: see the [README](https://github.com/wpm/FreeAgent/tree/main/apps/$product/cli#servers-and-ci).
NOTES

if [ "${MACOS_NOTARIZED:-}" = true ]; then
  :
elif [ "${MACOS_SIGNED:-}" = true ]; then
  cat <<'NOTES'

The macOS app is signed but not notarized, so the first time it opens, macOS asks to confirm.
NOTES
else
  cat <<NOTES

The macOS app is unsigned, so macOS says it is damaged. Clear the quarantine mark once and it opens normally:

\`\`\`sh
xattr -d com.apple.quarantine "/Applications/$title.app"
\`\`\`
NOTES
fi

cat <<'NOTES'

Windows may show a SmartScreen warning for a new installer; choose "More info" and "Run anyway".
NOTES
