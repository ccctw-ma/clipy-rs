# clipy-rs

[English README](README.md)

`clipy-rs` 是一个用 Rust 编写的 macOS 剪贴板历史工具，灵感来自
Clipy 的核心工作流：记录文本、图片和文件引用剪贴板历史、搜索历史、复制旧内容，以及维护可复用的文本片段。

它同时提供原生 macOS 菜单栏应用和命令行界面。剪贴板读写基于 AppKit
的 `NSPasteboard`，粘贴集成通过 CoreGraphics 和 macOS 辅助功能发送键盘事件。

## 功能

- 记录文本剪贴板历史。
- 在菜单栏应用中记录图片和文件剪贴板格式。
- 列表查看、搜索、置顶、删除和清空历史项。
- 将历史项重新复制回系统剪贴板。
- 复制后可选自动向当前前台应用发送 Cmd+V。
- 管理可复用文本片段，并在主菜单提供增删改查入口。
- 提供原生 macOS 菜单栏 GUI。
- 支持全局快捷键 `Cmd+Shift+V` 呼出菜单。
- 鼠标悬停图片历史项时，在浮窗中预览原图。
- 自动粘贴失败时弹出引导对话框，一键跳转到「辅助功能」设置。
- 提供设置子菜单，用于配置语言和剪贴板格式选项。
- 支持菜单栏 UI 在英文和中文之间切换。
- 默认在 capture 和 watch 时跳过明显的敏感内容。
- 数据本地存储在 `~/Library/Application Support/clipy-rs`。

## 快速开始

本项目只通过源码分发，没有任何签名 / 公证过的现成发行版。推荐用户克隆仓库后用一条命令本地构建并安装：

```sh
git clone https://github.com/ccctw-ma/clipy-rs.git
cd clipy-rs
scripts/install-macos-app.sh
```

该脚本会构建 release 二进制、打包 `Clipy RS.app`、复制进 `/Applications` 并重新启动应用。本地构建出来的应用**不会带 `com.apple.quarantine` 隔离属性**，因此不会被 Gatekeeper 拦截。

前置依赖：macOS、Rust stable 工具链（`rustup`）、Xcode Command Line Tools（`xcode-select --install`）。

如果确实想离线拷贝一份 DMG，可以本地构建：

```sh
scripts/package-macos-dmg.sh
open "target/macos-dmg/Clipy RS.dmg"
```

该 DMG **未签名也未公证**。把 `Clipy RS.app` 拖到 `Applications` 后，macOS 会提示「无法打开 Clipy RS，因为 Apple 无法检查其是否包含恶意软件」。一次性剥离隔离属性即可：

```sh
xattr -dr com.apple.quarantine "/Applications/Clipy RS.app"
```

或者右键 `Clipy RS.app` → 打开 → 再次确认打开。从源码构建可以完全避免这两步。

## 安装

构建 CLI 二进制：

```sh
cargo build --release
```

从当前仓库安装 CLI：

```sh
cargo install --path .
```

只构建 `.app` 应用包：

```sh
scripts/package-macos-app.sh
open "target/macos-app/Clipy RS.app"
```

一键安装到 `/Applications`（重新构建、关闭旧实例、复制并重新启动）：

```sh
scripts/install-macos-app.sh
```

自定义应用名或 bundle identifier：

```sh
APP_NAME="Clipy" BUNDLE_ID="com.example.clipy" scripts/package-macos-app.sh
```

`.app` 会启动原生菜单栏 GUI，等价于运行：

```sh
clipy-rs gui
```

## 使用

启动菜单栏 GUI：

```sh
clipy-rs gui
```

状态栏标题是 `Clip`。菜单包含最近文本历史、图片/文件历史、文本片段、设置、手动捕获、刷新、清空历史、打开数据目录和退出。

设置子菜单支持：

- 在 English 和 Chinese 之间切换语言
- 开启或关闭图片/文件剪贴板捕获

全局快捷键：

```text
Cmd+Shift+V
```

如果其他应用已经占用了这个快捷键，状态栏菜单仍可使用，注册失败信息会显示在菜单里。

启动前台剪贴板监听：

```sh
clipy-rs watch
```

捕获当前剪贴板一次：

```sh
clipy-rs capture
```

查看最近历史：

```sh
clipy-rs list
clipy-rs list query --limit 20
```

复制最新一条历史回剪贴板：

```sh
clipy-rs copy 1
```

复制并粘贴到当前前台应用：

```sh
clipy-rs copy 1 --paste
```

管理文本片段：

```sh
clipy-rs snip add email "hello@example.com"
clipy-rs snip add work/signature "Regards,"
clipy-rs snip save copied-note
clipy-rs snip list
clipy-rs snip pick sig --paste
clipy-rs snip copy email --paste
clipy-rs snip remove email
```

片段名称可以用 `/` 形成菜单分组，例如 `work/signature`。片段内容支持
`{{clipboard}}` 插入粘贴前的当前剪贴板文本，也支持 `{{cursor}}` 或 `$|$`
控制插入后的光标位置。

查看本地数据目录：

```sh
clipy-rs path
```

`--paste` 和 GUI 菜单选择会发送 Cmd+V 键盘事件，因此 macOS 可能要求你给 Terminal、iTerm 或启动该工具的应用授予辅助功能权限。

## 数据和隐私

剪贴板管理器可能捕获密码、token 和其他隐私内容。`clipy-rs capture` 和 `clipy-rs watch` 默认会跳过明显的敏感内容。

如果你确实希望保存所有内容，可以传入：

```sh
clipy-rs capture --allow-sensitive
clipy-rs watch --allow-sensitive
```

历史记录存储在本地二进制文件：

```text
~/Library/Application Support/clipy-rs/history.bin
```

文本片段存储在同一目录：

```text
~/Library/Application Support/clipy-rs/snippets.bin
```

图片/文件剪贴板历史和应用设置也存储在同一目录：

```text
~/Library/Application Support/clipy-rs/rich_history.bin
~/Library/Application Support/clipy-rs/settings.conf
```

通过环境变量覆盖数据目录：

```sh
RCLIPY_HOME="/path/to/data" clipy-rs path
```

## GitHub CI

仓库的 GitHub Actions workflow 只在每次 push 到 `main` 时跑格式化、Clippy 与单元测试，**不会**发布 DMG。原因是未签名 DMG 通过 GitHub Release 分发会强制每个下载者要么右键 → 打开，要么手动 `xattr -dr com.apple.quarantine ...`。直接从源码构建（[`scripts/install-macos-app.sh`](scripts/install-macos-app.sh)）可以完全避免这一步，所以推荐用户自行 clone 后构建。

## 开发

前置要求：

- macOS
- Rust stable toolchain
- Xcode Command Line Tools

运行测试：

```sh
cargo test
```

构建 release 二进制：

```sh
cargo build --release
```

从源码运行 GUI：

```sh
cargo run -- gui
```

查看 CLI 帮助：

```sh
cargo run -- help
```

安装 Git pre-commit 检查：

```sh
scripts/install-git-hooks.sh
```

安装后，每次 `git commit` 都会执行：

- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`

如果测试临时太慢，可以只在本次提交跳过测试步骤：

```sh
SKIP_TESTS=1 git commit
```

项目结构：

- `src/main.rs`：CLI 入口和命令处理。
- `src/gui.rs`：原生 macOS 菜单栏 GUI。
- `src/clipboard.rs`：剪贴板读写和粘贴集成。
- `src/storage.rs`：历史记录和文本片段的二进制存储。
- `src/sensitive.rs`：简单敏感内容检测。
- `.githooks/pre-commit`：提交前格式、lint 和测试检查。
- `scripts/install-git-hooks.sh`：启用仓库 Git hooks。
- `scripts/install-macos-app.sh`：一键构建并安装到 `/Applications`。
- `scripts/package-macos-app.sh`：构建 `Clipy RS.app`（图标和 Info.plist 内联）。
- `scripts/package-macos-dmg.sh`：构建拖拽安装 DMG。
- `scripts/package-macos-release.sh`：构建签名并公证的 DMG。
- `.github/workflows/ci.yml`：每次 push 跑 fmt/clippy/test 的 CI 流水线。

## 打包脚本

构建未签名的个人使用 `.app`：

```sh
scripts/package-macos-app.sh
```

构建未签名的个人使用 `.dmg`：

```sh
scripts/package-macos-dmg.sh
```

构建签名并公证的分发 DMG：

```sh
SIGN_IDENTITY="Developer ID Application: YOUR NAME (TEAMID)" \
NOTARY_PROFILE="clipy-rs-notary" \
scripts/package-macos-release.sh
```

个人使用不需要签名和公证流程。

## 高级分发

如果以后想把 DMG 分发给其他用户，可以使用 Apple 的 `codesign + notarize + stapler` 流程，让 macOS 能验证应用，减少 Gatekeeper 警告。

先创建一次 `notarytool` keychain profile：

```sh
xcrun notarytool store-credentials "clipy-rs-notary" \
  --apple-id "you@example.com" \
  --team-id "TEAMID" \
  --password "app-specific-password"
```

如果要启用 CI 签名构建，需要在 GitHub 仓库添加这些 secrets：

```text
MACOS_CERTIFICATE_P12_BASE64
MACOS_CERTIFICATE_PASSWORD
MACOS_SIGN_IDENTITY
APPLE_ID
APPLE_TEAM_ID
APPLE_APP_PASSWORD
```

从 Keychain Access 导出 `Developer ID Application` 证书为 `.p12` 文件，然后编码成 `MACOS_CERTIFICATE_P12_BASE64`：

```sh
base64 -i DeveloperIDApplication.p12 | pbcopy
```

`MACOS_CERTIFICATE_PASSWORD` 是导出 `.p12` 时设置的密码。`MACOS_SIGN_IDENTITY` 使用以下命令查看：

```sh
security find-identity -v -p codesigning
```

`APPLE_APP_PASSWORD` 使用 Apple ID 的 app-specific password。你需要 Apple Developer Program 会员、`Developer ID Application` 证书和用于公证的 app-specific password。

## 后台运行

如果只需要 CLI 后台监听，先执行 `cargo install --path .`，然后创建运行以下命令的 LaunchAgent plist：

```sh
$HOME/.cargo/bin/clipy-rs watch
```

如果需要菜单栏 GUI，则运行：

```sh
$HOME/.cargo/bin/clipy-rs gui
```

最小 plist 命令片段如下：

```xml
<key>ProgramArguments</key>
<array>
  <string>/Users/YOUR_USER/.cargo/bin/clipy-rs</string>
  <string>watch</string>
</array>
<key>RunAtLoad</key>
<true/>
<key>KeepAlive</key>
<true/>
```

## 与 Clipy 的范围对比

当前 Rust 版本已实现：

- 剪贴板历史
- 文本片段
- 历史搜索
- 一键复制/粘贴
- 置顶、删除、清空操作
- 菜单栏 GUI
- 全局快捷键弹出菜单
- 设置子菜单
- 英文/中文语言切换
- GUI 中的图片/文件剪贴板格式

尚未实现：

- 应用排除规则
- iCloud 同步

## 开源许可

本项目基于 [MIT 许可证](LICENSE) 开源。
