# P0 developer teams decision record

Date: 2026-11-03. Source baseline: `aaaf09fe6c45cce2fe42bf9488ac3815156f4ad9` on `phase4-workspace-20261103`. Scope: developer-team feasibility and contracts only. The owner accepted the Phase 4 defaults and multi-agent implementation; this record does not authorize production changes before the coordinator releases the next lane.

## Decision and gate

**Conditional go for persistent tasks and safe Git workspaces. Automated reviewer execution is blocked until provider enforcement is verified.** A role prompt, existing `ask` autonomy, a separate worktree, or a passing test of the snapshot gateway is insufficient to call a provider session read-only.

Reuse existing bots, account selection, queue, rooms, approvals, spending limits and Stop. Add project/task metadata and immutable execution context. The minimum reviewer interface is a bounded, daemon-owned commit snapshot with only `list_files`, `read_file` and `read_diff`. No project scripts run for review. All provider-native mutation/execution tools, browser tools and ordinary Tjommie mutation tools must be unavailable. If that policy cannot be verified for the chosen provider/runtime, return `review_isolation_unavailable`; never fall back to a normal bot turn.

For integration, prepare and test an integration branch in its own worktree. Never advance a branch behind a user's checked-out index. A target checked out by the user, whether dirty or clean, blocks target promotion and leaves the verified candidate available. The user can finish/save their work and leave the branch, or choose an unoccupied target. A new managed promotion worktree claims an unoccupied target through Git's normal worktree registration, then fast-forwards to the exact tested candidate. Future integrations can reuse that recorded, clean promotion worktree.

## Evidence collected

Host: Windows, Node `v24.14.1`, Git `2.53.0.windows.2`, Claude Code `2.1.314`, Codex CLI `0.160.0`. No model turns, provider logins, owner app/data, credentials, phone, network deployment or release packaging were used.

Run from the Phase 4 checkout:

```powershell
node --test scripts/phase4-spikes/teams/probe.test.mjs
```

Final run: **7 tests passed, 0 failed, 0 skipped**, `19255.5463 ms`, with an allowlisted subprocess environment. Earlier runs passed 6/6 in `17974.3991 ms`, 6/6 after tighter promotion/attempt checks in `27665.902 ms`, and the seven-test suite in `41452.8646 ms`; the seven-test suite adds concurrent synthetic worker execution and Stop preservation. These are local fixture durations, not performance promises. Both spike files pass `node --check`; the owned files contain no trailing whitespace.

| Test | What was actually established |
| --- | --- |
| Concurrent synthetic developers and isolated tester | Two Node workers write in different Git worktrees based on the same recorded commit. A tester at the first exact commit writes its check output only in its own worktree. The second task and original checkout are unchanged. |
| Stop | Terminating one owned synthetic Node worker retains its in-progress file, branch and base HEAD. This does not establish real provider process-tree Stop behavior. |
| Reviewer gateway | The in-process fixture gateway reads immutable Git objects. Write/edit/patch/shell/Git/message/share/browser requests, extra parameters and traversal are rejected. An uncommitted source replacement does not change returned bytes. Both provider execution policies deliberately return unavailable. |
| Session identity | Context hashes change for a different worktree, attempt, role/phase, project, account, policy or reviewed commit. This tests the proposed key, not the shipping runner. |
| Integration | Exact accepted commits are ancestors of a clean merged candidate; configured combined checks run there. A user-checked-out target is refused. Two accepted independent tasks integrate sequentially into an unoccupied target through its managed promotion worktree. |
| Refusal cases | Stale review/test SHAs, dirty/moved attempt HEAD, target movement before preparation or promotion, merge conflicts and failed combined checks block completion. Conflicting files and candidate work remain available. |
| Persistence | A real on-disk SQLite fixture is closed/reopened. Completed/cancelled rows stay settled, running work becomes interrupted, a missing worktree becomes blocked, and a late acceptance update cannot match a newer attempt revision. |

Dirty-checkout assertions compare original file digests, raw index digest, staged and unstaged binary diffs, porcelain status and HEAD before/after. Fixtures include staged and unstaged edits to the same file plus an untracked file. All mutating Git commands, synthetic commits and process-scoped synthetic identities are confined to newly created `.fixture-*` directories under the spike directory. The fixture runner validates the absolute cleanup target and its marker before removal. No owner/shared branch, index, Git identity, hooks, credentials or remote was modified.

## Provider enforcement: not yet demonstrated

Source facts at the baseline:

- `packages/daemon/src/providers/claude.ts:21` exposes ordinary built-in tools and uses autonomy plus `--allowedTools`. An allow rule is permission preapproval, not a complete tool allowlist.
- `packages/daemon/src/providers/codex.ts:34` maps `ask` to `untrusted` plus `read-only`, which can request escalation. No independent reviewer policy exists.
- `packages/daemon/src/mcp.ts:28` binds tokens only to bot IDs; `tools/list` and `tools/call` currently have no task/role capability binding. The server's ordinary tools can create messages, shares and routines.

The installed Claude help advertises `--tools ""` to disable built-in tools, `--strict-mcp-config`, `--restricted`, `--safe-mode`, `--disable-slash-commands`, `--setting-sources` and `--permission-prompts none`. This is a plausible minimum implementation: fresh isolated scratch cwd, managed provider account, all built-ins disabled, only the snapshot MCP server attached, no browser/team server, no saved allow rules, no resumed developer session, no write approvals. Do not use `--bare` as a silent subscription-account substitute: its help says it changes authentication behavior.

**Help output proves flag availability only.** It does not prove effective tool exposure, policy precedence, hook/plugin suppression, tool-search behavior or denial under a running authenticated model. Codex needs an independently established equivalent: a verified read-only sandbox with escalation denied and all external mutation capabilities gated, or a verified tool-free mode. Current autonomy mapping is insufficient. A non-billable generated-schema/help probe was inspected; the attempted Windows sandbox help invocation stalled and was interrupted without starting an intentional model turn. No sandbox setup or readiness claim is made.

Before enabling automated reviewer dispatch, an owned synthetic provider acceptance fixture must attempt file edits, shell/process execution, repository hooks/settings, ordinary MCP tools and browser mutation; inspect actual effective tool exposure; assert all source/index/ref bytes unchanged; and retain a passing CLI-version/platform/policy receipt. The daemon must derive this receipt locally, not accept a client `verified` flag. Missing/stale/failed receipts block reviewer execution. A real fixture requires separately authorized provider execution; it was outside this P0 lane's no-paid-turn limit.

The gateway has 256 KiB per-file, 2 MiB total source, and 512 KiB diff limits; subprocess capture has a 4 MiB hard cap. Larger inputs fail explicitly. Binary files, symlinks and submodules have no text-review promise; symlinks/submodules are excluded without dereferencing. Do not present excluded files as reviewed. A production report needs a coverage manifest and an incomplete-review state when required content is omitted.

## Tester boundary

The tester gets a separate detached/managed worktree at the exact candidate SHA and a distinct session context. Only host-configured, user-accepted check specifications may run. Each specification contains a command and argument array, a time/output bound and its approval/autonomy requirement; models and remote clients cannot submit arbitrary shell text as a configured check. Record command, arguments, working commit, attempt/revision, exit status, timestamps and bounded output. Test results never count as source review.

The fixture proves separate cwd/output using benign synthetic code. **A worktree and a scrubbed environment are not a security sandbox for hostile project scripts.** Production checks must execute under the project's accepted existing autonomy and a verified platform sandbox where required; otherwise show the real approval/unavailable state. Do not inherit account credentials, daemon capabilities or browser capabilities into a generic test subprocess. Capture source state before/after; changed tracked source cannot receive an exact-commit acceptance. Retain generated output for explicit cleanup rather than merging it.

## Minimal persisted model

Use an append-only migration at the actual next version chosen by the coordinator. The inspected baseline has nine migrations; other Phase 4 lanes may append first. Never reserve/reuse a stale number. Histories survive bot/team archival and must not be deleted through cascading bot removal.

| Record | Required fields and constraints |
| --- | --- |
| `projects` | ID, host ID, registered repository ID/canonical Git common directory, canonical host path, name, room ID, base/target refs, active/archived state, accepted check specs, timestamps. Repository paths are host-only capabilities. |
| `project_members` | Project ID, bot ID, role labels/instructions. Unique active binding per bot across projects. Reject membership/project/provider changes while queued/running work depends on them. Membership does not grant bot messaging permissions. |
| `project_tasks` | ID, project ID, title, instructions, acceptance checklist, developer/reviewer/tester IDs, state, current attempt ID, revision and timestamps. State is separate from integration status. |
| `task_attempts` | ID, task ID, ordinal, base/head SHAs, owned branch/worktree, execution phase, lifecycle state, revision, work-item ID, immutable context hash, active bot lease, timestamps and reason. Unique active lease per bot and writer per worktree. |
| `task_evaluations` | ID, attempt ID/revision, source SHA/tree, kind (`review`/`test`), actor, passed/findings/failed/unavailable state, findings or check ID/command/exit/output reference, coverage manifest and timestamps. Old results remain history. |
| `task_integrations` | ID, attempt/revision, accepted SHA, expected target SHA/ref, candidate SHA, candidate branch/worktree, promotion worktree ownership, lifecycle state, combined-check result IDs, reason and timestamps. |

Task states: `todo`, `running`, `review`, `done`, `blocked`, `cancelled`. Attempt states: `provisioning`, `queued`, `running`, `interrupted`, `completed`, `failed`, `blocked`, `cancelled`; phases: `develop`, `review`, `test`. Integration states: `none`, `preparing`, `checking`, `ready`, `integrated`, `blocked`. `done` means accepted execution; it does not imply a merge/push. Removing a team archives it and stops new dispatch without removing repositories, histories, branches or worktrees.

Indexes/constraints: tasks by `(project_id, state, created_at)`; attempts by `(task_id, ordinal)` with unique ordinal; partial unique active bot/worktree leases; evaluations by `(attempt_id, revision, source_sha, kind)`; integrations by attempt plus expected target; unique active project binding; unique request/idempotency token for starts/integrations. Enqueue and lifecycle updates occur in the same SQLite transaction. Worktree creation is not transactional with SQLite: persist `provisioning` first, create using a UUID-derived path/branch, record verified ownership, then enqueue. Restart reconciles each stage without recreating an occupied branch/path.

## Runner, queue, MCP and file-sharing contract

`WorkItem`/`NewWork` need nullable `taskAttemptId`, `taskPhase`, `taskRevision` and immutable `executionContextKey` fields, with legacy rows representing standalone execution. Keep one active turn per bot and preserve current priorities, account leases, spending limits, chain limits and interruption handling.

Resolve a task's cwd at admission and again before launch from its recorded managed worktree. Do not update `Bot.projectFolder`. Session identity includes bot/provider/account plus project, attempt, phase, canonical cwd, policy/instruction hash and source commit for review/test. Add a context key to persisted session uniqueness or a task-session table; standalone sessions retain their old identity. A session from a different context must close and finish retiring before the next starts. Recaps/approvals/inbox access must be scoped consistently; changing only cwd is unsafe.

Required existing call sites:

- `queue.ts:6`, `:19`, `:100`, `:124`: persist/hydrate task context, exclude cross-context steering, and reconcile interrupted task rows with work items.
- `runner.ts:553`, `:583`, `:587`, `:682`, `:750`: select cwd/policy/session by work context; never resume/steer a different attempt or role; save sessions under the same context; gate reviewer approvals before saved allow rules.
- `runner.ts:33` and daemon startup: pass the active immutable context to MCP server selection and task lifecycle hooks. Preserve account-selection leases and process-exit fences.
- `mcp.ts:28`, `:67`, `:69`: bind short-lived tokens to bot plus session/attempt/revision/role capabilities. Filter tools on listing and enforce the same allowlist on every call; revoke/fence queued and late calls on context close. Listing alone is not enforcement.
- `team.ts:488`: resolve sharing roots from current developer work context plus the bot's inbox/home as already allowed; never accept an arbitrary project path or grant sharing to reviewer sessions.
- `team.ts:635` and exchange enqueueing: record the requesting attempt context for replies. The receiving bot uses its own active attempt, never the sender's cwd. A reply for an old attempt may be retained in history but cannot steer a newer context.
- Bot/project mutation and deletion handlers: reject busy binding changes; preserve ordinary standalone bots/rooms and existing `canMessage` checks; team archival must not reuse destructive bot deletion.

Restart marks genuinely abandoned running jobs interrupted, preserves completed work, checks branch/common-directory/path ownership and reports missing/moved worktrees as blocked. It must verify owned process exit before releasing a live lease; the SQLite spike does not prove provider process reconciliation. Stop/cancel never removes a branch or worktree. New code/dirty state increments the attempt revision and invalidates both acceptances; late results use revision/SHA compare-and-set and cannot make newer code done.

## RPC and event contract

Implement strict schemas in a dedicated protocol module and compose them into the existing method/event maps. Reuse existing error codes plus a structured reason; do not promise mixed-version peers without negotiation. Keep results bounded and never expose secrets through check output/errors.

| Method | Minimal request/result |
| --- | --- |
| `projects.repositories` | Empty request; list host-registered IDs and display names. |
| `projects.registerRepository` | Local-host-only native folder grant/reference; validate an existing Git repo, canonical identity and refs. Remote calls cannot submit arbitrary host paths. |
| `projects.list`, `projects.get` | IDs and bounded project/task summaries. |
| `projects.create` | Registered repository ID, name, base/target refs, participating existing bots/roles and room link/create choice. Returns project; starts no work. |
| `projects.update`, `projects.archive` | ID/revision plus allowed metadata/roles/check changes; reject active-context conflicts. Archive retains work. |
| `tasks.create`, `tasks.update` | Project/task ID, expected revision, title/instructions/checklist and assignments. Returns task; starts no paid work. |
| `tasks.start` | Task IDs, expected revisions, idempotency token. Returns persisted queued/provisioning attempts; user/batch action only. |
| `tasks.get`, `tasks.list` | Task/project ID; task detail/history and paginated attempts/evaluations/integrations. |
| `tasks.stop`, `tasks.cancel`, `tasks.resume` | Task/attempt ID and expected revision; preserve work. Resume is explicit after reconciliation. |
| `tasks.diff` | Attempt/revision/exact SHA and bounded page/range; returns commit identity, coverage and bounded diff without a filesystem path input. |
| `tasks.evaluate` | Attempt/revision/exact SHA and review/test kind. Resolve assigned actor and host check spec; never accept arbitrary command, cwd, findings or a forged acceptance from a renderer. |
| `tasks.integrate` | Attempt/revision, exact accepted SHA, expected target SHA, idempotency token. Return the persistent integration record immediately; report later progress. |
| `tasks.cleanupPreview`, `tasks.cleanup` | Attempt/integration ID; preview owned dirty/untracked files, then explicit cleanup token/confirmation. No implicit force removal. Cleanup is separate from Stop/archive. |

Events: `project.updated`, `task.updated`, `task.attemptUpdated`, `task.evaluationUpdated`, `task.integrationUpdated`; existing room/chat events carry task links. Reconnect requests authoritative current state; event delivery is not the durable task store. Lead proposals are normal task drafts and require the user's `tasks.start` action.

## Exact-commit integration sequence

1. Lock the project target and attempt inside the daemon; verify no active writer/process, expected task revision, actual repository/common directory, ownership receipt, clean attempt and exact current HEAD. Verify passing review/test cover that SHA/revision with complete required coverage.
2. Resolve/validate full refs through Git argument arrays. Validate exact object IDs; never pass client-supplied rev expressions, paths or shell text. Confirm current target equals the user's expected SHA.
3. Persist integration preparation. Create a unique owned candidate branch/worktree from that exact target; never overwrite an existing ref/path. Merge the exact accepted commit without rebasing or cherry-picking away its identity. Conflicts stay blocked in the managed candidate with their paths visible.
4. Run all required combined checks under the tester boundary, record their exact candidate tree/commit and retain failures. Source changes or failed checks block readiness. A fixed conflict/new candidate requires fresh checks and source review for any new resolution.
5. At promotion, recheck expected target, ownership and clean candidate. Refuse targets checked out in user/unowned worktrees. Claim an unoccupied target using a managed target worktree; normal Git branch occupancy prevents a second normal checkout. Recheck its HEAD and cleanliness, then fast-forward only to the already tested candidate. Keep/persist the promotion ownership receipt for reuse/restart.
6. Record actual final target SHA and reconcile interrupted promotions on restart before retrying. No push, PR, force, rebase, stash, checkout switch in the user's worktree, or implicit branch/worktree deletion.

The fixture intentionally supplies a synthetic identity and empty hooks/config within its disposable repository. Production must use the project's existing identity and must not copy those synthetic overrides. Choose hook-free Git plumbing or explicitly accepted hook execution without editing repository/global hooks/config; ordinary merge/commit hooks can execute project code and need the same trust treatment as checks. The fixture does not validate hostile hooks, signing requirements, junction attacks, non-UTF-8 filenames, cross-volume storage, disk-full/crash windows, or a concurrent external Git process using force/ignore-worktree safeguards. Git's ordinary locks/occupancy and daemon locks do not protect against deliberate external ref/file tampering; drift must fail closed and preserve evidence.

## Proposed complete production allowlist

This is proposed reach, not an editing grant. Split exact files after the coordinator approves contracts. No package/lock change is required for the core team feature: Node Git subprocesses, SQLite, installed Zod and existing UI primitives are sufficient.

| Owner/lane | Paths |
| --- | --- |
| Team implementation | New `packages/protocol/src/projects.ts`, `projects.test.ts`; new `packages/daemon/src/projects.ts`, `project-git.ts`, `project-review.ts`, `project-checks.ts` and focused sibling tests; new `packages/ui/lib/projects.ts` and focused test; new `packages/ui/components/projects/project-list.tsx`, `project-editor.tsx`, `task-list.tsx`, `task-detail.tsx`, `task-diff.tsx` and focused tests; new `packages/ui/app/projects/page.tsx`. |
| Coordinator/shared protocol and persistence | `packages/protocol/src/index.ts`, `wire.ts` and contract fixtures/tests; `packages/daemon/src/db.ts`, `database-upgrade.test.ts`; no edits to shipped migration text. If session context is added to the shared sessions table, update its schema tests here. |
| Coordinator/shared orchestration | `packages/daemon/src/queue.ts`, `queue.test.ts`, `runner.ts`, `runner.test.ts`, `main.ts`, `team.ts` plus its routing/path tests, `mcp.ts`, `mcp.test.ts`, `bots.ts` plus its tests. Assign individual file ownership before changes. |
| Provider enforcement lane | `packages/daemon/src/providers/types.ts`, `claude.ts`, `claude.test.ts`, `codex.ts`, `codex.test.ts`; dedicated synthetic real-provider reviewer acceptance fixture, gated and never in the default paid test suite. |
| UI/shared host integration | `packages/ui/components/rail.tsx`, `tab-bar.tsx`, `tab-bar.test.tsx`; `packages/ui/lib/phone-nav.ts`, `phone-nav.test.ts`, `store.ts`, `connection.ts` and its focused reconnect tests only where project entry/events require them; `packages/client/src/client.ts` only if a helper beyond its generic RPC call is necessary. Reuse existing `localFileActions().pickFolder()` without a native implementation change. Repository registration accepts its path only on the local host transport; remote clients use registered IDs. |
| Coordinator documentation | `CHANGELOG.md`, shared Phase 4 plan/status/indexes and migration/protocol compatibility evidence. |

At the end of the P0 wave, its actual allowlist was only this record and `scripts/phase4-spikes/teams/**`; production code, dependencies and lockfiles were unchanged. The coordinator subsequently released the source implementation below. Neither wave establishes provider/native end-to-end or physical-device acceptance.

## P3/P4 source handoff

The coordinator released this lane for source implementation after reviewing P0. New protocol schemas, project/task services, Git workspaces/integration, reviewer snapshot/check boundaries, a project/task UI, and queue/runner context fencing are now present. No package/lock, shared wire/index, production migration registration, daemon startup, Team, bot handlers, navigation, installed app or release artifact was changed by this lane.

### Owned implementation

- `packages/protocol/src/projects.ts`: strict project/member/check/task/attempt/evaluation/integration schemas, methods and events. Create defaults do not leak into patch schemas. Exact Git object IDs and user revision/ref fences are required.
- `packages/daemon/src/project-schema.ts`: `PROJECT_MIGRATION`; the exact proposed SQL is also in `teams-migration.sql`. It includes repository-scoped target locks, shared developer/evaluator leases, work-item/session context, and three origin-context columns on exchanges for the coordinator's Team integration. Tests self-apply this exact proposed migration to disposable DBs when the shared migration registry has not yet integrated it.
- `projects.ts`, `tasks.ts`, `project-handlers.ts`: registered host repositories, a normal linked room, editable roles, drafts, explicit Start/Resume, persistence, revision/SHA/project-configuration acceptance, lifecycle events, Stop/cancel and clean-only task worktree cleanup. Registering a host path requires a live local transport context. Draft creation dispatches no provider work.
- `worktrees.ts`, `task-integration.ts`: UUID-owned branches/paths, argument-array Git, read-only original checkout, hook/filter-free Git-object materialization, no replacement-object indirection, conflict retention, combined checks, target occupancy/CAS guards and managed promotion. Legacy grafts and unsupported tree entries/paths fail closed. Directory-to-file refactors preserve exact object bytes. Cleanup lists all untracked files, including ignored output, and refuses dirty or filtered checkouts; it retains branches/history.
- `project-review.ts`, `project-checks.ts`: immutable, bounded commit text snapshots; no production reviewer adapter is enabled. The candidate Claude restriction flags are recorded without claiming enforcement. Explicit configured host checks use a scrubbed environment, argument arrays, bounded output, cancellation/timeouts and owned-process termination handling. Default `ask`/`folder` testers return `check_isolation_unavailable`; direct host execution is available only after explicit Evaluate/Integrate with a Full auto tester, or a trusted isolated executor supplied by the host.
- `task-execution.ts`, `queue.ts`, `runner.ts`: immutable task context survives persistence; no cross-context steering/resume, cost baselines are scoped to their saved context, and the prior owned process must retire before the next task launches. Per-turn bot prompt copies use the actual worktree without changing stored `Bot.projectFolder`. Project room/bot-request discussion turns use the bot home, so they do not inherit an old user-repository cwd. Standalone bots retain normal behavior. `isHeld` now gates pump admission and approval answers; provider-native automatic-tool Pause enforcement is still a separate unproved boundary.
- `packages/ui/app/projects/page.tsx`, `components/projects/**`, `lib/projects.ts`: project selection/editor, registered-host repository picker, roles/check configuration, task drafts/detail, committed diff, evaluation history, explicit Start/Resume/review/check/integration actions and clean-worktree cleanup preview. The page subscribes to project/task events itself and resets on host change; global project state reducers are unnecessary. Stop remains usable during a pending evaluation. Code/diff content is rendered as text. House dialogs, fields and buttons are reused.

### Verification receipt

Across focused batches, **168 unique source tests passed**: runner 126, queue 11, projects 4, tasks 9, worktrees 7, integrations 4, check runtime 2, handler context 1, protocol 2, UI helpers/rendering 2. The runner/queue fixtures use the proposed migration when the shared registry lacks it. This is not proof that the shipped migration registry is wired. Final context/Stop/local-registration regression selection passed 7/7 (129 unrelated cases skipped by that selection); earlier complete focused suites cover those skipped cases.

The last complete integration/tasks/handler batch passed 13/13 in `59.21 s`; the object-safety/check/protocol/UI batch passed 13/13 in `26.05 s`; the runner suite passed 126/126 in `38.21 s`; queue passed 11/11. These are local test durations. Earlier failures exposed a patch-default reset and a missing additive queue expectation; those were fixed. A two-task integration fixture needed a 60-second test allowance on this Windows host; no performance guarantee is inferred.

Owned UI lint and whitespace checks pass. The no-`any` AST scan passes across **527 TypeScript files** in the shared checkout. Final root typecheck reports **zero teams-owned diagnostics and 37 diagnostics outside this lane**; earlier output identifies uncomposed attachment error codes and computer-spike type narrowing among those blockers. Actual UI typecheck also awaits shared protocol method/event composition (plus the attachment bridge). A read-only TypeScript compiler-host overlay of only the proposed project wire/index composition produced **zero teams-owned UI diagnostics**, with eight unrelated diagnostics; no shared source file was edited for that check. The proposed SQL copy exactly matches `PROJECT_MIGRATION`, and shared Git HEAD remains the baseline SHA.

The local installed Next guides for `use client` and static export were read before the UI implementation. The source uses the existing client-page pattern; it does not add a Next server route or require a hosted runtime. No local app build, installed-app launch, screenshots, browser/device QA, real provider/model execution, sandbox setup, owner data/credentials, commit/push or release was performed.

### Required coordinator integration

1. Export the project module from the protocol index and compose `projectMethods`/`projectEvents` into wire maps. Coordinate the strict protocol version/peer update; old clients must not silently accept unknown new events under the old handshake.
2. Append `PROJECT_MIGRATION` as the actual next migration; do not copy it over a shipped migration. Update migration/upgrade fixtures. The SQL copy is proposed SQL only.
3. Instantiate `Worktrees(join(paths.root, "workspaces"))`, `Projects(db, worktrees, broadcast)`, `Tasks(projects, runner)` and `TaskIntegrations(tasks)`, then spread `projectHandlers(projects, tasks, integrations)` into existing handlers. Do not supply a reviewer runtime without an effective-policy acceptance receipt.
4. Compose runner hooks: `executionContext(bot,item) => tasks.executionContext(bot,item)`, `turnStarted => tasks.turnStarted`, `turnEnded => tasks.turnEnded`, `turnRequeued => tasks.turnRequeued`. Existing hooks remain composed, not replaced. `mcpServers` now also receives the work item. Use `runner.effectiveFolder(botId)` for task/discussion file-sharing roots. Do not infer the folder from stored `Bot.projectFolder`.
5. Persist a task request's origin triple in the new exchange columns and copy `workContext(origin)` only when delivering a still-valid reply to the requester. Late/stale replies remain history; they cannot resume/steer a newer attempt. The recipient uses its own context, never the sender's cwd. Preserve existing bot links, chain limits, queues and attachment snapshot lifecycle. Bind/revoke MCP tokens and enforce capabilities on both listing and calling as described earlier.
6. Reject bot project/provider/account/folder changes while task/check leases or queued/running dependent work exist. A bot still bound to an active project must be removed from that team before deletion; histories must survive. Use `projects.botInUse`, `tasks.botBusy` and normal queue checks as appropriate.
7. Construct services before recovery hooks execute. Reconcile actual owned process exit, run `runner.recover()`, `tasks.recover()`, `integrations.recover()`, then `runner.start()`. Unknown crash-era leases remain held. `tasks.confirmedExited(botId,ownerId)` is a trusted-host seam after actual process proof, not a remote override. Do not auto-release an unknown live process merely to make Resume work.
8. Stop new requests at shutdown, then await `tasks.shutdown()`, `integrations.shutdown()` and existing runner shutdown before closing SQLite. Unknown termination rejects shutdown and retains state. Add the Projects navigation entry and task/room links; the new page needs no global project store.
9. Run integrated migration/contract/daemon/UI checks and independent review. Source services expose all records needed for the owner demo, but real provider review, isolated execution on default autonomy, crash/PID reconciliation, visual/native behavior and the complete accepted team workflow remain open.

The source reviewer remains unavailable by default, and its current text snapshot fails closed on binary/large/unsupported repository content. Full auto host checks are host code execution, not an OS sandbox. Tester/integration worktrees remain visible in their results and are retained; this source UI's cleanup action handles primary task worktrees only. No source fixture result is an accepted native/provider demonstration.
