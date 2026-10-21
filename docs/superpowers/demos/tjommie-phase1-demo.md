# Phase 1 installed-app acceptance

Date: 2026-10-21. App version: 0.1.0. CLI versions checked on this PC: Claude Code 2.1.304 and Codex 0.160.0.

Status: in progress. This record distinguishes source tests, packaged smoke verification, and the installed-app demo. A source test passing does not mark an installed-app item as passed.

Working branch: `phase1-completion-20261021`. The coordinator integrated `p1de-fix-b-ui` through `741a10c` on top of `06e00c5`.

## Installed-app demo (Phase 1E Task 16)

| Item | Result | Evidence or remaining action |
|---|---|---|
| Installer installs and opens; Start menu and tray available | Pending | Build and verify the final package, then install it. |
| Create Claude and Codex bots, one from a Suggestion | Pending | Exercise the installed application. |
| Chat with both; steer; Stop; Always allow persists | Pending | Exercise both providers in the installed application. |
| Cross-provider request, rows in both chats, room exchange and reply | Pending | Exercise the installed application. |
| Own Chrome window and a manually signed-in site stays signed in | Pending | Requires a person to sign in through Show window. Never record credentials. |
| Routine fires with window closed and appears in recent runs | Pending | Schedule a real run, close the installed window to tray, then inspect its result. |
| Bot proposes a routine and approval creates it | Pending | Exercise the installed application. |
| Screens match Phase 1 designs | Pending | Existing screenshot evidence is from September 28 source production mode. Final build needs verification. |

## Current completion validation

- Provider setup: 3 real setup tests passed; both Tjommie-owned accounts signed in and the Codex sandbox reported ready.
- Routine crash-safety lane: 114 focused scheduler, routines, and queue tests passed.
- Combined TypeScript check, syntax-based no-any gate, UI lint and static production build passed.
- Initial full fast suite: 770 passed, 2 failed. The failures reported Windows PowerShell module-loading errors in the ACL helper under the daemon startup tests' restricted environment. Investigation and rerun are required.
- Required packaged gate correctly failed with exit code 1 before any executable was built. No packaged pass is claimed yet.

Final test counts, artifact path and hash, review results, installed checks, and follow-ups will be recorded after the corresponding actions complete.
