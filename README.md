# Pulse

Local menu-bar / tray health monitor for HTTP endpoints you own. No account, no cloud.

Pulse is a Tauri 2 + React 19 desktop app (macOS and Windows). This repo is early: `pnpm tauri dev` currently opens an empty popover only.

Installer title: **Pulse — Service Monitor**. Bundle ID: `dev.pulsebar.app`.

## Prerequisites

- [Node.js](https://nodejs.org/) 22+
- [pnpm](https://pnpm.io/) 11 (`corepack enable && corepack prepare pnpm@11.22.0 --activate`)
- [Rust](https://rustup.rs/) 1.77+ (edition 2021), with Xcode Command Line Tools on macOS

## Develop

```sh
pnpm install
pnpm tauri dev
```

That starts Vite on port 1420 and opens the 372×480 popover. There is no poller, tray painter, or settings window yet.

## Test

```sh
pnpm test
cd src-tauri && cargo test
```

`pnpm test` is a placeholder until the UI has a test runner.

## Platforms

macOS and Windows only. Linux is out of scope.

## Install

Distribution is **direct download only** from [GitHub Releases](https://github.com/pulsebar/pulse/releases): notarized macOS `.dmg` and Windows NSIS `.exe`. There is no Mac App Store or Microsoft Store package.

Push a `v*` tag to run [`.github/workflows/release.yml`](.github/workflows/release.yml). That workflow builds macOS (Apple Silicon + Intel) and Windows, uploads the installers, and writes `latest.json` for the updater endpoint. The default binary does **not** enable the updater.

### Unsigned macOS (right-click → Open)

Until a Developer ID certificate and notarization secrets are configured, Gatekeeper will block a double-click:

1. In Finder, right-click (or Control-click) `Pulse.app`.
2. Choose **Open**.
3. Confirm **Open** in the dialog.

Do not use `xattr -cr` as the everyday path; right-click → Open is the documented Gatekeeper bypass for an unsigned first install.

### Unsigned Windows / SmartScreen

Without Authenticode, SmartScreen will warn that the NSIS installer is unrecognized. Choose **More info** → **Run anyway**. Win10 machines without WebView2 will download it via the embedded bootstrapper on first launch (needs network once).

### Keychain: Always Allow

The first check that reads a just-saved secret on unsigned macOS shows the system dialog *Pulse wants to access the keychain*. Choose **Always Allow**. Pulse cannot click that for you.

### Signing identity change (unsigned → signed)

macOS Keychain ACLs are bound to the code-signing identity. An unsigned `dev.pulsebar.app` you clicked **Always Allow** on is **not** readable by a later Developer ID–signed binary. After that upgrade, Pulse will prompt **Re-enter secret headers** for the affected services. It never falls back to storing secrets in plaintext, and it does not delete the old unreadable items (so you can revert the binary).

### Config paths (`app_config_dir()`)

Identifier `dev.pulsebar.app`. Files sit **directly** in that directory (no extra `config` leaf):

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/dev.pulsebar.app/` |
| Windows | `%APPDATA%\dev.pulsebar.app\` |

Typical files: `config.json`, `services.json`, `history.sqlite3`, `logs/pulse.log`.

### Updater

`tauri-plugin-updater` is wired behind the Cargo feature `updater`, which is **off by default**. Releases never force-install. Enable later with `cargo tauri build --features updater` once `TAURI_SIGNING_PRIVATE_KEY` (and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) are set as GitHub Actions secrets. The committed `plugins.updater.pubkey` is a placeholder and must be replaced with the public half of that key.

The endpoint is GitHub Releases `latest.json`:

`https://github.com/pulsebar/pulse/releases/latest/download/latest.json`

Settings → “Check for updates” is not built yet (no Settings window).

### Icons

App icons are a simple filled circle (not emoji). Tray status marks (green / amber / red / hollow / slash) are a later change and are not these bundle icons.
