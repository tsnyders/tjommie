# Changelog

## Unreleased

- Markdown tables keep readable columns and scroll sideways in chats and file previews; short files have simpler search controls.
- Aurora phone dialogs use solid backgrounds so the chat no longer shows through file previews and forms.
- New browsers and tabs open DuckDuckGo, matching address-bar searches, while existing pages stay open.
- Browser pages stay aligned when the app is zoomed; the toolbar has roomier tabs and aligned navigation.
- Saved logins lives in the browser's three-dot menu; menus and login dialogs keep browser control with you until you close the browser panel or resume the bot.

## 0.4.1 — 2026-11-03

- File cards and previews reuse the original compact styling and shared controls while keeping real content thumbnails.
- Developer teams, settings, voice and browser logins now use the app's existing cards, menus, fields and pill buttons.
- Chat, room and exchange composers and dismissible sign-in toasts match the original sizing, spacing and colours, with larger touch targets on phones.

## 0.4.0 — 2026-11-03

- Shared file cards now put real image, video, PDF and Markdown thumbnails above readable filenames, with download actions kept separate.
- Desktop file previews recover normally when you leave a chat and return.
- Chat, room and exchange inputs use aligned touch controls; voice settings open on demand and idle dictation guidance no longer fills the space below the input.
- Provider sign-in and account-check reminders are dismissible toasts with an account-settings shortcut instead of a permanent chat banner.
- Dark input hints are easier to read, and long file and project titles wrap inside dialogs.
- Developer team pages keep controls clear of the phone navigation, focus selected task details, and ask before archiving a team.
- Preview shared Markdown, images, PDFs, audio and video, and download delivered files on the device you are using.
- Markdown files can include explicitly shared companion images, with the same file permissions and cleanup as their document.
- Large Markdown files open in readable sections, and PDFs use bounded range reads with selectable text, search and zoom.
- Switching PDF pages no longer leaves text from the previous page.
- Shared files work when Windows uses a shortened folder path.
- Android downloads now ask where to save before transferring the file, reconnect securely after the picker closes, and cancel active transfers when the connection is lost.
- Android warns if a save provider keeps an incomplete file after cancellation or a failed download.
- Android dictation now asks for microphone permission on first use before checking local speech languages, while keeping denied permission and unavailable speech services blocked.
- Android voice controls show clearer speech-service errors and can request English (United States) language data without starting a recording.
- Quitting the desktop app now finishes shutdown after temporary file and speech cleanup.
- Full-screen phone dialogs keep their controls clear of system bars.
- Create project teams with bot roles, tasks and separate Git worktrees. Review and checks stay attached to the exact task commit before integration.
- Task helper replies continue in the original managed worktree and keep the task active until they finish.
- Settings now groups appearance, notifications, files, voice, diagnostics and About while preserving existing settings links.
- Choose larger text and compact spacing on each device; preview, copy or save a diagnostic report without raw logs or private conversation data.
- Dictate an editable message draft using supported local desktop or Android speech engines before sending it.
- View a bot's computer from a paired device, take control and return it to the bot with private-mode safeguards.
- Manage saved website logins on the host, with exact-site confirmation and private browser filling after Windows worker isolation is verified. Bots can ask you for a named login without reading its secret.
- Configured project checks use the same Windows worker isolation as bots when it is enabled.
- Isolated Windows launches support longer provider and review arguments while keeping their input private and their process trees tracked.
- Pending login operations are cancelled safely when the app closes or reconnects.
- Private sign-in console output stays hidden after browser control returns to a bot.
- Update paired devices together for the new connection protocol.

## 0.3.2 — 2026-10-30

### What's new

- The PC now shows notices when bots finish or work fails. Paired phones can receive matching done and failed alerts, including for routines and time limits.
- On the phone, choose separately whether to receive "A bot needs you", "A bot is done", and "Something failed" notifications.
- Tapping a notification about a room opens that room.
- Approval requests can still reach the phone after other notifications have used the hourly limit.
- The relay now carries done and failed alerts and keeps the room information needed to open the right conversation.
- Actions that change a PC or open its local files are available only when connected to that PC.
- The PC gives clearer update errors and recovers more reliably if its background service fails to stop. Phone push setup and removal are more reliable too.
- Updates and the Android APK are published on GitHub Releases.

## 0.3.0

- Use the phone over mobile data through the relay, with push alerts when a bot needs approval.
- Connect another PC, start the PC app when you sign in, and receive app updates automatically.
