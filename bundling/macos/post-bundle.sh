#!/usr/bin/env bash
set -euo pipefail

# post-bundle.sh <version> <build-number> <arch> <app>
# Copies a single-architecture KnotQ.app, signs it, packages it as an
# architecture-specific DMG, and notarizes the DMG when credentials are present.
#
# Signing:
#   CODESIGN_IDENTITY             Developer ID Application signing identity
#   CODESIGN_ENTITLEMENTS         entitlements plist path
#   MACOS_PROVISIONING_PROFILE    base64-encoded provisioning profile
#   MACOS_PROVISIONING_PROFILE_PATH
#                                 path to provisioning profile
#
# Notarization, either:
#   APPLE_NOTARY_KEYCHAIN_PROFILE
# or all of:
#   APPLE_NOTARY_APPLE_ID
#   APPLE_NOTARY_PASSWORD
#   APPLE_NOTARY_TEAM_ID

VERSION="${1:?usage: $0 <version> <build-number> <arch> <app>}"
BUILD_NUMBER="${2:?usage: $0 <version> <build-number> <arch> <app>}"
ARCH="${3:?usage: $0 <version> <build-number> <arch> <app>}"
APP_INPUT="${4:?usage: $0 <version> <build-number> <arch> <app>}"

case "$ARCH" in
    arm64 | x86_64) ;;
    *) echo "[error] unsupported macOS architecture: $ARCH" >&2; exit 1 ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

abs_path() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$ROOT" "$1" ;;
    esac
}

set_plist_string() {
    local plist="$1"
    local key="$2"
    local value="$3"
    plutil -replace "$key" -string "$value" "$plist" 2>/dev/null \
        || plutil -insert "$key" -string "$value" "$plist"
}

INPUT_APP="$(abs_path "$APP_INPUT")"
OUT_DIR="$ROOT/target/package-macos/$ARCH"
APP="$OUT_DIR/KnotQ.app"
DIST="$ROOT/dist/macos"
DMG="$DIST/KnotQ-$VERSION-macos-$ARCH.dmg"

[[ -d "$INPUT_APP" ]] || { echo "[error] $ARCH app bundle not found: $INPUT_APP" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$OUT_DIR"
ditto --noextattr --norsrc "$INPUT_APP" "$APP"
mkdir -p "$APP/Contents/Resources/assets"
ditto --noextattr --norsrc "$ROOT/desktop/app/assets" "$APP/Contents/Resources/assets"
find "$APP/Contents/Resources/assets" -name .DS_Store -delete

EXE_NAME="$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$APP/Contents/Info.plist")"
EXE="$APP/Contents/MacOS/$EXE_NAME"

[[ -f "$EXE" ]] || { echo "[error] $ARCH executable not found: $EXE" >&2; exit 1; }
chmod +x "$EXE"
lipo "$EXE" -verify_arch "$ARCH"

PLIST="$APP/Contents/Info.plist"
set_plist_string "$PLIST" CFBundleIdentifier "com.enigmadux.knotq"
set_plist_string "$PLIST" CFBundleShortVersionString "$VERSION"
set_plist_string "$PLIST" CFBundleVersion "$BUILD_NUMBER"
set_plist_string "$PLIST" LSMinimumSystemVersion "11.0"
set_plist_string "$PLIST" NSUserNotificationAlertStyle "alert"

if [[ -n "${MACOS_PROVISIONING_PROFILE_PATH:-}" ]]; then
    cp "$MACOS_PROVISIONING_PROFILE_PATH" "$APP/Contents/embedded.provisionprofile"
elif [[ -n "${MACOS_PROVISIONING_PROFILE:-}" ]]; then
    printf '%s' "$MACOS_PROVISIONING_PROFILE" | base64 -D > "$APP/Contents/embedded.provisionprofile"
else
    rm -f "$APP/Contents/embedded.provisionprofile"
fi

notary_requested=false
if [[ -n "${APPLE_NOTARY_KEYCHAIN_PROFILE:-}" ]] || {
    [[ -n "${APPLE_NOTARY_APPLE_ID:-}" ]] \
        && [[ -n "${APPLE_NOTARY_PASSWORD:-}" ]] \
        && [[ -n "${APPLE_NOTARY_TEAM_ID:-}" ]]
}; then
    notary_requested=true
fi

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    ENTITLEMENTS="${CODESIGN_ENTITLEMENTS:-$ROOT/bundling/macos/KnotQ.entitlements}"
    if ! security find-identity -v -p codesigning | grep -F "$CODESIGN_IDENTITY" >/dev/null; then
        echo "[error] codesign identity not found in keychain: $CODESIGN_IDENTITY" >&2
        security find-identity -v -p codesigning >&2 || true
        exit 1
    fi
    xattr -cr "$APP"
    codesign --force --deep --timestamp --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --sign "$CODESIGN_IDENTITY" \
        "$APP"
    codesign --verify --deep --strict --verbose=2 "$APP"
elif [[ "$notary_requested" == true ]]; then
    echo "[error] notarization credentials were provided, but CODESIGN_IDENTITY is empty" >&2
    exit 1
else
    xattr -cr "$APP" 2>/dev/null || true
    codesign --force --deep --sign - "$APP"
    codesign --verify --deep --strict "$APP"
fi

mkdir -p "$DIST"
rm -f "$DMG"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
ditto --noextattr --norsrc "$APP" "$stage/KnotQ.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname KnotQ -srcfolder "$stage" -ov -format UDZO "$DMG"

if [[ -n "${APPLE_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
    xcrun notarytool submit "$DMG" --wait --keychain-profile "$APPLE_NOTARY_KEYCHAIN_PROFILE"
    xcrun stapler staple "$DMG"
    xcrun stapler validate "$DMG"
elif [[ -n "${APPLE_NOTARY_APPLE_ID:-}" && -n "${APPLE_NOTARY_PASSWORD:-}" && -n "${APPLE_NOTARY_TEAM_ID:-}" ]]; then
    xcrun notarytool submit "$DMG" --wait \
        --apple-id "$APPLE_NOTARY_APPLE_ID" \
        --password "$APPLE_NOTARY_PASSWORD" \
        --team-id "$APPLE_NOTARY_TEAM_ID"
    xcrun stapler staple "$DMG"
    xcrun stapler validate "$DMG"
fi

echo "[ok] $DMG"
