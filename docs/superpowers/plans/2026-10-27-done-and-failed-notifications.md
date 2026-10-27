# Plan: "Done" and "Failed" notifications (desktop and phone)

**On approval:** the planner writes this plan to `docs/superpowers/plans/2026-10-27-done-and-failed-notifications.md` in the Phase 3 integration worktree `C:/Users/tyrel/.paseo/worktrees/2cb7zj88/tjommie-phase3-remote` (branch `phase3-remote-20261025` @ `4624db1`), then stops. Implementation runs through Paseo agents. All git and all Cloudflare work go to the ops agent.

## Context

The owner tested 0.3.0 on 2026-10-27 and got notifications only for approvals. They want to be told when:
- a bot is **done**;
- a bot **needs approval** (this already exists);
- a bot **failed** at something.

Today `notify` has exactly three kinds (`protocol/src/wire.ts:105-109`): `approval`, `limit` and `routine_failed`. Spec §7 (line 352) limits notifications to those cases, so this request extends the spec.

There is one existing hook every turn passes through, `turnEnded(item, result)` at `daemon/src/daemon.ts:239-245`. The deployed relay **closes the PC's whole control socket** when it gets an unknown push kind (`relay/src/room.ts:147-148`, close 1008). So the relay must change and be redeployed first.

## Decisions

**Owner request:** add `done` and `failed`, on both the desktop and the phone.

**Planner's calls:**
1. **"Done" means the whole piece of work is finished, not each turn.**
   - It fires when a chain you started ends: your message, a room post or a routine.
   - The check runs after `team.turnEnded`: no `work_items` on that `chain_id` is `queued` or `running`, and no `exchanges` row on that chain is still waiting.
   - So a bot that asks another bot gives one "done", when the real answer lands, not three. A room post that mentions several bots gives one "done" for the room.
2. **"Failed" fires right away** when a turn ends with outcome `failed` and its source is `you`, `room` or `bot_reply`.
   - `bot_request` failures are skipped, because the asking bot is told and carries on.
   - `interrupted` doesn't count (Stop, limits and the cap have their own handling).
   - Routine turns are skipped here, because the scheduler handles them (next item).
3. **Routines:**
   - A run that ends `done` sends `done`.
   - A run `stopped` at the time limit now sends `routine_failed` with "Stopped at the time limit." (today it is silent).
   - A user Stop stays silent.
4. **No bot content in a push.** The relay's fixed bodies:
   - `done` → "A bot finished";
   - `failed` → "A bot ran into a problem".

   Desktop toasts are local, so they may show detail, as approvals already do:
   - **done:** title `<Bot> is done`, body the first 140 characters of the reply. For a room: title `Done in <Room>`.
   - **failed:** title `<Bot> ran into a problem`, body the error text.
5. **The phone gets three switches, all on by default:**
   - "A bot needs you" covers `approval` and `limit`;
   - "A bot is done" covers `done`;
   - "Something failed" covers `failed` and `routine_failed`.

   They are stored on the PC per device, so the PC filters before pushing. There are no desktop toggles; a desktop toast is already hidden while the window is focused. Add toggles if "done" toasts turn out noisy.
6. **Approval pushes are exempt from the 30-per-hour budget.** They keep the 30 s coalescing. Otherwise "done" pushes could use up the budget and block approvals.
7. **Safe with old relays.** The relay's `ready` advertises the kinds it supports. The PC sends `done`/`failed` only when they are advertised, so an old or self-hosted relay never drops the connection. The relay also changes to answer an unknown kind with `ok:false` instead of closing.
8. **Room routing.** Room notifications carry `roomId`. Tapping a toast or push opens `/room/?id=<roomId>`; otherwise it opens `/chat/?bot=<botId>`.

## Contracts (N1)

```ts
// protocol/src/wire.ts notify union: add
Notification.extend({ kind: z.literal("done"), roomId: z.string().max(64).optional() }),
Notification.extend({ kind: z.literal("failed"), roomId: z.string().max(64).optional() }),
// protocol/src/relay.ts
export const PushKind = z.enum(["approval", "limit", "routine_failed", "done", "failed"]);
export const LEGACY_PUSH_KINDS = ["approval", "limit", "routine_failed"] as const;   // assumed when ready.kinds is absent
RelayToHost ready: z.object({ t: z.literal("ready"), push: z.boolean(), kinds: z.array(PushKind).max(16).optional() })
HostToRelay push: + roomId: z.string().max(64).optional()
PushData: + roomId: z.string().max(64).optional()      // still .strict()
// protocol/src/remote.ts
"device.registerPush": params { token: …nullable(), kinds: z.array(PushKind).max(16).optional() }   // absent = all kinds
```

## Tasks

| Task | Agent | Owns | Reviewer |
|---|---|---|---|
| N1 contracts | Codex gpt-6-sol max | `protocol/src/wire.ts`, `relay.ts`, `remote.ts` and their tests | Opus 5.5 max |
| N2 relay | Codex gpt-6-sol max | `packages/relay/src/{fcm,room}.ts` and tests | Opus 5.5 max |
| N3 daemon + desktop | Codex gpt-6-sol max | new `daemon/src/turn-notices.ts`; `daemon.ts` (turnEnded), `scheduler.ts` (runEnded), `push.ts`, `relay-client.ts` (ready kinds), `remote-host.ts` and `devices-store.ts` (kinds), `db.ts` (migration 8: `ALTER TABLE devices ADD COLUMN push_kinds TEXT`), `desktop/src/main.ts:337-341` (room click route) | Opus 5.5 max |
| N4 UI | Opus 5.5 max (+fast) | `ui/components/devices/this-phone.tsx` (3 switches), `ui/lib/push.ts` (send kinds; parse `roomId`), `ui/lib/connection.ts:148-153` (route rooms) | Codex gpt-6-sol max |
| N5 spec line | gpt-6-luna max | spec §7 line 352 and §9.2.9 wording | Opus 5.5 max |

**Waves:**
1. N1 merges first.
2. N2, N3, N4 and N5 then run in parallel (4 agents, plus reviewers, never more than 7 live).
3. Then the ops agent works through the delivery order below.

### N1: Contracts
- Tests:
  - `wire.test.ts`: `done` and `failed` parse with and without `roomId`; `kind:"other"` still fails.
  - `relay.test.ts`: `PushKind` has 5 kinds; `PushData` accepts `roomId` and still rejects `title`; `ready` parses with `kinds` omitted.
  - `remote.test.ts`: `registerPush` accepts `kinds` omitted, `[]` and `["done"]`.
- Run `npx vitest run packages/protocol` (via the PowerShell tool).

### N2: Relay
- `fcm.ts:9-13` bodies gain `done: "A bot finished"` and `failed: "A bot ran into a problem"`.
- The tag stays `${kind}:${botId}`, so a newer "done" for the same bot replaces the older one in the tray. `roomId` passes through in `data`.
- `room.ts`: `ready` sends `kinds: PushKind.options`. A push whose `kind` isn't known answers `pushed` with every result `{ok:false, unregistered:false}` and **does not close** the socket. Other malformed control messages still close with 1008.
- Tests:
  - `fcm.test.ts`: exact bodies for the 2 new kinds, and `roomId` in `data`.
  - `room.test.ts:311`: rewrite to "unknown push kind answers ok:false and keeps the host online".
  - `room.test.ts`: `ready` includes all 5 kinds.
- Run `npx vitest run packages/relay`, then `relay.real.test.ts`.

### N3: Daemon and desktop
- **`turn-notices.ts`:** `turnNotice(db: Db, item: WorkItem, result: TurnResult): EventData<"notify"> | null`. It applies calls 1–2 and 4, and stays pure apart from the two read queries.
  - Wire it in `daemon.ts` `turnEnded` **after** `team.turnEnded`, and broadcast it when non-null.
  - Bot and room names come from the store.
- **`scheduler.ts` `runEnded`:** add the routine `done` and the time-limit `routine_failed` (call 3). For `done`, the title is `<Bot> finished "<Routine>"`.
- **`push.ts`:**
  - Skip a device whose `push_kinds` excludes the kind.
  - Skip kinds the relay didn't advertise; `RelayClient.kinds()` returns `LEGACY_PUSH_KINDS` until a `ready` carrying `kinds` arrives.
  - Approval is exempt from the 30-per-hour budget.
  - Forward `roomId`.
- **`remote-host.ts`:** `device.registerPush` stores `kinds` as JSON, or null for all. Revoke clears it.
- **`desktop/src/main.ts:337-341`:** a notice with `roomId` opens `/room/?id=`.
- **Tests:**
  - `turn-notices.test.ts`:
    - `you` turn done with nothing left in its chain → `done`;
    - `you` turn that sent a bot request → nothing, then the `bot_reply` ending → one `done`;
    - `bot_request` failing → nothing;
    - a room post with 3 bots → one `done` (with `roomId`) when the last finishes;
    - `you` failed → `failed` with the error as body;
    - `interrupted` → nothing;
    - a routine item → nothing (the scheduler owns it).
  - `scheduler.test.ts`: done → `done` notify; time-limit stop → `routine_failed` with "Stopped at the time limit."; user stop → nothing.
  - `push.test.ts`:
    - per-device kinds filter;
    - approval still sent after 30 "done" pushes in an hour;
    - `done` not sent while the relay advertises only legacy kinds.
  - `daemon.test.ts:237`-style: the push carries only `kind`, `botId` and `roomId`, never the title or body.
  - `db.test.ts`: migration 8 applies.
  - `notification-delivery.test.ts` or a `main` routing test: room click.
- Run `npx vitest run packages/daemon packages/desktop`, then `npm run typecheck`.

### N4: UI
- **`this-phone.tsx`:** under Notifications, three switches with the labels from call 5, shown only when permission is On.
  - Each change calls `device.registerPush {token, kinds}` again.
  - The choice is saved in localStorage `tjommie.mobile.notifyKinds.v1`, so it survives reconnects and is re-sent after `welcome`.
- **`push.ts`:** registration sends `kinds`, and `onPushTap` accepts `roomId`.
- **`connection.ts`:** `routePush` goes to `/room/?id=` when `roomId` is present.
- **Tests:**
  - `push.test.ts`: kinds are sent; a tap with `roomId` routes to the room; an unknown field is still rejected.
  - Component test: turning off "A bot is done" re-registers without `done`.
- Run `npx vitest run packages/ui`, then `npm run lint`.

## Delivery order (ops agent)
1. After N1 and N2 merge, redeploy the relay. Check `/v1/health` and that the PC's relay state stays "Connected".
2. Merge N3, N4 and N5. Run the full gate: typecheck, no-any, lint, `npm test`.
3. Build 0.3.1 (installer and APK). Back up the DB, then install on the PC and the S25 per the install procedure. Never use a test uninstaller on the owner's install.
   - Alternative: publish 0.3.1 as a GitHub Release, which also gives the first real auto-update test. Needs the branch pushed and the owner's OK.

## Verification
- **Automated:** the tests above, plus `relay.real.test.ts` and the full suite.
- **Owner on devices:**
  1. Ask a bot something on the PC, then switch to another window. A "<Bot> is done" toast appears with the start of the reply, and clicking it opens the chat.
  2. With the phone app swiped away, message a bot from the PC. A push saying "A bot finished" arrives, and tapping it opens that chat.
  3. Make a bot ask another bot. You get one "done", when the answer arrives.
  4. Post `@everyone` in a room. You get one "Done in <Room>", and tapping it opens the room.
  5. Force a failure, for example a bot whose project folder was removed. You get "<Bot> ran into a problem" on both the PC and the phone.
  6. A routine run finishes and you get "done". A routine with a 1-minute limit that runs over gives "Stopped at the time limit."
  7. On the phone, turn off "A bot is done". Done pushes stop, and approval pushes still arrive.

## Out of scope
- Desktop notification toggles.
- Per-bot mute.
- Quiet hours.
- Saving done/failed notices across a daemon restart (approvals aren't saved either).
- Turns the CLI starts by itself (`runner.ts:717-722`), which never reach the hook.
