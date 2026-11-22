# Developing Tjommie

Tjommie 0.6.0 is a native Windows app. The desktop app and the background service that runs the agents are both Rust, in one workspace, `tjommie-rs`. The Android companion, the relay and a few shared packages are TypeScript, in `packages`.
The TypeScript daemon and the pre-Rust structure live on the `legacy` branch, for reference.

Rules for working in the repository are in [`AGENTS.md`](../AGENTS.md). Release steps are there too. Releases are built and published by GitHub Actions only: never package a release locally.

## What you need

- Windows 11, which is what the spec targets, with the Visual Studio Build Tools (MSVC) and a Windows SDK, as any Rust build on Windows does. CI builds on `windows-latest`.
- Rust. The version is pinned in `tjommie-rs/rust-toolchain.toml`, and rustup installs it the first time you run cargo inside `tjommie-rs`. **Run every cargo command from inside `tjommie-rs`**, so the pin applies.
- Node 24 or newer. The TypeScript packages and their tests need it, and so do some Rust tests: the interop tests drive the TypeScript client through `tjommie-rs/tools/remote-probe.ts`, and the worker and command-line tests run `node.exe`. Run `npm ci` in the repository root once.
- The Microsoft Edge WebView2 runtime. The agents' browser runs on it. It is part of Windows 11, and hosted Windows runners have it.
- To use real agents: the Claude Code and Codex command-line tools, installed and on PATH. Tests that use recorded conversations (below) need neither.

## Repository layout

`tjommie-rs` is a Cargo workspace. Its crates are under `tjommie-rs/crates`:

| Crate | What it is |
|---|---|
| `protocol` | The messages and data shapes shared by the daemon, the client and the desktop app. `tjommie-rs/protocol/manifest.json` and `browser-tools.json` pin the wire format and the 26 browser tools. |
| `transport` | The encrypted channel to paired devices (Noise, through the `snow` crate), pinned to the same version as `packages/transport/native` so both sides run the same code. |
| `daemon` | The background service, `tjommie-daemon.exe`. It runs the agents through Claude Code and Codex, keeps the SQLite database, accounts, usage, routines and rooms, and listens for the desktop app and for paired phones. It has a test-only feature, `provider-fixtures`. |
| `client` | The typed WebSocket client for the daemon, used by the desktop app and by tests. |
| `desktop-host` | What the desktop app needs from Windows, with no UI: starting and watching the daemon, the agents' browser on WebView2, the login vault on Credential Manager, speech capture and the speech model, the updater, the tray, notifications, the start-at-sign-in entry, the crash log, and the app's identity. |
| `desktop` | The app itself, `tjommie-desktop.exe`: the GPUI window, the screens under `src/ui`, their models under `src/model`, motion, the keymap, the theme and the embedded assets. Its `test-driver` feature adds a named pipe that the e2e tests drive the app through. |

Other folders in `tjommie-rs`:

- `tools`: `check-guarded.ps1` (the UI boundary check, below), `perf` (memory and motion measurements, run by hand on a quiet PC), and the probes the tests use.
- `design`: the Aegis theme tokens, fonts and icons the app is drawn with.
- `SPEC.md`, `test-map.md`, `desktop-test-map.md` and `phase3-checklist.md`: the pointer to the spec, and the maps and checklists the plan uses. The spec is `docs/superpowers/specs/2026-11-09-tjommie-rust-rewrite-spec.md`, and the plan for this release is `docs/superpowers/plans/2026-11-13-tjommie-rs-phase3-gpui-desktop.md`.

The TypeScript packages in `packages`:

| Package | What it is |
|---|---|
| `protocol`, `client`, `transport` | The TypeScript side of the same messages, client and encrypted channel, used by the Android app, the relay and the UI. |
| `mobile` | The Android companion: Capacitor, with native plugins. See [`packages/mobile/README.md`](../packages/mobile/README.md). |
| `relay` | The relay that connects phones to a PC away from home, and delivers push notifications. See [`relay-self-host.md`](relay-self-host.md). |
| `ui` | A Next.js static export that the Android app shows. The GPUI app does not use it. |
| `desktop` | The Electron shell of 0.5.x and earlier. It also holds the speech-runtime build (`scripts/build-speech-runtime.ps1`) that the installer's payload uses, and the `daemon-process` test that CI runs against the Rust daemon. The 0.6.0 app ships no Electron. |

## Build

Inside `tjommie-rs`:

```powershell
cargo build -p daemon --bin tjommie-daemon      # target\debug\tjommie-daemon.exe
cargo build -p desktop --bin tjommie-desktop    # target\debug\tjommie-desktop.exe
```

A cold debug build compiles GPUI and takes a while. The dev profile keeps debug information to line tables only, so `target` stays smaller. Several worktrees each with their own `target` add up fast, so check the free disk space before you start another.

`TJOMMIE_IDENTITY` is read when the crates are built. It picks which app the binary is. Unset, it is `dev`. Changing it rebuilds the desktop crates.

| Identity | Product name | Data folder | Ports (app, phones) | Update feed |
|---|---|---|---|---|
| `dev` (the default) | Tjommie RS | `%LOCALAPPDATA%\Tjommie-rs` | 47200, 47201 | none |
| `e2e` | Tjommie E2E | a temporary folder, from `TJOMMIE_HOME` | any free port | none |
| `updatetest` | Tjommie Update Test | a temporary folder, from `TJOMMIE_HOME` | any free port | a local one, from `TJOMMIE_UPDATE_FEED` |
| `production` | Tjommie | `%LOCALAPPDATA%\Tjommie` | 47100, 47101 | GitHub Releases |

Only the production build has the update feed, and its address is not even in a dev or e2e binary. A release build is `TJOMMIE_IDENTITY=production cargo build --release --locked -p daemon -p desktop`, and GitHub Actions does it.

## Run from source

Build the daemon first, then start the app:

```powershell
cd tjommie-rs
cargo build -p daemon --bin tjommie-daemon
cargo run -p desktop --bin tjommie-desktop
```

A run from source is the `dev` identity. It keeps its data in `%LOCALAPPDATA%\Tjommie-rs` and uses ports 47200 and 47201, so it runs beside an installed Tjommie without touching its data, its ports or its login entry. It never looks for updates.

The app starts its own daemon. It runs `tjommie-rs\target\debug\tjommie-daemon.exe`, or the exe named by `TJOMMIE_DAEMON_EXE`. If neither exists, the app says to build the daemon. An installed app runs the daemon bundled in its `resources` folder, whatever the environment says. That holds for both apps the installer installs, production and update test. The update test identity run from source has no `resources` folder beside it and follows the source rules above, as do the `dev` and `e2e` identities, which never use a bundled daemon.

To debug the daemon on its own, start it in development mode first and then start the app:

```powershell
cargo run -p daemon --bin tjommie-daemon -- --dev     # in one terminal, inside tjommie-rs
cargo run -p desktop --bin tjommie-desktop            # in another
```

`--dev` (or `TJOMMIE_DEV=1`) saves a `daemon-token` file in the data folder. A `dev` app that finds that file connects to the daemon you started instead of starting its own. If the app says the daemon is not running, a leftover `daemon-token` file from an earlier `--dev` run is the usual cause: start the daemon with `--dev`, or delete the file. If you set `TJOMMIE_HOME` or `TJOMMIE_PORT` for one, give the other the same value.

A debug build of the daemon defaults to the `Tjommie-rs` folder and port 47200. A release build defaults to `Tjommie` and port 47100, like the installed app.

Sign the Claude and Codex accounts in from the app. A run from source keeps them in `Tjommie-rs`, not in the installed app's folder.

The app has no console window. Its log is `logs\desktop.log` in the data folder, and a crash writes `logs\desktop-crash.log` there.

### How the app starts the daemon

The app passes the daemon a fresh token as the first line of its standard input, with `--token-stdin`, and keeps that pipe open. The token is 32 random bytes as 43 base64url characters. The daemon requires at least 43 characters within 5 seconds, never saves the token to a file, deletes an old `daemon-token` file left by development mode, and stops when the pipe closes. Quitting or crashing the app therefore never leaves a daemon running. A daemon started with neither `--dev` nor `--token-stdin` exits with an error.
The app starts the daemon without `TJOMMIE_DEV`, `TJOMMIE_ACCOUNTS_DIR` and every `TJOMMIE_SKIP_*` switch, so nothing meant only for development or tests reaches it.

### Folders and environment variables

| Variable | What it does |
|---|---|
| `TJOMMIE_IDENTITY` | Build time. `dev`, `e2e`, `updatetest` or `production`. |
| `TJOMMIE_HOME` | Another data folder. Give the daemon the same value as the app. |
| `TJOMMIE_PORT`, `TJOMMIE_LAN_PORT` | Another port for the app's connection and for phones. |
| `TJOMMIE_USER_DATA` | Another folder for what the app writes beside the data folder: the one-time markers (`tray-hint-shown`, `login-item-set`) and the speech model. By default a `dev` run uses `<data folder>\electron` for the markers (the name is historical) and `<data folder>\speech` for the model. The installed app uses `%APPDATA%\Tjommie`. |
| `TJOMMIE_DAEMON_EXE` | The daemon exe a non-production app runs. |
| `TJOMMIE_UPDATES=off` | Turns the updater off. |
| `TJOMMIE_UPDATE_FEED` | A local update feed. Only the `updatetest` identity honours it. |
| `TJOMMIE_PROVIDER_FIXTURES` | A folder of recorded conversations. Only a daemon built with `--features provider-fixtures` accepts it. |
| `TJOMMIE_E2E_EXE` | Tests only. The e2e tests launch this desktop exe (an absolute path to an existing file) instead of the one cargo built, and look for `tjommie-daemon.exe` beside it. A bad value fails the test with the reason; it never falls back. Production code does not read it. |
| `TJOMMIE_ACCOUNTS_DIR` | Tests only. Uses a folder of signed-in account folders with a disposable data root. |
| `TJOMMIE_CI_HOSTED=1` | Set on GitHub's hosted runners. Tests that need Credential Manager or a real logon skip themselves, and run on real PCs instead. |

An app never opens a data folder that belongs to another identity. A `dev`, `e2e` or `updatetest` build refuses the installed app's `%LOCALAPPDATA%\Tjommie` and `%APPDATA%\Tjommie`, however the path is spelled, and exits without opening a window.

### Reading a crash log

A panic on the UI thread that nothing catches is a crash. The app adds an entry to `logs\desktop-crash.log` in its data folder and stops, and Windows starts it again (`--hidden --after-crash`). Panics that were caught, and panics on other threads, are logged too, but a panic from one place is written in full once a minute at most. Each entry has a timestamp, the version, the thread and the panic's message, then what a later reader needs to match it to the release that wrote it:

```
2026-11-22T13:08:45Z Tjommie 0.6.0 crash in thread 'main': panicked at crates\desktop\src\ui\example.rs:12:5:
the message
modules:
  tjommie-desktop.exe base=0x00007ff78a280000 size=0x1b01000 timestamp=0x6b02e94d pdb=tjommie_desktop.pdb guid=01234567-89AB-CDEF-0123-456789ABCDEF age=1
  tjommie-daemon.exe (another process) size=0x9c4000 timestamp=0x6b02e931 pdb=tjommie_daemon.pdb guid=FEDCBA98-7654-3210-FEDC-BA9876543210 age=1
stack: tjommie-desktop.exe+0x471bfb tjommie-desktop.exe+0x3aa073 tjommie-desktop.exe+0x3aac03 0x7ffa1c2d1234
   0: …a backtrace, with names when the PDB was beside the exe…
```

- `modules:` names each exe by file name only (never a folder) and gives its build id: the PE header's `timestamp` and `size`, and the name, `guid` and `age` of the PDB that goes with it. The running app also has `base`, where Windows loaded it this time. The daemon is another process with an address space of its own, so its line has no `base`; it is there so that a failure of the daemon can be matched to its PDB.
- `stack:` is the return addresses of the panicking thread. A frame inside the app is `tjommie-desktop.exe+0x<offset from base>`, and the others (Windows' own libraries) are plain addresses. The first frames are the crash handler's own and the standard library's panic machinery; the code that panicked comes after them.

A release is built with line tables kept, and its PDBs are in `Tjommie-<version>-symbols.zip` on the release. To read an entry's stack by function and line:

1. Download that zip for the version the entry names and extract it into a new empty folder. It holds `tjommie_desktop.pdb` and `tjommie_daemon.pdb` at its top level.
2. Check that the PDB is the one that was built with the exe: the entry's `guid` and `age` for the exe must be the PDB's own. `llvm-pdbutil dump --summary tjommie_desktop.pdb` prints them as `GUID` and `Age`. A PDB from another build, even of the same version, has another GUID and gives wrong names, so do not use one.
3. Put the PDB beside a copy of the same version's `tjommie-desktop.exe` (the installed app's, or the release payload's). The symboliser finds the PDB next to the exe, and says `no such file or directory` when it can't.
4. Ask `llvm-symbolizer` (LLVM's, from `winget install LLVM.LLVM` or `scoop install llvm`) for each offset. Every frame is a return address, the instruction after a call, so ask for the offset minus 1:

```powershell
$stack = (Select-String -Path desktop-crash.log -Pattern '^stack: ' | Select-Object -Last 1).Line.Substring(7)
$offsets = $stack -split ' ' | Where-Object { $_ -like 'tjommie-desktop.exe+0x*' } |
  ForEach-Object { '0x{0:x}' -f ([Convert]::ToInt64(($_ -split '\+0x')[1], 16) - 1) }
llvm-symbolizer --obj=.\tjommie-desktop.exe --relative-address @offsets
```

It prints the function and the `file:line` of each frame, innermost first. `crash::tests::hook_writes_log` in `crates/desktop-host/src/crash.rs` proves the numbers are right: it crashes a copy of the test binary on purpose and has DbgHelp name the test's own frame from the logged base and offsets alone.

## Tests

The test commands below are the ones in `.github/workflows/ci.yml`, in its order. CI has two jobs, `check` and `rust`. Run the cargo commands inside `tjommie-rs` and the npm commands in the repository root.

### The `check` job (TypeScript)

```powershell
npm ci
node scripts/protocol-manifest.ts --check
node scripts/browser-tools-manifest.ts --check
node scripts/aegis-theme.ts --check
npm run typecheck
npm run lint
$env:TJOMMIE_CI_HOSTED = '1'; npm test           # vitest
./packages/desktop/scripts/build-speech-runtime.ps1
```

`npm run typecheck` begins with `npm run check:no-any`, which rejects explicit `any` types in first-party TypeScript. `npm test` fails when it finds no test files.

### The `rust` job

```powershell
npm ci                                           # in the repository root: some Rust tests run node
rustup toolchain install                         # inside tjommie-rs
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p daemon --all-targets --features provider-fixtures --locked -- -D warnings
./tools/check-guarded.ps1 -SelfTest
./tools/check-guarded.ps1
$env:TJOMMIE_PDFIUM_DLL = ./tools/fetch-pdfium.ps1 -Destination "$env:TEMP\pdfium\pdfium.dll"
$env:TJOMMIE_CI_HOSTED = '1'; cargo test --workspace --locked --no-fail-fast
```

`tools/fetch-pdfium.ps1` downloads the pinned `pdfium.dll` (the PDF viewer's engine) from the `bblanchon/pdfium-binaries` release named in the script. It checks the archive and the DLL by SHA-256 and throws on a mismatch, then prints the DLL's path, which goes in `TJOMMIE_PDFIUM_DLL`. The PDF tests and the e2e app's PDF viewer look for it there, and on CI they fail without it rather than skip. Run from source, the app also finds it at `tjommie-rs\resources\pdfium\pdfium.dll`, which is where the script puts it when it is given no `-Destination`. An installed app carries its own copy in `resources`. The release pins the same build: the hash is in `crates/desktop-host/src/pdf.rs` and in the script, and they change together.

Then CI checks that the dev and e2e desktop builds have no update feed. It builds the tests without running them, once per identity, and reads `target\debug\tjommie-desktop.exe` for the feed's path, `tsnyders/tjommie/releases/latest/download`:

```powershell
$env:TJOMMIE_IDENTITY = 'dev'; cargo test --workspace --locked --no-run
$env:TJOMMIE_IDENTITY = 'e2e'; cargo test --workspace --locked --no-run --features desktop/test-driver
```

Then the e2e tests (next section), then the daemon-process test against the feature-off daemon, then the provider-fixtures tests, last:

```powershell
cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked
$env:TJOMMIE_IDENTITY = 'e2e'; cargo test --workspace --locked --features desktop/test-driver --test 'e2e_*' --no-fail-fast
cargo build --locked -p daemon --bin tjommie-daemon
$env:TJOMMIE_DAEMON_EXE = "$PWD\target\debug\tjommie-daemon.exe"; $env:TJOMMIE_CI_HOSTED = '1'
npx vitest run packages/desktop/src/daemon-process.test.ts     # from the repository root
$env:TJOMMIE_CI_HOSTED = '1'; cargo test -p daemon --features provider-fixtures --locked --no-fail-fast
```

Clear `TJOMMIE_IDENTITY` and `TJOMMIE_DAEMON_EXE` in your shell between steps if you run them by hand, or the next build is made for the wrong identity.

The order is not arbitrary. All of them write `target\debug\tjommie-daemon.exe`: the feature build comes first for the e2e tests, the feature-off build replaces it before the daemon-process test, and the last step rebuilds it with the feature again.

Under `--locked`, cargo refuses a stale `Cargo.lock`. Commit the lockfile with any dependency change.

### The provider-fixtures trap

The e2e tests start the daemon beside the app, and they set `TJOMMIE_PROVIDER_FIXTURES` to a folder of recorded conversations, so no real Claude or Codex runs and no account is touched. A daemon built **without** `--features provider-fixtures` refuses to start when that variable is set at all, even empty: it exits with code 1, because starting anyway would run the real providers in a test that meant to replay recorded ones. The symptom is an e2e test that waits 60 seconds for the Agents screen and fails.

So build the daemon with the feature before any e2e test:

```powershell
cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked
```

The e2e tests look for `tjommie-daemon.exe` beside `tjommie-desktop.exe` and stop with that command in their message if it is missing.
The feature builds only in the dev profile. It is refused for the production identity and for `--release`, so a shipped daemon cannot contain it. A marker string is in a daemon binary only when the feature is compiled in, so a release check can search for it. The format of the recorded conversations is documented at the top of `tjommie-rs/crates/daemon/src/provider_fixtures.rs`.

### The e2e tests

The e2e tests are `tjommie-rs/crates/desktop/tests/e2e_*.rs`. Each starts the e2e build of the app on a temporary root and drives it through the test driver: `Client::click`, `key`, `text`, `wait`, `snapshot` and `resize`. They need the `test-driver` feature and the e2e identity (`TJOMMIE_IDENTITY=e2e`), and a test says so and stops if the app was built as anything else. To run one:

```powershell
cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked
$env:TJOMMIE_IDENTITY = 'e2e'; cargo test -p desktop --features test-driver --test e2e_navigation --locked
```

The test driver is never in a release build; `tests/pins.rs` checks it. An e2e app puts fake in-memory versions of the tray, notifications and registry in place of the real ones, so a test never adds an icon to your tray, a notification to your desktop or a value to your registry. It never takes your keyboard focus.

**Every e2e test runs the window at 1000 by 700.** `common::make_agent` calls `app.resize(1000., 700.)`, and the keyboard test sets the same size. The size is the same on every machine on purpose: it fits a hosted runner's 1024 by 768 display, and Windows holds a window to its display, so a taller window cannot exist there. Nothing in a test may depend on a taller one. A click on a control below the fold lands outside the window and does nothing, without an error. Use `tab_to`, which sends Tab until the control has focus, because the screen scrolls the focused control into view.

### The README's pictures

The pictures in the README (`docs/screens/`) are taken by one e2e test, `e2e_readme_shots`, which `.github/workflows/readme-shots.yml` runs on a GitHub hosted runner. `docs/screens/SHOTS.md` lists every picture, the invented data it shows, and what to check by eye before one is committed.

**Never take them on your own PC, and never commit a picture made on one.** The sidebar's host button shows the first letter of the Windows account's name, and Settings shows the PC's host name. A runner's are generic. The workflow has `contents: read`, uses no secrets, and runs only when started by hand:

```powershell
gh workflow run readme-shots.yml --ref <branch>
gh run download <run id> --name readme-shots --dir <a new empty folder>
```

The test is marked `#[ignore]`, because `ci.yml` runs every `e2e_*` test and this one takes pictures instead of checking anything. The workflow asks for it with `-- --ignored`. It builds the normal daemon and copies it away, builds the fixtures daemon (the two share `target\debug\tjommie-daemon.exe`), fetches pdfium, and runs the test. One picture, the first-run screen that says Claude Code and Codex are not installed, needs the normal daemon on a machine that has neither, and the test takes it only when `TJOMMIE_SHOTS_PLAIN_DAEMON` names that daemon's exe, so it is skipped on a PC. Every other picture uses recorded conversations, so no provider runs and no account is touched.

You can run the test on a PC to check that a change to it works. The pictures it writes (`tjommie-rs\target\readme-shots`) are not for the README: look at one, then delete them all.

```powershell
cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked
$env:TJOMMIE_IDENTITY = 'e2e'; cargo test -p desktop --features test-driver --test e2e_readme_shots --locked -- --ignored --nocapture
```

A picture that can't be taken is reported at the end of the run and does not stop the others. The artifact keeps whatever was taken, and the test fails when any is missing.

### Rules the code enforces

- **No panics at UI boundaries.** The `client`, `desktop-host` and `desktop` crates deny `unwrap`, `expect`, indexing, `panic!`, `unreachable!`, `todo!`, `unimplemented!` and string slicing in non-test code (`[workspace.lints.clippy]` in `tjommie-rs/Cargo.toml`). The daemon does not carry these lints.
- **Every handler runs inside `guarded(`.** In the `desktop` crate, every handler registration runs its whole closure inside `guarded(`, so a panic in a handler becomes a notice instead of closing the app. `tools/check-guarded.ps1` checks this, and `-SelfTest` runs the checker's own self-test first.
- **Dependencies are pinned.** GPUI and its component kit are pinned to exact versions, and `crates/desktop/tests/pins.rs` checks the pins. Upgrade only for a named reason.
- **TypeScript has no `any`.**

## Security notes and known limits

These are true of the daemon and the agents' browser, and they carry over from the earlier README.

- API keys are stored in Windows Credential Manager. Windows protects those entries for the current user. That is not protection against unrestricted code running as the same user.
- API-key work with Claude needs Claude Code 2.1.251 or later, so that a command the agent runs does not inherit the key.
- On Windows, a Codex agent at **Own folder** needs Codex's elevated sandbox. Setting it up asks for administrator permission once. Until it is ready the agent shows **Needs setup** and does not run. A Codex agent can still read the rest of your user profile, and Codex agents can read each other's folders.
- The agents' browser grants a page nothing: every permission request is denied, including the camera, the microphone and screen capture.
- Windows builds are unsigned. The updater checks the download against the SHA-512 in the release's `latest.yml`, over HTTPS from GitHub only, and checks that the installer is for this app. There is no publisher signature behind that check.
- `tjommie-daemon accept-worker` is the one-time check that proves isolated mode on a real Windows account. Run it from your own unlocked, signed-in session with Tjommie closed. It starts the real provider tools, so it refuses to run while `TJOMMIE_PROVIDER_FIXTURES` is set.
