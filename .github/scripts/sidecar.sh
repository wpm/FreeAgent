#!/usr/bin/env bash
# Puts the social-deduction command line where the desktop app's bundle
# picks it up as a sidecar (bundle.externalBin in tauri.conf.json), under
# Tauri's name for it: binaries/social-deduction-<target triple>, with
# .exe on Windows.
#
#   sidecar.sh [--target <triple>] [--release <tag> | --debug]
#
# The target is the host's by default. On a release, the sidecar is the
# binary dist built, taken from its archive on the release with <tag>;
# otherwise cargo builds it here, optimized, or with --debug as a debug
# build that shares what `cargo test` builds. universal-apple-darwin is
# the two macOS binaries joined with lipo, and the two stay beside it,
# since cargo builds the app once for each chip and Tauri looks for each
# chip's sidecar then. Prints each file it writes, the target's last.
#
# Environment:
#   SIDECAR_DIR       where the sidecars go (default the desktop app's
#                     src-tauri/binaries)
#   CARGO_TARGET_DIR  where cargo builds (default the workspace's target)
#   GH_TOKEN          for gh, on a release
set -euo pipefail

usage="usage: sidecar.sh [--target <triple>] [--release <tag> | --debug]"
target=
tag=
profile=release
while [ $# -gt 0 ]; do
  case $1 in
    --target) target=${2:?$usage}; shift 2 ;;
    --release) tag=${2:?$usage}; shift 2 ;;
    --debug) profile=debug; shift ;;
    *) echo "$usage" >&2; exit 2 ;;
  esac
done
# Built for the host without --target, cargo puts the binary where its
# other builds for the host go, and reuses them.
host=$(rustc -vV | sed -n 's/^host: //p')
host=${host%$'\r'}
target=${target:-$host}

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${SIDECAR_DIR:-$root/apps/social-deduction-gui/src-tauri/binaries}
mkdir -p "$out"

exe() { # <triple>
  case $1 in *windows*) echo .exe ;; esac
}
name() { # <triple>
  echo "social-deduction-$1$(exe "$1")"
}

# Puts the binary for one target into the sidecar directory.
one() { # <triple>
  local triple=$1 binary
  binary=social-deduction$(exe "$triple")
  if [ -n "$tag" ]; then
    local scratch archive=social-deduction-$triple.tar.xz
    case $triple in *windows*) archive=social-deduction-$triple.zip ;; esac
    scratch=$(mktemp -d)
    gh release download "$tag" -p "$archive" -D "$scratch"
    mkdir "$scratch/x"
    case $archive in
      *.zip) unzip -q "$scratch/$archive" -d "$scratch/x" ;;
      *) tar -xJf "$scratch/$archive" -C "$scratch/x" ;;
    esac
    # dist's tarballs hold a directory named for the archive; its zips don't.
    cp "$(find "$scratch/x" -type f -name "$binary" | head -n 1)" "$out/$(name "$triple")"
    rm -rf "$scratch"
  else
    local built=${CARGO_TARGET_DIR:-$root/target} args=(--locked -p social-deduction --bin social-deduction)
    if [ "$profile" = release ]; then args+=(--release); fi
    if [ "$triple" != "$host" ]; then args+=(--target "$triple"); built=$built/$triple; fi
    cargo build "${args[@]}"
    cp "$built/$profile/$binary" "$out/$(name "$triple")"
  fi
  echo "$out/$(name "$triple")"
}

if [ "$target" = universal-apple-darwin ]; then
  one aarch64-apple-darwin
  one x86_64-apple-darwin
  lipo -create "$out/$(name aarch64-apple-darwin)" "$out/$(name x86_64-apple-darwin)" \
    -output "$out/$(name universal-apple-darwin)"
  echo "$out/$(name universal-apple-darwin)"
else
  one "$target"
fi
