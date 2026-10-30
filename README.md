# Tjommie

Tjommie is a free, open-source desktop app for AI bots that chat with you, work on your PC, talk to each other and run routines. The bots run on your own computer through the official Claude Code and Codex CLIs, signed in with your own accounts. An Android companion connects over the same Wi-Fi; mobile-data access and push are still planned.

**Status:** early development. The design is in [`docs/superpowers/specs/2026-09-25-tjommie-design.md`](docs/superpowers/specs/2026-09-25-tjommie-design.md).

## Develop

You need Windows and Node 24 or newer.

Install the Claude Code and Codex CLIs and put them on PATH. Tjommie stores its data in `%LOCALAPPDATA%\Tjommie`; set `TJOMMIE_HOME` to use another folder and `TJOMMIE_PORT` to change the default port, 47100. The installed browser uses Electron's bundled Chromium; standalone development browser fixtures use Chrome or Edge. From the repo root, `npm run chat -w @tjommie/daemon -- <name>` creates the named bot if it does not exist.

`npm start -w @tjommie/daemon` starts development mode. It saves a `daemon-token` file in the Tjommie data folder so the terminal chat can connect. Development mode also allows the UI at `http://localhost:3000`. Running `src/main.ts --dev` directly or setting `$env:TJOMMIE_DEV = '1'` does the same thing. The desktop origin `app://tjommie` is always allowed.

The installed desktop app starts the daemon with `--token-stdin`. It must start the daemon with `TJOMMIE_DEV` unset. It passes a fresh token as the first line of a pipe to the daemon's standard input and keeps that pipe open. The daemon requires at least 43 base64url characters within 5 seconds, never saves this token to a file, deletes an old `daemon-token` file left by development mode, and stops when the pipe closes, so quitting or crashing the desktop app never leaves a daemon running. The terminal chat only works with a development daemon. Starting the daemon with neither mode exits with instructions to use the desktop app or the development command. Tests can set `TJOMMIE_ACCOUNTS_DIR` to use signed-in account folders with a disposable data root.

The desktop app has two modes. By default it reads the development daemon's `daemon-token` file and shows the UI from `http://localhost:3000`, so both must be running. With `$env:TJOMMIE_DESKTOP = 'production'` it starts its own daemon with `--token-stdin` and serves the static export in `packages/ui/out` (run `npm run build` first), the way the installed app will. Closing the window keeps Tjommie running in the tray; Quit Tjommie in the tray menu stops the daemon, and with it the bots and routines. `npm run test:real` also drives the desktop app in production mode.

```sh
npm install
npm run signin -w @tjommie/daemon    # once: sign Tjommie's own Claude and Codex accounts in
npm start -w @tjommie/daemon         # development daemon, on http://127.0.0.1:47100
npm run chat -w @tjommie/daemon -- "Researcher" claude   # chat with a bot from the terminal
#   in the chat: /room <text> posts in the Team room; /allow <id> answers a card, including a routine a bot proposes
npm run dev        # UI at http://localhost:3000
npm run desktop    # the desktop app, on that UI and the development daemon (keep both running)
npm test           # fast tests
npm run test:real  # tests that drive the real Claude and Codex CLIs (needs the sign-in above)
npm run check:no-any # rejects explicit any types in first-party TypeScript
npm run typecheck
npm run lint
npm run build      # static export to packages/ui/out
```

| Package | What it is |
|---|---|
| `packages/protocol` | Messages shared by the daemon and the UI |
| `packages/client` | The typed WebSocket client for the daemon, shared by the UI, the desktop app and the terminal chat |
| `packages/daemon` | The background process that runs the bots |
| `packages/ui` | The UI: Next.js static export, Base UI, Tailwind |
| `packages/desktop` | Electron shell |
| `packages/mobile` | Android Capacitor companion and native plugins |
| `packages/transport` | Shared pinned Snow WASM Noise implementation |

## Install

`npm run package` builds the Windows 0.3.1 installer, `dist-desktop\installer\Tjommie-Setup-0.3.1.exe`. It installs Tjommie for the current Windows user, without admin rights, and starts it. The installer isn't signed yet, so Windows SmartScreen asks once: choose "More info", then "Run anyway". Uninstalling keeps your bots and their data in `%LOCALAPPDATA%\Tjommie`.

After packaging, run `npm run test:packaged`. This mandatory gate requires the unpacked executable and fails if the smoke suite is missing, skipped, empty, or failing. The build stamps `dist-desktop/provenance.json`; the gate checks source-input hashes and every installer/payload file before and after the smoke test, rejecting stale or changed builds. It verifies packaged startup, required browser-tool files, and shutdown. `npm run test:real` covers the source application and providers separately. Installation and the full installed-app demo in Phase 1E Task 16 remain separate acceptance checks.

After installing, set `TJOMMIE_TEST_EXECUTABLE` to the installed `Tjommie.exe` absolute path to run the existing desktop lifecycle and screen suites against that executable. They retain disposable data roots and Electron profiles and use the Tjommie-owned provider accounts. Unset it before returning to source-app tests. The mandatory packaged smoke always supplies its own unpacked executable path.

TypeScript must never use explicit `any` types. `npm run typecheck` enforces this with the syntax-based `check:no-any` gate before checking types. Test commands fail when no test files are found.

## Accounts and usage

For Android, run `npm run package:android` for a debug-signed test APK and a UI-byte/source provenance record in `dist-mobile/`. On the PC, open Settings → Devices → Pair a device, then scan or paste its code in the Android app. Pairing expires after ten minutes and works once. The PC must be awake and reachable on the same network (TCP port 47101); Windows may ask to allow the app on the private network. Every paired device has full owner authority. Revoke a lost phone from Devices. See [`packages/mobile/README.md`](packages/mobile/README.md) for signing, build requirements and current limits.

Settings → Accounts manages separate Claude Code and Codex accounts. Existing accounts keep their IDs and sign-in folders when upgraded. Each bot has an account pin; changing the provider's default affects new bots, while changing a bot's pin starts a fresh provider session and keeps its chat.

Subscription sign-in is an explicit action that opens the provider's page in the system browser. Claude uses the browser's loopback callback without opening a terminal; manual-code entry is not offered by this integration. API keys are stored in Windows Credential Manager, with only an opaque reference in the database. They are not returned in account status. Windows protects these entries for the current user; this is not protection against unrestricted code running as that same user.

Claude API-key work requires Claude Code 2.1.251 or later for subprocess environment scrubbing. A synthetic loopback API test with the installed CLI verified that its Bash subprocess did not inherit the API key. This does not isolate secrets from arbitrary code running under the same Windows account.

An API-key account needs a spending cap. The cap covers its total recorded API spending, with no automatic monthly reset. Prices and provider reports are local accounting inputs, not an invoice or a guaranteed billing ceiling. Unsupported Codex pricing prevents a paid turn; an unpriced completed turn blocks further API-key work rather than treating its cost as zero. A subscription can use the provider's explicitly selected API-key fallback when limited, then return to its pinned account when the limit resets. Tjommie does not silently rotate between subscription accounts.

Settings → Usage shows Today, This week, This month and This year (calendar year to date). A combined cost estimate gives the API-equivalent value of all recorded token usage, alongside separate API-key spending and subscription API value. It is not your subscription bill. Missing costs remain unknown; incomplete totals show a known minimum. Date ranges use the displayed time zone; the year chart groups tokens by month.

The provider cards show reported allowance windows, reset times, plan/account details, provider credits and extra-usage status where available. Credits are provider units, not dollars. Refresh asks for metadata only, without starting a model turn. Codex uses its app-server metadata RPC. Claude's selected-account reader uses the same fixed OAuth usage endpoint as Paseo; it is an unofficial metadata endpoint and can become unavailable. The backend does not expose, log, modify or refresh the CLI token. A failed reading retains and labels the last successful data. Removing an account requires moving its assigned bots first, disconnects it locally, and retains historical usage.

Below 640 px, the Quiet pill replaces the desktop rail with Agents, Team and Settings navigation. Details, exchanges and members become full-screen sheets, and forms and account dialogs fit the narrow layout. Narrowing the desktop window still provides its native browser. The Android companion connects through the encrypted pairing flow described above.

The isolated desktop account check can be run with `npx vitest run --mode real packages/desktop/src/accounts.real.test.ts` after `npm run build`. It uses unsigned disposable profiles, synthetic usage, and a fake key stored under a disposable credential target; it does not sign in or make a paid provider call. The broader `test:real` command includes real-provider tests and must not be used as a substitute for this isolated check.

## Bot browser

Enable Browser for a bot, then choose Open browser in its chat to browse inside the right panel. The desktop app owns a live native browser view with a separate persistent profile for each bot. The person and the bot use the same tabs. Browser tools need the desktop app running, including when its window is hidden in the tray.

Use the Details / Browser tabs to return to bot settings, routines and activity without losing the browser page or address draft. Drag the panel divider to change its width; the focused divider also accepts arrow keys. Expand browser fills the application content area, and Restore browser returns to the previous width.

Take control before typing or clicking. Resume bot gives its browser tools control again. Returning to Details or hiding the panel releases manual control without closing the browser session. Website pages run in isolated native guest contents without the application's preload or Node access. Human browsing no longer uses JPEG polling, and manual input is not stored as chat messages.

Closing or disabling a bot's browser keeps its sign-ins. Deleting the bot clears its browser storage and cookies before deleting the bot. Windows may temporarily lock cleared profile files; startup retries removing those orphaned files. Camera, microphone, screen capture and device access are denied.

Old headless Chrome profiles are retained for rollback and are not imported into the native browser. Local synthetic-site checks establish cookie persistence, not compatibility with every third-party sign-in service. Google rejected sign-in in the previous automated panel during owner testing. Native rendering does not establish that Google accepts sign-in; owner-controlled real-site acceptance remains required. Media codecs, protected playback, passkeys and operating-system authentication also require site-specific validation.

The separate Google API connection is on hold while native website browsing is validated. OAuth API consent does not sign the browser into Google websites. No Google API connection is offered in the bot panel in this version.

Startup routine failures are retained until the desktop handles and acknowledges them. Reconnecting retries an unacknowledged notice. A desktop crash between showing a notification and acknowledging it can cause one replay after restart.

## Autonomy

On Windows, Codex bots at Own folder need Codex's elevated sandbox. Setting it up may show one Windows admin prompt. Until it is ready, those bots show Needs setup and do not run. Tjommie denies sandboxed Codex commands access to its sign-in folders, Claude bots' folders, logs, database and shared-files folder. The installed daemon token is passed through a pipe; a development token file is protected if present.

Codex bots can still read the rest of your user profile. A Codex bot at Own folder can read the folders of your other Codex bots (their notes and attached files), whether or not they're running. Claude bots' folders are protected. Browser cookies stay unreadable. Local services, including Tjommie's health endpoint, may be reachable from a sandboxed command.

## License

Apache-2.0. Tjommie is an independent project, not affiliated with the makers of Grok Bot, Claude Code or Codex.
