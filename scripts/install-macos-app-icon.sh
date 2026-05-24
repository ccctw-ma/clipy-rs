#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-Clipy RS}"
APP_ROOT="${APP_ROOT:-$PROJECT_DIR/target/macos-app}"
APP_DIR="${APP_DIR:-$APP_ROOT/$APP_NAME.app}"
ICON_PATH="${ICON_PATH:-$PROJECT_DIR/icons/AppIcon.icns}"
ICON_FILE="${ICON_FILE:-$(basename "$ICON_PATH")}"
ICON_NAME="${ICON_FILE%.icns}"

INFO_PLIST="$APP_DIR/Contents/Info.plist"
RESOURCES_DIR="$APP_DIR/Contents/Resources"
PLIST_BUDDY="/usr/libexec/PlistBuddy"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS app icons can only be installed on macOS" >&2
  exit 1
fi

if [[ ! -d "$APP_DIR" ]]; then
  echo "error: app bundle not found: $APP_DIR" >&2
  echo "hint: build the app bundle first, for example: scripts/package-macos-app.sh" >&2
  exit 1
fi

if [[ ! -f "$INFO_PLIST" ]]; then
  echo "error: Info.plist not found: $INFO_PLIST" >&2
  exit 1
fi

if [[ ! -f "$ICON_PATH" ]]; then
  echo "error: icon file not found: $ICON_PATH" >&2
  echo "hint: generate it first, for example: iconutil -c icns icons/AppIcon.iconset -o icons/AppIcon.icns" >&2
  exit 1
fi

if [[ "${ICON_FILE##*.}" != "icns" ]]; then
  echo "error: ICON_FILE must end with .icns: $ICON_FILE" >&2
  exit 1
fi

mkdir -p "$RESOURCES_DIR"
cp "$ICON_PATH" "$RESOURCES_DIR/$ICON_FILE"

if "$PLIST_BUDDY" -c "Print :CFBundleIconFile" "$INFO_PLIST" >/dev/null 2>&1; then
  "$PLIST_BUDDY" -c "Set :CFBundleIconFile $ICON_NAME" "$INFO_PLIST"
else
  "$PLIST_BUDDY" -c "Add :CFBundleIconFile string $ICON_NAME" "$INFO_PLIST"
fi

plutil -lint "$INFO_PLIST" >/dev/null

echo "Installed icon: $RESOURCES_DIR/$ICON_FILE"
echo "Updated Info.plist: CFBundleIconFile=$ICON_NAME"
