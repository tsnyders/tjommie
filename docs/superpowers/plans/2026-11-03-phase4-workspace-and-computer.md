# Phase 4: files, developer teams, settings, dictation and computer

**Status:** accepted by the owner on 2026-11-03; implementation authorized with multiple GPT-6.1 Sol agents at maximum reasoning. Release publication remains a separate action.
**Date:** 2026-11-03.
**Source baseline:** `origin/main` at `aaaf09fe6c45cce2fe42bf9488ac3815156f4ad9` (v0.3.2).
**Phase 3:** accepted by the owner in this conversation. Do not reopen its acceptance gate.

## Confirmed scope

The owner requested these additions to the original Phase 4:

1. Preview agent-shared/generated Markdown, images, PDFs, audio and video, and download them on the device being used.
2. Developer teams: AI bots assigned to a project, with roles, tasks and Git worktrees.
3. More fitting settings, including customization, diagnostics and About.
4. Voice dictation on Windows desktop and Android, preferring local transcription, with editable text before sending.
5. Retain the original browser/computer work and Logins vault alongside these additions.

This plan proposes defaults below; those defaults are not additional owner decisions. Human collaboration accounts, voice conversations, a general desktop-control agent and a cloud file service are outside this scope.

## Current implementation to reuse

The active checkout's local `main` is an old scaffold and contains unrelated uncommitted files. The implementation must start in a separate worktree from the current remote branch. This document alone is written into the existing checkout; it does not update that checkout or its README.

| Existing behavior | Source at the baseline | Phase 4 consequence |
| --- | --- | --- |
| `share_file` validates roots, takes a bounded snapshot and retains delivered files in a bot inbox. Existing sharing limit: 200 MB. | `packages/daemon/src/team.ts`, `team.paths.test.ts`, `team.snapshots.test.ts` | Extend the shared-file lifecycle; do not expose arbitrary host paths through a new download API. |
| File cards contain a path, name and size. Their apparent download button actually reveals the file in Explorer, only on the host PC. | `packages/protocol/src/model.ts`, `packages/ui/components/chat/file-card.tsx` | Add real Preview and Download actions, including on a paired device. Keep Show in folder as a separate local action. |
| Markdown rendering and GFM are installed. | `packages/ui/components/chat/markdown.tsx`, `packages/ui/package.json` | Reuse the renderer with file-specific link/image handling. |
| Remote application messages are limited to 16 MiB and fragmented into bounded Noise records. | `packages/client/src/encrypted-channel.ts`, `packages/transport/src/index.ts` | Transfer bounded file chunks over the existing encrypted channel; never put an entire large file into a chat/RPC payload. |
| Rooms, membership, bot links, queues, routines and account pinning exist. No project-task/worktree service was found in the inspected daemon/protocol sources. | `packages/daemon/src/{rooms,team,runner}.ts`, `packages/protocol/src/{model,wire}.ts` | Reuse rooms and runner scheduling; add only project/task/Git ownership. |
| Settings has General, Bots, Accounts, Usage and Devices. Theme, mobile styles, reduced effects and desktop startup/update controls exist. | `packages/ui/app/settings/page.tsx`, `packages/daemon/src/settings.ts` | Reorganize and extend these settings instead of duplicating them. |
| Native desktop browser tabs, shared human/bot sessions, exclusive manual control, panel resizing and expand/restore already exist. The current BrowserPanel requires the desktop native bridge. | `packages/ui/components/chat/browser-panel.tsx`, `packages/desktop/src/native-browser-*.ts` | Preserve those behaviors; finish remote viewing/control and live thumbnails rather than rebuilding expand/restore. |
| Android native plugins are maintained under `packages/mobile/native/` and copied into the Android project by native sync. | `packages/mobile/scripts/apply-native.mjs` | Edit plugin sources and their source tests, not only generated Android copies. |

## A. Files: preview and download

### User experience

- A file card shows its filename, type and size, with Preview and Download. Unsupported preview types still offer Download.
- Generated deliverables are published through the existing `share_file` tool and appear as cards in the relevant conversation. Update bot instructions and both provider acceptance fixtures so delivery does not depend on an unusable raw host-path link in the final reply. Do not scan arbitrary project files for automatic publication.
- Desktop opens a preview panel with expand/restore. Android opens a full-screen viewer with Back. Closing returns focus and preserves the chat draft/scroll position.
- Markdown: headings, lists, tables, code blocks, internal anchors and controlled links. Relative images can resolve only to explicitly shared companion files; no host filesystem crawling or automatic remote image requests.
- Images: fit, zoom, pan and reset. Support a tested set of common raster formats; SVG and active formats remain download-only until safe rendering is established.
- PDF: a real reader with page count/navigation, zoom, text selection and search. Handle password-protected documents with a session-only password and a clear error when unsupported. No editing, OCR, form submission or annotations in this phase.
- Audio/video: standard playback controls, seeking, volume, duration and fullscreen video where supported. No autoplay. Unsupported codecs have a download fallback; do not advertise every audio/video format as playable.
- Download saves to the current device: desktop Save As; Android's system document picker. Cancelling does not report success, leave a completed-looking partial file or change the source file.
- Loading, progress, cancel, missing/deleted file, offline host, revoked device, corrupt content and unsupported format each have an explicit state.

### File delivery and persistence

- Refer to a published attachment by message ID plus attachment identity/index. The daemon resolves it to a registered durable snapshot; clients never submit an arbitrary filesystem path for reading.
- Add a persistent attachment record containing its ID, owning host/message, durable storage reference, original filename, byte length, detected content type and content digest. Keep existing message data readable and backfill/lazily register only validated legacy shares. A stale legacy path becomes Unavailable, never a broader filesystem read.
- Preserve the current safe snapshot behavior when agents share a file. Register and retain it before announcing availability. Changing/deleting the source afterwards must not change the delivered bytes. Link exchange attachments to the same delivery lifecycle; do not leave room/exchange cards pointing only to disposable staging files.
- Proposed RPC shape: metadata/open, bounded range/chunk reads, close/cancel. A transfer is bound to the authenticated device/session and attachment revision. Validate authorization and revocation on every read.
- Keep the existing share limit for this release. Proposed starting chunk size: 256 KiB raw, one outstanding read per transfer and at most two active transfers per device. Validate these limits in the transfer spike; they are not measured performance claims.
- Start with bounded base64 chunks inside existing RPCs, below current message limits, rather than a separate unencrypted HTTP server. Measure overhead before considering a binary protocol extension.
- Stream large downloads to a native temporary destination, verify completion/digest, then finalize the user-selected save. Use bounded byte ranges for media seeking and PDF reads; do not assemble every 200 MB file in the renderer or send it across one IPC call.
- Local and remote preview URLs are scoped, short-lived handles that only serve the already authorized attachment. They must not expose daemon credentials, raw paths or a network-accessible file endpoint.
- Desktop can bridge the bytes through a scoped protocol handler; Android needs a bounded app-owned content/WebView bridge. Prove range/seek and PDF access on both before building all viewers.
- Revoke/host-switch/disconnect stops active reads and clears preview buffers/cache. Already explicitly saved downloads remain the user's copies. No new offline preview cache in this phase.
- Bind attachment retention to message ownership and active-transfer leases. Delete unreferenced managed copies only after leases end; do not remove inbox files merely because a preview closed. Migration and cleanup must preserve existing chat history and delivered files.
- Treat content as untrusted: no HTML execution, scripts, external resource fetches or executable auto-open. Isolate the PDF viewer and disable document scripting/active actions. Validate actual type/size and constrain Markdown resource links.

### Dependencies and acceptance

Use the installed Markdown renderer and platform image/audio/video controls. Propose the maintained Mozilla PDF.js viewer as the one justified PDF dependency; vendor/bundle its worker and viewer assets locally and preserve licenses. Confirm CSP, worker loading, static export and Android behavior before pinning a release. No hosted document viewers.

Acceptance: an agent shares one of each requested type; Windows, another PC and Android can preview and save identical bytes over the existing connection. A large video can seek without loading the whole file. An in-progress transfer does not prevent chat, Stop or approval handling. Revocation stops further reads. Tests cover unauthorized IDs, cross-host mixups, traversal/link swaps, malformed chunks/ranges, missing files, source mutation, cancellation, interrupted saves and renderer memory bounds.

## B. Developer teams

### Smallest complete workflow

1. Create a project team: choose an existing Git repository on its host PC, a base branch and participating bots. Remote clients may select repositories already registered on that host; they cannot submit their own local paths as host paths.
2. Assign roles using editable role instructions. Start with Lead, Developer, Reviewer and Tester presets; retain each bot's existing provider/account/autonomy settings. One bot can have multiple role labels but only one active task attempt at a time.
3. Tjommie creates or links a normal team room for discussion, @mentions and task links. Membership does not silently broaden bot-to-bot messaging permissions.
4. Add a task with a title, instructions, acceptance checklist, assignee and optional reviewer/tester. A lead can propose subtasks and assignments; the user starts the batch. Begin with a task list and detail view, not a separate planning product.
5. Each coding task attempt receives its own branch and worktree from a recorded base SHA. Its assigned developer works there; review/test jobs inspect that same attempt. Different tasks never concurrently edit the same worktree.
6. Show task state, current owner, worktree/branch, diff, reported checks and independently verified check results. Suggested states: Todo, Running, Review, Done, Blocked and Cancelled. Keep execution result separate from integration status.
7. A reviewer reports findings without editing files. A tester runs the configured project checks in an isolated execution context and reports command, exit status and commit/attempt. Findings return the task to its developer. Changing code invalidates prior review/test acceptance.
8. The user explicitly chooses Integrate for an exact reviewed commit. Integration uses a separate managed worktree, checks the current target head, and reruns the required combined checks. Conflicts become Blocked with a concrete resolution path. Never silently resolve by choosing one side.
9. Merge locally only after those gates; pushing or opening a PR is a separate explicit action. No automatic remote push, force push, rebase, stash, branch deletion or destructive cleanup.

### Model and ownership rules

- Add minimal project/team, role-assignment, task and task-attempt records. Reuse room messages, runner work items and existing approvals instead of another bot runtime or event bus.
- Record repository identity, canonical host path, base/target refs, attempt branch, worktree path, owner, timestamps, status, reviewed/tested commit and integration result. Match the actual current DB migration number at implementation time; do not reuse migration numbers from old plans.
- Proposed starting restriction: a bot belongs to one project team at a time. Reject assigning a busy bot or changing its project binding while queued/running work still depends on it. Keep ordinary standalone bots and existing rooms working.
- Task execution carries its project/worktree context through queue persistence, resume, bot requests and file sharing. Do not mutate a bot's folder halfway through a turn or resume a CLI session created for a different worktree.
- Preserve one active turn per bot, spending/account limits, existing autonomy rules, Stop, interruption recovery and chain limits. Proposed task assignment must not start paid work before the user starts that task/batch.
- Execute Git through argument arrays and canonical paths; validate refs and branch ownership. Never modify repository/global Git identity, hooks or credentials as a convenience. Report absent identity/authentication as a setup issue.
- A dirty user checkout is preserved. Use a managed worktree based on an explicit commit without switching or cleaning the user's checkout. Never overwrite an existing branch/worktree.
- Worktree separation prevents accidental simultaneous edits; it is not a security sandbox. Reviewer execution must deny mutation tools and run against read-only source/isolated scratch space where the platform supports enforcement. If enforcement cannot be established, block the automated reviewer rather than claim it is read-only. Do not run repository scripts as the reviewer.
- Test commands execute project code and can write or contact services. Run only checks configured/accepted for that project under existing autonomy limits, separate from the reviewer's read-only session. A test result is not a source review.
- Stop/cancel preserves branches and edits. Restart reconciles persisted attempts with actual worktrees without rerunning completed work. Worktree removal is an explicit cleanup flow that shows dirty/untracked files and never discards them implicitly.
- Removing a team must not delete its repository, task history or worktrees. Leave retained work visible for cleanup and prevent new dispatch.

Acceptance: two developer bots complete independent tasks in separate worktrees while a reviewer reports a defect without editing. One task returns for a fix; review/test results become stale until rerun. Both accepted tasks integrate safely, with combined checks. Restart, Stop, dirty checkout, deleted worktree, conflicting changes, target-head movement and failed tests preserve work and report accurate state. Human users, organization permissions, CI/CD orchestration and a hosted Git service are not added.

## C. Settings

Retain existing Accounts, Usage, Devices and Bots sections, including old deep links. Proposed grouping:

| Section | Contents and persistence |
| --- | --- |
| General | Existing launch-at-login, tray/close behavior and update controls. Label controls belonging to the connected host versus the current device. |
| Appearance | Existing theme/mobile styles/reduced effects; add readable text sizing and comfortable/compact spacing using current design tokens. Persist per device; follow system accessibility defaults unless explicitly changed. |
| Notifications | Expose existing device notification choices coherently. Add optional local notification sound/mute controls without changing approval delivery policy. |
| Files | Download behavior, preview resource limits/status and clearing temporary preview data. Keep Save As as the initial default; no unnecessary storage permission. |
| Voice | Input device on desktop, language, local engine/model status and cancel/remove downloaded model. State where transcription happens. No cloud fallback switch until a cloud option is separately chosen. |
| Diagnostics | App/daemon/provider versions, connected host, relay/direct state, last connection error, storage/model/permission health; no paid model turn for a health check. |
| About | Current-device app version, connected-host version, update link/state, project/source links, license and third-party notices. |

Diagnostics offers a previewable, redacted report that the user can copy/save. No automatic upload or chat-database attachment. Exclude tokens, passwords, pairing codes, private keys, raw transcripts, message bodies and file contents; redact personal paths, account identifiers and credential-bearing URLs. Use structured allowlisted fields rather than dumping all logs. Clearing diagnostics/cache must not remove chats, accounts, saved downloads, worktrees or vault entries.

Acceptance: settings persist in the correct scope; switching host never changes another PC's settings accidentally. Old links work. Report fixtures containing synthetic secrets cannot leak them. Keyboard navigation, screen-reader names, text scaling and phone layouts remain usable.

## D. Voice dictation

- Add a labeled microphone action to bot chat, room and exchange composers. Use the same draft insertion logic; leave existing text and attachments intact.
- Tap to start, tap to stop; allow Cancel. Show listening/transcribing/error states and an accessible status announcement. Stop microphone capture on cancellation, navigation, backgrounding, host switch or composer unmount.
- Transcript goes into the initiating draft at the captured insertion range. It never auto-sends, never becomes a tool command and never overwrites text typed while transcription runs. Late results cannot land in another conversation.
- Use local transcription on the capturing device by default, including Android when the host PC is offline. Ask for microphone permission when recording starts. Do not silently switch to a cloud recognizer when local recognition is unavailable.
- Windows proposal: a bundled, supervised whisper.cpp runtime with a separately downloaded, verified model. Prefer CPU operation initially; do not require a GPU. Record binary/model provenance, redistribution licenses, download size and checksum before adding them to release assets.
- Android proposal: explicitly use the native on-device recognition API when the selected language is supported. If unavailable, evaluate whisper.cpp locally as a fallback in the spike. Otherwise show an honest unavailable/download-needed state; do not substitute an online service.
- Start with selectable supported languages and a tested English default. Do not promise South African languages, accent accuracy, punctuation or technical identifier accuracy without fixtures/device evidence.
- Raw audio is temporary, bounded and deleted after completion/cancellation/error; no raw audio or transcript in diagnostics/logs. Bound recording duration and runtime resources. Let the user cancel model download and transcription.

Acceptance: offline dictation on supported Windows/Android configurations produces editable text, works with an existing draft, handles permission denial, silence, cancellation, incoming messages, typed edits and navigation races, and never sends without pressing Send. Test silence/noise so hallucinated text is not accepted as a successful dictation. Establish measured engine/model latency, memory, disk and battery behavior before promising performance.

## E. Computer/browser and Logins vault

### Finish the browser surface

- Preserve native desktop tabs, persistence, manual control, resizing and expand/restore.
- Add a clear Computer view for the selected bot and live browser thumbnails on visible agent cards. Thumbnail capture is low-rate, only while visible, and stops for hidden/minimized/backgrounded screens.
- Add authenticated remote viewing and Take control for paired Android/PC clients, backed by the host's existing native browser engine. Reuse existing frame/input schemas where suitable, but first verify their native-host integration; protocol definitions alone do not establish a working remote path.
- Only one human controller can own a bot's browser at a time. Generation/page/viewport checks reject stale frames and input. Disconnect/revocation/backgrounding releases keys, pointer and control. Never restore bot control while a private vault-fill interaction is still active.
- Keep Pause distinct from Stop: Pause prevents further bot work/browser actions after the current bounded operation, retains task state and does not claim rollback. Stop interrupts the current turn using existing semantics and preserves user data. Human takeover pauses browser automation; explicit Pause also prevents that bot issuing other tools until resumed. Implement this in the runner/tool gate, not just a disabled toolbar button.
- Frame/input traffic is ephemeral, end-to-end encrypted on remote paths and absent from chat/history/logs. Browser pages remain isolated from the app preload and filesystem. No general Windows desktop streaming, arbitrary app control or VM target is included.

Acceptance: inspect the same bot browser on Windows and Android; transfer human control safely; type a synthetic PIN; pause/resume and Stop; reconnect with no stuck keys or stale input. Thumbnails stop when hidden and are suppressed during private interactions. No real account credentials are needed for automated tests.

### Vault: contract must be explicit

The original roadmap says a test proves a password never reaches the model. That wording is too broad for arbitrary websites: a site receiving a password can re-expose it in page content. A passing test on a synthetic site does not prove otherwise.

Proposed enforceable contract: **Tjommie never includes vault secrets in model prompts, chat/events, tool results, screenshots it captures during fill, logs or diagnostics.** Website behavior beyond that boundary is not guaranteed. Resolve this wording before implementing vault automation; do not silently claim the original universal guarantee.

- Keep encrypted secrets on the host using existing OS credential protection patterns; database records contain metadata and opaque references, not plaintext. No plaintext sync/export and no secrets in normal renderer state.
- First version: user-managed website username/password entries, bound to an exact HTTPS origin and bot browser profile. No password generation, TOTP seed storage, passkey management or vault sharing.
- A bot can request a named login but cannot read a secret. The person selects/confirms the fill in trusted app UI, with the exact destination shown. Never accept a bot-supplied selector/script that can redirect a secret into an arbitrary field or site.
- Fill through a narrowly scoped native operation. Pause bot browser tools, DOM inspection, evaluation, network/body capture and screenshots/thumbnails during the private interaction; fence queued and late operations too. Remote clients may request the host operation using references, not receive plaintext credentials.
- Refuse insecure/mismatched origins, unexpected navigation, unsupported frames and stale page generations. Keep automation paused until the secret fields are cleared/submitted and the user explicitly resumes. Do not assume masking a password field alone prevents leakage.
- Lock/unlock and deletion are explicit. Deleting an entry removes the OS secret/reference without deleting the browser profile or signing the user out unexpectedly. Vault metadata is redacted from diagnostic reports where necessary.

Acceptance: exact-origin fill on a synthetic login, origin/navigation/iframe mismatch refusal, cancellation, revoke, queued-tool races and restart. Place a unique synthetic secret in the vault and prove absence from captured prompts/results/events/logs/diagnostic exports and application-generated images. Document the website boundary and tests' limits. Do not use or inspect owner credentials for this work.

## Architecture decisions (proposed)

| Decision | Selected approach | Alternative and consequence |
| --- | --- | --- |
| File access | Registered snapshots plus authenticated chunk/range reads over current RPC/Noise. | Whole-file payloads break size/memory limits; a public file server expands the security surface. Base64 has overhead to measure. |
| PDF | Bundle PDF.js locally. | Native iframe/PDF handling would need separate platform behavior and cannot be assumed sufficient for Android; adding the reader increases bundle size. |
| Developer teams | Project/task metadata around existing bots, rooms and runner. | A second orchestration runtime duplicates scheduling, approvals and account-limit handling. Worktree and review isolation still require real implementation. |
| Voice | Local capture/transcription on each device; engine availability established first. | Cloud transcription needs a separate consent/data/cost decision. Local models add packaging, storage and hardware variability. |
| Vault | OS-protected host secrets and user-mediated native fill with private browser state. | A general password-manager or unrestricted bot secret API is larger and cannot support the stated disclosure boundary. |

## Delivery order and work packages

Do not start all features at once. Each milestone leaves a demonstrable result and a plain-English changelog entry for its user-visible changes.

| Order | Work package | Must produce before the next dependent package |
| --- | --- | --- |
| P0 | Lock UX flows, acceptance fixtures and the vault contract. Spike encrypted file ranges/native saves/PDF-media seeking; local speech on Windows/Android; native remote browser frames; read-only review execution. | A brief measured decision record for each uncertain boundary, selected dependencies/models and explicit unsupported cases. No production deployment or owner-data access. |
| P1 | File metadata/storage lifecycle, range RPC, revocation/cancel, native saves and scoped preview serving. | Working Windows/Android download of identical bytes, bounded-memory large-file proof and authorization tests. |
| P2 | Markdown/image/PDF/audio/video viewers and chat/room/exchange integration. | All requested formats demonstrated on both platforms, unsupported-codec fallback, accessible navigation. |
| P3 | Projects/team roles, persistent tasks/attempts, runner context and safe worktree lifecycle. | Two concurrent independent task attempts with restart/Stop recovery and preserved user checkout. |
| P4 | Review/test loop, diff/task UI and exact-commit integration. | Read-only reviewer, stale-result invalidation, conflict handling and combined integration checks. |
| P5 | Settings organization, customization, diagnostics and About. | Correct local/host persistence, redacted export and retained deep links. Can proceed once P0 establishes file/voice configuration needs. |
| P6 | Dictation runtime/plugin, model lifecycle and common composer behavior. | Offline editable dictation on both supported platforms, lifecycle/race tests and measured hardware results. Depends on P0, not on task orchestration. |
| P7 | Computer view, native remote browser control, thumbnails and Pause. | Single-controller/generation safety across desktop/Android with private-mode hooks. |
| P8 | Vault metadata/OS store, trusted UI and private fill. | Synthetic-secret disclosure suite and documented threat boundary. Depends on the P0 contract and P7 private-mode enforcement. |
| P9 | Integration/demo, compatibility migration and GitHub release. | All milestone gates plus the final owner demo; then OPS-managed reviewed merge/tag and GitHub Actions build/release. |

### Change reach and ownership

- Protocol/client: attachment, task/project, pause/private-browser contracts; validated errors/events; client helpers and fixtures. Strict protocol-version handshake means coordinate any wire changes and app updates; do not promise mixed-version compatibility without negotiation.
- Daemon: append-only migrations, legacy attachment records, transfer authorization/lifecycle, rooms/team/runner context, task attempts, Git integration, pause gate and settings/diagnostics.
- UI: file cards and all attachment presentations, viewers, project/task routes and states, existing bot/room/exchange composers, browser controls/agent cards, settings and common error/loading/accessibility behavior.
- Desktop: minimal preload/IPC, current-device saves, scoped file serving, local speech runtime, native browser captures/control/private fill and OS credential references. Keep secret operations out of generic renderer APIs.
- Android: native-source plugins/tests for save/content serving and local speech; existing transport/lifecycle/permission handling; generated copies through normal native sync.
- Release/config: PDF assets/worker/CSP, speech binary/model provenance/licenses, migration tests, Android manifest changes, packaged smoke fixtures, CHANGELOG and CI. OPS owns version/tag/publish; builds/releases run on GitHub Actions only.
- If the implementation is delegated later, assign non-overlapping file lanes after contracts are agreed. Reviewers report findings without editing; implementation owners fix them. Do not infer delegation or publication authorization from this planning document.
- Never use `any` as a TypeScript type.

## Verification and final demo

Use disposable repositories/profiles, synthetic documents/media/login secrets and approved speech fixtures. Unit/contract tests cover trust boundaries, state transitions and recovery; real platform checks cover native behavior that unit tests cannot establish.

1. Files: preview and save every supported type on Windows and Android; verify hashes, seeking, cancellation, offline errors and revocation during transfer.
2. Teams: lead proposes tasks; two developers use separate worktrees; reviewer returns a finding without edits; fix/test/review; user integrates exact accepted commits. Restart and a conflicting task preserve all work.
3. Settings: theme/text sizing/spacing persist on the right device; host controls are labeled; diagnostic preview/export contains no synthetic secrets; About distinguishes device and host versions.
4. Voice: dictate locally offline into each composer, edit, then explicitly Send. Denied permission, cancellation, silence and navigation never send or misplace text.
5. Computer/vault: remote view and safe takeover on Android, visible thumbnails, Pause/Stop, exact-origin synthetic fill and explicit resume. Demonstrate the disclosure boundary and its limitation.
6. Run affected tests, typecheck/no-any, lint and integration tests once the change is integrated. Add migration/protocol incompatibility fixtures and a secret scan; do not equate hosted tests with physical-device proof.
7. Check narrow layouts at 360/390 px and desktop at 1280/1440 px, text scaling, keyboard/screen reader focus, and real Android system pickers/media/microphone behavior. Record screenshots and hardware/OS/app versions used.
8. Preserve the owner's installed application/data throughout development. Installation, cloud deployment, release publishing and pushes require their own explicit task authorization. The final release builds and packaged checks run in GitHub Actions.

No implementation estimates or performance guarantees are assigned before P0. Do not turn a failed privacy/isolation/streaming spike into an undocumented fallback.

## Sources checked for this plan

- Repository baseline above, original design section 12 and the concrete source paths listed under Current implementation.
- [PDF.js getting started](https://mozilla.github.io/pdf.js/getting_started/) and [viewer integration](https://github.com/mozilla/pdf.js/wiki/Setup-pdf.js-in-a-website): locally bundled reader candidate; exact version remains to be selected.
- [Electron protocol API](https://www.electronjs.org/docs/latest/api/protocol): scoped local resource-serving candidate; security/range behavior still needs a Tjommie spike.
- [Android document storage](https://developer.android.com/training/data-storage/shared/documents-files): user-selected save destinations through the Storage Access Framework.
- [Android SpeechRecognizer](https://developer.android.com/reference/android/speech/SpeechRecognizer): explicit on-device availability/creation rather than assuming the default recognizer stays offline.
- [Windows speech recognition](https://learn.microsoft.com/en-us/windows/apps/develop/input/speech-recognition): general dictation cannot be assumed offline merely because it uses a platform speech API.
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp): local Windows/Android runtime candidate; no engine/model quality or packaging acceptance is claimed by this plan.
