# Developing Tjommie

How to build and run Tjommie from source, test it, and find your way around the repository. For installing the app, see the [README](../README.md).

## Requirements

You need Windows, Node 24 or newer, and Rust. The Rust toolchain version is pinned in `tjommie-rs/rust-toolchain.toml`; rustup installs it the first time you build.

The daemon is Rust and lives in `tjommie-rs`. The TypeScript daemon and the pre-Rust structure live on the `legacy` branch, for reference.

## Data folders and ports

Install the Claude Code and Codex CLIs and put them on PATH. The installed app keeps its data in `%LOCALAPPDATA%\Tjommie`, its Electron profile in `%APPDATA%\Tjommie`, and uses ports 47100 and 47101. A run from source (`npm run desktop`, `npm run dev` or `TJOMMIE_DESKTOP=production`) keeps its data in `%LOCALAPPDATA%\Tjommie-rs`, its Electron profile and single-instance lock in `%LOCALAPPDATA%\Tjommie-rs\electron`, and uses ports 47200 and 47201, so it runs beside an installed Tjommie without touching its data, ports or profile. Set `TJOMMIE_HOME`, `TJOMMIE_PORT` or `TJOMMIE_USER_DATA` to use another data folder, port or Electron profile in either case. The installed browser uses Electron's bundled Chromium; standalone development browser fixtures use Chrome or Edge.

## Run from source

To run Tjommie from source, build the Rust daemon, then run the Electron UI. Production from source starts `tjommie-rs\target\debug\tjommie-daemon.exe`, or the exe named by `TJOMMIE_DAEMON_EXE`, with `--token-stdin`. If the exe isn't built, the app says to build it. Sign the Claude and Codex accounts in from Settings → Accounts; a run from source keeps them in `Tjommie-rs`.

The daemon starts in one of two modes. `--dev` (or `$env:TJOMMIE_DEV = '1'`) is development mode. It saves a `daemon-token` file in its data folder so a development desktop app can connect, and it also allows the UI at `http://localhost:3000`. The desktop origin `app://tjommie` is always allowed. A debug build of the daemon defaults to the same `Tjommie-rs` folder and port 47200 as a run from source; a release build defaults to `Tjommie` and port 47100, like the installed app.

The installed desktop app starts the daemon with `--token-stdin`. It must start the daemon with `TJOMMIE_DEV` unset. It passes a fresh token as the first line of a pipe to the daemon's standard input and keeps that pipe open. The daemon requires at least 43 base64url characters within 5 seconds, never saves this token to a file, deletes an old `daemon-token` file left by development mode, and stops when the pipe closes, so quitting or crashing the desktop app never leaves a daemon running. Starting the daemon with neither mode exits with an error. Tests can set `TJOMMIE_ACCOUNTS_DIR` to use signed-in account folders with a disposable data root.

The desktop app has two modes from source. With `$env:TJOMMIE_DESKTOP = 'production'` it starts its own daemon with `--token-stdin` and serves the static export in `packages/ui/out` (run `npm run build` first), the way the installed app will. Otherwise it is in development mode: it connects to a daemon you start yourself with `--dev` and shows the UI from `http://localhost:3000`, so `npm run dev` and that daemon must both be running. No environment variables are needed while the app and the daemon use their defaults. If you set `TJOMMIE_HOME` or `TJOMMIE_PORT`, give the daemon the same values. Closing the window keeps Tjommie running in the tray; Quit Tjommie in the tray menu stops the daemon, and with it the agents and routines. `npm run test:real` also drives the desktop app in production mode, on a disposable data root and port. It reads the signed-in accounts from `%LOCALAPPDATA%\Tjommie\accounts` (or `TJOMMIE_HOME\accounts`), which is where the installed app keeps them.

```sh
npm install
cd tjommie-rs                                # run every cargo command inside tjommie-rs
cargo build -p daemon --bin tjommie-daemon   # the Rust daemon
cargo test --workspace                       # daemon tests
cd ..
npm run build      # static export to packages/ui/out
$env:TJOMMIE_DESKTOP = 'production'; npm run desktop   # the Electron UI, on the Rust daemon it starts

# or in development mode, each in its own terminal (the first from tjommie-rs):
cargo run -p daemon --bin tjommie-daemon -- --dev   # development daemon, on http://127.0.0.1:47200
npm run dev        # UI at http://localhost:3000
npm run desktop    # the Electron UI, on that UI and the development daemon (keep both running)

npm test           # TypeScript tests (vitest)
npm run test:real  # tests that drive the real Claude and Codex CLIs (needs the accounts signed in under Tjommie, as above)
npm run check:no-any # rejects explicit any types in first-party TypeScript
npm run typecheck
npm run lint
```

## Repository layout

| Package | What it is |
|---|---|
| `packages/protocol` | Messages shared by the daemon and the UI |
| `packages/client` | The typed WebSocket client for the daemon, shared by the UI and the desktop app |
| `tjommie-rs` | The Rust workspace: the daemon that runs the agents (`tjommie-daemon.exe`), plus its protocol and transport crates |
| `packages/ui` | The UI: Next.js static export, Base UI, Tailwind |
| `packages/desktop` | Electron shell |
| `packages/mobile` | Android Capacitor companion and native plugins |
| `packages/transport` | Shared pinned Snow WASM Noise implementation |

## Releases and packaged checks

GitHub Actions runs the release checks and builds after a version tag is pushed. It checks the installer against `latest.yml` before publishing. The build also stamps `dist-desktop/provenance.json`. `npm run test:packaged` remains a Windows smoke check for packaged startup, browser-tool files, shutdown, and source/artifact provenance; the hosted Windows release workflow runs it after packaging with disposable data and account folders. `npm run test:real` covers the source application and providers separately. Installation and the full installed-app demo remain separate acceptance checks.

After installing, set `TJOMMIE_TEST_EXECUTABLE` to the installed `Tjommie.exe` absolute path to run the existing desktop lifecycle and screen suites against that executable. They retain disposable data roots and Electron profiles and use the Tjommie-owned provider accounts. Unset it before returning to source-app tests. The mandatory packaged smoke always supplies its own unpacked executable path.

## Rules

TypeScript must never use explicit `any` types. `npm run typecheck` enforces this with the syntax-based `check:no-any` gate before checking types. Test commands fail when no test files are found.

## The isolated account check

The isolated desktop account check can be run with `npx vitest run --mode real packages/desktop/src/accounts.real.test.ts` after `npm run build` and after building the debug Rust daemon (or setting `TJOMMIE_DAEMON_EXE`); it stops at once, with the build instruction, if the daemon exe is missing. It uses unsigned disposable profiles, synthetic usage, and a fake key stored under a disposable credential target; it does not sign in or make a paid provider call. The broader `test:real` command includes real-provider tests and must not be used as a substitute for this isolated check.
