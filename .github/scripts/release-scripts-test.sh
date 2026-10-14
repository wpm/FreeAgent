#!/usr/bin/env bash
# Tests for release-body.sh and stable-names.sh, run by ci.yml.
set -euo pipefail
cd "$(dirname "$0")"

failures=0
check() { # <description> <condition>...
  local what=$1; shift
  if "$@"; then echo "ok: $what"; else echo "FAIL: $what" >&2; failures=$((failures + 1)); fi
}
fails() { ! "$@" 2> /dev/null; }

generated=$'## Install social-deduction 1.2.3\n\nThe notes from CHANGELOG.md.'
notes=$(./release-notes.sh 1.2.3)

# The notes go on top of what dist wrote, with a blank line between.
body=$(./release-body.sh 1.2.3 <<< "$generated")
check "notes come first" test "$body" = "$notes"$'\n\n'"$generated"

# A rerun finds the notes already there and leaves the body alone.
again=$(./release-body.sh 1.2.3 <<< "$body")
check "a rerun changes nothing" test "$again" = "$body"

# An empty body is just the notes.
empty=$(./release-body.sh 1.2.3 < /dev/null)
check "an empty body gets the notes alone" test "$empty" = "$notes"

# The macOS signing status follows the environment.
check "an unsigned build says how to open it" grep -q "xattr -d com.apple.quarantine" <<< "$notes"
signed=$(MACOS_SIGNED=true ./release-body.sh 1.2.3 <<< "$generated")
check "a signed build says it is not notarized" grep -q "signed but not notarized" <<< "$signed"
notarized=$(MACOS_SIGNED=true MACOS_NOTARIZED=true ./release-notes.sh 1.2.3)
check "a notarized build needs no word" fails grep -q "macOS app is" <<< "$notarized"

# Each installer is copied under its stable name, and nothing else is.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/built/Social Deduction.app"
for file in "Social Deduction_1.2.3_universal.dmg" "Social Deduction.app.tar.gz" \
    "Social Deduction_1.2.3_x64-setup.exe" "Social Deduction_1.2.3_x64_en-US.msi" \
    "Social Deduction_1.2.3_amd64.AppImage" "Social Deduction_1.2.3_amd64.deb" \
    "Social Deduction-1.2.3-1.x86_64.rpm"; do
  echo "$file" > "$scratch/built/$file"
done
jq -n --arg dir "$scratch/built" '[$dir + "/" + ($ARGS.positional[])]' --args \
  "Social Deduction.app" "Social Deduction.app.tar.gz" \
  "Social Deduction_1.2.3_universal.dmg" "Social Deduction_1.2.3_x64-setup.exe" \
  "Social Deduction_1.2.3_x64_en-US.msi" "Social Deduction_1.2.3_amd64.AppImage" \
  "Social Deduction_1.2.3_amd64.deb" "Social Deduction-1.2.3-1.x86_64.rpm" \
  > "$scratch/paths.json"
./stable-names.sh "$scratch/stable" < "$scratch/paths.json" > /dev/null
listing=$(find "$scratch/stable" -type f -exec basename {} \; | LC_ALL=C sort | tr '\n' ' ')
check "installers get their stable names" test "$listing" = \
  "Social-Deduction-setup.exe Social-Deduction.AppImage Social-Deduction.dmg Social-Deduction.msi social-deduction.deb social-deduction.rpm "
check "a copy is the installer it names" test "$(cat "$scratch/stable/Social-Deduction.dmg")" = "Social Deduction_1.2.3_universal.dmg"

# No installer at all is a failed build.
check "no installer fails" fails ./stable-names.sh "$scratch/none" <<< "[\"$scratch/built/Social Deduction.app\"]"

[ "$failures" -eq 0 ]
