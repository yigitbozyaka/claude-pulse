<p align="center">
  <img src="src-tauri/icons/128x128.png" width="112" alt="Claude Pulse">
</p>

<h1 align="center">Claude Pulse</h1>

<p align="center">
  A native Windows system-tray app that shows your Claude usage at a glance,
  across <b>multiple accounts</b>.
  <br>
  Session &amp; weekly limits, model distribution, daily activity, token stats.
</p>

<p align="center">
  Built in <a href="https://tauri.app">Tauri</a> (Rust backend + web frontend).
</p>

<p align="center">
  <img src="docs/tray.png" width="640" alt="Tray battery icon across session states"><br>
  <em>The tray icon — remaining session % as a filling battery, in JetBrains Mono.</em>
</p>

<p align="center">
  <img src="docs/app.png" width="360" alt="Claude Pulse dashboard">
</p>

## Features

- **Multi-account** — auto-detects every Claude config dir (`~/.claude`,
  `~/.claude-work`, …) and switches between them via dashboard tabs or the tray
  menu.
- **Live tray battery icon** — remaining session % as a Claude-orange battery
  that fills from the bottom, with the number set in JetBrains Mono ExtraBold and
  rendered supersampled at the exact size Windows requests.
- **Session flyout** — big "% of session left", a progress bar, and *resets in*,
  *window started* and *burn rate* at a glance.
- **Dashboard** — weekly limit meters (all models / Sonnet / Opus), model
  distribution, a 7-day activity chart, and weekly token totals.
- **Accurate plan detection** — reads the real subscription (`Pro` / `Max 5x` /
  `Max 20x` / `Team`) from Claude Code's credentials, per account.
- **Self-updating** — checks GitHub releases and updates the portable exe
  in place, no installer needed.
- **Per-account caching** with atomic writes and rate-limit backoff. Frameless,
  dark UI; closing the window hides it back to the tray.

## Requirements

- Windows 10/11 (WebView2 ships with Windows 11).
- One or more Claude accounts signed in via Claude Code (a `.credentials.json`
  under `~/.claude*`).

## Run

Download `ClaudeUsage.exe` from the
[latest release](https://github.com/yigitbozyaka/claude-pulse/releases/latest)
and run it. It lives in the system tray — click the icon or **Open Dashboard**
to open the window.

## Build from source

Requires the [Rust toolchain](https://rustup.rs) and the Tauri CLI
(`cargo install tauri-cli`).

```bash
cd src-tauri
cargo tauri dev               # run in development
cargo tauri build --no-bundle # just the exe -> src-tauri/target/release/app.exe
cargo tauri build             # installer + exe
```

## How it works

The Rust backend discovers accounts, reads each account's OAuth credentials,
fetches usage from the Anthropic OAuth API, and parses local JSONL session logs
for the model/token breakdown. The frontend (vanilla HTML/CSS/JS, no bundler)
renders the dashboard and talks to the backend over Tauri commands; a background
thread refreshes the active account every 60s and updates the tray icon.

## Design

"Claude Pulse" palette, inspired by Claude's brand:

| Element    | Colour    |
|------------|-----------|
| Background | `#12100e` |
| Surface    | `#1a1714` |
| Accent     | `#d97757` |
| Text       | `#ede7de` |

## License

MIT — see [LICENSE](LICENSE).
