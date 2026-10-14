#!/usr/bin/env bash
# The version a release is of, from the crate that carries it, checked
# against the tag that asked for the release.
#
#   release-version.sh <tag prefix> <package>... [--tag <tag>]
#
# Prints the version of the named workspace packages, which must all be
# the same; the desktop app has a crate for the frontend and one for the
# shell, and a release is of both. With --tag, the tag must be exactly
# the prefix followed by that version, so a tag that does not match what
# the manifests say stops the release before anything is built. Without
# it, a trial run, the version is only reported.
#
#   release-version.sh v social-deduction-desktop social-deduction-gui --tag v0.1.0
#   release-version.sh v social-deduction-desktop social-deduction-gui
#
# Environment:
#   GITHUB_OUTPUT   where the version output goes (stdout without it)
set -euo pipefail

prefix=${1:?usage: release-version.sh <tag prefix> <package>... [--tag <tag>]}
shift
packages=()
tag=
while [ $# -gt 0 ]; do
  case $1 in
    --tag) tag=${2:?--tag needs a tag}; shift 2 ;;
    *) packages+=("$1"); shift ;;
  esac
done
[ ${#packages[@]} -gt 0 ] || { echo "no packages named" >&2; exit 2; }

metadata=$(cargo metadata --no-deps --format-version 1)
version=
for package in "${packages[@]}"; do
  found=$(jq -r --arg name "$package" '.packages[] | select(.name == $name) | .version' <<< "$metadata")
  [ -n "$found" ] || { echo "no package named $package in the workspace" >&2; exit 1; }
  if [ -z "$version" ]; then
    version=$found
  elif [ "$found" != "$version" ]; then
    echo "$package is $found but ${packages[0]} is $version; a release needs them to agree" >&2
    exit 1
  fi
done

if [ -n "$tag" ] && [ "$tag" != "$prefix$version" ]; then
  echo "tag $tag does not match the manifests, which say $prefix$version" >&2
  exit 1
fi

echo "${packages[*]}: $version${tag:+ (tag $tag)}" >&2
echo "version=$version" >> "${GITHUB_OUTPUT:-/dev/stdout}"
