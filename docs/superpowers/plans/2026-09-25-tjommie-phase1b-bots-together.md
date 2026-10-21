# Tjommie Phase 1B: Bots Working Together Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bots can ask each other for help (Claude to Codex and back), talk in the Team room when @mentioned, share files, run on routines, and browse the web in their own Chrome profile, all on top of the Phase 1A daemon.

**Architecture:**
- **App MCP server.** The daemon gets its own MCP endpoint at `/mcp` on its existing local HTTP server. It's a small, stateless JSON-RPC handler, with no SDK. Every CLI session gets a bearer token that says which bot is calling.
- **Tools.** Through it, bots call `list_bots`, `message_bot`, `share_file` and `add_routine`.
- **Turn destinations.** Each turn writes into its own conversation: the bot's chat, the Team room, or an exchange. When a turn ends, hooks turn its last message into a reply to the bot that asked.
- **Routines.** A 30-second scheduler turns routines into queued work.
- **Browser.** Playwright MCP gives each bot a real Chrome window with its own profile.

**Tech Stack:** as in plan 1A, plus `@playwright/mcp` 0.0.82. zod 4's built-in `z.toJSONSchema` provides the tool schemas.

**Spec:** `docs/superpowers/specs/2026-09-25-tjommie-design.md` §4.3 and §4.4 (queue and where things show), §5.6 (the app's MCP server), §5.7 (browser), §6 (bots talking to each other), §7 (routines).

**Builds on:** `docs/superpowers/plans/2026-09-25-tjommie-phase1a-daemon-core.md`. That plan must be finished first. This plan changes some of its files, and each step says exactly how.

## Where this plan sits

| Plan | Delivers | Needs |
|---|---|---|
| 1A Daemon core | Bots, chat, both CLIs, steering, Stop, approvals, sign-in, usage records | nothing |
| **1B Bots working together (this plan)** | App MCP server, bot-to-bot exchanges with a chain limit, Team room, file sharing, routines and scheduler, browser per bot | 1A |
| 1C Screen designs | OpenPencil frames for the Phase 1 screens | nothing |
| 1D Desktop app and UI | Electron shell, all Phase 1 screens, installer, Phase 1 demo | 1A, 1B, 1C |

## Spike findings this plan relies on

All of these were measured on this PC on 2026-09-25 against Claude Code 2.1.280, Codex CLI 0.155.1 and `@playwright/mcp` 0.0.82.

**The MCP endpoint**
- **No SDK needed.** A hand-rolled, stateless MCP endpoint on plain `node:http` works with both CLIs. It answers POSTed JSON-RPC with plain JSON (`content-type: application/json`), 202 for notifications, and 405 for GET.
- **Handshake.**
  - The only methods needed are `initialize`, `tools/list`, `tools/call` and `ping`.
  - Answer `initialize` by echoing the client's `protocolVersion`: Claude sends `2025-11-25`, Codex `2025-06-18`.
  - Claude first probes a newer `server/discover` method. Answering it with error -32601 (method not found) makes Claude fall back to `initialize`.
  - Some of Claude's messages omit the `jsonrpc` field, so the handler mustn't require it.

**Claude**
- **Attaching servers:** `--mcp-config '{"mcpServers":{"tjommie":{"type":"http","url":"http://127.0.0.1:<port>/mcp","headers":{"Authorization":"Bearer <token>"}}}}'`. The header arrives on every request.
- **Permissions:** tools from an attached server need `--allowedTools mcp__<server>`, otherwise every call asks for permission. `mcp__tjommie` allows every tool of that server.
- **Stdio servers** (the browser): `{"type":"stdio","command":…,"args":[…],"env":{…}}`.

**Codex**
- **Attaching servers:** thread config `mcp_servers.<name> = { url, http_headers: { Authorization: "Bearer <token>" } }` for HTTP servers, and `{ command, args, env }` for stdio servers.
- **Tool approvals:** Codex asks before EVERY MCP tool call with a server request, `mcpServer/elicitation/request`. Its `params` are `{ serverName, mode: "form", message: "Allow the tjommie MCP server to run tool \"ping\"?", requestedSchema: {}, _meta: { codex_approval_kind: "mcp_tool_call", tool_params, persist: ["session","always"] } }`.
- **Answering:** `{ action: "accept" }` lets the call run; `{ action: "decline" }` rejects it. An answer in any other shape counts as a rejection.
- **Caller identity:** each `tools/call` carries `_meta["x-codex-turn-metadata"]` with the thread and turn ids. Tjommie identifies the caller by the bearer token anyway.

**Browser: Playwright MCP 0.0.82 (spike 5)**
- **Package:** it exports `./package.json`, and its CLI is `cli.js` next to it.
- **Options used:** `--user-data-dir <dir>`, `--output-dir <dir>`, `--browser chrome|msedge` and `--headless`. It runs headed unless told otherwise.
- **Claude:** the stdio entry was passed in `--mcp-config` as one argument through Node's `spawn`, with `--allowedTools mcp__browser`. Claude opened https://example.com in a fresh profile folder and read its title, with no approval prompts. It loaded the browser tools through its ToolSearch tool first.
- **Codex:** `mcp_servers.browser = { command, args }` did the same. Every browser call arrived as an elicitation for server `browser`. Chrome reached the network from a `workspace-write` thread under the unelevated sandbox.
- **Profile:** both runs left a Chrome profile (`Default\`) in the given folder, so sign-ins persist.
- **Decision:** Playwright MCP starts the browser. In Phase 1 the daemon doesn't run Chrome itself with a debugging port. "Show window" opens the bot's profile in the browser (Task 7). Phase 4's live view revisits this.
- **Browsers on this PC:** Chrome is at `C:\Program Files\Google\Chrome\Application\chrome.exe`; Edge (`C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`) is always there on Windows.

**Codex's own extras**
- **Default on:** `codex features list` shows `apps`, `browser_use`, `browser_use_external`, `computer_use`, `plugins`, `remote_plugin` and `memories` switched on.
- **What went wrong:** with the person's own Codex setup, a browsing request went to Codex's built-in browser use instead of the attached Playwright server. That drove the person's real Chrome.
- **The fix:** a thread `config.features` that turns them off (Task 7's `CODEX_FEATURES_OFF`). With it, only the `browser` server started, and the same request used Playwright.

## Global Constraints

Every Global Constraint in plan 1A still applies: TypeScript through Node type stripping, npm from the repo root in PowerShell, local-only security, provider rules, account folders, autonomy mapping, visible-text rules, commit style and the test commands. In addition:

- **Dependencies:** this plan adds exactly one, `@playwright/mcp@^0.0.82` (daemon). The MCP endpoint is hand-rolled; don't add `@modelcontextprotocol/sdk`.
- **The `/mcp` endpoint:**
  - It accepts only POST, only with a valid session bearer token, only with this machine's Host header, and only without an Origin header, because browsers always send one and the CLIs never do.
  - A token is issued when a bot's CLI session starts and revoked when it closes.
  - Request bodies are capped at 1 MB.
- **Tool permissions:**
  - Tjommie's own tools (`tjommie` server) never ask the person.
  - The browser (`browser` server) doesn't ask either, except for bots set to "Always ask".
  - Claude gets this through `--allowedTools mcp__tjommie` (plus `mcp__browser`); Codex through auto-answered elicitations.
- **Codex extras off:** every Codex thread turns off these features:
  - `apps`, `browser_use`, `browser_use_external`, `computer_use` and `in_app_browser`;
  - `plugins`, `remote_plugin`, `tool_suggest` and `skill_mcp_dependency_install`;
  - `memories`.

  Bots use Tjommie's tools. They never use the person's own browser, desktop or ChatGPT apps.
- **Browser:** each bot with the browser on gets Playwright MCP, started by its CLI, on `bots\<id>\chrome-profile`. It uses Chrome if it's installed, otherwise Edge.
- **Queue priorities:**
  - 0 (first): you in the bot's chat; you in the Team room; a reply to the bot's own request.
  - 1 (wait): a request from another bot; a routine run.
- **Steering:** only your 1:1 messages and replies to a bot's own requests are steered into a running turn, and only when they belong to the conversation that turn writes to.
- **Chain limit:** a chain is one 1:1 message from you, one Team room post, or one routine run, plus all the work it causes. Each `message_bot` call counts one. The limit is setting `chainLimit`, default 5.
- **Routines:**
  - Days are 0 (Sunday) to 6. Times are 24-hour `HH:MM` in the routine's IANA time zone, which defaults to the host's.
  - The scheduler ticks every 30 s. A run more than 2 minutes late is recorded as Missed and doesn't fire. A paused bot's run is Skipped.
  - Each routine keeps its last 20 runs.
- **The Team room's** conversation id is `"team"` (`TEAM_ROOM_ID`). Room answers go to the room, with a one-line "Answered in the Team room" row in the bot's own chat.
- **Real-CLI tests that use the browser** set `TJOMMIE_BROWSER_HEADLESS=1`, so no windows pop up during tests. The product runs the browser headed.

## Review Focus

These are the five things most likely to go wrong for a person using bots together, most likely first. Each has a test.

1. **Two bots asking each other in a loop.** The chain limit must stop it, and the bot is told to ask the person. *Test: Task 9, "chain limit".*
2. **A bot is deleted while its request or reply is in flight.** Finishing the exchange must not crash the daemon or leave the other bot waiting forever. *Test: Task 9, "requester deleted".*
3. **The PC was off or asleep through several routine times.** Exactly one Missed run is recorded and nothing fires in a burst. *Test: Task 6, "late tick".*
4. **`share_file` with a path outside the bot's folders** (`..\..\Windows\win.ini`, or another drive) must be refused. *Test: Task 9, "share outside folder".*
5. **The `/mcp` endpoint called by something that isn't the bot's CLI** (a browser page, a stale token after the session closed) must be refused. *Tests: Task 4, "refuses browsers and bad tokens", and Task 8, "token revoked on close".*

---

### Task 1: Protocol additions

**Files:**
- Modify: `packages/protocol/src/model.ts` (replace the file), `packages/protocol/src/wire.ts` (replace the file)
- Test: `packages/protocol/src/wire.test.ts` (add tests)

**Interfaces:**
- Consumes: plan 1A's protocol.
- Produces (new or changed exports):
  - `TEAM_ROOM_ID = "team"`.
  - `MessageKind` adds `"handoff"`, `"routine"` and `"file"`.
  - `Approval` gains `kind: "tool" | "routine"` and `payload: Record<string, unknown> | null`.
  - `RoutineInput`, `RoutinePatch`, `Routine`, `RoutineRunStatus`, `RoutineRun`, `Exchange` and `Settings` (schemas and types).
  - New methods: `bots.showBrowser`, `room.history`, `room.post`, `exchanges.get`, `routines.list`, `routines.create`, `routines.update`, `routines.delete`, `routines.runs`, `settings.get` and `settings.set`.
  - The `data` shapes of the new message kinds, in a comment above `MessageKind`. Plan 1D's screens read them.
  - New events: `routine.updated`, `routine.deleted`, `routine.run` and `settings.changed`.

- [ ] **Step 1: Write the failing tests**

In `packages/protocol/src/wire.test.ts`, extend the import from `./index.ts` so it reads:

```ts
import { Approval, BotInput, BotPatch, ClientMessage, PROTOCOL_VERSION, RoutineInput, RoutinePatch, Settings, TEAM_ROOM_ID, methods } from "./index.ts";
```

Then add this block at the end of the file:

```ts
describe("protocol for bots working together", () => {
  it("names the Team room", () => {
    expect(TEAM_ROOM_ID).toBe("team");
  });

  it("accepts a weekday routine and rejects bad days and times", () => {
    const routine = { botId: "b", name: "Morning check", prompt: "Check pnet.co.za", days: [1, 2, 3, 4, 5], times: ["08:48", "12:47"] };
    expect(RoutineInput.parse(routine)).toEqual(routine);
    expect(() => RoutineInput.parse({ ...routine, days: [7] })).toThrow();
    expect(() => RoutineInput.parse({ ...routine, times: ["8:48"] })).toThrow();
    expect(() => RoutineInput.parse({ ...routine, times: ["24:00"] })).toThrow();
  });

  it("leaves unspecified routine fields out of a patch", () => {
    expect(RoutinePatch.parse({ enabled: false })).toEqual({ enabled: false });
  });

  it("describes routine proposals as well as tool approvals", () => {
    const approval = { id: "a", botId: "b", kind: "routine", tool: "Routine", summary: "s", detail: "d", rule: null, payload: { name: "x" }, status: "pending", messageId: "m", createdAt: "t" };
    expect(Approval.parse(approval).kind).toBe("routine");
  });

  it("bounds the chain limit between 1 and 20", () => {
    expect(Settings.parse({ chainLimit: 5 })).toEqual({ chainLimit: 5 });
    expect(() => Settings.parse({ chainLimit: 0 })).toThrow();
    expect(() => Settings.parse({ chainLimit: 21 })).toThrow();
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. `RoutineInput`, `RoutinePatch`, `Settings` and `TEAM_ROOM_ID` are not exported.

- [ ] **Step 3: Replace model.ts**

Replace `packages/protocol/src/model.ts` with:

```ts
import { z } from "zod";

// The conversation id of the Team room, the one room every bot is in.
export const TEAM_ROOM_ID = "team";

export const Provider = z.enum(["claude", "codex"]);
export type Provider = z.infer<typeof Provider>;

// ask: every change needs you. folder: free inside its own folders. full: never asks.
export const Autonomy = z.enum(["ask", "folder", "full"]);
export type Autonomy = z.infer<typeof Autonomy>;

export const BotColor = z.enum(["brown", "red", "orange", "amber", "green", "teal", "blue", "purple", "pink", "gray"]);
export type BotColor = z.infer<typeof BotColor>;

export const BotShape = z.enum(["circle", "blob", "rounded-square", "pill", "triangle", "hexagon", "cloud", "droplet"]);
export type BotShape = z.infer<typeof BotShape>;

export const BotState = z.enum(["idle", "working", "waiting", "paused", "limited", "needs_setup"]);
export type BotState = z.infer<typeof BotState>;

export const Bot = z.object({
  id: z.string(),
  name: z.string(),
  color: BotColor,
  shape: BotShape,
  instructions: z.string(),
  provider: Provider,
  model: z.string(),
  autonomy: Autonomy,
  homeFolder: z.string(),
  projectFolder: z.string().nullable(),
  browser: z.boolean(),
  paused: z.boolean(),
  canMessage: z.array(z.string()),
  state: BotState,
  limitedUntil: z.string().nullable(),
  createdAt: z.string(),
});
export type Bot = z.infer<typeof Bot>;

const botFields = {
  name: z.string().trim().min(1).max(40),
  color: BotColor,
  shape: BotShape,
  instructions: z.string().max(20_000),
  provider: Provider,
  model: z.string().min(1).max(100),
  autonomy: Autonomy,
  projectFolder: z.string().nullable(),
  browser: z.boolean(),
  canMessage: z.array(z.string()),
};
export const BotInput = z.object(botFields).partial({ autonomy: true, projectFolder: true, browser: true, canMessage: true });
export type BotInput = z.output<typeof BotInput>;
export const BotPatch = z.object(botFields).partial();
export type BotPatch = z.output<typeof BotPatch>;

// What `data` holds for the newer kinds:
// - handoff, for an exchange: { exchangeId, fromBotId, toBotId, messages, status: "waiting" | "replied" | "failed" };
//   the body is the request's first line. For an answer given in the Team room: { roomMessageId }.
// - routine: { routineId, runId, status, note }, the "Routine: <name>" label above a run.
// - file: { path, name, size }, a file a bot shared; the body is its caption.
// - text posts in the Team room that belong to an exchange: { exchangeId, toBotId, files?: { name, path }[] }.
export const MessageKind = z.enum(["text", "activity", "approval", "notice", "error", "handoff", "routine", "file"]);
export type MessageKind = z.infer<typeof MessageKind>;

export const MessageStatus = z.enum(["queued", "sent", "streaming", "done", "failed", "interrupted"]);
export type MessageStatus = z.infer<typeof MessageStatus>;

export const Message = z.object({
  id: z.string(),
  conversationId: z.string(),
  author: z.string(), // "you", "system" or a bot id
  kind: MessageKind,
  body: z.string(),
  data: z.record(z.string(), z.unknown()).nullable(),
  status: MessageStatus,
  createdAt: z.string(),
});
export type Message = z.infer<typeof Message>;

export const ApprovalDecision = z.enum(["once", "always", "deny"]);
export type ApprovalDecision = z.infer<typeof ApprovalDecision>;

// tool: a CLI asking to use a tool. routine: a bot proposing a routine (payload holds the RoutineInput).
export const Approval = z.object({
  id: z.string(),
  botId: z.string(),
  kind: z.enum(["tool", "routine"]),
  tool: z.string(),
  summary: z.string(),
  detail: z.string(),
  rule: z.string().nullable(),
  payload: z.record(z.string(), z.unknown()).nullable(),
  status: z.enum(["pending", "allowed", "denied"]),
  messageId: z.string(),
  createdAt: z.string(),
});
export type Approval = z.infer<typeof Approval>;

export const ProviderSetup = z.object({
  provider: Provider,
  installed: z.boolean(),
  version: z.string().nullable(),
  signedIn: z.boolean(),
  account: z.string().nullable(),
  plan: z.string().nullable(),
  error: z.string().nullable(),
});
export type ProviderSetup = z.infer<typeof ProviderSetup>;

export const ModelInfo = z.object({ id: z.string(), label: z.string(), isDefault: z.boolean() });
export type ModelInfo = z.infer<typeof ModelInfo>;

// Days are 0 (Sunday) to 6 (Saturday). Times are 24-hour "HH:MM" wall-clock times in the routine's time zone.
const routineFields = {
  name: z.string().trim().min(1).max(60),
  prompt: z.string().trim().min(1).max(10_000),
  days: z.array(z.number().int().min(0).max(6)).min(1).max(7),
  times: z.array(z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/)).min(1).max(12),
  timeZone: z.string().min(1).max(64),
  timeLimitMinutes: z.number().int().min(1).max(1440).nullable(),
  enabled: z.boolean(),
};
export const RoutineInput = z.object({ botId: z.string(), ...routineFields }).partial({ timeZone: true, timeLimitMinutes: true, enabled: true });
export type RoutineInput = z.output<typeof RoutineInput>;
export const RoutinePatch = z.object(routineFields).partial();
export type RoutinePatch = z.output<typeof RoutinePatch>;
export const Routine = z.object({ id: z.string(), botId: z.string(), ...routineFields, nextRunAt: z.string().nullable(), createdAt: z.string() });
export type Routine = z.infer<typeof Routine>;

export const RoutineRunStatus = z.enum(["queued", "running", "done", "stopped", "failed", "missed", "skipped"]);
export type RoutineRunStatus = z.infer<typeof RoutineRunStatus>;
export const RoutineRun = z.object({
  id: z.string(),
  routineId: z.string(),
  dueAt: z.string(),
  startedAt: z.string().nullable(),
  endedAt: z.string().nullable(),
  status: RoutineRunStatus,
  note: z.string().nullable(),
});
export type RoutineRun = z.infer<typeof RoutineRun>;

export const Exchange = z.object({
  id: z.string(),
  fromBotId: z.string(),
  toBotId: z.string(),
  request: z.string(),
  reply: z.string().nullable(),
  status: z.enum(["waiting", "replied", "failed"]),
  createdAt: z.string(),
  repliedAt: z.string().nullable(),
});
export type Exchange = z.infer<typeof Exchange>;

// chainLimit: how many bot-to-bot requests one chain (a message from you, a room post or a routine run) may make.
export const Settings = z.object({ chainLimit: z.number().int().min(1).max(20) });
export type Settings = z.infer<typeof Settings>;
```

- [ ] **Step 4: Replace wire.ts**

Replace `packages/protocol/src/wire.ts` with:

```ts
import { z } from "zod";
import {
  Approval,
  ApprovalDecision,
  Bot,
  BotInput,
  BotPatch,
  Exchange,
  Message,
  ModelInfo,
  Provider,
  ProviderSetup,
  Routine,
  RoutineInput,
  RoutinePatch,
  RoutineRun,
  Settings,
} from "./model.ts";

export const PROTOCOL_VERSION = 1;

const Empty = z.object({});
const Id = z.object({ id: z.string() });
const Page = { before: z.string().optional(), limit: z.number().int().min(1).max(200).default(50) };

// Every request the daemon answers: params are validated on arrival, results are what handlers return.
export const methods = {
  "setup.status": { params: Empty, result: z.array(ProviderSetup) },
  "setup.signIn": { params: z.object({ provider: Provider }), result: z.object({ url: z.string().nullable() }) },
  "models.list": { params: z.object({ provider: Provider }), result: z.array(ModelInfo) },
  "bots.list": { params: Empty, result: z.array(Bot) },
  "bots.create": { params: BotInput, result: Bot },
  "bots.update": { params: z.object({ id: z.string(), patch: BotPatch }), result: Bot },
  "bots.delete": { params: Id, result: Empty },
  "bots.pause": { params: z.object({ id: z.string(), paused: z.boolean() }), result: Bot },
  "bots.showBrowser": { params: Id, result: Empty },
  "chat.history": { params: z.object({ botId: z.string(), ...Page }), result: z.array(Message) },
  "chat.send": {
    params: z.object({ botId: z.string(), text: z.string().trim().min(1).max(100_000), files: z.array(z.string()).max(20).default([]) }),
    result: Message,
  },
  "chat.stop": { params: z.object({ botId: z.string() }), result: Empty },
  "chat.retry": { params: z.object({ messageId: z.string() }), result: Message },
  "approvals.respond": { params: z.object({ approvalId: z.string(), decision: ApprovalDecision }), result: Approval },
  "room.history": { params: z.object(Page), result: z.array(Message) },
  "room.post": { params: z.object({ text: z.string().trim().min(1).max(100_000) }), result: Message },
  "exchanges.get": { params: Id, result: z.object({ exchange: Exchange, messages: z.array(Message) }) },
  "routines.list": { params: z.object({ botId: z.string() }), result: z.array(Routine) },
  "routines.create": { params: RoutineInput, result: Routine },
  "routines.update": { params: z.object({ id: z.string(), patch: RoutinePatch }), result: Routine },
  "routines.delete": { params: Id, result: Empty },
  "routines.runs": { params: z.object({ routineId: z.string() }), result: z.array(RoutineRun) },
  "settings.get": { params: Empty, result: Settings },
  "settings.set": { params: Settings.partial(), result: Settings },
} as const;

// Everything the daemon pushes to connected clients.
export const events = {
  "message.added": Message,
  "message.updated": Message,
  "message.delta": z.object({ id: z.string(), conversationId: z.string(), delta: z.string() }),
  "bot.updated": Bot,
  "bot.deleted": Id,
  "approval.added": Approval,
  "approval.resolved": Approval,
  "setup.changed": z.array(ProviderSetup),
  notify: z.object({ botId: z.string(), title: z.string(), body: z.string() }),
  "routine.updated": Routine,
  "routine.deleted": Id,
  "routine.run": RoutineRun,
  "settings.changed": Settings,
} as const;

export type MethodName = keyof typeof methods;
export type ParamsIn<M extends MethodName> = z.input<(typeof methods)[M]["params"]>;
export type ParamsOut<M extends MethodName> = z.output<(typeof methods)[M]["params"]>;
export type Result<M extends MethodName> = z.output<(typeof methods)[M]["result"]>;
export type EventName = keyof typeof events;
export type EventData<E extends EventName> = z.output<(typeof events)[E]>;

export const ClientMessage = z.discriminatedUnion("type", [
  z.object({ type: z.literal("hello"), protocol: z.number().int(), token: z.string() }),
  z.object({ type: z.literal("req"), id: z.number().int(), method: z.string(), params: z.unknown() }),
]);
export type ClientMessage = z.infer<typeof ClientMessage>;

export type ServerMessage =
  | { type: "welcome"; protocol: number }
  | { type: "res"; id: number; ok: true; result: unknown }
  | { type: "res"; id: number; ok: false; error: { code: string; message: string } }
  | { type: "event"; event: EventName; data: unknown };
```

- [ ] **Step 5: Run the tests**

Run: `npm test`
Expected: the new protocol tests PASS. `npm run typecheck` now reports errors in `packages/daemon/src/approvals.ts`, because `Approval` needs `kind` and `payload`. Task 2 fixes that; carry on.

- [ ] **Step 6: Commit**

```bash
git add packages/protocol
git commit -m "Add protocol messages for routines, exchanges, the Team room and settings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Storage for routines, exchanges, chains and settings

**Files:**
- Modify: `packages/daemon/src/db.ts` (add a migration), `packages/daemon/src/approvals.ts` (replace the file)
- Create: `packages/daemon/src/settings.ts`
- Test: `packages/daemon/src/db.test.ts` (fix one expectation), `packages/daemon/src/settings.test.ts`, `packages/daemon/src/approvals.test.ts` (add a test)

**Interfaces:**
- Consumes: `db.ts` from plan 1A, and the protocol from Task 1.
- Produces:
  - **Migration 2:** tables `settings`, `chains`, `exchanges`, `routines` and `routine_runs`; new `work_items` columns `conversation_id`, `priority`, `chain_id`, `exchange_id` and `routine_run_id`; new `approvals` columns `kind` and `payload`; and the conversation row `team`. Exchanges have no foreign keys, so an exchange outlives a deleted bot. `reply_to` is the conversation the asking bot's turn was writing to.
  - **`settings.ts`:** `getSettings(db): Settings` (defaults `{ chainLimit: 5 }`) and `setSettings(db, patch: Partial<Settings>): Settings`.
  - **`approvals.ts`:** `createApproval(db, { id, botId, tool, summary, detail, rule, messageId, kind?, payload? })`, which defaults to kind `"tool"` and payload `null`. Every approval it returns has `kind` and `payload`.

- [ ] **Step 1: Write the failing tests**

In `packages/daemon/src/db.test.ts`, change the expected version in the first test from `{ user_version: 1 }` to `{ user_version: 2 }`, and add this test to the `openDb` describe block:

```ts
  it("has the Team room conversation and the routine tables after migration 2", () => {
    const db = openDb(join(root(), "t.db"));
    expect(db.prepare("SELECT kind FROM conversations WHERE id = 'team'").get()).toEqual({ kind: "room" });
    for (const table of ["settings", "chains", "exchanges", "routines", "routine_runs"]) {
      expect(db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?").get(table)).toEqual({ name: table });
    }
  });
```

Create `packages/daemon/src/settings.test.ts`:

```ts
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { getSettings, setSettings } from "./settings.ts";
import { tempRoot } from "./testing.ts";

describe("settings", () => {
  it("defaults the chain limit to 5 and remembers a change", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    expect(getSettings(db)).toEqual({ chainLimit: 5 });
    expect(setSettings(db, { chainLimit: 2 })).toEqual({ chainLimit: 2 });
    expect(getSettings(db)).toEqual({ chainLimit: 2 });
  });
});
```

Add this test to `packages/daemon/src/approvals.test.ts` (inside the `describe`):

```ts
  it("keeps routine proposals, with their payload, pending after a restart; other approvals are kind tool", () => {
    const message = addMessage(db, { conversationId: botId, author: botId, kind: "approval", body: "Add a routine", status: "done" });
    const routine = createApproval(db, { id: "r1", botId, kind: "routine", tool: "Routine", summary: "s", detail: "d", rule: null, payload: { name: "Morning check" }, messageId: message.id });
    expect(routine).toMatchObject({ kind: "routine", payload: { name: "Morning check" } });
    expect(approval()).toMatchObject({ kind: "tool", payload: null });
    expirePendingApprovals(db);
    expect(getApproval(db, "r1").status).toBe("pending");
  });
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. The user version is still 1, there's no `team` conversation, `./settings.ts` is missing, and approvals have no `kind`.

- [ ] **Step 3: Add migration 2**

In `packages/daemon/src/db.ts`, the `migrations` array ends with the closing backtick of migration 1, followed by `,` and then `];`. Insert this second element before `];`:

```ts
  `
  CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
  CREATE TABLE chains (id TEXT PRIMARY KEY, requests_made INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL);
  CREATE TABLE exchanges (
    id TEXT PRIMARY KEY,
    from_bot_id TEXT NOT NULL,
    to_bot_id TEXT NOT NULL,
    request TEXT NOT NULL,
    reply TEXT,
    status TEXT NOT NULL,
    chain_id TEXT NOT NULL,
    reply_to TEXT NOT NULL,
    created_at TEXT NOT NULL,
    replied_at TEXT
  );
  CREATE TABLE routines (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    days TEXT NOT NULL,
    times TEXT NOT NULL,
    time_zone TEXT NOT NULL,
    time_limit_minutes INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1,
    next_run_at TEXT,
    created_at TEXT NOT NULL
  );
  CREATE TABLE routine_runs (
    id TEXT PRIMARY KEY,
    routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE,
    due_at TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT NOT NULL,
    note TEXT
  );
  CREATE INDEX routine_runs_by_routine ON routine_runs (routine_id, due_at);
  ALTER TABLE work_items ADD COLUMN conversation_id TEXT;
  ALTER TABLE work_items ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
  ALTER TABLE work_items ADD COLUMN chain_id TEXT;
  ALTER TABLE work_items ADD COLUMN exchange_id TEXT;
  ALTER TABLE work_items ADD COLUMN routine_run_id TEXT;
  UPDATE work_items SET conversation_id = bot_id;
  ALTER TABLE approvals ADD COLUMN kind TEXT NOT NULL DEFAULT 'tool';
  ALTER TABLE approvals ADD COLUMN payload TEXT;
  INSERT INTO conversations (id, kind, bot_id) VALUES ('team', 'room', NULL);
  `,
```

- [ ] **Step 4: Write settings.ts and replace approvals.ts**

Create `packages/daemon/src/settings.ts`:

```ts
import { Settings } from "@tjommie/protocol";
import type { Db } from "./db.ts";

const DEFAULTS: Settings = { chainLimit: 5 };

export function getSettings(db: Db): Settings {
  const rows = db.prepare("SELECT key, value FROM settings").all() as { key: string; value: string }[];
  return Settings.parse({ ...DEFAULTS, ...Object.fromEntries(rows.map((row) => [row.key, JSON.parse(row.value)])) });
}

export function setSettings(db: Db, patch: Partial<Settings>): Settings {
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    db.prepare("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value").run(key, JSON.stringify(value));
  }
  return getSettings(db);
}
```

Replace `packages/daemon/src/approvals.ts` with:

```ts
import type { Approval, ApprovalDecision } from "@tjommie/protocol";
import { nowIso, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

type ApprovalRow = {
  id: string;
  bot_id: string;
  kind: Approval["kind"];
  tool: string;
  summary: string;
  detail: string;
  rule: string | null;
  payload: string | null;
  status: Approval["status"];
  message_id: string;
  created_at: string;
};

const toApproval = (row: ApprovalRow): Approval => ({
  id: row.id,
  botId: row.bot_id,
  kind: row.kind,
  tool: row.tool,
  summary: row.summary,
  detail: row.detail,
  rule: row.rule,
  payload: row.payload === null ? null : (JSON.parse(row.payload) as Record<string, unknown>),
  status: row.status,
  messageId: row.message_id,
  createdAt: row.created_at,
});

export function createApproval(
  db: Db,
  a: {
    id: string;
    botId: string;
    tool: string;
    summary: string;
    detail: string;
    rule: string | null;
    messageId: string;
    kind?: Approval["kind"];
    payload?: Record<string, unknown> | null;
  },
): Approval {
  db.prepare(
    "INSERT INTO approvals (id, bot_id, kind, tool, summary, detail, rule, payload, status, message_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
  ).run(a.id, a.botId, a.kind ?? "tool", a.tool, a.summary, a.detail, a.rule, a.payload ? JSON.stringify(a.payload) : null, a.messageId, nowIso());
  return getApproval(db, a.id);
}

export function getApproval(db: Db, id: string): Approval {
  const row = db.prepare("SELECT * FROM approvals WHERE id = ?").get(id) as ApprovalRow | undefined;
  if (!row) throw new RpcError("not_found", "No such approval");
  return toApproval(row);
}

export function resolveApproval(db: Db, id: string, decision: ApprovalDecision): Approval {
  const approval = getApproval(db, id);
  if (approval.status !== "pending") throw new RpcError("already_answered", "This request was already answered");
  db.prepare("UPDATE approvals SET status = ?, decided_at = ? WHERE id = ?").run(decision === "deny" ? "denied" : "allowed", nowIso(), id);
  if (decision === "always" && approval.rule) db.prepare("INSERT OR IGNORE INTO allow_rules (bot_id, rule) VALUES (?, ?)").run(approval.botId, approval.rule);
  return getApproval(db, id);
}

export function allowRules(db: Db, botId: string): string[] {
  return (db.prepare("SELECT rule FROM allow_rules WHERE bot_id = ? ORDER BY rule").all(botId) as { rule: string }[]).map((row) => row.rule);
}

// After a restart the CLI that asked is gone, so nothing can answer its requests any more.
// Routine proposals don't depend on a CLI, so they stay pending.
export function expirePendingApprovals(db: Db): Approval[] {
  const ids = (db.prepare("SELECT id FROM approvals WHERE status = 'pending' AND kind = 'tool' ORDER BY created_at, rowid").all() as { id: string }[]).map((row) => row.id);
  db.prepare("UPDATE approvals SET status = 'denied', decided_at = ? WHERE status = 'pending' AND kind = 'tool'").run(nowIso());
  return ids.map((id) => getApproval(db, id));
}
```

- [ ] **Step 5: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including the db, settings and approvals tests.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/db.ts packages/daemon/src/db.test.ts packages/daemon/src/settings.ts packages/daemon/src/settings.test.ts packages/daemon/src/approvals.ts packages/daemon/src/approvals.test.ts
git commit -m "Add storage for routines, exchanges, chains and settings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Queue sources, priorities, steering and chains

**Files:**
- Modify: `packages/daemon/src/queue.ts` (replace the file)
- Test: `packages/daemon/src/queue.test.ts` (replace the file)

**Interfaces:**
- Consumes: migration 2 (Task 2).
- Produces:
  - `type WorkSource = "you" | "room" | "bot_request" | "bot_reply" | "routine"`.
  - `WorkItem`, which gains `conversationId`, `chainId`, `exchangeId` and `routineRunId`.
  - `type NewWork = { botId; source; text; messageId?; conversationId?; chainId?; exchangeId?; routineRunId? }`.
  - `enqueue(db, NewWork)`, `nextQueued(db, botId)` (priority first, then oldest), `nextSteerable(db, botId, conversationId)`, `setStatus`, `getWorkItem`, `lastItemForMessage` and `recoverAfterRestart`.
  - Chains: `newChain(db): string` and `useChainRequest(db, chainId, limit): boolean`.

- [ ] **Step 1: Write the failing tests**

Replace `packages/daemon/src/queue.test.ts` with:

```ts
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, lastItemForMessage, newChain, nextQueued, nextSteerable, recoverAfterRestart, setStatus, useChainRequest, type WorkSource } from "./queue.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let botId: string;

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
});

const add = (text: string, source: WorkSource = "you", extra: { messageId?: string | null; conversationId?: string } = {}) =>
  enqueue(db, { botId, source, text, messageId: extra.messageId ?? null, conversationId: extra.conversationId });

describe("work queue", () => {
  it("hands out queued work oldest first within a priority, even within one millisecond", () => {
    const [a, b, c] = [add("a"), add("b"), add("c")];
    expect(nextQueued(db, botId)?.id).toBe(a.id);
    setStatus(db, a.id, "running");
    expect(nextQueued(db, botId)?.id).toBe(b.id);
    setStatus(db, b.id, "done");
    expect(nextQueued(db, botId)?.id).toBe(c.id);
  });

  it("puts your messages and replies ahead of bot requests and routine runs", () => {
    add("routine", "routine");
    add("request", "bot_request");
    const reply = add("reply", "bot_reply");
    expect(nextQueued(db, botId)?.id).toBe(reply.id);
  });

  it("defaults the conversation to the bot's own chat", () => {
    expect(add("x").conversationId).toBe(botId);
    expect(add("y", "room", { conversationId: "team" }).conversationId).toBe("team");
  });

  it("only steers your messages and replies, and only into the same conversation", () => {
    add("room post", "room", { conversationId: "team" });
    add("request", "bot_request", { conversationId: "exchange-1" });
    expect(nextSteerable(db, botId, botId)).toBeNull();
    const mine = add("mine", "you");
    expect(nextSteerable(db, botId, botId)?.id).toBe(mine.id);
    expect(nextSteerable(db, botId, "team")).toBeNull();
  });

  it("puts running work back in the queue with its start time cleared", () => {
    const item = add("x");
    setStatus(db, item.id, "running");
    setStatus(db, item.id, "queued");
    expect(getWorkItem(db, item.id).status).toBe("queued");
    expect(db.prepare("SELECT started_at FROM work_items WHERE id = ?").get(item.id)).toEqual({ started_at: null });
  });

  it("finds the latest attempt for a message", () => {
    add("first try", "you", { messageId: "m1" });
    const retry = add("first try", "you", { messageId: "m1" });
    expect(lastItemForMessage(db, "m1")?.id).toBe(retry.id);
    expect(lastItemForMessage(db, "nope")).toBeNull();
  });

  it("marks running work interrupted after a restart and leaves queued work alone (recovery after restart)", () => {
    const running = add("running");
    const queued = add("queued");
    setStatus(db, running.id, "running");
    expect(recoverAfterRestart(db).map((item) => [item.id, item.status])).toEqual([[running.id, "interrupted"]]);
    expect(nextQueued(db, botId)?.id).toBe(queued.id);
  });

  it("counts bot-to-bot requests per chain up to the limit", () => {
    const chain = newChain(db);
    expect([1, 2, 3].map(() => useChainRequest(db, chain, 2))).toEqual([true, true, false]);
    expect(useChainRequest(db, newChain(db), 2)).toBe(true);
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. `newChain`, `nextSteerable` and `useChainRequest` are missing, and priorities aren't used yet.

- [ ] **Step 3: Replace queue.ts**

Replace `packages/daemon/src/queue.ts` with:

```ts
import { newId, nowIso, type Db } from "./db.ts";

// Where a piece of work came from.
export type WorkSource = "you" | "room" | "bot_request" | "bot_reply" | "routine";
export type WorkStatus = "queued" | "running" | "done" | "failed" | "interrupted";
export type WorkItem = {
  id: string;
  botId: string;
  source: WorkSource;
  text: string;
  messageId: string | null;
  conversationId: string; // where the turn's output goes: the bot's chat, "team" or an exchange
  chainId: string | null;
  exchangeId: string | null;
  routineRunId: string | null;
  status: WorkStatus;
  createdAt: string;
};
export type NewWork = {
  botId: string;
  source: WorkSource;
  text: string;
  messageId?: string | null;
  conversationId?: string;
  chainId?: string | null;
  exchangeId?: string | null;
  routineRunId?: string | null;
};

// Your messages and replies to a bot's own requests go first; requests from other bots and routine runs wait.
const PRIORITY: Record<WorkSource, number> = { you: 0, room: 0, bot_reply: 0, bot_request: 1, routine: 1 };

type WorkRow = {
  id: string;
  bot_id: string;
  source: WorkSource;
  text: string;
  message_id: string | null;
  conversation_id: string;
  chain_id: string | null;
  exchange_id: string | null;
  routine_run_id: string | null;
  status: WorkStatus;
  created_at: string;
};

const toItem = (row: WorkRow): WorkItem => ({
  id: row.id,
  botId: row.bot_id,
  source: row.source,
  text: row.text,
  messageId: row.message_id,
  conversationId: row.conversation_id,
  chainId: row.chain_id,
  exchangeId: row.exchange_id,
  routineRunId: row.routine_run_id,
  status: row.status,
  createdAt: row.created_at,
});

export function enqueue(db: Db, work: NewWork): WorkItem {
  const id = newId();
  db.prepare(
    `INSERT INTO work_items (id, bot_id, source, text, message_id, conversation_id, priority, chain_id, exchange_id, routine_run_id, status, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?)`,
  ).run(
    id,
    work.botId,
    work.source,
    work.text,
    work.messageId ?? null,
    work.conversationId ?? work.botId,
    PRIORITY[work.source],
    work.chainId ?? null,
    work.exchangeId ?? null,
    work.routineRunId ?? null,
    nowIso(),
  );
  return getWorkItem(db, id);
}

export function getWorkItem(db: Db, id: string): WorkItem {
  const row = db.prepare("SELECT * FROM work_items WHERE id = ?").get(id) as WorkRow | undefined;
  if (!row) throw new Error(`No work item ${id}`);
  return toItem(row);
}

// Highest priority first, then oldest. rowid breaks ties within one millisecond.
export function nextQueued(db: Db, botId: string): WorkItem | null {
  const row = db.prepare("SELECT * FROM work_items WHERE bot_id = ? AND status = 'queued' ORDER BY priority, created_at, rowid LIMIT 1").get(botId) as
    | WorkRow
    | undefined;
  return row ? toItem(row) : null;
}

// Work that may join a turn already writing to `conversationId`: your messages there, and replies to the bot's own requests.
export function nextSteerable(db: Db, botId: string, conversationId: string): WorkItem | null {
  const row = db
    .prepare(
      `SELECT * FROM work_items WHERE bot_id = ? AND status = 'queued' AND source IN ('you', 'bot_reply') AND conversation_id = ?
       ORDER BY priority, created_at, rowid LIMIT 1`,
    )
    .get(botId, conversationId) as WorkRow | undefined;
  return row ? toItem(row) : null;
}

export function setStatus(db: Db, id: string, status: WorkStatus): void {
  if (status === "queued") db.prepare("UPDATE work_items SET status = 'queued', started_at = NULL, ended_at = NULL WHERE id = ?").run(id);
  else if (status === "running") db.prepare("UPDATE work_items SET status = 'running', started_at = ? WHERE id = ?").run(nowIso(), id);
  else db.prepare("UPDATE work_items SET status = ?, ended_at = ? WHERE id = ?").run(status, nowIso(), id);
}

export function lastItemForMessage(db: Db, messageId: string): WorkItem | null {
  const row = db.prepare("SELECT * FROM work_items WHERE message_id = ? ORDER BY created_at DESC, rowid DESC LIMIT 1").get(messageId) as WorkRow | undefined;
  return row ? toItem(row) : null;
}

// After a restart nothing is running any more: mark what was running as interrupted and return it.
export function recoverAfterRestart(db: Db): WorkItem[] {
  const rows = db.prepare("SELECT * FROM work_items WHERE status = 'running' ORDER BY created_at, rowid").all() as WorkRow[];
  db.prepare("UPDATE work_items SET status = 'interrupted', ended_at = ? WHERE status = 'running'").run(nowIso());
  return rows.map((row) => ({ ...toItem(row), status: "interrupted" }));
}

// A chain is one message from you, one room post or one routine run, plus everything it causes.
export function newChain(db: Db): string {
  const id = newId();
  db.prepare("INSERT INTO chains (id, requests_made, created_at) VALUES (?, 0, ?)").run(id, nowIso());
  return id;
}

// Counts one bot-to-bot request against the chain. False once the chain has used `limit` requests.
export function useChainRequest(db: Db, chainId: string, limit: number): boolean {
  const result = db.prepare("UPDATE chains SET requests_made = requests_made + 1 WHERE id = ? AND requests_made < ?").run(chainId, limit);
  return Number(result.changes) === 1;
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS. The queue has 8 tests, and every earlier test still passes because `enqueue` keeps its old fields.

Run: `npm run typecheck`
Expected: no errors. The runner still compiles; it doesn't use the new fields until Task 8.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/queue.ts packages/daemon/src/queue.test.ts
git commit -m "Add queue sources, priorities, steering rules and chains

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The app's MCP endpoint

**Files:**
- Create: `packages/daemon/src/mcp.ts`
- Modify: `packages/daemon/src/server.ts` (add extra HTTP routes)
- Test: `packages/daemon/src/mcp.test.ts`

**Interfaces:**
- Consumes: `startServer` from plan 1A.
- Produces:
  - `server.ts`:
    - `type Route = (req: IncomingMessage, res: ServerResponse) => void | Promise<void>`.
    - `startServer` accepts `routes?: Record<string, Route>`. A route answers only requests with this machine's Host header and no Origin header; anything else gets 403.
  - `mcp.ts`:
    - `type McpTool = { name; description; input: z.ZodObject; run(botId, args): string | Promise<string> }`.
    - `defineTool(tool)`, which keeps `run`'s arguments typed by `input`.
    - `class McpTokens` with `issue(botId): string`, `revoke(botId): void` and `botFor(token): string | null`.
    - `mcpHandler({ tokens, tools }): Route`.
    - When a tool throws, its error message goes back to the bot as a failed call (`isError: true`). That's how tools say "not allowed" or "limit reached".

- [ ] **Step 1: Write the failing tests**

Create `packages/daemon/src/mcp.test.ts`:

```ts
import { request } from "node:http";
import { afterEach, describe, expect, it } from "vitest";
import { z } from "zod";
import { McpTokens, defineTool, mcpHandler } from "./mcp.ts";
import { startServer, type Server } from "./server.ts";

let server: Server | undefined;
afterEach(async () => {
  await server?.close();
  server = undefined;
});

const echo = defineTool({
  name: "echo",
  description: "Repeats the text back.",
  input: z.object({ text: z.string().describe("What to repeat") }),
  run: (botId, { text }) => `${botId} says ${text}`,
});
const refuses = defineTool({
  name: "refuses",
  description: "Always refuses.",
  input: z.object({}),
  run: () => {
    throw new Error("Limit reached, ask the user.");
  },
});

async function setup() {
  const tokens = new McpTokens();
  server = await startServer({ port: 0, token: "t", allowedOrigins: [], handlers: {}, routes: { "/mcp": mcpHandler({ tokens, tools: [echo, refuses] }) } });
  return { tokens, port: server.port };
}

// Plain node:http, so the test can send any Host or Origin header, like any program could.
function post(port: number, body: unknown, headers: Record<string, string> = {}, method = "POST"): Promise<{ status: number; body: any }> {
  return new Promise((resolve, reject) => {
    const req = request({ host: "127.0.0.1", port, path: "/mcp", method, headers: { "content-type": "application/json", ...headers } }, (res) => {
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => (text += chunk));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body: text ? JSON.parse(text) : null }));
    });
    req.on("error", reject);
    req.end(typeof body === "string" ? body : JSON.stringify(body));
  });
}

describe("the app's MCP endpoint", () => {
  it("echoes the client's protocol version, lists tools and runs them as the token's bot", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const init = await post(port, { jsonrpc: "2.0", id: 0, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "claude-code" } } }, auth);
    expect(init).toEqual({
      status: 200,
      body: { jsonrpc: "2.0", id: 0, result: { protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "tjommie", version: "0.1.0" } } },
    });
    const list = await post(port, { jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }, auth);
    expect(list.body.result.tools[0]).toEqual({
      name: "echo",
      description: "Repeats the text back.",
      inputSchema: { type: "object", properties: { text: { type: "string", description: "What to repeat" } }, required: ["text"] },
    });
    const call = await post(port, { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "echo", arguments: { text: "hi ñ" } } }, auth);
    expect(call.body.result).toEqual({ content: [{ type: "text", text: "bot-1 says hi ñ" }], isError: false });
  });

  it("turns bad arguments and refusals into failed calls the bot can read", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const bad = await post(port, { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "echo", arguments: {} } }, auth);
    expect(bad.body.result.isError).toBe(true);
    expect(bad.body.result.content[0].text).toContain("text");
    const refused = await post(port, { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "refuses", arguments: {} } }, auth);
    expect(refused.body.result).toEqual({ content: [{ type: "text", text: "Limit reached, ask the user." }], isError: true });
  });

  it("accepts notifications with 202, refuses unknown methods, and answers batches", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, { jsonrpc: "2.0", method: "notifications/initialized" }, auth)).toEqual({ status: 202, body: null });
    expect((await post(port, { id: 3, method: "server/discover", params: {} }, auth)).body.error.code).toBe(-32601); // Claude omits "jsonrpc" here
    expect((await post(port, [{ jsonrpc: "2.0", id: 4, method: "ping" }, { jsonrpc: "2.0", method: "notifications/cancelled" }], auth)).body).toEqual([
      { jsonrpc: "2.0", id: 4, result: {} },
    ]);
    expect(await post(port, "{not json", auth)).toMatchObject({ status: 400, body: { error: { code: -32700 } } });
  });

  it("refuses browsers, other hosts, bad tokens, revoked tokens and huge bodies (refuses browsers and bad tokens)", async () => {
    const { tokens, port } = await setup();
    const token = tokens.issue("bot-1");
    const ping = { jsonrpc: "2.0", id: 1, method: "ping" };
    expect((await post(port, ping, { authorization: `Bearer ${token}`, origin: "https://evil.example" })).status).toBe(403);
    expect((await post(port, ping, { authorization: `Bearer ${token}`, host: `evil.example:${port}` })).status).toBe(403);
    expect((await post(port, ping)).status).toBe(401);
    expect((await post(port, ping, { authorization: "Bearer nope" })).status).toBe(401);
    expect((await post(port, "", { authorization: `Bearer ${token}` }, "GET")).status).toBe(405);
    expect((await post(port, "x".repeat(1024 * 1024 + 1), { authorization: `Bearer ${token}` })).status).toBe(413);
    tokens.revoke("bot-1");
    expect((await post(port, ping, { authorization: `Bearer ${token}` })).status).toBe(401);
  });

  it("gives each session a new token and forgets the old one", () => {
    const tokens = new McpTokens();
    const first = tokens.issue("bot-1");
    const second = tokens.issue("bot-1");
    expect(tokens.botFor(first)).toBeNull();
    expect(tokens.botFor(second)).toBe("bot-1");
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. `./mcp.ts` can't be found.

- [ ] **Step 3: Add routes to the server**

Make three changes in `packages/daemon/src/server.ts`.

1. Change the `node:http` import to:

```ts
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
```

2. Replace everything from the `export type Server` line through the `export async function startServer(…)` line (the `HELLO_TIMEOUT_MS` constant sits between them) with:

```ts
export type Server = { port: number; broadcast: Broadcast; close: () => Promise<void> };
// An extra HTTP endpoint, such as the app's MCP server at /mcp.
export type Route = (req: IncomingMessage, res: ServerResponse) => void | Promise<void>;

const HELLO_TIMEOUT_MS = 5000;

export async function startServer(opts: {
  port: number;
  token: string;
  handlers: Handlers;
  allowedOrigins: string[];
  routes?: Record<string, Route>;
}): Promise<Server> {
```

3. In the `createServer` callback, replace the final `res.writeHead(404).end();` with:

```ts
    const route = opts.routes?.[(req.url ?? "").split("?")[0]];
    if (route) {
      // Only programs on this machine: browsers always send an Origin header, the CLIs never do.
      if (!isLocalHost(req, port) || req.headers.origin !== undefined) {
        res.writeHead(403).end();
        return;
      }
      Promise.resolve(route(req, res)).catch(() => {
        if (!res.headersSent) res.writeHead(500);
        res.end();
      });
      return;
    }
    res.writeHead(404).end();
```

- [ ] **Step 4: Write mcp.ts**

Create `packages/daemon/src/mcp.ts`:

```ts
import { randomBytes } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { z } from "zod";

const MAX_BODY_BYTES = 1024 * 1024;
const FALLBACK_PROTOCOL = "2025-06-18";

export type McpTool = {
  name: string;
  description: string;
  input: z.ZodObject;
  // Returns the text the bot sees. A thrown error's message goes back to the bot as a failed call.
  run(botId: string, args: any): string | Promise<string>;
};

// Keeps each tool's `run` typed by its own input schema.
export const defineTool = <S extends z.ZodObject>(tool: {
  name: string;
  description: string;
  input: S;
  run(botId: string, args: z.output<S>): string | Promise<string>;
}): McpTool => tool;

// One bearer token per running CLI session. It tells the endpoint which bot is calling.
export class McpTokens {
  #bots = new Map<string, string>(); // token -> bot id

  issue(botId: string): string {
    this.revoke(botId);
    const token = randomBytes(24).toString("base64url");
    this.#bots.set(token, botId);
    return token;
  }

  revoke(botId: string): void {
    for (const [token, id] of this.#bots) if (id === botId) this.#bots.delete(token);
  }

  botFor(token: string): string | null {
    return this.#bots.get(token) ?? null;
  }
}

type Answer = { jsonrpc: "2.0"; id: string | number; result?: unknown; error?: { code: number; message: string } };

// A stateless MCP endpoint (Streamable HTTP, plain JSON answers). Both CLIs only need initialize, ping,
// tools/list and tools/call. Notifications get 202 and no body.
export function mcpHandler(opts: { tokens: McpTokens; tools: McpTool[] }): (req: IncomingMessage, res: ServerResponse) => Promise<void> {
  const listed = opts.tools.map((tool) => ({ name: tool.name, description: tool.description, inputSchema: jsonSchema(tool.input) }));

  async function answer(botId: string, message: unknown): Promise<Answer | null> {
    if (typeof message !== "object" || message === null) return null;
    const { id, method, params } = message as { id?: string | number | null; method?: unknown; params?: any };
    if (typeof method !== "string" || id === undefined || id === null) return null; // a notification or a response
    const ok = (result: unknown): Answer => ({ jsonrpc: "2.0", id, result });
    switch (method) {
      case "initialize":
        return ok({
          protocolVersion: typeof params?.protocolVersion === "string" ? params.protocolVersion : FALLBACK_PROTOCOL,
          capabilities: { tools: {} },
          serverInfo: { name: "tjommie", version: "0.1.0" },
        });
      case "ping":
        return ok({});
      case "tools/list":
        return ok({ tools: listed });
      case "tools/call": {
        const tool = opts.tools.find((t) => t.name === params?.name);
        if (!tool) return { jsonrpc: "2.0", id, error: { code: -32602, message: `Unknown tool ${String(params?.name)}` } };
        const args = tool.input.safeParse(params?.arguments ?? {});
        if (!args.success) return ok(text(z.prettifyError(args.error), true));
        try {
          return ok(text(await tool.run(botId, args.data), false));
        } catch (error) {
          return ok(text(error instanceof Error ? error.message : String(error), true));
        }
      }
      default:
        return { jsonrpc: "2.0", id, error: { code: -32601, message: `Method not found: ${method}` } };
    }
  }

  return async (req, res) => {
    if (req.method !== "POST") {
      res.writeHead(405, { allow: "POST" }).end();
      return;
    }
    const auth = req.headers.authorization ?? "";
    const botId = auth.startsWith("Bearer ") ? opts.tokens.botFor(auth.slice(7)) : null;
    if (!botId) {
      res.writeHead(401).end();
      return;
    }
    let body: unknown;
    try {
      body = JSON.parse(await readBody(req));
    } catch (error) {
      if (error instanceof TooLarge) res.writeHead(413).end();
      else send(res, 400, { jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
      return;
    }
    const messages: unknown[] = Array.isArray(body) ? body : [body];
    const answers = (await Promise.all(messages.map((message) => answer(botId, message)))).filter((a) => a !== null);
    if (answers.length === 0) res.writeHead(202).end();
    else send(res, 200, Array.isArray(body) ? answers : answers[0]);
  };
}

const text = (value: string, isError: boolean) => ({ content: [{ type: "text", text: value }], isError });

function jsonSchema(schema: z.ZodObject): Record<string, unknown> {
  const out = z.toJSONSchema(schema, { io: "input" }) as Record<string, unknown>;
  delete out.$schema;
  return out;
}

function send(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(body));
}

class TooLarge extends Error {}

async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    size += (chunk as Buffer).length;
    if (size > MAX_BODY_BYTES) throw new TooLarge();
    chunks.push(chunk as Buffer);
  }
  return Buffer.concat(chunks).toString("utf8");
}
```

- [ ] **Step 5: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including the 5 new MCP tests and the 5 existing server tests.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/mcp.ts packages/daemon/src/mcp.test.ts packages/daemon/src/server.ts
git commit -m "Add the app's MCP endpoint with per-session tokens

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Routines and their schedule

**Files:**
- Create: `packages/daemon/src/routines.ts`
- Test: `packages/daemon/src/routines.test.ts`

**Interfaces:**
- Consumes: `getStoredBot` (bots.ts); `newId`, `nowIso` and `transaction` (db.ts); `RpcError`; migration 2 (Task 2).
- Produces:
  - Constants and helpers:
    - `RUNS_KEPT = 20` and `hostTimeZone(): string`.
    - `wallTimeToUtc(year, month, day, hour, minute, timeZone): number`.
    - `nextRunAfter({ days, times, timeZone }, afterMs): number | null`, the first run strictly after `afterMs`.
  - Routines:
    - `getRoutine`, `listRoutines(db, botId)`.
    - `createRoutine(db, input, nowMs?)` and `updateRoutine(db, id, patch, nowMs?)`. Both sort and de-duplicate days and times and compute `nextRunAt`, which is `null` while a routine is disabled. An unknown time zone is refused.
    - `deleteRoutine(db, id)`, which also removes its runs still waiting in the queue.
    - `setNextRun(db, id, nextRunAt)`.
  - Runs:
    - `getRun(db, id): RoutineRun | null` and `listRuns(db, routineId)`, newest first.
    - `addRun(db, routineId, dueAt, status, note?)`, which keeps the newest 20.
    - `setRunStatus(db, id, status, note?)` and `hasOpenRun(db, routineId)`.

- [ ] **Step 1: Write the failing tests**

Create `packages/daemon/src/routines.test.ts`:

```ts
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue } from "./queue.ts";
import { addRun, createRoutine, deleteRoutine, listRuns, nextRunAfter, updateRoutine } from "./routines.ts";
import { tempRoot } from "./testing.ts";

const at = (iso: string) => Date.parse(iso);
const iso = (ms: number | null) => (ms === null ? null : new Date(ms).toISOString());
const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];

describe("nextRunAfter", () => {
  const weekdays = { days: [1, 2, 3, 4, 5], times: ["08:48", "12:47"], timeZone: "Africa/Johannesburg" };

  it("finds the next time today, or the next matching day (2026-09-25 is a Friday)", () => {
    expect(iso(nextRunAfter(weekdays, at("2026-09-25T07:00:00Z")))).toBe("2026-09-25T10:47:00.000Z"); // 09:00 in Johannesburg
    expect(iso(nextRunAfter(weekdays, at("2026-09-25T11:00:00Z")))).toBe("2026-09-28T06:48:00.000Z"); // Monday 08:48
    expect(iso(nextRunAfter({ ...weekdays, days: [5], times: ["08:00"] }, at("2026-09-25T07:00:00Z")))).toBe("2026-10-02T06:00:00.000Z");
  });

  it("moves a time skipped by the spring clock change forward by the gap", () => {
    const routine = { days: EVERY_DAY, times: ["02:30"], timeZone: "America/New_York" };
    expect(iso(nextRunAfter(routine, at("2026-03-08T05:00:00Z")))).toBe("2026-03-08T07:30:00.000Z"); // 03:30 EDT
  });

  it("runs a time that happens twice in the autumn only once", () => {
    const routine = { days: EVERY_DAY, times: ["01:30"], timeZone: "America/New_York" };
    const first = nextRunAfter(routine, at("2026-11-01T04:00:00Z"));
    expect(iso(first)).toBe("2026-11-01T05:30:00.000Z"); // 01:30 EDT, the first of the two
    expect(iso(nextRunAfter(routine, first!))).toBe("2026-11-02T06:30:00.000Z"); // the next day, not 01:30 EST
  });
});

describe("routines store", () => {
  let db: Db;
  let botId: string;
  beforeEach(() => {
    const root = tempRoot();
    db = openDb(join(root, "t.db"));
    botId = createBot(db, join(root, "bots"), { name: "Pnet apply", color: "green", shape: "blob", instructions: "", provider: "claude", model: "sonnet" }).id;
  });
  const input = () => ({ botId, name: "Morning check", prompt: "Check pnet.co.za", days: [5, 1, 1], times: ["12:47", "08:48"], timeZone: "Africa/Johannesburg" });

  it("stores days and times sorted and computes the next run", () => {
    const routine = createRoutine(db, input(), at("2026-09-25T07:00:00Z"));
    expect(routine).toMatchObject({ days: [1, 5], times: ["08:48", "12:47"], enabled: true, timeLimitMinutes: null, nextRunAt: "2026-09-25T10:47:00.000Z" });
  });

  it("clears the next run while disabled, and refuses an unknown time zone", () => {
    const routine = createRoutine(db, input(), at("2026-09-25T07:00:00Z"));
    expect(updateRoutine(db, routine.id, { enabled: false }).nextRunAt).toBeNull();
    expect(updateRoutine(db, routine.id, { enabled: true }, at("2026-09-25T11:00:00Z")).nextRunAt).toBe("2026-09-28T06:48:00.000Z");
    expect(() => createRoutine(db, { ...input(), timeZone: "Mars/Olympus" })).toThrow(/time zone/);
  });

  it("keeps the newest 20 runs", () => {
    const routine = createRoutine(db, input());
    for (let day = 1; day <= 25; day++) addRun(db, routine.id, `2026-08-${String(day).padStart(2, "0")}T06:48:00.000Z`, "done");
    const runs = listRuns(db, routine.id);
    expect(runs).toHaveLength(20);
    expect(runs[0].dueAt).toBe("2026-08-25T06:48:00.000Z");
  });

  it("deletes a routine with its runs and any run still waiting in the queue", () => {
    const routine = createRoutine(db, input());
    const run = addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "queued");
    enqueue(db, { botId, source: "routine", text: "Check pnet.co.za", routineRunId: run.id });
    deleteRoutine(db, routine.id);
    expect(db.prepare("SELECT COUNT(*) AS n FROM work_items").get()).toEqual({ n: 0 });
    expect(db.prepare("SELECT COUNT(*) AS n FROM routine_runs").get()).toEqual({ n: 0 });
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. `./routines.ts` can't be found.

- [ ] **Step 3: Write routines.ts**

Create `packages/daemon/src/routines.ts`:

```ts
import type { Routine, RoutineInput, RoutinePatch, RoutineRun, RoutineRunStatus } from "@tjommie/protocol";
import { getStoredBot } from "./bots.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

export const RUNS_KEPT = 20;
const DAY_MS = 86_400_000;

export const hostTimeZone = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone;

type RoutineRow = {
  id: string;
  bot_id: string;
  name: string;
  prompt: string;
  days: string;
  times: string;
  time_zone: string;
  time_limit_minutes: number | null;
  enabled: number;
  next_run_at: string | null;
  created_at: string;
};
type RunRow = { id: string; routine_id: string; due_at: string; started_at: string | null; ended_at: string | null; status: RoutineRunStatus; note: string | null };

const toRoutine = (row: RoutineRow): Routine => ({
  id: row.id,
  botId: row.bot_id,
  name: row.name,
  prompt: row.prompt,
  days: JSON.parse(row.days) as number[],
  times: JSON.parse(row.times) as string[],
  timeZone: row.time_zone,
  timeLimitMinutes: row.time_limit_minutes,
  enabled: row.enabled === 1,
  nextRunAt: row.next_run_at,
  createdAt: row.created_at,
});

const toRun = (row: RunRow): RoutineRun => ({
  id: row.id,
  routineId: row.routine_id,
  dueAt: row.due_at,
  startedAt: row.started_at,
  endedAt: row.ended_at,
  status: row.status,
  note: row.note,
});

const formatters = new Map<string, Intl.DateTimeFormat>();

// Milliseconds to add to a UTC instant to get the wall-clock time in `timeZone` at that instant.
function offsetAt(utcMs: number, timeZone: string): number {
  let format = formatters.get(timeZone);
  if (!format) {
    format = new Intl.DateTimeFormat("en-US", { timeZone, hourCycle: "h23", year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric" });
    formatters.set(timeZone, format);
  }
  const p = Object.fromEntries(format.formatToParts(new Date(utcMs)).map((part) => [part.type, Number(part.value)]));
  return Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second) - Math.floor(utcMs / 1000) * 1000;
}

// The UTC instant of a wall-clock time in `timeZone`. A time that a clock change skips moves forward by the
// gap (02:30 becomes 03:30); a time that happens twice when clocks go back uses the first one.
export function wallTimeToUtc(year: number, month: number, day: number, hour: number, minute: number, timeZone: string): number {
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  const candidates = [offsetAt(wall - DAY_MS, timeZone), offsetAt(wall + DAY_MS, timeZone)].map((offset) => wall - offset);
  const exact = candidates.filter((at) => at + offsetAt(at, timeZone) === wall);
  return exact.length ? Math.min(...exact) : Math.max(...candidates);
}

// The first run time strictly after `afterMs`.
export function nextRunAfter(r: { days: number[]; times: string[]; timeZone: string }, afterMs: number): number | null {
  const local = new Date(afterMs + offsetAt(afterMs, r.timeZone));
  for (let i = 0; i <= 7; i++) {
    const date = new Date(Date.UTC(local.getUTCFullYear(), local.getUTCMonth(), local.getUTCDate() + i));
    if (!r.days.includes(date.getUTCDay())) continue;
    const upcoming = r.times
      .map((time) => wallTimeToUtc(date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), Number(time.slice(0, 2)), Number(time.slice(3)), r.timeZone))
      .filter((at) => at > afterMs);
    if (upcoming.length) return Math.min(...upcoming);
  }
  return null;
}

function checkTimeZone(timeZone: string): string {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone });
    return timeZone;
  } catch {
    throw new RpcError("bad_params", `Unknown time zone ${timeZone}`);
  }
}

const nextRunIso = (r: Routine, nowMs: number): string | null => {
  const next = r.enabled ? nextRunAfter(r, nowMs) : null;
  return next === null ? null : new Date(next).toISOString();
};

export function getRoutine(db: Db, id: string): Routine {
  const row = db.prepare("SELECT * FROM routines WHERE id = ?").get(id) as RoutineRow | undefined;
  if (!row) throw new RpcError("not_found", "No such routine");
  return toRoutine(row);
}

export function listRoutines(db: Db, botId: string): Routine[] {
  return (db.prepare("SELECT * FROM routines WHERE bot_id = ? ORDER BY created_at, rowid").all(botId) as RoutineRow[]).map(toRoutine);
}

export function createRoutine(db: Db, input: RoutineInput, nowMs = Date.now()): Routine {
  getStoredBot(db, input.botId);
  const id = newId();
  const days = [...new Set(input.days)].sort((a, b) => a - b);
  const times = [...new Set(input.times)].sort();
  db.prepare(
    `INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, time_limit_minutes, enabled, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    id,
    input.botId,
    input.name,
    input.prompt,
    JSON.stringify(days),
    JSON.stringify(times),
    checkTimeZone(input.timeZone ?? hostTimeZone()),
    input.timeLimitMinutes ?? null,
    input.enabled === false ? 0 : 1,
    nowIso(),
  );
  const routine = getRoutine(db, id);
  setNextRun(db, id, nextRunIso(routine, nowMs));
  return getRoutine(db, id);
}

export function updateRoutine(db: Db, id: string, patch: RoutinePatch, nowMs = Date.now()): Routine {
  const next = { ...getRoutine(db, id), ...patch };
  db.prepare("UPDATE routines SET name = ?, prompt = ?, days = ?, times = ?, time_zone = ?, time_limit_minutes = ?, enabled = ? WHERE id = ?").run(
    next.name,
    next.prompt,
    JSON.stringify([...new Set(next.days)].sort((a, b) => a - b)),
    JSON.stringify([...new Set(next.times)].sort()),
    checkTimeZone(next.timeZone),
    next.timeLimitMinutes,
    next.enabled ? 1 : 0,
    id,
  );
  setNextRun(db, id, nextRunIso(getRoutine(db, id), nowMs));
  return getRoutine(db, id);
}

// Deletes the routine, its run history and any run still waiting in the bot's queue.
export function deleteRoutine(db: Db, id: string): void {
  getRoutine(db, id);
  transaction(db, () => {
    db.prepare("DELETE FROM work_items WHERE status = 'queued' AND routine_run_id IN (SELECT id FROM routine_runs WHERE routine_id = ?)").run(id);
    db.prepare("DELETE FROM routines WHERE id = ?").run(id);
  });
}

export function setNextRun(db: Db, id: string, nextRunAt: string | null): void {
  db.prepare("UPDATE routines SET next_run_at = ? WHERE id = ?").run(nextRunAt, id);
}

export function getRun(db: Db, id: string): RoutineRun | null {
  const row = db.prepare("SELECT * FROM routine_runs WHERE id = ?").get(id) as RunRow | undefined;
  return row ? toRun(row) : null;
}

// Newest first.
export function listRuns(db: Db, routineId: string): RoutineRun[] {
  return (db.prepare("SELECT * FROM routine_runs WHERE routine_id = ? ORDER BY due_at DESC, rowid DESC").all(routineId) as RunRow[]).map(toRun);
}

// Records a run and keeps only the newest RUNS_KEPT for the routine.
export function addRun(db: Db, routineId: string, dueAt: string, status: RoutineRunStatus, note: string | null = null): RoutineRun {
  const id = newId();
  const ended = status === "missed" || status === "skipped" ? nowIso() : null;
  db.prepare("INSERT INTO routine_runs (id, routine_id, due_at, ended_at, status, note) VALUES (?, ?, ?, ?, ?, ?)").run(id, routineId, dueAt, ended, status, note);
  db.prepare(
    `DELETE FROM routine_runs WHERE routine_id = ? AND id NOT IN
       (SELECT id FROM routine_runs WHERE routine_id = ? ORDER BY due_at DESC, rowid DESC LIMIT ?)`,
  ).run(routineId, routineId, RUNS_KEPT);
  return getRun(db, id)!;
}

export function setRunStatus(db: Db, id: string, status: RoutineRunStatus, note: string | null = null): RoutineRun | null {
  if (status === "running") db.prepare("UPDATE routine_runs SET status = 'running', started_at = ? WHERE id = ?").run(nowIso(), id);
  else db.prepare("UPDATE routine_runs SET status = ?, note = ?, ended_at = ? WHERE id = ?").run(status, note, nowIso(), id);
  return getRun(db, id);
}

// True while an earlier run of the routine is still waiting or running.
export function hasOpenRun(db: Db, routineId: string): boolean {
  return db.prepare("SELECT 1 FROM routine_runs WHERE routine_id = ? AND status IN ('queued', 'running')").get(routineId) !== undefined;
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including the 7 routine tests.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/routines.ts packages/daemon/src/routines.test.ts
git commit -m "Add routines with time zone aware run times

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The scheduler

**Files:**
- Create: `packages/daemon/src/scheduler.ts`
- Test: `packages/daemon/src/scheduler.test.ts`

**Interfaces:**
- Consumes: routines.ts (Task 5); `newChain`, `NewWork` and `WorkItem` (Task 3); `addMessage`; `getStoredBot`.
- Produces:
  - `TICK_MS = 30_000`, `GRACE_MS = 120_000`, `duration(minutes): string` ("3 hours", "90 minutes") and `routinePrompt(routine): string`.
  - `type TurnOutcome = { outcome: "done" | "failed" | "interrupted"; error: string | null }`. Task 8's `TurnResult` has these fields and more, so it can be passed straight in.
  - `class Scheduler`:
    - The constructor takes `{ db, broadcast, deliver, stopItem }`. `deliver(work)` is Task 8's `runner.deliver`. `stopItem(itemId, notice)` is Task 8's `runner.stopItem`.
    - `start()` ticks at once, then every 30 s. `stop()` clears the timers. `tick(nowMs?)` can also be called by tests.
    - `runStarted(item)` and `runEnded(item, outcome)` are the runner hooks for routine work items.

A late run (past `GRACE_MS`) is recorded as missed, and the next run is computed from now. That way several missed times produce one Missed row, not a burst. A missed or skipped run gets a "Routine: <name>" row in the bot's chat right away. A queued run gets its row when its turn starts.

- [ ] **Step 1: Write the failing tests**

Create `packages/daemon/src/scheduler.test.ts`:

```ts
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, setPaused } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, type NewWork } from "./queue.ts";
import { addRun, createRoutine, getRoutine, getRun, listRuns } from "./routines.ts";
import { Scheduler } from "./scheduler.ts";
import { tempRoot } from "./testing.ts";

const at = (iso: string) => Date.parse(iso);
let db: Db;
let botId: string;
let delivered: NewWork[];
let stopped: [string, string][];
let events: { event: string; data: any }[];

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Night Shift", color: "purple", shape: "cloud", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
  delivered = [];
  stopped = [];
  events = [];
});
afterEach(() => {
  vi.useRealTimers();
});

const scheduler = () =>
  new Scheduler({
    db,
    broadcast: (event, data) => events.push({ event, data }),
    deliver: (work) => delivered.push(work),
    stopItem: (itemId, notice) => stopped.push([itemId, notice]),
  });

// Every day at 08:00 and 17:00 UTC; the first run is due at 2026-09-20 08:00.
const digest = (extra: { timeLimitMinutes?: number } = {}) =>
  createRoutine(db, { botId, name: "Digest", prompt: "Summarize my inbox", days: [0, 1, 2, 3, 4, 5, 6], times: ["08:00", "17:00"], timeZone: "UTC", ...extra }, at("2026-09-20T00:00:00Z"));

describe("Scheduler", () => {
  it("queues a due run for the bot, with a chain of its own", () => {
    const routine = digest();
    scheduler().tick(at("2026-09-20T08:00:10Z"));
    const [run] = listRuns(db, routine.id);
    expect(run).toMatchObject({ status: "queued", dueAt: "2026-09-20T08:00:00.000Z" });
    expect(delivered).toEqual([{ botId, source: "routine", text: expect.stringContaining("Summarize my inbox"), chainId: expect.any(String), routineRunId: run.id }]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
  });

  it("records one Missed run when the PC was off through several run times, and starts nothing (late tick)", () => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-25T09:00:00Z")); // five days and ten run times later
    expect(listRuns(db, routine.id).map((r) => [r.status, r.dueAt])).toEqual([["missed", "2026-09-20T08:00:00.000Z"]]);
    expect(delivered).toEqual([]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-25T17:00:00.000Z");
    s.tick(at("2026-09-25T09:00:30Z"));
    expect(listRuns(db, routine.id)).toHaveLength(1);
    expect(history(db, botId, undefined, 10).map((m) => [m.kind, m.body, m.data?.status])).toEqual([["routine", "Routine: Digest", "missed"]]);
  });

  it("skips a run while the bot is paused, or while the previous run is still going", () => {
    const routine = digest();
    setPaused(db, botId, true);
    scheduler().tick(at("2026-09-20T08:00:10Z"));
    setPaused(db, botId, false);
    addRun(db, routine.id, "2026-09-20T12:00:00.000Z", "running");
    scheduler().tick(at("2026-09-20T17:00:10Z"));
    expect(listRuns(db, routine.id).map((r) => [r.status, r.note])).toEqual([
      ["skipped", "The previous run was still going."],
      ["running", null],
      ["skipped", "Night Shift was paused."],
    ]);
    expect(delivered).toEqual([]);
  });

  it("labels a run when it starts, and stops it at the time limit", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:10Z"));
    digest({ timeLimitMinutes: 180 });
    const s = scheduler();
    s.tick();
    const item = enqueue(db, delivered[0]);
    s.runStarted(item);
    const runId = item.routineRunId!;
    expect(getRun(db, runId)?.status).toBe("running");
    expect(history(db, botId, undefined, 10).at(-1)).toMatchObject({ kind: "routine", body: "Routine: Digest", data: { runId, status: "running" } });
    vi.advanceTimersByTime(180 * 60_000);
    expect(stopped).toEqual([[item.id, "Stopped at the time limit (3 hours)."]]);
    s.runEnded(getWorkItem(db, item.id), { outcome: "interrupted", error: null });
    expect(getRun(db, runId)).toMatchObject({ status: "stopped", note: "Stopped at the time limit." });
  });

  it("notifies the person when a run fails", () => {
    digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    const item = enqueue(db, delivered[0]);
    s.runStarted(item);
    s.runEnded(item, { outcome: "failed", error: "Codex exited with code 1" });
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "failed", note: "Codex exited with code 1" });
    expect(events.find((e) => e.event === "notify")?.data).toEqual({ botId, title: "Night Shift's routine failed", body: "Digest: Codex exited with code 1" });
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. `./scheduler.ts` can't be found.

- [ ] **Step 3: Write scheduler.ts**

Create `packages/daemon/src/scheduler.ts`:

```ts
import type { Routine, RoutineRun } from "@tjommie/protocol";
import { getStoredBot } from "./bots.ts";
import { addMessage } from "./chat.ts";
import type { Db } from "./db.ts";
import { newChain, type NewWork, type WorkItem } from "./queue.ts";
import { addRun, getRoutine, getRun, hasOpenRun, nextRunAfter, setNextRun, setRunStatus } from "./routines.ts";
import type { Broadcast } from "./server.ts";

export const TICK_MS = 30_000;
export const GRACE_MS = 2 * 60_000; // a run later than this counts as missed

export type TurnOutcome = { outcome: "done" | "failed" | "interrupted"; error: string | null };

export const duration = (minutes: number): string =>
  minutes % 60 === 0 ? `${minutes / 60} ${minutes === 60 ? "hour" : "hours"}` : `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;

// What the bot gets when its routine runs.
export function routinePrompt(routine: Routine): string {
  const limit = routine.timeLimitMinutes ? ` You have ${duration(routine.timeLimitMinutes)}; then Tjommie stops you.` : "";
  return `[Routine "${routine.name}", started by Tjommie on schedule.${limit}]\n${routine.prompt}`;
}

// Starts routine runs when they're due, and records how each one went.
export class Scheduler {
  #db: Db;
  #broadcast: Broadcast;
  #deliver: (work: NewWork) => void;
  #stopItem: (itemId: string, notice: string) => void;
  #timer: NodeJS.Timeout | null = null;
  #limits = new Map<string, NodeJS.Timeout>(); // run id -> its time limit
  #timedOut = new Set<string>();

  constructor(deps: { db: Db; broadcast: Broadcast; deliver: (work: NewWork) => void; stopItem: (itemId: string, notice: string) => void }) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#deliver = deps.deliver;
    this.#stopItem = deps.stopItem;
  }

  // Checks for due runs now and every 30 seconds. Runs that a restart cut off are settled by the runner's
  // restart recovery, which reports them through runEnded (Task 8).
  start(): void {
    this.tick();
    this.#timer = setInterval(() => this.tick(), TICK_MS);
    this.#timer.unref();
  }

  stop(): void {
    if (this.#timer) clearInterval(this.#timer);
    for (const timer of this.#limits.values()) clearTimeout(timer);
    this.#limits.clear();
  }

  tick(nowMs = Date.now()): void {
    const due = this.#db
      .prepare("SELECT id FROM routines WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ? ORDER BY next_run_at")
      .all(new Date(nowMs).toISOString()) as { id: string }[];
    for (const { id } of due) this.#fire(getRoutine(this.#db, id), nowMs);
  }

  // The runner calls this when a routine run's turn starts.
  runStarted(item: WorkItem): void {
    const run = item.routineRunId ? getRun(this.#db, item.routineRunId) : null;
    if (!run || run.status !== "queued") return; // deleted, or starting again after a usage limit
    const routine = getRoutine(this.#db, run.routineId);
    const started = setRunStatus(this.#db, run.id, "running")!;
    this.#label(routine, started);
    this.#broadcast("routine.run", started);
    if (!routine.timeLimitMinutes) return;
    const notice = `Stopped at the time limit (${duration(routine.timeLimitMinutes)}).`;
    const timer = setTimeout(() => {
      this.#limits.delete(run.id);
      this.#timedOut.add(run.id);
      this.#stopItem(item.id, notice);
    }, routine.timeLimitMinutes * 60_000);
    timer.unref();
    this.#limits.set(run.id, timer);
  }

  // The runner calls this when a routine run's turn ends.
  runEnded(item: WorkItem, result: TurnOutcome): void {
    const id = item.routineRunId;
    if (!id) return;
    clearTimeout(this.#limits.get(id));
    this.#limits.delete(id);
    const timedOut = this.#timedOut.delete(id);
    const run = getRun(this.#db, id);
    if (!run) return; // the routine was deleted meanwhile
    const status = timedOut || result.outcome === "interrupted" ? "stopped" : result.outcome;
    const note = timedOut ? "Stopped at the time limit." : status === "stopped" ? "Stopped by you." : status === "failed" ? (result.error ?? "Something went wrong.") : null;
    const ended = setRunStatus(this.#db, id, status, note)!;
    this.#broadcast("routine.run", ended);
    if (status !== "failed") return;
    const routine = getRoutine(this.#db, run.routineId);
    this.#broadcast("notify", { botId: routine.botId, title: `${getStoredBot(this.#db, routine.botId).name}'s routine failed`, body: `${routine.name}: ${note}` });
  }

  #fire(routine: Routine, nowMs: number): void {
    const dueAt = routine.nextRunAt!;
    const bot = getStoredBot(this.#db, routine.botId);
    let run: RoutineRun;
    if (nowMs - Date.parse(dueAt) > GRACE_MS) run = addRun(this.#db, routine.id, dueAt, "missed", "The PC was off or asleep, or Tjommie wasn't running.");
    else if (bot.paused) run = addRun(this.#db, routine.id, dueAt, "skipped", `${bot.name} was paused.`);
    else if (hasOpenRun(this.#db, routine.id)) run = addRun(this.#db, routine.id, dueAt, "skipped", "The previous run was still going.");
    else run = addRun(this.#db, routine.id, dueAt, "queued");
    // Later times that are already due are skipped too, so a PC that slept through several runs gets one Missed.
    const next = nextRunAfter(routine, Math.max(nowMs, Date.parse(dueAt)));
    setNextRun(this.#db, routine.id, next === null ? null : new Date(next).toISOString());
    this.#broadcast("routine.updated", getRoutine(this.#db, routine.id));
    this.#broadcast("routine.run", run);
    if (run.status !== "queued") return this.#label(routine, run);
    this.#deliver({ botId: routine.botId, source: "routine", text: routinePrompt(routine), chainId: newChain(this.#db), routineRunId: run.id });
  }

  // The "Routine: <name>" row in the bot's chat, above the run or saying why it didn't run.
  #label(routine: Routine, run: RoutineRun): void {
    const message = addMessage(this.#db, {
      conversationId: routine.botId,
      author: "system",
      kind: "routine",
      body: `Routine: ${routine.name}`,
      data: { routineId: routine.id, runId: run.id, status: run.status, note: run.note },
      status: "done",
    });
    this.#broadcast("message.added", message);
  }
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including the 5 scheduler tests.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/scheduler.ts packages/daemon/src/scheduler.test.ts
git commit -m "Add the routine scheduler with missed, skipped and time-limited runs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: MCP servers in CLI sessions, and the browser

**Files:**
- Modify: `packages/daemon/src/providers/types.ts`, `packages/daemon/src/providers/claude.ts`, `packages/daemon/src/providers/codex-parse.ts`, `packages/daemon/src/runner.ts` (one line), `packages/daemon/package.json` (npm adds `@playwright/mcp`)
- Replace: `packages/daemon/src/providers/codex.ts`, `packages/daemon/src/providers/claude.real.test.ts`, `packages/daemon/src/providers/codex.real.test.ts`
- Create: `packages/daemon/src/browser.ts`
- Test: `packages/daemon/src/providers/claude.test.ts`, `packages/daemon/src/providers/codex-parse.test.ts`, `packages/daemon/src/browser.test.ts`

**Interfaces:**
- Consumes: plan 1A's adapters and `StoredBot`.
- Produces:
  - `providers/types.ts`:
    - `type McpServer`: `{ name, url, headers, autoAllow }` for HTTP servers, or `{ name, command, args, env, autoAllow }` for programs the CLI starts.
    - `StartOptions` gains `mcpServers: McpServer[]`.
  - `claude.ts`:
    - `claudeMcpConfig(servers)`.
    - `claudeArgs` adds `--mcp-config <json>` when there are servers. Each autoAllow server adds `mcp__<name>` in front of the saved rules in `--allowedTools`.
  - `codex.ts`:
    - `CODEX_FEATURES_OFF`.
    - `codexConfig` adds `features` and `mcp_servers`.
    - `CodexSession` answers elicitations. An autoAllow server's tool call is accepted at once. Any other server's tool call becomes an approval, answered `{ action }`. Anything else is declined.
  - `codex-parse.ts`: `mapCodexElicitation(id, params)`. Its approval rule is `codex-mcp:<server>.<tool>`.
  - `browser.ts`:
    - `findBrowser(exists?, env?): { channel: "chrome" | "msedge"; path }` and `browserProfile(bot)`.
    - `browserServer(bot, env?): McpServer`, whose `autoAllow` is `bot.autonomy !== "ask"`. It adds `--headless` when `TJOMMIE_BROWSER_HEADLESS=1`.
    - `showBrowser(bot)`.

- [ ] **Step 1: Add Playwright MCP**

Run: `npm install @playwright/mcp@^0.0.82 -w @tjommie/daemon`
Expected: `packages/daemon/package.json` lists `"@playwright/mcp": "^0.0.82"` under dependencies. It downloads no browsers; Tjommie uses the installed Chrome or Edge.

- [ ] **Step 2: Write the failing tests**

In `packages/daemon/src/providers/claude.test.ts`:
- Add `mcpServers: [],` to the `base` options, after `env: {},`.
- Add this test before the "argument round trip" test:

```ts
  it("attaches MCP servers, and allows the tools of the ones marked autoAllow", () => {
    const args = claudeArgs({
      ...base,
      allowRules: ["Write"],
      mcpServers: [
        { name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: "Bearer t0k" }, autoAllow: true },
        { name: "browser", command: "C:\\node.exe", args: ["cli.js", "--headless"], env: {}, autoAllow: false },
      ],
    });
    expect(JSON.parse(args[args.indexOf("--mcp-config") + 1])).toEqual({
      mcpServers: {
        tjommie: { type: "http", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: "Bearer t0k" } },
        browser: { type: "stdio", command: "C:\\node.exe", args: ["cli.js", "--headless"], env: {} },
      },
    });
    expect(args.slice(-3)).toEqual(["--allowedTools", "mcp__tjommie", "Write"]);
  });
```

In `packages/daemon/src/providers/codex-parse.test.ts`:
- Change the two imports to:

```ts
import { CODEX_FEATURES_OFF, codexConfig, codexPolicy } from "./codex.ts";
import { mapCodexElicitation, mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
```

- After the `FILE_APPROVAL` line, add this fixture from the spike 5 run:

```ts
const ELICITATION = parse( // the log cut the line off after "requestedSchema"; its end is as in the run's other requests
  '{"method":"mcpServer/elicitation/request","id":0,"params":{"threadId":"01a0d957-abb2-79b3-a19e-4293c113bea1","turnId":"01a0d957-adfa-7f20-8558-a1eb40e165f2","serverName":"browser","mode":"form","_meta":{"codex_approval_kind":"mcp_tool_call","persist":["session","always"],"tool_description":"Navigate to a URL","tool_params":{"url":"https://example.com"},"tool_params_display":[{"name":"url","value":"https://example.com","display_name":"url"}]},"message":"Allow the browser MCP server to run tool \\"browser_navigate\\"?","requestedSchema":{"type":"object","properties":{}}}}',
);
```

- At the end of the `mapCodexRequest` describe block, add:

```ts
  it("turns the question before an MCP tool call into a card with a reusable rule", () => {
    expect(mapCodexElicitation(ELICITATION.id, ELICITATION.params)).toEqual({
      type: "approval",
      id: "0",
      tool: "browser.browser_navigate",
      summary: "Navigate to a URL",
      detail: JSON.stringify({ url: "https://example.com" }, null, 2),
      rule: "codex-mcp:browser.browser_navigate",
    });
  });
```

- In the `codex policy and config` block, add `mcpServers: []` to `options`, after `env: {}`.
- Replace the test "opens the network and extra folders to the sandbox, unelevated on Windows" with these two:

```ts
  it("opens the network and extra folders to the sandbox, unelevated on Windows, with Codex's own extras off", () => {
    const common = { sandbox_workspace_write: { network_access: true, writable_roots: ["C:\\home"] }, features: CODEX_FEATURES_OFF, mcp_servers: {} };
    expect(codexConfig(options, "win32")).toEqual({ ...common, windows: { sandbox: "unelevated" } });
    expect(codexConfig(options, "darwin")).toEqual(common);
    expect(CODEX_FEATURES_OFF).toMatchObject({ apps: false, browser_use: false, computer_use: false, plugins: false, memories: false });
  });

  it("attaches Tjommie's MCP servers to the thread", () => {
    const config = codexConfig({
      ...options,
      mcpServers: [
        { name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: "Bearer t0k" }, autoAllow: true },
        { name: "browser", command: "C:\\node.exe", args: ["cli.js"], env: { ELECTRON_RUN_AS_NODE: "1" }, autoAllow: true },
      ],
    });
    expect(config.mcp_servers).toEqual({
      tjommie: { url: "http://127.0.0.1:47100/mcp", http_headers: { Authorization: "Bearer t0k" } },
      browser: { command: "C:\\node.exe", args: ["cli.js"], env: { ELECTRON_RUN_AS_NODE: "1" } },
    });
  });
```

Create `packages/daemon/src/browser.test.ts`:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { StoredBot } from "./bots.ts";
import { browserServer, findBrowser } from "./browser.ts";

const env = { ProgramFiles: "C:\\Program Files", "ProgramFiles(x86)": "C:\\Program Files (x86)", LOCALAPPDATA: "C:\\Users\\Thabo Mokoena\\AppData\\Local" };
const bot = { homeFolder: "C:\\Users\\Thabo Mokoena\\AppData\\Local\\Tjommie\\bots\\b1", autonomy: "folder" } as StoredBot;

describe("browser", () => {
  it("uses Chrome when it's installed, otherwise Edge", () => {
    expect(findBrowser((path) => path === "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", env)).toEqual({
      channel: "chrome",
      path: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    });
    expect(findBrowser(() => false, env)).toEqual({ channel: "msedge", path: "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe" });
  });

  it("runs the bundled Playwright MCP on the bot's own profile, asking first only in Always ask", () => {
    const server = browserServer(bot, {});
    expect(server).toMatchObject({ name: "browser", command: process.execPath, autoAllow: true });
    if (!("command" in server)) throw new Error("expected a program");
    expect(existsSync(server.args[0])).toBe(true);
    expect(server.args.slice(1, 5)).toEqual(["--user-data-dir", join(bot.homeFolder, "chrome-profile"), "--output-dir", join(bot.homeFolder, "browser-files")]);
    expect(server.args).not.toContain("--headless");
    expect(browserServer(bot, { TJOMMIE_BROWSER_HEADLESS: "1" })).toMatchObject({ args: expect.arrayContaining(["--headless"]) });
    expect(browserServer({ ...bot, autonomy: "ask" }, {}).autoAllow).toBe(false);
  });
});
```

- [ ] **Step 3: Run them to see them fail**

Run: `npm test`
Expected: FAIL. The tests use `mcpServers`, `CODEX_FEATURES_OFF`, `mapCodexElicitation` and `./browser.ts`, none of which exist yet.

- [ ] **Step 4: Add MCP servers to the session options**

In `packages/daemon/src/providers/types.ts`, replace the `StartOptions` type with:

```ts
// An MCP server attached to one CLI session: Tjommie's own over HTTP, or a program the CLI starts (the browser).
// autoAllow: its tools run without asking the person.
export type McpServer =
  | { name: string; url: string; headers: Record<string, string>; autoAllow: boolean }
  | { name: string; command: string; args: string[]; env: Record<string, string>; autoAllow: boolean };

export type StartOptions = {
  cwd: string;
  extraDirs: string[];
  model: string;
  instructions: string;
  autonomy: Autonomy;
  allowRules: string[];
  resumeId: string | null;
  env: Record<string, string>;
  mcpServers: McpServer[];
};
```

In `packages/daemon/src/runner.ts`, in `#startTurn`, add `mcpServers: [],` right after `env: accountEnv(account),` (Task 8 replaces this line).

- [ ] **Step 5: Pass them to Claude**

In `packages/daemon/src/providers/claude.ts`, change the `./types.ts` import to:

```ts
import type { ApprovalAnswer, McpServer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";
```

Then replace the end of `claudeArgs`, from `"--strict-mcp-config",` through the function's closing brace, with:

```ts
    "--strict-mcp-config",
  ];
  if (options.mcpServers.length) args.push("--mcp-config", JSON.stringify(claudeMcpConfig(options.mcpServers)));
  for (const dir of options.extraDirs) args.push("--add-dir", dir);
  if (options.resumeId) args.push("--resume", options.resumeId);
  const allowed = [...options.mcpServers.filter((server) => server.autoAllow).map((server) => `mcp__${server.name}`), ...options.allowRules];
  if (allowed.length) args.push("--allowedTools", ...allowed); // takes several values, so it goes last
  return args;
}

export function claudeMcpConfig(servers: McpServer[]): { mcpServers: Record<string, unknown> } {
  return {
    mcpServers: Object.fromEntries(
      servers.map((s) => [s.name, "url" in s ? { type: "http", url: s.url, headers: s.headers } : { type: "stdio", command: s.command, args: s.args, env: s.env }]),
    ),
  };
}
```

- [ ] **Step 6: Pass them to Codex, with its own extras off**

Add to the end of `packages/daemon/src/providers/codex-parse.ts`:

```ts
// Codex asks before every MCP tool call. For a server that isn't allowed outright this becomes an approval
// card. The tool's name only appears in the message text.
export function mapCodexElicitation(id: number | string, params: any): Extract<ProviderEvent, { type: "approval" }> {
  const tool = String(params.message ?? "").match(/tool "([^"]+)"/)?.[1] ?? "tool";
  return {
    type: "approval",
    id: String(id),
    tool: `${params.serverName}.${tool}`,
    summary: String(params._meta?.tool_description ?? `Use ${tool}`),
    detail: JSON.stringify(params._meta?.tool_params ?? {}, null, 2).slice(0, 4000),
    rule: `codex-mcp:${params.serverName}.${tool}`,
  };
}
```

Replace `packages/daemon/src/providers/codex.ts` with:

```ts
import type { Autonomy } from "@tjommie/protocol";
import type { CliCommand } from "../cli.ts";
import { startCodexServer, type CodexServer, type JsonRpcLines } from "./codex-rpc.ts";
import { mapCodexElicitation, mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { ApprovalAnswer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

type Policy = { approvalPolicy: "untrusted" | "on-request" | "never"; sandbox: "read-only" | "workspace-write" | "danger-full-access" };

export function codexPolicy(autonomy: Autonomy): Policy {
  if (autonomy === "ask") return { approvalPolicy: "untrusted", sandbox: "read-only" };
  if (autonomy === "folder") return { approvalPolicy: "on-request", sandbox: "workspace-write" };
  return { approvalPolicy: "never", sandbox: "danger-full-access" };
}

// Codex features that are on by default and would reach past Tjommie: ChatGPT apps, driving the person's own
// browser and desktop, plugins, and memories shared by every bot on the account. Bots get Tjommie's MCP servers only.
export const CODEX_FEATURES_OFF = {
  apps: false,
  browser_use: false,
  browser_use_external: false,
  computer_use: false,
  in_app_browser: false,
  plugins: false,
  remote_plugin: false,
  memories: false,
  tool_suggest: false,
  skill_mcp_dependency_install: false,
};

// Per-thread config overrides. The unelevated Windows sandbox needs no admin rights. Shell commands still
// can't reach the network inside it (TLS fails), so they ask; bots browse with the browser tool instead.
export function codexConfig(options: StartOptions, platform: NodeJS.Platform = process.platform): Record<string, unknown> {
  return {
    sandbox_workspace_write: { network_access: true, writable_roots: options.extraDirs },
    ...(platform === "win32" ? { windows: { sandbox: "unelevated" } } : {}),
    features: CODEX_FEATURES_OFF,
    mcp_servers: Object.fromEntries(
      options.mcpServers.map((s) => [s.name, "url" in s ? { url: s.url, http_headers: s.headers } : { command: s.command, args: s.args, env: s.env }]),
    ),
  };
}

type ThreadResponse = { thread: { id: string }; model: string };
type Pending = { rpcId: number | string; answers: "decision" | "action" }; // command and file approvals, or MCP tool calls

// Drives `codex app-server`: one process and one thread per bot session.
export class CodexSession implements ProviderSession {
  #cmd: CliCommand;
  #options: StartOptions;
  #server: CodexServer | null = null;
  #listeners: ((event: ProviderEvent) => void)[] = [];
  #approvals = new Map<string, Pending>();
  #threadId = "";
  #turnId: string | null = null;
  #model = "";

  constructor(cmd: CliCommand, options: StartOptions) {
    this.#cmd = cmd;
    this.#options = options;
  }

  onEvent(listener: (event: ProviderEvent) => void): void {
    this.#listeners.push(listener);
  }

  pid(): number | undefined {
    return this.#server?.child.pid;
  }

  async start(): Promise<void> {
    const server = await startCodexServer(this.#cmd, this.#options.env, this.#options.cwd);
    this.#server = server;
    server.child.on("exit", (code) => {
      if (this.#server !== server) return; // closed on purpose
      this.#server = null;
      this.#emit({ type: "exit", error: code === 0 || code === null ? null : `Codex exited with code ${code}` });
    });
    server.rpc.onNotification = (method, params) => {
      if (method === "turn/started") this.#turnId = params.turn.id;
      if (method === "turn/completed") this.#turnId = null;
      for (const event of mapCodexNotification(method, params, this.#model)) this.#emit(event);
    };
    server.rpc.onRequest = (id, method, params) => {
      if (method === "mcpServer/elicitation/request") return this.#onElicitation(server.rpc, id, params);
      const approval = mapCodexRequest(id, method, params);
      if (!approval) {
        server.rpc.respondError(id, -32601, "Not supported by Tjommie");
        return;
      }
      this.#approvals.set(approval.id, { rpcId: id, answers: "decision" });
      this.#emit(approval);
    };
    const params = {
      cwd: this.#options.cwd,
      model: this.#options.model || null,
      ...codexPolicy(this.#options.autonomy),
      developerInstructions: this.#options.instructions,
      config: codexConfig(this.#options),
    };
    try {
      const response = this.#options.resumeId
        ? await server.rpc.request<ThreadResponse>("thread/resume", { threadId: this.#options.resumeId, ...params })
        : await server.rpc.request<ThreadResponse>("thread/start", params);
      this.#threadId = response.thread.id;
      this.#model = response.model;
    } catch (error) {
      this.close();
      throw error;
    }
    this.#emit({ type: "session", cliSessionId: this.#threadId });
  }

  async send(text: string): Promise<void> {
    const response = await this.#rpc().request<{ turn: { id: string } }>("turn/start", { threadId: this.#threadId, input: [{ type: "text", text }] });
    this.#turnId = response.turn.id;
  }

  async steer(text: string): Promise<boolean> {
    if (!this.#turnId || !this.#server) return false;
    try {
      await this.#rpc().request("turn/steer", { threadId: this.#threadId, expectedTurnId: this.#turnId, input: [{ type: "text", text }] });
      return true;
    } catch {
      return false; // the turn ended in the meantime, so the caller queues the message
    }
  }

  async interrupt(): Promise<void> {
    if (!this.#turnId || !this.#server) return;
    await this.#rpc()
      .request("turn/interrupt", { threadId: this.#threadId, turnId: this.#turnId })
      .catch(() => {});
  }

  answer(approvalId: string, answer: ApprovalAnswer): void {
    const pending = this.#approvals.get(approvalId);
    if (pending === undefined || !this.#server) return;
    this.#approvals.delete(approvalId);
    // The daemon remembers "Always allow" per bot and answers matching requests itself, so Codex only
    // ever gets yes or no.
    const yes = answer !== "deny";
    this.#server.rpc.respond(pending.rpcId, pending.answers === "action" ? { action: yes ? "accept" : "decline" } : { decision: yes ? "accept" : "decline" });
  }

  close(): void {
    const server = this.#server;
    this.#server = null;
    server?.close();
  }

  // Codex asks before every MCP tool call. Tjommie's own servers marked autoAllow are answered at once; other
  // tool calls become approval cards. Any other question from an MCP server is declined.
  #onElicitation(rpc: JsonRpcLines, id: number | string, params: any): void {
    const server = this.#options.mcpServers.find((s) => s.name === params?.serverName);
    if (params?._meta?.codex_approval_kind !== "mcp_tool_call" || !server) {
      rpc.respond(id, { action: "decline" });
      return;
    }
    if (server.autoAllow) {
      rpc.respond(id, { action: "accept" });
      return;
    }
    const approval = mapCodexElicitation(id, params);
    this.#approvals.set(approval.id, { rpcId: id, answers: "action" });
    this.#emit(approval);
  }

  #emit(event: ProviderEvent): void {
    for (const listener of this.#listeners) listener(event);
  }

  #rpc(): JsonRpcLines {
    if (!this.#server) throw new Error("The Codex session isn't running");
    return this.#server.rpc;
  }
}
```

- [ ] **Step 7: Write browser.ts**

Create `packages/daemon/src/browser.ts`:

```ts
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import type { StoredBot } from "./bots.ts";
import type { McpServer } from "./providers/types.ts";

const require = createRequire(import.meta.url);

const chromePaths = (env: NodeJS.ProcessEnv) => [
  join(env.ProgramFiles ?? "C:\\Program Files", "Google", "Chrome", "Application", "chrome.exe"),
  join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Google", "Chrome", "Application", "chrome.exe"),
  join(env.LOCALAPPDATA ?? "", "Google", "Chrome", "Application", "chrome.exe"),
];
const edgePath = (env: NodeJS.ProcessEnv) => join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft", "Edge", "Application", "msedge.exe");

// Chrome if it's installed, otherwise Edge, which every Windows PC has.
export function findBrowser(exists: (path: string) => boolean = existsSync, env: NodeJS.ProcessEnv = process.env): { channel: "chrome" | "msedge"; path: string } {
  const chrome = chromePaths(env).find(exists);
  return chrome ? { channel: "chrome", path: chrome } : { channel: "msedge", path: edgePath(env) };
}

export const browserProfile = (bot: StoredBot): string => join(bot.homeFolder, "chrome-profile");

// Playwright MCP (bundled with Tjommie), started by the bot's CLI. It opens a real browser window on the bot's
// own profile, so sites the person signs in to there stay signed in.
export function browserServer(bot: StoredBot, env: NodeJS.ProcessEnv = process.env): McpServer {
  const cli = join(dirname(require.resolve("@playwright/mcp/package.json")), "cli.js");
  const args = [cli, "--user-data-dir", browserProfile(bot), "--output-dir", join(bot.homeFolder, "browser-files"), "--browser", findBrowser().channel];
  if (env.TJOMMIE_BROWSER_HEADLESS === "1") args.push("--headless"); // tests only
  return {
    name: "browser",
    command: process.execPath,
    args,
    env: process.versions.electron ? { ELECTRON_RUN_AS_NODE: "1" } : {}, // inside Electron, run the script as plain Node
    autoAllow: bot.autonomy !== "ask",
  };
}

// "Show window": opens the bot's browser profile. If the bot's browser is already open, the browser opens another
// window of it. ponytail: while a window opened here stays open, the bot's own browser can't start ("already in
// use") and the bot asks the person to close it; a daemon-owned browser (Phase 4, for the live view) removes that.
export function showBrowser(bot: StoredBot): void {
  const child = spawn(findBrowser().path, [`--user-data-dir=${browserProfile(bot)}`, "--no-first-run", "--no-default-browser-check"], { detached: true, stdio: "ignore" });
  child.on("error", () => {}); // no browser found: nothing to show
  child.unref();
}
```

Plan 1D packages the desktop app. `@playwright/mcp`, `playwright` and `playwright-core` must stay outside the asar archive there, because the CLI runs `cli.js` as a separate process.

- [ ] **Step 8: Add real browser runs to the adapter tests**

Replace `packages/daemon/src/providers/claude.real.test.ts` with:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { StoredBot } from "../bots.ts";
import { browserServer } from "../browser.ts";
import { resolveCli } from "../cli.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { ClaudeSession } from "./claude.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}, cwd = tempRoot()) {
  const claude = new ClaudeSession(resolveCli("claude")!, {
    cwd,
    extraDirs: [],
    model: "haiku",
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("claude")),
    mcpServers: [],
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  claude.onEvent((event) => events.push(event));
  return { claude, events, cwd };
}

// A headless browser on a profile in `home`, as the daemon attaches it to a bot with the browser on.
const browser = (home: string, autonomy: StoredBot["autonomy"]) => browserServer({ homeFolder: home, autonomy } as StoredBot, { TJOMMIE_BROWSER_HEADLESS: "1" });

const text = (events: ProviderEvent[]) => events.map((e) => (e.type === "text" ? e.delta : "")).join("");

describe("ClaudeSession against the real CLI", () => {
  it("answers, reports its session id and the turn's cost", async () => {
    const { claude, events } = session();
    await claude.start();
    await claude.send("Reply with exactly: PONG");
    expect(await waitFor(() => firstOf(events, "turn_done"))).toEqual({ type: "turn_done", ok: true, error: null });
    expect(text(events)).toContain("PONG");
    expect(firstOf(events, "session")?.cliSessionId).toMatch(/^[0-9a-f-]{36}$/);
    expect(firstOf(events, "usage")?.costUsd).toBeGreaterThan(0);
    claude.close();
  });

  it("asks before writing in Always ask mode, then writes once allowed", async () => {
    const { claude, events, cwd } = session({ autonomy: "ask" });
    await claude.start();
    await claude.send("Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const approval = await waitFor(() => firstOf(events, "approval"));
    expect(approval).toMatchObject({ tool: "Write", rule: "Write" });
    claude.answer(approval.id, "once");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(existsSync(join(cwd, "hello.txt"))).toBe(true);
    claude.close();
  });

  it("stops a running turn", async () => {
    const { claude, events } = session({ autonomy: "full" });
    await claude.start();
    await claude.send(`Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('late'), 30000)". Then tell me what it printed.`);
    await waitFor(() => firstOf(events, "tool"));
    await claude.interrupt();
    expect(await waitFor(() => firstOf(events, "turn_done"), 20_000)).toMatchObject({ ok: false });
    claude.close();
  });

  it("browses in its own profile through the bundled Playwright MCP, without asking", async () => {
    const cwd = tempRoot();
    const { claude, events } = session({ mcpServers: [browser(cwd, "folder")] }, cwd);
    await claude.start();
    await claude.send("Use the browser tools to open https://example.com, then reply with the page title only.");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(text(events)).toContain("Example Domain");
    expect(firstOf(events, "approval")).toBeUndefined();
    expect(existsSync(join(cwd, "chrome-profile", "Default"))).toBe(true);
    claude.close();
  });
});
```

Replace `packages/daemon/src/providers/codex.real.test.ts` with:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { StoredBot } from "../bots.ts";
import { browserServer } from "../browser.ts";
import { resolveCli } from "../cli.ts";
import { dataPaths } from "../paths.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { CodexSession } from "./codex.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}, cwd = tempRoot()) {
  const codex = new CodexSession(resolveCli("codex")!, {
    cwd,
    extraDirs: [],
    model: "", // the account's default model
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("codex")),
    mcpServers: [],
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  codex.onEvent((event) => events.push(event));
  return { codex, events, cwd };
}

// A headless browser on a profile in `home`, as the daemon attaches it to a bot with the browser on.
const browser = (home: string, autonomy: StoredBot["autonomy"]) => browserServer({ homeFolder: home, autonomy } as StoredBot, { TJOMMIE_BROWSER_HEADLESS: "1" });

const text = (events: ProviderEvent[]) => events.map((e) => (e.type === "text" ? e.delta : "")).join("");

describe("CodexSession against the real CLI", () => {
  it("answers and reports its thread id and token usage", async () => {
    const { codex, events } = session();
    await codex.start();
    await codex.send("Reply with exactly: PONG");
    expect(await waitFor(() => firstOf(events, "turn_done"))).toEqual({ type: "turn_done", ok: true, error: null });
    expect(text(events)).toContain("PONG");
    expect(firstOf(events, "session")?.cliSessionId).toBeTruthy();
    expect(firstOf(events, "usage")?.output).toBeGreaterThan(0);
    codex.close();
  });

  it("adds a steered message to the running turn", async () => {
    const { codex, events } = session();
    await codex.start();
    await codex.send(`Run this shell command and wait for it: node -e "setTimeout(() => console.log('first'), 8000)". Then tell me what it printed.`);
    await waitFor(() => firstOf(events, "tool"));
    expect(await codex.steer("Also end your final answer with the word BANANA.")).toBe(true);
    await waitFor(() => firstOf(events, "turn_done"));
    expect(text(events)).toContain("BANANA");
    codex.close();
  });

  it("asks before writing outside its folder, and a denial keeps the file from being written", async () => {
    const outside = join(dataPaths().root, `tjommie-outside-${Date.now()}.txt`); // not under %TEMP%, which the sandbox allows
    const { codex, events } = session();
    await codex.start();
    await codex.send(`Write the text hi to the file ${outside} with a shell command.`);
    const approval = await waitFor(() => firstOf(events, "approval"));
    codex.answer(approval.id, "deny");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(existsSync(outside)).toBe(false);
    codex.close();
  });

  it("asks before each browser tool of a server that isn't allowed outright, then browses in its own profile", async () => {
    // The thread runs in Own folder (workspace-write), where spike 5 measured Playwright; only the browser server asks.
    const cwd = tempRoot();
    const { codex, events } = session({ mcpServers: [browser(cwd, "ask")] }, cwd);
    await codex.start();
    await codex.send("Use the browser MCP tools, not the shell or web search, to open https://example.com. Reply with the page title only.");
    const answered = new Set<string>();
    await waitFor(() => {
      for (const event of events) {
        if (event.type === "approval" && !answered.has(event.id)) {
          answered.add(event.id);
          codex.answer(event.id, "once");
        }
      }
      return firstOf(events, "turn_done");
    });
    const approvals = events.filter((event) => event.type === "approval");
    expect(approvals.length).toBeGreaterThan(0);
    for (const approval of approvals) expect(approval).toMatchObject({ tool: expect.stringMatching(/^browser\./) });
    expect(text(events)).toContain("Example Domain");
    expect(existsSync(join(cwd, "chrome-profile", "Default"))).toBe(true);
    codex.close();
  });
});
```

- [ ] **Step 9: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including the new Claude, Codex and browser tests.

Run: `npm run typecheck`
Expected: no errors.

Run: `npx vitest run --mode real packages/daemon/src/providers/claude.real.test.ts packages/daemon/src/providers/codex.real.test.ts -t "browse"`
Expected: 2 tests PASS. Each opens example.com headless in a profile under the test folder.

- [ ] **Step 10: Commit**

```bash
git add package-lock.json packages/daemon/package.json packages/daemon/src/providers packages/daemon/src/browser.ts packages/daemon/src/browser.test.ts packages/daemon/src/runner.ts
git commit -m "Attach MCP servers to CLI sessions and give bots their own browser

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The runner: turn destinations, delivery and hooks

**Files:**
- Replace: `packages/daemon/src/runner.ts`, `packages/daemon/src/runner.real.test.ts`
- Test: `packages/daemon/src/runner.test.ts` (add a test)

**Interfaces:**
- Consumes: the queue (Task 3), `McpServer` (Task 7).
- Produces (changes to plan 1A's `Runner`):
  - `type TurnResult = { outcome: "done" | "failed" | "interrupted"; text: string | null; textMessageId: string | null; error: string | null }`. `text` is the turn's last finished text.
  - `type RunnerHooks = { mcpServers?(bot): McpServer[]; sessionClosed?(botId): void; turnStarted?(item): void; turnEnded?(item, result): void }`. The constructor takes `hooks?: RunnerHooks`.
  - `deliver(work: NewWork): WorkItem`: queues work, then starts it or steers it in.
  - `currentWork(botId): WorkItem | null`.
  - `stopItem(itemId, notice)`: stops the turn if it's still on that item, and shows `notice` instead of "Stopped.".
  - `submit` starts a new chain for every message from you.
  - A turn's text, activity, "Stopped." and errors go to `item.conversationId`. Approval cards and limit notices stay in the bot's own chat, where the notification leads.
  - Only work that `nextSteerable` returns is steered into a running turn.
  - After a restart, each interrupted item goes to `turnEnded` as failed, with the error "Tjommie restarted while this was running.".
  - A hook that throws is logged and never leaves a bot stuck.

- [ ] **Step 1: Write the failing test**

In `packages/daemon/src/runner.test.ts`, change the imports to:

```ts
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb } from "./db.ts";
import { enqueue, setStatus } from "./queue.ts";
import { Runner, activityBody, botStateOf, copyIntoInbox, type TurnResult } from "./runner.ts";
import { tempRoot } from "./testing.ts";
```

and add at the end of the file:

```ts
describe("Runner.start", () => {
  it("reports a turn a restart cut off to the hooks, and says so where the turn was writing", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const bot = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" });
    const item = enqueue(db, { botId: bot.id, source: "room", text: "@Writer hi", conversationId: "team" });
    setStatus(db, item.id, "running");
    const ended: [string, TurnResult][] = [];
    const runner = new Runner({ db, broadcast: () => {}, isSignedIn: () => true, hooks: { turnEnded: (work, result) => ended.push([work.id, result]) } });
    runner.start();
    expect(ended).toEqual([[item.id, { outcome: "failed", text: null, textMessageId: null, error: "Tjommie restarted while this was running." }]]);
    expect(history(db, "team", undefined, 1)[0]).toMatchObject({ author: "system", kind: "error", body: "Tjommie restarted while this was running." });
    runner.shutdown();
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `TurnResult` and `hooks` don't exist, and the notice goes to the bot's chat instead of the room.

- [ ] **Step 3: Replace runner.ts**

Replace `packages/daemon/src/runner.ts` with:

```ts
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { basename, join } from "node:path";
import type { Approval, ApprovalDecision, Bot, BotState, Message } from "@tjommie/protocol";
import { allowRules, createApproval, expirePendingApprovals, resolveApproval } from "./approvals.ts";
import { botPrompt, getStoredBot, listStoredBots, setLimitedUntil, type StoredBot } from "./bots.ts";
import { addMessage, buildRecap, getMessage, history, updateMessage } from "./chat.ts";
import { resolveCli } from "./cli.ts";
import { newId, nowIso, type Db } from "./db.ts";
import { ClaudeSession } from "./providers/claude.ts";
import { CodexSession } from "./providers/codex.ts";
import type { McpServer, ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, lastItemForMessage, newChain, nextQueued, nextSteerable, recoverAfterRestart, setStatus, type NewWork, type WorkItem } from "./queue.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { accountEnv, accountFor } from "./setup.ts";

export type SessionFactory = (bot: StoredBot, options: StartOptions) => ProviderSession;

export const cliSessionFactory: SessionFactory = (bot, options) => {
  const cmd = resolveCli(bot.provider);
  if (!cmd) throw new RpcError("not_installed", `${bot.provider === "claude" ? "Claude Code" : "Codex"} is not installed`);
  return bot.provider === "claude" ? new ClaudeSession(cmd, options) : new CodexSession(cmd, options);
};

// How a turn ended. `text` is its last finished text: the answer, when the turn was answering another bot.
export type TurnResult = { outcome: "done" | "failed" | "interrupted"; text: string | null; textMessageId: string | null; error: string | null };

// How the rest of the daemon joins in: MCP servers for each new CLI session, and hooks around turns.
export type RunnerHooks = {
  mcpServers?: (bot: StoredBot) => McpServer[];
  sessionClosed?: (botId: string) => void;
  turnStarted?: (item: WorkItem) => void;
  turnEnded?: (item: WorkItem, result: TurnResult) => void;
};

const IDLE_SHUTDOWN_MS = 10 * 60_000;
const RECAP_MESSAGES = 20;
const UNKNOWN_RESET_MS = 60 * 60_000;
const MAX_TIMER_MS = 2 ** 31 - 1;
const TURN_EVENTS = new Set<ProviderEvent["type"]>(["text", "text_end", "tool", "tool_done", "approval"]);
const RESTARTED = "Tjommie restarted while this was running.";

type ActivityItem = { id: string; name: string; summary: string; status: "running" | "done" | "failed" };

// What the runner knows about a bot that has (or recently had) a CLI session.
type Live = {
  session: ProviderSession | null;
  fresh: boolean; // started without resuming
  sawSession: boolean; // the CLI reported its session id since it started
  busy: boolean; // a turn is running
  sent: boolean; // the current turn's first message has reached the CLI, so more can be steered in
  steering: boolean;
  item: WorkItem | null; // null while the CLI runs a turn it started by itself
  text: { id: string; body: string } | null;
  lastText: { id: string; body: string } | null; // the turn's latest finished text
  activity: { id: string; items: ActivityItem[] } | null;
  stopRequested: boolean;
  stopNotice: string | null; // shown instead of "Stopped." (a routine's time limit)
  limitHit: boolean;
  lastResetsAt: string | null;
  reload: boolean;
  approvals: Map<string, string>; // Tjommie approval id -> the provider's own request id
  idleTimer: NodeJS.Timeout | null;
  limitTimer: NodeJS.Timeout | null;
};

export function botStateOf(
  s: { signedIn: boolean; paused: boolean; limitedUntil: string | null; waiting: boolean; busy: boolean },
  now: string,
): BotState {
  if (!s.signedIn) return "needs_setup";
  if (s.paused) return "paused";
  if (s.limitedUntil !== null && s.limitedUntil > now) return "limited";
  if (s.waiting) return "waiting";
  if (s.busy) return "working";
  return "idle";
}

export function activityBody(items: { summary: string }[]): string {
  return `${items.length} ${items.length === 1 ? "step" : "steps"}: ${items.at(-1)?.summary ?? ""}`.slice(0, 160);
}

// Copies a file into the bot's inbox, never overwriting an earlier one.
export function copyIntoInbox(homeFolder: string, file: string): string {
  const inbox = join(homeFolder, "inbox");
  mkdirSync(inbox, { recursive: true });
  let name = basename(file);
  if (existsSync(join(inbox, name))) name = `${Date.now()}-${name}`;
  copyFileSync(file, join(inbox, name));
  return name;
}

export class Runner {
  #db: Db;
  #broadcast: Broadcast;
  #isSignedIn: (provider: Bot["provider"]) => boolean;
  #createSession: SessionFactory;
  #hooks: RunnerHooks;
  #live = new Map<string, Live>();

  constructor(deps: { db: Db; broadcast: Broadcast; isSignedIn: (provider: Bot["provider"]) => boolean; createSession?: SessionFactory; hooks?: RunnerHooks }) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#isSignedIn = deps.isSignedIn;
    this.#createSession = deps.createSession ?? cliSessionFactory;
    this.#hooks = deps.hooks ?? {};
  }

  // Call once at startup: settle work a restart cut off, then carry on with whatever is queued.
  start(): void {
    for (const item of recoverAfterRestart(this.#db)) {
      if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "interrupted" }));
      this.#notice(item.conversationId, "error", RESTARTED, item.messageId ? { retryMessageId: item.messageId } : null);
      this.#safely(() => this.#hooks.turnEnded?.(item, { outcome: "failed", text: null, textMessageId: null, error: RESTARTED }));
    }
    for (const approval of expirePendingApprovals(this.#db)) this.#showApprovalStatus(approval);
    for (const bot of listStoredBots(this.#db)) {
      if (bot.limitedUntil) this.#armLimitTimer(bot.id, bot.limitedUntil);
      this.pump(bot.id);
    }
  }

  toBot(bot: StoredBot): Bot {
    const live = this.#live.get(bot.id);
    const state = botStateOf(
      { signedIn: this.#isSignedIn(bot.provider), paused: bot.paused, limitedUntil: bot.limitedUntil, waiting: (live?.approvals.size ?? 0) > 0, busy: live?.busy ?? false },
      nowIso(),
    );
    return { ...bot, state };
  }

  processId(botId: string): number | undefined {
    return this.#live.get(botId)?.session?.pid();
  }

  // The work the bot's running turn is doing; null when idle or in a turn the CLI started by itself.
  currentWork(botId: string): WorkItem | null {
    const live = this.#live.get(botId);
    return live?.busy ? live.item : null;
  }

  submit(botId: string, text: string, files: string[]): Message {
    const bot = getStoredBot(this.#db, botId);
    const attached = files.map((file) => copyIntoInbox(bot.homeFolder, file));
    const message = addMessage(this.#db, { conversationId: botId, author: "you", kind: "text", body: text, data: attached.length ? { files: attached } : null, status: "queued" });
    this.#emitMessage(message, "message.added");
    const prompt = attached.length ? `${text}\n\n[Attached files, now in your inbox folder: ${attached.join(", ")}]` : text;
    this.deliver({ botId, source: "you", text: prompt, messageId: message.id, chainId: newChain(this.#db) });
    return getMessage(this.#db, message.id);
  }

  // Queues work for a bot and starts it, or steers it into the running turn when the rules allow.
  deliver(work: NewWork): WorkItem {
    const item = enqueue(this.#db, work);
    const live = this.#liveFor(work.botId);
    if (!live.busy) this.pump(work.botId);
    else if (live.sent) void this.#steerQueued(work.botId); // otherwise the turn steers it in as soon as its first message is sent
    return item;
  }

  retry(messageId: string): Message {
    const previous = lastItemForMessage(this.#db, messageId);
    if (!previous) throw new RpcError("not_found", "There's nothing to retry for this message");
    this.#emitMessage(updateMessage(this.#db, messageId, { status: "queued" }));
    this.deliver({ botId: previous.botId, source: previous.source, text: previous.text, messageId, conversationId: previous.conversationId, chainId: newChain(this.#db) });
    return getMessage(this.#db, messageId);
  }

  async stop(botId: string): Promise<void> {
    const live = this.#live.get(botId);
    if (!live?.busy || !live.session) return;
    live.stopRequested = true;
    await live.session.interrupt();
  }

  // Stops the turn working on this item, if it still is. `notice` replaces "Stopped.".
  stopItem(itemId: string, notice: string): void {
    for (const [botId, live] of this.#live) {
      if (!live.busy || live.item?.id !== itemId) continue;
      live.stopNotice = notice;
      void this.stop(botId);
    }
  }

  answerApproval(approvalId: string, decision: ApprovalDecision): Approval {
    const approval = resolveApproval(this.#db, approvalId, decision);
    const live = this.#live.get(approval.botId);
    const providerId = live?.approvals.get(approvalId);
    if (live && providerId !== undefined) {
      live.approvals.delete(approvalId);
      live.session?.answer(providerId, decision);
    }
    this.#showApprovalStatus(approval);
    this.#emitBot(approval.botId);
    return approval;
  }

  // Settings changed. The next turn restarts the CLI with the new settings. The conversation carries on
  // unless the provider changed.
  botChanged(botId: string, providerChanged: boolean): void {
    if (providerChanged) this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(botId);
    const live = this.#live.get(botId);
    if (!live?.session) return;
    if (live.busy) live.reload = true;
    else this.#closeSession(botId, live);
  }

  // Call before deleting a bot.
  forget(botId: string): void {
    const live = this.#live.get(botId);
    if (!live) return;
    if (live.idleTimer) clearTimeout(live.idleTimer);
    if (live.limitTimer) clearTimeout(live.limitTimer);
    this.#closeSession(botId, live);
    this.#live.delete(botId);
  }

  pump(botId: string): void {
    const live = this.#liveFor(botId);
    if (live.busy) return;
    let bot: StoredBot;
    try {
      bot = getStoredBot(this.#db, botId);
    } catch {
      return; // deleted
    }
    if (this.toBot(bot).state !== "idle") {
      this.#emitBot(botId);
      return;
    }
    const item = nextQueued(this.#db, botId);
    if (!item) {
      this.#armIdle(botId, live);
      return;
    }
    live.busy = true; // set before any await, so a second pump can't start another turn
    live.sent = false;
    live.item = item;
    live.stopRequested = false;
    live.stopNotice = null;
    live.limitHit = false;
    live.lastText = null;
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    setStatus(this.#db, item.id, "running");
    if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "sent" }));
    this.#emitBot(botId);
    this.#safely(() => this.#hooks.turnStarted?.(item));
    this.#startTurn(bot, item).catch((error: Error) => this.#finishTurn(botId, false, error.message));
  }

  pumpAll(): void {
    for (const bot of listStoredBots(this.#db)) this.pump(bot.id);
  }

  shutdown(): void {
    for (const botId of [...this.#live.keys()]) this.forget(botId);
  }

  #liveFor(botId: string): Live {
    let live = this.#live.get(botId);
    if (!live) {
      live = {
        session: null,
        fresh: false,
        sawSession: false,
        busy: false,
        sent: false,
        steering: false,
        item: null,
        text: null,
        lastText: null,
        activity: null,
        stopRequested: false,
        stopNotice: null,
        limitHit: false,
        lastResetsAt: null,
        reload: false,
        approvals: new Map(),
        idleTimer: null,
        limitTimer: null,
      };
      this.#live.set(botId, live);
    }
    return live;
  }

  // Where the running turn's output goes: its work item's conversation, or the bot's chat.
  #dest(botId: string, live: Live): string {
    return live.item?.conversationId ?? botId;
  }

  async #startTurn(bot: StoredBot, item: WorkItem): Promise<void> {
    const live = this.#liveFor(bot.id);
    let text = item.text;
    let session = live.session;
    if (!session) {
      const account = accountFor(this.#db, bot.provider);
      const saved = this.#db.prepare("SELECT provider, account_id, cli_session_id FROM sessions WHERE bot_id = ?").get(bot.id) as
        | { provider: string; account_id: string; cli_session_id: string }
        | undefined;
      const resumeId = saved && saved.provider === bot.provider && saved.account_id === account.id ? saved.cli_session_id : null;
      const created = this.#createSession(bot, {
        cwd: bot.projectFolder ?? bot.homeFolder,
        extraDirs: bot.projectFolder ? [bot.homeFolder] : [],
        model: bot.model,
        instructions: botPrompt(bot),
        autonomy: bot.autonomy,
        allowRules: allowRules(this.#db, bot.id),
        resumeId,
        env: accountEnv(account),
        mcpServers: this.#hooks.mcpServers?.(bot) ?? [],
      });
      created.onEvent((event) => this.#onEvent(bot.id, created, event));
      live.session = created;
      live.fresh = resumeId === null;
      live.sawSession = false;
      try {
        await created.start();
      } catch (error) {
        this.#closeSession(bot.id, live);
        if (resumeId === null) throw error;
        this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(bot.id); // couldn't resume: start fresh with a recap
        return this.#startTurn(bot, item);
      }
      if (live.fresh) {
        const recap = buildRecap(history(this.#db, bot.id, item.messageId ?? undefined, RECAP_MESSAGES), bot.name);
        if (recap) text = `${recap}\n\n${text}`;
      }
      session = created;
    }
    await session.send(text);
    live.sent = true;
    await this.#steerQueued(bot.id); // anything sent while the turn was starting goes in now, in order
  }

  // Steers waiting work into the running turn, oldest first: your messages and replies to the bot's own
  // requests, when they belong to the conversation the turn writes to. One loop at a time keeps the order.
  async #steerQueued(botId: string): Promise<void> {
    const live = this.#liveFor(botId);
    if (live.steering) return;
    live.steering = true;
    try {
      const next = () => nextSteerable(this.#db, botId, this.#dest(botId, live));
      for (let item = next(); item && live.busy && live.sent; item = next()) {
        const session = live.session;
        if (!session) break;
        setStatus(this.#db, item.id, "done"); // claim it first, so a turn that ends meanwhile can't pick it up too
        if (!(await session.steer(item.text).catch(() => false))) {
          setStatus(this.#db, item.id, "queued"); // it runs as the next turn instead
          break;
        }
        if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "sent" }));
      }
    } finally {
      live.steering = false;
    }
    if (!live.busy) this.pump(botId);
  }

  #onEvent(botId: string, session: ProviderSession, event: ProviderEvent): void {
    const live = this.#live.get(botId);
    if (!live || live.session !== session) return; // a session we closed or replaced
    if (!live.busy && TURN_EVENTS.has(event.type)) {
      // The CLI started a turn by itself, for example after a background task finished.
      live.busy = true;
      live.sent = true;
      live.item = null;
      this.#emitBot(botId);
    }
    switch (event.type) {
      case "session":
        live.sawSession = true;
        this.#saveSession(botId, event.cliSessionId);
        return;
      case "text":
        return this.#appendText(botId, live, event.delta);
      case "text_end":
        return this.#flushText(live, "done");
      case "tool":
        return this.#addTool(botId, live, event);
      case "tool_done":
        return this.#toolDone(live, event.id, event.ok);
      case "approval":
        return this.#onApproval(botId, live, event);
      case "usage":
        return this.#recordUsage(botId, event);
      case "limit":
        return this.#onLimit(botId, live, event);
      case "turn_done":
        return this.#finishTurn(botId, event.ok, event.error);
      case "exit":
        return this.#onExit(botId, live, event.error);
    }
  }

  #saveSession(botId: string, cliSessionId: string): void {
    const bot = getStoredBot(this.#db, botId);
    this.#db
      .prepare(
        `INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(bot_id) DO UPDATE SET provider = excluded.provider, account_id = excluded.account_id,
           cli_session_id = excluded.cli_session_id, updated_at = excluded.updated_at`,
      )
      .run(botId, bot.provider, accountFor(this.#db, bot.provider).id, cliSessionId, nowIso());
  }

  #appendText(botId: string, live: Live, delta: string): void {
    const conversationId = this.#dest(botId, live);
    if (!live.text) {
      if (live.activity) this.#closeActivity(live);
      const message = addMessage(this.#db, { conversationId, author: botId, kind: "text", body: "", status: "streaming" });
      live.text = { id: message.id, body: "" };
      this.#emitMessage(message, "message.added");
    }
    live.text.body += delta;
    this.#broadcast("message.delta", { id: live.text.id, conversationId, delta });
  }

  #flushText(live: Live, status: Message["status"]): void {
    if (!live.text) return;
    this.#emitMessage(updateMessage(this.#db, live.text.id, { body: live.text.body, status }));
    if (status === "done" && live.text.body.trim()) live.lastText = live.text;
    live.text = null;
  }

  #addTool(botId: string, live: Live, event: { id: string; name: string; summary: string }): void {
    this.#flushText(live, "done");
    const item: ActivityItem = { id: event.id, name: event.name, summary: event.summary, status: "running" };
    if (live.activity) {
      live.activity.items.push(item);
      this.#saveActivity(live, "streaming");
      return;
    }
    const message = addMessage(this.#db, {
      conversationId: this.#dest(botId, live),
      author: botId,
      kind: "activity",
      body: activityBody([item]),
      data: { items: [item] },
      status: "streaming",
    });
    live.activity = { id: message.id, items: [item] };
    this.#emitMessage(message, "message.added");
  }

  #toolDone(live: Live, id: string, ok: boolean): void {
    const item = live.activity?.items.find((i) => i.id === id);
    if (!item) return;
    item.status = ok ? "done" : "failed";
    this.#saveActivity(live, "streaming");
  }

  #saveActivity(live: Live, status: Message["status"]): void {
    if (!live.activity) return;
    this.#emitMessage(updateMessage(this.#db, live.activity.id, { body: activityBody(live.activity.items), data: { items: live.activity.items }, status }));
  }

  #closeActivity(live: Live): void {
    this.#saveActivity(live, "done");
    live.activity = null;
  }

  // Approval cards always go to the bot's own chat, where the notification leads.
  #onApproval(botId: string, live: Live, event: { id: string; tool: string; summary: string; detail: string; rule: string | null }): void {
    if (event.rule && allowRules(this.#db, botId).includes(event.rule)) {
      live.session?.answer(event.id, "once"); // "Always allow" was chosen for this before
      return;
    }
    this.#flushText(live, "done");
    const approvalId = newId();
    const message = addMessage(this.#db, {
      conversationId: botId,
      author: botId,
      kind: "approval",
      body: event.summary,
      data: { approvalId, tool: event.tool, detail: event.detail, rule: event.rule, status: "pending" },
      status: "done",
    });
    const approval = createApproval(this.#db, { id: approvalId, botId, tool: event.tool, summary: event.summary, detail: event.detail, rule: event.rule, messageId: message.id });
    live.approvals.set(approvalId, event.id);
    this.#emitMessage(message, "message.added");
    this.#broadcast("approval.added", approval);
    this.#broadcast("notify", { botId, title: `${getStoredBot(this.#db, botId).name} is waiting for you`, body: event.summary });
    this.#emitBot(botId);
  }

  #showApprovalStatus(approval: Approval): void {
    const message = getMessage(this.#db, approval.messageId);
    this.#emitMessage(updateMessage(this.#db, message.id, { data: { ...message.data, status: approval.status } }));
    this.#broadcast("approval.resolved", approval);
  }

  #recordUsage(botId: string, e: Extract<ProviderEvent, { type: "usage" }>): void {
    const bot = getStoredBot(this.#db, botId);
    this.#db
      .prepare(
        `INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(newId(), botId, accountFor(this.#db, bot.provider).id, bot.provider, e.model, e.input, e.cachedInput, e.output, e.reasoning, e.costUsd, e.costUsd === null ? null : "reported", nowIso());
  }

  #onLimit(botId: string, live: Live, e: Extract<ProviderEvent, { type: "limit" }>): void {
    if (e.resetsAt) live.lastResetsAt = e.resetsAt;
    if (e.state !== "reached" || live.limitHit) return; // "ok" and "near" feed the Usage page in Phase 2
    live.limitHit = true;
    const until = e.resetsAt ?? live.lastResetsAt ?? new Date(Date.now() + UNKNOWN_RESET_MS).toISOString();
    setLimitedUntil(this.#db, botId, until);
    const name = getStoredBot(this.#db, botId).name;
    const when = new Date(until);
    const at = `${when.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })} on ${when.toLocaleDateString([], { weekday: "long" })}`;
    this.#notice(botId, "notice", `${name} hit its usage limit. It will pick up again at ${at}.`, null);
    this.#broadcast("notify", { botId, title: `${name} hit its usage limit`, body: `It will pick up again at ${at}.` });
    this.#armLimitTimer(botId, until);
    this.#emitBot(botId);
  }

  #armLimitTimer(botId: string, until: string): void {
    const live = this.#liveFor(botId);
    if (live.limitTimer) clearTimeout(live.limitTimer);
    const delay = Math.min(Math.max(0, Date.parse(until) - Date.now()) + 60_000, MAX_TIMER_MS);
    live.limitTimer = setTimeout(() => {
      live.limitTimer = null;
      setLimitedUntil(this.#db, botId, null);
      this.#emitBot(botId);
      this.pump(botId);
    }, delay);
    live.limitTimer.unref();
  }

  #finishTurn(botId: string, ok: boolean, error: string | null): void {
    const live = this.#live.get(botId);
    if (!live?.busy) return;
    const outcome = ok ? "done" : live.stopRequested ? "interrupted" : "failed";
    this.#flushText(live, outcome === "interrupted" ? "interrupted" : "done");
    if (live.activity) this.#closeActivity(live);
    const item = live.item;
    const conversationId = this.#dest(botId, live);
    const finished = item && !live.limitHit ? item : null;
    if (item && live.limitHit) {
      setStatus(this.#db, item.id, "queued"); // runs again once the limit resets
      if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "queued" }));
    } else if (item) {
      setStatus(this.#db, item.id, outcome);
      if (outcome === "interrupted") this.#notice(conversationId, "notice", live.stopNotice ?? "Stopped.", null);
      if (outcome === "failed") this.#notice(conversationId, "error", error ?? "Something went wrong.", item.messageId ? { retryMessageId: item.messageId } : null);
    }
    for (const approvalId of live.approvals.keys()) this.#showApprovalStatus(resolveApproval(this.#db, approvalId, "deny")); // the turn that asked is over
    live.approvals.clear();
    const result: TurnResult = { outcome, text: live.lastText?.body ?? null, textMessageId: live.lastText?.id ?? null, error };
    live.busy = false;
    live.sent = false;
    live.item = null;
    live.stopRequested = false;
    live.stopNotice = null;
    live.limitHit = false;
    live.lastText = null;
    if (live.reload) {
      live.reload = false;
      this.#closeSession(botId, live);
    }
    if (finished) this.#safely(() => this.#hooks.turnEnded?.(finished, result));
    this.#emitBot(botId);
    this.pump(botId);
  }

  #onExit(botId: string, live: Live, error: string | null): void {
    const resumeFailed = !live.fresh && !live.sawSession;
    live.session = null;
    this.#safely(() => this.#hooks.sessionClosed?.(botId));
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    if (!live.busy) return;
    if (resumeFailed && live.item) {
      // The CLI couldn't resume this session: start fresh with a recap instead.
      this.#db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(botId);
      setStatus(this.#db, live.item.id, "queued");
      live.busy = false;
      live.sent = false;
      live.item = null;
      this.pump(botId);
      return;
    }
    this.#finishTurn(botId, false, error ?? "The CLI stopped unexpectedly.");
  }

  #closeSession(botId: string, live: Live): void {
    const session = live.session;
    live.session = null; // its exit event is ignored from now on
    session?.close();
    this.#safely(() => this.#hooks.sessionClosed?.(botId));
  }

  #armIdle(botId: string, live: Live): void {
    if (!live.session || live.idleTimer) return;
    live.idleTimer = setTimeout(() => {
      live.idleTimer = null;
      if (!live.busy) this.#closeSession(botId, live);
    }, IDLE_SHUTDOWN_MS);
    live.idleTimer.unref();
  }

  #notice(conversationId: string, kind: "notice" | "error", body: string, data: Record<string, unknown> | null): void {
    this.#emitMessage(addMessage(this.#db, { conversationId, author: "system", kind, body: body.slice(0, 500), data, status: "done" }), "message.added");
  }

  // A hook that throws must never leave a bot stuck.
  #safely(hook: () => void): void {
    try {
      hook();
    } catch (error) {
      console.error("Tjommie: a runner hook failed", error);
    }
  }

  #emitMessage(message: Message, event: "message.added" | "message.updated" = "message.updated"): void {
    this.#broadcast(event, message);
  }

  #emitBot(botId: string): void {
    try {
      this.#broadcast("bot.updated", this.toBot(getStoredBot(this.#db, botId)));
    } catch {
      // the bot was deleted
    }
  }
}
```

- [ ] **Step 4: Replace the runner's real tests**

Replace `packages/daemon/src/runner.real.test.ts` with:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { BotInput } from "@tjommie/protocol";
import { createBot, getStoredBot } from "./bots.ts";
import { killTree } from "./cli.ts";
import { openDb, type Db } from "./db.ts";
import { McpTokens } from "./mcp.ts";
import type { WorkItem } from "./queue.ts";
import { Runner, cliSessionFactory, type RunnerHooks, type TurnResult } from "./runner.ts";
import { ensureAccounts, listModels } from "./setup.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

type Sent = { event: string; data: any };
let runners: Runner[] = [];
afterEach(() => {
  for (const runner of runners) runner.shutdown();
  runners = [];
});

// A runner on a fresh database in a spaced, accented folder, using the real signed-in account folders.
function harness(root = tempRoot(), db: Db = openDb(join(root, "t.db")), hooks: RunnerHooks = {}) {
  ensureAccounts(db, realAccountsDir());
  const events: Sent[] = [];
  let sessions = 0;
  const runner = new Runner({
    db,
    broadcast: (event, data) => events.push({ event, data }),
    isSignedIn: () => true,
    createSession: (bot, options) => {
      sessions++;
      return cliSessionFactory(bot, options);
    },
    hooks,
  });
  runners.push(runner);
  runner.start();
  return {
    root,
    db,
    runner,
    events,
    sessions: () => sessions,
    state: (botId: string) => runner.toBot(getStoredBot(db, botId)).state,
    replies: (botId: string) =>
      events.filter((e) => e.event === "message.updated" && e.data.author === botId && e.data.kind === "text" && e.data.status === "done").map((e) => e.data.body as string),
    // Finished texts written into one conversation: a bot's chat, "team" or an exchange.
    said: (conversationId: string) =>
      events.filter((e) => e.event === "message.updated" && e.data.conversationId === conversationId && e.data.kind === "text" && e.data.status === "done").map((e) => e.data.body as string),
    firstAdded: (conversationId: string, kind: string) => events.find((e) => e.event === "message.added" && e.data.conversationId === conversationId && e.data.kind === kind),
  };
}

let count = 0;
const newBot = (h: ReturnType<typeof harness>, overrides: Partial<BotInput> = {}) =>
  createBot(h.db, join(h.root, "bots"), {
    name: `Bot ${++count}`,
    color: "blue",
    shape: "circle",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...overrides,
  });

const LONG_COMMAND = (seconds: number) =>
  `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('done'), ${seconds * 1000})". Then tell me what it printed.`;

describe("Runner against the real CLIs", () => {
  it("a Claude bot answers, streams, records usage and saves its session", async () => {
    const h = harness();
    const bot = newBot(h);
    h.runner.submit(bot.id, "Reply with exactly: PONG", []);
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("PONG")));
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.events.some((e) => e.event === "message.delta")).toBe(true);
    expect((h.db.prepare("SELECT cost_usd FROM usage WHERE bot_id = ?").get(bot.id) as { cost_usd: number }).cost_usd).toBeGreaterThan(0);
    expect(h.db.prepare("SELECT provider FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ provider: "claude" });
  });

  it("a Codex bot answers", async () => {
    const h = harness();
    const model = (await listModels(h.db, "codex")).find((m) => m.isDefault)!.id;
    const bot = newBot(h, { provider: "codex", model });
    h.runner.submit(bot.id, "Reply with exactly: PONG", []);
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("PONG")));
    await waitFor(() => h.state(bot.id) === "idle");
  });

  it("two quick messages to an idle bot start one session and reach it in order (double send)", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "full" });
    const first = h.runner.submit(bot.id, LONG_COMMAND(8), []);
    const second = h.runner.submit(bot.id, "Also end your reply with the word BANANA.", []);
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("BANANA")));
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.sessions()).toBe(1);
    const statuses = h.db.prepare("SELECT status FROM messages WHERE id IN (?, ?)").all(first.id, second.id) as { status: string }[];
    expect(statuses.map((s) => s.status)).toEqual(["sent", "sent"]);
  });

  it("turns a permission request into a card; Always allow saves the rule and the turn carries on", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "ask" });
    h.runner.submit(bot.id, "Use the Write tool to create hello.txt containing the word hi. Then reply DONE.", []);
    const added = await waitFor(() => h.events.find((e) => e.event === "approval.added"));
    expect(h.state(bot.id)).toBe("waiting");
    expect(h.events.some((e) => e.event === "notify")).toBe(true);
    h.runner.answerApproval(added.data.id, "always");
    await waitFor(() => h.replies(bot.id).some((reply) => reply.includes("DONE")));
    expect(existsSync(join(bot.homeFolder, "hello.txt"))).toBe(true);
    expect(h.db.prepare("SELECT rule FROM allow_rules WHERE bot_id = ?").all(bot.id)).toEqual([{ rule: "Write" }]);
  });

  it("gets back to Idle with a Retry when the CLI dies mid-turn (CLI killed mid-turn)", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "full" });
    const message = h.runner.submit(bot.id, LONG_COMMAND(60), []);
    await waitFor(() => h.firstAdded(bot.id, "activity"));
    killTree(h.runner.processId(bot.id));
    const error = await waitFor(() => h.firstAdded(bot.id, "error"));
    expect(error.data.data).toEqual({ retryMessageId: message.id });
    await waitFor(() => h.state(bot.id) === "idle");
    expect(h.db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "failed" });
  });

  it("keeps a conversation across a restart (resume) and across a fresh session (recap)", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const first = harness(root, db);
    const bot = newBot(first);
    first.runner.submit(bot.id, "Remember the code word ZEBRA. Reply with just OK.", []);
    await waitFor(() => first.replies(bot.id).length > 0);
    await waitFor(() => first.state(bot.id) === "idle");
    first.runner.shutdown();

    const second = harness(root, db); // the same database: a restarted daemon
    second.runner.submit(bot.id, "What was the code word? Reply with just the word.", []);
    await waitFor(() => second.replies(bot.id).some((reply) => /ZEBRA/i.test(reply)));
    await waitFor(() => second.state(bot.id) === "idle");

    db.prepare("DELETE FROM sessions WHERE bot_id = ?").run(bot.id); // the next turn has to start a fresh CLI session
    second.runner.botChanged(bot.id, false);
    const before = second.replies(bot.id).length;
    second.runner.submit(bot.id, "Say the code word again. Reply with just the word.", []);
    await waitFor(() => second.replies(bot.id).length > before && /ZEBRA/i.test(second.replies(bot.id).at(-1)!));
  });

  it("writes a turn where its work belongs, and doesn't steer a chat message into a Team room answer", async () => {
    const h = harness();
    const bot = newBot(h, { autonomy: "full" });
    h.runner.deliver({ botId: bot.id, source: "room", text: `${LONG_COMMAND(8)} End your reply with the word ROOM.`, conversationId: "team" });
    await waitFor(() => h.firstAdded("team", "activity"));
    const message = h.runner.submit(bot.id, "Reply with exactly: CHAT", []);
    expect(h.db.prepare("SELECT status FROM messages WHERE id = ?").get(message.id)).toEqual({ status: "queued" });
    await waitFor(() => h.said(bot.id).some((text) => text.includes("CHAT")));
    expect(h.said("team").some((text) => text.includes("ROOM"))).toBe(true);
    expect(h.said("team").some((text) => text.includes("CHAT"))).toBe(false);
    expect(h.said(bot.id).some((text) => text.includes("ROOM"))).toBe(false);
  });

  it("tells the hooks how each turn went, and revokes the session's MCP token when it closes (token revoked on close)", async () => {
    const tokens = new McpTokens();
    const ended: [WorkItem, TurnResult][] = [];
    let token = "";
    const h = harness(undefined, undefined, {
      mcpServers: (bot) => {
        token = tokens.issue(bot.id);
        return [];
      },
      sessionClosed: (botId) => tokens.revoke(botId),
      turnEnded: (item, result) => ended.push([item, result]),
    });
    const bot = newBot(h);
    h.runner.submit(bot.id, "Reply with exactly: PONG", []);
    await waitFor(() => ended.length > 0);
    expect(ended[0][1]).toMatchObject({ outcome: "done", text: expect.stringContaining("PONG"), error: null });
    expect(tokens.botFor(token)).toBe(bot.id);
    h.runner.forget(bot.id);
    expect(tokens.botFor(token)).toBeNull();
  });
});
```

- [ ] **Step 5: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including "Runner.start".

Run: `npm run typecheck`
Expected: no errors.

Run: `npx vitest run --mode real packages/daemon/src/runner.real.test.ts`
Expected: all 8 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/runner.ts packages/daemon/src/runner.test.ts packages/daemon/src/runner.real.test.ts
git commit -m "Route each turn to its conversation and add runner hooks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Bots working together

**Files:**
- Create: `packages/daemon/src/team.ts`
- Modify: `packages/daemon/src/bots.ts` (`botPrompt`)
- Test: `packages/daemon/src/team.test.ts`, `packages/daemon/src/bots.test.ts` (add a test)

**Interfaces:**
- Consumes:
  - `defineTool` and `McpTool` (Task 4); `createRoutine` and `hostTimeZone` (Task 5); `duration` (Task 6).
  - `TurnResult` and `copyIntoInbox` (Task 8).
  - `newChain`, `useChainRequest`, `NewWork` and `WorkItem` (Task 3); `getSettings` (Task 2).
- Produces:
  - `getExchange(db, id): Exchange`.
  - `findMentions(text, bots): string[]`.
  - `fileInside(roots, target): string`, which returns the real path, or throws a message the bot can read.
  - `class Team`, constructed with `{ db, broadcast, deliver, currentWork, stateOf }`:
    - `tools(): McpTool[]`, which are `list_bots`, `message_bot`, `share_file` and `add_routine`.
    - `postToRoom(text): Message`.
    - `turnEnded(item, result)`, the runner hook.
    - `botDeleted(botId)`, to call before deleting a bot.
    - `answerRoutineProposal(approvalId, decision): Approval`.
  - `botPrompt` tells the bot about Tjommie's tools, and about its browser only when the browser is on.

How an exchange flows, as the spec's §6 describes:
- **Asking.** `message_bot` checks the Can message list and the chain limit.
- **Recording.** It creates the exchange and its conversation, and adds the request as the conversation's first message. It posts "Researcher to Writer" in the room and adds a handoff row to both chats.
- **Delivering.** It queues the target's work in that conversation, then answers at once: "Sent. Writer will reply."
- **Replying.** When any turn of the target bot in that conversation ends, `turnEnded` sends the turn's last text back to the asking bot. It becomes a `bot_reply` in the conversation the asking turn was writing to. Files the target shared travel with the reply, copied into the asker's inbox.
- **Follow-ups.** The first answer marks the exchange replied. Later turns there (after nested requests) send follow-ups.

- [ ] **Step 1: Write the failing tests**

Create `packages/daemon/src/team.test.ts`:

```ts
import { mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { TEAM_ROOM_ID } from "@tjommie/protocol";
import { getApproval } from "./approvals.ts";
import { createBot, deleteBot, updateBot, type StoredBot } from "./bots.ts";
import { addMessage, history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, newChain, setStatus, type WorkItem } from "./queue.ts";
import { listRoutines } from "./routines.ts";
import { setSettings } from "./settings.ts";
import { Team, findMentions, getExchange } from "./team.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let root: string;
let team: Team;
let delivered: WorkItem[];
let current: Map<string, WorkItem>;
let events: { event: string; data: any }[];
let researcher: StoredBot;
let writer: StoredBot;

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "t.db"));
  const bots = join(root, "bots");
  writer = createBot(db, bots, { name: "Writer", color: "blue", shape: "circle", instructions: "Writes one-page summaries.\nMore detail here.", provider: "codex", model: "gpt-5.6-sol" });
  researcher = createBot(db, bots, { name: "Researcher", color: "teal", shape: "droplet", instructions: "Finds sources.", provider: "claude", model: "sonnet", canMessage: [writer.id] });
  writer = updateBot(db, writer.id, { canMessage: [researcher.id] });
  delivered = [];
  current = new Map();
  events = [];
  team = new Team({
    db,
    broadcast: (event, data) => events.push({ event, data }),
    deliver: (work) => delivered.push(enqueue(db, work)),
    currentWork: (botId) => current.get(botId) ?? null,
    stateOf: () => "idle",
  });
});

const tool = (name: string) => team.tools().find((t) => t.name === name)!;
const call = (botId: string, name: string, args: unknown) => tool(name).run(botId, tool(name).input.parse(args));
// A turn started by the person in the bot's own chat, with a chain of its own.
const chatTurn = (botId: string) => {
  const item = enqueue(db, { botId, source: "you", text: "hi", chainId: newChain(db) });
  current.set(botId, item);
  return item;
};
const done = (text: string | null, textMessageId: string | null = null) => ({ outcome: "done" as const, text, textMessageId, error: null });

describe("Team tools", () => {
  it("lists only the bots on the caller's Can message list", () => {
    expect(call(researcher.id, "list_bots", {})).toBe("- Writer (idle): Writes one-page summaries.");
    const loner = createBot(db, join(root, "bots"), { name: "Loner", color: "gray", shape: "pill", instructions: "", provider: "claude", model: "haiku" });
    expect(call(loner.id, "list_bots", {})).toMatch(/can't message any bots yet/);
  });

  it("delivers a request, shows it in both chats and the room, and sends the answer and files back", () => {
    chatTurn(researcher.id);
    expect(call(researcher.id, "message_bot", { bot: "@writer", message: "Turn these notes into a summary." })).toBe("Sent. Writer will reply.");
    const [request] = delivered;
    const exchangeId = request.exchangeId!;
    expect(request).toMatchObject({ botId: writer.id, source: "bot_request", conversationId: exchangeId, chainId: current.get(researcher.id)!.chainId });
    expect(request.text).toContain("Message from Researcher");
    for (const botId of [researcher.id, writer.id]) {
      expect(history(db, botId, undefined, 5).at(-1)).toMatchObject({ kind: "handoff", body: "Turn these notes into a summary.", data: { exchangeId, messages: 1, status: "waiting" } });
    }
    expect(history(db, TEAM_ROOM_ID, undefined, 5)).toMatchObject([{ author: researcher.id, body: "Turn these notes into a summary.", data: { exchangeId, toBotId: writer.id } }]);

    // Writer's turn: it shares a file, then answers. The runner writes the answer into the exchange.
    current.set(writer.id, request);
    writeFileSync(join(writer.homeFolder, "summary.md"), "# Summary");
    expect(call(writer.id, "share_file", { path: "summary.md" })).toBe("Shared summary.md.");
    addMessage(db, { conversationId: exchangeId, author: writer.id, kind: "text", body: "Here's the summary.", status: "done" });
    team.turnEnded(request, done("Here's the summary."));

    expect(getExchange(db, exchangeId)).toMatchObject({ status: "replied", reply: "Here's the summary." });
    const reply = delivered[1];
    expect(reply).toMatchObject({ botId: researcher.id, source: "bot_reply", conversationId: researcher.id, exchangeId });
    expect(reply.text).toContain("Writer replied");
    expect(reply.text).toContain("Here's the summary.");
    expect(reply.text).toContain("summary.md");
    expect(readFileSync(join(researcher.homeFolder, "inbox", "summary.md"), "utf8")).toBe("# Summary");
    expect(history(db, researcher.id, undefined, 5).find((m) => m.kind === "handoff")?.data).toMatchObject({ messages: 2, status: "replied" });
    expect(history(db, TEAM_ROOM_ID, undefined, 5).at(-1)).toMatchObject({ author: writer.id, body: "Here's the summary.", data: { toBotId: researcher.id, files: [{ name: "summary.md" }] } });
  });

  it("refuses unknown bots, bots off the list, and the caller itself", () => {
    createBot(db, join(root, "bots"), { name: "Loner", color: "gray", shape: "pill", instructions: "", provider: "claude", model: "haiku" });
    expect(() => call(researcher.id, "message_bot", { bot: "Nobody", message: "hi" })).toThrow(/no bot called Nobody/);
    expect(() => call(researcher.id, "message_bot", { bot: "Loner", message: "hi" })).toThrow(/not allowed to message Loner/);
    expect(() => call(researcher.id, "message_bot", { bot: "Researcher", message: "hi" })).toThrow(/can't message yourself/);
    expect(delivered).toEqual([]);
  });

  it("stops two bots asking each other in a loop at the chain limit (chain limit)", () => {
    setSettings(db, { chainLimit: 2 });
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Ask me something back." });
    current.set(writer.id, delivered[0]);
    call(writer.id, "message_bot", { bot: "Researcher", message: "What do you need?" });
    expect(delivered[1].chainId).toBe(delivered[0].chainId);
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Again." })).toThrow(/^Limit reached, ask the user/);
    chatTurn(researcher.id); // the person's next message starts a new chain
    expect(call(researcher.id, "message_bot", { bot: "Writer", message: "Fresh start." })).toBe("Sent. Writer will reply.");
  });

  it("tells the asking bot when the other bot couldn't answer", () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Summarize." });
    team.turnEnded(delivered[0], { outcome: "failed", text: null, textMessageId: null, error: "Codex exited with code 1" });
    expect(delivered[1]).toMatchObject({ botId: researcher.id, source: "bot_reply" });
    expect(delivered[1].text).toContain("Writer couldn't finish your request");
    expect(delivered[1].text).toContain("Codex exited with code 1");
    expect(getExchange(db, delivered[0].exchangeId!).status).toBe("failed");
  });

  it("ends a deleted asker's exchanges without errors (requester deleted)", () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "First." });
    call(researcher.id, "message_bot", { bot: "Writer", message: "Second." });
    const [started, waiting] = delivered;
    setStatus(db, started.id, "running");
    team.botDeleted(researcher.id);
    deleteBot(db, researcher.id);
    expect(db.prepare("SELECT id FROM work_items WHERE id = ?").get(waiting.id)).toBeUndefined();
    expect(getExchange(db, waiting.exchangeId!).status).toBe("failed");
    current.set(writer.id, started);
    expect(() => team.turnEnded(started, done("Done anyway."))).not.toThrow();
    expect(delivered).toHaveLength(2); // nothing goes back to the deleted bot
    expect(getExchange(db, started.exchangeId!).status).toBe("replied");
  });

  it("tells the asking bot when the bot it asked is deleted", () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Summarize." });
    team.botDeleted(writer.id);
    deleteBot(db, writer.id);
    expect(delivered[1]).toMatchObject({ botId: researcher.id, source: "bot_reply" });
    expect(delivered[1].text).toContain("Writer was deleted");
    expect(getExchange(db, delivered[0].exchangeId!).status).toBe("failed");
  });

  it("shares only existing files inside the bot's own folders (share outside folder)", () => {
    const outside = join(root, "outside");
    mkdirSync(outside);
    writeFileSync(join(outside, "secret.txt"), "no");
    writeFileSync(join(writer.homeFolder, "ok.txt"), "yes");
    symlinkSync(outside, join(writer.homeFolder, "link"), "junction");
    expect(call(writer.id, "share_file", { path: "ok.txt", caption: "The summary" })).toBe("Shared ok.txt.");
    expect(history(db, writer.id, undefined, 1)[0]).toMatchObject({ kind: "file", body: "The summary", data: { name: "ok.txt", size: 3 } });
    for (const path of [join("..", "..", "outside", "secret.txt"), join(outside, "secret.txt"), join("link", "secret.txt")]) {
      expect(() => call(writer.id, "share_file", { path })).toThrow(/only share files from your own folders/);
    }
    expect(() => call(writer.id, "share_file", { path: "missing.txt" })).toThrow(/no file/);
  });

  it("proposes a routine as a card, and creates it only when the person approves", () => {
    expect(call(writer.id, "add_routine", { name: "Morning check", days: ["mon", "tue", "wed", "thu", "fri"], times: ["08:48"], prompt: "Check pnet.co.za", time_limit_minutes: 60 })).toMatch(/^Proposed/);
    const first = events.find((e) => e.event === "approval.added")!.data;
    expect(first).toMatchObject({ kind: "routine", summary: "Add a routine: Morning check, weekdays at 08:48, stop after 1 hour." });
    expect(listRoutines(db, writer.id)).toEqual([]);
    team.answerRoutineProposal(first.id, "once");
    expect(listRoutines(db, writer.id)).toMatchObject([{ name: "Morning check", days: [1, 2, 3, 4, 5], times: ["08:48"], timeLimitMinutes: 60 }]);
    call(writer.id, "add_routine", { name: "Other", days: ["sat"], times: ["10:00"], prompt: "Tidy the inbox" });
    const second = events.filter((e) => e.event === "approval.added")[1].data;
    team.answerRoutineProposal(second.id, "deny");
    expect(listRoutines(db, writer.id)).toHaveLength(1);
    expect(getApproval(db, second.id).status).toBe("denied");
  });
});

describe("Team room", () => {
  it("asks each bot a post mentions, with the recent room messages, on one chain", () => {
    team.postToRoom("Morning all.");
    expect(delivered).toEqual([]);
    team.postToRoom("@Writer and @researcher, what's new?");
    expect(delivered.map((item) => item.botId).sort()).toEqual([writer.id, researcher.id].sort());
    expect(delivered[0]).toMatchObject({ source: "room", conversationId: TEAM_ROOM_ID });
    expect(delivered[0].chainId).toBe(delivered[1].chainId);
    expect(delivered[0].text).toContain("Person: Morning all.");
    expect(delivered[0].text).toContain("what's new?");
  });

  it("notes a room answer in the bot's own chat", () => {
    team.postToRoom("@Writer hi");
    team.turnEnded(delivered[0], done("Hello!", "m1"));
    expect(history(db, writer.id, undefined, 1)[0]).toMatchObject({ kind: "handoff", body: "Answered in the Team room", data: { roomMessageId: "m1" } });
  });
});

describe("findMentions", () => {
  const bots = [
    { id: "w", name: "Writer" },
    { id: "cv", name: "CV Writer" },
    { id: "b", name: "Bot" },
    { id: "b2", name: "Bot 2" },
  ];

  it("matches whole names, longest first, and ignores email addresses", () => {
    expect(findMentions("@CV Writer can you help?", bots)).toEqual(["cv"]);
    expect(findMentions("@writer and @cv writer!", bots)).toEqual(["w", "cv"]);
    expect(findMentions("@Bot 2 and @Bot 2 again", bots)).toEqual(["b2"]);
    expect(findMentions("mail me@writer.com or @Writers", bots)).toEqual([]);
    expect(findMentions("@everyone stop", bots)).toEqual(["w", "cv", "b", "b2"]);
  });
});
```

Add this test to `packages/daemon/src/bots.test.ts` (inside the `describe`):

```ts
  it("tells the bot about Tjommie's tools, and about its browser only when it has one", () => {
    const bot = createBot(db, botsDir, input);
    expect(botPrompt(bot)).toContain("message_bot");
    expect(botPrompt(bot)).not.toContain("browser tools");
    expect(botPrompt({ ...bot, browser: true })).toContain("browser tools");
  });
```

- [ ] **Step 2: Run them to see them fail**

Run: `npm test`
Expected: FAIL. `./team.ts` can't be found, and `botPrompt` doesn't mention `message_bot`.

- [ ] **Step 3: Tell bots about their tools**

In `packages/daemon/src/bots.ts`, replace `botPrompt` with:

```ts
// The bot's standing instructions, appended to the CLI's own system prompt.
export function botPrompt(bot: StoredBot): string {
  return [
    `You are ${bot.name}, one of the bots a person runs in Tjommie on their own computer.`,
    `Your home folder is ${bot.homeFolder}.${bot.projectFolder ? ` You work in the project folder ${bot.projectFolder}.` : ""}`,
    "Keep notes.md in your home folder up to date with the person's preferences, important facts and short summaries of finished work, and read it when a new session starts.",
    "Files the person attaches are copied into the inbox folder inside your home folder.",
    "Tjommie's tools: list_bots shows the bots you may ask for help; message_bot sends one a request, and its answer arrives later as a new message, so don't wait for it; share_file shows the person a file; add_routine proposes work on a schedule.",
    ...(bot.browser
      ? ["You have your own browser through the browser tools. Sites the person signed in to there stay signed in. If the browser says it's already in use, ask the person to close your browser window."]
      : []),
    "Reply in plain, friendly language, and keep replies short unless the person asks for detail.",
    "",
    "What the person wants from you:",
    bot.instructions.trim() || "No instructions yet. Ask what they need.",
  ].join("\n");
}
```

- [ ] **Step 4: Write team.ts**

Create `packages/daemon/src/team.ts`:

```ts
import { realpathSync, statSync } from "node:fs";
import { basename, isAbsolute, relative, resolve, sep } from "node:path";
import { RoutineInput, TEAM_ROOM_ID, type Approval, type ApprovalDecision, type BotState, type Exchange, type Message } from "@tjommie/protocol";
import { z } from "zod";
import { createApproval, resolveApproval } from "./approvals.ts";
import { getStoredBot, listStoredBots, type StoredBot } from "./bots.ts";
import { addMessage, getMessage, history, updateMessage } from "./chat.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { defineTool, type McpTool } from "./mcp.ts";
import { newChain, useChainRequest, type NewWork, type WorkItem } from "./queue.ts";
import { createRoutine, hostTimeZone } from "./routines.ts";
import { copyIntoInbox, type TurnResult } from "./runner.ts";
import { duration } from "./scheduler.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { getSettings } from "./settings.ts";

const ROOM_CONTEXT = 20;
const MAX_SHARE_BYTES = 200 * 1024 * 1024;
const DAYS = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"] as const;
const DAY_LABELS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const STATE_WORDS: Record<BotState, string> = {
  idle: "idle",
  working: "working",
  waiting: "waiting for the person",
  paused: "paused",
  limited: "at its usage limit",
  needs_setup: "not set up yet",
};
const WORD = /[\p{L}\p{N}]/u;

type ExchangeRow = {
  id: string;
  from_bot_id: string;
  to_bot_id: string;
  request: string;
  reply: string | null;
  status: Exchange["status"];
  chain_id: string;
  reply_to: string;
  created_at: string;
  replied_at: string | null;
};

const toExchange = (row: ExchangeRow): Exchange => ({
  id: row.id,
  fromBotId: row.from_bot_id,
  toBotId: row.to_bot_id,
  request: row.request,
  reply: row.reply,
  status: row.status,
  createdAt: row.created_at,
  repliedAt: row.replied_at,
});

const firstLine = (text: string, max = 200): string => text.trim().split(/\r?\n/, 1)[0].slice(0, max);

const requestPrompt = (from: string, request: string) =>
  `[Message from ${from}, another bot. Your final message in this turn goes back to ${from} as your reply.]\n${request}`;

const replyPrompt = (to: string, request: string, reply: string, files: string[], first: boolean) =>
  [
    `[${to} ${first ? "replied" : "followed up"} on your request "${firstLine(request, 120)}":]`,
    reply,
    ...(files.length ? [`[Files ${to} shared, now in your inbox folder: ${files.join(", ")}]`] : []),
  ].join("\n");

const failedPrompt = (to: string, request: string, reason: string) =>
  `[${to} couldn't finish your request "${firstLine(request, 120)}": ${reason}. Tell the person if it matters.]`;

const roomPrompt = (context: string[], text: string) =>
  [
    "[Team room: the person mentioned you. Your reply is posted in the room for everyone.",
    ...(context.length ? ["Recent room messages, oldest first:", ...context] : []),
    "]",
    `The person wrote: ${text}`,
  ].join("\n");

function describeDays(days: number[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const key = sorted.join("");
  if (key === "0123456") return "every day";
  if (key === "12345") return "weekdays";
  if (key === "06") return "weekends";
  return sorted.map((day) => DAY_LABELS[day]).join(", ");
}

export function getExchange(db: Db, id: string): Exchange {
  const row = db.prepare("SELECT * FROM exchanges WHERE id = ?").get(id) as ExchangeRow | undefined;
  if (!row) throw new RpcError("not_found", "No such exchange");
  return toExchange(row);
}

// The bots a Team room post mentions: "@Name" for whole names, longest names first, or "@everyone".
export function findMentions(text: string, bots: { id: string; name: string }[]): string[] {
  let rest = text.toLowerCase();
  const take = (name: string): boolean => {
    const tag = `@${name.toLowerCase()}`;
    let hit = false;
    for (let at = rest.indexOf(tag); at !== -1; at = rest.indexOf(tag, at + tag.length)) {
      if (WORD.test(rest[at - 1] ?? "") || WORD.test(rest[at + tag.length] ?? "")) continue;
      rest = rest.slice(0, at) + " ".repeat(tag.length) + rest.slice(at + tag.length);
      hit = true;
    }
    return hit;
  };
  const found = new Set<string>();
  for (const bot of [...bots].sort((a, b) => b.name.length - a.name.length)) if (take(bot.name)) found.add(bot.id);
  if (take("everyone")) return bots.map((bot) => bot.id);
  return bots.filter((bot) => found.has(bot.id)).map((bot) => bot.id);
}

// The real path of `target` when it's an existing file inside one of `roots`, with links followed.
// Anything else is an error the bot can read.
export function fileInside(roots: string[], target: string): string {
  let real: string;
  try {
    real = realpathSync.native(target);
  } catch {
    throw new Error(`There's no file at ${target}.`);
  }
  const inside = roots.some((root) => {
    let base: string;
    try {
      base = realpathSync.native(root);
    } catch {
      return false;
    }
    const rel = relative(base, real);
    return rel !== "" && rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
  });
  if (!inside) throw new Error("You can only share files from your own folders.");
  const stats = statSync(real);
  if (!stats.isFile()) throw new Error(`${target} is a folder, not a file.`);
  if (stats.size > MAX_SHARE_BYTES) throw new Error("That file is too big to share. The limit is 200 MB.");
  return real;
}

export type TeamDeps = {
  db: Db;
  broadcast: Broadcast;
  deliver: (work: NewWork) => void;
  currentWork: (botId: string) => WorkItem | null;
  stateOf: (bot: StoredBot) => BotState;
};

// Bots working together: the tools on Tjommie's MCP server, exchanges between bots, and the Team room.
export class Team {
  #db: Db;
  #broadcast: Broadcast;
  #deliver: (work: NewWork) => void;
  #currentWork: (botId: string) => WorkItem | null;
  #stateOf: (bot: StoredBot) => BotState;
  #shared = new Map<string, string[]>(); // work item id -> files the bot shared during that turn

  constructor(deps: TeamDeps) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#deliver = deps.deliver;
    this.#currentWork = deps.currentWork;
    this.#stateOf = deps.stateOf;
  }

  // The tools on Tjommie's MCP server. `botId` is the calling bot, known from its session's token.
  tools(): McpTool[] {
    return [
      defineTool({
        name: "list_bots",
        description: "Lists the bots you may ask for help with message_bot: their names, what they do, and whether they're busy.",
        input: z.object({}),
        run: (botId) => this.#listBots(botId),
      }),
      defineTool({
        name: "message_bot",
        description:
          "Sends a request to another bot and returns at once. The bot works on it when it's free, and its answer arrives later as a new message to you. Don't wait or check for it.",
        input: z.object({
          bot: z.string().trim().min(1).describe("The bot's name, as list_bots shows it"),
          message: z.string().trim().min(1).max(20_000).describe("What you want, with everything the bot needs to know"),
        }),
        run: (botId, { bot, message }) => this.#messageBot(botId, bot, message),
      }),
      defineTool({
        name: "share_file",
        description: "Shows a file from your own folders to the person, as a file card. When you're answering another bot, the file goes to that bot too.",
        input: z.object({
          path: z.string().min(1).describe("An absolute path, or one relative to your working folder"),
          caption: z.string().max(500).optional().describe("A short note shown on the card"),
        }),
        run: (botId, { path, caption }) => this.#shareFile(botId, path, caption),
      }),
      defineTool({
        name: "add_routine",
        description: "Proposes a routine: work you do on a schedule. The person sees a card and decides. Nothing is saved until they approve it.",
        input: z.object({
          name: z.string().trim().min(1).max(60),
          days: z.array(z.enum(DAYS)).min(1).max(7).describe("Days of the week"),
          times: z
            .array(z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/))
            .min(1)
            .max(12)
            .describe(`24-hour times in the person's time zone, like "08:30"`),
          prompt: z.string().trim().min(1).max(10_000).describe("What to do on each run"),
          time_limit_minutes: z.number().int().min(1).max(1440).optional().describe("Stop each run after this many minutes"),
        }),
        run: (botId, args) =>
          this.#proposeRoutine(botId, {
            name: args.name,
            days: args.days.map((day) => DAYS.indexOf(day)),
            times: args.times,
            prompt: args.prompt,
            timeLimitMinutes: args.time_limit_minutes ?? null,
          }),
      }),
    ];
  }

  // A post from the person in the Team room. Every bot it mentions answers in the room.
  postToRoom(text: string): Message {
    const context = this.#describe(history(this.#db, TEAM_ROOM_ID, undefined, ROOM_CONTEXT));
    const post = this.#add({ conversationId: TEAM_ROOM_ID, author: "you", kind: "text", body: text, status: "done" });
    const mentioned = findMentions(text, listStoredBots(this.#db));
    if (mentioned.length === 0) return post;
    const chainId = newChain(this.#db); // one post, one chain, however many bots it asks
    for (const botId of mentioned) this.#deliver({ botId, source: "room", text: roomPrompt(context, text), conversationId: TEAM_ROOM_ID, chainId });
    return post;
  }

  // The runner calls this after each turn it started.
  turnEnded(item: WorkItem, result: TurnResult): void {
    const files = this.#shared.get(item.id) ?? [];
    this.#shared.delete(item.id);
    const exchange = this.#exchange(item.conversationId);
    if (exchange && exchange.to_bot_id === item.botId) this.#answer(exchange, result, files);
    if (item.source === "room" && result.outcome === "done" && result.textMessageId) {
      this.#add({ conversationId: item.botId, author: item.botId, kind: "handoff", body: "Answered in the Team room", data: { roomMessageId: result.textMessageId }, status: "done" });
    }
  }

  // Call before deleting a bot, so no other bot is left waiting on it.
  botDeleted(botId: string): void {
    const name = getStoredBot(this.#db, botId).name;
    const open = this.#db.prepare("SELECT * FROM exchanges WHERE status = 'waiting' AND (from_bot_id = ? OR to_bot_id = ?)").all(botId, botId) as ExchangeRow[];
    for (const row of open) {
      if (row.to_bot_id === botId) {
        this.#fail(row, `${name} was deleted`);
        this.#tell(row, failedPrompt(name, row.request, `${name} was deleted`));
        continue;
      }
      // The asking bot is going. A request not started yet is dropped; one already running finishes with no one to tell.
      const dropped = this.#db.prepare("DELETE FROM work_items WHERE exchange_id = ? AND source = 'bot_request' AND status = 'queued'").run(row.id);
      if (Number(dropped.changes) > 0) this.#fail(row, `${name} was deleted`);
    }
  }

  // The person answered a routine proposal: "Add routine" (once or always) or "Dismiss" (deny).
  answerRoutineProposal(approvalId: string, decision: ApprovalDecision): Approval {
    const approval = resolveApproval(this.#db, approvalId, decision);
    if (approval.status === "allowed") this.#broadcast("routine.updated", createRoutine(this.#db, RoutineInput.parse(approval.payload)));
    const message = getMessage(this.#db, approval.messageId);
    this.#broadcast("message.updated", updateMessage(this.#db, message.id, { data: { ...message.data, status: approval.status } }));
    this.#broadcast("approval.resolved", approval);
    return approval;
  }

  #listBots(botId: string): string {
    const targets = getStoredBot(this.#db, botId).canMessage.map((id) => getStoredBot(this.#db, id));
    if (targets.length === 0) return "You can't message any bots yet. The person can add bots to your Can message list in your settings.";
    return targets.map((bot) => `- ${bot.name} (${STATE_WORDS[this.#stateOf(bot)]}): ${firstLine(bot.instructions, 120) || "No description."}`).join("\n");
  }

  #messageBot(fromId: string, name: string, request: string): string {
    const from = getStoredBot(this.#db, fromId);
    const wanted = name.replace(/^@/, "").toLowerCase();
    const to = listStoredBots(this.#db).find((bot) => bot.name.toLowerCase() === wanted);
    if (!to) throw new Error(`There's no bot called ${name}. Use list_bots to see who you can ask.`);
    if (to.id === from.id) throw new Error("You can't message yourself.");
    if (!from.canMessage.includes(to.id)) throw new Error(`You're not allowed to message ${to.name}. The person can add ${to.name} to your Can message list.`);
    const work = this.#currentWork(fromId);
    const chainId = work?.chainId ?? newChain(this.#db);
    if (!useChainRequest(this.#db, chainId, getSettings(this.#db).chainLimit)) {
      throw new Error("Limit reached, ask the user. This chain of requests between bots has used all its requests, so ask the person before asking another bot.");
    }
    const id = newId();
    transaction(this.#db, () => {
      this.#db
        .prepare("INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, status, chain_id, reply_to, created_at) VALUES (?, ?, ?, ?, 'waiting', ?, ?, ?)")
        .run(id, from.id, to.id, request, chainId, work?.conversationId ?? from.id, nowIso());
      this.#db.prepare("INSERT INTO conversations (id, kind, bot_id) VALUES (?, 'exchange', NULL)").run(id);
    });
    this.#add({ conversationId: id, author: from.id, kind: "text", body: request, status: "done" });
    this.#add({ conversationId: TEAM_ROOM_ID, author: from.id, kind: "text", body: request, data: { exchangeId: id, toBotId: to.id }, status: "done" });
    const row = { exchangeId: id, fromBotId: from.id, toBotId: to.id, messages: 1, status: "waiting" };
    for (const botId of [from.id, to.id]) this.#add({ conversationId: botId, author: from.id, kind: "handoff", body: firstLine(request), data: row, status: "done" });
    this.#deliver({ botId: to.id, source: "bot_request", text: requestPrompt(from.name, request), conversationId: id, chainId, exchangeId: id });
    return `Sent. ${to.name} will reply.`;
  }

  #shareFile(botId: string, path: string, caption: string | undefined): string {
    const bot = getStoredBot(this.#db, botId);
    const roots = bot.projectFolder ? [bot.projectFolder, bot.homeFolder] : [bot.homeFolder];
    const file = fileInside(roots, resolve(roots[0], path));
    const work = this.#currentWork(botId);
    const name = basename(file);
    this.#add({ conversationId: work?.conversationId ?? botId, author: botId, kind: "file", body: caption?.trim() || name, data: { path: file, name, size: statSync(file).size }, status: "done" });
    if (work) this.#shared.set(work.id, [...(this.#shared.get(work.id) ?? []), file]);
    return `Shared ${name}.`;
  }

  #proposeRoutine(botId: string, r: { name: string; days: number[]; times: string[]; prompt: string; timeLimitMinutes: number | null }): string {
    const bot = getStoredBot(this.#db, botId);
    const times = [...new Set(r.times)].sort();
    const summary = `Add a routine: ${r.name}, ${describeDays(r.days)} at ${times.join(", ")}${r.timeLimitMinutes ? `, stop after ${duration(r.timeLimitMinutes)}` : ""}.`;
    const approvalId = newId();
    const message = this.#add({
      conversationId: botId,
      author: botId,
      kind: "approval",
      body: summary,
      data: { approvalId, kind: "routine", tool: "Routine", detail: r.prompt, status: "pending" },
      status: "done",
    });
    const approval = createApproval(this.#db, {
      id: approvalId,
      botId,
      kind: "routine",
      tool: "Routine",
      summary,
      detail: r.prompt,
      rule: null,
      payload: { botId, ...r, timeZone: hostTimeZone() },
      messageId: message.id,
    });
    this.#broadcast("approval.added", approval);
    this.#broadcast("notify", { botId, title: `${bot.name} is waiting for you`, body: summary });
    return "Proposed. The person sees a card and decides; nothing is saved until they approve it.";
  }

  // What the bot that was asked said at the end of a turn in the exchange, or why it couldn't answer.
  #answer(row: ExchangeRow, result: TurnResult, files: string[]): void {
    const first = row.status === "waiting";
    const to = getStoredBot(this.#db, row.to_bot_id);
    if (result.outcome !== "done") {
      const reason = result.outcome === "interrupted" ? "it was stopped" : (result.error ?? "something went wrong");
      if (first) this.#fail(row, reason);
      this.#tell(row, failedPrompt(to.name, row.request, reason));
      return;
    }
    const reply = result.text ?? "(No written reply.)";
    if (first) this.#db.prepare("UPDATE exchanges SET status = 'replied', reply = ?, replied_at = ? WHERE id = ?").run(reply, nowIso(), row.id);
    const shared = files.map((path) => ({ name: basename(path), path }));
    this.#add({ conversationId: TEAM_ROOM_ID, author: to.id, kind: "text", body: reply, data: { exchangeId: row.id, toBotId: row.from_bot_id, files: shared }, status: "done" });
    this.#refreshRows(row.id);
    const from = this.#botOrNull(row.from_bot_id);
    if (!from) return; // the bot that asked was deleted
    const inbox = files.map((path) => copyIntoInbox(from.homeFolder, path));
    this.#tell(row, replyPrompt(to.name, row.request, reply, inbox, first));
  }

  // Ends an exchange that won't get an answer, in its handoff rows and in the room.
  #fail(row: ExchangeRow, reason: string): void {
    this.#db.prepare("UPDATE exchanges SET status = 'failed', reply = ?, replied_at = ? WHERE id = ?").run(reason, nowIso(), row.id);
    this.#refreshRows(row.id);
    this.#add({
      conversationId: TEAM_ROOM_ID,
      author: "system",
      kind: "notice",
      body: `${this.#name(row.to_bot_id)} couldn't answer ${this.#name(row.from_bot_id)}: ${reason}.`,
      data: { exchangeId: row.id },
      status: "done",
    });
  }

  // Sends a reply or a failure to the bot that asked, into the conversation its asking turn was writing to.
  #tell(row: ExchangeRow, text: string): void {
    if (!this.#botOrNull(row.from_bot_id)) return;
    const replyTo = this.#db.prepare("SELECT 1 FROM conversations WHERE id = ?").get(row.reply_to) ? row.reply_to : row.from_bot_id;
    this.#deliver({ botId: row.from_bot_id, source: "bot_reply", text, conversationId: replyTo, chainId: row.chain_id, exchangeId: row.id });
  }

  // Brings the handoff rows in both chats up to date: message count and status.
  #refreshRows(exchangeId: string): void {
    const row = this.#exchange(exchangeId)!;
    const { n } = this.#db.prepare("SELECT COUNT(*) AS n FROM messages WHERE conversation_id = ? AND kind = 'text'").get(exchangeId) as { n: number };
    const ids = this.#db.prepare("SELECT id FROM messages WHERE kind = 'handoff' AND json_extract(data, '$.exchangeId') = ?").all(exchangeId) as { id: string }[];
    for (const { id } of ids) {
      const message = getMessage(this.#db, id);
      this.#broadcast("message.updated", updateMessage(this.#db, id, { data: { ...message.data, messages: n, status: row.status } }));
    }
  }

  #exchange(id: string): ExchangeRow | undefined {
    return this.#db.prepare("SELECT * FROM exchanges WHERE id = ?").get(id) as ExchangeRow | undefined;
  }

  #botOrNull(id: string): StoredBot | null {
    try {
      return getStoredBot(this.#db, id);
    } catch {
      return null;
    }
  }

  #name(id: string): string {
    return this.#botOrNull(id)?.name ?? "A deleted bot";
  }

  // Room messages as lines of text for a bot's prompt.
  #describe(messages: Message[]): string[] {
    const name = (id: string) => (id === "you" ? "Person" : id === "system" ? "Tjommie" : this.#name(id));
    return messages
      .filter((m) => (m.kind === "text" || m.kind === "notice") && m.body.trim() !== "")
      .map((m) => {
        const to = m.data?.toBotId;
        return `${name(m.author)}${typeof to === "string" ? ` to ${name(to)}` : ""}: ${m.body.trim().slice(0, 2000)}`;
      });
  }

  #add(m: { conversationId: string; author: string; kind: Message["kind"]; body: string; data?: Record<string, unknown> | null; status: Message["status"] }): Message {
    const message = addMessage(this.#db, m);
    this.#broadcast("message.added", message);
    return message;
  }
}
```

- [ ] **Step 5: Run the tests and the type check**

Run: `npm test`
Expected: PASS, including the 12 Team tests and the new bots test.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/team.ts packages/daemon/src/team.test.ts packages/daemon/src/bots.ts packages/daemon/src/bots.test.ts
git commit -m "Let bots message each other, share files, propose routines and answer in the Team room

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Daemon wiring and the bots-together demo tests

**Files:**
- Replace: `packages/daemon/src/daemon.ts`, `packages/daemon/src/daemon.real.test.ts`

**Interfaces:**
- Consumes: everything from Tasks 1 to 9.
- Produces:
  - `Daemon` gains `scheduler: Scheduler`, so tests can tick it.
  - The WebSocket handlers for every method in Task 1.
  - `/mcp` on the daemon's port.
  - Each CLI session gets `tjommie` (HTTP, a fresh token, autoAllow), plus `browser` when the bot's browser is on.
  - Deleting a bot first ends its exchanges (`team.botDeleted`).
  - `approvals.respond` sends routine proposals to the Team and tool approvals to the runner.

- [ ] **Step 1: Write the failing tests**

Replace `packages/daemon/src/daemon.real.test.ts` with:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import { startDaemon, type Daemon } from "./daemon.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

process.env.TJOMMIE_BROWSER_HEADLESS = "1"; // the browser test opens no window

let daemon: Daemon;
beforeAll(async () => {
  daemon = await startDaemon({ root: tempRoot(), port: 0, accountsDir: realAccountsDir() });
});
afterAll(() => daemon.close());

// A minimal client: Node 24's built-in WebSocket, speaking the protocol like the UI will.
async function connect(port: number, token: string) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}`);
  const events: { event: string; data: any }[] = [];
  const pending = new Map<number, (message: any) => void>();
  let nextId = 1;
  let welcomed: () => void = () => {};
  const welcome = new Promise<void>((resolve) => (welcomed = resolve));
  ws.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    if (message.type === "welcome") welcomed();
    if (message.type === "event") events.push(message);
    if (message.type === "res") {
      pending.get(message.id)?.(message);
      pending.delete(message.id);
    }
  });
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve);
    ws.addEventListener("error", reject);
  });
  ws.send(JSON.stringify({ type: "hello", protocol: 1, token }));
  await welcome;
  const call = (method: string, params: unknown): Promise<any> =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, (m) => (m.ok ? resolve(m.result) : reject(new Error(`${m.error.code}: ${m.error.message}`))));
      ws.send(JSON.stringify({ type: "req", id, method, params }));
    });
  return { call, events, close: () => ws.close() };
}

type Client = Awaited<ReturnType<typeof connect>>;

// Waits for a finished text by `author` in `conversationId` containing `word`, among the events after index `from`.
const said = (client: Client, conversationId: string, author: string, word: string, from = 0) =>
  waitFor(() =>
    client.events
      .slice(from)
      .find((e) => e.event === "message.updated" && e.data.conversationId === conversationId && e.data.author === author && e.data.kind === "text" && e.data.status === "done" && e.data.body.includes(word)),
  );

const claudeBot = (client: Client, name: string, extra: Record<string, unknown> = {}) =>
  client.call("bots.create", { name, color: "green", shape: "pill", instructions: "You are a terse test bot. Answer in one short line.", provider: "claude", model: "haiku", ...extra });

it("creates a bot and chats with it over the WebSocket API", async () => {
  const client = await connect(daemon.port, daemon.token);
  const setup = await client.call("setup.status", {});
  expect(setup.map((s: any) => s.signedIn)).toEqual([true, true]);
  const bot = await client.call("bots.create", { name: "Echo", color: "green", shape: "pill", instructions: "Answer in as few words as possible.", provider: "claude", model: "haiku" });
  expect(bot).toMatchObject({ name: "Echo", state: "idle", autonomy: "folder" });
  await client.call("chat.send", { botId: bot.id, text: "Reply with exactly: PONG" });
  const reply = await waitFor(() =>
    client.events.find((e) => e.event === "message.updated" && e.data.author === bot.id && e.data.kind === "text" && e.data.status === "done"),
  );
  expect(reply.data.body).toContain("PONG");
  expect(client.events.some((e) => e.event === "message.delta")).toBe(true);
  const history = await client.call("chat.history", { botId: bot.id });
  expect(history.map((m: any) => m.author)).toEqual(["you", bot.id]);
  await expect(client.call("chat.send", { botId: bot.id, text: "   " })).rejects.toThrow(/bad_params/);
  client.close();
});

it("a Claude bot asks a Codex bot for help and gets the answer back", async () => {
  const client = await connect(daemon.port, daemon.token);
  const models = await client.call("models.list", { provider: "codex" });
  const writer = await client.call("bots.create", {
    name: "Writer",
    color: "blue",
    shape: "circle",
    instructions: "Answer requests in as few words as possible.",
    provider: "codex",
    model: models.find((m: any) => m.isDefault).id,
  });
  const researcher = await claudeBot(client, "Researcher", { canMessage: [writer.id] });
  await client.call("chat.send", {
    botId: researcher.id,
    text: "Use the message_bot tool to ask Writer to reply with exactly the word MANGO. When Writer's answer arrives, tell me what Writer said.",
  });
  const replied = await waitFor(
    () => client.events.findIndex((e) => e.event === "message.updated" && e.data.conversationId === researcher.id && e.data.kind === "handoff" && e.data.data.status === "replied") + 1,
  );
  await said(client, researcher.id, researcher.id, "MANGO", replied);
  const row = client.events.find((e) => e.event === "message.added" && e.data.conversationId === writer.id && e.data.kind === "handoff");
  const { exchange, messages } = await client.call("exchanges.get", { id: row.data.data.exchangeId });
  expect(exchange).toMatchObject({ status: "replied", fromBotId: researcher.id, toBotId: writer.id });
  expect(exchange.reply).toContain("MANGO");
  expect(messages[0]).toMatchObject({ author: researcher.id, kind: "text" });
  const room = await client.call("room.history", {});
  expect(room.filter((m: any) => m.data?.exchangeId === exchange.id).map((m: any) => [m.author, m.data.toBotId])).toEqual([
    [researcher.id, writer.id],
    [writer.id, researcher.id],
  ]);
  client.close();
});

it("a bot mentioned in the Team room answers there", async () => {
  const client = await connect(daemon.port, daemon.token);
  const bot = await claudeBot(client, "Roomy");
  await client.call("room.post", { text: "@Roomy reply with exactly: ROOM-OK" });
  await said(client, "team", bot.id, "ROOM-OK");
  await waitFor(() =>
    client.events.find((e) => e.event === "message.added" && e.data.conversationId === bot.id && e.data.kind === "handoff" && e.data.body === "Answered in the Team room"),
  );
  await expect(client.call("bots.showBrowser", { id: bot.id })).rejects.toThrow(/bad_params/); // its browser is off
  client.close();
});

it("a bot proposes a routine from chat, and approving the card creates it", async () => {
  const client = await connect(daemon.port, daemon.token);
  const bot = await claudeBot(client, "Planner");
  await client.call("chat.send", { botId: bot.id, text: "Use the add_routine tool to propose a routine named Morning check, on weekdays at 08:48, that checks the news. Then reply DONE." });
  const card = await waitFor(() => client.events.find((e) => e.event === "approval.added" && e.data.botId === bot.id && e.data.kind === "routine"));
  await client.call("approvals.respond", { approvalId: card.data.id, decision: "once" });
  expect(await client.call("routines.list", { botId: bot.id })).toMatchObject([{ name: expect.stringMatching(/morning check/i), days: [1, 2, 3, 4, 5], times: ["08:48"], enabled: true }]);
  client.close();
});

it("runs a routine when it's due, under a routine label", async () => {
  const client = await connect(daemon.port, daemon.token);
  const bot = await claudeBot(client, "Clockwork");
  const routine = await client.call("routines.create", { botId: bot.id, name: "Tick", prompt: "Reply with exactly: TICK", days: [0, 1, 2, 3, 4, 5, 6], times: ["03:00"] });
  daemon.scheduler.tick(Date.parse(routine.nextRunAt)); // as if the clock had reached the run time
  await said(client, bot.id, bot.id, "TICK");
  await waitFor(() => client.events.find((e) => e.event === "routine.run" && e.data.routineId === routine.id && e.data.status === "done"));
  const chat = await client.call("chat.history", { botId: bot.id });
  expect(chat[0]).toMatchObject({ kind: "routine", body: "Routine: Tick" });
  client.close();
});

it("a bot with the browser on reads a page in its own browser profile", async () => {
  const client = await connect(daemon.port, daemon.token);
  const bot = await claudeBot(client, "Surfer", { browser: true });
  await client.call("chat.send", { botId: bot.id, text: "Use your browser tools to open https://example.com and reply with the page title only." });
  await said(client, bot.id, bot.id, "Example Domain");
  expect(existsSync(join(bot.homeFolder, "chrome-profile", "Default"))).toBe(true);
  client.close();
});
```

- [ ] **Step 2: Run the type check to see it fail**

Run: `npm run typecheck`
Expected: FAIL. `daemon.scheduler` doesn't exist yet.

- [ ] **Step 3: Replace daemon.ts**

Replace `packages/daemon/src/daemon.ts` with:

```ts
import { TEAM_ROOM_ID, type ProviderSetup } from "@tjommie/protocol";
import { getApproval } from "./approvals.ts";
import { createBot, deleteBot, getStoredBot, listStoredBots, setPaused, updateBot } from "./bots.ts";
import { browserServer, showBrowser } from "./browser.ts";
import { history } from "./chat.ts";
import { openDb } from "./db.ts";
import { McpTokens, mcpHandler } from "./mcp.ts";
import { DEFAULT_PORT, dataPaths, readOrCreateToken } from "./paths.ts";
import { createRoutine, deleteRoutine, getRoutine, listRoutines, listRuns, updateRoutine } from "./routines.ts";
import { Runner, type SessionFactory } from "./runner.ts";
import { Scheduler } from "./scheduler.ts";
import { RpcError, startServer, type Broadcast, type Handlers } from "./server.ts";
import { getSettings, setSettings } from "./settings.ts";
import { checkSetup, ensureAccounts, listModels, signIn } from "./setup.ts";
import { Team, getExchange } from "./team.ts";

// The desktop app loads the UI from app://tjommie (plan 1D); `npm run dev` serves it on localhost:3000.
const ALLOWED_ORIGINS = ["app://tjommie", "http://localhost:3000"];

export type Daemon = { port: number; token: string; runner: Runner; scheduler: Scheduler; close: () => Promise<void> };

export async function startDaemon(options: { root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory } = {}): Promise<Daemon> {
  const paths = dataPaths(options.root);
  const db = openDb(paths.db);
  const token = readOrCreateToken(paths.token);
  ensureAccounts(db, options.accountsDir ?? paths.accounts);
  let setup: ProviderSetup[] = await checkSetup(db);
  let broadcast: Broadcast = () => {};
  const send: Broadcast = (event, data) => broadcast(event, data);
  let mcpUrl = ""; // set once the server listens; CLI sessions only start after that
  const tokens = new McpTokens();

  const runner = new Runner({
    db,
    broadcast: send,
    isSignedIn: (provider) => setup.some((s) => s.provider === provider && s.signedIn),
    createSession: options.createSession,
    hooks: {
      mcpServers: (bot) => [
        { name: "tjommie", url: mcpUrl, headers: { Authorization: `Bearer ${tokens.issue(bot.id)}` }, autoAllow: true },
        ...(bot.browser ? [browserServer(bot)] : []),
      ],
      sessionClosed: (botId) => tokens.revoke(botId),
      turnStarted: (item) => scheduler.runStarted(item),
      turnEnded: (item, result) => {
        scheduler.runEnded(item, result);
        team.turnEnded(item, result);
      },
    },
  });
  const team = new Team({
    db,
    broadcast: send,
    deliver: (work) => runner.deliver(work),
    currentWork: (botId) => runner.currentWork(botId),
    stateOf: (bot) => runner.toBot(bot).state,
  });
  const scheduler = new Scheduler({ db, broadcast: send, deliver: (work) => runner.deliver(work), stopItem: (itemId, notice) => runner.stopItem(itemId, notice) });

  const refreshSetup = async (): Promise<ProviderSetup[]> => {
    setup = await checkSetup(db);
    broadcast("setup.changed", setup);
    for (const bot of listStoredBots(db)) broadcast("bot.updated", runner.toBot(bot));
    runner.pumpAll();
    return setup;
  };

  const handlers: Handlers = {
    "setup.status": () => refreshSetup(),
    "setup.signIn": ({ provider }) => signIn(db, provider, () => void refreshSetup()),
    "models.list": ({ provider }) => listModels(db, provider),
    "bots.list": () => listStoredBots(db).map((bot) => runner.toBot(bot)),
    "bots.create": (input) => {
      const bot = runner.toBot(createBot(db, paths.bots, input));
      broadcast("bot.updated", bot);
      return bot;
    },
    "bots.update": ({ id, patch }) => {
      const before = getStoredBot(db, id);
      const bot = runner.toBot(updateBot(db, id, patch));
      runner.botChanged(id, before.provider !== bot.provider);
      broadcast("bot.updated", bot);
      return bot;
    },
    "bots.delete": ({ id }) => {
      team.botDeleted(id);
      runner.forget(id);
      deleteBot(db, id);
      broadcast("bot.deleted", { id });
      return {};
    },
    "bots.pause": ({ id, paused }) => {
      setPaused(db, id, paused);
      const bot = runner.toBot(getStoredBot(db, id));
      broadcast("bot.updated", bot);
      if (!paused) runner.pump(id);
      return bot;
    },
    "bots.showBrowser": ({ id }) => {
      const bot = getStoredBot(db, id);
      if (!bot.browser) throw new RpcError("bad_params", "This bot's browser is off");
      showBrowser(bot);
      return {};
    },
    "chat.history": ({ botId, before, limit }) => {
      getStoredBot(db, botId);
      return history(db, botId, before, limit);
    },
    "chat.send": ({ botId, text, files }) => runner.submit(botId, text, files),
    "chat.stop": async ({ botId }) => {
      await runner.stop(botId);
      return {};
    },
    "chat.retry": ({ messageId }) => runner.retry(messageId),
    "approvals.respond": ({ approvalId, decision }) =>
      getApproval(db, approvalId).kind === "routine" ? team.answerRoutineProposal(approvalId, decision) : runner.answerApproval(approvalId, decision),
    "room.history": ({ before, limit }) => history(db, TEAM_ROOM_ID, before, limit),
    "room.post": ({ text }) => team.postToRoom(text),
    "exchanges.get": ({ id }) => ({ exchange: getExchange(db, id), messages: history(db, id, undefined, 200) }),
    "routines.list": ({ botId }) => {
      getStoredBot(db, botId);
      return listRoutines(db, botId);
    },
    "routines.create": (input) => {
      const routine = createRoutine(db, input);
      broadcast("routine.updated", routine);
      return routine;
    },
    "routines.update": ({ id, patch }) => {
      const routine = updateRoutine(db, id, patch);
      broadcast("routine.updated", routine);
      return routine;
    },
    "routines.delete": ({ id }) => {
      deleteRoutine(db, id);
      broadcast("routine.deleted", { id });
      return {};
    },
    "routines.runs": ({ routineId }) => {
      getRoutine(db, routineId);
      return listRuns(db, routineId);
    },
    "settings.get": () => getSettings(db),
    "settings.set": (patch) => {
      const settings = setSettings(db, patch);
      broadcast("settings.changed", settings);
      return settings;
    },
  };

  const server = await startServer({
    port: options.port ?? DEFAULT_PORT,
    token,
    handlers,
    allowedOrigins: ALLOWED_ORIGINS,
    routes: { "/mcp": mcpHandler({ tokens, tools: team.tools() }) },
  });
  mcpUrl = `http://127.0.0.1:${server.port}/mcp`;
  broadcast = server.broadcast;
  runner.start();
  scheduler.start();
  return {
    port: server.port,
    token,
    runner,
    scheduler,
    close: async () => {
      scheduler.stop();
      runner.shutdown();
      await server.close();
      db.close();
    },
  };
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm run typecheck`
Expected: no errors.

Run: `npm test`
Expected: PASS.

Run: `npx vitest run --mode real packages/daemon/src/daemon.real.test.ts`
Expected: all 6 tests PASS. The bot-to-bot test uses a Claude bot and a Codex bot and takes a few minutes. The routine test ticks the scheduler at the routine's run time instead of waiting for it.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/daemon.ts packages/daemon/src/daemon.real.test.ts
git commit -m "Wire the MCP endpoint, Team room, routines and browser into the daemon

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Terminal chat, README and the full run

**Files:**
- Replace: `packages/daemon/scripts/chat.ts`
- Modify: `README.md` (the Develop section)

**Interfaces:**
- Consumes: the daemon from Task 10.
- Produces:
  - The terminal chat prints Team room posts, handoff rows, routine labels and file cards.
  - `/room <text>` posts in the Team room.

- [ ] **Step 1: Replace the terminal chat**

Replace `packages/daemon/scripts/chat.ts` with:

```ts
// Dev tool: chat with a bot through the running daemon before the desktop app exists.
//   npm run chat -w @tjommie/daemon -- "<bot name>" [claude|codex] [ask|folder|full]
// In the chat: /stop interrupts, /allow <id>, /always <id> and /deny <id> answer cards,
// /room <text> posts in the Team room, Ctrl+C quits.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { PROTOCOL_VERSION, TEAM_ROOM_ID } from "@tjommie/protocol";
import { DEFAULT_PORT, dataPaths } from "../src/paths.ts";

const [name = "Tester", provider = "claude", autonomy = "folder"] = process.argv.slice(2);
const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
const token = readFileSync(dataPaths().token, "utf8").trim();
const ws = new WebSocket(`ws://127.0.0.1:${port}`);
const pending = new Map<number, (message: any) => void>();
const names = new Map<string, string>([["system", "Tjommie"]]);
let nextId = 1;
let botId = "";
let welcomed: () => void = () => {};
const welcome = new Promise<void>((resolve) => (welcomed = resolve));

const call = (method: string, params: unknown): Promise<any> =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, (m) => (m.ok ? resolve(m.result) : reject(new Error(`${m.error.code}: ${m.error.message}`))));
    ws.send(JSON.stringify({ type: "req", id, method, params }));
  });

ws.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data));
  if (message.type === "welcome") return welcomed();
  if (message.type === "res") {
    pending.get(message.id)?.(message);
    pending.delete(message.id);
    return;
  }
  if (message.type !== "event") return;
  const d = message.data;
  const finished = (message.event === "message.added" || message.event === "message.updated") && d.status === "done";
  if (message.event === "bot.updated") names.set(d.id, d.name);
  else if (message.event === "message.delta" && d.conversationId === botId) process.stdout.write(d.delta);
  else if (message.event === "message.updated" && d.conversationId === botId && d.kind === "text" && d.author === botId && d.status !== "streaming") process.stdout.write("\n");
  else if (finished && d.conversationId === TEAM_ROOM_ID && d.author !== "you" && (d.kind === "text" || d.kind === "notice")) {
    const to = d.data?.toBotId ? ` to ${names.get(d.data.toBotId) ?? "a bot"}` : "";
    console.log(`  [Team room] ${names.get(d.author) ?? "A bot"}${to}: ${d.body}`);
  } else if (message.event === "message.added" && d.conversationId === botId && d.kind === "activity") console.log(`  [${d.body}]`);
  else if (message.event === "message.added" && d.conversationId === botId && ["notice", "error", "handoff", "routine", "file"].includes(d.kind)) console.log(`  (${d.body})`);
  else if (message.event === "approval.added" && d.botId === botId) console.log(`\n  Waiting for you: ${d.summary}\n  /allow ${d.id}   /always ${d.id}   /deny ${d.id}`);
});
ws.addEventListener("close", () => {
  console.log("Disconnected from the daemon.");
  process.exit(0);
});
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve);
  ws.addEventListener("error", reject);
});
ws.send(JSON.stringify({ type: "hello", protocol: PROTOCOL_VERSION, token }));
await welcome;

const bots: any[] = await call("bots.list", {});
for (const b of bots) names.set(b.id, b.name);
let bot = bots.find((b) => b.name === name);
if (!bot) {
  const models: any[] = await call("models.list", { provider });
  const model = (models.find((m) => m.isDefault) ?? models[0]).id;
  bot = await call("bots.create", { name, color: "blue", shape: "circle", instructions: "", provider, model, autonomy });
}
botId = bot.id;
names.set(bot.id, bot.name);
console.log(`Chatting with ${bot.name} (${bot.provider}, ${bot.autonomy}, ${bot.state}). /room <text> posts in the Team room, /stop interrupts, Ctrl+C quits.`);

const decisions: Record<string, string> = { "/allow": "once", "/always": "always", "/deny": "deny" };
for await (const line of createInterface({ input: process.stdin })) {
  const text = line.trim();
  const [command, arg] = text.split(/\s+/, 2);
  try {
    if (command === "/stop") await call("chat.stop", { botId });
    else if (command === "/room" && text.length > "/room ".length) await call("room.post", { text: text.slice("/room ".length) });
    else if (decisions[command] && arg) await call("approvals.respond", { approvalId: arg, decision: decisions[command] });
    else if (text) await call("chat.send", { botId, text: line });
  } catch (error) {
    console.log(`  (${(error as Error).message})`);
  }
}
```

- [ ] **Step 2: Mention it in the README**

In `README.md`, under "## Develop", add this line inside the code block, right after the `npm run chat` line:

```sh
#   in the chat: /room <text> posts in the Team room; /allow <id> answers a card, including a routine a bot proposes
```

- [ ] **Step 3: Run everything**

Run: `npm run typecheck`
Expected: no errors.

Run: `npm test`
Expected: PASS.

Run: `npm run test:real`
Expected: PASS. It needs plan 1A's one-time sign-in (`npm run signin -w @tjommie/daemon`) and takes several minutes.

- [ ] **Step 4: Try it by hand**

1. Start the daemon: `npm start -w @tjommie/daemon`.
2. Open two more terminals:
   - `npm run chat -w @tjommie/daemon -- "Researcher" claude`
   - `npm run chat -w @tjommie/daemon -- "Writer" codex`
3. In Researcher's terminal, type `/room @Writer say hello to the room in five words`. Both terminals print Writer's answer as a Team room post, and Writer's terminal shows "(Answered in the Team room)".
4. In Researcher's terminal, type `Use add_routine to propose a routine: weekdays at 08:00, check the news.`
   - A "Waiting for you: Add a routine: …" card appears.
   - `/allow <id>` creates the routine.
   - Ctrl+C both chats and stop the daemon.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/scripts/chat.ts README.md
git commit -m "Show Team room posts in the terminal chat and add /room

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Not measured yet

The executing agent should look out for these. None of them blocks a task.

- **A Codex bot set to Always ask, with its browser on.** Its thread uses the `read-only` sandbox. Spike 5 ran Playwright only under `workspace-write`. If Chrome can't write its profile there, the fix belongs in `codexConfig`.
- **"Show window" while the bot's own browser is open.** Chrome normally opens another window of a profile that's already running. That wasn't tried against a Playwright-launched browser. Plan 1D's manual demo covers it.
- **Real CLI wording.** The real tests ask haiku and Codex's default model to use a tool or say a word. If a model changes its phrasing, adjust the prompt, not the code.