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

generated=$'## What\'s Changed\n\nThe notes GitHub generated.'
notes=$(./release-notes.sh 1.2.3)

section=$'<!-- desktop-app -->\n'"$notes"$'\n<!-- /desktop-app -->'

# The notes go on top of the generated ones, between markers, with a blank
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
  "Social-Deduction-aarch64.AppImage Social-Deduction-setup.exe Social-Deduction-x86_64.AppImage Social-Deduction.dmg Social-Deduction.msi social-deduction-aarch64.rpm social-deduction-amd64.deb social-deduction-arm64.deb social-deduction-x86_64.rpm "
check "a copy is the installer it names" test "$(cat "$scratch/stable/Social-Deduction.dmg")" = "Social Deduction_1.2.3_universal.dmg"
check "an arm64 package is the arm64 build" test "$(cat "$scratch/stable/social-deduction-arm64.deb")" = "Social Deduction_1.2.3_arm64.deb"
check "an aarch64 AppImage is the aarch64 build" test "$(cat "$scratch/stable/Social-Deduction-aarch64.AppImage")" = "Social Deduction_1.2.3_aarch64.AppImage"
check "an x86_64 package is the x86_64 build" test "$(cat "$scratch/stable/social-deduction-x86_64.rpm")" = "Social Deduction-1.2.3-1.x86_64.rpm"
check "an x86_64 AppImage spelled amd64 is the x86_64 build" test "$(cat "$scratch/stable/Social-Deduction-x86_64.AppImage")" = "Social Deduction_1.2.3_amd64.AppImage"

# The website offers six of the installers: the .dmg, the setup.exe, and
# a .deb and an .rpm for each Linux architecture, each linked through the
# newest full release of social-deduction. The release notes' table names
# the same ones.
cp -R ../../site "$scratch/site"
cat > "$scratch/releases.json" <<'JSON'
[
  {"tag_name": "social-deduction-v0.3.0-rc.1", "draft": false, "prerelease": true},
  {"tag_name": "social-deduction-v0.4.0", "draft": true, "prerelease": false},
  {"tag_name": "social-deduction-v0.1.4", "draft": false, "prerelease": false},
  {"tag_name": "free-agent-v9.0.0", "draft": false, "prerelease": false},
  {"tag_name": "v0.9.0", "draft": false, "prerelease": false}
]
[
  {"tag_name": "social-deduction-v0.2.10", "draft": false, "prerelease": false},
  {"tag_name": "social-deduction-v0.2.9", "draft": false, "prerelease": false},
  {"tag_name": "social-deduction-gui-v5.0.0", "draft": false, "prerelease": false}
]
JSON
resolved=$(./site-releases.sh "$scratch/site" < "$scratch/releases.json")
check "the site resolves social-deduction to its newest full release" test "$resolved" = "social-deduction social-deduction-v0.2.10"
check "no placeholder is left" fails grep -rq '{{release:' "$scratch/site"
check "no link goes through releases/latest" fails grep -rq 'releases/latest' "$scratch/site"
check "the source page links through the placeholder" grep -q 'href="{{release:social-deduction}}/Social-Deduction.dmg"' ../../site/social-deduction.html
release=https://github.com/wpm/FreeAgent/releases/download/social-deduction-v0.2.10
check "a placeholder becomes the release's download address" grep -q "href=\"$release/Social-Deduction.dmg\"" "$scratch/site/social-deduction.html"
check "pages without placeholders are left alone" cmp -s ../../site/index.html "$scratch/site/index.html"
# Another product is resolved from its own releases, and a product with no
# full release fails, naming it.
mkdir -p "$scratch/two"
printf '<a href="{{release:free-agent}}/a.txt">a</a> <a href="{{release:social-deduction}}/b.txt">b</a>\n' > "$scratch/two/index.html"
check "each product resolves to its own release" test "$(./site-releases.sh "$scratch/two" < "$scratch/releases.json")" = \
  $'free-agent free-agent-v9.0.0\nsocial-deduction social-deduction-v0.2.10'
mkdir -p "$scratch/none"
printf '<a href="{{release:mathdoku}}/a.txt">a</a>\n' > "$scratch/none/index.html"
status=0; unknown=$(./site-releases.sh "$scratch/none" < "$scratch/releases.json" 2>&1 > /dev/null) || status=$?
check "a placeholder with no release fails" test "$status" -ne 0
check "the product with no release is named" grep -q "mathdoku" <<< "$unknown"
check "a site with no release fails before changing anything" grep -q '{{release:mathdoku}}' "$scratch/none/index.html"
# A link through releases/latest may reach a release of another product,
# so the placeholder is the only way to link to a release.
mkdir -p "$scratch/latest"
printf '<a href="https://github.com/wpm/FreeAgent/releases/latest/download/a.txt">a</a>\n' > "$scratch/latest/index.html"
status=0; latest=$(./site-releases.sh "$scratch/latest" < "$scratch/releases.json" 2>&1 > /dev/null) || status=$?
check "a link through releases/latest fails" test "$status" -ne 0
check "the page linking through releases/latest is named" grep -q "$scratch/latest/index.html" <<< "$latest"

site=$(./site-downloads.sh "$scratch/site" social-deduction-v0.2.10 | tr '\n' ' ')
check "the site offers the installers that put the command line on the PATH" test "$site" = \
  "Social-Deduction-setup.exe Social-Deduction.dmg social-deduction-aarch64.rpm social-deduction-amd64.deb social-deduction-arm64.deb social-deduction-x86_64.rpm "
check "every download on the site has a stable name" quiet ./site-downloads.sh "$scratch/site" social-deduction-v0.2.10 <(ls "$scratch/stable")
check "the notes' table names the site's downloads" test \
  "$(awk -F' [|] ' '/^\| / { print $2 }' <<< "$notes" | grep -oE '[[:alnum:]_.-]+[.](dmg|exe|deb|rpm)' | LC_ALL=C sort | tr '\n' ' ')" = "$site"
check "the notes say the installers carry the command line" grep -q "Each download gives you the app and the \`social-deduction\` command line" <<< "$notes"
check "the notes send the command line alone to the README" grep -q "Command line only, for servers and CI: see the \[README\](https://github.com/wpm/FreeAgent/tree/main/apps/social-deduction/cli#servers-and-ci)." <<< "$notes"
# A download the release doesn't have fails, and says which.
rm "$scratch/stable/social-deduction-arm64.deb"
status=0; missing=$(./site-downloads.sh "$scratch/site" social-deduction-v0.2.10 <(ls "$scratch/stable") 2>&1 > /dev/null) || status=$?
check "a download missing from the release fails" test "$status" -ne 0
check "the missing download is named" test "$missing" = "not among the release's assets: social-deduction-arm64.deb"
check "a site with no downloads from the release fails" fails ./site-downloads.sh "$scratch/site" social-deduction-v0.2.9
check "a site with no downloads at all fails" fails ./site-downloads.sh "$scratch/none" social-deduction-v0.2.10

# A package for an architecture with no stable name fails.
echo riscv > "$scratch/built/Social Deduction_1.2.3_riscv64.deb"
check "an unknown architecture fails" fails ./stable-names.sh "$scratch/riscv" <<< "[\"$scratch/built/Social Deduction_1.2.3_riscv64.deb\"]"

# Lines that end in a carriage return, as jq writes them on Windows, match too.
./stable-names.sh "$scratch/crlf" <<< "[\"$scratch/built/Social Deduction_1.2.3_x64_en-US.msi\r\"]" > /dev/null
check "a carriage return is ignored" test -f "$scratch/crlf/Social-Deduction.msi"

# No installer at all is a failed build.
check "no installer fails" fails ./stable-names.sh "$scratch/none" <<< "[\"$scratch/built/Social Deduction.app\"]"

# sidecar.sh, with stand-ins for the tools it runs: cargo writes what it
# was asked to build, and lipo joins its inputs.
tools="$scratch/tools"
mkdir -p "$tools"
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
chmod +x "$tools"/*
sidecar() { # <directory> <argument>...
  local out=$1; shift
  PATH="$tools:$PATH" CARGO_TARGET_DIR="$scratch/target" \
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

check "there is no release mode" fails sidecar "$scratch/gone" --release social-deduction-v1.2.3

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


# stamp-version.sh, in a workspace with the crates of both products, each
# at 0.0.0 as on main.
workspace="$scratch/workspace"
crate() { # <directory> <name>
  mkdir -p "$workspace/$1/src"
  printf '[package]\nname = "%s"\n# Stamped from the tag.\nversion = "0.0.0"\nedition = "2024"\n' "$2" > "$workspace/$1/Cargo.toml"
  touch "$workspace/$1/src/lib.rs"
}
crate . free-agent
printf '\n[workspace]\nmembers = ["apps/*/cli", "apps/*/gui", "apps/*/gui/src-tauri", "apps/*/gui-types"]\nresolver = "3"\n' >> "$workspace/Cargo.toml"
crate apps/social-deduction/cli social-deduction
crate apps/social-deduction/gui social-deduction-gui
crate apps/social-deduction/gui/src-tauri social-deduction-desktop
crate apps/social-deduction/gui-types social-deduction-gui-types
crate apps/other/cli other
cp -R "$workspace" "$scratch/pristine"
scripts=$PWD
stamp() { # <tag>, in a fresh copy of the workspace
  rm -rf "$workspace" && cp -R "$scratch/pristine" "$workspace"
  (cd "$workspace" && GITHUB_OUTPUT='' "$scripts/stamp-version.sh" "$@")
}
versions() { # every crate's version in the workspace, sorted by name
  (cd "$workspace" && cargo metadata --no-deps --format-version 1 --offline) \
    | jq -r '.packages | sort_by(.name)[] | "\(.name) \(.version)"' | tr '\n' ' '
}
locked() { # every workspace crate's version in Cargo.lock
  awk '/^name = / { name = $3 } /^version = / && name { print name, $3 }' "$workspace/Cargo.lock" | tr -d '"' | LC_ALL=C sort | tr '\n' ' '
}

output=$(stamp social-deduction-v0.2.0 2> /dev/null)
check "stamping social-deduction reports the product and version" test "$output" = $'product=social-deduction\nversion=0.2.0'
check "stamping social-deduction sets its four crates" test "$(versions)" = \
  "free-agent 0.0.0 other 0.0.0 social-deduction 0.2.0 social-deduction-desktop 0.2.0 social-deduction-gui 0.2.0 social-deduction-gui-types 0.2.0 "
check "the lock file agrees" test "$(locked)" = "$(versions)"
check "the comment above the version stays" grep -q '^# Stamped from the tag.' "$workspace/apps/social-deduction/cli/Cargo.toml"
stamp free-agent-v0.3.0 > /dev/null 2>&1
check "stamping free-agent sets only free-agent" test "$(versions)" = \
  "free-agent 0.3.0 other 0.0.0 social-deduction 0.0.0 social-deduction-desktop 0.0.0 social-deduction-gui 0.0.0 social-deduction-gui-types 0.0.0 "
stamp free-agent-v0.3.0-rc.1 > /dev/null 2>&1
check "a prerelease version is stamped as it is" test "$(versions)" = \
  "free-agent 0.3.0-rc.1 other 0.0.0 social-deduction 0.0.0 social-deduction-desktop 0.0.0 social-deduction-gui 0.0.0 social-deduction-gui-types 0.0.0 "
(cd "$workspace" && GITHUB_OUTPUT="$scratch/github-output" "$scripts/stamp-version.sh" free-agent-v1.0.0 > /dev/null 2>&1)
check "the outputs go to GITHUB_OUTPUT when it is set" test "$(cat "$scratch/github-output")" = $'product=free-agent\nversion=1.0.0'
for tag in mathdoku-v1.0.0 social-deduction-1.0.0 social-deduction-v1.0 v1.0.0 social-deduction-v01.0.0 ""; do
  status=0; said=$(stamp "$tag" 2>&1 > /dev/null) || status=$?
  check "the tag '$tag' fails" test "$status" -ne 0
  check "the tag '$tag' is named" grep -qF "'$tag'" <<< "$said"
done
stamp mathdoku-v1.0.0 > /dev/null 2>&1 || true
check "a failed stamp changes nothing" test "$(versions)" = \
  "free-agent 0.0.0 other 0.0.0 social-deduction 0.0.0 social-deduction-desktop 0.0.0 social-deduction-gui 0.0.0 social-deduction-gui-types 0.0.0 "

# cli-archive.sh packs the command line with the example configurations,
# the README and the licenses.
printf 'the binary\n' > "$scratch/social-deduction-x86_64-unknown-linux-gnu"
printf 'the Windows binary\n' > "$scratch/social-deduction-x86_64-pc-windows-msvc.exe"
archives=$(./cli-archive.sh x86_64-unknown-linux-gnu "$scratch/social-deduction-x86_64-unknown-linux-gnu" "$scratch/archives")
check "an archive and its checksum are printed" test "$archives" = \
  "$scratch/archives/social-deduction-x86_64-unknown-linux-gnu.tar.gz"$'\n'"$scratch/archives/social-deduction-x86_64-unknown-linux-gnu.tar.gz.sha256"
listing=$(tar -tzf "$scratch/archives/social-deduction-x86_64-unknown-linux-gnu.tar.gz" | grep -v '/$' | grep -v '/configs/.*/' | LC_ALL=C sort | tr '\n' ' ')
check "a tarball holds the binary, the README and the licenses in a folder named for it" test "$listing" = \
  "social-deduction-x86_64-unknown-linux-gnu/LICENSE-APACHE social-deduction-x86_64-unknown-linux-gnu/LICENSE-MIT social-deduction-x86_64-unknown-linux-gnu/README.md social-deduction-x86_64-unknown-linux-gnu/social-deduction "
check "a tarball holds the example configurations" quiet \
  tar -xzOf "$scratch/archives/social-deduction-x86_64-unknown-linux-gnu.tar.gz" social-deduction-x86_64-unknown-linux-gnu/configs/werewolf/random.toml
check "a tarball's binary is the one given" test \
  "$(tar -xzOf "$scratch/archives/social-deduction-x86_64-unknown-linux-gnu.tar.gz" social-deduction-x86_64-unknown-linux-gnu/social-deduction)" = "the binary"
check "a tarball's README is the command line's" test \
  "$(tar -xzOf "$scratch/archives/social-deduction-x86_64-unknown-linux-gnu.tar.gz" social-deduction-x86_64-unknown-linux-gnu/README.md)" = "$(cat ../../apps/social-deduction/cli/README.md)"
sums() { # <directory> <checksum file>
  (cd "$1" && if command -v sha256sum > /dev/null; then sha256sum -c "$2"; else shasum -a 256 -c "$2"; fi) > /dev/null 2>&1
}
check "a checksum checks out" sums \
  "$scratch/archives" social-deduction-x86_64-unknown-linux-gnu.tar.gz.sha256
quiet ./cli-archive.sh x86_64-pc-windows-msvc "$scratch/social-deduction-x86_64-pc-windows-msvc.exe" "$scratch/archives"
check "a Windows archive is a zip with the binary at its top" test \
  "$(unzip -p "$scratch/archives/social-deduction-x86_64-pc-windows-msvc.zip" social-deduction.exe)" = "the Windows binary"
check "a zip holds the README" quiet unzip -l "$scratch/archives/social-deduction-x86_64-pc-windows-msvc.zip" README.md
check "a zip has its checksum" test -s "$scratch/archives/social-deduction-x86_64-pc-windows-msvc.zip.sha256"
check "a missing binary fails" fails ./cli-archive.sh x86_64-unknown-linux-gnu "$scratch/nothing" "$scratch/archives"

[ "$failures" -eq 0 ]
