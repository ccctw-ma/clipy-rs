#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-Clipy RS}"
BUNDLE_ID="${BUNDLE_ID:-dev.clipy-rs.app}"
PROFILE="${PROFILE:-release}"
BIN_NAME="clipy-rs"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS app bundles can only be built on macOS" >&2
  exit 1
fi

cd "$PROJECT_DIR"
cargo build --profile "$PROFILE"

SOURCE_BIN="$PROJECT_DIR/target/$PROFILE/$BIN_NAME"
if [[ ! -x "$SOURCE_BIN" ]]; then
  echo "error: built binary not found at $SOURCE_BIN" >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$PROJECT_DIR/Cargo.toml" | head -n 1)"
VERSION="${VERSION:-0.1.0}"

APP_ROOT="$PROJECT_DIR/target/macos-app"
APP_DIR="$APP_ROOT/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$SOURCE_BIN" "$MACOS_DIR/$BIN_NAME"
chmod +x "$MACOS_DIR/$BIN_NAME"

cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleExecutable</key>
  <string>$BIN_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSAccessibilityUsageDescription</key>
  <string>Clipy RS needs accessibility access to simulate keyboard shortcuts (Cmd+V) for automatic pasting of clipboard content into the active application.</string>
</dict>
</plist>
PLIST

ICON_PATH="$PROJECT_DIR/icons/AppIcon.icns"
if [[ -f "$ICON_PATH" ]]; then
  cp "$ICON_PATH" "$RESOURCES_DIR/AppIcon.icns"
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$CONTENTS_DIR/Info.plist"
  plutil -lint "$CONTENTS_DIR/Info.plist" >/dev/null
  echo "Installed icon: $RESOURCES_DIR/AppIcon.icns"
else
  echo "warning: icon file not found at $ICON_PATH, skipping icon installation"
  echo "hint: generate it first, for example: iconutil -c icns icons/AppIcon.iconset -o icons/AppIcon.icns"
fi

echo "Created $APP_DIR"
echo "Run: open \"$APP_DIR\""
echo "Install: cp -R \"$APP_DIR\" /Applications/"
