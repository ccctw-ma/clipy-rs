# clipy-rs

[中文文档](README.zh-CN.md)

`clipy-rs` is a small macOS clipboard history tool written in Rust, inspired by
Clipy's core workflow: record clipboard text, images, and file references,
search history, copy previous items back, and keep reusable snippets.

It provides both a native macOS menu bar app and a command-line interface. The
clipboard read/write path uses AppKit's `NSPasteboard`; paste integration posts
keyboard events through CoreGraphics and macOS Accessibility.

## Features

- Records text clipboard history.
- Records image and file clipboard formats in the menu bar app.
- Lists, searches, pins, removes, and clears history items.
- Copies a previous history item back to the system clipboard.
- Optionally sends Cmd+V to the frontmost app after copying.
- Manages reusable text snippets.
- Provides a native macOS menu bar GUI.
- Shows the menu with the global hotkey `Cmd+Shift+V`.
- Provides a Settings submenu for language and clipboard-format options.
- Switches the menu bar UI between English and Chinese.
- Skips obvious secrets by default during capture and watch.
- Stores data locally under `~/Library/Application Support/clipy-rs`.

## Quick Start

Build a personal-use drag-to-install DMG:

```sh
scripts/package-macos-dmg.sh
open "target/macos-dmg/Clipy RS.dmg"
```

Open the DMG, then drag `Clipy RS.app` into `Applications`.

If macOS says the app cannot be verified, right-click `Clipy RS.app`, choose
Open, then confirm Open again. This is expected for an unsigned personal build.

## Installation

Build the CLI binary:

```sh
cargo build --release
```

Install the CLI from this checkout:

```sh
cargo install --path .
```

Build only the `.app` bundle:

```sh
scripts/package-macos-app.sh
open "target/macos-app/Clipy RS.app"
```

Customize the generated app name or bundle identifier:

```sh
APP_NAME="Clipy" BUNDLE_ID="com.example.clipy" scripts/package-macos-app.sh
```

The app bundle launches the native menu bar GUI. It is equivalent to running:

```sh
clipy-rs gui
```

## Usage

Start the menu bar GUI:

```sh
clipy-rs gui
```

The status bar item is titled `Clip`. Its menu includes recent text history,
image/file history, snippets, settings, manual capture, refresh, clear history,
data-directory open, and quit actions.

The Settings submenu supports:

- language switching between English and Chinese
- enabling or disabling image/file clipboard capture

The global hotkey is:

```text
Cmd+Shift+V
```

If another app already owns this shortcut, the menu still works from the status
bar and the registration error appears inside the menu.

Start a foreground clipboard watcher:

```sh
clipy-rs watch
```

Capture the current clipboard once:

```sh
clipy-rs capture
```

List recent history:

```sh
clipy-rs list
clipy-rs list query --limit 20
```

Copy the newest item back to the clipboard:

```sh
clipy-rs copy 1
```

Copy and paste into the frontmost app:

```sh
clipy-rs copy 1 --paste
```

Manage snippets:

```sh
clipy-rs snip add email "hello@example.com"
clipy-rs snip add work/signature "Regards,"
clipy-rs snip save copied-note
clipy-rs snip list
clipy-rs snip pick sig --paste
clipy-rs snip copy email --paste
clipy-rs snip remove email
```

Snippet names can use `/` to create menu folders, for example
`work/signature`. Snippet content supports `{{clipboard}}` to insert the current
clipboard text before pasting, and `{{cursor}}` or `$|$` to move the cursor back
after insertion.

Show the local data directory:

```sh
clipy-rs path
```

`--paste` and GUI menu selection post a Cmd+V keyboard event, so macOS may ask
you to grant Accessibility permission to Terminal, iTerm, or the app that
launches this tool.

## Data and Privacy

Clipboard managers can capture passwords, tokens, and other private content.
`clipy-rs capture` and `clipy-rs watch` skip obvious secrets by default.

If you really want to store everything, pass:

```sh
clipy-rs capture --allow-sensitive
clipy-rs watch --allow-sensitive
```

History is stored in a local binary file:

```text
~/Library/Application Support/clipy-rs/history.bin
```

Snippets are stored next to it:

```text
~/Library/Application Support/clipy-rs/snippets.bin
```

Image/file clipboard history and app settings are stored next to them:

```text
~/Library/Application Support/clipy-rs/rich_history.bin
~/Library/Application Support/clipy-rs/settings.conf
```

Override the data directory with:

```sh
RCLIPY_HOME="/path/to/data" clipy-rs path
```

## GitHub CI

The repository includes a GitHub Actions workflow that builds a macOS DMG on
every push to `main`:

```text
.github/workflows/macos-dmg.yml
```

For personal use, you do not need Apple signing secrets. After pushing to
`main`, GitHub Actions builds an unsigned DMG and uploads it in two places:

- the workflow artifact named `clipy-rs-macos-dmg-<commit-sha>`
- the rolling GitHub Release named `main-latest`

Download `Clipy-RS-main.dmg` from the `main-latest` release, open it, then drag
`Clipy RS.app` into `Applications`.

To trigger the CI build:

```sh
git add .
git commit -m "Update macOS DMG build"
git push origin main
```

## Development

Prerequisites:

- macOS
- Rust stable toolchain
- Xcode Command Line Tools

Run tests:

```sh
cargo test
```

Build release binary:

```sh
cargo build --release
```

Run the GUI from source:

```sh
cargo run -- gui
```

Run the CLI help:

```sh
cargo run -- help
```

Install Git pre-commit checks:

```sh
scripts/install-git-hooks.sh
```

After installation, every `git commit` runs:

- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`

If tests are temporarily too slow, skip only the test step for one commit:

```sh
SKIP_TESTS=1 git commit
```

Project layout:

- `src/main.rs`: CLI entrypoint and command handling.
- `src/gui.rs`: native macOS menu bar GUI.
- `src/clipboard.rs`: clipboard read/write and paste integration.
- `src/storage.rs`: binary history and snippet storage.
- `src/sensitive.rs`: simple sensitive-content detection.
- `.githooks/pre-commit`: local commit-time format, lint, and test checks.
- `scripts/install-git-hooks.sh`: enables the repository Git hooks.
- `scripts/package-macos-app.sh`: builds `Clipy RS.app`.
- `scripts/package-macos-dmg.sh`: builds a drag-to-install DMG.
- `scripts/package-macos-release.sh`: builds a signed and notarized DMG.
- `.github/workflows/macos-dmg.yml`: CI DMG build workflow.

## Packaging Scripts

Build an unsigned personal-use `.app`:

```sh
scripts/package-macos-app.sh
```

Build an unsigned personal-use `.dmg`:

```sh
scripts/package-macos-dmg.sh
```

Build a signed and notarized distribution DMG:

```sh
SIGN_IDENTITY="Developer ID Application: YOUR NAME (TEAMID)" \
NOTARY_PROFILE="clipy-rs-notary" \
scripts/package-macos-release.sh
```

The signed release flow is optional for personal use.

## Advanced Distribution

If you want to distribute the DMG to other people later, use Apple
`codesign + notarize + stapler` so macOS can verify the app without Gatekeeper
warnings.

Create the `notarytool` keychain profile once:

```sh
xcrun notarytool store-credentials "clipy-rs-notary" \
  --apple-id "you@example.com" \
  --team-id "TEAMID" \
  --password "app-specific-password"
```

To enable signed CI builds, add these repository secrets in GitHub:

```text
MACOS_CERTIFICATE_P12_BASE64
MACOS_CERTIFICATE_PASSWORD
MACOS_SIGN_IDENTITY
APPLE_ID
APPLE_TEAM_ID
APPLE_APP_PASSWORD
```

Export your `Developer ID Application` certificate from Keychain Access as a
`.p12` file, then encode it for `MACOS_CERTIFICATE_P12_BASE64`:

```sh
base64 -i DeveloperIDApplication.p12 | pbcopy
```

Set `MACOS_CERTIFICATE_PASSWORD` to the password used when exporting the `.p12`.
Set `MACOS_SIGN_IDENTITY` to the exact identity shown by:

```sh
security find-identity -v -p codesigning
```

Use an Apple ID app-specific password for `APPLE_APP_PASSWORD`. You need an
Apple Developer Program membership, a `Developer ID Application` certificate,
and an app-specific password for notarization.

## Running in the Background

For CLI-only background capture, after `cargo install --path .`, create a
LaunchAgent plist that runs:

```sh
$HOME/.cargo/bin/clipy-rs watch
```

For the menu bar GUI, run this instead:

```sh
$HOME/.cargo/bin/clipy-rs gui
```

A minimal plist command body looks like this:

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

## Scope Compared With Clipy

Implemented in this Rust version:

- clipboard history
- snippets
- history search
- one-command copy/paste
- pin/remove/clear operations
- menu bar GUI
- global hotkey popup
- settings submenu
- English/Chinese language switching
- image/file clipboard formats in the GUI

Not implemented yet:

- app exclusion rules
- iCloud sync
