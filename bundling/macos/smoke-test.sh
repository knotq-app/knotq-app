#!/usr/bin/env bash
set -euo pipefail

# smoke-test.sh <dmg>
# Installs KnotQ from the release DMG the way a user would and fails unless the
# signature holds and the app starts. Set KNOTQ_SMOKE_REQUIRE_SIGNED=1 to also
# require Gatekeeper to accept it.

DMG="${1:?usage: $0 <dmg>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT/bundling/smoke-test-common.sh"

work="$(mktemp -d)"
mount="$work/mnt"
trap 'hdiutil detach -quiet "$mount" 2>/dev/null || true; rm -rf "$work"' EXIT

hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$DMG" >/dev/null
cp -R "$mount/KnotQ.app" "$work/"
hdiutil detach -quiet "$mount"
APP="$work/KnotQ.app"

codesign --verify --deep --strict "$APP"
if [[ "${KNOTQ_SMOKE_REQUIRE_SIGNED:-0}" == 1 ]]; then
    spctl --assess --type execute --verbose "$APP"
fi

smoke_launch "$APP/Contents/MacOS/knotq" "$work/run"
echo "[ok] macOS smoke test passed"
