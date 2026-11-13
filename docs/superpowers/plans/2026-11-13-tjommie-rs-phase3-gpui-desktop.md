# Phase 3: GPUI desktop UI in the Aegis design (the desktop-UI release)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

- **Status:** draft, 2026-11-13. The owner answered all 8 open questions on 2026-11-13; see §"Owner decisions (resolved 2026-11-13)".
- **Spec:** `docs/superpowers/specs/2026-11-09-tjommie-rust-rewrite-spec.md`. The parts that matter most here are §1, §2, §3.6, §3.9, §3.10, §4, §5 (Phase 0, the Phase 2 decision record, Phase 3), §6 (the D rows), §7, §8 and §10. Executors read the spec and this plan together.
- **Evidence:** the spike report, code and scripts at `C:\Users\tyrel\.paseo\worktrees\2cb7zj88\rs-gpui-spike\tjommie-rs\spikes\gpui\` (branch `rs/gpui-spike`, commit `a1b1f59`). The browser memory research is at `C:\Users\tyrel\tmp\orchestrator\webview2-memory-research.md`. Below, "spike `src/x.rs`" means a file in that folder, and "research #n" means the n-th row of the research's ranked recommendations.
- **Branch:** `rust-rewrite`. Each lane works in its own worktree. The ops agent merges, pushes and tags.
- **Note:** TypeScript references (`packages/daemon/...`) point at the `legacy` branch since R1.

**Goal:** a Rust desktop app, `Tjommie.exe`, built on GPUI in the Aegis design. It replaces the Electron shell with full desktop parity (every D row in §6) and runs R1's Rust daemon unchanged. It installs over R1 through R1's own electron-updater.

**Architecture:** three new crates in the `tjommie-rs` workspace.
- `client`: the typed WebSocket client to a daemon, plus Noise connections to other PCs. Phase 4 reuses it.
- `desktop-host`: everything without GPUI. That is the WebView2 browser engine and its 26 tools, the broker clients, the login vault, speech, the updater, the tray, notifications, the login item, crash handling and the daemon process.
- `desktop`: the GPUI app, with its view models, screens and the motion layer.

The app is the second daemon client (spec §2), exactly like Electron: it serves `/browser-host`, `/worker-host` and `/vault-host` with `desktopHostToken`.

**Tech stack:**
- Rust 1.97.1
- `gpui` (crates.io `gpui-pre =0.3.12`, Zed `d2a4fb6`) and `gpui-kit` (git `39ab849a`, tag v0.9.1)
- `webview2-com =0.39.1` and `windows 0.62`
- tokio and tokio-tungstenite (as in the daemon)
- `pulldown-cmark` for Markdown
- pdfium for PDF, through `pdfium-render`, with a pinned `pdfium.dll`
- `cpal` for microphone capture
- NSIS (`makensis`) for the installer

## Global constraints (every task includes these)
- **Pins:** `gpui-pre =0.3.12` and `gpui-kit` at rev `39ab849a93b5f15507b969164d49891db8d5b470`, exactly as in the spike's `Cargo.toml`. `Cargo.lock` is committed, and every cargo command uses `--locked`. The upgrade policy is in §"Pins and upgrades".
- **No `any` in TypeScript.** No `serde_json::Value` past the wire-decode boundary.
- **No console windows.** Every process is spawned through `spawn::command()`. Move it from `crates/daemon/src/spawn.rs` to a tiny shared crate only if both crates need it; otherwise `desktop-host` keeps its own copy with the same `clippy.toml` ban. That includes `whisper-cli.exe` and the daemon.
- **No new migrations.** Secrets stay byte-compatible (spec §5 Phase 1 rollback rule). The bundled daemon is R1's daemon.
- **The production root is never touched by tests or dev builds.** Every test and dev run uses a temp root or `%LOCALAPPDATA%\Tjommie-rs`. `desktop-host::owner_safety` (a port of `desktop/src/owner-safety.ts`) refuses a root that resolves to `%LOCALAPPDATA%\Tjommie` or `%APPDATA%\Tjommie` unless the build identity is production.
- **No real accounts or paid turns in automated tests.** Provider turns come from the daemon's recorded fixtures. The real-Claude smoke test is owner-run.
- **No `0.0.0.0` binds in tests.** Everything binds `127.0.0.1`.
- **Credential Manager:** tests write only under a random test namespace derived from their temp root, and clean up by that namespace. They never read or write the owner's `Tjommie/<prod ns>/…` entries.
- **Windows settings:** tests never change display scale or Animation effects. The owner-check scripts that do (spike `owner-checks-lib.ps1`) record the originals and restore them in `finally`.
- **No panics at UI boundaries.** See T0.4.
- **Identity:** dev builds use spec §4's dev column (appId `com.tsnyders.tjommie-rs`, `TjommieRS.exe`, root `Tjommie-rs`, ports 47200/47201, no update feed, login item off) until M8.
- **CHANGELOG:** only tasks marked **CHANGELOG** add a line under `## Unreleased`, in plain English, in the same change. Every user-visible change in this phase is one of those tasks.
- **Disk:** check free space before starting a worktree build. GPUI debug builds are large; see the memory note on the build disk budget. At most 3 worktrees build GPUI at once. Reviewers delete their scratch copies.

## Rules for every task (as in Phase 1)
- **The code wins.** Read the cited TypeScript before porting it. If the spec or this plan disagrees with it, follow the code and report the difference.
- **Tests first.** Write the named test, see it fail, implement, then see it pass. Port TypeScript tests one for one, with the same fixture data.
- **Commands** are run in PowerShell from inside `tjommie-rs/`: `cargo test -p <crate> <filter> --locked`. Before handing in: `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo fmt --check`. From the repo root: `npm run typecheck` when TypeScript changed.
- **On this shared PC**, run focused tests while working, and the crate's full suite once at the end. A failure that passes in isolation is load; report it as load.
- Implementation agents do not push, tag or merge. Ops merges each lane at the end of each milestone.

## Review focus
These are the inputs and conditions most likely to hurt the owner that no single feature test covers. Each has a named test in its owning task.
1. **A tool call or a remote stream starts on a bot whose page is suspended or not yet created.** Expected: the page resumes or is created, and is visible (parked) before the first CDP command. No tool times out. Test: `lifecycle::tool_on_suspended_bot_resumes_first` (T3.1).
2. **The daemon restarts while the panel, an overlay or the vault entry is open.** Expected: the shield comes down, every controller of a deleted or browser-off bot closes, the vault locks, and the UI reconnects with the new token. Test: `e2e_daemon_restart_with_panel_open` (T3.2).
3. **The window is hidden to the tray or minimized while a paired device streams a bot.** Expected: the page stays parked, visible and awake. The stream holds a median of at least 55 fps. Test: `stream::hidden_window_keeps_streaming` (T3.4).
4. **The display scale or monitor changes while the app runs.** Expected: the rem scale and the page frame follow `WM_DPICHANGED` with no restart, within the alignment rule. Test: `focus_input::dpi_change_realigns_page` (T3.5).
5. **Animation effects change while the app runs.** The spike read the setting only once. Expected: the next motion follows the new value. Test: `reduced::follows_setting_change` (T2.3).

---

## Pins and upgrades (gate condition 8)
- **The pins.** `gpui` is `package = "gpui-pre", version = "=0.3.12"`. `gpui-kit` is `git = "https://github.com/longbridge/gpui-kit", rev = "39ab849a93b5f15507b969164d49891db8d5b470"`. `webview2-com` is `=0.39.1`. They are declared once, in `[workspace.dependencies]`.
- **When to upgrade:** only for a named reason: a bug we hit, a security fix, or a needed API. Never just to stay current. No upgrades between M7 and the release.
- **How to upgrade:** one PR that bumps gpui-kit and its matching `gpui-pre` together. Every GPUI bump is a gpui-kit bump (spike risk "pre-1.0 churn"). The PR must:
  - pass the full workspace tests;
  - re-run the motion p95 set (T7.4) and the browser checks (T3.1–T3.5) on the owner's PC;
  - pass a Fable review of the API changes.
- **Patches:** if a GPUI patch is unavoidable, it is a `[patch]` to a fork commit under the owner's GitHub account, with an upstream issue linked. It is never a vendored copy.

## Crates and files (decided here)
```
tjommie-rs/
  design/themes/aegis.json            semantic tokens (T0.2)
  design/fonts/*.ttf, OFL.txt         Archivo 400/500/600, "Archivo Expanded Tight" 800, JetBrains Mono 400/500/600 (T0.5)
  design/icons/*.svg                  baked Lucide strokes and avatar layers (T0.5)
  design/scripts/{fetch-fonts.py,gen-icons.py}   moved from the spike
  tools/perf/{measure-memory.ps1,run-motion-checks.ps1,frame-stats.py,covered.ps1}  (T0.3)
  installer/{tjommie.nsi,r1-install-facts.json}  (M8)
  crates/client/        Client, local daemon and remote PCs, host list
  crates/desktop-host/  no GPUI: identity, paths, owner_safety, crash, daemon_process, browser/, vault/, speech/, updater, shell/ (tray, notify, login_item, single_instance), device_keys, diagnostics, pdf
  crates/desktop/       bin tjommie-desktop: app, theme, keymap, zoom, guard, test_driver (feature), motion/, model/, ui/
```
**Why `desktop-host` is its own crate** (spec §2: "split only when a crate needs it"):
- Its tests run without compiling GPUI. That saves lane build time and disk.
- The browser lane can work and be reviewed in parallel with the UI lanes.

Record this in spec §2 (done in this change).

---

## Lanes and models (at most 7 agents at once, reviewers included)
| Lane | Model | Tasks, in order | Starts after |
|---|---|---|---|
| Core | Opus 5.5 max, fast mode | M0 (T0.1–T0.6) → M2 | — |
| Client | Sonnet 5.5 xhigh (may use Haiku 5.5 xhigh helpers) | M1. T1.6 is TypeScript on R1's patch line and ships early as 0.5.x | T0.1 |
| Browser | Sonnet 5.5 xhigh | T3.1 → T3.6 | T0.1, T0.4 |
| Services | Sonnet 5.5 xhigh | M6 | T0.1, T0.4 |
| Browser UI | Opus 5.5 max, fast | T3.7 | T2.2, T3.2 |
| Chat | Opus 5.5 max, fast | M4 | T2.2, T1.4 |
| Screens | Opus 5.5 max, fast | M5 | T2.2, T1.4 |
| Motion | Opus 5.5 max, fast | M7 | M2, T3.7, M4 |
| Release | Sonnet 5.5 xhigh, then ops | M8 | M3–M7 (T8.1 may start after T0.1) |
| Review | Fable | one review per merged milestone, plus the whole-branch review | each merge |

**Waves:**
1. Core, Client, Browser, Services and one reviewer.
2. Core becomes Chat; Screens, Browser UI, Browser, Services and one reviewer.
3. Motion, Release, the fix lanes and one reviewer.

No wave goes over 7. No GPT or Codex model is used anywhere.

---

## M0 Foundation (Core, sequential)

### T0.1 Workspace, pins, lints, CI
- **Files:**
  - `tjommie-rs/Cargo.toml`: add the pins under `[workspace.dependencies]`, and `[workspace.lints.clippy]` for the UI crates (T0.4).
  - `crates/client`, `crates/desktop-host` and `crates/desktop`, each with `Cargo.toml` and `src/lib.rs` or `src/main.rs`.
  - `.github/workflows/ci.yml`: the `rust` job already builds the workspace. Add the identity check (T0.6) and the `test-driver` e2e run (T0.5).
  - `.github/workflows/rust-dev.yml` (spec §7): on push to `rust-rewrite`, build the dev-identity installer as a workflow artifact only.
- **Test first:** `crates/desktop/tests/pins.rs` reads `Cargo.lock` and asserts that `gpui-pre` is 0.3.12 and that `gpui-kit`'s source contains rev `39ab849a`.
- **Done when:** `cargo build -p desktop --locked` opens an empty window (`cargo run -p desktop`). CI is green.
- **Also check:** GPUI's accessibility support at this pin (UI Automation provider: yes or no). Record the answer in the M0 report. Owner decision 4: no screen reader is accepted for this release, provided full keyboard navigation works (T5.6).

### T0.2 The Aegis theme file (Phase 0 leftover)
- **Files:**
  - `scripts/aegis-theme.ts` (Node, from the repo root) converts `design/claude-design/aegis/tokens.json` into `tjommie-rs/design/themes/aegis.json` with **semantic keys**. `--check` exits non-zero when the file is stale. The root `package.json` gets a script `theme:aegis`.
  - `crates/desktop/src/theme.rs` holds `Theme` and its typed sub-structs. It is loaded with `include_str!` and `serde` at start. This is the "desktop theme generated from it". There is no build-time codegen.
- **Key set:**
  - `color.light.*`, `color.dark.*` and `color.bot.*` (10 bot colours)
  - `type.desktop.*` and `type.phone.*`
  - `space.*`, `radius.*`, `size.*` and `shadow.*`
  - `motion.*`: all 39 motion tokens, i.e. `duration` ×12, `easing` ×4, `travel`, `stagger` ×3, `spring` ×3 as `{stiffness, dampingRatio}`, `distance` ×7, `flick`, `eyes` ×6 and `sweep` ×2
  - 219 leaves in total
- **Keys are stable and theme-neutral**, for example `color.<mode>.surface.paper`, not `color.light.stone`. The Workbench and Mission Control themes come later as files with the same key set. No switcher is built now.
- **Tests first:**
  - `scripts/aegis-theme.test.ts`: the key count equals the tokens' leaf count, and `--check` fails on a stale file.
  - `crates/desktop/tests/theme_keys.rs`:
    - every `tjommie-rs/design/themes/*.json` has exactly `aegis.json`'s flattened key set (spec Phase 0 exit criterion);
    - `Theme` deserializes `aegis.json` with `deny_unknown_fields`;
    - `motion.spring.calm` is `{stiffness: 260, dampingRatio: 1.0}`.
- **CI:** the Node job runs `node scripts/aegis-theme.ts --check`.

### T0.3 Measurement tools, fixed first (gate condition 1)
- **Files:** `tjommie-rs/tools/perf/`:
  - `measure-memory.ps1`, from spike `scripts/measure-baseline.ps1`, fixed as research Step 1.
  - `frame-stats.py`, `run-motion-checks.ps1` and `covered.ps1`, moved from the spike.
- **`measure-memory.ps1 -Target <Electron|Desktop> -State <S0..S4> -Pid <root>`** reports per process and in total:
  - `private_working_set_mib`: the sum of `Win32_PerfFormattedData_PerfProc_Process.WorkingSetPrivate`, matched by `IDProcess`;
  - `private_commit_mib`: the sum of `PrivateMemorySize64`;
  - `working_set_mib`: the legacy sum of `WorkingSet64`, kept for continuity with the 496.1 MiB baseline only.
- **The process set** is the whole process tree from the app's PID (the app, `tjommie-daemon.exe`, every `msedgewebview2.exe` whose browser process is the app's child, and `whisper-cli.exe` if running). It is never matched by name.
- **Gating:** the same CPU gate as the spike (system CPU at most 35% over 5 s before the run; marked `loaded` above 50%). It waits 60 s idle, then samples 10 times, 1 s apart, and reports the median.
- **Test first:** `measure-memory.ps1 -SelfTest` runs against a spawned `notepad.exe` tree and asserts that, per process, `0 < private_ws ≤ ws` and `private_commit > 0`, and that the totals equal the per-process sums.
- **Owner step D2 (after this task):** re-measure the installed production app (R1 if installed, else 0.4.2; record which) in states S0 and S1 with the fixed script. That is the private baseline the budget below is checked against.

### T0.4 Crash safety at UI boundaries (gate condition 4)
- **Lints** in `desktop`, `desktop-host` and `client`, through `[lints.clippy]`: `unwrap_used`, `expect_used`, `indexing_slicing`, `panic`, `unreachable`, `todo`, `unimplemented` and `string_slice` set to `deny`. They are allowed in `#[cfg(test)]` code and `tests/`.
- **`desktop::guard`:**
  - `guarded<R>(what: &'static str, f: impl FnOnce() -> R) -> Option<R>` runs `std::panic::catch_unwind(AssertUnwindSafe(f))`. On a panic it logs, raises the "Something went wrong" toast through a `Notices` global, and returns `None`.
  - Every GPUI event handler, `on_action`, `cx.spawn` body and timer callback in `desktop` goes through `guarded`. `render` methods that call fallible model code use `guarded` and draw an inline error row instead.
  - A test-only lint script, `tools/check-guarded.ps1`, greps `on_click(`, `on_action(`, `spawn(` and `on_key_down(` in `crates/desktop/src/ui` and fails any call whose closure doesn't start with `guarded(`. It runs in CI.
- **`desktop-host::log`:** appends to `<root>/logs/desktop.log`, the same file Electron writes (`main.ts:96-103`). It never fails the caller.
- **`desktop-host::crash`:**
  - `install(root: &Path)` sets a panic hook. It appends the message, the thread and the backtrace (line tables are kept in release) to `<root>/logs/desktop-crash.log` with a timestamp and the version, then lets the process die. Panics that GPUI's window procedure turns into an abort (0xC0000409) still reach the hook first.
  - It also calls `RegisterApplicationRestart("--hidden --after-crash", 0)`, so Windows restarts the app after a crash once it has run 60 s. The daemon then restarts too: it is the app's child and exits when its stdin closes (spec §2). That is the same blast radius as an Electron main-process crash today.
  - On start with `--after-crash`, the UI shows a notice: "Tjommie closed unexpectedly and restarted. Your bots were restarted too." The notice has a **Save report** action that writes the crash log into a diagnostics report (T6.6).
- **COM and WebView2 callbacks never panic:** they only push typed messages into a channel (see the T3.2 bridge rule).
- **Tests first:**
  - `guard::panicking_handler_returns_none_and_notices`.
  - `crash::hook_writes_log` (in a child process, through `spawn::command`, which panics on purpose; assert the log line and exit code).
  - `check-guarded.ps1` fails on a planted unguarded handler.
- **CHANGELOG** (in M8, once the app ships): "If Tjommie ever closes unexpectedly, it restarts by itself and offers a report you can save."

### T0.5 App skeleton: window, fonts, icons, zoom, focus root, test driver
- **Assets:** move spike `scripts/fetch-fonts.py` and `gen-icons.py` to `tjommie-rs/design/scripts/`. Commit their outputs (fonts with `OFL.txt`; the 161 baked SVGs, including the cropped eye layers per shape) under `tjommie-rs/design/`. Bundle them with `include_bytes!` and `cx.text_system().add_fonts`, and an `AssetSource` for `icons/`.
- **Window** (from spike `src/main.rs` and `shell.rs`):
  - 1440×900, min 360×600, title "Tjommie" (dev: "Tjommie RS").
  - Client-side caption buttons drawn in GPUI with window control areas (Lucide at stroke 1.25), as the Aegis shell shows them.
  - The root `track_focus`es and holds focus (GPUI limit 17).
  - `inactive_frame_interval` is unthrottled at creation (gate condition 5). Throttling is done by the motion clock (T2.3).
- **App zoom (GPUI limit 11):**
  - The rem is 16 px × zoom × text-size factor. Zoom steps are 0.8, 0.9, 1.0, 1.1, 1.25 and 1.5.
  - Keys: Ctrl+= (in), Ctrl+- (out), Ctrl+0 (reset). Zoom is stored per device in `desktop-ui.json` (T1.5).
  - Shadows use pixels and do not scale. That is accepted, as in the spike.
- **Single instance:** `desktop-host::shell::single_instance`. A named mutex `Local\<appId>` plus a registered window message, so a second launch shows the first window and exits (Electron `second-instance` → `showWindow`).
- **Test driver (feature `test-driver`, never in release):**
  - `desktop::test_driver` serves `\\.\pipe\tjommie-ui-<pid>` with JSON lines: `click{id}`, `type{id,text}`, `key{chord}`, `snapshot` (returns a typed `UiSnapshot { nodes: Vec<UiNode { id, role, text, enabled, focused, bounds }> }`) and `wait{id, predicate, timeout_ms}`.
  - Elements register through `.test_id("composer.send")`, which compiles to nothing without the feature.
  - The pipe's ACL allows only the current user.
- **Tests first:**
  - `tests/e2e_window.rs` (feature `test-driver`): launch with a temp root and an e2e identity, snapshot, and assert the root is focused and `sidebar.wordmark` reads "Tjommie".
  - `zoom::steps_clamp`.
  - `single_instance::second_launch_signals_first` (two processes).
  - `pins.rs` (from T0.1) also asserts that the release build has no `test_driver` symbols: `strings` on `target/release/tjommie-desktop.exe` must not contain `tjommie-ui-`.

### T0.6 Desktop test map and identity
- **`tjommie-rs/desktop-test-map.md`** maps every `packages/ui/**/*.test.ts(x)` and every `packages/desktop/src/**/*.test.ts` file to a Rust test path, or to "n/a: reason" (for example "phone only, Phase 4" or "Electron-specific, replaced by T3.x"). The 15 desktop `*.real.test.ts` files are mapped with the table in §"Test plan". **Done when** the counts match `git ls-files`.
- **`desktop-host::identity`:** `Identity { app_id, product, exe, data_root, ports, update_feed: Option<&'static str>, login_item }`, chosen at build time by the `TJOMMIE_IDENTITY` env var (`dev` by default, `production`, `updatetest`, `e2e`) through `build.rs`.
  - `updatetest` copies `packages/desktop/scripts/update-test-guards.ts` (`com.tsnyders.tjommie.updatetest`, GUID `5891e549-…`, product "Tjommie Update Test").
  - `e2e` uses a temp root and port 0.
- **`desktop-host::owner_safety`** (port `owner-safety.ts`):
  - `test_root()` returns a fresh folder under `%TEMP%` and asserts that.
  - `assert_root_allowed(root, identity)` refuses the production roots for any identity except production.
- **Tests first:** `identity::dev_has_no_feed` and `owner_safety::refuses_prod_root_for_dev`. A CI step asserts that the dev and e2e binaries contain no `github.com/tsnyders/tjommie` (spec §3.10).

---

## M1 Client, daemon process, view models (Client lane)

### T1.1 `client` crate
- **API:**
  - `Client::connect(endpoint: Endpoint, token: &str) -> Result<Client, ConnectError>`, where `Endpoint::Local { port }`.
  - `client.call::<M: ClientMethod>(params: M::Params) -> Result<M::Result, RpcError>`.
  - `client.events() -> tokio::sync::broadcast::Receiver<protocol::Event>`.
  - `client.closed() -> impl Future<Output = CloseReason>`.
- **`ClientMethod`** extends `protocol::Method` with `Params: Serialize` and `Result: DeserializeOwned`. Implement it for every method in the `methods!` table: add a second arm in the macro, not a hand list.
- Hello/welcome with `PROTOCOL_VERSION = 2`. Close codes 4003 and 4401 map to typed errors.
- **Tests first:** `client/tests/local.rs` starts the real daemon library in-process on `127.0.0.1:0` with a temp root, then:
  - calls `bots.list`;
  - checks that a bad token gives `ConnectError::Unauthorized`;
  - checks event delivery;
  - validates every response it receives with `protocol::assert_valid`.

### T1.2 Daemon process (port `desktop/src/daemon-process.ts`, `main.ts:263-387`)
- **`desktop-host::daemon_process`:**
  - spawns `resources\tjommie-daemon.exe --token-stdin` through `spawn::command` (or `TJOMMIE_DAEMON_EXE` in dev);
  - writes `{token, desktopHostToken}` as one JSON line;
  - reads the `Tjommie daemon listening on http://127.0.0.1:<port>` line.
- **Restarts:** at most 3 in 60 s, then `DaemonState::Failed("Tjommie's background service keeps stopping.")`.
- **Stop:** `stop_before_quit(timeout = 20 s) -> Stopped | Forced`. It closes stdin, then tree-kills (spec §2).
- **Dev mode:** if an existing dev daemon token file exists (`<root>/daemon-token`), connect instead of spawning.
- **Tests first:** port `daemon-process.test.ts`. Add `restart_limit_three_per_minute` (a fake daemon exe that exits at once).
- **Review focus 2:** `e2e_daemon_restart_with_panel_open` is written here as a failing e2e test and finished in T3.2.

### T1.3 Remote PCs and the host switcher (port `remote-wire.ts`, `device-keys.ts`, `ui/lib/remote-hosts.ts`, `relay-wire.ts`, `connection.ts`)
- **`client::remote`:** `Endpoint::Lan{host, port}` and `Endpoint::Relay{url, host_id}`. It uses Noise through the `transport` crate, with the framing from spec §3.3. At most 8 open connections and a 32 MiB backlog.
- **`desktop-host::device_keys`:** Credential Manager `Tjommie/<ns24>/device/<name>`. The name matches `^[a-z0-9._-]{1,64}$`, the value is at most 1920 bytes as base64url, and the bytes are identical to TypeScript's.
- **Hosts** are kept in `desktop-ui.json` (T1.5), with at most 16.
- **Tests first:**
  - port `remote-wire` and `remote-hosts`;
  - a cross-implementation test: `tools/cred-probe.ts` writes a device key and Rust reads it, then the reverse, under a random test namespace;
  - an interop test: the Rust client pairs with the Rust daemon's LAN host and the local relay (as Phase 1 M7).

### T1.4 View models (port `packages/ui/lib/*` logic, no GPUI)
- **Port to `crates/desktop/src/model/`, one module per file:**
  - `accounts`, `activity`, `approval`, `attachments`, `attachment-transfer`, `bot-form`, `browser-geometry` (address-to-URL, key routes), `browser-session`, `native-browser-session`, `document-ranges`, `exchange`, `format`, `labels`, `logins`, `markdown-document`, `mentions`, `overview`, `panel-layout`, `projects`, `routine-form`, `routines`, `setup`, `state` and `store` (as `AppStore`: the event-fed state, entity-free), `suggestions`, `timeline`, `usage`, `dictation`, `google-connection` ("not enabled" only), `file-thumbnails` and `browser-preview`.
- **Each module ports its `*.test.ts` cases one for one** (`model::timeline::tests::…`).
- **Not ported, with the reason recorded in `desktop-test-map.md`:**
  - phone only (Phase 4): `push`, `phone-nav`, `use-phone`, `use-keyboard`, `mobile-identity`, `mobile-style`, `keep-alive` and `native-capability`;
  - React hooks that become GPUI entities (`use-*`; their logic goes into the matching model);
  - `utils` (`cn`);
  - `theme` (T0.2);
  - `desktop` (the Electron preload bridge, replaced by direct calls);
  - `native-overlay` (the scrim bridge, replaced by T3.5's shield);
  - `local-speech` and `attachment-device` (replaced by M6);
  - `remote-pc-fixture` (a test fixture, ported where its tests are);
  - `google-api` (not enabled).
- **Done when** every `packages/ui/lib/*.test.ts` case is ported or marked n/a in the map.

### T1.5 Device-local settings and the Electron hand-over
- **`desktop-host::device_settings`:** `<root>/data/desktop-ui.json` (typed, `deny_unknown_fields`, written atomically) holds:
  - theme (`system | light | dark`), text size (`system | large | larger`), density (`comfortable | compact`) and zoom;
  - paired hosts (`RemoteHostSchema`, at most 16) and pending hosts;
  - a `tray_hint_shown` marker.
- **Import, once, when the file is missing:**
  - if `<root>/data/desktop-ui-export.json` exists (written by the 0.5.x Electron patch, T1.6; owner decision 3), read it and validate every field;
  - otherwise use the defaults.
  - Either way the import is logged.
- **App zoom is not exported.** Electron keeps it in Chromium's preferences, not in page storage, so it starts at 100%.
- **Tests first:**
  - `device_settings::imports_export_once`
  - `device_settings::rejects_bad_export_field`
  - `device_settings::atomic_write_survives_partial`
  - `device_settings::reads_electron_fixture`, which reads the fixture T1.6 writes from TypeScript.
- **CHANGELOG** (in M8, for 0.6.0): "Your paired PCs, theme, text size and density carry over to the new app. Zoom starts at 100%."

### T1.6 Electron 0.5.x patch: export paired PCs and appearance (owner decision 3; R1's patch line, TypeScript)
- **Branch:** this task runs on R1's patch line: a branch from `main` after `v0.5.0`, released as 0.5.1 (or the next 0.5.x). It does **not** run on `rust-rewrite`. Ops ports the commit forward to `rust-rewrite`.
- **New file:** `packages/desktop/src/desktop-ui-export.ts`, with `exportDesktopUi(read: (key: string) => Promise<string | null>, dataRoot: string): Promise<void>`.
  - It reads the 5 page-storage keys:
    - `tjommie.theme`
    - `tjommie.textSize`
    - `tjommie.density`
    - `tjommie.mobile.hosts.v1`
    - `tjommie.mobile.pending.v1`
  - It validates them with the same zod schemas the UI uses (`RemoteHostSchema`, at most 16; the theme, text-size and density enums).
  - It writes `<dataRoot>\data\desktop-ui-export.json` atomically (temp file, then rename): `{ version: 1, theme, textSize, density, hosts, pending }`.
  - An invalid field is dropped and the log records it. It never throws into the app.
- **Called from:** `main.ts`, after the window's `did-finish-load` in production. `read` is `win.webContents.executeJavaScript('localStorage.getItem(<key>)')`, run on the app's own origin only (`speechOrigin` check). It runs on every start, so the file is always current.
- **The device keys** that the hosts reference are already in Credential Manager under the root's namespace, so they carry over without export.
- **Tests first:** `packages/desktop/src/desktop-ui-export.test.ts`.
  - `writes all five fields`.
  - `drops an invalid host and keeps the rest`.
  - `missing keys give defaults`.
  - `atomic write leaves no partial file`.
  - `writes the cross-implementation fixture`: it writes `tjommie-rs/crates/desktop-host/tests/fixtures/desktop-ui-export.json`, and `--check` mode fails if that file is stale. T1.5's `reads_electron_fixture` reads it.
- **Release:** follow the AGENTS.md steps for 0.5.x:
  - bump the versions;
  - add the versionCode map entry;
  - rename `## Unreleased`;
  - ops merges and tags.

  It must be published **at least 7 days before 0.6.0**, so R1's updater has installed it on the owner's PC. T8.4 uses this 0.5.x build as the R1 baseline.
- **If an install skipped 0.5.x** (it updates straight to 0.6.0), the export file is missing and T1.5 falls back to defaults. The 0.6.0 CHANGELOG line covers that case.
- **CHANGELOG** (0.5.x): "Tjommie now keeps a copy of your paired PCs and appearance settings, so the next update can carry them over."
- **Done when:** the vitest file passes, `npm run typecheck` is green, and the Rust fixture test reads the TypeScript output.

---

## M2 Shell, primitives, motion runtime (Core lane, Opus)

### T2.1 App shell and navigation (`app-shell`, `rail`, `tab-bar`, `panel-bar`, `back-link`, `host-switcher`, `connection-screen`, `failed-load`)
- **`ui/shell/`:**
  - the Aegis persistent sidebar: wordmark "Tjommie"; sections Agents, Rooms, Projects, Routines, Settings; Recents with living avatars and state markers; New bot pill; the host avatar at the foot, which opens the host list;
  - the chat column;
  - the browser panel slot;
  - the connection screen (connecting, failed with the reason, retry);
  - a failed-load row.
- **Routes:** `Route::{Agents, Chat(BotId), Room(RoomId), Projects, Routines, Settings(Section), NewBot, BotSettings(BotId)}`.
- **Navigation from notifications** (T6.2) goes through `App::navigate(Route)`.
- **Tests:** GPUI `TestAppContext` view tests (`shell::navigates_to_chat_from_recents`) plus `e2e_navigation` (test driver).

### T2.2 Primitives (`dialog`, `menu`, `pill`, `icon-button`, `controls`, `bot-avatar`, toast, sheet, settings row, focus ring)
- **`ui/primitives/`:**
  - `Dialog`, `Menu` and `Sheet` on gpui-component overlays;
  - `Toast` (top centre, under the header);
  - `Pill`, `IconButton` (press fills compress, labels stay put) and `Toggle`;
  - `SettingsRow`;
  - `FocusRing` (2 px ink, 2 px gap, on `focus_visible`);
  - `BotAvatar { bot, size, pose }`: two SVG layers (body tinted, eyes white) from T0.5's assets.
- **Every overlay calls `BrowserShield::raise()` on open and `lower()` on close** (T3.5). This is the port of `dialog:scrim` (`main.ts:582-588`).
- **Escape closes the topmost overlay at once.** Keyboard actions never animate.
- **Tests:** `dialog::escape_closes_without_frames` (motion clock records 0 in-between frames) and `menu::raises_shield`.

### T2.3 Motion runtime (GPUI limits 13, 14, 15 and 18; gate condition 5)
- **`motion::spring`:** port spike `src/motion.rs`. Semi-implicit Euler, 8 substeps, dt capped at 64 ms, at rest within 0.25 px and 8 px/s. Retargeting keeps the velocity. The constants come from `Theme.motion.spring.*`.
- **`motion::clock`:**
  - a window-wide ticker that calls `request_animation_frame` only while something moves;
  - it records each frame's interval and work time in a ring buffer (the app's own frame log, the second measure next to PresentMon);
  - `--trace <csv>` writes the log.
  - **Throttle rule (gate condition 5):** while the window is inactive the clock advances at 30 fps, **unless** `Live::any()` is true. `Live::any()` is true while a remote live stream view is visible, while the browser panel's live variant is moving, or while a motion check scenario is running. In those cases it runs at the display rate. GPUI's own `inactive_frame_interval` stays unthrottled (T0.5), so this rule is the only throttle.
- **`motion::reduced`:**
  - reads `SPI_GETCLIENTAREAANIMATION` at start and on `WM_SETTINGCHANGE` (spike limit 18; review focus 5);
  - when on, every distance is 0, shared elements cross-fade, eyes, scans and the sweep stop, and numbers snap;
  - `--force-reduced` is for tests.
- **`motion::keyboard`:** a `Trigger::{Pointer, Keyboard}` travels with every state change. `Keyboard` snaps.
- **Helpers:**
  - `flip::Flip` keeps the previous painted bounds per key from the last paint (limit 14);
  - `flight::Flight` is a `deferred` overlay between two captured bounds (spike flight);
  - `digits::Rolling` rolls numbers;
  - `check::DrawOn` strokes a path to a partial length;
  - `eyes::Eyes` handles blink clocks, scan, glance and poses, with cropped eye assets (limit 15).
- **Tests first:**
  - `spring::retarget_keeps_velocity`;
  - `spring::calm_settles_in_560ms_within_tolerance`;
  - `reduced::follows_setting_change` (send a synthetic `WM_SETTINGCHANGE` after flipping the value through a test seam);
  - `clock::inactive_throttles_unless_live`;
  - `keyboard::snaps`.

### T2.4 Keymap and shortcut forwarding source (gate condition 2)
- **`desktop::keymap`** is the single list of app-wide bindings:
  - Ctrl+=, Ctrl+-, Ctrl+0 (zoom);
  - Ctrl+B (browser panel);
  - Escape (close the topmost overlay or the panel);
  - Ctrl+Enter (approve in the focused approval card);
  - Enter (send) and Shift+Enter (new line) in composers;
  - the room composer's mention keys (ArrowUp, ArrowDown, Tab, Enter, Escape; `room-composer.tsx:119-135`);
  - any binding added later.
- **`keymap::page_forwarded()`** returns every app-wide chord with Ctrl or Alt, plus Escape. T3.5 forwards exactly this set from WebView2.
- **Test:** `keymap::forwarded_set_equals_app_chords`. Adding an app-wide chord without it being forwarded fails the test.

---

## M3 Browser host (Browser lane, Sonnet; T3.7 Browser UI lane, Opus)

The engine is (a): a WebView2 child window per bot in **one** environment, with a profile per bot (spike `src/browser.rs:112-148`). It ports `native-browser.ts` and its helpers, with the memory fixes from day one (gate condition 1).

### T3.1 Engine and lifecycle (research #2, #3, #4, #5 and #6)
- **Files:**
  - `desktop-host/src/browser/engine.rs`: `Engine`, `open`, `close_bot`, `close_all`, `delete_profile` and `prune_profiles`;
  - `lifecycle.rs`: the visibility state machine;
  - `flags.rs`;
  - `profiles.rs`.
- **Environment:** one `ICoreWebView2Environment`, created **lazily** on the first `open(bot)`. The user data folder is `<root>/data/webview2`.
- **Profile name:** the full 64-hex `sha256(botId)`. That fits the 64-character limit, uses only allowed characters, and is the same hash as `native-browser-policy.ts:nativeBrowserProfile`.
- **Old Electron profiles** (`<root>/data/native-browser/<hash>`) are **left untouched**, so a rollback to R1 keeps the bots' site sessions. They are deleted one release later (§"Rollback").
- **Controllers:** created only by `open(bot)`, which matches Electron (`native-browser.ts:87`). Nothing is created at start.
- **Flags:** `--disable-features=SpareRendererForSitePerProcess,BackForwardCache,Translate,MediaRouter,OptimizationHints,EdgeCollections,msWebOOUI`.
  - Never `--disable-site-isolation-trials`, `IsolateOrigins` or `site-per-process`. Never `--single-process`, `--in-process-gpu` or `--disable-gpu`.
  - No `--renderer-process-limit`.
  - A test asserts the string.
- **`lifecycle::Need`** is computed per bot from three inputs:
  - `shown`: it is the active bot in the open local panel;
  - `streaming`: a `browser.stream` or `browser.frame` capture is on;
  - `tool`: an MCP tool, a vault fill or a UI command is executing.

  | Need | Placement | Visibility | Suspend |
  |---|---|---|---|
  | `shown` | in the panel | `SetIsVisible(true)` | `Resume()` first |
  | `streaming` or `tool`, and not shown | parked off-screen at the bot's viewport (spike `park`) | `SetIsVisible(true)` | `Resume()` first |
  | none | `place(None)` | `SetIsVisible(false)`, then `ICoreWebView2_3::TrySuspend` | suspended |

  - The parked state never applies to an idle hidden bot: closing the panel with nothing streaming suspends the page. This fixes the spike's permanent parking (`shell.rs:1108-1116`).
  - **Optional:** a bot hidden but kept awake by `tool` for over 30 s gets `ICoreWebView2_19::MemoryUsageTargetLevel = Low`. It is set back to `Normal` before it shows. It is never toggled on a page that `TrySuspend` handles (Microsoft's rule).
  - **Idle close:** a hidden bot with no need for **10 min** has its controller closed (`ICoreWebView2Controller::Close`). The profile stays on disk. It reopens at its last URL on the next `open`. A test seam changes the 10 min.
- **Tests first** (`desktop-host/tests/browser_lifecycle.rs`, with a plain Win32 parent window as in spike `src/win.rs`, the fixtures from spike `fixtures/`, and a temp root):
  - `no_environment_until_open` (0 `msedgewebview2.exe` children);
  - `profiles_isolated` (from the spike `bots` scenario);
  - `hidden_idle_bot_suspends`;
  - `tool_on_suspended_bot_resumes_first` (review focus 1);
  - `panel_closed_no_stream_does_not_park`;
  - `idle_controller_closes_after_timeout`;
  - `flags_exact`;
  - `delete_profile_removes_folder_after_close`.
- **CHANGELOG** (M8): "The bots' browsers use much less memory: a bot's browser starts only when it is used, and rests while it is hidden."

### T3.2 Host bridge, commands, control and tabs (port `native-browser.ts`, `-host.ts`, `-access.ts`, `-policy.ts`, `-permissions.ts`, `-profiles.ts`, and `main.ts:315-366`)
- **`browser::host_bridge`:** the `/browser-host` client with `desktopHostToken`. It executes `NativeBrowserCommand` (open, state, control, navigate, history, tab, dialog, tool, remote, close, delete_profile, cancel_tools) and sends `NativeBrowserStreamMessage`.
- **The `disabledNativeBots` rule:** `bot.deleted`, or `bot.updated` with the browser off, closes the bot's controller. Every command re-checks after it runs (`main.ts:329-331`).
- **The bridge rule (GPUI limit 5, the re-entrancy limit):**
  - WebView2 and COM callbacks never touch GPUI state. They push a typed `HostEvent` into an `async_channel`, and a GPUI foreground task applies them.
  - GPUI never makes a synchronous Win32 or WebView2 call inside an entity update. Those calls are queued with `cx.defer` or `window.defer`. That covers `SetParent`, `SetWindowPos` on the child, `SetFocus`, `CapturePreview` and `PrintWindow`.
  - A debug assertion (`RefCell` borrow probe) checks this in tests.
- **Permissions:** the policy from `native-browser-permissions.ts` (deny everything except what the file allows), through `add_PermissionRequested`.
- **New windows:** `NewWindowRequested` opens a new tab in the same bot (`#newTab` with the supplied contents).
- **Tests first:** port `native-browser*.test.ts` (access, policy, permissions, profiles, host). Finish `e2e_daemon_restart_with_panel_open` (review focus 2).

### T3.3 The 26 tools over CDP (port `native-browser-automation.ts`)
- **`browser::cdp`:**
  - `call(method, params) -> Result<T>` through `CallDevToolsProtocolMethod` (and `…ForSession`, `ICoreWebView2_11`, for out-of-process iframes);
  - typed events through `GetDevToolsProtocolEventReceiver`.
  - Params and results are typed structs per CDP method used. Untyped JSON is kept only at the decode boundary.
- **`browser::automation`:** the 26 tools.
  - `browser_click`, `browser_close`, `browser_console_messages`, `browser_drag`, `browser_drop`, `browser_emulate_media`, `browser_evaluate`;
  - `browser_file_upload`, `browser_fill_form`, `browser_find`, `browser_handle_dialog`, `browser_hover`;
  - `browser_navigate`, `browser_navigate_back`, `browser_navigate_forward`, `browser_network_request`, `browser_network_requests`;
  - `browser_press_key`, `browser_reload`, `browser_resize`, `browser_select_option`, `browser_snapshot`;
  - `browser_tabs`, `browser_take_screenshot`, `browser_type`, `browser_wait_for`.

  Their inputs, outputs and limits match `native-browser-automation.ts:387-495`. `cancel_tools` aborts in-flight tools.
- **Tests first:** port `native-browser-automation.test.ts` one for one, against the fixtures, and add one test per tool that checks its result shape against the protocol schema. **Done when** desktop `native-browser.real`'s cases pass as `desktop-host/tests/browser_tools.rs`.

### T3.4 Stream, frames, private mode (spec §3.9; port `native-browser-frame.ts`, `-stream` tests)
- **`browser::stream`:**
  - `Page.startScreencast` (JPEG q60, at most 2560×1600 and 2 MiB, `maxFps` honoured by dropping frames) while any `streaming` need is on;
  - frames go to the daemon over `/browser-host`;
  - `browser.frame` snapshots and thumbnails come from `Page.captureScreenshot`.
- **Private mode** (`beginPrivate`/`endPrivate`, `:362-385`) stops the stream (`stream_end{reason: private}`) and refuses frames, snapshots and the accessibility tree until it ends.
- **Tests first:**
  - port `native-browser-stream.test.ts`;
  - `stream::meets_lan_target` (spike `fixtures/stream.html`, 1280×800, 60 s, median ≥ 55 fps, p95 gap ≤ 34 ms, on loopback, CPU-gated);
  - `stream::hidden_window_keeps_streaming` (review focus 3: the panel closed and the window minimized);
  - `private_forwards_zero_frames`.

### T3.5 Shield, input, focus hand-off, shortcuts, alignment (gate conditions 2 and 6; GPUI limits 8, 9, 10, 12 and 16)
- **Shield (limit 8):**
  - `BrowserShield::raise()` takes a `CapturePreview` (PNG) of the live page **while it is still visible**, draws it as a GPUI `img()` in the page's frame, then hides the child window. `lower()` brings the live page back.
  - The overlay's covered part shows once the snapshot is ready, measured at 64 / 102 / 91 ms in the spike. That is accepted, and the log records it.
- **Click to focus (limit 9):** a capture-phase mouse handler on the root calls `SetFocus(gpui_hwnd)` when a click lands outside the page.
- **Focus hand-off out of the page (spike step 5 failure).** Shift+Tab is not an accelerator key, so it never reaches `AcceleratorKeyPressed`. That is the likely reason the spike's Ctrl+= worked and Shift+Tab did not. Two mechanisms replace it:
  1. **Shift+Tab always leaves the page**, as Electron does (`native-browser.ts:693-697`):
     - an isolated-world script, added through CDP `Page.addScriptToEvaluateOnNewDocument {worldName: "tjommie"}`, listens for a capture-phase `keydown` of Shift+Tab, calls `preventDefault`, and calls a binding added with `Runtime.addBinding {name, executionContextName: "tjommie"}`;
     - the host receives `Runtime.bindingCalled`, then `SetFocus(gpui_hwnd)`, focuses the panel's last control, and emits the focus-return event;
     - the page's own world cannot see or call the binding.
  2. **`add_MoveFocusRequested`** handles Tab past the last element (`Next`): GPUI focus goes to the next control after the page. With `Previous`, it goes to the previous control. Either way `Handled = true` and `SetFocus(gpui_hwnd)`.
  - If step 1's binding is not reachable from the isolated world at this WebView2 version, the test says so and the owner decides (`MoveFocusRequested` alone only covers the first and last element).
- **Shortcuts out of the page (limit 10):** `AcceleratorKeyPressed` forwards exactly `keymap::page_forwarded()` (T2.4). Those keys are marked handled and dispatched as GPUI actions. Every other key goes to the page.
- **Alignment:**
  - the page frame follows the panel bounds × the window scale factor, on `WM_DPICHANGED` (review focus 4) and on app zoom;
  - **rule:** every page edge is within 1 px of the frame; the viewport may differ from the frame by up to 2 px at fractional scales (whole CSS px; spike step A);
  - the alignment fixture is tall enough (at least 1000 CSS px of layout fits without a scrollbar at 150% on a 1080 px screen), so the page's own scrollbar doesn't trigger a false failure.
- **Corners (limit 12):** the page area has square corners. The panel's rounded frame is drawn around it in GPUI. That is a design deviation, listed in §"Design deviations".
- **Tests first:**
  - `focus_input::shift_tab_leaves_page` and `tab_past_end_moves_focus` (`SendInput` on a dedicated test desktop session in CI; on the owner's PC they run as owner step B again);
  - `forwards_every_app_chord`;
  - `click_outside_page_takes_focus`;
  - `shield_snapshot_before_hide`;
  - `dpi_change_realigns_page`;
  - `alignment_rule_100_125_150` (owner step A again, through spike `owner-checks-scale.ps1`, which restores the scale).

### T3.6 Login vault, vault host, worker isolation, secure entry (port `login-vault.ts`, `vault-storage.ts`, `vault-host.ts`, `vault-command-gate.ts`, `vault-entry.*`, `worker-isolation.ts`, `native-browser-login.ts`)
- **`desktop-host::vault`:**
  - **storage:** records in `data/logins.sqlite` (the same schema, `vault-storage.ts:63-64`) and secrets in Credential Manager `Tjommie/<ns24>/login/<uuid>`, byte-identical;
  - **gate:** a local request is accepted only while the window is visible, not minimized, not quitting, and the app document is ready (`main.ts:457`);
  - the `/vault-host` client;
  - `RemoteWorkerIsolation` over `/worker-host`;
  - fill, begin and finish against the engine's private mode (`main.ts:228-241`).
- **Secure entry:** a separate GPUI window (`ui/vault_entry.rs`) for typing a login. It is its own top-level window, with no test driver hooks in release, and secrets are zeroed after write. It replaces `vault-entry.html`.
- **The logins menu** (the key icon) in the panel lists the saved logins for the exact site, with Fill login, Save this login and Manage logins (spike step 8).
- **Tests first:** port `login-vault`, `vault-storage`, `vault-host`, `vault-command-gate`, `vault-entry` and `native-browser-login`. Add the cross-implementation test (TypeScript writes a login secret, Rust reads it, and the reverse, under a test namespace).

### T3.7 Browser panel UI and its motion (Browser UI lane, Opus; gate condition 3)
- **`ui/browser/`** (`browser-panel.tsx`, `browser-thumbnail.tsx`, `browser-logins.tsx`, `right-panel.tsx`, `bot-panel.tsx`):
  - tabs, address bar (`model::browser_geometry::address_to_url`, DuckDuckGo default), back, forward and reload;
  - Take control and Return to bot (raised white pills), the logins menu, the dialog prompt bar and the Google "automated browser" notice;
  - the remote live-stream view, which draws `browser.streamFrame` and acknowledges each drawn frame (`browser.streamAck`), falling back to `browser.frame` polling on `unknown_method`.
- **Panel motion: the live variant is the default.**
  - The panel width springs (calm), and the chat column takes the rest. The WebView2 window is moved and clipped every frame (spike `place_clipped`).
  - **Automatic per-motion fallback to the snapshot variant:** at each panel motion's start, `PanelVariant::choose(last_live_p95: Option<f32>, last_live_at, system_cpu_1s)` returns `Snapshot` if:
    - the last live panel motion in the past 60 s had a p95 frame interval (from `motion::clock`) above **16.7 ms**, or
    - the system CPU over the last second (`GetSystemTimes`) is above **60%**, the load at which the spike measured the snapshot variant still passing (15.85 and 16.49 ms at 53% and 62%).

    Otherwise it returns `Live`. After 60 s with no miss, live is tried again. The snapshot variant slides a `CapturePreview` image taken at rest, and brings the live page back at rest.
  - **What the fallback cannot do (stated in the UI docs and the report):**
    - it decides per motion, so the first live motion after load rises can still miss;
    - it cannot rescue a saturated CPU: at 100% CPU both variants missed in the spike (live 46–53 ms, snapshot 27–66 ms);
    - snapshot frames are lighter but still do layout, so the panel needs a mostly quiet PC to meet 16.7 ms.
  - **T7.3 tests the MMCSS option** for heavy load.
- **Tests first:**
  - `panel_variant::choose` table tests: no history gives `Live`; a 17.2 ms p95 within 60 s gives `Snapshot`; 61 s later it gives `Live`; CPU 65% gives `Snapshot`;
  - `e2e_panel_open_close` (state only);
  - `panel::keyboard_toggle_snaps` (Ctrl+B, 0 frames);
  - `panel::interrupt_continues_from_value` (step at the turn at most the largest uninterrupted step + 0.5 px, or the frame-time-normalised rule from the spike report).
- **CHANGELOG:** "The browser panel slides open with the live page moving with it." Also "Paired devices and the panel use the same live view of a bot's browser."

---

## M4 Chat and files (Chat lane, Opus)

### T4.1 Chat screen and thread (`chat-screen`, `chat-header`, `message-list`, `message-view`, `bubbles`, `activity-row`, `handoff-row`, `notices`, `setup-banner`, `sign-in-toast`, `exchange/exchange-view`)
- A virtualized, bottom-anchored GPUI `list()` with `ListState`: variable heights, measured lazily, 400 px overdraw (spike criterion 1).
- Your bubbles have corner radii 20, 20, 6 and 20. Bot turns show meta, "Thought for", tool rows, prose, code blocks, exchange cards, approval records and hand-off rows.
- Stop, retry, and steer or queue (spec §3.2).
- **Tests:**
  - port the component tests' behaviours as view tests;
  - `thread::ten_thousand_messages_scroll_p95` is a PresentMon check in T7.4.

### T4.2 Approval card (`approval-card`)
- Approve and Deny, with the autonomy hint.
- The Approve motion: stamp (playful), the edge flashes ochre, a 260 ms beat, then it collapses into its record (calm) while the followers stagger. Deny lets go quietly. Ctrl+Enter snaps.
- **Tests:** `approval::keyboard_snaps` and `approval::interrupt_undo_halfway`.

### T4.3 Composer (`composer`, `dictation-button`, `room/room-composer` shared parts)
- The white card lifts 2 px on focus. The model pill grows into its menu.
- Attachments: a picker through GPUI's `prompt_for_paths`, and drag-and-drop of files.
- Mentions: typing @ with the keys from T2.4.
- The send button pops awake. Enter sends at once; Shift+Enter adds a new line.
- Dictation fills an editable draft and never sends on its own (§6 row 14; T6.5).
- The placeholder is at the Aegis body size (14.5 px). Fix the gpui-component `Textarea` placeholder size (spike risk "composer text size").
- **Tests:** `composer::enter_sends_once`, `dictation_never_sends`, `placeholder_body_size`.

### T4.4 Files and previews (`file-card`, `file-thumbnail`, `file-viewer`, `markdown`, `markdown-file-viewer`, `pdf-file-viewer`, `projects/task-diff`)
- **Markdown:** `pulldown-cmark` with GFM: tables, task lists, strikethrough and autolinks.
  - Links: http and https open in the default browser, and every other target is blocked (`markdown.tsx:8`). Local images come only from `/_attachments/<uuid>`.
  - **Inline code (GPUI limit 1):** JetBrains Mono with a `TextRun` background, at full size and without radius or padding. That is a design deviation.
- **Code:** a monospace block with line settle. There is no syntax highlighting; 0.4.2 has none either.
- **Diffs:** `TaskDiff` from the protocol, drawn as in `task-diff.tsx`.
- **Images:** GPUI `img()` (png, jpeg, gif, webp).
- **PDF:** `desktop-host::pdf` wraps pdfium (`pdfium-render`). `pdfium.dll` is pinned to a release of `bblanchon/pdfium-binaries` and checked by sha256 in CI and at load. Pages are rendered on demand through `FPDF_LoadCustomDocument`, with block reads mapped to `attachments.read` ranges (256 KiB chunks), so a 200 MiB PDF is never read whole.
- **Audio and video:** a lazily created **media viewer** WebView2 controller in its own profile (`media`). It loads a page that plays the file through `WebResourceRequested`, streaming `attachments.read` ranges, with no network. It is closed when the viewer closes, and shielded like the browser. This is resolved by owner decision 5: the built-in WebView2 player.
- **Save:** a save dialog (GPUI `prompt_for_new_path`) streams the chunks to the file (port `attachment-save.ts`). There are also Open folder, Show in folder and Open notes (`main.ts:570-598`).
- **Tests:**
  - port `markdown`, `file-viewer`, `document-ranges` and `attachment-save`;
  - `pdf::range_reads_only` (a counting fake source);
  - `markdown::blocks_non_web_links`;
  - `media_viewer::closed_after_close` (0 media controllers).

---

## M5 Screens (Screens lane, Opus)

### T5.1 Agents overview and bots (`app/agents`, `agents/cards`, `app/bots/new`, `app/bots/settings`, `bot-form/*`)
- The Agents overview with Needs-you cards and rolling numbers.
- Bot create and edit (identity, model row, rows), pause, delete, notes (Open notes) and avatar, for §6 row 1.

### T5.2 Rooms (`app/room`, `room/*`)
- The rooms list, room screen, posts, members panel and room form; @mention and @all; exchanges and the chain limit, for row 4.

### T5.3 Projects (`app/projects`, `projects/*`)
- The project editor, task editor, task detail and task diff; worktrees, review, checks and integrate, for row 8.

### T5.4 Routines (`routines/*`)
- The routine editor and rows with run history, for row 5.

### T5.5 Settings (`app/settings`, `settings/*`, `accounts/*`, `usage/*`, `devices/*`, `general/desktop-app`, `google/google-connection`, `setup/provider-cards`, `mobile-connect`)
- **Sections:**
  - Accounts: sign-in, check, limits, Codex sandbox, setup (row 6);
  - Usage and spending caps, with the limit meter (row 7);
  - Devices: pairing QR through the `qrcode` crate, LAN and relay rows, this PC, host switcher (row 15);
  - Logins (row 11);
  - Voice (row 14);
  - Appearance: Aegis light and dark, follow system, text size, density, zoom and reduced motion status (rows 18 and 22);
  - Diagnostics, including Save report (row 18);
  - Desktop app: start at login and updates, with state, check and restart (row 19);
  - Google ("not enabled", row 21).
- The setup provider cards and the connection screen.
- **Tests (all of M5):** view tests per screen with a fake `AppStore`, plus one e2e flow per screen through the test driver against the e2e daemon with recorded provider fixtures. Examples: `e2e_create_bot_and_chat`, `e2e_room_mention`, `e2e_project_task_review`, `e2e_routine_run_history`, `e2e_settings_sections`.
- **CHANGELOG** (one line for M4–M5, added when the app ships in M8): "Tjommie has a new look: the Aegis design, with a sidebar, a composer card and bots that blink and look up when they need you."

### T5.6 Full keyboard navigation (owner decision 4; the condition for shipping without a screen reader)
- **Every interactive element is reachable and usable from the keyboard:**
  - Tab and Shift+Tab move through each screen in visual order;
  - the focus ring shows on every stop;
  - Enter and Space activate buttons, pills and toggles;
  - arrow keys move within menus, lists, Recents and tabs;
  - Escape closes the topmost overlay.
- The sidebar sections, Recents, the composer, approval cards (Ctrl+Enter), the browser panel controls and the page (T3.5 hand-off), every settings row and every dialog are all reachable.
- **There are no mouse-only actions.** Hover-only reveals have a focus equivalent.
- **Tests first:** `e2e_keyboard_navigation` (test driver, `key` ops only, no `click`). It starts at launch, then:
  - reaches every route in `Route`;
  - creates a bot and sends a message;
  - approves a card;
  - opens and closes the browser panel and moves into and out of the page;
  - opens every settings section and every dialog.

  It asserts that `focused` moves to each expected `test_id` in order, and fails on any element registered as interactive that no Tab sequence reaches. Interactive registration is a flag on `.test_id`.
- **Owner check:** one pass through the D-row checklist using only the keyboard (in `phase3-checklist.md`).
- **Exit criterion:** see §"Exit criteria additions".

---

## M6 Desktop services (Services lane, Sonnet)

### T6.1 Tray (port `main.ts:684-711`, `updater.ts` `trayTemplate`)
- **`desktop-host::shell::tray`:** `Shell_NotifyIconW` on a message-only window. The context menu (`TrackPopupMenu`) has Show, the update item (Restart to update when ready), and Quit. A left click shows the window.
- Closing the window hides it. The first time, the "Tjommie is still running" toast appears (the `tray_hint_shown` marker).
- **Test:** `tray::menu_items_follow_update_state` (pure template function, as `updater.test.ts`).

### T6.2 Notifications (port `notification-delivery.ts`, `main.ts:389-437`)
- **`shell::notify`:** WinRT `ToastNotificationManager::CreateToastNotifierWithId(appId)`.
  - Toasts are skipped (but acknowledged) when the window is visible and focused.
  - Notifications are acknowledged (`notifications.ack`) only after `Show()` succeeds and the notifier's `Setting` is `Enabled`. Otherwise the notice stays pending, which matches "only the OS-backed show permits acknowledgement".
  - A click (`Activated`) shows the window and navigates to `notificationRoute(notice)`.
  - `notifications.pending` is fetched after subscribing.
- The AUMID is registered by the installer's Start-menu shortcut (M8). Dev runs use the exe path, as Electron does.
- **Tests:** port `notification-delivery.test.ts`, plus `notify::not_acked_when_disabled` (fake notifier).

### T6.3 Start at login (port `login-item.ts`)
- **The facts come from R1 first.** T8.1 records the `HKCU\…\Run` value name and data, and the `StartupApproved\Run` entry, that R1's `setLoginItemSettings` writes. Electron's default value name is the AppUserModelID; confirm it, don't assume it.
- The Rust code writes the same value name, `"<install>\Tjommie.exe" --hidden`, so an existing R1 entry keeps working. It is set once on the first packaged run (the `login-item-set` marker in Electron's userData `%APPDATA%\Tjommie`, read and written at the same path). `--hidden` and `--updated` start in the tray.
- Dev, e2e and updatetest identities never write it.
- **Test:** port `login-item.test.ts` against a registry seam.

### T6.4 Rust updater (spec §3.10; port `updater.ts`)
- **`desktop-host::updater`:**
  - feed `https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml`, production identity only;
  - first check after 60 s, then every 6 h;
  - parses `version`, `path`, `sha512` and `files[]`;
  - downloads to `%TEMP%\tjommie-update-<version>\` and verifies the sha512;
  - refuses an installer whose embedded appId (read from the installer's version resource `AppId` string, which M8 writes) differs from its own;
  - on quit, runs the installer with `/S --updated` through `spawn::command`, after the daemon has stopped.
- **States:** `UpdateState` as in `updater.ts:6-11`.
- **Quit-retry (port `main.ts:119-183`):** if the daemon doesn't stop within 20 s, the update is **not** installed. The window comes back with "Update not installed", and the daemon is restarted or reconnected.
- **Tests:**
  - port `updater.test.ts`;
  - `updater::rejects_bad_sha512`;
  - `rejects_foreign_app_id`;
  - `quit_with_forced_daemon_stop_skips_install` (fake daemon);
  - `dev_identity_has_no_feed` (T0.6).

### T6.5 Speech (spec §3.10; port `speech-runtime.ts`, `speech-model.ts`, `speech-resources.ts`, `speech-permissions.ts`)
- **Runtime:** `whisper-cli.exe` from `resources\speech\`, built in CI by `packages/desktop/scripts/build-speech-runtime.ps1` (unchanged) and checked by digest.
- **Limits:** the same `SPEECH_LIMITS` (16 kHz, 60 s, 65,536-byte chunks, 1,920,000 audio bytes, 131,072 result bytes, 8,192 characters, 120 s timeout).
- **Model:** `ggml-base.en.bin`, 147,964,211 bytes, sha256 `a03779c8…d002`, from the pinned URL. Production keeps Electron's location, `%APPDATA%\Tjommie\speech`, so there is no re-download and a rollback still finds it. Dev uses `<dev root>\speech` (spec §4).
- **Capture:** `cpal` (WASAPI), 16 kHz mono PCM16 (resampled if the device can't). It stops on window blur, as `main.ts:628` does.
- **Microphone permission:** when Windows privacy blocks desktop apps, the status says "Microphone access is off for desktop apps in Windows Settings".
- **Tests:** port `speech-runtime`, `speech-model` and `speech-resources`. Add `speech::stops_on_blur` and `wav_header_16k` (`speech-runtime.ts:38-41`).

### T6.6 Diagnostics, dialogs, local paths (port `diagnostics.ts`, `local-file-path.ts`, `bot-files.ts`, `app-protocol.ts` link rules)
- `diagnostics.get` plus app info (version, identity, GPU adapter, WebView2 runtime version, memory snapshot) and the crash log, saved through a save dialog.
- External links open in the default browser (`ShellExecuteW`) for http and https only.
- **Tests:** port `diagnostics`, `local-file-path` and `bot-files`.

---

## M7 Motion parity and performance (Motion lane, Opus)

### T7.1 Every `motion.html` motion, on tokens
Implement each card in `design/claude-design/aegis/components/motion.html` with the runtime from T2.3:
1. The springs and the clock.
2. Bots feel alive: blink, scan, glance at the composer, needs-you look-up, drowsy, resting.
3. Approve (T4.2).
4. Message arrival: tool rows tick in (snappy), checks draw on, words fade in 34 ms apart (per-run alpha, never per letter), the code block grows, the bottom-anchored glide.
5. Composer: lift, pill to menu, the dictation meter with one spring per bar, the send pop, the send flight (FLIP).
6. Phone drawer: **not on desktop** (the sidebar is persistent). Recorded as n/a.
7. Numbers roll.
8. The stone mark: blink at rest, the light sweep every 4.6 s while any bot works.
9. First open: **phone only**, n/a on desktop.
10. Toast and sheet.
11. Press, hover and lift.
12. Lists reorder and reflow (Recents FLIP, item leaves).
13. Browser panel (T3.7).
14. Shared elements: the avatar flies from an Agents card or a Recents row into the chat header.

- **Done when** `tjommie-rs/motion-map.md` lists every card and every motion inside it, with its token names, its implementing type and its check, or "n/a on desktop: reason".

### T7.2 Motion checks (scenario-driven, as the spike)
- `tjommie-desktop --scenario motion-<name>` (feature `test-driver`) runs 50 pointer-triggered loops.
- **Checks per motion:**
  - p95 frame interval at most 16.7 ms on 3 runs (PresentMon `MsBetweenPresents`, plus the app's own frame log; runs that lose more than 1% of presents are judged on the app log);
  - interruption: the spike report's two rules;
  - keyboard: 0 frames in between;
  - reduced: 0 frames in between, both simulated and real.
- Runs use `run-motion-checks.ps1` with the CPU gate and the covered-screen gate.
- **Test:** `frame-stats.py --self-test`.

### T7.3 Heavy-load experiment
- Measure `AvSetMmThreadCharacteristicsW("Games")` while motion is running (and `SetThreadPriority(ABOVE_NORMAL)` as a second option) against neither. Use the panel and Recents at 60–80% background CPU (a fixed synthetic load: `tools/perf/load.ps1` spins N threads at idle priority).
- Keep an option only if the p95 improves by at least 2 ms on 3 runs with no worse idle CPU. Report either way.

### T7.4 Performance and memory exit runs (owner's PC, quiet)
- Motion p95 for every motion in `motion-map.md`, and the 10,000-message scroll.
- The memory budget states S0–S4 (below).
- Cold start to the first interactive frame. The window is interactive when the shell's first frame is presented and the daemon is connected (log marker `ui ready`). It is measured by PresentMon and the log, and compared with the baseline's 3,837.5 ms (which the spike measured to the first present; both are reported).

---

## Memory budget (gate condition 1; Phase 3 exit criterion)

**Metric:** the primary metric is **private working set**, summed over the app's process tree. **Private commit** is secondary. Summed working set is reported only for continuity with the 496.1 MiB baseline and never decides pass or fail.

**Tool:** `tools/perf/measure-memory.ps1` (T0.3). Every measurement is gated: 60 s idle, the median of 10 samples, not loaded.

**Fixture:** `fixtures/controls.html` from the spike. The bots are on the e2e daemon, in a dev root.

| State | Description | Private WS target | Private commit target |
|---|---|---|---|
| S0 | App + daemon, signed-in shell, **no bot browser ever opened** (0 `msedgewebview2.exe`) | **≤ 120 MiB** | ≤ 160 MiB |
| S1 | One bot's page shown in the panel | **≤ 280 MiB** | ≤ 330 MiB |
| S2 | S1 + a second bot opened, then hidden and idle (suspended) | **≤ S1 + 40 MiB** | ≤ S1 + 50 MiB |
| S3 | S1, then the panel closed with nothing streaming, after 60 s | **≤ S1 − 20 MiB** (the page is suspended, not parked) | — |
| S4 | S2 after the 10 min idle close of the hidden bot | **≤ S1 + 10 MiB** | — |

**And:**
- **Against Electron:** in S0 and S1, the private working set is at most **50%** of the installed production app's, measured by the same script in the same state (owner step D2). It is never worse.
- **Leak check:** 30 panel open/close cycles and 20 bot switches. Private commit grows by **≤ 15 MiB** after a 60 s settle.
- **Hidden bots use no CPU:** a hidden idle bot's renderer uses **≤ 1 s of CPU time over 60 s**.

**Why these numbers.** The research estimates about 45–52 MiB of private WS for the GPUI process and about 110–145 MiB for one bot's WebView2 set (browser, GPU, utility, crashpad and one renderer). An idle Rust daemon adds about 20–40 MiB (estimated; T0.3 measures it). So S0 is estimated at about 70–90 MiB and S1 at about 175–235 MiB. The targets leave about 30–45 MiB of headroom for the real UI's caches (fonts, images, the 10k-message list). The S2 and S3 deltas are the research's per-bot ranges (#3: 20–45 MiB saved per suspended bot; #2: 25–35 MiB per extra opened bot).

**Accepted by the owner as proposed** (owner decision 1, 2026-11-13). That covers the five state targets, the at-most-50% comparison with Electron, the D2 re-measure and the revision rule that follows. If T0.3's re-measure of the spike disagrees with the research estimate by more than 30%, this table is revised from the measured numbers before M3 merges.

---

## M8 Installer, release, rollback (Release lane, then ops)

### T8.1 R1 install facts (can start after T0.1, on a runner)
- **`.github/workflows/desktop-acceptance.yml`**, triggered by `desktop-acceptance-*` branches (modelled on `r1-acceptance.yml`). Step 1 installs the published R1 on a windows runner and records `installer/r1-install-facts.json`:
  - the install folder;
  - the `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{GUID}` values (`DisplayName`, `UninstallString`, `QuietUninstallString`, `InstallLocation`, `DisplayIcon`, `DisplayVersion`, `Publisher`);
  - the `HKCU\Software\<GUID>` install key;
  - the Start-menu and desktop shortcuts with their AppUserModelID property;
  - the Run and StartupApproved entries after first start;
  - the file count.
- **Nothing is guessed.** The NSIS script is written to these facts.

### T8.2 NSIS installer (`installer/tjommie.nsi`)
- Per-user, one-click, and silent with `/S`. It reproduces R1's facts exactly for the production identity: install folder `%LOCALAPPDATA%\Programs\Tjommie`, the same GUID and registry keys, shortcuts with AUMID `com.tsnyders.tjommie`, and `deleteAppDataOnUninstall: false` behaviour (the data root is never touched).
- It accepts electron-updater's arguments (`--updated`, `--force-run`) and relaunches `Tjommie.exe --updated`.
- **Over an existing install:** it closes running `Tjommie.exe` processes in the install folder only, runs the existing uninstaller silently in keep-data mode (as electron-builder's upgrade does; the facts from T8.1 say how), then copies the new files.
- **Its uninstaller** deletes only the install folder's own files and its own registry keys and shortcuts. It **refuses** to run if `InstallLocation` is not under `%LOCALAPPDATA%\Programs\` or doesn't end with the product name. This guards against a repeat of the test uninstaller that once deleted the owner's install.
- **Payload:**
  - `Tjommie.exe` (the `desktop` binary, production identity);
  - `resources\tjommie-daemon.exe`;
  - `resources\speech\whisper-cli.exe` (+ DLLs);
  - `resources\pdfium.dll`;
  - the version resource with `AppId`.
- **Dev and updatetest identities** have a distinct GUID and folder (spec §4).
- **Tests (on the runner):**
  - install fresh, then uninstall: the data root is unchanged (file count and hashes);
  - refuse-guard test with a planted `InstallLocation` of `C:\`.

### T8.3 Release workflow
- **`release.yml` windows job:** build `tjommie-daemon.exe` and `tjommie-desktop.exe` (release, `--locked`, `TJOMMIE_IDENTITY=production`), the speech runtime, then `makensis`. It writes `Tjommie-Setup-<version>.exe` and a `latest.yml` (`version`, `files[{url, sha512, size}]`, `path`, `sha512`, `releaseDate`) exactly as electron-builder's format, which R1's electron-updater reads.
- **It keeps:** verify, `apply-native --check`, the CHANGELOG notes, the `latest.yml` sha512 check, and the android job (the Capacitor APK still ships until Phase 4).
- **Removed:** the Electron packaging step.
- **No blockmap is published.** electron-updater falls back to a full download; T8.4 verifies this.
- **Check:** the `desktop` crate version equals the root `package.json` version.

### T8.4 Acceptance on a runner with R1 installed (spec Phase 3 exit)
**`desktop-acceptance.yml`:**
1. **Update path, real mechanism, test identity.** Install the R1 **updatetest** build. Serve a local generic feed with the Phase 3 **updatetest** `latest.yml` and installer (`TJOMMIE_UPDATE_FEED`, allowed only for the test identity, `main.ts:216-220`). Let R1's electron-updater download it, then quit so it installs. Assert:
   - the new exe runs, with `--updated`;
   - the data root's file count and the database's `user_version = 12` and row counts are unchanged;
   - the uninstall entry points to the new uninstaller;
   - the shortcut's AUMID is unchanged;
   - a notification posts.
2. **Production identity over R1:** install production R1, seed it as `r1-acceptance.yml` does, run the Phase 3 installer with `/S --updated`, and assert the same, plus that the start-at-login entry still launches the app.
3. **Rollback:** reinstall R1 over Phase 3 (`/S`). R1 starts, opens the database and its bots' Electron browser profiles are intact. No migration ran.
4. **The black-box suites** against the packaged test identity: the e2e set from §"Test plan".
- **Only after this passes** is the installer run on the owner's PC (owner step).

### T8.5 Owner parity run and smoke (owner present)
- Every D row on the owner's PC with the dev identity, ticking `tjommie-rs/phase3-checklist.md` (one line per D row, with how it was checked).
- The real-Claude smoke test: copy `C:\Users\tyrel\tmp\orchestrator\smoke-claude-checklist.md` to `tjommie-rs/smoke-desktop-checklist.md`, rewritten for the GPUI app:
  - a Haiku bot on Always ask;
  - checks 1–8 (sign in, create the bot, PONG, a file write that asks, a Bash command that asks, stop mid-turn, steer, quit and relaunch with nothing lost);
  - plus: open the bot's browser and run one tool; the panel opens live; the logins menu works; a notification arrives while the window is hidden; dictation fills a draft.
  - It runs beside the installed app, on `Tjommie-rs` and ports 47200/47201, with a script modelled on `smoke-claude.ps1`.
  - It uses the same red flags in `logs/desktop.log` and `desktop-crash.log`.
- The owner runs the T3.5 checks by hand: steps A, B (including Shift+Tab and Tab past the end) and C. Plus step F: the live panel by eye.

### T8.6 Release (AGENTS.md)
1. Bump all `package.json` versions and the workspace version to **0.6.0** (owner decision 7; R1 is 0.5.0).
2. Add `["0.6.0", <next>]` to `packages/mobile/scripts/apply-native.mjs`.
3. Rename `## Unreleased` to `## 0.6.0 — <date>` and add a fresh empty `## Unreleased`.
4. Ops merges to `main` and pushes `v0.6.0`. GitHub Actions builds and publishes. Never package locally.
- **CHANGELOG lines added here** (if not already added by their tasks):
  - the new look (T5);
  - the crash restart (T0.4);
  - the memory (T3.1);
  - **"Bots' browsers start fresh after this update: sign in to sites again (saved logins still fill in)"** (owner decision 2);
  - the carry-over line from T1.5.
  - "Tjommie is now a native Windows app and much smaller to install."

---

## Rollback
- **Rollback is reinstalling R1** (0.5.x) from GitHub Releases over 0.6.0. To keep that possible:
  - no migration 13 or later;
  - secrets stay byte-compatible (the same Credential Manager targets: device keys, logins, accounts);
  - the Electron browser profiles in `data/native-browser/` are kept through 0.6.x;
  - the speech model stays at `%APPDATA%\Tjommie\speech`;
  - `desktop-ui.json` is new and ignored by R1.
- **One release after 0.6.0:** `native-browser/` may be pruned, after the owner says so.

## Fallback path (gate condition 7)
Both decision points and the 5-day budget are **accepted** by owner decision 8 (2026-11-13).

- **Decision point: the end of M3** (T3.1–T3.6 merged and run on the owner's PC). The browser engine **blocks Phase 3** if any of these fails and cannot be fixed within **5 working days** of focused work:
  - S1 memory;
  - the focus hand-off (Shift+Tab or Tab);
  - the 26 tools;
  - the stream target;
  - the overlay shield;
  - the T3.1 lifecycle.
- **Second decision point: the end of T7.4.** It applies if the panel's live variant **with** its fallback misses 16.7 ms on the quiet PC.
- **Then the owner chooses:** GPUI with a documented exception, or the spec's fallback, the **Electron shell restyled to Aegis**.
- **What carries over to the fallback:**
  - the Rust daemon (unchanged; it is R1's);
  - the protocol, the manifest and the contract tests;
  - the `client` crate (not used by Electron, but kept for Phase 4);
  - `tjommie-rs/design/themes/aegis.json` (a CSS-variable generator, `scripts/aegis-theme.ts --css`, produces `tokens.css` for `packages/ui`);
  - the Aegis prototypes and `motion.html` (web code that runs as-is);
  - the memory measurement tool and its budget (the Electron side gets lazy `WebContentsView`s already);
  - the installer acceptance workflow (re-pointed at electron-builder);
  - the desktop test map.
- **What is lost:** the GPUI UI code (`desktop`) and the WebView2 engine in `desktop-host`. The vault, speech and updater ports stay unused.

## Design deviations (accepted by the owner, decision 6, 2026-11-13)
1. Inline code has no chip radius or padding, and is full size (GPUI limit 1).
2. The page area has square corners (limit 12).
3. Shadows don't scale with app zoom (limit 11).
4. While an overlay is open, the part of it over the page appears 60–100 ms late and the page is frozen (limit 8).
5. The panel slides with a still page when the fallback picks the snapshot variant.

---

## Test plan

### Kinds of test
| Kind | Where | Runs |
|---|---|---|
| Unit (models, springs, updater, vault, speech, lifecycle logic) | `#[cfg(test)]` in each crate | CI and local |
| View tests (GPUI `TestAppContext`, fake `AppStore`) | `crates/desktop/src/ui/**` | CI and local |
| Host integration (WebView2 + fixtures, Win32 parent, no GPUI) | `crates/desktop-host/tests/*.rs` | CI (windows-latest has the WebView2 runtime) and local |
| E2E (test driver + the debug `tjommie-daemon.exe` through `TJOMMIE_DAEMON_EXE`, built with the test-only provider-fixtures feature, on `127.0.0.1:0` with a temp root) | `crates/desktop/tests/e2e_*.rs`, feature `test-driver` | CI and local |
| Cross-implementation (TypeScript ↔ Rust: device keys, login secrets) | `client`, `desktop-host` tests, through `tjommie-rs/tools/cred-probe.ts` | CI and local, test namespace |
| Motion and frame time (PresentMon + app log) | `tools/perf/run-motion-checks.ps1` | owner's PC, quiet |
| Memory budget | `tools/perf/measure-memory.ps1` | owner's PC, quiet; CI runs S0 and S1 as a smoke (not a gate; runners are noisy) |
| Installer, update path, rollback | `desktop-acceptance.yml` | GitHub runner only |
| Owner checks (steps A–C, F, the D-row checklist, the real-Claude smoke) | `phase3-checklist.md`, `smoke-desktop-checklist.md` | owner present |

### The desktop real tests, mapped (finalized in T0.6)
| Electron real test | Phase 3 equivalent |
|---|---|
| `accounts.real` | Owner (real accounts), D row 6 in the checklist |
| `android-companion.real`, `mobile-appearance.real`, `mobile-motion.real`, `phone-layout.real` | n/a: phone, Phase 4 |
| `browser-stream.real` | `desktop-host/tests/browser_stream.rs` (`meets_lan_target`) |
| `desktop.real`, `screens.real` | Never automated (real accounts and paid turns). `screens` becomes `e2e_screens` with fixtures, which writes screenshots to a temp folder, never tracked files |
| `embedded-browser.real`, `native-browser.real` | `desktop-host/tests/browser_*.rs` + `e2e_browser_panel` |
| `packaged.real` | `desktop-acceptance.yml` steps 1–2 (payload check: daemon, speech, pdfium, no Electron files) |
| `remote-bridge.real`, `two-hosts.real` | `e2e_remote_host` (two e2e daemons, host switcher, LAN + local relay) |
| `ui-bughunt.real` | `e2e_ui_bughunt`: its cases re-expressed through the test driver; any case that tests DOM-only behaviour is n/a with a reason |
| `updater.real` | `desktop-acceptance.yml` step 1 (the real electron-updater → Rust installer) + `updater` unit tests |

### §6 D rows: how each is proven
| Row | Automated | Owner |
|---|---|---|
| 1 Bots | `e2e_create_bot_and_chat`, T5.1 view tests | checklist |
| 2 Chat | `e2e_chat_stream_stop_retry_steer` (fixtures) | smoke 3, 6, 7 |
| 3 Approvals | `e2e_approval_ask_folder_full` | smoke 4, 5 |
| 4 Rooms | `e2e_room_mention` | checklist |
| 5 Routines | `e2e_routine_run_history` | checklist |
| 6 Accounts | — | smoke 1 + checklist |
| 7 Usage | `e2e_usage_caps` | checklist |
| 8 Projects | `e2e_project_task_review` | checklist |
| 9 Browser | `browser_tools.rs`, `browser_lifecycle.rs`, `focus_input.rs`, `e2e_browser_panel` | steps A, B, F |
| 10 Remote view + stream | `browser_stream.rs`, `e2e_remote_host` | checklist (phone views the PC) |
| 11 Saved logins | vault tests + cross-implementation | step B8 + checklist |
| 12 Worker isolation | daemon §3.8 harness (unchanged) + `RemoteWorkerIsolation` tests | — |
| 13 Files | T4.4 tests | checklist (PDF, audio, video, 200 MiB) |
| 14 Dictation | `speech` tests | smoke + checklist |
| 15 Pairing, hosts | `e2e_remote_host`, cross-implementation device keys | checklist |
| 17 PC notices | `notify` tests + acceptance step 1 | smoke |
| 18 Settings, diagnostics | `e2e_settings_sections` | checklist |
| 19 Login, tray, update, clean quit | T6.1–T6.4 tests + `desktop-acceptance.yml` | checklist |
| 20 Collaboration tools | daemon (unchanged) | — |
| 22 Appearance, motion | `reduced::*`, T7.2 | step C + checklist |

### The safety rules, applied
- Every e2e and host test gets a temp root from `owner_safety::test_root()`, which asserts the root is under `%TEMP%`.
- Ports are 0 or 47200+, and every bind is `127.0.0.1`.
- `spawn::command` is used everywhere, and the CI `console` check asserts that the process tree has no `conhost.exe` child during the e2e run.
- Credential Manager access only goes under the test namespace, with cleanup.
- No test changes Windows settings.
- No real provider: the e2e daemon runs with `TJOMMIE_PROVIDER_FIXTURES`. If that switch is missing in the daemon, T1.1 adds it as a test-only feature, not a runtime flag.

---

## Feature-parity inventory (nothing dropped silently)

### `packages/ui` screens (`app/`)
| Screen | GPUI | Task |
|---|---|---|
| `page.tsx` (home → Agents or setup) | `Route` default | T2.1 |
| `agents/page.tsx` | `ui/agents` | T5.1 |
| `bots/new/page.tsx`, `bots/settings/page.tsx` | `ui/bots` | T5.1 |
| `chat/page.tsx` | `ui/chat` | T4.1 |
| `room/page.tsx` | `ui/rooms` | T5.2 |
| `projects/page.tsx` | `ui/projects` | T5.3 |
| `settings/page.tsx` | `ui/settings` | T5.5 |
| `layout.tsx` | `ui/shell` | T2.1 |

### `packages/ui` components
| Components | GPUI | Task |
|---|---|---|
| `app-shell`, `rail`, `tab-bar`, `panel-bar`, `back-link`, `host-switcher`, `connection-screen`, `failed-load` | `ui/shell` | T2.1 |
| `dialog`, `menu`, `pill`, `icon-button`, `controls`, `bot-avatar` | `ui/primitives` | T2.2 |
| `press-fixture` | n/a: a test fixture; press is covered by T7.1 item 11 | — |
| `mobile-connect` | Settings, Devices (the pairing QR for phones) | T5.5 |
| `accounts/account-card`, `account-dialogs`, `accounts-section`, `limit-meter` | `ui/settings/accounts` | T5.5 |
| `agents/cards` | `ui/agents` | T5.1 |
| `bot-form/bot-form`, `identity`, `model-row`, `rows` | `ui/bots` | T5.1 |
| `chat/chat-screen`, `chat-header`, `message-list`, `message-view`, `bubbles`, `activity-row`, `handoff-row`, `notices`, `setup-banner`, `sign-in-toast` | `ui/chat` | T4.1 |
| `chat/approval-card` | `ui/chat/approval` | T4.2 |
| `chat/composer`, `dictation-button` | `ui/chat/composer` | T4.3 |
| `chat/file-card`, `file-thumbnail`, `file-viewer`, `markdown`, `markdown-file-viewer`, `pdf-file-viewer` | `ui/files` | T4.4 |
| `chat/browser-panel`, `browser-thumbnail`, `browser-logins`, `right-panel`, `bot-panel` | `ui/browser` | T3.7 |
| `devices/devices-section`, `pair-dialog`, `pairing`, `relay-row`, `this-phone`, `use-devices` | `ui/settings/devices` (`this-phone` → "This PC") | T5.5 |
| `exchange/exchange-view` | `ui/chat` | T4.1 |
| `general/desktop-app` | `ui/settings/desktop_app` | T5.5 |
| `google/google-connection` | `ui/settings/google` ("not enabled") | T5.5 |
| `projects/project-editor`, `task-detail`, `task-diff`, `task-editor` | `ui/projects` | T5.3 (diff view in T4.4) |
| `room/members-panel`, `room-composer`, `room-form`, `room-post`, `room-screen`, `rooms-list` | `ui/rooms` | T5.2 |
| `routines/routine-editor`, `routine-rows` | `ui/routines` | T5.4 |
| `settings/appearance-section`, `device-settings`, `diagnostics-section`, `diagnostics`, `logins-section`, `save-report`, `section`, `use-system-info`, `voice-section` | `ui/settings` | T5.5 |
| `setup/provider-cards` | `ui/setup` | T5.5 |
| `usage/provider-usage`, `usage-section` | `ui/settings/usage` | T5.5 |
| `packages/ui/lib/*` | `model/*`, or n/a with a reason | T1.4 |

### Electron main process (`packages/desktop/src`)
| Feature (source) | Rust | Task |
|---|---|---|
| Single instance, second launch shows the window (`main.ts:113-118`) | `shell::single_instance` | T0.5 |
| AppUserModelID (`:117`) | installer shortcut + `notify` | T8.2, T6.2 |
| Window: hidden title bar with caption overlay and themed, scrim-dimmed caption buttons (`:601-615`, `:676-682`) | GPUI client-side caption buttons, dimmed by the overlay scrim | T0.5, T2.2 |
| Close hides to the tray; first-close hint (`:649-654`, `:704-711`) | `shell::tray` | T6.1 |
| Daemon spawn, token, restarts 3/min, dev token (`:263-387`, `daemon-process.ts`) | `daemon_process` | T1.2 |
| Clean quit: lock vault, close engine, stop daemon 20 s, quit-retry dialog (`:119-183`) | `updater` + `daemon_process` | T6.4 |
| Notifications with ack after OS show, click to route (`:389-437`, `notification-delivery.ts`) | `shell::notify` | T6.2 |
| Tray with update item (`:684-695`, `updater.ts`) | `shell::tray` | T6.1 |
| Start at login, `--hidden` / `--updated` (`login-item.ts`) | `shell::login_item` | T6.3 |
| electron-updater (`updater.ts`) | `updater` | T6.4 |
| Native browser engine (`native-browser.ts`, `-access`, `-policy`, `-permissions`, `-profiles`) | `browser::*` | T3.1, T3.2 |
| 26 tools (`native-browser-automation.ts`) | `browser::automation` | T3.3 |
| Stream and frames (`native-browser-frame.ts`) | `browser::stream` | T3.4 |
| `/browser-host` (`native-browser-host.ts`) | `browser::host_bridge` | T3.2 |
| Shield and scrim (`native-browser.ts:719`, `:765-808`; `dialog:scrim`) | `BrowserShield` | T3.5 |
| Shift+Tab focus return (`native-browser.ts:693-697`) | isolated world + `MoveFocusRequested` | T3.5 |
| Login vault, storage, entry window, command gate, `/vault-host`, worker isolation (`login-vault`, `vault-*`, `worker-isolation`, `native-browser-login`) | `vault::*`, `ui/vault_entry` | T3.6 |
| Remote wires to other PCs, device keys (`remote-wire.ts`, `device-keys.ts`) | `client::remote`, `device_keys` | T1.3 |
| Attachments: `app://` serving, previews, save (`attachment-protocol.ts`, `attachment-save.ts`, `local-device-bridge.ts`) | direct `attachments.*` reads; save dialog | T4.4 |
| Speech runtime, model, resources, permissions (`speech-*.ts`) | `speech::*` | T6.5 |
| Diagnostics info and save (`diagnostics.ts`) | `diagnostics` | T6.6 |
| File and folder pickers, open folder, show in folder, open notes (`:565-598`, `bot-files.ts`, `local-file-path.ts`) | GPUI prompts + `ShellExecuteW` | T4.4, T6.6 |
| External links, navigation policy, CSP (`app-protocol.ts`, `:55-57`, `:655-666`) | `markdown` link rule + `ShellExecuteW` (no web content in the app, so no CSP) | T4.4 |
| Theme to the title bar (`theme:set`) | GPUI theme | T2.1 |
| `app:info` username (`:589`) | `std::env` / `GetUserNameW` | T5.5 |
| Google consent (`google-consent.ts`) | n/a: Google is not enabled in production (§6 row 21); "not enabled" only | T5.5 |
| Owner safety (`owner-safety.ts`) | `owner_safety` | T0.6 |
| `preload.cjs`, `vault-entry-preload.cjs` | n/a: there is no renderer; the calls are direct | — |
| Test fixtures (`*-fixture.ts`, `quit-retry.fixture.cjs`, `real-app.ts`, `native-window-capture.ps1`, `browser-stream-stats.ts`) | spike fixtures + the test driver + `tools/perf` | T0.5, T3.x |
| Logging to `logs/desktop.log` (`:96-103`) | `desktop-host::log` (the same file) | T0.4 |

---

## Owner steps in this phase
**Still open:**
1. **`/design-sync`:** upload the Aegis design to Claude Design, then add the link to spec Phase 0 ("Claude Design link").
2. **D2:** re-measure the installed app's private memory with the fixed script, after T0.3. The budget's 30% revision rule depends on it.

**Later in the phase:**
3. The quiet-PC runs in T7.4. Steps A, B, C and F in T8.5, plus the keyboard-only checklist pass (T5.6).
4. The real-Claude smoke test, and the D-row checklist.
5. Let the 0.5.x export patch (T1.6) install on the PC at least 7 days before 0.6.0.
6. The installer on the PC after `desktop-acceptance.yml` passes. Then the release.

## Exit criteria additions (from the owner's decisions)
- **The memory budget**, as accepted (decision 1).
- **Full keyboard navigation:** `e2e_keyboard_navigation` passes, and the owner's keyboard-only checklist pass is done (decision 4).
- **0.5.x with the export** was published at least 7 days before 0.6.0 (decision 3).

## Tracked follow-ups (after 0.6.0)
- **Screen-reader support** (decision 4). Owner-accepted for the first GPUI release only. Track it as an issue: "Desktop: screen reader support (UI Automation)". Revisit at every GPUI pin upgrade, and do it upstream if GPUI gains an accessibility layer, or with a UI Automation provider fed from the `test_id` semantic tree. Spec §9 lists it.
- **Prune `data/native-browser/`** one release after 0.6.0, after the owner says so (§"Rollback").

## Risks
- **Frame pacing under load.** One UI thread does everything (limit 7). The panel misses at 100% CPU even with the fallback. Mitigations: the fallback (T3.7), MMCSS if T7.3 shows a gain, and the honest limits in the docs.
- **The installer takeover.** A wrong GUID, folder or uninstaller step strands the owner on Electron or deletes files. Mitigations: the T8.1 facts, the uninstaller guard, runner-only tests, and the update-path test through the real electron-updater.
- **Pre-1.0 churn:** the pins and the upgrade policy.
- **The focus hand-off mechanism** depends on CDP isolated-world bindings in WebView2. There is a fallback (`MoveFocusRequested` only). If the T3.5 test shows the binding is unreachable, the owner decides then.
- **The media viewer** adds a second WebView2 use. It is lazy, closed after use, and inside the budget only while open.
- **No screen reader** at this pin. It was accepted, with full keyboard navigation as the condition (T5.6), and is tracked as a follow-up.
- **Installs that skip 0.5.x** don't carry over paired PCs or appearance. They fall back to the defaults (T1.5), and the CHANGELOG covers it.
- **The scale of the hand-written UI:** about 85 components ported by hand. The lanes are sized for that, and the inventory makes gaps visible.
- **PresentMon loses presents during the panel motion:** the app's frame log is the second measure (T2.3).

## Owner decisions (resolved 2026-11-13)
The owner answered all 8 open questions. Nothing remains open.

1. **Memory budget: accepted as proposed.** That means S0 ≤ 120, S1 ≤ 280, S2 ≤ S1 + 40, S3 ≤ S1 − 20 and S4 ≤ S1 + 10 MiB of private working set. It also includes at most 50% of Electron's in the same state, the D2 re-measure and the 30% revision rule. See §"Memory budget".
2. **Bots sign in to sites again after the switch: accepted.** There is no cookie migration, and saved logins still fill. CHANGELOG in T8.6.
3. **Paired PCs and appearance: option (a).** A 0.5.x Electron patch exports them, and the new app imports them once. That patch is T1.6, on R1's patch line, with its own test and CHANGELOG line. The import is T1.5.
4. **No screen reader at this GPUI pin: accepted for the first GPUI release, provided full keyboard navigation works.** That is T5.6 and an exit criterion. Screen-reader support is a tracked follow-up.
5. **Audio and video previews: the built-in WebView2 player** (T4.4).
6. **Design deviations 1–5: accepted** (§"Design deviations").
7. **The desktop-UI release is version 0.6.0.** R1 is 0.5.0 (T8.6).
8. **Fallback decision points: accepted.** They are the end of M3 plus 5 working days, and the end of the motion exit runs (T7.4). See §"Fallback path".
