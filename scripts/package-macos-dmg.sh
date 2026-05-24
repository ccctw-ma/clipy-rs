#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-Clipy RS}"
DMG_NAME="${DMG_NAME:-$APP_NAME}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS dmg files can only be built on macOS" >&2
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

echo "Created $DMG_PATH"
echo "Open: open \"$DMG_PATH\""
