#!/usr/bin/env bash
# Tests for the release scripts, run by ci.yml.
set -euo pipefail
cd "$(dirname "$0")"

failures=0
check() { # <description> <condition>...
  local what=$1; shift
  if "$@"; then echo "ok: $what"; else echo "FAIL: $what" >&2; failures=$((failures + 1)); fi
}
fails() { ! "$@" > /dev/null 2>&1; }
quiet() { "$@" > /dev/null; }

generated=$'## Install social-deduction 1.2.3\n\nThe notes from CHANGELOG.md.'
notes=$(./release-notes.sh 1.2.3)

section=$'<!-- desktop-app -->\n'"$notes"$'\n<!-- /desktop-app -->'

# The notes go on top of what dist wrote, between markers, with a blank
# line after them.
body=$(./release-body.sh 1.2.3 <<< "$generated")
check "notes come first" test "$body" = "$section"$'\n\n'"$generated"

# A rerun finds the notes already there and leaves the body alone.
again=$(./release-body.sh 1.2.3 <<< "$body")
check "a rerun changes nothing" test "$again" = "$body"

# A rerun with different notes replaces the old ones, however they read.
resigned=$(MACOS_SIGNED=true ./release-body.sh 1.2.3 <<< "$body")
fresh=$(MACOS_SIGNED=true ./release-body.sh 1.2.3 <<< "$generated")
check "a rerun with new notes replaces the old" test "$resigned" = "$fresh"
check "a rerun leaves one table" test "$(grep -c '^## Desktop app' <<< "$resigned")" = 1
crlf=$(MACOS_SIGNED=true ./release-body.sh 1.2.3 <<< "${body//$'\n'/$'\r\n'}")
check "markers that end in a carriage return are found" test "$(grep -c '^## Desktop app' <<< "$crlf")" = 1

# An empty body is just the notes.
empty=$(./release-body.sh 1.2.3 < /dev/null)
check "an empty body gets the notes alone" test "$empty" = "$section"

# The macOS signing status follows the environment.
check "an unsigned build says how to open it" grep -q "xattr -d com.apple.quarantine" <<< "$notes"
signed=$(MACOS_SIGNED=true ./release-body.sh 1.2.3 <<< "$generated")
check "a signed build says it is not notarized" grep -q "signed but not notarized" <<< "$signed"
notarized=$(MACOS_SIGNED=true MACOS_NOTARIZED=true ./release-notes.sh 1.2.3)
check "a notarized build needs no word" fails grep -q "macOS app is" <<< "$notarized"

# The setup.exe puts the command line on the PATH; the .msi does not.
check "the setup.exe gives terminals the command line" grep -q "Social-Deduction-setup.exe\` | Run it, and terminals opened afterward have \`social-deduction\` on the PATH" <<< "$notes"
check "the .msi installs the app alone" grep -q "Social-Deduction.msi\` installs the app alone" <<< "$notes"

# Each installer is copied under its stable name, and nothing else is.
# The Linux packages' names say their architecture, in each format's own
# spelling.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/built/Social Deduction.app"
built=("Social Deduction.app.tar.gz" \
  "Social Deduction_1.2.3_universal.dmg" "Social Deduction_1.2.3_x64-setup.exe" \
  "Social Deduction_1.2.3_x64_en-US.msi" \
  "Social Deduction_1.2.3_amd64.AppImage" "Social Deduction_1.2.3_amd64.deb" "Social Deduction-1.2.3-1.x86_64.rpm" \
  "Social Deduction_1.2.3_aarch64.AppImage" "Social Deduction_1.2.3_arm64.deb" "Social Deduction-1.2.3-1.aarch64.rpm")
for file in "${built[@]}"; do
  echo "$file" > "$scratch/built/$file"
done
jq -n --arg dir "$scratch/built" '[$dir + "/" + ($ARGS.positional[])]' --args "Social Deduction.app" "${built[@]}" > "$scratch/paths.json"
./stable-names.sh "$scratch/stable" < "$scratch/paths.json" > /dev/null
listing=$(find "$scratch/stable" -type f -exec basename {} \; | LC_ALL=C sort | tr '\n' ' ')
check "installers get their stable names" test "$listing" = \
  "Social-Deduction-aarch64.AppImage Social-Deduction-setup.exe Social-Deduction-x86_64.AppImage Social-Deduction.AppImage Social-Deduction.dmg Social-Deduction.msi social-deduction-aarch64.rpm social-deduction-amd64.deb social-deduction-arm64.deb social-deduction-x86_64.rpm social-deduction.deb social-deduction.rpm "
check "a copy is the installer it names" test "$(cat "$scratch/stable/Social-Deduction.dmg")" = "Social Deduction_1.2.3_universal.dmg"
check "an arm64 package is the arm64 build" test "$(cat "$scratch/stable/social-deduction-arm64.deb")" = "Social Deduction_1.2.3_arm64.deb"
check "an aarch64 AppImage is the aarch64 build" test "$(cat "$scratch/stable/Social-Deduction-aarch64.AppImage")" = "Social Deduction_1.2.3_aarch64.AppImage"
# For one release, the x86_64 packages keep their old names as well.
check "an old name is the x86_64 build" test "$(cat "$scratch/stable/social-deduction.rpm")" = "Social Deduction-1.2.3-1.x86_64.rpm"
check "an x86_64 AppImage spelled amd64 is the x86_64 build" test "$(cat "$scratch/stable/Social-Deduction-x86_64.AppImage")" = "Social Deduction_1.2.3_amd64.AppImage"

# The website offers six of the installers: the .dmg, the setup.exe, and
# a .deb and an .rpm for each Linux architecture. The release notes'
# table names the same ones.
site=$(./site-downloads.sh ../../site/index.html | tr '\n' ' ')
check "the site offers the installers that put the command line on the PATH" test "$site" = \
  "Social-Deduction-setup.exe Social-Deduction.dmg social-deduction-aarch64.rpm social-deduction-amd64.deb social-deduction-arm64.deb social-deduction-x86_64.rpm "
check "every download on the site has a stable name" quiet ./site-downloads.sh ../../site/index.html <(ls "$scratch/stable")
check "the notes' table names the site's downloads" test \
  "$(awk -F' [|] ' '/^\| / { print $2 }' <<< "$notes" | grep -oE '[[:alnum:]_.-]+[.](dmg|exe|deb|rpm)' | LC_ALL=C sort | tr '\n' ' ')" = "$site"
check "the notes say the installers carry the command line" grep -q "Each download gives you the app and the \`social-deduction\` command line" <<< "$notes"
check "the notes send the command line alone to the README" grep -q "Command line only, for servers and CI: see the \[README\](https://github.com/wpm/FreeAgent/tree/main/apps/social-deduction#servers-and-ci)." <<< "$notes"
# A download the release doesn't have fails, and says which.
rm "$scratch/stable/social-deduction-arm64.deb"
status=0; missing=$(./site-downloads.sh ../../site/index.html <(ls "$scratch/stable") 2>&1 > /dev/null) || status=$?
check "a download missing from the release fails" test "$status" -ne 0
check "the missing download is named" test "$missing" = "not among the release's assets: social-deduction-arm64.deb"
mkdir -p "$scratch/page"
printf '<a href="https://example.com/">nothing to download</a>\n' > "$scratch/page/index.html"
check "a page with no downloads fails" fails ./site-downloads.sh "$scratch/page/index.html"

# A package for an architecture with no stable name fails.
echo riscv > "$scratch/built/Social Deduction_1.2.3_riscv64.deb"
check "an unknown architecture fails" fails ./stable-names.sh "$scratch/riscv" <<< "[\"$scratch/built/Social Deduction_1.2.3_riscv64.deb\"]"

# Lines that end in a carriage return, as jq writes them on Windows, match too.
./stable-names.sh "$scratch/crlf" <<< "[\"$scratch/built/Social Deduction_1.2.3_x64_en-US.msi\r\"]" > /dev/null
check "a carriage return is ignored" test -f "$scratch/crlf/Social-Deduction.msi"

# No installer at all is a failed build.
check "no installer fails" fails ./stable-names.sh "$scratch/none" <<< "[\"$scratch/built/Social Deduction.app\"]"

# sidecar.sh, with stand-ins for the tools it runs: cargo writes what it
# was asked to build, lipo joins its inputs, and gh hands over archives
# shaped like dist's.
tools="$scratch/tools"
mkdir -p "$tools" "$scratch/releases"
cat > "$tools/rustc" <<'TOOL'
#!/usr/bin/env bash
printf 'rustc 1.0.0\nhost: x86_64-unknown-linux-gnu\r\n'
TOOL
cat > "$tools/cargo" <<'TOOL'
#!/usr/bin/env bash
target=; profile=debug
while [ $# -gt 0 ]; do case $1 in --target) target=$2; shift 2 ;; --release) profile=release; shift ;; *) shift ;; esac; done
exe=; case $target in *windows*) exe=.exe ;; esac
dir=$CARGO_TARGET_DIR/${target:+$target/}$profile
mkdir -p "$dir"
echo "$profile build for ${target:-the host}" > "$dir/social-deduction$exe"
TOOL
cat > "$tools/lipo" <<'TOOL'
#!/usr/bin/env bash
inputs=(); while [ $# -gt 0 ]; do case $1 in -create) shift ;; -output) out=$2; shift 2 ;; *) inputs+=("$1"); shift ;; esac; done
cat "${inputs[@]}" > "$out"
TOOL
cat > "$tools/gh" <<'TOOL'
#!/usr/bin/env bash
# gh release download <tag> -p <pattern>... -D <directory>
tag=$3; shift 3; patterns=()
while [ $# -gt 0 ]; do case $1 in -p) patterns+=("$2"); shift 2 ;; -D) dir=$2; shift 2 ;; *) shift ;; esac; done
for pattern in "${patterns[@]}"; do cp "$RELEASES/$tag/"$pattern "$dir/"; done
TOOL
chmod +x "$tools"/*
for triple in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
  mkdir -p "$scratch/archive/social-deduction-$triple" "$scratch/releases/v1.2.3"
  echo "dist's $triple" > "$scratch/archive/social-deduction-$triple/social-deduction"
  tar -cJf "$scratch/releases/v1.2.3/social-deduction-$triple.tar.xz" -C "$scratch/archive" "social-deduction-$triple"
done
mkdir -p "$scratch/archive/windows"
echo "dist's windows" > "$scratch/archive/windows/social-deduction.exe"
(cd "$scratch/archive/windows" && zip -q "$scratch/releases/v1.2.3/social-deduction-x86_64-pc-windows-msvc.zip" social-deduction.exe)

sidecar() { # <directory> <argument>...
  local out=$1; shift
  PATH="$tools:$PATH" CARGO_TARGET_DIR="$scratch/target" RELEASES="$scratch/releases" \
    SIDECAR_DIR="$out" ./sidecar.sh "$@" > /dev/null
}
files() { find "$1" -type f -exec basename {} \; | LC_ALL=C sort | tr '\n' ' '; }

# Built here, for the host by default, under Tauri's name for the target.
sidecar "$scratch/host"
check "the host's sidecar is named for the host" test "$(files "$scratch/host")" = "social-deduction-x86_64-unknown-linux-gnu "
check "the host's sidecar is the one cargo built" test "$(cat "$scratch/host/social-deduction-x86_64-unknown-linux-gnu")" = "release build for the host"
sidecar "$scratch/debug" --debug
check "a debug sidecar is cargo's debug build" test "$(cat "$scratch/debug/social-deduction-x86_64-unknown-linux-gnu")" = "debug build for the host"
sidecar "$scratch/windows" --target x86_64-pc-windows-msvc
check "a Windows sidecar ends in .exe" test "$(files "$scratch/windows")" = "social-deduction-x86_64-pc-windows-msvc.exe "

# A universal app needs a sidecar for each chip, which cargo builds the
# app for, and one for the bundle that joins them.
sidecar "$scratch/universal" --target universal-apple-darwin
check "a universal sidecar comes with one for each chip" test "$(files "$scratch/universal")" = \
  "social-deduction-aarch64-apple-darwin social-deduction-universal-apple-darwin social-deduction-x86_64-apple-darwin "
check "a universal sidecar joins the two" test "$(cat "$scratch/universal/social-deduction-universal-apple-darwin")" = \
  $'release build for aarch64-apple-darwin\nrelease build for x86_64-apple-darwin'

# On a release, the sidecar is the binary dist built, from its archive.
sidecar "$scratch/released-linux" --target x86_64-unknown-linux-gnu --release v1.2.3
check "a released sidecar is dist's binary" test "$(cat "$scratch/released-linux/social-deduction-x86_64-unknown-linux-gnu")" = "dist's x86_64-unknown-linux-gnu"
sidecar "$scratch/released-windows" --target x86_64-pc-windows-msvc --release v1.2.3
check "a released Windows sidecar comes out of dist's zip" test "$(cat "$scratch/released-windows/social-deduction-x86_64-pc-windows-msvc.exe")" = "dist's windows"
sidecar "$scratch/released-universal" --target universal-apple-darwin --release v1.2.3
check "a released universal sidecar joins dist's two" test "$(cat "$scratch/released-universal/social-deduction-universal-apple-darwin")" = \
  $'dist\'s aarch64-apple-darwin\ndist\'s x86_64-apple-darwin'
check "a release without the target's archive fails" fails sidecar "$scratch/missing" --target aarch64-unknown-linux-gnu --release v1.2.3

# bundled-cli.sh finds social-deduction in each bundle and asks its version.
mkdir -p "$scratch/bundles/Social Deduction.app/Contents/MacOS" "$scratch/bundles/Empty.app/Contents/MacOS"
printf '#!/bin/sh\necho "social-deduction 1.2.3"\n' > "$scratch/bundles/Social Deduction.app/Contents/MacOS/social-deduction"
chmod +x "$scratch/bundles/Social Deduction.app/Contents/MacOS/social-deduction"
app=$(jq -n --arg path "$scratch/bundles/Social Deduction.app" '[$path, $path + ".tar.gz"]')
check "a bundle with the release's CLI passes" quiet ./bundled-cli.sh 1.2.3 <<< "$app"
check "a bundle with another version fails" fails ./bundled-cli.sh 1.2.4 <<< "$app"
check "a bundle without the CLI fails" fails ./bundled-cli.sh 1.2.3 <<< "[\"$scratch/bundles/Empty.app\"]"
check "no bundle at all fails" fails ./bundled-cli.sh 1.2.3 <<< "[\"$scratch/bundles/Social Deduction.app.tar.gz\"]"
# Given the sidecar, each bundle's CLI must be that very file.
cp "$scratch/bundles/Social Deduction.app/Contents/MacOS/social-deduction" "$scratch/same"
echo different > "$scratch/different"
check "a bundle's CLI that is the sidecar passes" quiet ./bundled-cli.sh 1.2.3 "$scratch/same" <<< "$app"
check "a bundle's CLI that is not the sidecar fails" fails ./bundled-cli.sh 1.2.3 "$scratch/different" <<< "$app"
# linuxdeploy rewrites the binaries it puts in an AppImage, so only its
# version counts.
cat > "$scratch/bundles/Social Deduction.AppImage" <<'TOOL'
#!/usr/bin/env bash
mkdir -p squashfs-root/usr/bin
printf '#!/bin/sh\necho "social-deduction 1.2.3"\n' > squashfs-root/usr/bin/social-deduction
chmod +x squashfs-root/usr/bin/social-deduction
TOOL
check "an AppImage's CLI need only say the version" quiet ./bundled-cli.sh 1.2.3 "$scratch/different" \
  <<< "[\"$scratch/bundles/Social Deduction.AppImage\"]"
# A bundle that doesn't open is a failure, not a bundle to skip.
echo "not an rpm" > "$scratch/bundles/broken.rpm"
check "a bundle that doesn't open fails" fails ./bundled-cli.sh 1.2.3 \
  <<< "[\"$scratch/bundles/Social Deduction.app\", \"$scratch/bundles/broken.rpm\"]"

[ "$failures" -eq 0 ]
