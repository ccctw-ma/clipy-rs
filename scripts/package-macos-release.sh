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

APP_DIR="$PROJECT_DIR/target/macos-app/$APP_NAME.app"
DMG_PATH="$PROJECT_DIR/target/macos-dmg/$DMG_NAME.dmg"

# 1. 构建 .app（package-macos-app.sh 会处理图标）
"$PROJECT_DIR/scripts/package-macos-app.sh"

# 2. 签名 .app（必须在打包成 dmg 之前完成）
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

# 3. 复用 dmg 脚本生成 dmg（跳过其内部的 .app 重新构建）
SKIP_APP_BUILD=1 "$PROJECT_DIR/scripts/package-macos-dmg.sh"

# 4. 签名 dmg
echo "Signing dmg: $DMG_PATH"
codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG_PATH"
codesign --verify --verbose=2 "$DMG_PATH"
hdiutil verify "$DMG_PATH"

# 5. 公证
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

# 6. Staple 公证票据
echo "Stapling app and dmg"
xcrun stapler staple "$APP_DIR"
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$APP_DIR"
xcrun stapler validate "$DMG_PATH"

echo "Created signed and notarized dmg: $DMG_PATH"
echo "Open: open \"$DMG_PATH\""
