#!/usr/bin/env bash
# Puts a product's command line where its desktop app's bundle picks it
# up as a sidecar (bundle.externalBin in tauri.conf.json), under Tauri's
# name for it: binaries/<product>-<target triple>, with .exe on Windows.
#
#   sidecar.sh [<product>] [--target <triple>] [--debug]
#
# The product is social-deduction by default, or another of product.sh's
# desktop apps.
# The target is the host's by default. cargo builds it, optimized, or with
# --debug as a debug build that shares what `cargo test` builds.
# universal-apple-darwin is the two macOS binaries joined with lipo, and
# the two stay beside it, since cargo builds the app once for each chip
# and Tauri looks for each chip's sidecar then. Prints each file it
# writes, the target's last.
#
# Environment:
#   SIDECAR_DIR       where the sidecars go (default
#                     apps/<product>/gui/src-tauri/binaries)
#   CARGO_TARGET_DIR  where cargo builds (default the workspace's target)
set -euo pipefail

usage="usage: sidecar.sh [<product>] [--target <triple>] [--debug]"
product=social-deduction
# shellcheck source=SCRIPTDIR/product.sh
. "$(dirname "$0")/product.sh"
target=
profile=release
while [ $# -gt 0 ]; do
  case $1 in
    --target) target=${2:?$usage}; shift 2 ;;
    --debug) profile=debug; shift ;;
    -*) echo "$usage" >&2; exit 2 ;;
    *) product_names "$1" || exit 2; product=$1; shift ;;
  esac
done
# Built for the host without --target, cargo puts the binary where its
# other builds for the host go, and reuses them.
host=$(rustc -vV | sed -n 's/^host: //p')
host=${host%$'\r'}
target=${target:-$host}

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${SIDECAR_DIR:-$root/apps/$product/gui/src-tauri/binaries}
mkdir -p "$out"

exe() { # <triple>
  case $1 in *windows*) echo .exe ;; esac
}
name() { # <triple>
  echo "$product-$1$(exe "$1")"
}

# Puts the binary for one target into the sidecar directory.
one() { # <triple>
  local triple=$1 built=${CARGO_TARGET_DIR:-$root/target} args=(--locked -p "$product" --bin "$product")
  if [ "$profile" = release ]; then args+=(--release); fi
  if [ "$triple" != "$host" ]; then args+=(--target "$triple"); built=$built/$triple; fi
  cargo build "${args[@]}"
  cp "$built/$profile/$product$(exe "$triple")" "$out/$(name "$triple")"
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
