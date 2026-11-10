# Tjommie Rust rewrite: spec

- Date: 2026-11-09. Branch: `rust-rewrite`. Baseline: v0.4.2 (`aa20216`, `packages/*`).
- Targets: the owner's Windows 11 PC and Samsung S25 Ultra only.
- Rule: `packages/*` at the baseline defines behaviour. If this spec and the code disagree, the code wins and the spec gets fixed. Facts are cited, not copied. Read the cited lines.

## 1. Scope
**Goal:** replace the Node daemon, then Electron and Capacitor, with:
- A Rust daemon
- A Rust desktop UI (GPUI, unless the spike gate in Phase 2 says otherwise)
- A Kotlin/Jetpack Compose Android app

Parity is defined in §6. The UI follows the design system the owner picks in Phase 0.

The first UI is **Aegis** (`design/claude-design/aegis/`), including its motion. Workbench (`design/claude-design/workbench/`) and Mission Control (`design/claude-design/mission-control/`) come later as switchable themes, each with light and dark modes (§9).

**Order (owner decision): daemon first.** The Rust daemon ships first inside the existing Electron app, as a separate process, as today. The desktop UI and the Compose app come later. There is no in-process daemon.

**Added in the first release (owner decision):**
- A live browser stream (§3.9)
- Native Win32 code instead of the PowerShell scripts (§3.8)

Auto-update stays working through every release (§3.10).

**Not in scope:**
- Other OSes or devices.
- Windows code signing (builds stay unsigned).
- Rewriting the relay. `packages/relay` stays the TypeScript Cloudflare Worker.
- Everything in §9.

**Repo rules (AGENTS.md):**
- Every user-visible change adds a line under `## Unreleased` in `CHANGELOG.md`.
- Releases follow the AGENTS.md steps.
- Builds and releases run on GitHub Actions only.
- No `any` in TypeScript. In Rust and Kotlin, no untyped JSON (`serde_json::Value` or `JsonElement`) past the wire-decode boundary.
- No console windows ever. Every spawned process gets `CREATE_NO_WINDOW` through one helper, and a test asserts it. Today 13 production files pass `windowsHide`.

## 2. Architecture
`tjommie-rs/` is a Cargo workspace plus `android/`. Crates:

| Crate | Purpose |
|---|---|
| `protocol` | Wire types |
| `transport` | Port of `packages/transport/native/src/lib.rs` without wasm-bindgen |
| `daemon` | Binary `tjommie-daemon.exe`. Includes SQLite and Win32 |
| `client` | Shared by desktop and Android |
| `ffi` | UniFFI bindings for Android |
| `desktop` | Desktop UI |

Split further only when a crate needs it.

**Daemon launch contract** (unchanged, so Electron can host it):
- Started with `--token-stdin` (`desktop/src/daemon-process.ts:75`).
- Reads one JSON line `{token, desktopHostToken}` from stdin (`:94`).
- Exits when stdin closes.
- Accepts `--dev`. Production vs dev handling is at `daemon/src/main.ts:26-40`.
- Stopped with a tree kill (`daemon-process.ts:100`).

**The desktop process is a second client of the daemon.** It serves the native browser, the worker boundary and the vault host through `/browser-host`, `/worker-host` and `/vault-host`, authenticated with `desktopHostToken` (`daemon.ts:175-179`, `:648-656`). The schemas are in `protocol/src/native-browser.ts` and `vault-host.ts`, and the server side is in `daemon/src/worker-boundary-host.ts`.

## 3. Contracts and facts (port these 1:1)

### 3.1 Wire protocol
- **Version:** `PROTOCOL_VERSION = 2` (`protocol/src/wire.ts:32`).
  - The server rejects other versions with close code 4003 (`daemon/src/server.ts:131-132`).
  - A bad token gets 4401 (`:127-128`).
  - The welcome message is at `:138`. `/health` is at `:56-57`.
- **Frames:** `wire.ts:137-147`. `ErrorCode` has 14 codes (`:34-38`).
- **Methods:** 101, in `methods` (`wire.ts:50-94`). That includes the spread sub-tables in `vault.ts`, `diagnostics.ts`, `attachments.ts`, `projects.ts`, `remote.ts`, `accounts.ts`, `browser.ts` and `google.ts`.
- **Events:** 26, in `events` (`wire.ts:99-128`). `notify` kinds are at `:115-121`.
- **Port all of `protocol/src/*.ts`.**

**Contract test (new).** Today none exists; `wire.test.ts` checks keys one at a time.
1. `scripts/protocol-manifest.ts` writes `tjommie-rs/protocol/manifest.json` with every method and event name and its params, result and data as JSON Schema (zod v4 `z.toJSONSchema`).
2. Rust types are generated from the manifest or checked against it.
3. Rust tests check two things:
   - The method and event name sets equal the manifest.
   - Every response and event that the Rust integration tests emit validates against the manifest schema.
4. CI regenerates the manifest and fails on a diff.
5. Schemas that zod cannot export get a `custom` entry and a hand-written test. Zod's JSON Schema export also silently drops refinements (`.refine`, `.superRefine`, `.check`, `z.url()` trimming), so each refinement is ported by hand as a Rust newtype with a test. Examples are IANA time zones, https-only relay URLs, API-key byte limits, unique names and date ranges. String trimming that zod does before length checks is marked `x-trim` in the manifest.

### 3.2 Chat semantics and limits
- **Chat:** `chat.send` takes `{botId, text, files}` (`wire.ts:71-74`). There is also `chat.stop` (`:75`).
- **Steer or queue:** the daemon decides (`runner.ts:268-282`, `:726-762`; `queue.ts:123-132`; `providers/types.ts:45`).
- **Hop limit:** `chainLimit` = 5 (`settings.ts:5`), enforced at `team.ts:483`.
- **Scheduler:** tick 30 s, grace 120 s (`scheduler.ts:9-10`). `RUNS_KEPT = 20` (`routines.ts:6`).
- **Attachments:** chunk 256 KiB, max 200 MiB, up to 64 companions (`attachments.ts:3-5`).
- **Speech limits:** `speech.ts:4`. Speech never crosses the wire (`:3`).

**Enums:**

| Enum | Values | Source |
|---|---|---|
| `Autonomy` | `ask`, `folder`, `full` | `model.ts:14` |
| `BotState` | | `model.ts:23` |
| `Provider` | | `model.ts:10` |
| `ProjectRole` | `lead`, `developer`, `reviewer`, `tester` | `projects.ts:8` |
| `PushKind` | `approval`, `limit`, `routine_failed`, `done`, `failed` | `remote.ts:40` |

### 3.3 Transport and relay
- **Noise:** patterns at `transport/native/src/lib.rs:5-6`; `snow =0.10.0` (`Cargo.toml:13`). The prologue is the domain prefix `"Tjommie/remote/v1\0"` followed by `remotePrologue(preface)` (`transport/src/index.ts`).
- **Framing:** a 4-byte big-endian length prefix (`lengthPrefixed` / `FrameDecoder`), with each Noise record capped at `MAX_NOISE_FRAME = 65535` (`client/src/encrypted-channel.ts:7`, `:66`). Larger messages span several records.
- **Errors to remote devices:** any non-`RpcError` message and any schema failure is replaced with the generic text "This request could not be completed on the PC." The error code is kept.
- **Peer wire:** `daemon/src/peer-wire.ts`.
- **Relay:**
  - URL, limits, login message and close codes: `protocol/src/relay.ts:5-25`.
  - Ed25519 challenge/sign: `daemon/src/relay-client.ts:146-160` and `relay-identity.ts`.
- **LAN:** port 47101 (`remote-host.ts:24`).

### 3.4 HTTP and MCP routes
- **`/mcp`:** a stateless Streamable HTTP MCP server (`daemon/src/mcp.ts:48`).
  - It has 5 collaboration tools (`team.ts:241-282`): `request_login`, `list_bots`, `message_bot`, `share_file`, `add_routine`.
  - The `google_gmail_*` tools (`google-tools.ts:50-64`) are not registered in production.
- **`/mcp/browser`:** the 26 `browser_*` tools (`desktop/src/native-browser-automation.ts:387-495`), served through `daemon/src/native-browser-mcp.ts`.

### 3.5 Providers
**Claude:**
- Flags: `providers/claude.ts:20-50`.
- Interrupt is a `control_request` (`:159-162`).
- `CLAUDE_CONFIG_DIR` points at the account folder (`setup.ts:124`).

**Codex:**
- Launched as `codex app-server` (`codex-rpc.ts:131`), with `CODEX_HOME` set.
- Requests it sends:
  - `initialize`, then the `initialized` notification
  - `thread/start`, `thread/resume`
  - `turn/start`, `turn/steer`, `turn/interrupt`
  - `account/login/start`, `account/login/cancel`
  - `account/read`, `account/rateLimits/read`
  - `config/read`, `model/list`

  Sources: `codex.ts`, `setup.ts:184-477`.
- Notifications it handles:
  - `turn/started`, `turn/completed`, `thread/tokenUsage/updated`
  - `item/agentMessage/delta`, `item/started`, `item/completed`
  - `account/rateLimits/updated`, `account/login/completed`
- Server requests it answers: `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `mcpServer/elicitation/request` (`codex-parse.ts`, `codex.ts:184`). Any other server request gets JSON-RPC error `-32601`.
- For API-key accounts, `codex app-server` also gets an ephemeral `-c` config flag (see `codex.ts`).
- A lost app-server process (not only a normal exit) ends the session with an error. The Rust port goes beyond TypeScript here, which listened only for `exit`.

**Project folders:** Rust rejects a drive-less rooted path such as `\projects\x`, which Node's `isAbsolute` accepts. That's stricter and intentional.

**Quota:** `account-quota.ts`. Codex uses `rateLimits` (`:24`). Claude uses the OAuth usage endpoint (`:113-172`).

### 3.6 Database and paths
**Database:**
- Library: `node:sqlite` (`db.ts:2`).
- **12 migrations** (`db.ts:10-241`, two of them from `project-schema.ts:2`, `:28`). Copy the SQL byte for byte.
- PRAGMAs: `db.ts:256`.
- Migrations run one at a time, each in a transaction, then set `user_version` (`:257-262`).
- After the migrations, an unversioned index is created (`:265`).
- Legacy files are moved into `data\` first (`paths.ts:30-48`).
- `data\logins.sqlite` holds login metadata only (`desktop/src/main.ts:226`; schema at `vault-storage.ts:63-64`).

**Paths:**
- **Root:** `TJOMMIE_HOME` or `%LOCALAPPDATA%\Tjommie` (`paths.ts:10`). Folders are created at `:12-23`.
- **Ports:** daemon 47100 (`paths.ts:6`, `TJOMMIE_PORT` at `main.ts:26`), LAN 47101.
- **Per-bot folders:** `bots/<id>/`.
- **Browser profiles:** `data/native-browser/<sha256(botId)>` (`desktop/src/main.ts:617`).
- **Account folders:** `accounts/`.
- **Worktrees:** `workspaces/`.
- **Speech models:** Electron userData `speech/` (`main.ts:195`).
- **Paired devices:** tables `devices`, `remote_host` and `pairing_codes` (`db.ts:210-221`).

### 3.7 Secrets
**Credential Manager.** Type 1 (generic), Persist 2. UserName is `Tjommie` (`account-credentials.ts:28-51`), except the `isolated-worker` and `isolated-review` passwords, which use `Tjommie isolated worker` (`scripts/vault/worker-credential.ps1`). The blob is UTF-8 with no terminator, and its length is in bytes. Target names are `Tjommie/<ns>/…`, where `<ns>` is a prefix of the sha256 of the lowercased root:

| Target | Namespace length | Source |
|---|---|---|
| `account/<sha256(id)>` | 24 hex | `account-credentials.ts:92-97` |
| `login/<uuid>` | 24 hex | `vault-storage.ts:27-32` |
| `isolated-worker`, `isolated-review` | 24 hex | `windows-worker.ts:16`, `:169` |
| `remote-host` (Noise key) | 32 hex | `remote-identity.ts:17-38` |
| `relay-login` (Ed25519 key) | 32 hex | `relay-identity.ts:10-25` |

Because `<ns>` comes from the root, a different root means different targets.

**Claude CLI OAuth:** `accounts/<id>/.credentials.json`.

**Google DPAPI vault** (`google-vault.ts`): not wired in production, so there is nothing to migrate.

**Rule:** Rust writes byte-identical entries, so the 0.4.2 TypeScript daemon can read them after a rollback.

### 3.8 Windows isolation: native Rust (owner decision)
Port `account-credentials.ts`, `windows-acl.ts`, `windows-worker.ts` plus `windows-worker.ps1`, and `scripts/vault/provision-worker.ps1` to the `windows` crate:

| Today | Native Rust |
|---|---|
| Credential Manager via PowerShell | `CredReadW`, `CredWriteW`, `CredDeleteW` |
| `icacls` and `windows-acl.ts` grants and denies, including the `CodexSandboxUsers` checks at `:54`, `:98`, `:271` | `GetNamedSecurityInfoW`, `SetEntriesInAclW`, `SetNamedSecurityInfoW` |
| Worker accounts `TjWorker-<ns8>` and `TjReview-<ns8>`, in the Users group | `NetUserAdd`, `NetLocalGroupAddMembers` |
| Named job `Global\TjommieWorkerJob-…` (`windows-worker.ts:312`) with kill-on-close | `CreateJobObjectW` |
| `CreateProcessWithLogonW` launches (`windows-worker.ps1:237-271`) | Same call, plus `CREATE_NO_WINDOW` |

Provisioning keeps today's elevation flow.

**Parity harness:**
1. Cross-implementation tests:
   - A secret written by TypeScript is read by Rust, and the reverse.
   - The SDDL of a folder prepared by TypeScript equals the SDDL of the same folder prepared by Rust.
   - The job and account names match.
2. The existing real tests are run against the Rust binary, through a `TJOMMIE_DAEMON_EXE` switch, wherever they talk to the daemon from outside: `windows-acl`, `daemon`, `account-scrub`, `runner`, `project-review`, and the desktop `accounts` and `native-browser` real tests.

**Owner-install safety:**
- Account, ACL and job tests run first on GitHub windows runners (disposable).
- On the owner's PC they run only with a dev root. Its `<ns>` gives distinct `TjWorker-` names and Credential Manager targets.
- The harness refuses to run if the root resolves to `%LOCALAPPDATA%\Tjommie`.
- Cleanup deletes only accounts and targets that carry the dev `<ns>`.

### 3.9 Browser and live stream (owner decision)
**Today:**
- `WebContentsView` per bot (`native-browser.ts:577`), with a shield view over it (`:719`).
- Private mode for login filling: `:339-385`.
- Remote viewing is a pull loop. `browser.frame` takes `{botId, after:{streamId, sequence}}` (`protocol/src/browser.ts:46`).
- That loop is fed by an Electron `Page.startScreencast` at JPEG quality 60, at most 2560×1600 and 2 MiB (`native-browser-frame.ts:7-26`).

**New (additive, so `PROTOCOL_VERSION` stays 2):**
- Method `browser.stream {botId, on, maxFps}`.
- A per-connection event `browser.streamFrame {streamId, sequence, jpeg}`. It goes only to the subscribing connection.
- The client acknowledges each frame with `browser.streamAck {botId, sequence}`. At most 2 frames are unacknowledged per connection. A newer frame replaces an unsent one: frames are dropped, never queued.
- Private mode stops the stream.
- Old daemons answer `unknown_method`, and clients fall back to `browser.frame` polling.

**Where it lives:**
- Daemon-first phase: the source is Electron's existing screencast. It is sent through `/browser-host` to the Rust daemon and drawn by `packages/ui` on the phone and on paired PCs.
- Desktop-UI phase: the source is the engine chosen at the Phase 2 gate.

**Target and acceptance test:**
- Fixture page: a `requestAnimationFrame` counter at 1280×800, JPEG quality 60, streamed for 60 s.
- On LAN: delivered median ≥ 55 fps, and p95 gap between frames ≤ 34 ms.
- Automated on loopback in CI. On the owner's S25, the diagnostics overlay shows the numbers.
- Over the relay: fps and MB/min are measured and reported, and the owner sets the relay cap.
- If the Electron screencast cannot meet the LAN target, the owner decides: ship R1 at the measured rate, or move the stream to the desktop-UI release.

### 3.10 Desktop shell, updates, speech, PDF
- **Identity:** appId `com.tsnyders.tjommie`, productName `Tjommie`, artifact `Tjommie-Setup-${version}.exe`, per-user oneClick NSIS (`desktop/scripts/package.ts:34-35`, `:103`, `:111`). The NSIS GUID is derived from the appId.
- **Isolated test identity:** `package.ts:20-27`.
- **Start at login:** `login-item.ts:16-21`.
- **The updater exists today.** It is electron-updater (`desktop/package.json`; `updater.ts:29-45`). First check after 60 s, then every 6 h. It downloads automatically and installs on quit. The feed is GitHub Releases `latest.yml` (`package.ts:112`), checked in `release.yml:66-84`.
  - **R1:** unchanged. The Rust daemon is bundled as a resource.
  - **Desktop-UI release:** a Rust updater reads the same `latest.yml`. It checks the version, verifies the sha512, downloads to a temp folder and runs the NSIS installer with `/S` on quit. This works unsigned.
  - **Why not Velopack:** it uses its own installer and install folder. Moving from 0.4.x would need a bridge release that removes the NSIS install, and an uninstaller once deleted the owner's install.
  - **Dev builds** are compiled with no feed URL. CI asserts that the dev binary contains no `github.com/tsnyders/tjommie` feed. The updater also refuses any installer whose embedded appId differs from its own.
- **Speech:** CPU `whisper-cli.exe`, built from a pinned commit in CI (`scripts/build-speech-runtime.ps1`). The model is `ggml-base.en.bin`, downloaded at runtime and checked by sha256 (`speech-model.ts:7-11`, `:81-151`).
- **PDF:** `pdfjs-dist` today. Desktop-UI phase: bundled pdfium. Android: `PdfRenderer`.

### 3.11 Android app
- **Identity:** `dev.tjommie.app` (`capacitor.config.ts:6`).
- **SDK levels:** min 24 (kept, owner decision), compile and target 36 (`variables.gradle:2-5`).
- **Plugins:** 7 in `packages/mobile/native/java/dev/tjommie/app/Tjommie*Plugin.java`, plus the Attachment* range and save helpers.
- **Secure store:** an AES-GCM key in AndroidKeyStore with alias `tjommie.securestore.v1`. Ciphertext goes in the `tjommie_secure` prefs (`TjommieSecureStorePlugin.java:27-99`).
- **WebView `localStorage`:** holds hosts (`ui/lib/remote-hosts.ts:42-53`), push kinds (`push.ts:121-132`) and appearance (`appearance-settings.ts`).
- **FCM:** `apply-native.mjs:23-26`, `:110`, `:139`.
- **versionCode map:** `apply-native.mjs:11`.

**API levels with minSdk 24.** Keep today's gates:

| Feature | Gate |
|---|---|
| On-device dictation (`SpeechRecognizer.createOnDeviceSpeechRecognizer`) | API 33. Below that, dictation shows "unavailable" (`TjommieSpeechPlugin.java:61`, `:244`). No cloud fallback: audio stays on the device |
| Speech model download | API 34 (`:165`) |
| Notification channels | API 26 (`TjommiePushRoutingPlugin.java:24`) |
| `POST_NOTIFICATIONS` runtime request | API 33 only. Granted at install below that |
| Appearance system-theme branch | API 29 (`TjommieAppearancePlugin.java:48`) |

Pickers use SAF (`OpenDocument`, `CreateDocument`), which needs API 19 or later. The Keystore AES-GCM scheme, CameraX, ML Kit, Compose and `registerDefaultNetworkCallback` all work at API 24. Version branches are tested with Robolectric at sdk 24 and 36.

### 3.12 CI and tests today
- `ci.yml`: windows-latest, Node 24.
- `release.yml`: runs on a `v*` tag. Jobs: verify (`apply-native --check`, CHANGELOG notes), windows, android (signing secrets at `:122-136`), release.
- Tests: vitest. 299 test files, of which 30 are `*.real.test.ts` (excluded by default, `vitest.config.ts:20`).
- No Rust runs in CI.

## 4. Coexistence identity (dev builds, until each cutover)

| Item | Production | Dev |
|---|---|---|
| Desktop appId / NSIS GUID | `com.tsnyders.tjommie` / derived | `com.tsnyders.tjommie-rs` / explicit, distinct GUID |
| Product / exe | Tjommie / `Tjommie.exe` | Tjommie RS / `TjommieRS.exe` |
| Data root | `%LOCALAPPDATA%\Tjommie` | `%LOCALAPPDATA%\Tjommie-rs` (via `TJOMMIE_HOME`) |
| Ports | 47100 / 47101 | 47200 / 47201 |
| Credential Manager namespace, worker accounts | from the root | distinct automatically. Removed on dev uninstall |
| Speech cache | Electron userData | `<dev root>\speech` |
| Update feed / login item | GitHub `latest.yml` / on | none / off |
| Android applicationId | `dev.tjommie.app` | `dev.tjommie.rs`. The owner registers it in Firebase; until then, push is off |

Dev installers are tested first on a GitHub runner with a v0.4.2 install present. That test checks the production install's file count before and after against a fresh build. They go to the owner's PC only after that test passes.

**One-way import for dev testing.** `tjommie-daemon import --from <prod root>` does the following:
- Refuses if the target exists, unless `--replace` is given.
- Copies `tjommie.db` and `logins.sqlite` with the SQLite backup API (safe while production runs) and copies `bots/`.
- Re-writes the `account/` and `login/` secrets under the dev namespace.
- Does **not** copy:
  - `.credentials.json`. Refresh-token rotation would sign out one install.
  - The `remote-host` or `relay-login` keys.
  - Worker passwords.
- Clears `devices`, `remote_host` and `pairing_codes`.
- Never writes to the production root.

## 5. Phases and exit criteria
Phases 0 and 2 run in parallel with Phase 1.

| Phase | Ships as |
|---|---|
| 1 | Release R1 |
| 3 | Desktop-UI release |
| 4 | Android release |

### Phase 0: design selection (before any UI code)
Three directions are produced as Claude Design design systems in `design/claude-design/<direction>/`. Each direction includes:
- **Tokens:** colour (light and dark), type scale, spacing, radius, elevation and motion durations.
- **Core components:**
  - App shell and navigation, host switcher
  - Bot avatar, status pill
  - Message, tool, approval and exchange cards
  - Composer with dictation and attachments
  - Browser panel chrome (tabs, address bar, take control / resume, logins menu, live-stream view)
  - File card and preview frame
  - Dialog and sheet, toast, settings row, task card
- **Four screens:** desktop chat with browser panel, desktop agents overview, phone chat, phone agents home.
- **A rendering table.** Every visual effect is mapped to a GPUI primitive and a Jetpack Compose primitive. Effects without both are removed.

**Exit criteria:**
- The owner has picked a direction. The name and Claude Design link are recorded here.
- The owner has uploaded it with `/design-sync`.
- The Aegis tokens, including every motion token, are committed as `tjommie-rs/design/themes/aegis.json` with semantic keys. Any later theme file must have the same key set, and a test checks this.
- The desktop and Compose themes are generated from that file, or a test checks them against it.
- No theme switcher or theme engine is built now.
- Every UI phase builds only the chosen system.

**Decision (owner, 2026-11-09):**
- Direction C **Aegis** (`design/claude-design/aegis/`: README, `tokens.json`/`tokens.css`, `components/`, `screens/`, `shots/`) is the first UI.
- Its motion spec is `components/motion.html`, the live prototype screens and the `motion` tokens (durations, easings, springs, staggers).
- Claude Design link: _added after the owner's `/design-sync` upload_.

### Phase 1: Rust daemon inside Electron (release R1)
**Scope:**
- Everything in §3.1–§3.9 for the daemon:
  - All methods and events, plus `browser.stream`
  - The MCP routes and the broker routes
  - Providers, scheduler, rooms and exchanges, projects, tasks and worktrees, review and checks
  - Attachments, push, LAN and relay
  - Native Win32
- TypeScript changes:
  - `desktop/src/main.ts:292-300` spawns `resources/tjommie-daemon.exe` instead of the Node daemon. `TJOMMIE_DAEMON_EXE` can override this in unpackaged runs and tests.
  - The Electron host feeds `browser.stream`.
  - `packages/ui` draws the stream.

**First task:** classify each of the 30 real tests and the desktop/daemon process tests as black-box (talks to the daemon over WebSocket, HTTP or stdin) or white-box. Black-box tests get the `TJOMMIE_DAEMON_EXE` switch. `tjommie-rs/test-map.md` maps every `packages/daemon/src/*.test.ts` file to a Rust test or to "n/a: reason".

**Exit criteria:**
- The contract test and the Rust unit tests are green.
- Black-box real tests pass against the Rust binary on a GitHub runner and on the owner's PC, with a dev root.
- The §3.8 cross-implementation tests pass.
- The §3.9 stream acceptance test passes, or the owner has made the decision described there.
- **Fixture database test:**
  - A v0.4.2 database built with the TypeScript `openDb` and seeded as in `database-upgrade.test.ts` is committed as `crates/daemon/tests/fixtures/fixture.db` plus `schema.json`. They're regenerated by `node scripts/make-fixture-db.ts`, and a vitest test fails if `schema.json` drifts from TypeScript. `.gitattributes` keeps the migration SQL byte-exact (no CRLF).
  - The Rust daemon opens it with `user_version = 12` and the same row counts.
  - A fresh Rust database has the same `sqlite_master` SQL as a fresh TypeScript one.
- Every §6 row passes with the Electron 0.4.x UI and the Capacitor phone app on a dev root, over LAN and relay.

**Release R1:**
- A normal `v*` release with the production identity. The Rust daemon opens `%LOCALAPPDATA%\Tjommie` in place: same schema, same Credential Manager namespace.
- Before the first open, it backs up the database with the SQLite backup API to `data\backup\tjommie-<version>.db`.
- **Rollback** means reinstalling 0.4.2 from GitHub Releases. To keep that possible:
  - No migration 13 or later until one release after R1.
  - Secrets stay byte-compatible.
- **Exit:** the owner's PC and S25 run R1 for 7 days with no data loss.

### Phase 2: GPUI spike and owner gate (time-boxed to 2 weeks, on the owner's PC)
Pin `gpui` and `gpui-component` to exact git commits. Pass criteria, all measured and reported:
1. A window with a sidebar and a virtualized list of 10,000 messages. While scrolling, p95 frame time is ≤ 16.7 ms.
2. **Per-bot browser.** Two bots, each with its own profile folder, shown in the browser panel. Try engines in this order and use the first that passes:
   - (a) WebView2 child window, with a snapshot/shield while an overlay is open (as `native-browser.ts:719` does today)
   - (b) WebView2 composition hosting
   - (c) Chromium through CDP screencast into a GPUI image
3. GPUI menus, popovers and dialogs draw correctly over the browser area.
4. Input forwarding works: click, drag, wheel, typing, shortcuts and clipboard. Take control and return to the bot both work.
5. Page alignment is correct at 100%, 125% and 150% Windows scaling and at app zoom (CHANGELOG 0.4.2).
6. **CDP on the same instance:**
   - The accessibility snapshot works for `browser_snapshot`.
   - The §3.9 stream target is met from this engine.
   - Private mode stops capture.
7. No console window appears.
8. **Aegis motion.** Three motions are reproduced with the Aegis motion tokens:
   - the browser panel opening while the chat column resizes
   - a FLIP reorder of the sidebar recents
   - the approval card resolving into a record while the content below slides up

   Each must meet all of these:
   - p95 frame time ≤ 16.7 ms during the motion.
   - Correct interruption mid-flight: it continues from the current value, with no jump.
   - Nothing animates on keyboard actions.
   - Reduced motion is honoured (Windows "Animation effects" off makes it instant).

**Baseline:** measure the installed production Electron app (0.4.2 or R1; record which) on the same PC for cold start to first interactive frame, idle working set of all Tjommie processes, and installer size. Phase 3 targets are "no worse than baseline".

**Owner gate at the end of the spike.** The owner chooses based on the results:

| Option | Gains | Costs |
|---|---|---|
| **GPUI** (if it passes) | One Rust codebase for the desktop UI, no Node or Chromium shell | Pre-1.0 libraries pinned to commits; the browser engine is the riskiest part |
| **Electron shell, restyled to the chosen design** | Reuses the working `WebContentsView` browser, pdfjs and the whole tested `packages/ui`; lowest risk; the design can use anything CSS draws | Keeps Electron's size and memory and a Node main process; the desktop UI stays TypeScript |
| **Tauri 2** (Rust shell with WebView2, reusing `packages/ui`) | Rust host, smaller install | Per-bot browsers need multi-webview (marked unstable in Tauri 2) and WebView2 CDP interop; all Electron-specific code must be rewritten (`WebContentsView`, the `debugger` API, `session.fromPath` profiles, the shield view) |

The decision is recorded here before Phase 3 starts.

### Phase 3: desktop UI (gate outcome, chosen design)
Includes:
- The browser host (broker clients, private mode, stream source)
- CPU speech at the same limits
- PDF, Markdown, code and diffs
- Notifications, tray, start at login
- The §3.10 updater

Built under the dev identity until release.

**Exit criteria:**
- Every desktop (D) row passes on the owner's PC.
- No worse than the baseline.
- **Release:** production appId, GUID, install folder and artifact name, published with `latest.yml` so R1's electron-updater installs it.
- The installer is tested first on a runner with R1 installed.
- Rollback means reinstalling R1. No new migrations in this release.
- **Motion parity with Aegis `components/motion.html`:**
  - Every listed motion uses the token timings.
  - p95 frame time ≤ 16.7 ms on the owner's PC, measured with PresentMon.
  - Every motion is interruptible.
  - Nothing animates on keyboard actions.
  - Reduced motion is honoured (Windows "Animation effects" off).

### Phase 4: Android Compose
- Kotlin and Jetpack Compose with UniFFI over `client` and `transport`. Ships `arm64-v8a` only. minSdk 24, with the §3.11 gates.
- Plugin replacements:

  | Today | Replacement |
  |---|---|
  | Socket | Rust client |
  | SecureStore | Same Keystore alias and prefs |
  | PairingScanner | CameraX with ML Kit |
  | PushRouting | `FirebaseMessagingService` |
  | Speech | On-device `SpeechRecognizer` |
  | Attachment | SAF with range reads |
  | Appearance | Compose theme |

- Live stream view.
- Liveness is reconnect when the app returns or the network changes, plus content-free FCM with the §3.2 push kinds. There is no foreground service.
- **Before cutover:** the last Capacitor release copies the `localStorage` hosts, push kinds and appearance into native SharedPreferences. Compose imports those once. Anything missing means re-pairing, and the CHANGELOG says so.

**Exit criteria:**
- Every phone (M) row passes on the S25 over LAN and relay, with `dev.tjommie.rs`.
- Robolectric tests at sdk 24 and 36 and `gradlew testDebugUnitTest` are green.
- **Motion parity with Aegis `components/motion.html`:**
  - Every listed motion uses the token timings.
  - p95 frame time ≤ 16.7 ms on the S25, measured with `adb shell dumpsys gfxinfo <pkg> framestats`.
  - Every motion is interruptible.
  - Nothing animates on keyboard actions.
  - Reduced motion is honoured (Android "Remove animations").
- **Release:** the APK keeps `dev.tjommie.app` and the release keystore. The next versionCode goes in the map.

## 6. Parity matrix
D = desktop, M = phone. "Auto" means a Rust, TypeScript black-box or Gradle test. "Owner" means a manual check on the owner's device.

| # | Feature (source) | Protocol | D | M | Proof |
|---|---|---|---|---|---|
| 1 | Bots: create, edit, pause, delete, notes, avatar | `bots.*`, `bot.*`, `models.list` | ✓ | ✓ | Auto + owner |
| 2 | Chat stream, stop, retry, steer or queue | `chat.*`, `message.*` | ✓ | ✓ | Auto (provider fixtures) |
| 3 | Approvals, autonomy ask/folder/full | `approvals.respond`, `approval.*` | ✓ | ✓ | Auto |
| 4 | Rooms, @mention/@all, exchanges, chain limit | `room.*`, `rooms.*`, `exchanges.get` | ✓ | ✓ | Auto |
| 5 | Routines, run history | `routines.*`, `routine.*` | ✓ | ✓ | Auto |
| 6 | Accounts sign-in, check, limits, Codex sandbox; setup | `accounts.*`, `setup.*` | ✓ | view | Owner (real CLIs) |
| 7 | Usage and spending caps | `usage.*` | ✓ | ✓ | Auto |
| 8 | Project teams: tasks, worktrees, review, checks, integrate (0.4.0) | `projects.*`, `tasks.*`, `project.*`, `task.*` | ✓ | ✓ | Auto + owner |
| 9 | Per-bot browser, tabs, DuckDuckGo default, zoom alignment (0.4.2), 26 MCP tools | `browser.*`, `/mcp/browser` | ✓ | — | Auto + owner |
| 10 | View, take control of and return a bot's browser from a paired device (0.4.0), plus the live stream (§3.9) | `browser.frame/input/control/stream` | ✓ | ✓ | Auto (stream test) + owner |
| 11 | Saved logins: exact-site confirm, private fill, `request_login`, menu (0.4.0, 0.4.2) | `vault.*` | ✓ | — | Auto + owner |
| 12 | Windows worker isolation for bots and checks (0.4.0) | — | ✓ | — | Auto (§3.8 harness) |
| 13 | Files: Markdown, images, PDF range, audio, video, companions, 200 MiB, save (0.4.0) | `attachments.*` | ✓ | ✓ | Auto + owner |
| 14 | Dictation into an editable draft (0.4.0) | local only | ✓ | ✓ (API ≥ 33) | Owner |
| 15 | Pairing, devices, LAN, relay, another PC, host switcher (0.3.0) | `pairing.*`, `devices.*`, `device.*`, `remote.*` | ✓ | ✓ | Auto + owner |
| 16 | Push kinds with per-kind toggles, room deep link, approval bypasses hourly limit (0.3.2) | `notify` | — | ✓ | Owner |
| 17 | PC notices for done and failed | `notifications.*`, `notify` | ✓ | — | Auto |
| 18 | Settings sections, text size and density per device, diagnostics report (0.4.0) | `settings.*`, `diagnostics.get` | ✓ | ✓ | Auto + owner |
| 19 | Start at login, tray, auto-update (0.3.0), clean quit (0.4.0) | — | ✓ | — | Owner + `updater.real` |
| 20 | Collaboration tools | `/mcp` | ✓ | — | Auto |
| 21 | Google: port only the "not enabled" responses (disabled in production) | `google.*` | — | — | Auto |
| 22 | Appearance: Aegis light and dark, follow system, motion, reduced motion | `settings.*` (per device) | ✓ | ✓ | Auto (reduced-motion tests) + owner |

## 7. Test and CI strategy
- **`ci.yml` gets a `rust` job** (windows-latest), in `tjommie-rs`:
  - `cargo fmt --check`
  - `cargo clippy --locked -- -D warnings`
  - `cargo test --locked`
  - `node scripts/protocol-manifest.ts --check`
  - The §3.8 cross-implementation tests (the runner is disposable and has admin)
  - The stream loopback test
  - Toolchain pinned in `tjommie-rs/rust-toolchain.toml`.
- **`ci.yml` gets an `android-rs` job** from Phase 4: cargo-ndk for `arm64-v8a`, then `gradlew testDebugUnitTest assembleDebug`, with Robolectric at sdk 24 and 36.
- **`release.yml`:**
  - From R1, the windows job builds `tjommie-daemon.exe` before `npm run package`.
  - From Phase 3 and Phase 4, the Rust/Gradle jobs replace the Electron and Capacitor jobs. They keep verify, `apply-native --check`, the CHANGELOG notes, the `latest.yml` sha512 checks and the APK signer checks.
- **Dev builds:** `rust-dev.yml` runs on pushes to `rust-rewrite`. It produces dev-identity installers and APKs as workflow artifacts only.
- **Provider tests:** use recorded stream-json and app-server fixtures. Paid real runs are owner-triggered only, like today's `*.real.test.ts`.

## 8. Measurable targets
- All §6 rows pass.
- Stream on LAN: median ≥ 55 fps, p95 gap ≤ 34 ms.
- Spike: p95 frame time ≤ 16.7 ms.
- Desktop UI is no worse than the R1 baseline (cold start, idle working set, installer size).
- Zero console windows.
- Aegis motions: p95 frame time ≤ 16.7 ms on the owner's PC and S25.
- Reduced motion makes every motion instant.
- `user_version = 12` and row counts are preserved through import, R1 and every cutover.

## 9. Cut or deferred (owner decision)
- GPU speech-to-text (CUDA, Vulkan, DirectML, Parakeet, VAD). CPU whisper gives parity.
- Android foreground services (dropped after the S25 spike).
- The Android x86_64 ABI.
- An in-process daemon.
- Google OAuth and the Gmail tools (not shipped today).
- `EncryptedSharedPreferences` (reuse the existing Keystore scheme).
- Themes Workbench and Mission Control, and a theme switcher, come after the Aegis UI ships.
- The Playwright browser fallback (`daemon/src/browser.ts`, `browser-manager.ts`) is not ported. Production always uses the desktop-host browser (`daemon.ts:187-192`).

## 10. Owner decisions (resolved 2026-11-09)
1. **Bot colour amber** keeps its light value (`#E2A72E`, 1.9:1 on paper) as an accepted exception, so it is never mistaken for the ochre Needs-you accent. An amber avatar always appears with the bot's name.
2. **The stone-shield mark** proposed in `aegis/README.md` is kept.
3. **The app name shown in the UI is Tjommie.** The Aegis design's "Aegis Orchestrator" wordmark becomes "Tjommie" in the same display face. Install identity is unchanged: appId, applicationId, data root and exe (§3.10, §3.11).
