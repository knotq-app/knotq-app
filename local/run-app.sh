#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ENV_FILE="$ROOT/local/.env"
if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck source=/dev/null
  source "$ENV_FILE"
  set +a
fi

PROFILE="${KNOTQ_PROFILE:-debug}"
PACKAGE="${KNOTQ_PACKAGE:-knotq-app}"
APP_PATH="target/${PROFILE}/bundle/osx/KnotQ.app"
BUNDLE_ID="${KNOTQ_BUNDLE_ID:-com.enigmadux.knotq}"
OPEN_ENV_ARGS=()

for name in \
  KNOTQ_GOOGLE_CLIENT_ID \
  KNOTQ_GOOGLE_CLIENT_SECRET \
  GOOGLE_CLIENT_ID \
  GOOGLE_CLIENT_SECRET; do
  if [[ -n "${!name:-}" ]]; then
    OPEN_ENV_ARGS+=(--env "$name=${!name}")
  fi
done

if [[ -z "${KNOTQ_GOOGLE_CLIENT_ID:-${GOOGLE_CLIENT_ID:-}}" ]]; then
  echo "Google OAuth is not configured. Set KNOTQ_GOOGLE_CLIENT_ID in local/.env to enable Google Calendar import." >&2
fi

if [[ "$PROFILE" == "release" ]]; then
  cargo bundle -p "$PACKAGE" --release
else
  cargo bundle -p "$PACKAGE"
fi

plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "$APP_PATH/Contents/Info.plist"
if [[ "${KNOTQ_REFRESH_BUNDLE_VERSION:-0}" == "1" ]]; then
  plutil -replace CFBundleVersion -string "$(date +%Y%m%d.%H%M%S)" "$APP_PATH/Contents/Info.plist"
else
  plutil -replace CFBundleVersion -string "${KNOTQ_BUNDLE_VERSION:-0.1.0}" "$APP_PATH/Contents/Info.plist"
fi
rm -f "$APP_PATH/Contents/embedded.provisionprofile"
codesign \
  --force \
  --deep \
  --sign - \
  --identifier "$BUNDLE_ID" \
  "$APP_PATH"
codesign --verify --deep --strict "$APP_PATH"

osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
open -n "${OPEN_ENV_ARGS[@]}" "$APP_PATH"
