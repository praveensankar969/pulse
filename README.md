# Pulse

[![CI](https://github.com/praveensankar969/pulse/actions/workflows/ci.yml/badge.svg)](https://github.com/praveensankar969/pulse/actions/workflows/ci.yml)

Menu bar app for Mac that checks HTTP endpoints you own. Green is healthy, red is down. No account, no cloud — nothing is sent anywhere except the URLs you add.

**macOS 13+ · Apple silicon** · [Website](https://praveensankar969.github.io/pulse/)

## Install

1. Download the latest DMG from [Releases](https://github.com/praveensankar969/pulse/releases/latest).
2. Drag **Pulse.app** to **Applications**.
3. The build is unsigned (no Apple Developer ID). In Terminal:

   ```sh
   xattr -cr /Applications/Pulse.app
   ```

4. Open Pulse. It lives in the menu bar.

The first time Pulse reads a saved secret, macOS Keychain will ask. Choose **Always Allow**.

## Features

- Checks URLs on an interval you set (localhost, LAN, or public)
- Optional headers, with secret values stored in Keychain
- Optional body / JSON assertions
- Menu bar icon plus a list; click a row for last status, latency, and 24 hours of checks
- Notifications on down and recovery
- Pause, snooze, quiet hours, launch at login
- Import / export of your local config

Pulse marks a service down after three consecutive failed checks, so a single blip does not page you.

## Development

You need [Node.js](https://nodejs.org/) 22+, [pnpm](https://pnpm.io/) 11, [Rust](https://rustup.rs/), and Xcode Command Line Tools.

```sh
corepack enable && corepack prepare pnpm@11.22.0 --activate
pnpm install
pnpm tauri dev
```

The first launch shows the empty popover so you can find the icon. After that, Pulse stays in the menu bar.

```sh
pnpm dev:demo     # seed a few sample services
pnpm dev:paused   # start every service paused
pnpm test
cd src-tauri && cargo test
```

## Build

```sh
pnpm tauri build --bundles dmg
# src-tauri/target/release/bundle/dmg/Pulse_0.1.0_aarch64.dmg
```

To publish: push `master`, tag `v0.1.0`, push the tag (Actions opens the GitHub Release), then attach that DMG and keep the release marked latest. The website Download button uses the `.dmg` on the latest release.

Pushes that change `docs/` deploy the site via GitHub Pages.

## Data

Config lives only on your Mac:

`~/Library/Application Support/dev.pulsebar.app/`

Typical files: `config.json`, `services.json`, `history.sqlite3`, `logs/pulse.log`. Secret header values are in Keychain, never in those files.

## Contributing

Issues and pull requests are welcome.

## License

[MIT](LICENSE)
