# Phase 4 dispatch

Owner accepted `../plans/2026-11-03-phase4-workspace-and-computer.md` and requested multiple GPT-6.1 Sol agents at maximum reasoning on 2026-11-03.

Integration worktree: `C:/Users/tyrel/.paseo/worktrees/2cb7zj88/tjommie-phase4`.
Branch: `phase4-workspace-20261103`, baseline `aaaf09fe6c45cce2fe42bf9488ac3815156f4ad9`.

The original dirty checkout is preserved. The original dispatch was source-only; the owner's subsequent request to implement all remaining work authorizes isolated CI and device acceptance. See `acceptance-20261103.md` for the current evidence and boundaries. No production release or owner-data migration has occurred.

## First wave: P0

| Agent | Model / reasoning | Assigned output | State |
| --- | --- | --- | --- |
| `phase4_files` | `gpt-6.1-sol` / `max` | `p0-files.md`, `scripts/phase4-spikes/files/**` | P0 closing; P1/P2 source candidate lane released with native/PDF gates retained. |
| `phase4_teams` | `gpt-6.1-sol` / `max` | `p0-teams.md`, `scripts/phase4-spikes/teams/**` | P0 recorded; P3/P4 source lane released, provider reviewer enforcement remains open. |
| `phase4_voice` | `gpt-6.1-sol` / `max` | `p0-voice.md`, `scripts/phase4-spikes/voice/**` | P0 recorded; P6 source lane released, runtime/CI/hardware acceptance remains open. |
| Coordinator | Parent agent | `p0-computer.md`, `scripts/phase4-spikes/computer/**`, accepted plan and this manifest | Browser/vault prerequisites, common contract decisions and integration. |

Agents may read the full implementation. In this first wave, they only write their assigned decision record and isolated validation fixtures. Only the coordinator installs dependencies. No agent edits another lane, shared protocol/DB/wiring, manifests or lockfiles without a subsequent explicit assignment.

Each decision record must distinguish measured behavior, source evidence and unverified hardware/native behavior. A failed isolation/privacy/streaming check is not permission to silently weaken the accepted design. The next wave receives concrete file ownership after those results are reconciled.

Reviewers report findings without edits. Builders fix their own lanes. No TypeScript `any` types.

## Source-wave ownership

- Files: new attachment protocol/service/handler/save/serving modules and tests; file card/viewers/range helpers; native attachment plugin sources. Excludes voice-owned composers/exchange-view and all shared wiring. Exact source allowlist was issued in the agent dispatch.
- Teams: new project/task/worktree/integration schemas/services/UI/tests; runner, queue and required provider task-context/role-policy seams. Coordinator owns `team.ts`, `bots.ts`, DB/daemon/global UI wiring. Proposed migration lives in `teams-migration.sql` until applied centrally.
- Voice: local speech schemas, capture/worklet/draft safety, all three composers and tests, desktop runtime/model modules, Android source plugin/session and tests. Also temporarily owns `packages/ui/lib/desktop.ts` speech typing. Shared host/preload/native registration/workflow wiring remains central.
- Coordinator: trusted `RequestContext` transport seam, migrations/common protocol/daemon/UI registration, main/preload/manifests/lockfiles/CHANGELOG, settings and computer/vault prerequisites. Request context tests passed **32/32** across local WS and actual Noise LAN/relay, including parameter spoofing, connection lifetime and revocation. Baseline browser handler/permission tests passed **4/4**. No-any passed across **466** TypeScript files at that checkpoint.

Source lane release is not milestone acceptance. The initial hidden browser probe failed; the computer lane later measured a scoped native screencast capture with the owner and child hidden (4,480 bytes, 640 x 480, 176 ms). Physical paired-device and encrypted end-to-end browser acceptance remain open. No feature release, commit or push has occurred.

## Integration checkpoint

- Latest broad source suite: **230 files passed; 2,173 tests passed and one expected failure**, with no unexpected failure or unhandled error. Two subsequently added helper-deletion continuation cases are covered by a **46/46** affected rerun; these counts overlap and must not be added.
- Settings source and diagnostics bridges are integrated: 66 focused tests, scoped lint and root/UI type checks passed. Diagnostic metadata uses cached provider versions and a bounded database check; it does not run models or read account credentials.
- Android source registration and cancellation are integrated. An isolated native-only fixture compiled actual Capacitor 8 and Android SDK 36 sources and passed 31 JVM tests. The shared checkout still lacks generated Cordova prerequisites until normal CI sync; no local APK, UI export or release build ran.
- Common RPC schema, attachment/project exports, append-only migrations, daemon services/recovery/shutdown, normal file publication and project navigation are wired. RPC version is 2; secure transport preface/confirmation remains independently versioned at 1.
- A running-daemon synthetic test verifies generated bytes through metadata/open/read/close RPC and cached diagnostic reads. File sharing publishes its durable ID/mapping before broadcasting the card; exchange copies retain that ID.
- The first broad unit run passed 2,134 tests with three failures. Two obsolete local-only Computer expectations were corrected. The Git integration fixture now uses its existing 30-second allowance and awaits service shutdown before closing SQLite; the focused rerun passed 15 tests with no unhandled error. This is not a final integrated-suite pass.
- Independent read-only review findings are fixed and source-rechecked: task helper continuations retain exact context through delayed steering and helper deletion; new room events arrive after commit; active project rooms cannot be deleted; cleanup failure cannot hide committed deletions; shutdown closes request admission before waiting. The task continuation/Runner/Team affected selection passed 176 tests before the additional helper-deletion cases were added.
- Desktop file/speech IPC, frame-scoped streaming/native Save As, microphone permission and cancellation are wired. The builder passed 79 tests across 15 files. Failed audio/model/partial-save cleanup cannot become a false successful clear merely because its operation handle was released. Reviewers remain read-only.
- The pinned CPU-only speech runtime recipe, resource manifests/notices, package verification and packaged-smoke assertions are prepared for GitHub Actions. The PowerShell recipe was syntax-parsed only; actual compilation, runtime output and packaged execution were not run here.
- About reads bundled third-party notices in an in-app dialog. Android's static route map includes Projects; the focused attachment/routes JVM check passed 9 tests after applying authoritative native sources with the Firebase-file override removed from that child process.

Open acceptance blockers: enforced provider reviewer runtime, trusted native Logins entry/fill and Full auto containment, physical Android microphone/media/save acceptance, packaged desktop speech/runtime/PDF acceptance, large-file/native resource measurements, and explicit Markdown companion associations. These are not represented as working production features or waived by passing unit tests.
