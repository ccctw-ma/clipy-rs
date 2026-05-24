#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-Clipy RS}"
DMG_NAME="${DMG_NAME:-$APP_NAME}"
SIGN_IDENTITY="${SIGN_IDENTITY:-}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"
APPLE_ID="${APPLE_ID:-}"
APPLE_TEAM_ID="${APPLE_TEAM_ID:-}"
APPLE_APP_PASSWORD="${APPLE_APP_PASSWORD:-}"
ENTITLEMENTS_PATH="${ENTITLEMENTS_PATH:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: signed and notarized macOS releases can only be built on macOS" >&2
  exit 1
fi

if [[ -z "$SIGN_IDENTITY" ]]; then
  echo "error: SIGN_IDENTITY is required, for example: Developer ID Application: Your Name (TEAMID)" >&2
  exit 1
fi

if [[ -z "$NOTARY_PROFILE" && ( -z "$APPLE_ID" || -z "$APPLE_TEAM_ID" || -z "$APPLE_APP_PASSWORD" ) ]]; then
  echo "error: set NOTARY_PROFILE, or set APPLE_ID, APPLE_TEAM_ID, and APPLE_APP_PASSWORD" >&2
  exit 1
fi

if [[ -n "$ENTITLEMENTS_PATH" && ! -f "$ENTITLEMENTS_PATH" ]]; then
  echo "error: ENTITLEMENTS_PATH does not exist: $ENTITLEMENTS_PATH" >&2
  exit 1
fi

"$PROJECT_DIR/scripts/package-macos-app.sh"

APP_ROOT="$PROJECT_DIR/target/macos-app"
APP_DIR="$APP_ROOT/$APP_NAME.app"
DMG_ROOT="$PROJECT_DIR/target/macos-dmg"
STAGING_DIR="$DMG_ROOT/staging"
DMG_PATH="$DMG_ROOT/$DMG_NAME.dmg"

if [[ ! -d "$APP_DIR" ]]; then
  echo "error: app bundle not found at $APP_DIR" >&2
  exit 1
fi

SIGN_ARGS=(
  --force
  --timestamp
  --options runtime
  --sign "$SIGN_IDENTITY"
)

if [[ -n "$ENTITLEMENTS_PATH" ]]; then
  SIGN_ARGS+=(--entitlements "$ENTITLEMENTS_PATH")
fi

echo "Signing app: $APP_DIR"
codesign "${SIGN_ARGS[@]}" "$APP_DIR"
codesign --verify --strict --verbose=2 "$APP_DIR"
spctl --assess --type execute --verbose=2 "$APP_DIR" || true

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR" "$DMG_ROOT"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

rm -f "$DMG_PATH"
hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH"

echo "Signing dmg: $DMG_PATH"
codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG_PATH"
codesign --verify --verbose=2 "$DMG_PATH"
hdiutil verify "$DMG_PATH"

echo "Submitting dmg for notarization"
if [[ -n "$NOTARY_PROFILE" ]]; then
  xcrun notarytool submit "$DMG_PATH" \
    --keychain-profile "$NOTARY_PROFILE" \
    --wait
else
  xcrun notarytool submit "$DMG_PATH" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" \
    --wait
fi

echo "Stapling app and dmg"
xcrun stapler staple "$APP_DIR"
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$APP_DIR"
xcrun stapler validate "$DMG_PATH"

echo "Created signed and notarized dmg: $DMG_PATH"
echo "Open: open \"$DMG_PATH\""
