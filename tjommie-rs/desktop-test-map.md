# Desktop test map (Phase 3 plan T0.6)

This file has two parts: the first maps every TypeScript test file in `packages/ui` and `packages/desktop/src` to its Rust test (T0.6); Part 2 maps each `packages/ui/lib` test case to its `model::` test (T1.4).

This map covers every TypeScript test file in the `packages/ui` tree (the desktop UI) and in `packages/desktop/src` (the Electron main process). Each file is mapped to the Rust test that will cover the same behaviour in the GPUI desktop, or marked `n/a:` with a reason; the owning plan task writes that Rust test. The row counts in the Counts section must equal the `git ls-files` lists, and each path appears in exactly one row.

Location kinds: `(unit)` is a `#[cfg(test)]` module, `(view)` is a GPUI `TestAppContext` view test, `(e2e)` is a test-driver file under `crates/desktop/tests/`. The crate and folder come from the plan; file names inside a folder are derived where the plan names none.

## packages/ui (112 files)

| TypeScript test | Rust test | Task | Notes |
|---|---|---|---|
| `packages/ui/components/accounts/accounts.test.tsx` | `crates/desktop/src/ui/settings/accounts.rs` (view) | T5.5 | Accounts: sign-in, check, limits (row 6). |
| `packages/ui/components/agents/cards.test.tsx` | `crates/desktop/src/ui/agents/cards.rs` (view) | T5.1 | Needs-you cards. |
| `packages/ui/components/app-shell.test.tsx` | `crates/desktop/src/ui/shell/mod.rs` (view) | T2.1 | `shell::the_connection_screen_takes_the_main_columns_place`, `shell::navigates_to_chat_from_recents`, `e2e_navigation`; the status pill in `shell/connection.rs` (unit), `Connection::blocks` in `shell/data.rs` (unit). |
| `packages/ui/components/back-link.test.tsx` | `n/a: phone only (the back link shows below 640 px; the desktop has the sidebar)` | - | Ruled in T2.1. |
| `packages/ui/components/bot-form/identity.test.tsx` | `crates/desktop/src/ui/bots/identity.rs` (view) | T5.1 | Bot identity fields. |
| `packages/ui/components/bot-form/phone.test.tsx` | `n/a: phone only (Phase 4, Android Compose)` | - | Bot form stacked for a phone screen. |
| `packages/ui/components/chat/approval-card.test.tsx` | `crates/desktop/src/ui/chat/approval.rs` (view) | T4.2 | `approval::keyboard_snaps`, `approval::interrupt_undo_halfway`. |
| `packages/ui/components/chat/bot-panel.test.tsx` | `crates/desktop/src/ui/browser/bot_panel.rs` (view) | T3.7 | The bot's browser panel. |
| `packages/ui/components/chat/browser-panel.test.tsx` | `crates/desktop/src/ui/browser/panel.rs` (view) | T3.7 | `panel::keyboard_toggle_snaps`, `e2e_panel_open_close`. |
| `packages/ui/components/chat/bubbles.test.tsx` | `crates/desktop/src/ui/chat/bubbles.rs` (view) | T4.1 | Corner radii 20, 20, 6, 20. |
| `packages/ui/components/chat/chat-header.test.tsx` | `crates/desktop/src/ui/chat/header.rs` (view) | T4.1 | The bot avatar flies into this header (T7.1 item 14). |
| `packages/ui/components/chat/composer.test.tsx` | `crates/desktop/src/ui/chat/composer.rs` (view) | T4.3 | `composer::enter_sends_once`, `composer::placeholder_body_size`. |
| `packages/ui/components/chat/dictation-button.test.tsx` | `crates/desktop/src/ui/chat/composer.rs` (view) | T4.3 | `composer::dictation_never_sends`; audio capture is T6.5. |
| `packages/ui/components/chat/draft-keeping.test.ts` | `crates/desktop/src/ui/chat/composer.rs` (view) | T4.3 | Also covers the exchange reply (T4.1) and routine editor close (T5.4). The phone length cap is n/a. |
| `packages/ui/components/chat/file-card-download.test.tsx` | `crates/desktop/src/ui/files/card.rs` (view) | T4.4 | Download uses the save path in `model/attachment_transfer.rs`. |
| `packages/ui/components/chat/file-card.test.tsx` | `crates/desktop/src/ui/files/card.rs` (view) | T4.4 | - |
| `packages/ui/components/chat/file-thumbnail.test.tsx` | `crates/desktop/src/ui/files/thumbnail.rs` (view) | T4.4 | Images are drawn with GPUI `img()`. |
| `packages/ui/components/chat/file-viewer-download.test.tsx` | `crates/desktop/src/ui/files/viewer.rs` (view) | T4.4 | Save from the viewer. |
| `packages/ui/components/chat/file-viewer.test.tsx` | `crates/desktop/src/ui/files/viewer.rs` (view) | T4.4 | - |
| `packages/ui/components/chat/handoff-row.test.tsx` | `crates/desktop/src/ui/chat/rows.rs` (view) | T4.1 | Hand-off row. |
| `packages/ui/components/chat/markdown-file-viewer.test.tsx` | `crates/desktop/src/ui/files/markdown.rs` (view) | T4.4 | - |
| `packages/ui/components/chat/markdown.test.tsx` | `crates/desktop/src/ui/files/markdown.rs` (view) | T4.4 | `markdown::blocks_non_web_links`. |
| `packages/ui/components/chat/message-view.test.tsx` | `crates/desktop/src/ui/chat/message_view.rs` (view) | T4.1 | - |
| `packages/ui/components/chat/notices.test.tsx` | `crates/desktop/src/ui/chat/notices.rs` (view) | T4.1 | The crash notice from T0.4 also lands here. |
| `packages/ui/components/chat/pdf-file-viewer.test.tsx` | `crates/desktop/src/ui/files/pdf.rs` (view) | T4.4 | Range reads are `pdf::range_reads_only` in desktop-host. |
| `packages/ui/components/chat/right-panel.test.tsx` | `crates/desktop/src/ui/browser/right_panel.rs` (view) | T3.7 | - |
| `packages/ui/components/chat/setup-banner.test.tsx` | `crates/desktop/src/ui/chat/setup_banner.rs` (view) | T4.1 | - |
| `packages/ui/components/chat/sign-in-toast.test.tsx` | `crates/desktop/src/ui/chat/sign_in_toast.rs` (view) | T4.1 | - |
| `packages/ui/components/connection-screen.test.tsx` | `crates/desktop/src/ui/shell/connection.rs` (unit, view) | T2.1 | Every case but the phone's Pair again (Android only). |
| `packages/ui/components/controls.test.tsx` | `crates/desktop/src/ui/primitives/controls.rs` (view) | T2.2 | Pill, IconButton, Toggle. |
| `packages/ui/components/devices/devices.test.tsx` | `crates/desktop/src/ui/settings/devices.rs` (view) | T5.5 | Devices section (row 15). |
| `packages/ui/components/devices/pairing.test.ts` | `crates/desktop/src/ui/settings/devices.rs` (unit) | T5.5 | Pair-code check and countdown; pure helpers. |
| `packages/ui/components/devices/relay-row.test.tsx` | `crates/desktop/src/ui/settings/devices.rs` (view) | T5.5 | Relay row. |
| `packages/ui/components/devices/this-phone.test.tsx` | `crates/desktop/src/ui/settings/devices.rs` (view) | T5.5 | Shown as "This PC". |
| `packages/ui/components/dialog.test.tsx` | `crates/desktop/src/ui/primitives/dialog.rs` (view) | T2.2 | `dialog::escape_closes_without_frames`. |
| `packages/ui/components/exchange/exchange-composer.test.tsx` | `crates/desktop/src/ui/chat/exchange.rs` (view) | T4.1 | - |
| `packages/ui/components/general/desktop-app.test.tsx` | `crates/desktop/src/ui/settings/desktop_app.rs` (view) | T5.5 | Start at login and updates (row 19); the logic is in T6.3 and T6.4. |
| `packages/ui/components/google/google-connection.test.tsx` | `crates/desktop/src/ui/settings/google.rs` (view) | T5.5 | Only the "not enabled" state (row 21). |
| `packages/ui/components/host-gate.test.tsx` | `crates/desktop/src/ui/shell/host_switcher.rs` (view) | T2.1 | Gating when the window is on another PC. Its cases also touch T4.3, T5.1 and T5.5. |
| `packages/ui/components/host-switcher.test.tsx` | `crates/desktop/src/ui/shell/sidebar.rs` (unit) | T2.1 | `hostRows`, `chooseHost` and the button; the phone's Switch PC pill and phone list are n/a. |
| `packages/ui/components/mobile-connect.test.tsx` | `crates/desktop/src/ui/settings/devices.rs` (view) | T5.5 | Pairing QR, shown under Devices. |
| `packages/ui/components/projects/project-workspace.test.tsx` | `crates/desktop/src/ui/projects/workspace.rs` (view) | T5.3 | - |
| `packages/ui/components/projects/task-detail.test.tsx` | `crates/desktop/src/ui/projects/task_detail.rs` (view) | T5.3 | - |
| `packages/ui/components/projects/task-diff.test.tsx` | `crates/desktop/src/ui/files/diff.rs` (view) | T4.4 | The diff view is T4.4 in the plan's inventory. |
| `packages/ui/components/room/room-composer.test.tsx` | `crates/desktop/src/ui/rooms/composer.rs` (view) | T5.2 | Mention keys are shared with T2.4 and T4.3. |
| `packages/ui/components/room/room-form.test.tsx` | `crates/desktop/src/ui/rooms/form.rs` (view) | T5.2 | - |
| `packages/ui/components/room/room-screen.test.tsx` | `crates/desktop/src/ui/rooms/screen.rs` (view) | T5.2 | Also `e2e_room_mention`. |
| `packages/ui/components/room/rooms-list.test.ts` | `crates/desktop/src/ui/rooms/list.rs` (view) | T5.2 | - |
| `packages/ui/components/safe-area.test.ts` | `n/a: phone only (Phase 4, Android safe-area insets)` | - | - |
| `packages/ui/components/settings-list.test.ts` | `n/a: phone only (Phase 4, phone Settings list)` | - | The desktop Settings list is T5.5. |
| `packages/ui/components/settings/device-settings.test.tsx` | `crates/desktop/src/ui/settings/appearance.rs` (view) | T5.5 | Appearance controls; saving is T1.5. |
| `packages/ui/components/settings/diagnostics-section.test.tsx` | `crates/desktop/src/ui/settings/diagnostics.rs` (view) | T5.5 | Save report (row 18). |
| `packages/ui/components/settings/diagnostics.test.ts` | `crates/desktop-host/src/diagnostics.rs` (unit) | T6.6 | Report privacy: versions and health categories only. |
| `packages/ui/components/settings/logins-section.test.tsx` | `crates/desktop/src/ui/settings/logins.rs` (view) | T5.5 | Logins list (row 11); the vault is T3.6. |
| `packages/ui/components/settings/save-report.test.ts` | `crates/desktop-host/src/diagnostics.rs` (unit) | T6.6 | Save through the system picker; cancel; size limit. |
| `packages/ui/components/settings/settings-page.test.tsx` | `crates/desktop/src/ui/settings/mod.rs` (view) | T5.5 | Also `e2e_settings_sections`. |
| `packages/ui/components/settings/voice-section.test.tsx` | `crates/desktop/src/ui/settings/voice.rs` (view) | T5.5 | Voice (row 14). |
| `packages/ui/components/tab-bar.test.tsx` | `n/a: phone only (the floating tab bar; the desktop has the sidebar)` | - | Ruled in T2.1. |
| `packages/ui/components/usage/provider-usage.test.tsx` | `crates/desktop/src/ui/settings/usage.rs` (view) | T5.5 | Usage and limit meter (row 7); also `e2e_usage_caps`. |
| `packages/ui/components/usage/usage-section.test.tsx` | `crates/desktop/src/ui/settings/usage.rs` (view) | T5.5 | - |
| `packages/ui/lib/accounts.test.ts` | `crates/desktop/src/model/accounts.rs` (unit) | T1.4 | - |
| `packages/ui/lib/activity.test.ts` | `crates/desktop/src/model/activity.rs` (unit) | T1.4 | - |
| `packages/ui/lib/appearance-settings.test.ts` | `crates/desktop-host/src/device_settings.rs` (unit) | T1.5 | Device appearance: defaults, local save, blocked storage (`device_settings::*`). |
| `packages/ui/lib/approval.test.ts` | `crates/desktop/src/model/approval.rs` (unit) | T1.4 | - |
| `packages/ui/lib/attachment-device.test.ts` | `n/a: Electron trusted-renderer bridge; replaced by direct calls and GPUI save prompts` | T4.4, T6.6 | Save authorisation moves to the save dialog path. |
| `packages/ui/lib/attachment-transfer.test.ts` | `crates/desktop/src/model/attachment_transfer.rs` (unit) | T1.4 | - |
| `packages/ui/lib/attachments.test.ts` | `crates/desktop/src/model/attachments.rs` (unit) | T1.4 | - |
| `packages/ui/lib/bot-form.test.ts` | `crates/desktop/src/model/bot_form.rs` (unit) | T1.4 | - |
| `packages/ui/lib/browser-geometry.test.ts` | `crates/desktop/src/model/browser_geometry.rs` (unit) | T1.4 | Address to URL (DuckDuckGo default); key routes. |
| `packages/ui/lib/browser-preview.test.ts` | `crates/desktop/src/model/browser_preview.rs` (unit) | T1.4 | - |
| `packages/ui/lib/browser-session.test.ts` | `crates/desktop/src/model/browser_session.rs` (unit) | T1.4 | - |
| `packages/ui/lib/connection-host.test.ts` | `crates/client/src/hosts.rs` (unit) | T1.3 | Which saved PC the window is on; forgetting a removed PC. |
| `packages/ui/lib/connection-start.test.ts` | `crates/client/src/connection.rs` (unit) | T1.3 | First-connection retries, Reconnecting, at most 16 calls in flight. |
| `packages/ui/lib/connection.heartbeat.test.ts` | `crates/client/src/connection.rs` (unit) | T1.3 | - |
| `packages/ui/lib/connection.resume.test.ts` | `crates/client/src/connection.rs` (unit) | T1.3 | - |
| `packages/ui/lib/connection.test.ts` | `crates/client/src/connection.rs` (unit) | T1.3 | - |
| `packages/ui/lib/dependencies.test.ts` | `n/a: npm workspace range check; Cargo pins replace it` | T0.1 | Checks a zod range; the Rust app has no zod. |
| `packages/ui/lib/dictation.test.ts` | `crates/desktop/src/model/dictation.rs` (unit) | T1.4 | Dictation state; capture is T6.5. |
| `packages/ui/lib/document-ranges.test.ts` | `crates/desktop/src/model/document_ranges.rs` (unit) | T1.4 | Used by the PDF and file reads (T4.4). |
| `packages/ui/lib/exchange.test.ts` | `crates/desktop/src/model/exchange.rs` (unit) | T1.4 | - |
| `packages/ui/lib/file-thumbnails-retirement.test.ts` | `n/a: native preview-handle retirement; GPUI img() decodes in-process, so there is no handle to retire` | T4.4 | - |
| `packages/ui/lib/file-thumbnails.test.ts` | `crates/desktop/src/model/file_thumbnails.rs` (unit) | T1.4 | - |
| `packages/ui/lib/format.test.ts` | `crates/desktop/src/model/format.rs` (unit) | T1.4 | - |
| `packages/ui/lib/google-connection.test.ts` | `crates/desktop/src/model/google_connection.rs` (unit) | T1.4 | Only the "not enabled" state. |
| `packages/ui/lib/keep-alive.test.ts` | `n/a: phone only (Phase 4, Android keep-alive)` | - | - |
| `packages/ui/lib/local-speech.test.ts` | `n/a: Electron speech bridge; replaced by the M6 speech runtime` | T6.5 | - |
| `packages/ui/lib/logins.test.ts` | `crates/desktop/src/model/logins.rs` (unit) | T1.4 | - |
| `packages/ui/lib/mentions.test.ts` | `crates/desktop/src/model/mentions.rs` (unit) | T1.4 | - |
| `packages/ui/lib/mobile-identity.test.ts` | `n/a: phone only (Phase 4, mobile identity)` | - | - |
| `packages/ui/lib/mobile-style.test.ts` | `n/a: phone only (Phase 4, mobile style)` | - | - |
| `packages/ui/lib/native-browser-session.test.ts` | `crates/desktop/src/model/native_browser_session.rs` (unit) | T1.4 | - |
| `packages/ui/lib/native-capability.test.ts` | `n/a: phone only (Phase 4, Capacitor native plugins)` | - | - |
| `packages/ui/lib/native-overlay.test.ts` | `n/a: Electron scrim bridge; replaced by the BrowserShield` | T3.5 | Covered by `shield_snapshot_before_hide` and `menu::raises_shield`. |
| `packages/ui/lib/overview.test.ts` | `crates/desktop/src/model/overview.rs` (unit) | T1.4 | - |
| `packages/ui/lib/panel-layout.test.ts` | `crates/desktop/src/model/panel_layout.rs` (unit) | T1.4 | - |
| `packages/ui/lib/phone-nav.test.ts` | `n/a: phone only (Phase 4, phone navigation)` | - | - |
| `packages/ui/lib/projects.test.ts` | `crates/desktop/src/model/projects.rs` (unit) | T1.4 | - |
| `packages/ui/lib/push.lifecycle.test.ts` | `n/a: phone only (Phase 4, Android push)` | - | Push is not in the desktop plan; phone only. |
| `packages/ui/lib/push.test.ts` | `n/a: phone only (Phase 4, Android push)` | - | - |
| `packages/ui/lib/relay-wire.test.ts` | `crates/client/src/remote.rs` (unit) | T1.3 | Relay endpoint framing. |
| `packages/ui/lib/remote-hosts.test.ts` | `crates/client/src/hosts.rs` (unit) | T1.3 | Host list, at most 16. |
| `packages/ui/lib/routine-form.test.ts` | `crates/desktop/src/model/routine_form.rs` (unit) | T1.4 | - |
| `packages/ui/lib/setup.test.ts` | `crates/desktop/src/model/setup.rs` (unit) | T1.4 | - |
| `packages/ui/lib/state.test.ts` | `crates/desktop/src/model/state.rs` (unit) | T1.4 | Event-fed `AppStore` state. |
| `packages/ui/lib/suggestions.test.ts` | `crates/desktop/src/model/suggestions.rs` (unit) | T1.4 | - |
| `packages/ui/lib/theme.test.ts` | `crates/desktop/src/theme.rs` (unit) and `crates/desktop/tests/theme_keys.rs` | T0.2 | Startup defaults. Saved choices are T1.5; follow-system is T5.5. |
| `packages/ui/lib/timeline.test.ts` | `crates/desktop/src/model/timeline.rs` (unit) | T1.4 | - |
| `packages/ui/lib/usage.test.ts` | `crates/desktop/src/model/usage.rs` (unit) | T1.4 | - |
| `packages/ui/lib/use-dictation.test.tsx` | `crates/desktop/src/model/dictation.rs` (unit) | T4.3 | Hook logic moves into the model. Android permission cases are n/a (phone). |
| `packages/ui/lib/use-draft.test.ts` | `crates/desktop/src/model/attachments.rs` (unit) and `crates/desktop/src/ui/chat/composer.rs` (view) | T4.3 | Attachments across a send. `scrollToShow` cases go to the T4.1 list; `ifAlive` is GPUI entity lifetime. |
| `packages/ui/lib/use-sheet.test.ts` | `crates/desktop/src/ui/primitives/sheet.rs` (view) | T2.2 | Escape cases. Android Back cases are n/a (phone). |
| `packages/ui/visual/provider-usage.real.test.ts` | `None: owner run with real accounts and paid turns; never automated` | T5.5 | Same treatment as `accounts.real`. The plan does not map this file; row 7 (Usage) in the checklist is the closest owner check. |

## packages/desktop/src (64 files)

| TypeScript test | Rust test | Task | Notes |
|---|---|---|---|
| `packages/desktop/src/accounts.real.test.ts` | `None: owner run (real accounts)` | T5.5 | Plan: owner, real accounts, D row 6 in the checklist; smoke check 1 covers sign-in. |
| `packages/desktop/src/android-companion.real.test.ts` | `n/a: phone only (Phase 4, Android companion)` | - | - |
| `packages/desktop/src/app-protocol.test.ts` | `n/a: Electron-specific (app:// static export and navigation); the app has no web content` | T4.4 | The external-link rule is covered by `markdown::blocks_non_web_links`. |
| `packages/desktop/src/attachment-protocol.test.ts` | `n/a: Electron-specific (app:// protocol and media-element bridge); replaced by direct attachments.* range reads` | T4.4 | Range reads are `pdf::range_reads_only` and the attachment reads in T4.4. |
| `packages/desktop/src/attachment-save-opening.test.ts` | `crates/desktop/src/model/attachment_transfer.rs` (unit) | T4.4 | Waits for cancelled opens, reports deletion failure, keeps the partial file for retry. |
| `packages/desktop/src/attachment-save.test.ts` | `crates/desktop/src/model/attachment_transfer.rs` (unit) | T4.4 | Streams the chunks to the chosen file (port of attachment-save). |
| `packages/desktop/src/bot-files.test.ts` | `crates/desktop-host/src/paths.rs` (unit) | T6.6 | Open notes action: opens only a bot's own file. |
| `packages/desktop/src/browser-stream-stats.test.ts` | `crates/desktop-host/tests/browser_stream.rs` (summary helper) | T3.4 | Stream summary against the 55 fps and 34 ms targets. |
| `packages/desktop/src/browser-stream.real.test.ts` | `crates/desktop-host/tests/browser_stream.rs` (`stream::meets_lan_target`) | T3.4 | Per the plan's real-test table. |
| `packages/desktop/src/credential-transport.test.ts` | `crates/desktop-host/src/device_keys.rs` (unit) and the cross-implementation test | T1.3 | Namespace derivation and byte-identical device keys. Login secrets are covered by the T3.6 cross-implementation test. |
| `packages/desktop/src/daemon-process-tree-kill.test.ts` | `crates/desktop-host/src/daemon_process.rs` (unit) | T1.2 | Tree kill on shutdown timeout and on startup timeout. |
| `packages/desktop/src/daemon-process.test.ts` | `crates/desktop-host/src/daemon_process.rs` (unit) | T1.2 | Includes `restart_limit_three_per_minute`. |
| `packages/desktop/src/desktop.real.test.ts` | `None: never automated (real accounts and paid turns)` | - | Per the plan's real-test table. The owner smoke and checklist (T8.5) cover the flows. |
| `packages/desktop/src/device-keys.test.ts` | `crates/desktop-host/src/device_keys.rs` (unit) | T1.3 | Name and size rules; cross-implementation test with `tools/cred-probe.ts`. |
| `packages/desktop/src/diagnostics.test.ts` | `crates/desktop-host/src/diagnostics.rs` (unit) | T6.6 | Diagnostics info (version, identity, GPU, WebView2) and the crash log. |
| `packages/desktop/src/embedded-browser.real.test.ts` | `crates/desktop-host/tests/browser_*.rs` and `crates/desktop/tests/e2e_browser_panel.rs` (e2e) | T3.1-T3.5 | Per the plan's real-test table. |
| `packages/desktop/src/google-consent.test.ts` | `n/a: Google is not enabled in production (row 21); only the "not enabled" state is ported` | T5.5 | - |
| `packages/desktop/src/local-device-bridge.test.ts` | `n/a: Electron trusted-renderer bridge; replaced by direct calls` | T4.4, T6.5, T6.6 | Native preview retirement, save authorisation and speech fences move into the direct calls and their owning tasks. |
| `packages/desktop/src/local-file-path.test.ts` | `crates/desktop-host/src/paths.rs` (unit) | T6.6 | Rejects UNC and device paths before any lookup. |
| `packages/desktop/src/login-item.test.ts` | `crates/desktop-host/src/shell/login_item.rs` (unit) | T6.3 | Registry seam; value name and `--hidden`. |
| `packages/desktop/src/login-vault.test.ts` | `crates/desktop-host/src/vault/` (unit) | T3.6 | Logins database and Credential Manager secrets. |
| `packages/desktop/src/mobile-appearance.real.test.ts` | `n/a: phone only (Phase 4)` | - | - |
| `packages/desktop/src/mobile-motion.real.test.ts` | `n/a: phone only (Phase 4)` | - | - |
| `packages/desktop/src/native-browser-access.test.ts` | `crates/desktop-host/src/browser/access.rs` (unit) | T3.2 | One controller; stale results fenced; private after hide. Location derived. |
| `packages/desktop/src/native-browser-automation.test.ts` | `crates/desktop-host/src/browser/automation.rs` (unit) and `crates/desktop-host/tests/browser_tools.rs` | T3.3 | One test per tool checks its result shape. |
| `packages/desktop/src/native-browser-frame.test.ts` | `crates/desktop-host/src/browser/stream.rs` (unit) | T3.4 | Snapshots and thumbnails from `Page.captureScreenshot`. |
| `packages/desktop/src/native-browser-host.test.ts` | `crates/desktop-host/src/browser/host_bridge.rs` (unit) | T3.2 | Command order per bot; urgent lane for release. |
| `packages/desktop/src/native-browser-login.test.ts` | `crates/desktop-host/src/vault/` (unit) | T3.6 | Fill, begin and finish against the engine's private mode. |
| `packages/desktop/src/native-browser-permissions.test.ts` | `crates/desktop-host/src/browser/permissions.rs` (unit) | T3.2 | Deny everything except what the policy allows. Location derived. |
| `packages/desktop/src/native-browser-profiles.test.ts` | `crates/desktop-host/src/browser/profiles.rs` (unit) | T3.1 | Includes `delete_profile_removes_folder_after_close`. |
| `packages/desktop/src/native-browser-stream.test.ts` | `crates/desktop-host/src/browser/stream.rs` (unit) and `crates/desktop-host/tests/browser_stream.rs` | T3.4 | Includes `private_forwards_zero_frames`. |
| `packages/desktop/src/native-browser.real.test.ts` | `crates/desktop-host/tests/browser_*.rs` and `crates/desktop/tests/e2e_browser_panel.rs` (e2e) | T3.1-T3.5 | Per the plan's real-test table. |
| `packages/desktop/src/native-browser.test.ts` | `crates/desktop-host/src/browser/policy.rs` (unit) | T3.2 | Address and navigation policy. Profile naming is T3.1; clip and scale is T3.5 (`dpi_change_realigns_page`). Location derived. |
| `packages/desktop/src/notification-delivery.test.ts` | `crates/desktop-host/src/shell/notify.rs` (unit) | T6.2 | Includes `notify::not_acked_when_disabled`. |
| `packages/desktop/src/owner-safety.test.ts` | `crates/desktop-host/src/owner_safety.rs` (unit) | T0.6 | Includes `owner_safety::refuses_prod_root_for_dev`. |
| `packages/desktop/src/package-identity.test.ts` | `crates/desktop-host/src/identity.rs` (unit) and the CI identity check | T0.6 | The packaging-option gate becomes the CI assertion; `pins.rs` (T0.5) checks that release has no test-driver symbols. |
| `packages/desktop/src/packaged-provenance.test.ts` | `desktop-acceptance.yml` payload check (steps 1-2) | T8.4 | Keeps the payload check (daemon, speech, pdfium, no Electron files). Worktree hashing and stamping are Electron build only and are not ported. |
| `packages/desktop/src/packaged.real.test.ts` | `desktop-acceptance.yml` steps 1-2 | T8.4 | Per the plan's real-test table. |
| `packages/desktop/src/phone-layout.real.test.ts` | `n/a: phone only (Phase 4)` | - | - |
| `packages/desktop/src/preload.local-device.test.ts` | `n/a: Electron preload IPC; replaced by direct calls` | T4.4 | - |
| `packages/desktop/src/preload.remote.test.ts` | `n/a: Electron preload IPC; replaced by direct calls` | T1.3 | Remote calls go straight to `client::remote`. |
| `packages/desktop/src/quit-retry.test.ts` | `crates/desktop-host/src/updater/` (unit) and `crates/desktop-host/src/daemon_process.rs` (unit) | T6.4 | Includes `quit_with_forced_daemon_stop_skips_install`. |
| `packages/desktop/src/real-seed.test.ts` | `n/a: test seed for the never-run real suites; e2e fixtures replace it` | T1.1 | The e2e daemon takes `TJOMMIE_PROVIDER_FIXTURES`. |
| `packages/desktop/src/remote-bridge.real.test.ts` | `crates/desktop/tests/e2e_remote_host.rs` (e2e) | T1.3 | Per the plan's real-test table. |
| `packages/desktop/src/remote-wire.test.ts` | `crates/client/src/remote.rs` (unit) | T1.3 | Noise framing; at most 8 connections; 32 MiB backlog. |
| `packages/desktop/src/screens.real.test.ts` | `crates/desktop/tests/e2e_screens.rs` (e2e, fixtures) | M5 (T5.1-T5.5) | Never run with real accounts. The e2e version writes screenshots to a temp folder. |
| `packages/desktop/src/speech-delivery.test.ts` | CI check of the speech recipe; `crates/desktop-host/src/speech/runtime.rs` (unit) | T6.5 | The pinned speech build runs in CI and before release packaging. |
| `packages/desktop/src/speech-model-cleanup.test.ts` | `crates/desktop-host/src/speech/model.rs` (unit) | T6.5 | A failed cancel stays failed until restart verifies the partial is gone. |
| `packages/desktop/src/speech-model.test.ts` | `crates/desktop-host/src/speech/model.rs` (unit) | T6.5 | Model file and digest. |
| `packages/desktop/src/speech-permissions.test.ts` | `crates/desktop-host/src/speech/permission.rs` (unit; the Electron media-permission handler is n/a) | T6.5 | Microphone-off status text. |
| `packages/desktop/src/speech-resources.test.ts` | `crates/desktop-host/src/speech/resources.rs` (unit) | T6.5 | Runtime files and digests. |
| `packages/desktop/src/speech-runtime.test.ts` | `crates/desktop-host/src/speech/runtime.rs` (unit) | T6.5 | Includes `speech::stops_on_blur` and `wav_header_16k`. |
| `packages/desktop/src/test-acl.test.ts` | `n/a: test-harness workaround that removes the Codex sandbox's deny entries from test folders; Phase 3 tests never run under Codex` | - | Not owner safety: `owner_safety::test_root()` is T0.6's own test. |
| `packages/desktop/src/two-hosts.real.test.ts` | `crates/desktop/tests/e2e_remote_host.rs` (e2e) | T1.3 | Per the plan's real-test table. |
| `packages/desktop/src/ui-bughunt.real.test.ts` | `crates/desktop/tests/e2e_ui_bughunt.rs` (e2e, test driver) | M4-M5 | Cases re-expressed through the test driver; any DOM-only case is n/a with a reason in that file. The plan names no task; placed with M4-M5. |
| `packages/desktop/src/update-test-guards.test.ts` | `crates/desktop-host/src/identity.rs` (unit) and the installer runner checks | T8.2 | The updatetest identity guard (GUID, file name) and the refuse checks on the compiled headers. |
| `packages/desktop/src/update-test-nsis.test.ts` | `installer/tjommie.nsi` checks on the runner | T8.2 | Test-only NSIS include; target invariants. |
| `packages/desktop/src/updater.real.test.ts` | `desktop-acceptance.yml` step 1 and `crates/desktop-host/src/updater/` (unit) | T8.4 | Per the plan's real-test table; the real electron-updater hands off to the Rust installer. |
| `packages/desktop/src/updater.test.ts` | `crates/desktop-host/src/updater/` (unit) | T6.4 | Includes `updater::rejects_bad_sha512` and `rejects_foreign_app_id`. |
| `packages/desktop/src/vault-assets.test.ts` | `n/a: Electron vault-entry bundle and preload bridge; the entry is a GPUI window` | T3.6 | - |
| `packages/desktop/src/vault-command-gate.test.ts` | `crates/desktop-host/src/vault/` (unit, gate) | T3.6 | Accepted only while the window is visible, not minimized, and the app is ready. |
| `packages/desktop/src/vault-entry.test.ts` | `crates/desktop/src/ui/vault_entry.rs` (view) | T3.6 | Secure entry window; secrets zeroed after write. |
| `packages/desktop/src/vault-host.test.ts` | `crates/desktop-host/src/vault/` (unit, host client) | T3.6 | The `/vault-host` client. |
| `packages/desktop/src/worker-isolation.test.ts` | `crates/desktop-host/src/vault/` (unit, worker isolation) | T3.6 | `RemoteWorkerIsolation` over `/worker-host` (row 12). |

## Counts

Source lists, run from the repo root:

```
git ls-files ':(glob)packages/ui/**/*.test.ts' ':(glob)packages/ui/**/*.test.tsx'   -> 112 files
git ls-files ':(glob)packages/desktop/src/**/*.test.ts'                              -> 64 files (15 are *.real.test.ts)
```

| Section | Rows | Real tests | n/a rows | Owner run or never automated |
|---|---|---|---|---|
| packages/ui (112 files) | 112 | 1 | 17 | 1 |
| packages/desktop/src (64 files) | 64 | 15 | 13 | 3 |
| Total | 176 | 16 | 30 | 4 |

Each path from both `git ls-files` lists appears in exactly one row (verified by script), and no other test path appears in either table. The 176 rows are 112 + 64. Of the 16 real tests, 4 are phone-only n/a rows, 4 are owner-run or never automated, and 8 are mapped to automated Rust tests. The desktop n/a rows include those 4 phone-only real tests.

## Part 2: packages/ui/lib cases (T1.4)

Each TypeScript module in `packages/ui/lib` that carries logic is ported to `crates/desktop/src/model/<module>.rs`, with its
test cases ported one for one as `model::<module>::tests::...` (same fixtures, same expected values; times are pinned to UTC
instead of the machine's zone). A case that is about React, the DOM, a hook or Electron is `n/a` below, with the reason.
The TypeScript tests live on the `legacy` branch's `packages/ui/lib`.

Run without compiling GPUI: the models are plain Rust. In CI they run with the rest of the workspace
(`cargo test -p desktop model::`).

### Not ported, with the reason
| TypeScript module | Why |
|---|---|
| `push`, `phone-nav`, `use-phone`, `use-keyboard`, `mobile-identity`, `mobile-style`, `keep-alive`, `native-capability` | phone only (Phase 4) |
| `use-*` hooks | React hooks become GPUI entities; their logic is in the matching model (`use-dictation` in `dictation`, `use-usage`'s rules in `usage` (`concerns`, `REFETCH_MS`, `TICK_CAP_MS`, `clock_tick_ms`, `Generation`, `Throttle`; the entity owns the effect: subscribing, the timers and re-rendering), `use-draft` and `use-sheet` are view state with no logic to port) |
| `utils` (`cn`) | a Tailwind class helper |
| `theme` | the Aegis theme file (T0.2) |
| `desktop` | the Electron preload bridge, replaced by direct calls (traits in `logins` and `native_browser_session` mark where) |
| `native-overlay` | the scrim bridge, replaced by the shield (T3.5) |
| `local-speech`, `attachment-device` | replaced by the speech and device services (M6) |
| `remote-pc-fixture` | a test fixture, ported where its tests are (`desktop-host::remote_hosts` tests) |
| `google-api` | Google is not enabled |
| `connection`, `remote-hosts`, `relay-wire` | ported: `connection.ts` in `model::connection` (below), `remote-hosts.ts` and the pairing in `desktop-host::remote_hosts`, `relay-wire.ts` in `client::remote`. What `model::connection` leaves to the app is listed at the end of its section |
| `file-thumbnails-retirement.test.ts` (5 cases) | The wait for the native side to finish cleaning up an invalidated thumbnail generation is not ported: `ThumbnailQueue::invalidate` does not wait, and the caller (the thumbnails entity) awaits the native cleanup it started; the contract is documented in `file_thumbnails.rs`. Port these cases with that entity in M5 |
| `dependencies.test.ts` (1 case) | a `package.json` dependency check; there is no equivalent for the Rust crate (`Cargo.lock` is checked by `--locked` and `tests/pins.rs`) |
| `appearance-settings`, `theme` storage | device-local settings: T1.5 (`desktop-host::device_settings`) |

## Ported tests
### accounts (packages/ui/lib/accounts.ts)
| TypeScript test (file: title) | Rust test (model::accounts::tests::...) |
|---|---|
| accounts.test.ts: "limit labels and values: names the 5-hour and weekly windows, other lengths, and a window with no length" | names_the_5_hour_and_weekly_windows_other_lengths_and_a_window_with_no_length |
| accounts.test.ts: "limit labels and values: says the use and the reset, 'Usage unknown' for no reading, and how old an old reading is" | says_the_use_and_the_reset_usage_unknown_for_no_reading_and_how_old_an_old_reading_is |
| accounts.test.ts: "limit labels and values: is limited until the latest reset among reached windows still ahead" | is_limited_until_the_latest_reset_among_reached_windows_still_ahead |
| accounts.test.ts: "limit labels and values: finds the window with the highest known use" | finds_the_window_with_the_highest_known_use |
| accounts.test.ts: "a limit with no reset time: still blocks, as its own state, and a reset in the past does not" | still_blocks_as_its_own_state_and_a_reset_in_the_past_does_not |
| accounts.test.ts: "a limit with no reset time: says the reset is unknown in the card, the picker and the help line" | says_the_reset_is_unknown_in_the_card_the_picker_and_the_help_line |
| accounts.test.ts: "a limit with no reset time: makes a banner for a limited bot, with no time, and doesn't offer the account as a way out" | makes_a_banner_for_a_limited_bot_with_no_time_and_doesnt_offer_the_account_as_a_way_out |
| accounts.test.ts: "a limit with no reset time: explains a fallback whose pinned account has no reset time" | explains_a_fallback_whose_pinned_account_has_no_reset_time |
| accounts.test.ts: "Retry queued work: is offered for an unknown reset with nothing known and in the future, and not otherwise" | is_offered_for_an_unknown_reset_with_nothing_known_and_in_the_future_and_not_otherwise |
| accounts.test.ts: "Retry queued work: is not offered past a cap" | is_not_offered_past_a_cap |
| accounts.test.ts: "Retry queued work: keeps Check again apart from it, and only for a Codex subscription" | keeps_check_again_apart_from_it_and_only_for_a_codex_subscription |
| accounts.test.ts: "Retry queued work: says what a retry may cost" | says_what_a_retry_may_cost |
| accounts.test.ts: "focusReady: waits for the control a link names: not while the card is still checking, and once it appears" | waits_for_the_control_a_link_names_not_while_the_card_is_still_checking_and_once_it_appears |
| accounts.test.ts: "focusReady: is ready for a cap on an API key at once, and never for the wrong control" | is_ready_for_a_cap_on_an_api_key_at_once_and_never_for_the_wrong_control |
| accounts.test.ts: "closeEpoch: closes only the dialog that asked, so a late save can't close the next one" | closes_only_the_dialog_that_asked_so_a_late_save_cant_close_the_next_one |
| accounts.test.ts: "statusLine: takes the first rule that fits, in order" | takes_the_first_rule_that_fits_in_order |
| accounts.test.ts: "statusLine: says limited until a time, ahead of plain signed in" | says_limited_until_a_time_ahead_of_plain_signed_in |
| accounts.test.ts: "statusLine: reads an API key's state: no key, over its cap, stopped for unpriced turns, or fine" | reads_an_api_keys_state_no_key_over_its_cap_stopped_for_unpriced_turns_or_fine |
| accounts.test.ts: "spending and the picker: shows spend against the cap, near it from 80%, and what has no price" | shows_spend_against_the_cap_near_it_from_80_and_what_has_no_price |
| accounts.test.ts: "spending and the picker: orders a provider's accounts with the default first, and finds the default" | orders_a_providers_accounts_with_the_default_first_and_finds_the_default |
| accounts.test.ts: "spending and the picker: names an account's state on its picker row" | names_an_accounts_state_on_its_picker_row |
| accounts.test.ts: "spending and the picker: warns about the account in use, and about a switch" | warns_about_the_account_in_use_and_about_a_switch |
| accounts.test.ts: "spending and the picker: finds the account a bot runs on: in use, pinned, or the default" | finds_the_account_a_bot_runs_on_in_use_pinned_or_the_default |
| accounts.test.ts: "bannerFor: notifies for a checked signed-out subscription using the account runtime reason" | notifies_for_a_checked_signed_out_subscription_using_the_account_runtime_reason |
| accounts.test.ts: "bannerFor: asks to sign in the account a setup bot is on, and keeps the sandbox banner" | asks_to_sign_in_the_account_a_setup_bot_is_on_and_keeps_the_sandbox_banner |
| accounts.test.ts: "bannerFor: says a capped account's cap first, with no switch offered" | says_a_capped_accounts_cap_first_with_no_switch_offered |
| accounts.test.ts: "bannerFor: offers only the accounts that could take over for a limited bot, and never switches by itself" | offers_only_the_accounts_that_could_take_over_for_a_limited_bot_and_never_switches_by_itself |
| accounts.test.ts: "bannerFor: explains a bot that carries on with its fallback key, with nothing to press" | explains_a_bot_that_carries_on_with_its_fallback_key_with_nothing_to_press |
| accounts.test.ts: "bannerFor: has nothing to say for a healthy bot, or before the accounts are known" | has_nothing_to_say_for_a_healthy_bot_or_before_the_accounts_are_known |
| accounts.test.ts: "botsOn: lists the bots pinned to an account, which keep it from being removed" | lists_the_bots_pinned_to_an_account_which_keep_it_from_being_removed |
| accounts.test.ts: "rememberUrl: only keeps the address, by account, and drops it when a new sign-in gave none" | only_keeps_the_address_by_account_and_drops_it_when_a_new_sign_in_gave_none |
| accounts.test.ts: "takeKey: clears the field before it hands the key over, so a failing or pending request never leaves it" | clears_the_field_before_it_hands_the_key_over_so_a_failing_or_pending_request_never_leaves_it |
| accounts.test.ts: "what a person types: takes a cap in dollars, positive, with at most two decimals" | takes_a_cap_in_dollars_positive_with_at_most_two_decimals |
| accounts.test.ts: "what a person types: takes a key that is something typed and fits the credential store" | takes_a_key_that_is_something_typed_and_fits_the_credential_store |
| accounts.test.ts: "quotaCard: shows limits when there are windows, and says so honestly when there are none" | shows_limits_when_there_are_windows_and_says_so_honestly_when_there_are_none |
| accounts.test.ts: "quotaCard: is checking before the first answer, or a sign-in is waiting, and shows a problem or a signed-out account as itself" | is_checking_before_the_first_answer_or_a_sign_in_is_waiting_and_shows_a_problem_or_a_signed_out_account_as_itself |
| accounts.test.ts: "provider reading: names a window with its model or surface" | names_a_window_with_its_model_or_surface |
| accounts.test.ts: "provider reading: writes credits as the provider's units" | writes_credits_as_the_providers_units |
| accounts.test.ts: "provider reading: takes the plan from the reading before setup, and the rows from the reading before the observed windows" | takes_the_plan_from_the_reading_before_setup_and_the_rows_from_the_reading_before_the_observed_windows |
| accounts.test.ts: "provider reading: marks windows kept from an earlier success as stale, and leaves out a reset that has passed" | marks_windows_kept_from_an_earlier_success_as_stale_and_leaves_out_a_reset_that_has_passed |
| accounts.test.ts: "provider reading: never shows an unknown use as 0%" | never_shows_an_unknown_use_as_0 |
| accounts.test.ts: "provider reading: is a card, not 'unavailable', when only Extra usage or Credits is known" | is_a_card_not_unavailable_when_only_extra_usage_or_credits_is_known |
| accounts.test.ts: "provider reading: says an extras-only reading is stale after a failed check" | says_an_extras_only_reading_is_stale_after_a_failed_check |
| accounts.test.ts: "provider reading: is a problem card when the check failed and there is nothing earlier to show" | is_a_problem_card_when_the_check_failed_and_there_is_nothing_earlier_to_show |
| (extra, not in TypeScript) | money_rounds_a_tie_up_as_javascript_does |
| (extra, not in TypeScript) | credits_group_thousands_and_round_half_up_as_intl_does |
| (extra, not in TypeScript) | subscription_label_joins_the_plan_and_the_account |

### activity (packages/ui/lib/activity.ts)
| TypeScript test (file: title) | Rust test (model::activity::tests::...) |
|---|---|
| activity.test.ts: "lists the newest steps first, each with its turn's time, up to the limit" | lists_the_newest_steps_first_each_with_its_turns_time_up_to_the_limit |
| activity.test.ts: "skips activity rows whose payload doesn't match" | skips_activity_rows_whose_payload_doesnt_match |

Extra tests (no TypeScript case): none.

### approval (packages/ui/lib/approval.ts)
| TypeScript test (file: title) | Rust test (model::approval::tests::...) |
|---|---|
| approval.test.ts: "picks the path, command or address out of Claude's tool input" | picks_the_path_command_or_address_out_of_claudes_tool_input |
| approval.test.ts: "keeps Codex's command text on one line, and shortens very long details" | keeps_codexs_command_text_on_one_line_and_shortens_very_long_details |
| approval.test.ts: "says what Always allow saves in words, including Codex's command prefixes and tools" | says_what_always_allow_saves_in_words_including_codexs_command_prefixes_and_tools |

Extra tests (no TypeScript case): shows_text_that_is_not_a_known_input_as_it_is (non-object input shown as it is; a Codex list that isn't a non-empty text list is shown as it is).

### attachment_transfer (packages/ui/lib/attachment-transfer.ts)
| TypeScript test (file: title) | Rust test (model::attachment_transfer::tests::...) |
|---|---|
| attachment-transfer.test.ts: "bounded attachment client: reports failed URI cleanup after %s fails and still closes its transfer" (it.each: reauth, revision, activation) | reports_failed_uri_cleanup_after_reauth_revision_or_activation_fails_and_still_closes_its_transfer |
| attachment-transfer.test.ts: "holds no transfer while a ten-minute picker waits, then opens on the reacquired client" | holds_no_transfer_while_a_ten_minute_picker_waits_then_opens_on_the_reacquired_client |
| attachment-transfer.test.ts: "refuses changed %s after the picker before activating or reading" (it.each: id, hostId, name, size, contentType, sha256) | refuses_changed_fields_after_the_picker_before_activating_or_reading |
| attachment-transfer.test.ts: "enters transfer cancellation before open and closes a late open after genuine loss" | enters_transfer_cancellation_before_open_and_closes_a_late_open_after_genuine_loss |
| attachment-transfer.test.ts: "lets a native %s picker wait beyond the host RPC deadline" (it.each: start, finish) | lets_a_native_start_or_finish_picker_wait_beyond_the_host_rpc_deadline |
| attachment-transfer.test.ts: "streams downloads into the native destination without assembling the file" | streams_downloads_into_the_native_destination_without_assembling_the_file |
| attachment-transfer.test.ts: "does not write bytes after a cancelled picker or report a cancelled finish as saved" | does_not_write_bytes_after_a_cancelled_picker_or_report_a_cancelled_finish_as_saved |
| attachment-transfer.test.ts: "rejects revision, count, offset, transfer and base64 corruption" | rejects_revision_count_offset_transfer_and_base64_corruption |
| attachment-transfer.test.ts: "refuses a network preview URL and closes its lease/native handle" | refuses_a_network_preview_url_and_closes_its_lease_and_native_handle |
| attachment-transfer.test.ts: "serves only the matching native preview and disposes its listener once" | serves_only_the_matching_native_preview_and_disposes_its_listener_once |
| attachment-transfer.test.ts: "waits for the first in-flight abort cleanup on every subsequent preview close" | waits_for_the_first_in_flight_abort_cleanup_on_every_subsequent_preview_close |
| attachment-transfer.test.ts: "retains the first preview cleanup failure for subsequent close callers while closing the host lease" | retains_the_first_preview_cleanup_failure_for_later_close_callers_while_closing_the_host_lease |
| attachment-transfer.test.ts: "reports a host read failure separately from an unsupported media codec" | reports_a_host_read_failure_separately_from_an_unsupported_media_codec |
| attachment-transfer.test.ts: "closes an open reply that arrives after cancellation" | closes_an_open_reply_that_arrives_after_cancellation |
| attachment-transfer.test.ts: "never finalizes a native save when cancellation wins just before its start reply resumes" | never_finalizes_a_native_save_when_cancellation_wins_just_before_its_start_reply_resumes |

The file has 15 `it` blocks (the brief's count was 12). Extra test: `says_why_an_attachment_failed_in_the_words_the_app_shows` (the `attachmentError` mapping, which no TypeScript test covers).

Decisions: the daemon client is `&impl client::Rpc`; a `ClientConnectionError` is `RpcError::Closed`, a `ClientRpcError` is `RpcError::Daemon`. The TypeScript's detached promises (late replies that must still be closed) are `Retirements`, futures the caller drives. `read` returns decoded bytes. The `strictRetirement` flag is dropped: close errors propagate from a preview and are ignored by a download, as the TypeScript does without a retirement.

### attachments (packages/ui/lib/attachments.ts)
| TypeScript test (file: title) | Rust test (model::attachments::tests::...) |
|---|---|
| attachments.test.ts: "adds picked or dropped files once each, and skips dropped items that aren't files" | adds_picked_or_dropped_files_once_each_and_skips_dropped_items_that_arent_files |
| attachments.test.ts: "refuses to go over the daemon's limit of 20 files" | refuses_to_go_over_the_daemons_limit_of_20_files |
| attachments.test.ts: "shows a file's name from its Windows path" | shows_a_files_name_from_its_windows_path |

Extra tests (no TypeScript case): none (the POSIX separator is checked inside shows_a_files_name_from_its_windows_path).

### bot_form (packages/ui/lib/bot-form.ts)
| TypeScript test (file: title) | Rust test (model::bot_form::tests::...) |
|---|---|
| bot-form.test.ts: "the bot form: sends only what the person changed" | sends_only_what_the_person_changed |
| bot-form.test.ts: "the bot form: checks the name and trims it for the daemon" | checks_the_name_and_trims_it_for_the_daemon |
| bot-form.test.ts: "the bot form: sums up routines and Can message for their rows" | sums_up_routines_and_can_message_for_their_rows |
| bot-form.test.ts: "the bot form: says what Own folder protects for each CLI" | says_what_own_folder_protects_for_each_cli |
| bot-form.test.ts: "saving bot settings: sends only what was changed in the form, so a change made elsewhere meanwhile isn't put back" | sends_only_what_was_changed_in_the_form_so_a_change_made_elsewhere_meanwhile_isnt_put_back |
| bot-form.test.ts: "saving bot settings: measures the edit from the bot as the form opened, not from the bot as it is now" | n/a: reads `app/bots/settings/page.tsx` as text and checks the page calls `patchFor(opened, ...)`; a source check of a screen that is not ported yet (M5). `patch_for` takes the opened bot as its first argument, and the settings screen's port gets the behavior test |
| bot-form.test.ts: "saving bot settings: never sends a Can message bot deleted since the form opened, and doesn't count it as a change" | never_sends_a_can_message_bot_deleted_since_the_form_opened_and_doesnt_count_it_as_a_change |
| bot-form.test.ts: "the account: starts a new bot on the provider's default account, and sends the pin when creating" | starts_a_new_bot_on_the_providers_default_account_and_sends_the_pin_when_creating |
| bot-form.test.ts: "the account: reads a bot's pin, and sends it only when it changed, apart from the provider and model" | reads_a_bots_pin_and_sends_it_only_when_it_changed_apart_from_the_provider_and_model |
| (extra, not in TypeScript) | a_name_is_measured_in_javascript_characters_so_an_emoji_counts_twice |

### browser_geometry (packages/ui/lib/browser-geometry.ts)

| TypeScript test (file: title) | Rust test (model::browser_geometry::tests::...) |
|---|---|
| browser-geometry.test.ts: "letterboxes a frame that is wider or taller than its box, and fills one that matches" | model::browser_geometry::tests::letterboxes_a_frame_that_is_wider_or_taller_than_its_box_and_fills_one_that_matches |
| browser-geometry.test.ts: "maps a pointer over the drawn picture to the frame's pixels" | model::browser_geometry::tests::maps_a_pointer_over_the_drawn_picture_to_the_frames_pixels |
| browser-geometry.test.ts: "ignores the letterbox" | model::browser_geometry::tests::ignores_the_letterbox |
| browser-geometry.test.ts: "never reaches the frame's own width or height" | model::browser_geometry::tests::never_reaches_the_frames_own_width_or_height |
| browser-geometry.test.ts: "maps onto the page's own size when the picture was scaled down" | model::browser_geometry::tests::maps_onto_the_pages_own_size_when_the_picture_was_scaled_down |
| browser-geometry.test.ts: "has no point while there is no drawn picture" | model::browser_geometry::tests::has_no_point_while_there_is_no_drawn_picture |
| browser-geometry.test.ts: "asks for what the viewport shows, within the daemon's limits" | model::browser_geometry::tests::asks_for_what_the_viewport_shows_within_the_daemons_limits |
| browser-geometry.test.ts: "skips sizes within a step of the last one asked for" | model::browser_geometry::tests::skips_sizes_within_a_step_of_the_last_one_asked_for |
| browser-geometry.test.ts: "names mouse buttons, and ignores the side buttons" | model::browser_geometry::tests::names_mouse_buttons_and_ignores_the_side_buttons |
| browser-geometry.test.ts: "turns wheel lines and pages into pixels within the daemon's limit" | model::browser_geometry::tests::turns_wheel_lines_and_pages_into_pixels_within_the_daemons_limit |
| browser-geometry.test.ts: "reads the address bar as an address, a host, this computer, or a search" | model::browser_geometry::tests::reads_the_address_bar_as_an_address_a_host_this_computer_or_a_search |
| browser-geometry.test.ts: "lets Shift+Tab leave the page, waits on paste, and sends other keys" | model::browser_geometry::tests::lets_shifttab_leave_the_page_waits_on_paste_and_sends_other_keys |
| browser-geometry.test.ts: "is true only on Google's rejected sign-in page" | model::browser_geometry::tests::is_true_only_on_googles_rejected_sign_in_page |
| browser-geometry.test.ts: "is the page size a frame names, else the picture's own" | model::browser_geometry::tests::is_the_page_size_a_frame_names_else_the_pictures_own |
| browser-geometry.test.ts: "puts a pointer that left the picture on its nearest edge, so a release still reaches the page" | model::browser_geometry::tests::puts_a_pointer_that_left_the_picture_on_its_nearest_edge_so_a_release_still_reaches_the_page |
| browser-geometry.test.ts: "agrees with framePoint inside the picture" | model::browser_geometry::tests::agrees_with_frame_point_inside_the_picture |
| browser-geometry.test.ts: "is ready only for a decoded picture of this frame's size and source" | model::browser_geometry::tests::is_ready_only_for_a_decoded_picture_of_this_frames_size_and_source |
| browser-geometry.test.ts: "changes with the session, page, layout or stream, but not with each frame" | model::browser_geometry::tests::changes_with_the_session_page_layout_or_stream_but_not_with_each_frame |

Extra (no TypeScript case): model::browser_geometry::tests::address_bar_fallbacks_and_host_shapes, the address bar's regex edges (localhost, host, one-letter suffix).

### browser_preview (packages/ui/lib/browser-preview.ts)

| TypeScript test (file: title) | Rust test (model::browser_preview::tests::...) |
|---|---|
| browser-preview.test.ts: "polls once a second only while visible and clears retained pixels when hidden/private" | model::browser_preview::tests::polls_once_a_second_only_while_visible_and_clears_retained_pixels_when_hidden_private |
| browser-preview.test.ts: "drops late hidden replies and never has a second request queued" | model::browser_preview::tests::drops_late_hidden_replies_and_never_has_a_second_request_queued |

### browser_session (packages/ui/lib/browser-session.ts)

| TypeScript test (file: title) | Rust test (model::browser_session::tests::...) |
|---|---|
| browser-session.test.ts: "does not send input for another controller and suppresses private pixels" | model::browser_session::tests::does_not_send_input_for_another_controller_and_suppresses_private_pixels |
| browser-session.test.ts: "opens, then pulls frames one at a time every 250 ms when idle without redrawing identical images" | model::browser_session::tests::opens_then_pulls_frames_one_at_a_time_every_250_ms_when_idle_without_redrawing_identical_images |
| browser-session.test.ts: "stops pulling while hidden and resumes when visible" | model::browser_session::tests::stops_pulling_while_hidden_and_resumes_when_visible |
| browser-session.test.ts: "keeps pulling after a failed pull, slower, and shows the problem" | model::browser_session::tests::keeps_pulling_after_a_failed_pull_slower_and_shows_the_problem |
| browser-session.test.ts: "sends input in order, with the generation and page from when the user acted" | model::browser_session::tests::sends_input_in_order_with_the_generation_and_page_from_when_the_user_acted |
| browser-session.test.ts: "keeps only the newest of queued pointer moves" | model::browser_session::tests::keeps_only_the_newest_of_queued_pointer_moves |
| browser-session.test.ts: "sends no input while the bot is in control" | model::browser_session::tests::sends_no_input_while_the_bot_is_in_control |
| browser-session.test.ts: "releases held keys and buttons, and closing releases them too but leaves the browser open" | model::browser_session::tests::releases_held_keys_and_buttons_and_closing_releases_them_too_but_leaves_the_browser_open |
| browser-session.test.ts: "releases a key whose press was still in flight when the panel closed" | model::browser_session::tests::releases_a_key_whose_press_was_still_in_flight_when_the_panel_closed |
| browser-session.test.ts: "still sends a queued release at close, and drops a queued press" | model::browser_session::tests::still_sends_a_queued_release_at_close_and_drops_a_queued_press |
| browser-session.test.ts: "hands the browser back to the bot when closed, and not when the bot already has it" | model::browser_session::tests::hands_the_browser_back_to_the_bot_when_closed_and_not_when_the_bot_already_has_it |
| browser-session.test.ts: "does not leave the user in control when closed while Take control is still in flight" | model::browser_session::tests::does_not_leave_the_user_in_control_when_closed_while_take_control_is_still_in_flight |
| browser-session.test.ts: "hands control back even when the browser can't be asked, if Take control was in flight" | model::browser_session::tests::hands_control_back_even_when_the_browser_cant_be_asked_if_take_control_was_in_flight |
| browser-session.test.ts: "reads fresh ownership without a picture and hands back what it says is human" | model::browser_session::tests::reads_fresh_ownership_without_a_picture_and_hands_back_what_it_says_is_human |
| browser-session.test.ts: "makes a quick reopen wait for the old panel's hand-back" | model::browser_session::tests::makes_a_quick_reopen_wait_for_the_old_panels_hand_back |
| browser-session.test.ts: "clears the picture when the page is resized or moved, and takes no input until a fresh frame" | model::browser_session::tests::clears_the_picture_when_the_page_is_resized_or_moved_and_takes_no_input_until_a_fresh_frame |
| browser-session.test.ts: "keeps the picture through a key press, a control change or a dialog answer" | model::browser_session::tests::keeps_the_picture_through_a_key_press_a_control_change_or_a_dialog_answer |
| browser-session.test.ts: "dismisses a page dialog left open before handing the browser back" | model::browser_session::tests::dismisses_a_page_dialog_left_open_before_handing_the_browser_back |
| browser-session.test.ts: "ignores a key-up for a key it never saw go down" | model::browser_session::tests::ignores_a_key_up_for_a_key_it_never_saw_go_down |
| browser-session.test.ts: "does nothing after close, even for a pull that was in flight" | model::browser_session::tests::does_nothing_after_close_even_for_a_pull_that_was_in_flight |
| browser-session.test.ts: "drops the old picture when the page or generation changes, and takes no input until a new one shows" | model::browser_session::tests::drops_the_old_picture_when_the_page_or_generation_changes_and_takes_no_input_until_a_new_one_shows |
| browser-session.test.ts: "switches control and applies the returned state" | model::browser_session::tests::switches_control_and_applies_the_returned_state |
| browser-session.test.ts: "sends navigation, history, tab, resize and dialog calls for the active page" | model::browser_session::tests::sends_navigation_history_tab_resize_and_dialog_calls_for_the_active_page |
| browser-session.test.ts: "keeps one frame request in flight while ordered input and a modal answer proceed independently" | model::browser_session::tests::keeps_one_frame_request_in_flight_while_ordered_input_and_a_modal_answer_proceed_independently |
| browser-session.test.ts: "fences an old in-flight frame across control and rejects lower server revisions" | model::browser_session::tests::fences_an_old_in_flight_frame_across_control_and_rejects_lower_server_revisions |
| browser-session.test.ts: "fences an old in-flight frame across tab and rejects lower server revisions" | model::browser_session::tests::fences_an_old_in_flight_frame_across_tab_and_rejects_lower_server_revisions |
| browser-session.test.ts: "fences an old in-flight frame across navigate and rejects lower server revisions" | model::browser_session::tests::fences_an_old_in_flight_frame_across_navigate_and_rejects_lower_server_revisions |
| browser-session.test.ts: "fences an old in-flight frame across resize and rejects lower server revisions" | model::browser_session::tests::fences_an_old_in_flight_frame_across_resize_and_rejects_lower_server_revisions |
| browser-session.test.ts: "coalesces adjacent wheels and resizes but preserves clicks, keys and release barriers" | model::browser_session::tests::coalesces_adjacent_wheels_and_resizes_but_preserves_clicks_keys_and_release_barriers |
| browser-session.test.ts: "suspends through pending takeover and resumes behind handback without losing session state" | model::browser_session::tests::suspends_through_pending_takeover_and_resumes_behind_handback_without_losing_session_state |
| browser-session.test.ts: "a second hide drops a takeover queued during a still-pending handback" | model::browser_session::tests::a_second_hide_drops_a_takeover_queued_during_a_still_pending_handback |
| browser-session.test.ts: "retains the last image and avoids view notifications for an unchanged cursor response" | model::browser_session::tests::retains_the_last_image_and_avoids_view_notifications_for_an_unchanged_cursor_response |
| browser-session.test.ts: "hands back promptly through an unresolved image request and discards its eventual response" | model::browser_session::tests::hands_back_promptly_through_an_unresolved_image_request_and_discards_its_eventual_response |
| browser-session.test.ts: "sends drag and key releases to their original tab after a tab selection" | model::browser_session::tests::sends_drag_and_key_releases_to_their_original_tab_after_a_tab_selection |
| browser-session.test.ts: "asks for the stream while shown, and stops it when hidden or closed" | model::browser_session::tests::asks_for_the_stream_while_shown_and_stops_it_when_hidden_or_closed |
| browser-session.test.ts: "acknowledges each streamed frame once, in order, as it is drawn, replaced or dropped" | model::browser_session::tests::acknowledges_each_streamed_frame_once_in_order_as_it_is_drawn_replaced_or_dropped |
| browser-session.test.ts: "polls browser.frame once the stream ends, and asks again when the private interaction is over" | model::browser_session::tests::polls_browser_frame_once_the_stream_ends_and_asks_again_when_the_private_interaction_is_over |
| browser-session.test.ts: "keeps polling browser.frame, and never asks again, when an older daemon answers unknown_method" | model::browser_session::tests::keeps_polling_browser_frame_and_never_asks_again_when_an_older_daemon_answers_unknown_method |
| browser-session.test.ts: "pulls one fresh picture when a command leaves the streamed one out of date" | model::browser_session::tests::pulls_one_fresh_picture_when_a_command_leaves_the_streamed_one_out_of_date |
| browser-session.test.ts: "maps a pointer onto the page's own pixels, inside its edges" | model::browser_session::tests::maps_a_pointer_onto_the_pages_own_pixels_inside_its_edges |
| browser-session.test.ts: "names mouse buttons, and ignores the side buttons" | model::browser_session::tests::names_mouse_buttons_and_ignores_the_side_buttons |
| browser-session.test.ts: "turns wheel lines and pages into pixels within the daemon's limit" | model::browser_session::tests::turns_wheel_lines_and_pages_into_pixels_within_the_daemons_limit |
| browser-session.test.ts: "reads the address bar as an address, a host, this computer, or a search" | model::browser_session::tests::reads_the_address_bar_as_an_address_a_host_this_computer_or_a_search |
| browser-session.test.ts: "lets Shift+Tab leave the page, waits on paste, and sends other keys" | model::browser_session::tests::lets_shifttab_leave_the_page_waits_on_paste_and_sends_other_keys |
| browser-session.test.ts: "keeps the requested viewport 4:3 and inside the daemon's limits" | model::browser_session::tests::keeps_the_requested_viewport_4_3_and_inside_the_daemons_limits |

### dictation (packages/ui/lib/dictation.ts, and the logic of packages/ui/lib/use-dictation.ts)
| TypeScript test (file: title) | Rust test (model::dictation::tests::...) |
|---|---|
| dictation.test.ts: "draft insertion: moves the original range across typed edits before and after it" | moves_the_original_range_across_typed_edits_before_and_after_it |
| dictation.test.ts: "draft insertion: refuses overlapping, ambiguous, IME, and same-text replacement edits" | refuses_overlapping_ambiguous_ime_and_same_text_replacement_edits |
| dictation.test.ts: "draft insertion: keeps typing at a collapsed anchor before the final transcript" | keeps_typing_at_a_collapsed_anchor_before_the_final_transcript |
| dictation.test.ts: "draft insertion: maps ordinary browser edits and refuses composition/history edits" | maps_ordinary_browser_edits_and_refuses_composition_and_history_edits |

use-dictation.ts has no test file. Its logic is ported as decisions (extra tests): `counts_positions_in_utf_16_units_as_the_dom_does`, `keeps_only_the_english_language_a_saved_preference_may_name`, `starts_in_the_language_the_user_chose_or_the_first_installed`, `refuses_a_start_with_nothing_installed_and_says_why`, `asks_for_permission_through_the_native_path_when_android_needs_it`, `reports_a_speech_event_for_the_draft_only_while_it_is_transcribing`, `drops_the_session_range_an_edit_touches_and_keeps_the_rest`, `says_what_a_model_action_left_behind`.

Not ported (the hook's effects, React or the platform): `useDictation` itself, the `localSpeech` bridge and `claimSpeech`, `capturePcm` (Web Audio), the 59.75 s listen timer, the `beforeinput` and `visibilitychange`/`pagehide`/`blur` listeners, `localStorage` preferences, `navigator.mediaDevices` device list, and React state. The desktop drives these decisions from its own speech bridge.

### document_ranges (packages/ui/lib/document-ranges.ts)
| TypeScript test (file: title) | Rust test (model::document_ranges::tests::...) |
|---|---|
| document-ranges.test.ts: "delivers all 800 full-size PDF callbacks serially, one exact chunk each, without the provisional queue/range cap" | delivers_all_800_full_size_pdf_callbacks_serially_one_exact_chunk_each_without_the_provisional_queue_range_cap |
| document-ranges.test.ts: "fails duplicate/overlapping PDF requests and drops all queued callbacks after abort" | fails_duplicate_overlapping_pdf_requests_and_drops_all_queued_callbacks_after_abort |

The other three cases of document-ranges.test.ts test markdown-document.ts; they are in `markdown_document.md`.

Extra tests (no TypeScript case; decisions the port makes): `refuses_a_reply_that_does_not_say_exactly_the_range_asked_for`, `refuses_ranges_outside_the_document_or_over_a_chunk_before_fetching`, `an_aborted_signal_is_a_cancellation_not_a_failure`.

Not ported: `Number.isSafeInteger` checks (u64 inputs make them unnecessary beyond the size bound); the `AbortSignal` is `Signal`, a cancel flag.

### exchange (packages/ui/lib/exchange.ts)
| TypeScript test (file: title) | Rust test (model::exchange::tests::...) |
|---|---|
| exchange.test.ts: "says when the exchange was asked and how it ended" | says_when_the_exchange_was_asked_and_how_it_ended |
| exchange.test.ts: "says where the answer and its files went" | says_where_the_answer_and_its_files_went |
| exchange.test.ts: "reads an exchange's status from a chat's handoff row, which the daemon updates when it's answered" | reads_an_exchanges_status_from_a_chats_handoff_row_which_the_daemon_updates_when_its_answered |

Extra tests (no TypeScript case): says_how_long_the_answer_took_in_words_rounding_halves_up_like_javascript (30 seconds is a minute; 89 minutes is an hour; 90 minutes is two hours).

### file_thumbnails (packages/ui/lib/file-thumbnails.ts)
| TypeScript test (file: title) | Rust test (model::file_thumbnails::tests::...) |
|---|---|
| file-thumbnails.test.ts: "admits ordinary phone photos and rejects large automatic resources" | admits_ordinary_phone_photos_and_rejects_large_automatic_resources |
| file-thumbnails.test.ts: "reads only the actual first4KiB of a200MiB document and keeps markup literal" | reads_only_the_actual_first_4kib_of_a_200mib_document_and_keeps_markup_literal |
| file-thumbnails.test.ts: "rejects duplicate range lengths and closes the secure preview" | rejects_duplicate_range_lengths_and_closes_the_secure_preview |
| file-thumbnails.test.ts: "decodes bounded12MP images after exact chunk reads and releases the bitmap and native handle" | decodes_bounded_12mp_images_after_exact_chunk_reads_and_releases_the_bitmap_and_native_handle (the renderer releases its bitmap; the test counts it) |
| file-thumbnails.test.ts: "checks raster pixel limits before decode, including JPEG/GIF/WebP dimensions" | checks_raster_pixel_limits_before_decode_including_jpeg_gif_webp_dimensions |
| file-thumbnails.test.ts: "closes a bitmap that finishes decoding after cancellation without drawing it" | closes_a_bitmap_that_finishes_decoding_after_cancellation_without_drawing_it (drawing is the WebView's; the test checks the bitmap is released) |
| file-thumbnails.test.ts: "renders only PDF page1 through local ranges and destroys its worker" | n/a: pdf.js in the WebView (the pinned worker, page 1 render and cleanup). The Rust port has no PDF renderer; PDF thumbnails are `Unsupported`. |
| file-thumbnails.test.ts: "terminates its owned PDF worker even when loading-task teardown rejects" | n/a: pdf.js worker lifecycle in the WebView (no worker exists in Rust) |
| file-thumbnails.test.ts: "never allocates the PDF worker for a200MiB automatic preview" | never_allocates_the_pdf_worker_for_a_200mib_automatic_preview (no renderer call; the lease closes) |
| file-thumbnails.test.ts: "stops PDF loading before range requests exceed2MiB and retires its worker" | stops_pdf_loading_before_range_requests_exceed_2mib_and_retires_its_worker (the 2 MiB budget and range fetches; the worker retirement is pdf.js's) |
| file-thumbnails.test.ts: "creates an actual muted video frame through the scoped native URL without playback or a CSP-blocked blob" | n/a: HTMLVideoElement (muted, play/pause, seek) and the blob/CSP rules are WebView DOM. The renderer's `video_frame` seam takes the preview URL. |
| file-thumbnails.test.ts: "retires an active thumbnail before foreground work and does not admit queued cards while paused" | retires_an_active_thumbnail_before_foreground_work_and_does_not_admit_queued_cards_while_paused |

Extra test: `refuses_text_with_control_characters_as_not_a_text_thumbnail`.

Not ported: the canvas draw (`draw`, `canvas()`), the pdf.js loader, and the wait on native cleanup of an invalidated generation (`invalidateThumbnailWork` bumps the generation and aborts the running work; the caller awaits its own native cleanup before it queues again). `readFileThumbnail`'s `nativeInvalidated` race is also the caller's.

### google_connection (packages/ui/lib/google-connection.ts)
| TypeScript test (file: title) | Rust test (model::google_connection::tests::...) |
|---|---|
| google-connection.test.ts: "google status: is set up, connect, waiting or connected" | is_set_up_connect_waiting_or_connected |
| google-connection.test.ts: "google status: gives access only to the bots granted it, and none by default" | gives_access_only_to_the_bots_granted_it_and_none_by_default |
| google-connection.test.ts: "google status: says why a sign-in failed" | says_why_a_sign_in_failed |
| google-connection.test.ts: "google status: describes itself for the bot panel's card" | describes_itself_for_the_bot_panels_card |
| google-connection.test.ts: "services and clearing: lists services in words, and before any is connected names what will be asked for" | lists_services_in_words_and_before_any_is_connected_names_what_will_be_asked_for |
| google-connection.test.ts: "services and clearing: says what full access allows for the services covered" | says_what_full_access_allows_for_the_services_covered |
| google-connection.test.ts: "services and clearing: needs a new consent only for a connection the daemon says is read-only" | needs_a_new_consent_only_for_a_connection_the_daemon_says_is_read_only |
| google-connection.test.ts: "services and clearing: can clear local settings only while not connected and something is saved or wrong" | can_clear_local_settings_only_while_not_connected_and_something_is_saved_or_wrong |
| google-connection.test.ts: "validClientId: accepts a Google client ID and nothing else" | accepts_a_google_client_id_and_nothing_else |
| google-connection.test.ts: "watchGoogle: reads once when nothing is waiting" | reads_once_when_nothing_is_waiting |
| google-connection.test.ts: "watchGoogle: polls every second while a sign-in is waiting, and stops when it's done" | polls_every_interval_while_a_sign_in_is_waiting_and_stops_when_it_is_done |
| google-connection.test.ts: "watchGoogle: follows the daemon's change events, and stops following when stopped" | follows_the_daemons_change_events_and_stops_following_when_stopped |
| google-connection.test.ts: "watchGoogle: reports a failed read" | reports_a_failed_read |

Decisions: the wire `GoogleStatus` has no `access` field (protocol google.ts), so `needs_reconsent` and `card_line` take the daemon's `access` as a parameter. `watch_google` runs until it is dropped (the TypeScript `stop`); the poll interval is a parameter (`POLL` = 1 s). The `GoogleApi` type's Electron-side `connect`/`cancel`/`onChanged` are not ported (the desktop opens the consent page itself).

### logins (packages/ui/lib/logins.ts)
The three TypeScript tests need the Electron bridge (`desktop.ts`, n/a in Rust) and `daemon()` (connection.ts): the Rust port takes both as the `Desktop`, `NativeVault` and `LiveState` traits, and the test fakes stand in for them. `LOGINS_PROBLEM` is asserted in the third test.

| TypeScript test (file: title) | Rust test (model::logins::tests::...) |
|---|---|
| logins.test.ts: "captures the selected host and refuses a late response after host/client replacement" | captures_the_selected_host_and_refuses_a_late_response_after_host_client_replacement |
| logins.test.ts: "cancels the captured operation without tombstoning the connection and accepts a later fresh request" | cancels_the_captured_operation_without_tombstoning_the_connection_and_accepts_a_later_fresh_request |
| logins.test.ts: "uses the direct native API only for this PC, and rejects secret-bearing responses without repeating them" | uses_the_direct_native_api_only_for_this_pc_and_rejects_secret_bearing_responses_without_repeating_them |
| (extra, not in TypeScript) | a_native_abort_reads_as_cancelled_and_any_other_native_failure_as_the_problem |
| (extra, not in TypeScript) | disconnect_is_sent_as_vault_cancel_and_answers_done |
| (extra, not in TypeScript) | cancel_sends_nothing_when_no_operation_is_running |
| (extra, not in TypeScript) | a_lock_during_an_entry_cancels_the_entry |
| (extra, not in TypeScript) | a_daemon_answer_of_the_wrong_shape_reads_as_the_problem |
| (extra, not in TypeScript) | with_no_daemon_while_offline_nothing_is_sent_and_the_request_reads_as_the_problem |

### markdown_document (packages/ui/lib/markdown-document.ts)
markdown-document.ts has no test file of its own. Its three cases are in document-ranges.test.ts:

| TypeScript test (file: title) | Rust test (model::markdown_document::tests::...) |
|---|---|
| document-ranges.test.ts: "reads first and last sections of a 200MiB Markdown file without reading or assembling the intervening file" | reads_first_and_last_sections_of_a_200mib_markdown_file_without_reading_or_assembling_the_intervening_file |
| document-ranges.test.ts: "preserves UTF-8 characters across every paged boundary and finds text crossing a boundary" | preserves_utf_8_characters_across_every_paged_boundary_and_finds_text_crossing_a_boundary |
| document-ranges.test.ts: "rejects non-UTF8, range header/count corruption and an aborted read" | rejects_non_utf8_range_header_count_corruption_and_an_aborted_read |

Extra tests (decisions the port makes): `an_empty_document_is_one_empty_section_and_nothing_is_fetched`, `refuses_sections_and_searches_outside_the_document_before_fetching`, `search_starts_at_the_given_offset_and_reports_no_match_as_none`.

Note: search offsets are byte positions taken from the lowercased text, as in the TypeScript; a case mapping that changes length would shift them the same way.

### mentions (packages/ui/lib/mentions.ts)
| TypeScript test (file: title) | Rust test (model::mentions::tests::...) |
|---|---|
| mentions.test.ts: "finds the @ being typed, and ignores email addresses" | finds_the_at_being_typed_and_ignores_email_addresses |
| mentions.test.ts: "offers the bots whose names start with it, then everyone" | offers_the_bots_whose_names_start_with_it_then_everyone |
| mentions.test.ts: "puts the whole mention in, with a space after it" | puts_the_whole_mention_in_with_a_space_after_it |

Extra tests (no TypeScript case): reads_carets_the_way_the_composer_gives_them (carets past the end clamp as in JavaScript's `slice`; a caret inside a character is no mention; the 40-unit query limit counts UTF-16 units).

### native_browser_session (packages/ui/lib/native-browser-session.ts)

| TypeScript test (file: title) | Rust test (model::native_browser_session::tests::...) |
|---|---|
| native-browser-session.test.ts: "opens the bot's browser and shows its state, with no picture" | model::native_browser_session::tests::opens_the_bots_browser_and_shows_its_state_with_no_picture |
| native-browser-session.test.ts: "follows the desktop app's state events for its own bot only" | model::native_browser_session::tests::follows_the_desktop_apps_state_events_for_its_own_bot_only |
| native-browser-session.test.ts: "rebinds its listeners when opened again, without leaking the old ones" | model::native_browser_session::tests::rebinds_its_listeners_when_opened_again_without_leaking_the_old_ones |
| native-browser-session.test.ts: "sends bounds only when they differ from what was accepted, and only the newest while one is in flight" | model::native_browser_session::tests::sends_bounds_only_when_they_differ_from_what_was_accepted_and_only_the_newest_while_one_is_in_flight |
| native-browser-session.test.ts: "tries a mount again after it fails, instead of treating the bounds as sent" | model::native_browser_session::tests::tries_a_mount_again_after_it_fails_instead_of_treating_the_bounds_as_sent |
| native-browser-session.test.ts: "temporarily covers the page without releasing control, then restores it at the same bounds" | model::native_browser_session::tests::temporarily_covers_the_page_without_releasing_control_then_restores_it_at_the_same_bounds |
| native-browser-session.test.ts: "hides the view at once, not behind a slow navigation, and mounts the latest bounds when shown again" | model::native_browser_session::tests::hides_the_view_at_once_not_behind_a_slow_navigation_and_mounts_the_latest_bounds_when_shown_again |
| native-browser-session.test.ts: "takes and gives back control through the desktop app" | model::native_browser_session::tests::takes_and_gives_back_control_through_the_desktop_app |
| native-browser-session.test.ts: "acts on the generation and page the user saw, in the order they acted" | model::native_browser_session::tests::acts_on_the_generation_and_page_the_user_saw_in_the_order_they_acted |
| native-browser-session.test.ts: "answers the dialog on the page it belongs to, not the page in front" | model::native_browser_session::tests::answers_the_dialog_on_the_page_it_belongs_to_not_the_page_in_front |
| native-browser-session.test.ts: "never shows an older state than the one shown, within a generation" | model::native_browser_session::tests::never_shows_an_older_state_than_the_one_shown_within_a_generation |
| native-browser-session.test.ts: "drops a reply that an event overtook (no revision), and reads the state again" | model::native_browser_session::tests::drops_a_reply_that_an_event_overtook_no_revision_and_reads_the_state_again |
| native-browser-session.test.ts: "tells the panel when the page let go of the keyboard, for its own bot" | model::native_browser_session::tests::tells_the_panel_when_the_page_let_go_of_the_keyboard_for_its_own_bot |
| native-browser-session.test.ts: "suspends from the desktop app's own state: dismisses the dialog, then hands the browser back, keeping the session" | model::native_browser_session::tests::suspends_from_the_desktop_apps_own_state_dismisses_the_dialog_then_hands_the_browser_back_keeping_the_session |
| native-browser-session.test.ts: "suspends a bot-controlled browser without touching it" | model::native_browser_session::tests::suspends_a_bot_controlled_browser_without_touching_it |
| native-browser-session.test.ts: "still hands control back when the state can't be read but Take control was asked for" | model::native_browser_session::tests::still_hands_control_back_when_the_state_cant_be_read_but_take_control_was_asked_for |
| native-browser-session.test.ts: "hands back a take-control that was still in flight when it closed, dropping queued actions, and hides the view" | model::native_browser_session::tests::hands_back_a_take_control_that_was_still_in_flight_when_it_closed_dropping_queued_actions_and_hides_the_view |
| native-browser-session.test.ts: "closes even when the hand-back fails, hiding the view" | model::native_browser_session::tests::closes_even_when_the_hand_back_fails_hiding_the_view |
| native-browser-session.test.ts: "makes a quick reopen wait for the old panel's hand-back" | model::native_browser_session::tests::makes_a_quick_reopen_wait_for_the_old_panels_hand_back |
| native-browser-session.test.ts: "drops a delayed mount reply from an older generation after an event of a newer one" | model::native_browser_session::tests::drops_a_delayed_mount_reply_from_an_older_generation_after_an_event_of_a_newer_one |
| native-browser-session.test.ts: "doesn't let a mount reply without a revision overwrite a newer event, and doesn't ask again for it" | model::native_browser_session::tests::doesnt_let_a_mount_reply_without_a_revision_overwrite_a_newer_event_and_doesnt_ask_again_for_it |
| native-browser-session.test.ts: "still takes a reply that is plainly newer: the same generation, a higher revision" | model::native_browser_session::tests::still_takes_a_reply_that_is_plainly_newer_the_same_generation_a_higher_revision |
| native-browser-session.test.ts: "is tried again by itself, without the viewport asking again" | model::native_browser_session::tests::is_tried_again_by_itself_without_the_viewport_asking_again |
| native-browser-session.test.ts: "keeps a close waiting until the hide is accepted" | model::native_browser_session::tests::keeps_a_close_waiting_until_the_hide_is_accepted |
| native-browser-session.test.ts: "gives up after a bounded number of tries, and says the page may still be showing" | model::native_browser_session::tests::gives_up_after_a_bounded_number_of_tries_and_says_the_page_may_still_be_showing |
| native-browser-session.test.ts: "starts again after a close, waiting for it, and keeps its new listeners" | model::native_browser_session::tests::starts_again_after_a_close_waiting_for_it_and_keeps_its_new_listeners |
| native-browser-session.test.ts: "shows a state as the shared chrome expects it, with no frame" | model::native_browser_session::tests::shows_a_state_as_the_shared_chrome_expects_it_with_no_frame |
| native-browser-session.test.ts: "compares rectangles, with null as hidden" | model::native_browser_session::tests::compares_rectangles_with_null_as_hidden |

### overview (packages/ui/lib/overview.ts)
| TypeScript test (file: title) | Rust test (model::overview::tests::...) |
|---|---|
| overview.test.ts: "sums up the bots and filters them" | sums_up_the_bots_and_filters_them |
| overview.test.ts: "says what each bot is doing" | says_what_each_bot_is_doing |
| overview.test.ts: "shows the next routine run" | shows_the_next_routine_run |
| overview.test.ts: "finds today's partners and the room's last exchange" | finds_todays_partners_and_the_rooms_last_exchange |
| overview.test.ts: "gives each room the line from its own posts" | gives_each_room_the_line_from_its_own_posts |
| overview.test.ts: "gives a room the line from its newest post of any kind, and says so when it has none" | gives_a_room_the_line_from_its_newest_post_of_any_kind_and_says_so_when_it_has_none |
| overview.test.ts: "leaves a room's line blank while its posts are being read, rather than saying it has none" | leaves_a_rooms_line_blank_while_its_posts_are_being_read_rather_than_saying_it_has_none |
| overview.test.ts: "offers only the states some bot is in, with counts" | offers_only_the_states_some_bot_is_in_with_counts |
| overview.test.ts: "shows nothing when no chip would narrow the list" | shows_nothing_when_no_chip_would_narrow_the_list |
| overview.test.ts: "keeps the picked chip, so a filter that now matches nobody can be left" | keeps_the_picked_chip_so_a_filter_that_now_matches_nobody_can_be_left |
| overview.test.ts: "names a lone bot and counts several" | names_a_lone_bot_and_counts_several |

Extra tests (no TypeScript case): none. Decisions: the TypeScript `Pick<Bot, ...>` arguments are the whole protocol `Bot`; `taskLine` takes `now` (the zone for a limited bot's time, which the TypeScript read from the machine).

### panel_layout (packages/ui/lib/panel-layout.ts)
| TypeScript test (file: title) | Rust test (model::panel_layout::tests::...) |
|---|---|
| panel-layout.test.ts: "panel width: keeps the chat's minimum beside the panel, and the pane's own minimum" | keeps_the_chat_minimum_beside_the_panel_and_the_panes_own_minimum |
| panel-layout.test.ts: "panel width: becomes an overlay below 1000 px, with a strip of chat left showing" | becomes_an_overlay_below_1000_px_with_a_strip_of_chat_left_showing |
| panel-layout.test.ts: "panel width: clamps a width into its bounds" | clamps_a_width_into_its_bounds |
| panel-layout.test.ts: "panel width: steps with the arrow keys, by 64 with Shift, and jumps with Home and End" | steps_with_the_arrow_keys_by_64_with_shift_and_jumps_with_home_and_end |
| panel-layout.test.ts: "panel width: remembers a width per pane, and falls back to the default for nothing or junk" | remembers_a_width_per_pane_and_falls_back_to_the_default_for_nothing_or_junk |
| panel-layout.test.ts: "isPhone: is a viewport narrower than 640 px, and unknown (0) is not a phone" | is_a_viewport_narrower_than_640_px_and_unknown_0_is_not_a_phone |
| panel-layout.test.ts: "panel view: docks the browser when leaving it, so Expand, Details, Browser comes back docked" | docks_the_browser_when_leaving_it_so_expand_details_browser_comes_back_docked |
| panel-layout.test.ts: "panel view: keeps an expanded browser expanded when the browser is chosen again" | keeps_an_expanded_browser_expanded_when_the_browser_is_chosen_again |
| panel-layout.test.ts: "panel view: starts another bot at Details" | starts_another_bot_at_details |
| panel-layout.test.ts: "panel view: covers the chat for an overlay, or an expanded browser only while the bot has a browser" | covers_the_chat_for_an_overlay_or_an_expanded_browser_only_while_the_bot_has_a_browser |

Storage: `WidthStorage` stands in for `Pick<Storage, ...>`; the test uses a HashMap.

### projects (packages/ui/lib/projects.ts)
| TypeScript test (file: title) | Rust test (model::projects::tests::...) |
|---|---|
| projects.test.ts: "enables exact integration only for fresh independent results and a clean completed attempt" | enables_exact_integration_only_for_fresh_independent_results_and_a_clean_completed_attempt |
| (extra, not in TypeScript) | an_empty_reviewer_is_no_reviewer_as_in_javascript |
| (extra, not in TypeScript) | short_sha_cuts_to_twelve_characters_and_says_when_none |

### routine_form (packages/ui/lib/routine-form.ts)
| TypeScript test (file: title) | Rust test (model::routine_form::tests::...) |
|---|---|
| routine-form.test.ts: "the routine editor: edits days and times" | edits_days_and_times |
| routine-form.test.ts: "the routine editor: says what's missing before saving, and offers a proposal's own time limit" (its `zoneLine` line passes the zone's long name in, because `Intl`'s name lookup is the screen's; same expected text) | says_whats_missing_before_saving_and_offers_a_proposals_own_time_limit |
| routine-form.test.ts: "the routine editor: builds the Recent runs rows" | builds_the_recent_runs_rows |
| (extra, not in TypeScript) | a_run_with_an_empty_or_unreadable_time_or_note_is_as_if_it_had_none |
| (extra, not in TypeScript) | zone_line_gives_only_the_zone_when_the_name_is_missing_or_the_same |

### routines (packages/ui/lib/routines.ts)
There is no `routines.test.ts`: 0 TypeScript test cases. The three tests below check the daemon calls the port makes.
| TypeScript test (file: title) | Rust test (model::routines::tests::...) |
|---|---|
| (none: no test file) | n/a |
| (extra, not in TypeScript) | a_new_routine_is_created_for_the_bot_with_its_text_trimmed |
| (extra, not in TypeScript) | an_existing_routine_is_updated_with_its_whole_draft_and_no_limit_is_sent_as_null |
| (extra, not in TypeScript) | removing_a_routine_deletes_it_by_its_id |

### setup (packages/ui/lib/setup.ts)
| TypeScript test (file: title) | Rust test (model::setup::tests::...) |
|---|---|
| setup.test.ts: "cardView: shows checking, not installed, a failed check, signed out and signing in" | shows_checking_not_installed_a_failed_check_signed_out_and_signing_in |
| setup.test.ts: "cardView: shows the account, and whether Codex's sandbox still needs setting up" | shows_the_account_and_whether_codexs_sandbox_still_needs_setting_up |
| setup.test.ts: "cardView: keeps a sign-in going through checks that report the same sign-in" | keeps_a_sign_in_going_through_checks_that_report_the_same_sign_in |
| setup.test.ts: "cardView: shows a failed setup check on both cards, with Check again" | shows_a_failed_setup_check_on_both_cards_with_check_again |
| setup.test.ts: "liveSignIns: keeps both providers' sign-ins, and the same object, while neither ended" | keeps_both_providers_sign_ins_and_the_same_object_while_neither_ended |
| setup.test.ts: "liveSignIns: ends only the provider whose attempt ended" | ends_only_the_provider_whose_attempt_ended |
| setup.test.ts: "liveSignIns: ends only the provider whose report changed" | ends_only_the_provider_whose_report_changed |
| (extra, not in TypeScript) | an_empty_error_or_plan_is_no_problem_and_no_plan_label_as_in_javascript |

### state (packages/ui/lib/state.ts)
| TypeScript test (file: title) | Rust test (model::state::tests::...) |
|---|---|
| state.test.ts: "freezes the reconnect timing contract" | freezes_the_reconnect_timing_contract |
| state.test.ts: "app state: holds the accounts: unknown until listed, replaced whole by each change, and listed again on a new connection" | holds_the_accounts_unknown_until_listed_replaced_whole_by_each_change_and_listed_again_on_a_new_connection |
| state.test.ts: "app state: keeps bots in creation order and forgets a deleted bot's chat and routines" | keeps_bots_in_creation_order_and_forgets_a_deleted_bots_chat_and_routines |
| state.test.ts: "app state: keeps Everyone first and the other rooms oldest first, updates and removes them, and lists them again on a new connection" | keeps_everyone_first_and_the_other_rooms_oldest_first_updates_and_removes_them_and_lists_them_again_on_a_new_connection |
| state.test.ts: "app state: takes a deleted bot out of every room" | takes_a_deleted_bot_out_of_every_room |
| state.test.ts: "app state: has every bot in Everyone, even one made after the rooms were listed, and only its members in another room" | has_every_bot_in_everyone_even_one_made_after_the_rooms_were_listed_and_only_its_members_in_another_room |
| state.test.ts: "app state: applies new, updated and streaming messages to an open chat, and ignores chats that aren't open" | applies_new_updated_and_streaming_messages_to_an_open_chat_and_ignores_chats_that_arent_open |
| state.test.ts: "app state: rebuilds a reply that was streaming while its chat opened (opened mid-stream)" | rebuilds_a_reply_that_was_streaming_while_its_chat_opened_opened_mid_stream |
| state.test.ts: "app state: shows a message that arrived while its chat was loading exactly once" | shows_a_message_that_arrived_while_its_chat_was_loading_exactly_once |
| state.test.ts: "app state: adds older pages above without duplicates" | adds_older_pages_above_without_duplicates |
| state.test.ts: "app state: keeps loaded chats on a new connection, so nothing blanks, and forgets ones still loading so their screens load them again" | keeps_loaded_chats_on_a_new_connection_so_nothing_blanks_and_forgets_ones_still_loading_so_their_screens_load_them_again |
| state.test.ts: "app state: lists the routine failures waiting to be shown on each connection" | lists_the_routine_failures_waiting_to_be_shown_on_each_connection |
| state.test.ts: "app state: keeps each routine's newest run event, so an open routine editor can fetch its runs again" | keeps_each_routines_newest_run_event_so_an_open_routine_editor_can_fetch_its_runs_again |
| state.test.ts: "app state: records ended sign-in attempts, so an old attempt ending never reads as the new one ending" | records_ended_sign_in_attempts_so_an_old_attempt_ending_never_reads_as_the_new_one_ending |
| state.test.ts: "app state > catching up after a reconnect: replaces from the newest page's oldest message onward when that message is loaded, keeping the older pages" | replaces_from_the_newest_pages_oldest_message_onward_when_that_message_is_loaded_keeping_the_older_pages |
| state.test.ts: "app state > catching up after a reconnect: replaces the chat with the newest page, with more to load, when more than a page arrived while away" | replaces_the_chat_with_the_newest_page_with_more_to_load_when_more_than_a_page_arrived_while_away |
| state.test.ts: "app state > catching up after a reconnect: replays what arrived while the page was being fetched, so a new message and its streamed text survive" | replays_what_arrived_while_the_page_was_being_fetched_so_a_new_message_and_its_streamed_text_survive |
| state.test.ts: "app state > catching up after a reconnect: leaves a chat that isn't loaded alone" | leaves_a_chat_that_isnt_loaded_alone |
| state.test.ts: "app state: keeps a failed history load until a new connection clears it" | keeps_a_failed_history_load_until_a_new_connection_clears_it (ported: the state half, that a new connection drops a failed chat. The other half calls `loadConversation` twice, which is connection.ts and goes with the connection port) |
| (extra, not in TypeScript) | the_flag_says_whether_the_content_changed_for_every_event |
| (extra, not in TypeScript) | an_updated_bot_moves_behind_bots_created_at_the_same_time_as_the_javascript_sort_does |
| (extra, not in TypeScript) | forget_conversation_drops_a_chat_and_says_whether_it_was_there |
| (extra, not in TypeScript) | only_a_loaded_chat_takes_older_pages_and_only_a_loading_one_finishes |

### store (packages/ui/lib/store.ts)
There is no `store.test.ts`: 0 TypeScript test cases. The six tests below are the port's own, for the subscriber mechanism that replaces the TypeScript's listener set.
| TypeScript test (file: title) | Rust test (model::store::tests::...) |
|---|---|
| (none: no test file) | n/a |
| (extra, not in TypeScript) | subscribers_are_told_after_every_change_with_the_new_state |
| (extra, not in TypeScript) | subscribers_are_not_told_when_an_event_changes_nothing |
| (extra, not in TypeScript) | unsubscribing_stops_telling_that_subscriber_and_only_that_one |
| (extra, not in TypeScript) | update_tells_subscribers_with_the_state_its_change_made |
| (extra, not in TypeScript) | a_snapshot_is_a_copy_that_later_changes_do_not_reach |
| (extra, not in TypeScript) | subscribers_are_told_in_the_order_they_subscribed |
| (n/a: `useApp`) | n/a: `useApp` is React's `useSyncExternalStore` hook over `getState`; there is no React in the Rust port, and the screens read the store through GPUI (M5) |

### suggestions (packages/ui/lib/suggestions.ts)
| TypeScript test (file: title) | Rust test (model::suggestions::tests::...) |
|---|---|
| suggestions.test.ts: "Suggestions: every suggestion is a valid bot with valid routines" (`BotInput.parse` and `RoutineInput.parse`: checked with `protocol::wire::Request::decode` for `bots.create` and `routines.create`, the manifest's params schemas that zod's parse mirrors) | every_suggestion_is_a_valid_bot_with_valid_routines |

### timeline (packages/ui/lib/timeline.ts)
| TypeScript test (file: title) | Rust test (model::timeline::tests::...) |
|---|---|
| timeline.test.ts: "labels the first message, gaps of 15 minutes or more, and each new day" | labels_the_first_message_gaps_of_15_minutes_or_more_and_each_new_day |

Extra tests (no TypeScript case): none.

### usage (packages/ui/lib/usage.ts)
| TypeScript test (file: title) | Rust test (model::usage::tests::...) |
|---|---|
| usage.test.ts: "changes identity at local midnight and on timezone changes" | changes_identity_at_local_midnight_and_on_timezone_changes |
| usage.test.ts: "schedules midnight across 23-hour and 25-hour daylight saving days" | schedules_midnight_across_23_hour_and_25_hour_daylight_saving_days |
| usage.test.ts: "starts today at the zone's midnight, not UTC's" | starts_today_at_the_zones_midnight_not_utcs |
| usage.test.ts: "starts the year on January 1st at the zone's midnight, to now" | starts_the_year_on_january_1st_at_the_zones_midnight_to_now |
| usage.test.ts: "starts the week on Monday and the month on the 1st" | starts_the_week_on_monday_and_the_month_on_the_1st |
| usage.test.ts: "lands on local midnight across a daylight saving change" | lands_on_local_midnight_across_a_daylight_saving_change |
| usage.test.ts: "builds a query with the zone and only the filters set" | builds_a_query_with_the_zone_and_only_the_filters_set |
| usage.test.ts: "writes the address for some filters, and reads it back" | writes_the_address_for_some_filters_and_reads_it_back |
| usage.test.ts: "reads the filters from the address, ignoring empty ones" | reads_the_filters_from_the_address_ignoring_empty_ones |
| usage.test.ts: "shortens counts: 950, 12.4K, 1.2M, 3.4B" | shortens_counts_950_12_4k_1_2m_3_4b |
| usage.test.ts: "is $0.00 only when there is nothing, and never shows an unknown cost as $0" | is_0_00_only_when_there_is_nothing_and_never_shows_an_unknown_cost_as_0 |
| usage.test.ts: "keeps spend and API value on lines of their own and never adds them" | keeps_spend_and_api_value_on_lines_of_their_own_and_never_adds_them |
| usage.test.ts: "sizes each day against the busiest, and says the highest in words" | sizes_each_day_against_the_busiest_and_says_the_highest_in_words |
| usage.test.ts: "lists rows busiest first, naming what was deleted" | lists_rows_busiest_first_naming_what_was_deleted |
| usage.test.ts: "is empty when there are no records" | is_empty_when_there_are_no_records |
| usage.test.ts: "shows a subscription's API value and an API key's spend, never both" | shows_a_subscriptions_api_value_and_an_api_keys_spend_never_both |
| usage.test.ts: "shows a bot's day with the cost it has, or $0.00 for nothing" | shows_a_bots_day_with_the_cost_it_has_or_0_00_for_nothing |
| usage.test.ts: "shows a held report only for the spec it answered" | shows_a_held_report_only_for_the_spec_it_answered |
| usage.test.ts: "runs once per interval under a steady stream of pokes, never pushed off" | runs_once_per_interval_under_a_steady_stream_of_pokes_never_pushed_off |
| usage.test.ts: "does not run without a poke, and cancel drops a waiting run" | does_not_run_without_a_poke_and_cancel_drops_a_waiting_run |
| usage.test.ts: "lists every calendar day from the first to the last in the zone, empty ones too" | lists_every_calendar_day_from_the_first_to_the_last_in_the_zone_empty_ones_too |
| usage.test.ts: "reads the days in the report's zone, not UTC's" | reads_the_days_in_the_reports_zone_not_utcs |
| usage.test.ts: "counts a 25-hour and a 23-hour day as one day each across daylight saving" | counts_a_25_hour_and_a_23_hour_day_as_one_day_each_across_daylight_saving |
| usage.test.ts: "stays bounded for a bad range" | stays_bounded_for_a_bad_range |
| usage.test.ts: "pads a report's days so one busy day takes its own slot, not the whole range" | pads_a_reports_days_so_one_busy_day_takes_its_own_slot_not_the_whole_range |
| usage.test.ts: "puts modest date ticks under the bars: the first, the last, and a few between" | puts_modest_date_ticks_under_the_bars_the_first_the_last_and_a_few_between |
| usage.test.ts: "adds the known costs of spend, value and unclassified, once each" | adds_the_known_costs_of_spend_value_and_unclassified_once_each |
| usage.test.ts: "says at least when some turns have no price, and unknown when none do" | says_at_least_when_some_turns_have_no_price_and_unknown_when_none_do |
| usage.test.ts: "is $0.00 for nothing at all, and carries estimates through" | is_0_00_for_nothing_at_all_and_carries_estimates_through |
| usage.test.ts: "draws twelve bars for a full year, however many days it has, with every month named" | draws_twelve_bars_for_a_full_year_however_many_days_it_has_with_every_month_named |
| usage.test.ts: "keeps a leap year's extra day in February and the 31st in December" | keeps_a_leap_years_extra_day_in_february_and_the_31st_in_december |
| usage.test.ts: "stops at the month of the range's last day, and reads months in the report's zone" | stops_at_the_month_of_the_ranges_last_day_and_reads_months_in_the_reports_zone |
| usage.test.ts: "keeps weeks and months as days, with a slot for every day" | keeps_weeks_and_months_as_days_with_a_slot_for_every_day |
| usage.test.ts: "sums by month without losing or doubling tokens" | sums_by_month_without_losing_or_doubling_tokens |

Left out, with the reason: `localZone` (reads the machine's zone; the caller passes the zone in). The throttle's callback and timer are the caller's: `Throttle` takes the time in and says when a run is due.

Extra tests (no TypeScript case): rounds_money_to_cents_the_way_javascript_does_on_exact_ties; a_zone_this_build_does_not_know_gives_no_range_and_no_days.

### format (packages/ui/lib/format.ts)
| TypeScript test (file: title) | Rust test (model::format::tests::...) |
|---|---|
| format.test.ts: "writes times the way the wireframes do" | writes_times_the_way_the_wireframes_do |
| format.test.ts: "names days: today, yesterday, a weekday this week, else the date" | names_days_today_yesterday_a_weekday_this_week_else_the_date |
| format.test.ts: "sizes files and describes routine schedules" | sizes_files_and_describes_routine_schedules |
| format.test.ts: "names tomorrow and the coming week, and writes long dates" | names_tomorrow_and_the_coming_week_and_writes_long_dates |
| format.test.ts: "says how long ago and how long" | says_how_long_ago_and_how_long |

### labels (packages/ui/lib/labels.ts)
| TypeScript test (file: title) | Rust test (model::labels::tests::...) |
|---|---|
| format.test.ts (describe "labels"): "names bots, models and states, and says when a bot is gone" | names_bots_models_and_states_and_says_when_a_bot_is_gone |

### connection (packages/ui/lib/connection.ts)
Five TypeScript files, 36 `it(...)` titles (37 cases: `connection-host.test.ts`'s `it.each(["forget", "refused"])` runs twice, and has a row for each). The rows use the same pinned fixture ids as the TypeScript (`HOST`, the synthetic PC); the PC is `FakePc` in the tests, standing in for `remote-pc-fixture.ts`, and time is a clock the tests move (`advance` is `vi.advanceTimersByTimeAsync`, `settle` is `settle`).

| TypeScript test (file: title) | Rust test (model::connection::tests::...) |
|---|---|
| connection.test.ts: "connection: rejects a call made while Tjommie isn't connected, instead of throwing" | rejects_a_call_made_while_tjommie_isnt_connected_instead_of_throwing |
| connection.test.ts: "connection: rejects a chat's history while Tjommie isn't connected, as the room's does" | rejects_a_chats_history_while_tjommie_isnt_connected_as_the_rooms_does |
| connection.test.ts: "connection: loads a chat whose history failed to load again on Retry" | loads_a_chat_whose_history_failed_to_load_again_on_retry |
| connection.test.ts: "connection: forgets a closed exchange, so its messages stop arriving and a reopen loads it again" | forgets_a_closed_exchange_so_its_messages_stop_arriving_and_a_reopen_loads_it_again |
| connection-start.test.ts: "the first connection: says why after a few failed tries, and keeps trying until the daemon answers" | says_why_after_a_few_failed_tries_and_keeps_trying_until_the_daemon_answers |
| connection-start.test.ts: "the first connection: says in plain words when the daemon can't be reached" | says_in_plain_words_when_the_daemon_cant_be_reached |
| connection-start.test.ts: "a pushed connection: shows Reconnecting while it replaces a live one" | shows_reconnecting_while_it_replaces_a_live_one |
| connection-start.test.ts: "calls in flight: keeps at most 16 calls outstanding, so a burst can't trip the phone host's limit of 32, and answers every call" | keeps_at_most_16_calls_outstanding_so_a_burst_cant_trip_the_phone_hosts_limit_of_32_and_answers_every_call |
| connection.heartbeat.test.ts: "pings every 20 s while in front, and replaces a link whose ping gets no reply within 10 s" | pings_every_20_s_while_in_front_and_replaces_a_link_whose_ping_gets_no_reply_within_10_s |
| connection.heartbeat.test.ts: "opens a new wire the moment the network changes" | opens_a_new_wire_the_moment_the_network_changes |
| connection.heartbeat.test.ts: "rejects a chat.send that was in flight when the link was lost, and never sends it again" | rejects_a_chat_send_that_was_in_flight_when_the_link_was_lost_and_never_sends_it_again |
| connection.heartbeat.test.ts: "races the LAN endpoints: the first to finish the handshake wins and the slower one is closed" | desktop-host/tests/remote_dialer_race.rs: races_the_lan_endpoints_the_first_to_finish_the_handshake_wins_and_the_slower_one_is_closed |
| connection.heartbeat.test.ts: "tries the relay after the 1 s LAN window when every LAN endpoint fails" | desktop-host/tests/remote_dialer_race.rs: tries_the_relay_after_the_lan_window_when_every_lan_endpoint_fails |
| connection.heartbeat.test.ts: "doesn't let a LAN endpoint that connects but stalls its handshake hold the relay back past the 1 s window" | desktop-host/tests/remote_dialer_race.rs: does_not_let_a_lan_endpoint_that_connects_but_stalls_its_handshake_hold_the_relay_back_past_the_lan_window |
| connection.heartbeat.test.ts: "stops heartbeat while hidden and checks the link on return without a foreground service" | stops_heartbeat_while_hidden_and_checks_the_link_on_return_without_a_foreground_service |
| connection.resume.test.ts: "locked 30 min then visible: the dead link is replaced after a bounded ping, without a failed screen, and the open chat catches up" | locked_30_min_then_visible_the_dead_link_is_replaced_after_a_bounded_ping_without_a_failed_screen_and_the_open_chat_catches_up |
| connection.resume.test.ts: "keeps the link through a quick trip away, checking it with a ping instead of reconnecting" | keeps_the_link_through_a_quick_trip_away_checking_it_with_a_ping_instead_of_reconnecting |
| connection.resume.test.ts: "admits fresh file actions after a disconnect only when native thumbnail cleanup is confirmed" | admits_fresh_file_actions_after_a_disconnect_only_when_native_thumbnail_cleanup_is_confirmed |
| connection.resume.test.ts: "keeps authorized native save data when returning from a picker after two minutes on a healthy link" | keeps_authorized_native_save_data_when_returning_from_a_picker_after_two_minutes_on_a_healthy_link |
| connection.resume.test.ts: "keeps only a byte-free pending picker through a ten-minute hidden disconnect and reacquires a fresh client" | keeps_only_a_byte_free_pending_picker_through_a_ten_minute_hidden_disconnect_and_reacquires_a_fresh_client |
| connection.resume.test.ts: "fences selection A to B to A while allowing a same-selection reconnect" | fences_selection_a_to_b_to_a_while_allowing_a_same_selection_reconnect |
| connection.resume.test.ts: "bounds fresh authorization to thirty visible seconds and leaves no active transfer" | bounds_fresh_authorization_to_thirty_visible_seconds_and_leaves_no_active_transfer |
| connection.resume.test.ts: "cancels a pending authorization promptly without a late ping closing a healthy link" | cancels_a_pending_authorization_promptly_without_a_late_ping_closing_a_healthy_link |
| connection.resume.test.ts: "does not cancel a staged native save when a foreground ping expires while the picker is hidden" | does_not_cancel_a_staged_native_save_when_a_foreground_ping_expires_while_the_picker_is_hidden |
| connection.resume.test.ts: "does not let an older unanswered ping replace a link verified after returning" | does_not_let_an_older_unanswered_ping_replace_a_link_verified_after_returning |
| connection.resume.test.ts: "fences an overdue hidden ping before the queued resume check runs" | fences_an_overdue_hidden_ping_before_the_queued_resume_check_runs |
| connection.resume.test.ts: "host offline over relay: the relay's 4404 says the PC is offline, over the bots already listed" | host_offline_over_relay_the_relays_4404_says_the_pc_is_offline_over_the_bots_already_listed |
| connection.resume.test.ts: "can't reach the PC: a saved PC that stops answering shows unreachable over the cached screens, never the failed screen" | cant_reach_the_pc_a_saved_pc_that_stops_answering_shows_unreachable_over_the_cached_screens_never_the_failed_screen |
| connection.resume.test.ts: "revoked: the PC's authenticated refusal is terminal: removed, and nothing reopens" | revoked_the_pcs_authenticated_refusal_is_terminal_removed_and_nothing_reopens |
| connection.resume.test.ts: "backs off 0, 1, 2, 5 and 10 s, then keeps trying every 10 s while the app is in front" | backs_off_0_1_2_5_and_10_s_then_keeps_trying_every_10_s_while_the_app_is_in_front |
| connection.resume.test.ts: "says why a first connect failed, and tries again on return without taking the connect screen away" | says_why_a_first_connect_failed_and_tries_again_on_return_without_taking_the_connect_screen_away |
| connection.resume.test.ts: "stops retrying while hidden without keep-alive, and tries again the moment the app is back" | stops_retrying_while_hidden_without_keep_alive_and_tries_again_the_moment_the_app_is_back |
| connection-host.test.ts: "hard-cancels a byte-free picker on forget instead of rebinding after reconnect" (it.each, "forget") | hard_cancels_a_byte_free_picker_on_forget_instead_of_rebinding_after_reconnect |
| connection-host.test.ts: "hard-cancels a byte-free picker on refused instead of rebinding after reconnect" (it.each, "refused") | hard_cancels_a_byte_free_picker_on_refused_instead_of_rebinding_after_reconnect |
| connection-host.test.ts: "closes previous host file handles before a new host becomes ready" | closes_previous_host_file_handles_before_a_new_host_becomes_ready |
| connection-host.test.ts: "remembers which saved PC the desktop window is on, from the moment it's picked until This PC or Forget" | remembers_which_saved_pc_the_desktop_window_is_on_from_the_moment_its_picked_until_this_pc_or_forget |
| connection-host.test.ts: "forgets a PC that removed this device only once its key is gone: a failed delete leaves the removed screen, the PC and its key as they were" | forgets_a_pc_that_removed_this_device_only_once_its_key_is_gone_a_failed_delete_leaves_the_removed_screen_the_pc_and_its_key_as_they_were |

`admits_fresh_file_actions_after_a_disconnect_only_when_native_thumbnail_cleanup_is_confirmed` ports the connection's half of its TypeScript test: no new link opens before the file cleanup is confirmed. Its `queueThumbnail` and `pauseThumbnailWork` assertions are `file_thumbnails`' (`ThumbnailQueue`), whose header says the wait on native cleanup is not ported there.

Decisions the TypeScript did not make, each with a test of its own in `model::connection::tests`: a close code 4003 says `update` and is not retried (`a_daemon_that_speaks_another_protocol_says_update_and_is_not_retried`, `a_remote_link_that_ends_is_replaced_and_a_pc_on_another_protocol_says_update`; the TypeScript has the code and no test); events a link missed end it and start a reconnect (`events_that_a_link_missed_end_it_and_start_a_reconnect`); a chat load and a Forget finish even if the caller stops waiting (`a_chat_load_whose_caller_gives_up_still_finishes`, `a_forget_whose_caller_gives_up_still_leaves_the_pc`); a call, a wait for the file host and a download that are dropped leave nothing registered (`a_dropped_call_gives_its_place_in_line_back`, `a_download_that_is_dropped_or_waiting_leaves_nothing_registered`); a timer that fires twice, or after its connection was replaced, starts nothing (`a_timer_that_fires_twice_or_after_its_connection_was_replaced_starts_nothing`); `next_timer` and `timers_changed` (the sans-IO clock); `route_push`, the daemon-failed and restart path, Forget on a phone, and events during the startup lists and a catch-up (the remaining tests with no TypeScript counterpart).

Not ported: `desktop()` and its IPC subscriptions (the app calls `on_connection` and `on_daemon_failed`), `document` and the page identity `captureAttachmentSave` compared (a WebView reload), `onNetworkChange` (the app calls `network_changed`), the `outside` status for a missing Electron bridge, and `invalidateThumbnailWork` (the app retires its `ThumbnailQueue` inside `Environment::invalidate_files`).
