# Desktop test map (Phase 3 plan T0.6)

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
