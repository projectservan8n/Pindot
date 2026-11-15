#!/bin/sh
# Build Pindot.app signed with a stable certificate, so macOS keeps the
# Accessibility, Input Monitoring, Screen Recording and Microphone grants
# across rebuilds. (An ad-hoc signature changes on every build, and macOS
# drops the grants each time.)
#
# Uses APPLE_SIGNING_IDENTITY if set, else the first "Developer ID
# Application" (then "Apple Development") identity in the keychain. The
# identity is never written into the repo.
set -eu
cd "$(dirname "$0")/../app"

if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  ids=$(security find-identity -v -p codesigning 2>/dev/null || true)
  for kind in "Developer ID Application" "Apple Development"; do
    APPLE_SIGNING_IDENTITY=$(printf '%s\n' "$ids" | sed -n "s/.*\"\($kind: .*\)\"/\1/p" | head -1)
    [ -n "$APPLE_SIGNING_IDENTITY" ] && break
  done
fi
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "WARNING: no code-signing identity found; the build is ad-hoc signed and" >&2
  echo "macOS will ask for every permission again after each rebuild." >&2
else
  export APPLE_SIGNING_IDENTITY
  echo "Signing with: $APPLE_SIGNING_IDENTITY"
fi

npx tauri build --bundles app "$@"

app=../target/release/bundle/macos/Pindot.app
codesign --verify --deep --strict "$app"
codesign -dv "$app" 2>&1 | grep -E '^(Authority=Developer|Authority=Apple Development|TeamIdentifier|Signature=adhoc)' | head -2
