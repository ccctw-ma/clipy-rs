# clipy-rs

A small macOS clipboard history tool written in Rust, inspired by Clipy's core
workflow: keep clipboard history, search it, copy an old item back, and maintain
reusable snippets.

It includes both a CLI/daemon-style mode and a native macOS menu bar mode. The
clipboard read/write path uses macOS `pbpaste` and `pbcopy`; the GUI uses AppKit
and Carbon through Rust bindings.

## Features

- Poll and record clipboard text history.
- List and search history items.
- Copy a history item back to the system clipboard.
- Optionally send Cmd+V to the frontmost app after copying.
- Pin, remove, and clear history items.
- Add/list/copy/remove text snippets.
- Native menu bar GUI.
- Global hotkey menu popup: `Cmd+Shift+V`.
- Skip obvious secrets by default during capture/watch.
- Store data locally under `~/Library/Application Support/clipy-rs`.

## Build

```sh
cargo build --release
```

Install from this checkout:

```sh
cargo install --path .
```

Build a macOS `.app` bundle:

```sh
scripts/package-macos-app.sh
open "target/macos-app/Clipy RS.app"
```

Install it into Applications:

```sh
cp -R "target/macos-app/Clipy RS.app" /Applications/
```

The app bundle launches the native menu bar GUI, so it is equivalent to running
`clipy-rs gui`. To customize the generated bundle name or identifier:

```sh
APP_NAME="Clipy" BUNDLE_ID="com.example.clipy" scripts/package-macos-app.sh
```

Build a drag-to-install macOS `.dmg`:

```sh
scripts/package-macos-dmg.sh
open "target/macos-dmg/Clipy RS.dmg"
```

The DMG contains `Clipy RS.app` plus an `Applications` shortcut. Open the DMG,
then drag `Clipy RS.app` onto `Applications`.

Build a signed and notarized distribution DMG:

```sh
SIGN_IDENTITY="Developer ID Application: YOUR NAME (TEAMID)" \
NOTARY_PROFILE="clipy-rs-notary" \
scripts/package-macos-release.sh
```

Create the `notarytool` keychain profile once:

```sh
xcrun notarytool store-credentials "clipy-rs-notary" \
  --apple-id "you@example.com" \
  --team-id "TEAMID" \
  --password "app-specific-password"
```

Alternatively, pass notarization credentials directly:

```sh
SIGN_IDENTITY="Developer ID Application: YOUR NAME (TEAMID)" \
APPLE_ID="you@example.com" \
APPLE_TEAM_ID="TEAMID" \
APPLE_APP_PASSWORD="app-specific-password" \
scripts/package-macos-release.sh
```

The release DMG is written to `target/macos-dmg/Clipy RS.dmg`. You need an Apple
Developer Program membership, a `Developer ID Application` certificate, and an
app-specific password for notarization.

## Usage

Start the menu bar GUI:

```sh
clipy-rs gui
```

The status bar item is titled `Clip`. Its menu includes recent clipboard
history, snippets, manual capture, refresh, clear history, data-directory open,
and quit actions. Selecting a history or snippet item copies it and then sends
Cmd+V to the frontmost app.

The global hotkey is:

```text
Cmd+Shift+V
```

If another app already owns this shortcut, the menu still works from the status
bar and the registration error appears inside the menu.

Start a foreground watcher:

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

`--paste` and GUI menu selection use AppleScript/System Events, so macOS may ask
you to grant Accessibility permission to Terminal, iTerm, or the app that
launches this tool.

Manage snippets:

```sh
clipy-rs snip add email "hello@example.com"
clipy-rs snip list
clipy-rs snip copy email
clipy-rs snip remove email
```

Show the local data directory:

```sh
clipy-rs path
```

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

## Privacy

Clipboard managers can capture passwords, tokens, and other private content.
`clipy-rs capture` and `clipy-rs watch` skip obvious secrets by default. If you
really want to store everything, pass `--allow-sensitive`.

History is stored in a local binary file:

```text
~/Library/Application Support/clipy-rs/history.bin
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

Not implemented yet:

- app exclusion rules
- image/file clipboard formats
- iCloud sync
