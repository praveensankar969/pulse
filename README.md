# Pulse

Local menu-bar / tray health monitor for HTTP endpoints you own. No account, no cloud.

Installer title **Pulse — Service Monitor**. Bundle id `dev.pulsebar.app`. macOS and Windows only.

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

First launch opens the empty popover once so you can find the tray icon, then Pulse stays in the tray.

### Kill switch

A bad poller build: launch with `--paused` so every service starts paused (persisted to `services.json`).

Tauri 2 `dev` treats args after one `--` as cargo/runner args. App flags need a **second** `--` so they reach the Pulse process (`["pulse", "--paused"]`):

```sh
pnpm tauri dev -- -- --paused
# or
pnpm dev:paused
```

Unpause rows from the popover (`P`) or detail window when the build is good.

### Harbor demo

Optional 7-service Harbor fixture set (API, Web, Worker, Auth, Payments API, Docs, NAS) for screenshots / dogfood:

```sh
pnpm tauri dev -- -- --demo
# or
pnpm dev:demo
```

Combine flags: `pnpm tauri dev -- -- --demo --paused`. Re-running `--demo` does not overwrite services you already edited.

## Test

```sh
pnpm test
cd src-tauri && cargo test
```

## Data on disk

Tauri `app_config_dir()` (no extra `config` leaf):

| | |
|---|---|
| macOS | `~/Library/Application Support/dev.pulsebar.app/` |
| Windows | `%APPDATA%\dev.pulsebar.app\` |

Files: `config.json`, `services.json`, `history.sqlite3`, `logs/pulse.log`, `first-run.json`. Secret header values live in the OS keychain / Credential Manager, never in JSON.

## Unsigned builds

Until Developer ID / Authenticode exist, GitHub Release binaries may be unsigned.

- **macOS:** right-click the app → **Open**. The first check that reads a just-saved secret shows “Pulse wants to use the keychain” — choose **Always Allow**.
- **Windows:** SmartScreen may warn. Pulse payload stays under 20 MB; WebView2 is installed via the NSIS `embedBootstrapper` (~1.8 MB) if the machine does not already have it.
- **Unsigned → later Developer ID:** macOS Keychain ACLs are bound to the signing identity. Re-enter secret headers. Pulse does not fall back to plaintext.

## Notifications

OS toasts fire once on down and once on recovery. Sound is best-effort (`settings.sound`). Permission is requested on the first successful save of a service with `notify: true`, not at launch.

Click is **best-effort** show popover (never detail). Windows body-click cannot be QA’d in `tauri dev` (PowerShell name/icon). AUMID / `pulse:focus?id=` is only claimed on an **installed NSIS** build.

## Manual QA checklist

Match these before tagging. Plus the extras at the bottom.

- [ ] First launch shows the empty popover **once**, then later launches stay in the tray.
- [ ] Empty copy includes the Keychain hint: “macOS will ask Pulse to use the keychain — choose Always Allow.”
- [ ] Tray left-click toggles the popover; click again **dismisses without flicker**.
- [ ] `Esc` and click-outside hide the popover. Right-click menu does not toggle-fight the popover.
- [ ] Windows overflow flyout / missing tray rect: popover appears at that monitor’s work-area bottom-right minus 12 px and does **not** hide if already shown.
- [ ] Multi-monitor: popover stays on the monitor that owns the tray icon (or the cursor, for overflow).
- [ ] New service is **Pending**, not green. Tray is hollow until the first non-pending result.
- [ ] Snooze keeps the tray **red** and the primary label **Down** (Snoozed is an extra pill).
- [ ] Notification click is best-effort show popover. Windows click is only claimed on an installed NSIS build — do not fail QA on `tauri dev` toasts.
- [ ] `pnpm tauri dev -- -- --paused` starts every service paused. `pnpm tauri dev -- -- --demo` seeds the 7 Harbor rows.
- [ ] Check now / Check all, pause (`P`), add (`Cmd/Ctrl+N`), Enter opens the action URL, Shift+Enter opens detail.
- [ ] Import / export / reset; export with secrets uses the `.SECRETS.json` name.
- [ ] Quiet hours queue downs; Always alert bypasses the window. Launch-at-login prompt fires once after first save.

## Platforms

macOS, Apple silicon. Linux is out of scope.

## Site

The public site is `docs/` (GitHub Pages). Enable **Settings → Pages → Source: GitHub Actions**. Pushes that touch `docs/` deploy it.

If the GitHub repo is not `pulsebar/pulse`, edit `meta[name=pulse-github]` in `docs/index.html`.

## Install

Direct download from GitHub Releases. Unsigned Mac build (no Developer ID).

### Publish a version

1. `pnpm tauri build --bundles app`
2. Zip `src-tauri/target/release/bundle/macos/Pulse.app` as **`Pulse.app.zip`**
3. `git tag v0.1.0 && git push origin v0.1.0` — Actions opens the GitHub Release
4. Attach `Pulse.app.zip` to that release

The site Download button uses the latest zip on the latest release.

### Unsigned Mac (right-click → Open)

Gatekeeper blocks a double-click:

1. Unzip, then right-click (Control-click) `Pulse.app`
2. Choose **Open**
3. Confirm **Open**

Do not use `xattr -cr` as the everyday path.

### Keychain: Always Allow

The first check that reads a just-saved secret on unsigned macOS shows the system dialog *Pulse wants to access the keychain*. Choose **Always Allow**. Pulse cannot click that for you.

### Signing identity change (unsigned → signed)

macOS Keychain ACLs are bound to the code-signing identity. An unsigned `dev.pulsebar.app` you clicked **Always Allow** on is **not** readable by a later Developer ID–signed binary. After that upgrade, Pulse will prompt **Re-enter secret headers** for the affected services. It never falls back to storing secrets in plaintext, and it does not delete the old unreadable items (so you can revert the binary).

### Config paths (`app_config_dir()`)

Identifier `dev.pulsebar.app`. Files sit **directly** in that directory (no extra `config` leaf):

macOS: `~/Library/Application Support/dev.pulsebar.app/`

Typical files: `config.json`, `services.json`, `history.sqlite3`, `logs/pulse.log`.

### Updater

`tauri-plugin-updater` is wired behind the Cargo feature `updater`, which is **off by default**. Releases never force-install. Enable later with `cargo tauri build --features updater` once `TAURI_SIGNING_PRIVATE_KEY` (and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) are set as GitHub Actions secrets. The committed `plugins.updater.pubkey` is a placeholder and must be replaced with the public half of that key.

The endpoint is GitHub Releases `latest.json`:

`https://github.com/pulsebar/pulse/releases/latest/download/latest.json`

Settings → “Check for updates” is not built yet (no Settings window).

### Icons

App icon is the dual rounded square on a light plate (same mark as the landing favicon). Tray status marks use that glyph in green / amber / red / hollow / slash so health is readable from the menu extra without opening the popover.
## Notifications

OS toasts fire once on down and once on recovery. Sound is best-effort (`settings.sound`). Permission is requested on the first successful save of a service with `notify: true`, not at launch. Toasts are Rust-only (`OsNotifier`); webviews do not get `notification:default`.

Click is best-effort show popover (never detail). The plugin’s desktop `show()` has no click payload (“actions” are mobile-only), so we deliver through notify-rust and `wait_for_response`. Windows body-click is `NotificationResponse::Default` (not `"__closed"`). `RunEvent::Reopen` is only a Dock-relaunch fallback — this is an accessory / `LSUIElement` app with no Dock icon, so a banner click does not go through Reopen.

**Windows click cannot be QA’d in `tauri dev`.** Dev toasts show a PowerShell name/icon; click is not a product-quality test. AUMID / `pulse:focus?id=` is only claimed on an installed NSIS build.
