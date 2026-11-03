# Changelog

## Unreleased

- Preview shared Markdown, images, PDFs, audio and video, and download delivered files on the device you are using.
- Markdown files can include explicitly shared companion images, with the same file permissions and cleanup as their document.
- Large Markdown files open in readable sections, and PDFs use bounded range reads with selectable text, search and zoom.
- Switching PDF pages no longer leaves text from the previous page.
- Shared files work when Windows uses a shortened folder path.
- Returning from Android's Save picker keeps a healthy connection and its staged download.
- Create project teams with bot roles, tasks and separate Git worktrees. Review and checks stay attached to the exact task commit before integration.
- Task helper replies continue in the original managed worktree and keep the task active until they finish.
- Settings now groups appearance, notifications, files, voice, diagnostics and About while preserving existing settings links.
- Choose larger text and compact spacing on each device; preview, copy or save a diagnostic report without raw logs or private conversation data.
- Dictate an editable message draft using supported local desktop or Android speech engines before sending it.
- View a bot's computer from a paired device, take control and return it to the bot with private-mode safeguards.
- Manage saved website logins on the host, with exact-site confirmation and private browser filling after Windows worker isolation is verified. Bots can ask you for a named login without reading its secret.
- Configured project checks use the same Windows worker isolation as bots when it is enabled.
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
