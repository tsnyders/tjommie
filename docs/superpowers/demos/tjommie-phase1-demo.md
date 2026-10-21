# Phase 1 installed-app acceptance

Dates: 2026-10-21 to 2026-10-22 (Africa/Johannesburg). App version: 0.1.0. CLI versions checked during this work: Claude Code 2.1.304 and Codex 0.160.0.

Status: the installed demo checks passed. The owner confirmed the requested manual browser-persistence and visual checks. Tjommie was left closed after verification.

Branch: `phase1-completion-20261021`. The eight commits through `p1de-fix-b-ui` at `741a10c` were integrated over `06e00c5`. Completion fixes are in `3f970a3`, `4e02338` and `56f08c6`; the final installer was built from `56f08c6`.

## Installed demo results

| Requirement | Result | Evidence |
|---|---|---|
| Install and start the packaged app | Pass | Per-user NSIS installation exited 0. Windows registered Tjommie 0.1.0 and created its Start menu shortcut. The mandatory packaged gate ran its real startup/daemon/shutdown test without skips. |
| Create and chat with Claude and Codex bots, including a Suggestion | Pass | Initial installed screen tests created a Claude Suggestion bot. Final installed acceptance created and exercised both providers. |
| Steering changes the active turn | Pass | The marker arrived in the original turn. Read-only work-history observation confirms the steered item never started a separate queued turn and ended before the original item. |
| Stop interrupts work and follow-up still runs | Pass | Immediate-after-approval Stop completed in 477 ms; Stop after the actual sandbox command started completed in 465 ms. Both work items persisted as interrupted, well before natural 60-second expiry. The actual command PID returned ESRCH. Both follow-up messages completed. |
| Always allow persists | Pass | Before approval the harness checked tool and rule are exactly Write, and the input targets only the bot's first.txt with exactly hi. Both files contained the expected bytes; the second write required no new approval. |
| Cross-provider requests, room rows and replies | Pass | Final installed acceptance verified Claude-to-Codex delivery and a room reply. Initial installed screen tests also exercised handoff/exchange views and a Team room reply. |
| Browser tools and a separate profile | Pass | Installed browser tools loaded Example Domain using the bot's own temporary profile. |
| Manually signed-in site stays signed in | Owner-confirmed Pass | On October 22 the owner answered the requested browser-persistence and visual checks: “I did the the test and it works flawlessly”. No credentials or site details were recorded. |
| Bot proposes a routine and approval creates it | Pass | The installed bot made a real add_routine proposal; approval created the routine. The test then disabled that proposal. |
| Routine fires with the window closed | Pass | A routine fired at its actual due time while the window was hidden, completed after both prior Codex Stops, and displayed its run label. |
| Visual acceptance | Owner-confirmed Pass | The owner reported the requested manual checks work flawlessly. This records owner acceptance, not an exhaustive new automated wireframe comparison. |

Final machine-readable evidence: [installed-automated-results.json](installed-automated-results.json). All nine automated checks passed against the installed payload; the harness verified current source inputs and every installed payload file against the package manifest before launch.

[Installed chat screenshot](../../screenshots/phase1-installed-main-chat.png) shows the stopped commands, successful follow-ups and completed routine.

## Validation

- Final full fast suite: 848 tests passed across 68 files.
- TypeScript check, UI lint and production build passed. The syntax-based no-any gate checked 199 first-party TypeScript files.
- Routine crash-safety lane: 115 focused scheduler, routines and queue tests passed.
- Final Stop/recovery lane: 206 focused tests passed. Both live cases with browser MCP under the installed app's embedded Node runtime passed in 86.38 seconds. An additional live run exercised non-zero Windows recovery in 39.89 seconds: ten reported unique PIDs were confirmed gone before Stop returned and queued routine work completed.
- Provider setup: three real tests passed. Both managed accounts were signed in and the Codex sandbox reported ready.
- Earlier source runtime checks: 21 daemon/desktop/screen tests passed, plus six selected steering, Stop, ordering, approval and session tests; seven tests outside that name filter were excluded.
- The initial installed artifact at 3f970a3 passed 14 lifecycle/screen tests. UI and desktop source are unchanged through 56f08c6; final installed backend checks are the nine stricter checks above.
- The mandatory packaged gate failed when no artifact existed, then passed after packaging. The final rebuilt artifact passed its one required real smoke test; the gate checks source and all 569 artifact files before and after execution.
- Independent Claude and Codex reviewers reported findings without editing files. Their scoped re-reviews found no remaining issues after fixes.

## Final artifact

- Source commit: `56f08c6`.
- Installer: `dist-desktop/installer/Tjommie Setup 0.1.0.exe`, 120,392,677 bytes, unsigned.
- Installer SHA-256: `5649FF8A253749D22C87CF98E9D3CBD7BF340C72ADF11BACD698A8952FB7E5C2`.
- Installed path: `%LOCALAPPDATA%/Programs/Tjommie/Tjommie.exe`.
- Installed/unpacked executable SHA-256: `1CE2E7B70CB1E2C022F7737F0B5B12A8E4B0157ABDF605E1E117BE4ECD95A02C`. The manifest separately verifies the daemon, UI and all other payload bytes; the executable hash alone does not establish freshness.
- Provenance input SHA-256: `a048cede175ab61a39463b98a45307bf0e0ebf5d3741e1e9184d250b6f26581b`.

## Retained attempts

| Record | Meaning |
|---|---|
| installed-automated-initial-isolation.json | The first fixture correctly hit the Own folder guard because real account folders were outside its disposable data root. Later fixtures used Codex Always ask. |
| installed-automated-post-stop-failure.json | The original installed post-Stop routine stall, before the Codex fixes. Source investigation also proved the approved command could continue after interrupted completion. |
| installed-automated-approval-timing.json and installed-automated-polling.json | Coordinator harness bugs: advancing before the approval card, then treating false as a finished result through a shared helper. The corrected local poller passed waiting, timeout and error-propagation contract checks. |
| installed-automated-before-strict-assertions.json | All nine checks passed on 4e02338, before reviewers strengthened steering, Stop and exact approval assertions. |
| installed-automated-taskkill-255.json | The strengthened 4e02338 run passed steering but reported Windows taskkill exit 255 during Stop. Its OS output was not retained, so that precise cause cannot be reconstructed. A matching source failure with exit 128 was reproduced and diagnosed before implementing guarded recovery. |

No failed attempt was converted into a pass. The final record comes from a fresh strict run on the rebuilt 56f08c6 installer.

## Limits and remaining follow-ups

- Non-zero taskkill recovery requires a complete, connected English report including the owned root, and every reported PID must return ESRCH within a shared two-second wait. Unknown, truncated, localized, disconnected or live/inaccessible results still fail. This verifies processes taskkill reported, with the same enumeration boundary as ordinary successful taskkill; it does not establish OS job containment or cover escaped/orphaned processes outside that report.
- If termination fails while Codex remains alive, Stop reports an error and holds queued work for a retry. If the parent has already died and verification is incomplete, Stop reports uncertainty and the runner can resume queued work on a new session. A failed Stop followed by an unrelated external CLI crash may classify the held work as failed rather than interrupted.
- Normal persistent-root Codex Own folder execution was not newly demoed by the disposable Always ask fixture. Provider login cancellation races were tested without signing out or switching the person's real accounts.
- Edge fallback discovery still checks only Program Files (x86) (`packages/daemon/src/browser.ts:18`).
- Codex quota parsing still reads only the primary window (`packages/daemon/src/providers/codex-parse.ts:90`).
- Show window can hold the bot browser profile open; the UI does not explain the need to close that window before the bot starts its browser.
- Startup orphan-run failures persist in history, but their early notification is not replayed to a later client.
- Codex token-based cost pricing remains a documented Phase 2 feature.
- Initial npm ci reported 20 dependency audit findings (10 moderate, 10 high). They were not triaged or upgraded during this feature-completion pass.

No push, publication, auto-update setup or merge into main was performed. The original main checkout's unrelated untracked files were left untouched.

Re-run with `node --disable-warning=ExperimentalWarning docs/superpowers/demos/verify-installed.mjs "<absolute installed Tjommie.exe path>"`. The script uses disposable roots and managed provider accounts, validates exact test approvals, observes only its own database read-only, and quits/cleans its owned processes.
