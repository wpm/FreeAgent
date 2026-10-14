#!/usr/bin/env bash
# Hands Apple's signing and notarization credentials to the rest of the
# job, but only the ones that are set.
#
# Tauri signs the macOS bundle when APPLE_CERTIFICATE and its password
# are in the environment, and notarizes it when the Apple ID or the App
# Store Connect API key is there too; when a variable is missing it does
# neither. But a workflow that writes `${{ secrets.X }}` into the
# environment sets X to the empty string when the secret does not exist,
# and Tauri takes an empty certificate as one to import and fails on it.
# So the workflow sets them on this script alone, which passes on the
# non-empty ones through GITHUB_ENV and says which mode the build is in,
# so a release made without the secrets is an unsigned one and not a
# failed one. The mode goes to GITHUB_OUTPUT as well, for the release
# notes, so that they describe the build that is made.
#
# Environment:
#   APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD, APPLE_SIGNING_IDENTITY
#       the Developer ID Application certificate as a base64 .p12, its
#       export password, and (optional) its name in the keychain
#   APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID
#       notarization with an Apple ID and an app-specific password
#   APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH
#       notarization with an App Store Connect API key instead
#   GITHUB_ENV      where the set ones go (stdout without it)
#   GITHUB_OUTPUT   where signed=true|false and notarized=true|false go
set -euo pipefail

names=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY
  APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
  APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH)
for name in "${names[@]}"; do
  value=${!name:-}
  [ -n "$value" ] || continue
  # The heredoc form, in case a value has a line break in it.
  printf '%s<<__%s__\n%s\n__%s__\n' "$name" "$name" "$value" "$name" >> "${GITHUB_ENV:-/dev/stdout}"
done

has() { [ -n "${!1:-}" ]; }
signed=false
notarized=false
if has APPLE_CERTIFICATE && has APPLE_CERTIFICATE_PASSWORD; then
  signed=true
  if { has APPLE_ID && has APPLE_PASSWORD && has APPLE_TEAM_ID; } \
      || { has APPLE_API_KEY && has APPLE_API_ISSUER && has APPLE_API_KEY_PATH; }; then
    notarized=true
    echo "The macOS bundle will be signed and notarized."
  else
    echo "::warning::The macOS bundle will be signed but not notarized: set APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID (or the App Store Connect API key) to notarize it."
  fi
else
  echo "::warning::The macOS bundle will not be signed: set APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD to sign it. Users will have to clear the quarantine attribute to open it."
fi
printf 'signed=%s\nnotarized=%s\n' "$signed" "$notarized" >> "${GITHUB_OUTPUT:-/dev/stdout}"
