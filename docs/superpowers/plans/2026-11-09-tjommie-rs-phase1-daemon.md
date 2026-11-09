# Phase 1: Rust daemon inside Electron (release R1)

- **Status:** draft for owner approval, 2026-11-09.
- **Spec:** `docs/superpowers/specs/2026-11-09-tjommie-rust-rewrite-spec.md`, mainly §2, §3, §4, §5 Phase 1 and §6.
- **Branch:** `rust-rewrite`. Baseline v0.4.2 (`aa20216`).
- **Goal:** `tjommie-daemon.exe` replaces the Node daemon inside the Electron app. It is behaviourally identical to 0.4.2, plus native Win32 and `browser.stream`. It ships as R1 (0.5.0) with the production identity.

## Rules for every task
- **The code wins.** Read the spec section and the cited TypeScript files before writing. If they disagree, follow the code and report it so the spec gets fixed.
- **Tests first.** Write the named test, run it and see it **fail**, then implement and see it pass. Port the TypeScript test cases one for one, and use the same fixture data.
- **Where to run commands:** in PowerShell.
  - Rust: run cargo from inside `tjommie-rs/` so `rust-toolchain.toml` applies: `cargo test -p <crate> <filter>`. Run from the repo root, the pin is ignored.
  - TypeScript, from the repo root: `npx vitest run <file>`
  - Before handing in:
    - inside `tjommie-rs/`: `cargo clippy --workspace --all-targets --locked -- -D warnings`
    - from the root: `npm run typecheck`
  - On this shared PC, run focused tests while working and the full vitest suite once at the end. Failures that pass in isolation are load, not regressions; report them as such.
- **Schema checks** use `protocol::assert_valid` (in the library). Use `Request::decode` for calls and `protocol::parse::<T>()` for frames (see the M0 notes in the Core report).
- **No new migrations, and secrets stay byte-compatible.** Rollback to 0.4.2 must keep working (spec §5).
- **The production root is never touched.** Tests use temp roots. Anything that creates Windows accounts, ACLs or jobs needs `TJOMMIE_RS_ADMIN_TESTS=1`, refuses a root that resolves to `%LOCALAPPDATA%\Tjommie`, and runs first on a GitHub runner.
- **No console windows.** Spawn only through `spawn::command()`. Clippy `disallowed-methods` bans `std::process::Command::new` and `tokio::process::Command::new` everywhere else.
- **Git:** each lane works in its own git worktree from `rust-rewrite`. The ops agent merges lanes at the end of each milestone. Implementation agents do not push or tag.
- **Tags:** every task is backend unless it says **UI**.
- **CHANGELOG:** only tasks that say so add a line under `## Unreleased`.

## Lanes (what runs in parallel)

| Lane | Tasks, in order | Starts after |
|---|---|---|
| Core | M0 → T3.1 → T3.2 → T3.5 | — |
| A | M1 → M4 | T0.3 |
| B | M2 → M5 | T0.3 (M5 also needs T3.5) |
| C | T3.3 → T3.4 → M6 | T0.3 (T3.4 also needs T2.1; M6 also needs T3.1) |
| D | M7 | T3.1 and T2.1 |
| Stream | M8 | M6 and M7 |
| Release | M9 | everything |

That is seven lanes at most, matching the agent limit.

## M0 Foundation (sequential, Core)

**T0.1 Workspace and CI.**
- **Files:**
  - `tjommie-rs/Cargo.toml` (workspace, edition 2024, crates `protocol`, `transport`, `daemon`)
  - `tjommie-rs/rust-toolchain.toml`
  - `tjommie-rs/clippy.toml` (disallowed-methods)
  - `tjommie-rs/crates/daemon/src/spawn.rs`
  - `.github/workflows/ci.yml` (new `rust` job on windows-latest: fmt, clippy, test, all `--locked`)
- **Test first:** `spawn.rs` unit test. `command("x")` sets creation flags `0x08000000`; keep the flag in the wrapper so the test can read it back.
- **Done when:**
  - Clippy fails on a deliberate `Command::new` and passes without it.
  - The CI `rust` job is green.

**T0.2 Protocol manifest.**
- **Files:**
  - `scripts/protocol-manifest.ts` and `scripts/protocol-manifest.test.ts`
  - `tjommie-rs/protocol/manifest.json`
  - Root `package.json` script `protocol:manifest`
- **What it does:** for every entry in `methods` and `events` (`protocol/src/wire.ts:50-128`), it writes `{name, params, result | data}` as JSON Schema via `z.toJSONSchema`. Where zod cannot export a schema, it writes `custom`.
- **Test first:** the vitest test checks two things. The manifest names equal `Object.keys(methods)` (101) and `Object.keys(events)` (26). `--check` exits non-zero when the manifest is stale.
- **Done when:** `ci.yml`'s Node job runs `node scripts/protocol-manifest.ts --check`.

**T0.3 Protocol crate.**
- **Files:** `tjommie-rs/crates/protocol/src/*.rs`, one module per `packages/protocol/src/*.ts` file; `tests/contract.rs`.
- **Test first:** `contract.rs` checks that `METHODS` and `EVENTS` equal the manifest names. It also exposes `assert_valid(method_or_event, json)`, using the `jsonschema` crate, for all later integration tests.
- **Done when:** `cargo test -p protocol` is green, including serde round-trips of the TypeScript protocol test fixtures.

**T0.4 Test map** (can run in parallel with T0.2 and T0.3).
- **File:** `tjommie-rs/test-map.md`.
- **Content:**
  - All 99 `packages/daemon/src/**/*.test.ts` files, each mapped to a Rust test path or to "n/a: reason".
  - The 30 `*.real.test.ts` files plus `desktop/src/daemon-process.test.ts`, each marked **black-box** (talks to the daemon over WebSocket, HTTP or stdin) or white-box.
- **Done when:** both counts match `git ls-files`.

## M1 Database, paths, import (Lane A)

**T1.1 Paths.**
- **Port:** `paths.ts` to `daemon/src/paths.rs`:
  - root resolution (`TJOMMIE_HOME`, `:10`)
  - folders (`:12-23`)
  - `migrateLegacyDb` (`:30-48`)
  - dev token (`:51-65`)
- **Test:** port `paths.test.ts`.

**T1.2 Database.**
- **Files:**
  - `daemon/src/db.rs`
  - `tjommie-rs/crates/daemon/migrations/01.sql` … `12.sql`
  - `scripts/export-migrations.ts`, which writes the `.sql` files from `db.ts` `migrations`
  - `scripts/make-fixture-db.ts`, which seeds a v0.4.2 database the way `database-upgrade.test.ts` does and writes `fixture.db` plus `schema.json` (the `sqlite_master` SQL of a fresh TypeScript database)
- **Tests first:**
  1. A vitest check that the `.sql` files equal `migrations[i]` byte for byte.
  2. Rust `tests/db_fixture.rs`: opening `fixture.db` gives `user_version = 12` and the same row counts.
  3. A fresh Rust database's `sqlite_master` SQL equals `schema.json`.
- **Must match:** PRAGMAs and the per-migration transaction (`db.ts:254-267`), including the unversioned index at `:265`. Use rusqlite with `bundled`.

**T1.3 First-open backup.**
- **What:** before the first open of each new daemon version, copy the database with the SQLite backup API to `data\backup\tjommie-<version>.db`, once per version.
- **Test:** open twice; there is exactly one backup, and it is byte-readable with the same row counts.

**T1.4 `tjommie-daemon import --from <root>`.** Needs T2.1. Behaviour is in spec §4.
- **Tests:**
  - It refuses an existing target unless `--replace` is given.
  - A sha256 of the source tree is the same before and after.
  - `devices`, `remote_host` and `pairing_codes` are empty afterwards.
  - No `.credentials.json` is copied.
  - The `account/` and `login/` secrets exist under the new namespace.

## M2 Secrets and native Win32 (Lane B)

**T2.1 Credential Manager.**
- **File:** `daemon/src/win/cred.rs`, using `CredReadW`, `CredWriteW` and `CredDeleteW`.
- **Must match:** type, persist and username (`account-credentials.ts:28-51`), and the target names and namespaces (spec §3.7).
- **Probe:** `tjommie-rs/tools/cred-probe.ts` calls the TypeScript module.
- **Test first:** a cross-implementation test. TypeScript writes and Rust reads, then Rust writes and TypeScript reads. It uses a random test namespace and cleans up.

**T2.2 ACLs.**
- **File:** `daemon/src/win/acl.rs`, a port of `windows-acl.ts`, including the `CodexSandboxUsers` lookups (`:54`, `:98`, `:271`).
- **Tests:**
  - Port `windows-acl.test.ts` and `windows-acl.driver.test.ts`.
  - SDDL equality: a folder prepared by TypeScript (through a probe) and one prepared by Rust give equal SDDL once the folder path is normalised.

**T2.3 Worker accounts, jobs and launch.**
- **File:** `daemon/src/win/worker.rs`, porting:
  - `windows-worker.ts`
  - `windows-worker.ps1`: `CreateJobObject` at `:116`, `:171`; `CreateProcessWithLogonW` at `:237-271`, where `CREATE_NO_WINDOW` is added
  - `scripts/vault/provision-worker.ps1`, keeping today's elevation flow
- **Tests:**
  - Port the `windows-worker*.test.ts` files.
  - Admin tests, gated and CI-runner only: create `TjWorker-<test ns8>`, launch inside `Global\TjommieWorkerJob-…`, check that closing the job kills the tree, and clean up by `ns8`.

**T2.4 Real-harness run on the owner's PC** (owner present).
- **What:** run the `windows-acl` real test and the T2.1–T2.3 admin tests against a dev root (`%LOCALAPPDATA%\Tjommie-rs`).
- **Done when:**
  - Everything is green.
  - `Get-LocalUser TjWorker-*` shows only the production account and the dev `ns8` account.
  - The production root's file count is unchanged.

## M3 Server and chat

**T3.1 Server and launch contract** (Core).
- **Files:** `daemon/src/main.rs`, `server.rs`, using axum.
- **Behaviour:**
  - `127.0.0.1:TJOMMIE_PORT` (default 47100).
  - `/health`.
  - WebSocket hello/welcome with close codes 4003 and 4401 (`server.ts:127-138`).
  - Request/response with `ErrorCode`, including `unknown_method`.
  - Event broadcast.
  - `--token-stdin` reads `{token, desktopHostToken}`. The daemon exits when stdin closes.
  - `--dev`.
  - The LAN port comes from new `TJOMMIE_LAN_PORT` (default 47101).
  - On start it prints exactly `Tjommie daemon listening on http://127.0.0.1:<port>` to stdout (`daemon/src/main.ts:49`). `updater.real` and the desktop's log reading match this text.
- **Desktop override (moved here from T9.1):** in unpackaged runs, `desktop/src/main.ts` starts the exe named by `TJOMMIE_DAEMON_EXE` when that variable is set, so the black-box real tests in `tjommie-rs/test-map.md` can target the Rust daemon from here on. The packaged switch stays in T9.1. Test it in `daemon-process.test.ts`.
- **Tests:** port `server.test.ts` and `main.test.ts`. Every response goes through `assert_valid`.

**T3.2 Settings, bots and small domains** (Core).
- **Covers:**
  - `settings.*`
  - `bots.*`
  - `models.list`
  - `diagnostics.get`
  - `google.*`, returning today's "not enabled" answer (`daemon.ts:171-172`)
- **Tests:** port `settings`, `bots` and `diagnostics`.

**T3.3 Providers** (Lane C).
- **Files:** `daemon/src/providers/{claude,claude_parse,codex_rpc,codex,codex_parse}.rs`. Facts are in spec §3.5.
- **Tests:** port `claude`, `claude-parse`, `codex`, `codex-parse` and `codex-rpc`, using the same recorded stream fixtures.

**T3.4 Accounts, setup, usage** (Lane C).
- **Covers:** `account-*.ts`, `accounts-store.ts`, `setup.ts`, `usage.ts`, `cli.ts`. These are the `accounts.*`, `setup.*` and `usage.*` methods plus quota.
- **Tests:** port the matching tests.

**T3.5 Runner, queue, chat, approvals** (Core, after T3.1–T3.3).
- **Covers:** `runner.ts`, `queue.ts`, `chat.ts`, `approvals.ts`, `turn-notices.ts`, including steer vs queue (`runner.ts:268-282`, `:726-762`).
- **Tests:** port `runner`, `queue`, `chat`, `approvals`, `turn-notices` and `terminal-chat`.

## M4 Collaboration (Lane A, after T3.5)
- **Covers:**
  - `rooms`, `team` (the 5 tools)
  - `/mcp` (`mcp.ts:48`)
  - exchanges, `routines`, `scheduler`
  - `notifications.*`
- **Tests:** port `rooms`, `team.*` (7 files), `mcp`, `routines` and `scheduler`.

## M5 Projects (Lane B, after T3.5 and M2)
- **Covers:**
  - `projects`, `tasks`, `task-execution`, `task-integration`
  - `worktrees`
  - `project-review*`, `project-checks`

  Checks and review runs use the M2 worker isolation.
- **Tests:** port the 14 project, task and worktree test files.

## M6 Attachments and broker routes (Lane C, after T3.1)
- **Covers:**
  - `attachments.*`
  - `/browser-host` (`native-browser-broker.ts`, `native-browser-handlers.ts`)
  - `/mcp/browser` (`native-browser-mcp.ts`, passing the 26 tools through)
  - `/worker-host` (`worker-boundary-host.ts`)
  - `/vault-host` with `vault.*` (`vault-host-broker.ts`, `vault-handlers.ts`)

  Every broker route is authenticated with `desktopHostToken`. The Playwright fallback is not ported: in `--dev` without a desktop host, browser tools answer `unavailable`.
- **Tests:** port the matching tests.
- **Done when:** desktop `embedded-browser.real` and `desktop.real` pass with `TJOMMIE_DAEMON_EXE` pointing at the Rust daemon. Two tests can't prove the Rust daemon, per `tjommie-rs/test-map.md`: `native-browser.real` never starts a daemon, and `accounts.real` imports daemon code to seed its data.

## M7 Remote (Lane D)
- **Covers:**
  - The `transport` crate (port `transport/native/src/lib.rs`)
  - `peer-wire`, `remote-host`, `remote-identity`, `devices-store`
  - `relay-client`, `relay-identity`
  - `push`
  - `pairing.*`, `devices.*`, `device.*`, `remote.*`
- **Tests:**
  - Port the matching tests.
  - Interop: the TypeScript client (`packages/client`) pairs with and talks to the Rust LAN host, and goes through the local relay (`relay/src/local-relay.test-helper.ts`).

## M8 `browser.stream` (after M6 and M7). Spec §3.9

**T8.1 Protocol.**
- **What:** add `browser.stream` and `browser.streamAck` to `packages/protocol/src/browser.ts`, and add the `browser.streamFrame` event.
- **Then:** regenerate the manifest and update the Rust protocol crate.
- **Test first:** `browser.test.ts` cases, plus the contract test.

**T8.2 Electron source.**
- **What:** `desktop/src/native-browser-frame.ts` forwards every screencast frame to the daemon over `/browser-host` while a stream is on.
- **Test:** a broker-level vitest with a fake debugger.

**T8.3 Daemon fan-out.**
- **What:** send frames only to subscribed connections. Keep at most 2 unacknowledged and replace older unsent frames. Private mode stops the stream.
- **Test:** a Rust test with a slow fake client. Memory stays flat, and the frames it gets are always the newest.

**T8.4 UI: draw the stream.**
- **What:** the `packages/ui` remote browser view subscribes when shown and unsubscribes when hidden. It acknowledges each drawn frame and falls back to `browser.frame` polling on `unknown_method`.
- **Test:** a component test using a fake client.

**T8.5 Acceptance.**
- **What:** a fixture page (a `requestAnimationFrame` counter at 1280×800) and a loopback test that measures delivered fps and frame gaps over 60 s.
- **Target:** median ≥ 55 fps, p95 gap ≤ 34 ms. If Electron's screencast cannot reach that, report the numbers so the owner can decide (spec §3.9).
- **CHANGELOG:** "Paired devices can watch a bot's browser live instead of in snapshots."

## M9 Electron switch and release R1 (sequential, after everything)

**T9.1 Switch.**
- **`desktop/src/main.ts:49`, `:292-300`:**
  - Packaged builds run `resources/daemon/tjommie-daemon.exe` with no Node arguments.
  - Unpackaged builds keep the `TJOMMIE_DAEMON_EXE` override from T3.1, and otherwise run the Node daemon.
- **`desktop/scripts/package.ts:74-80`:** copy the cargo-built exe, and stop bundling `main.ts`, Playwright MCP and the wasm.
- **`scripts/verify-packaged.ts`:** expects the exe.
- **`packages/desktop/src/packaged.real.test.ts:20`:** assert `resources/daemon/tjommie-daemon.exe` instead of the Playwright MCP `cli.js`.
- **Tests:** `daemon-process.test.ts`, `verify-packaged` and `packaged.real`.
- **CHANGELOG:** "The background service that runs your bots was rebuilt; bots, chats, settings and logins carry over, and a backup of your data is made the first time it starts."

**T9.2 CI and release workflow.**
- **`release.yml` windows job:** install the pinned toolchain and run `cargo build --release --locked -p tjommie-daemon` before `npm run package`.
- **New `r1-acceptance.yml`:** on push to `r1-acceptance-*`, modelled on `phase4-acceptance.yml`. On a windows runner it:
  1. Installs 0.4.2.
  2. Seeds it.
  3. Installs R1 over it.
  4. Checks the backup, row counts and the production file count against a fresh build.
  5. Reinstalls 0.4.2 over R1 (the rollback) and checks that the database still opens.
  6. Runs every black-box real test from `test-map.md` against the packaged test identity.

**T9.3 Owner parity run** (owner present).
- **What:** the Electron 0.4.x UI, run unpackaged with:
  - `TJOMMIE_HOME=%LOCALAPPDATA%\Tjommie-rs`
  - `TJOMMIE_PORT=47200`
  - `TJOMMIE_LAN_PORT=47201`
  - `TJOMMIE_DAEMON_EXE`
- **Phone:** the S25 Capacitor app paired to that host.
- **Checklist:** walk every spec §6 row over LAN and over the relay, ticking `tjommie-rs/r1-checklist.md`.

**T9.4 Release steps (AGENTS.md):**
1. Bump all nine `package.json` versions to `0.5.0`.
2. Add `["0.5.0", 9]` to the versionCode map (`packages/mobile/scripts/apply-native.mjs:11`).
3. Rename `## Unreleased` to `## 0.5.0 — <date>` and add a fresh empty `## Unreleased`.
4. The ops agent merges to `main` and pushes tag `v0.5.0`. GitHub Actions builds and publishes.
5. Never package locally.

**T9.5 Soak.**
- **What:** the owner's PC and S25 run R1 for 7 days.
- **Done when:** no data loss and the §6 rows still pass. This ends Phase 1.

