#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-Clipy RS}"
BIN_NAME="clipy-rs"
INSTALL_DIR="${INSTALL_DIR:-/Applications}"
INSTALLED_APP="$INSTALL_DIR/$APP_NAME.app"
SOURCE_APP="$PROJECT_DIR/target/macos-app/$APP_NAME.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this script can only run on macOS" >&2
  exit 1
fi

# 1. 构建 .app bundle（包含图标和权限声明）
"$PROJECT_DIR/scripts/package-macos-app.sh"

if [[ ! -d "$SOURCE_APP" ]]; then
  echo "error: built app bundle not found at $SOURCE_APP" >&2
  exit 1
fi

# 2. 关闭已运行的实例（避免覆盖时被锁）
if pgrep -x "$BIN_NAME" >/dev/null 2>&1; then
  echo "Stopping running $APP_NAME instances"
  pkill -x "$BIN_NAME" || true
  sleep 1
fi

# 3. 移除旧版本，避免 Launch Services 残留权限记录与 Spotlight 重复条目
if [[ -d "$INSTALLED_APP" ]]; then
  echo "Removing existing $INSTALLED_APP"
  rm -rf "$INSTALLED_APP"
fi

# 4. 安装到目标目录
echo "Installing to $INSTALLED_APP"
cp -R "$SOURCE_APP" "$INSTALLED_APP"

# 5. 清理 target 下的开发副本，避免 Spotlight 出现多个同名应用
rm -rf "$PROJECT_DIR/target/macos-app"
rm -rf "$PROJECT_DIR/target/macos-dmg/staging"

# 6. 启动新版本
echo "Launching $APP_NAME"
open "$INSTALLED_APP"

cat <<EOF

Installed: $INSTALLED_APP

如果是首次安装，macOS 会弹窗请求“辅助功能”权限，请允许；
如果是覆盖安装，请到“系统设置 → 隐私与安全性 → 辅助功能”里
重新勾选 $APP_NAME 后再使用自动粘贴功能。
EOF
