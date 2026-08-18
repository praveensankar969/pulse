# Pulse

Local menu-bar / tray health monitor for HTTP endpoints you own. No account, no cloud.

Pulse is a Tauri 2 + React 19 desktop app (macOS and Windows). This repo is early: `pnpm tauri dev` currently opens an empty popover only.

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
