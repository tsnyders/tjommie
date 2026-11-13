# Test map: TypeScript tests to Rust tests (T0.4)

Plan: `docs/superpowers/plans/2026-11-09-tjommie-rs-phase1-daemon.md`. Rust paths are planned; milestones and tasks come from that plan.

Note: the TypeScript tests this file maps from now live on the `legacy` branch. Run the `git ls-files` commands below there to list them.

Commands used (from the repo root):
```
git ls-files ':(glob)packages/daemon/src/**/*.test.ts'   # 99 files
git ls-files '*.real.test.ts'                            # 30 files
git ls-files packages/desktop/src/daemon-process.test.ts # 1 file
```
The plain pathspec `'packages/daemon/src/**/*.test.ts'` matches only 9 files, so use the `:(glob)` form.

Counts: **99 daemon tests** (92 mapped, 7 n/a) and **31 real-harness files** (30 `*.real.test.ts` plus `daemon-process.test.ts`): 12 black-box (7 run against the Rust exe, 5 never start a daemon) and 19 white-box.

## 1. Daemon tests (99)

The 13 `*.real.test.ts` files in this table also appear in section 2. Rust paths are under `tjommie-rs/crates/daemon/tests/`.

| TypeScript file | Milestone / task | Planned Rust test | Note |
|---|---|---|---|
| `packages/daemon/src/account-credentials.test.ts` | M2 T2.1 | `tjommie-rs/crates/daemon/tests/cred_roundtrip.rs` |  |
| `packages/daemon/src/account-handlers.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/account_handlers.rs` |  |
| `packages/daemon/src/account-pricing.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/account_pricing.rs` |  |
| `packages/daemon/src/account-quota.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/account_quota.rs` |  |
| `packages/daemon/src/account-runtime.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/account_runtime.rs` |  |
| `packages/daemon/src/account-scrub.real.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/real_account_scrub.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/accounts-integration.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/accounts_integration.rs` |  |
| `packages/daemon/src/accounts-store.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/accounts_store.rs` |  |
| `packages/daemon/src/approvals.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/approvals.rs` |  |
| `packages/daemon/src/attachments.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/attachments.rs` |  |
| `packages/daemon/src/bots.test.ts` | M3 T3.2 | `tjommie-rs/crates/daemon/tests/bots.rs` |  |
| `packages/daemon/src/browser-embedded.real.test.ts` | n/a | n/a | drives BrowserManager (the Playwright fallback), which is not ported; see section 2 |
| `packages/daemon/src/browser-mcp.test.ts` | n/a | n/a | Playwright fallback (browser.ts, browser-manager.ts, browser-mcp.ts) is not ported; without a desktop host, browser tools answer `unavailable` (M6) |
| `packages/daemon/src/browser.test.ts` | n/a | n/a | Playwright fallback (browser.ts, browser-manager.ts, browser-mcp.ts) is not ported; without a desktop host, browser tools answer `unavailable` (M6) |
| `packages/daemon/src/chat.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/chat.rs` |  |
| `packages/daemon/src/cli-console.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/cli_console.rs` |  |
| `packages/daemon/src/cli-worker.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/cli_worker.rs` |  |
| `packages/daemon/src/cli.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/cli.rs` |  |
| `packages/daemon/src/client.test.ts` | M3 T3.1 | `tjommie-rs/crates/daemon/tests/server_client.rs` | imports only server.ts; becomes a WebSocket client test against the Rust server |
| `packages/daemon/src/daemon.real.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/real_daemon.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/daemon.test.ts` | M3 T3.1 | `tjommie-rs/crates/daemon/tests/daemon_wiring.rs` | start-up wiring; slices land with T3.2 and T3.5 |
| `packages/daemon/src/database-upgrade.test.ts` | M1 T1.2 | `tjommie-rs/crates/daemon/tests/db_fixture.rs` | fixture.db and schema.json from make-fixture-db.ts. The runner-recovery part of the first case is ported with the runner (M3 T3.5) |
| `packages/daemon/src/db.test.ts` | M1 T1.2 | `tjommie-rs/crates/daemon/tests/db.rs` |  |
| `packages/daemon/src/devices-store.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/devices_store.rs` |  |
| `packages/daemon/src/diagnostics.test.ts` | M3 T3.2 | `tjommie-rs/crates/daemon/tests/diagnostics.rs` |  |
| `packages/daemon/src/google-auth.test.ts` | n/a | n/a | Rust google.* only returns today's "not enabled" answer (T3.2, daemon.ts:171-172); covered by a case in settings.rs |
| `packages/daemon/src/google-tools.test.ts` | n/a | n/a | Rust google.* only returns today's "not enabled" answer (T3.2, daemon.ts:171-172); covered by a case in settings.rs |
| `packages/daemon/src/google-vault.test.ts` | n/a | n/a | Rust google.* only returns today's "not enabled" answer (T3.2, daemon.ts:171-172); covered by a case in settings.rs |
| `packages/daemon/src/main.test.ts` | M3 T3.1 | `tjommie-rs/crates/daemon/tests/main.rs` |  |
| `packages/daemon/src/mcp.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/mcp.rs` |  |
| `packages/daemon/src/native-browser-broker.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/native_browser_broker.rs` |  |
| `packages/daemon/src/native-browser-handlers.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/native_browser_handlers.rs` |  |
| `packages/daemon/src/native-browser-mcp.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/native_browser_mcp.rs` |  |
| `packages/daemon/src/paths.test.ts` | M1 T1.1 | `tjommie-rs/crates/daemon/tests/paths.rs` |  |
| `packages/daemon/src/peer-wire.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/peer_wire.rs` |  |
| `packages/daemon/src/project-checks-stop.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_checks_stop.rs` |  |
| `packages/daemon/src/project-checks.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_checks.rs` |  |
| `packages/daemon/src/project-handlers.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_handlers.rs` |  |
| `packages/daemon/src/project-review-account.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_review_account.rs` |  |
| `packages/daemon/src/project-review-cli.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_review_cli.rs` |  |
| `packages/daemon/src/project-review-migration.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_review_migration.rs` |  |
| `packages/daemon/src/project-review-policy.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_review_policy.rs` |  |
| `packages/daemon/src/project-review-runtime.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_review_runtime.rs` |  |
| `packages/daemon/src/project-review.real.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/real_project_review.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/project-review.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/project_review.rs` |  |
| `packages/daemon/src/projects.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/projects.rs` |  |
| `packages/daemon/src/providers/claude-parse.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/providers_claude_parse.rs` | same recorded stream fixtures |
| `packages/daemon/src/providers/claude.real.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/real_providers_claude.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/providers/claude.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/providers_claude.rs` | same recorded stream fixtures |
| `packages/daemon/src/providers/codex-parse.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/providers_codex_parse.rs` | same recorded stream fixtures |
| `packages/daemon/src/providers/codex-project-config.real.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/real_providers_codex_project_config.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/providers/codex-rpc.real.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/real_providers_codex_rpc.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/providers/codex-rpc.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/providers_codex_rpc.rs` | same recorded stream fixtures |
| `packages/daemon/src/providers/codex.real.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/real_providers_codex.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/providers/codex.test.ts` | M3 T3.3 | `tjommie-rs/crates/daemon/tests/providers_codex.rs` | same recorded stream fixtures |
| `packages/daemon/src/push.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/push.rs` |  |
| `packages/daemon/src/queue.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/queue.rs` |  |
| `packages/daemon/src/relay-client.real.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/real_relay_client.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/relay-client.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/relay_client.rs` |  |
| `packages/daemon/src/relay-identity.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/relay_identity.rs` |  |
| `packages/daemon/src/remote-host.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/remote_host.rs` |  |
| `packages/daemon/src/remote-identity.test.ts` | M7 | `tjommie-rs/crates/daemon/tests/remote_identity.rs` |  |
| `packages/daemon/src/rooms.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/rooms.rs` |  |
| `packages/daemon/src/routines.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/routines.rs` |  |
| `packages/daemon/src/runner-codex-stop.real.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/real_runner_codex_stop.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/runner.real.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/real_runner.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/runner.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/runner.rs` |  |
| `packages/daemon/src/scheduler.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/scheduler.rs` |  |
| `packages/daemon/src/server.test.ts` | M3 T3.1 | `tjommie-rs/crates/daemon/tests/server.rs` |  |
| `packages/daemon/src/settings.test.ts` | M3 T3.2 | `tjommie-rs/crates/daemon/tests/settings.rs` |  |
| `packages/daemon/src/setup.real.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/real_setup.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/setup.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/setup.rs` |  |
| `packages/daemon/src/task-continuation.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/task_continuation.rs` |  |
| `packages/daemon/src/task-integration.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/task_integration.rs` |  |
| `packages/daemon/src/tasks.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/tasks.rs` |  |
| `packages/daemon/src/team.atomicity.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team_atomicity.rs` |  |
| `packages/daemon/src/team.attachments.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team_attachments.rs` |  |
| `packages/daemon/src/team.deletion.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team_deletion.rs` |  |
| `packages/daemon/src/team.paths.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team_paths.rs` |  |
| `packages/daemon/src/team.routing.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team_routing.rs` |  |
| `packages/daemon/src/team.snapshots.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team_snapshots.rs` |  |
| `packages/daemon/src/team.test.ts` | M4 | `tjommie-rs/crates/daemon/tests/team.rs` |  |
| `packages/daemon/src/terminal-chat.test.ts` | — | n/a: stays TypeScript | tests the `scripts/chat.ts` dev tool on `@tjommie/client`, not the daemon; runs unchanged against the Rust daemon |
| `packages/daemon/src/testing.test.ts` | n/a | n/a | tests the TypeScript helper testing.ts (tempRoot, realAccount, waitFor); Rust has its own test support |
| `packages/daemon/src/turn-notices.test.ts` | M3 T3.5 | `tjommie-rs/crates/daemon/tests/turn_notices.rs` |  |
| `packages/daemon/src/usage.test.ts` | M3 T3.4 | `tjommie-rs/crates/daemon/tests/usage.rs` |  |
| `packages/daemon/src/vault-handlers.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/vault_handlers.rs` |  |
| `packages/daemon/src/vault-host-broker.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/vault_host_broker.rs` |  |
| `packages/daemon/src/windows-acl.driver.test.ts` | M2 T2.2 | `tjommie-rs/crates/daemon/tests/windows_acl_driver.rs` |  |
| `packages/daemon/src/windows-acl.real.test.ts` | M2 T2.4 | `tjommie-rs/crates/daemon/tests/real_windows_acl.rs` | real test, `#[ignore]` by default; see section 2 |
| `packages/daemon/src/windows-acl.test.ts` | M2 T2.2 | `tjommie-rs/crates/daemon/tests/windows_acl.rs` |  |
| `packages/daemon/src/windows-worker-jobs.test.ts` | M2 T2.3 | `tjommie-rs/crates/daemon/tests/windows_worker_jobs.rs` | Win32 parts gated by TJOMMIE_RS_ADMIN_TESTS=1 |
| `packages/daemon/src/windows-worker-launch.test.ts` | M2 T2.3 | `tjommie-rs/crates/daemon/tests/windows_worker_launch.rs` | Win32 parts gated by TJOMMIE_RS_ADMIN_TESTS=1 |
| `packages/daemon/src/windows-worker-pair.test.ts` | M2 T2.3 | `tjommie-rs/crates/daemon/tests/windows_worker_pair.rs` | Win32 parts gated by TJOMMIE_RS_ADMIN_TESTS=1 |
| `packages/daemon/src/windows-worker-preparation.test.ts` | M2 T2.3 | `tjommie-rs/crates/daemon/tests/windows_worker_preparation.rs` | Win32 parts gated by TJOMMIE_RS_ADMIN_TESTS=1 |
| `packages/daemon/src/windows-worker-runtime.test.ts` | M2 T2.3 | `tjommie-rs/crates/daemon/tests/windows_worker_runtime.rs` | Win32 parts gated by TJOMMIE_RS_ADMIN_TESTS=1 |
| `packages/daemon/src/windows-worker.test.ts` | M2 T2.3 | `tjommie-rs/crates/daemon/tests/windows_worker.rs` | Win32 parts gated by TJOMMIE_RS_ADMIN_TESTS=1 |
| `packages/daemon/src/worker-boundary-host.test.ts` | M6 | `tjommie-rs/crates/daemon/tests/worker_boundary_host.rs` |  |
| `packages/daemon/src/worktrees.test.ts` | M5 | `tjommie-rs/crates/daemon/tests/worktrees.rs` |  |

## 2. Real tests (31)

"Runs against exe" means it can run with `TJOMMIE_DAEMON_EXE` pointing at the Rust daemon (after T9.1). Black-box here means the file imports no daemon internals. Rows marked black-box with "no" never start a daemon at all.

| File | Class | Runs against exe | Reason |
|---|---|---|---|
| `packages/daemon/src/account-scrub.real.test.ts` | **white-box** | no | imports cli.ts and testing.ts; spawns the Claude CLI itself and never starts the daemon |
| `packages/daemon/src/browser-embedded.real.test.ts` | **white-box** | no | imports browser-handlers, BrowserManager, BrowserMcp, McpTokens and startServer; tests the Playwright fallback, which is dropped |
| `packages/daemon/src/daemon.real.test.ts` | **white-box** | no | calls startDaemon in-process and reads the database with openDb, though it also uses the WebSocket client |
| `packages/daemon/src/project-review.real.test.ts` | **white-box** | no | imports createProjectReviewRuntime, TaskIntegrations and project-test-support |
| `packages/daemon/src/providers/claude.real.test.ts` | **white-box** | no | imports ClaudeSession, Runner, bots, db and setup |
| `packages/daemon/src/providers/codex-project-config.real.test.ts` | **white-box** | no | imports codexPolicy and the spike script, which imports cli, setup and codex-rpc |
| `packages/daemon/src/providers/codex-rpc.real.test.ts` | **white-box** | no | imports startCodexServer and cli.ts |
| `packages/daemon/src/providers/codex.real.test.ts` | **white-box** | no | imports CodexSession, browser.ts and setup |
| `packages/daemon/src/relay-client.real.test.ts` | **white-box** | no | builds RemoteHost, RelayClient, relay identity and db in-process |
| `packages/daemon/src/runner-codex-stop.real.test.ts` | **white-box** | no | imports Runner, queue, bots, db and JsonRpcLines |
| `packages/daemon/src/runner.real.test.ts` | **white-box** | no | imports Runner, McpTokens, queue, bots and db |
| `packages/daemon/src/setup.real.test.ts` | **white-box** | no | imports checkSetup, ensureAccounts, listModels and db |
| `packages/daemon/src/windows-acl.real.test.ts` | **white-box** | no | calls startDaemon in-process and windowsAclDriver / protectCodexDataAsync |
| `packages/desktop/src/accounts.real.test.ts` | **white-box** | no | seeds usage with openDb and recordUsage from daemon src; the rest goes through launchRealApp (black-box if seeding moved to an RPC or fixture file) |
| `packages/desktop/src/android-companion.real.test.ts` | **white-box** | no | hosts RemoteHost in-process with db, chat, approvals and usage; only the phone side is a browser |
| `packages/desktop/src/desktop.real.test.ts` | **black-box** | yes | only launchRealApp and the app's WebSocket client; no daemon imports |
| `packages/desktop/src/embedded-browser.real.test.ts` | **black-box** | yes | only launchRealApp, Electron evaluate and the client; no daemon imports |
| `packages/desktop/src/mobile-appearance.real.test.ts` | **black-box** | yes | only launchRealApp and the page; no daemon imports |
| `packages/desktop/src/mobile-motion.real.test.ts` | **black-box** | yes | only launchRealApp and the page; no daemon imports |
| `packages/desktop/src/native-browser.real.test.ts` | **black-box** | no | launches its own Electron fixture (native-browser-fixture.ts) and never starts a daemon, so TJOMMIE_DAEMON_EXE has no effect |
| `packages/desktop/src/packaged.real.test.ts` | **black-box** | yes | launches the packaged exe and uses the client; T9.1 replaced its Playwright MCP assertion with one that `resources/daemon/tjommie-daemon.exe` ships and the Node daemon, Playwright MCP and daemon wasm don't |
| `packages/desktop/src/phone-layout.real.test.ts` | **white-box** | no | seeds with addMessage, openDb, recordUsage and replaceAccountLimits from daemon src |
| `packages/desktop/src/remote-bridge.real.test.ts` | **black-box** | no | bundles the remote bridge into an Electron CSP fixture; no daemon involved |
| `packages/desktop/src/screens.real.test.ts` | **black-box** | yes | only launchRealApp, bot helpers and the client; real accounts and paid turns |
| `packages/desktop/src/two-hosts.real.test.ts` | **white-box** | no | starts two daemons with startDaemon in-process plus a local relay |
| `packages/desktop/src/ui-bughunt.real.test.ts` | **white-box** | no | seeds a device with addDevice and openDb from daemon src |
| `packages/desktop/src/updater.real.test.ts` | **white-box** | no | imports tempRoot and windowsAclDriver from daemon src; it also matches the daemon's "listening on" log line, which the Rust daemon must print the same way |
| `packages/relay/src/relay.real.test.ts` | **black-box** | no | talks to the local relay over WebSocket; imports only protocol and the relay helper, no daemon |
| `packages/transport/src/noise.real.test.ts` | **black-box** | no | tests the Noise transport in Chromium and Electron; no daemon |
| `packages/ui/visual/provider-usage.real.test.ts` | **black-box** | no | renders a React component in Chromium; no daemon |
| `packages/desktop/src/daemon-process.test.ts` | **black-box** | yes | spawns the daemon, passes the token on stdin and connects over WebSocket; T9.1 swaps the entry for the Rust exe |
