# Tjommie Phase 1A: Daemon Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A headless Tjommie daemon you can drive over its local WebSocket API. It creates Claude and Codex bots, streams their replies, lets you steer and stop them, and turns their permission requests into approval cards, with everything stored in SQLite.

**Architecture:** One Node 24 process (`packages/daemon`) owns a SQLite database and serves typed JSON messages over WebSocket on 127.0.0.1. The message schemas are zod, in `packages/protocol`. For each busy bot the daemon runs one official CLI process: `claude -p` speaking stream-json, or `codex app-server` speaking JSON-RPC. A small adapter per CLI translates its output into one shared event type. Each provider runs from a Tjommie-owned config folder that the user signs in to once, so bots never load the user's personal CLI setup.

**Tech Stack:** Node 24 (type stripping, `node:sqlite`), TypeScript 5.9, zod 4.6, ws 8.21, vitest 5, Claude Code 2.1.280, Codex CLI 0.155.1.

**Spec:** `docs/superpowers/specs/2026-09-25-tjommie-design.md`. Read its "Changes after the Phase 1 spikes" section first.

## Where this plan sits

Phase 1 is split into four plans. Each is written just before it starts, because each depends on what the one before it built.

| Plan | Delivers | Needs |
|---|---|---|
| **1A Daemon core (this plan)** | Bots, chat, both CLIs, steering, Stop, approvals, one-time sign-in, usage records, a terminal chat tool | nothing |
| 1B Bots working together | App MCP server (`list_bots`, `message_bot`, `share_file`, `add_routine`), exchanges and chain limit, Team room, routines and scheduler, browser per bot, notifications | 1A |
| 1C Screen designs | OpenPencil designs: Team room, exchange view, routine editor, bot settings, first-run sign-in, Phase 1 right panel | nothing; can run alongside 1A |
| 1D Desktop app and UI | Electron shell (starts the daemon, tray, notifications), every Phase 1 screen, Windows installer, screenshot checks, the Phase 1 demo | 1A, 1B, 1C |

## Spike findings this plan relies on

All of these were measured on this PC on 2026-09-25 with throwaway harnesses. Real output lines from those runs are the parser test data in Tasks 9 and 11.

**Claude Code 2.1.280**
- **Flags:** `-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --permission-mode <manual|acceptEdits|bypassPermissions> --model <alias> --append-system-prompt <text> --strict-mcp-config [--add-dir <dir>] [--resume <id>] [--allowedTools <rules...>]`. The old `default` permission mode is now called `manual`.
- **Output lines:**
  - Every turn, including one Claude starts on its own, begins with `{"type":"system","subtype":"init","session_id":…}`.
  - Text streams as `stream_event` → `content_block_delta` → `text_delta`.
  - Complete `assistant` lines carry `text` and `tool_use` blocks, one block per line.
  - `user` lines carry `tool_result`.
  - A `result` line ends the turn.
- **Cost:** `result.total_cost_usd` is cumulative for the process (0.018, then 0.023, then 0.043 over three turns), while `result.usage` is per turn.
- **Stopped turns** end with `subtype: "error_during_execution"`, `is_error: true`, `terminal_reason: "aborted_tools"`.
- **Approvals:**
  - Claude writes `{"type":"control_request","request_id":…,"request":{"subtype":"can_use_tool","tool_name":…,"input":…,"description":…,"permission_suggestions":[…]}}`.
  - We answer `{"type":"control_response","response":{"subtype":"success","request_id":…,"response":{"behavior":"allow","updatedInput":…}}}` or `{"behavior":"deny","message":…}`.
  - Answering 70 s later was fine.
  - An allow that includes `updatedPermissions: [{type:"addRules", rules:[{toolName:"Write"}], behavior:"allow", destination:"session"}]` stops Claude asking again for that tool in the same session.
  - `echo` and `sleep` never ask, because Claude treats them as read-only.
- **`--strict-mcp-config` without `--mcp-config`** is accepted; the session simply has no MCP servers.
- **Steering:** a `{"type":"user",…}` line written mid-turn is read at the next step of the same turn.
- **Stop:** `{"type":"control_request","request_id":"x","request":{"subtype":"interrupt"}}` gets an immediate `control_response`, then the turn's `result`. When a background task is stopped, Claude may run a short turn on its own afterwards.
- **Limits:** `rate_limit_event.rate_limit_info` has `status` (`allowed`, `allowed_warning`, `rejected`), `resetsAt` (Unix seconds), `rateLimitType`, and `unifiedWindows.<type>.utilization` (0 to 1).
- **The user's own `~/.claude` loaded their hooks and plugins:** 66 s before `init` and about 28k extra prompt tokens. A Tjommie-owned `CLAUDE_CONFIG_DIR` starts in about 1 s.
- **Sign-in:** `claude auth status` prints JSON (`loggedIn`, `email`, `subscriptionType`), and `claude auth login` signs in.

**Codex CLI 0.155.1** (`codex app-server`: JSON lines, no `jsonrpc` field)
- **Launching:** npm installs `codex.cmd` and `codex.ps1`, which Windows can't spawn without a shell. We run `node <npm dir>\node_modules\@openai\codex\bin\codex.js app-server` instead.
- **Handshake and account:**
  - `initialize {clientInfo}`, then the notification `initialized`.
  - `account/read` returns `{account: {type, email, planType} | null, requiresOpenaiAuth}`.
  - `account/login/start {type:"chatgpt"}` returns `{authUrl, loginId}`; success arrives later as the notification `account/login/completed {success}`.
  - `model/list` returns `data[]` with `model`, `displayName`, `hidden` and `isDefault`.
- **Threads and turns:**
  - `thread/start` and `thread/resume {threadId}` take `cwd`, `model`, `approvalPolicy`, `sandbox`, `developerInstructions` and `config`, and return `{thread: {id}, model}`.
  - `turn/start {threadId, input: [{type:"text", text}]}` returns `{turn: {id}}`.
  - `turn/steer {threadId, expectedTurnId, input}` and `turn/interrupt {threadId, turnId}`.
- **Notifications we use:**
  - `turn/started`, `item/started`, `item/completed`, and `item/agentMessage/delta {delta}`.
  - `thread/tokenUsage/updated {tokenUsage.last}`. Its `inputTokens` includes `cachedInputTokens`.
  - `account/rateLimits/updated {rateLimits.primary.usedPercent, resetsAt, rateLimitReachedType}`.
  - `turn/completed {turn.status, turn.error.codexErrorInfo}`.
- **Approvals** arrive as server requests: `item/commandExecution/requestApproval` (with `commandActions` and `proposedExecpolicyAmendment`) and `item/fileChange/requestApproval`. We answer `{decision: "accept"}` or `{decision: "decline"}`.
- **Per-thread `config` merges with the account's `config.toml`.** It can't remove MCP servers that come from plugins; a Tjommie-owned `CODEX_HOME` has none to remove.
- **Windows sandbox:** `config.windows.sandbox = "unelevated"` needs no admin. Writes inside the bot's folder go through; a write outside asks. Shell network commands fail inside the sandbox with a TLS credentials error even with `network_access: true`, so they ask too.
- **Fresh `CODEX_HOME`:** `account/read` returns `account: null`, and `model/list` works before sign-in.

**Platform**
- **Electron as Node:** Electron 44.4.5 run with `ELECTRON_RUN_AS_NODE=1` is Node 24.21, and runs `.ts` files and `node:sqlite` directly.
- **System Node:** Node 24.14 prints an ExperimentalWarning for `node:sqlite`. Scripts pass `--disable-warning=ExperimentalWarning`.

## Global Constraints

- **TypeScript:** Node 24+ runs it by type stripping, so use only erasable syntax (no `enum`, `namespace` or constructor parameter properties). Relative imports end in `.ts`. Type-only imports use `import type`.
- **npm:** run `npm` and `npx` from the repo root in PowerShell, not the Bash tool. Never `cd` into a package before `npm install`.
- **Dependencies:** add exactly these and nothing else: `zod@^4.6.5` (protocol, daemon), `ws@^8.21.3` and `@types/ws@^8.18.1` (daemon). No native modules.
- **Local security:** the daemon binds 127.0.0.1 only and needs the token on every connection. It rejects Host headers other than `127.0.0.1:<port>` or `localhost:<port>`, and Origins other than `app://tjommie` and `http://localhost:3000`. A missing Origin is allowed (non-browser clients).
- **Port and data root:** default port 47100 (`TJOMMIE_PORT` overrides). Data root `%LOCALAPPDATA%\Tjommie` (`TJOMMIE_HOME` overrides).
- **Provider rules (spec §10):**
  - Run the unmodified official CLIs; never the Claude Agent SDK.
  - Sign in only through the providers' own flows.
  - Never read, copy, store or send CLI credential files or tokens.
  - Strip `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CONFIG_DIR`, `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `OPENAI_API_KEY`, `CODEX_API_KEY` and `CODEX_HOME` from every inherited environment before adding our own.
- **Account folders:** each bot's CLI runs in a Tjommie-owned account folder, `accounts\claude-main` (as `CLAUDE_CONFIG_DIR`) or `accounts\codex-main` (as `CODEX_HOME`).
- **Runtime rules:** one turn at a time per bot. Your messages steer a busy bot. A CLI process idle for 10 minutes is closed. A fresh session gets a recap of the last 20 chat messages.
- **Autonomy mapping:**
  - ask: Claude `manual`; Codex `untrusted` + `read-only`.
  - folder: Claude `acceptEdits`; Codex `on-request` + `workspace-write`.
  - full: Claude `bypassPermissions`; Codex `never` + `danger-full-access`.
- **Codex thread config:** always sets `sandbox_workspace_write.network_access = true`, `sandbox_workspace_write.writable_roots = <extra dirs>`, and on Windows `windows.sandbox = "unelevated"`.
- **Visible text:** notices and errors shown in the chat use no em dashes, en dashes or emoji.
- **Commits:**
  - **Message:** imperative sentence case, like the existing history ("Add …", "Scaffold …"), then a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  - **Identity:** the repo already has a local TSnyders identity configured.
  - **Pushing:** don't push in this plan.
- **Tests:** `npm test` runs the fast `*.test.ts` files. `npm run test:real` runs the `*.real.test.ts` files against the real CLIs; they need the one-time sign-in from Task 6 and spend a few tokens. The user asked for real CLIs instead of a fake CLI.

## Review Focus

These are the five failure modes the spec implies but ordinary feature tests wouldn't hit, most likely first. Each one has a test in the task that owns the code.

1. **A CLI process dies in the middle of a turn** (crash, antivirus, the PC sleeping). The bot must go back to Idle, with the turn marked failed and a Retry offered; it must never stay Working. *Test: Task 14, "CLI killed mid-turn".*
2. **Awkward text reaches a CLI.** Quotes, newlines, `%`, `^`, `&`, emoji and accented letters in bot instructions and messages must arrive unchanged. *Tests: Task 10, "argument round trip". Messages travel as JSON on stdin (Tasks 10 and 11).*
3. **Windows paths with spaces or accents,** such as a profile like `C:\Users\Thabo Mokoena`. Bot, account and project folders must all work. *Tests: Task 2 and Task 7 use a spaced, accented root; so do the Task 14 real tests.*
4. **Two messages sent back to back to an idle bot.** Exactly one CLI session and one turn start; the second message steers or waits. *Test: Task 14, "double send".*
5. **The daemon restarts with work in flight.** Running items become Interrupted with a Retry, pending approvals close, and queued items continue. *Tests: Task 13, "recovery after restart", and Task 14's real "resume after restart".*

---

### Task 1: Protocol messages

**Files:**
- Modify: `package.json` (root), `packages/protocol/package.json`, `packages/protocol/src/index.ts`
- Create: `vitest.config.ts`, `packages/protocol/src/model.ts`, `packages/protocol/src/wire.ts`
- Test: `packages/protocol/src/wire.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces, all exported from `@tjommie/protocol`:
  - **Version:** `PROTOCOL_VERSION` (`1`).
  - **Schemas, each with a same-named type:** `Provider` (`"claude" | "codex"`), `Autonomy` (`"ask" | "folder" | "full"`), `BotColor`, `BotShape`, `BotState` (`"idle" | "working" | "waiting" | "paused" | "limited" | "needs_setup"`), `Bot`, `Message`, `MessageKind` (`"text" | "activity" | "approval" | "notice" | "error"`), `MessageStatus` (`"queued" | "sent" | "streaming" | "done" | "failed" | "interrupted"`), `ApprovalDecision` (`"once" | "always" | "deny"`), `Approval`, `ProviderSetup`, `ModelInfo`.
  - **Input schemas:** `BotInput` and `BotPatch`. Their types are the parsed output.
  - **Method and event tables:** `methods` and `events`.
  - **Wire messages:** `ClientMessage` (zod) and the `ServerMessage` type.
  - **Helper types:** `MethodName`, `ParamsIn<M>`, `ParamsOut<M>`, `Result<M>`, `EventName`, `EventData<E>`.

- [ ] **Step 1: Add zod to the protocol package**

Run (PowerShell, repo root): `npm install zod@^4.6.5 -w @tjommie/protocol`
Expected: `packages/protocol/package.json` gains `"dependencies": { "zod": "^4.6.5" }`.

- [ ] **Step 2: Split fast and real-CLI tests**

Create `vitest.config.ts`:

```ts
import { defineConfig } from "vitest/config";

// `npm test` runs fast tests. `npm run test:real` (mode "real") runs the tests that drive the real CLIs.
export default defineConfig(({ mode }) => {
  const real = mode === "real";
  return {
    test: {
      include: [real ? "packages/*/src/**/*.real.test.ts" : "packages/*/src/**/*.test.ts"],
      exclude: real ? ["**/node_modules/**"] : ["**/node_modules/**", "**/*.real.test.ts"],
      testTimeout: real ? 600_000 : 10_000,
      hookTimeout: real ? 120_000 : 10_000,
      fileParallelism: !real,
    },
  };
});
```

In the root `package.json`, replace the `"test"` script and add `"test:real"` so the scripts block reads:

```json
  "scripts": {
    "dev": "npm run dev -w @tjommie/ui",
    "desktop": "npm start -w @tjommie/desktop",
    "build": "npm run build -w @tjommie/ui",
    "lint": "npm run lint -w @tjommie/ui",
    "typecheck": "tsc -p . && npm run typecheck -w @tjommie/ui",
    "test": "vitest run --passWithNoTests",
    "test:real": "vitest run --mode real --passWithNoTests"
  },
```

- [ ] **Step 3: Write the failing test**

Create `packages/protocol/src/wire.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { BotInput, BotPatch, ClientMessage, PROTOCOL_VERSION, methods } from "./index.ts";

const bot = { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" } as const;

describe("protocol", () => {
  it("is version 1", () => {
    expect(PROTOCOL_VERSION).toBe(1);
  });

  it("parses hello and request messages and rejects anything else", () => {
    expect(ClientMessage.parse({ type: "hello", protocol: 1, token: "t" }).type).toBe("hello");
    expect(ClientMessage.parse({ type: "req", id: 1, method: "bots.list", params: {} }).type).toBe("req");
    expect(() => ClientMessage.parse({ type: "nope" })).toThrow();
  });

  it("trims bot names and rejects blank ones", () => {
    expect(BotInput.parse({ ...bot, name: "  Writer " }).name).toBe("Writer");
    expect(() => BotInput.parse({ ...bot, name: "   " })).toThrow();
  });

  it("leaves unspecified fields out of a patch so they stay unchanged", () => {
    expect(BotPatch.parse({ autonomy: "full" })).toEqual({ autonomy: "full" });
  });

  it("applies chat.send defaults and rejects blank text", () => {
    expect(methods["chat.send"].params.parse({ botId: "b", text: " hi " })).toEqual({ botId: "b", text: "hi", files: [] });
    expect(() => methods["chat.send"].params.parse({ botId: "b", text: "  " })).toThrow();
  });
});
```

- [ ] **Step 4: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `BotInput`, `BotPatch`, `ClientMessage` and `methods` are not exported.

- [ ] **Step 5: Write the schemas**

Create `packages/protocol/src/model.ts`:

```ts
import { z } from "zod";

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

export const MessageKind = z.enum(["text", "activity", "approval", "notice", "error"]);
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

export const Approval = z.object({
  id: z.string(),
  botId: z.string(),
  tool: z.string(),
  summary: z.string(),
  detail: z.string(),
  rule: z.string().nullable(),
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
```

Create `packages/protocol/src/wire.ts`:

```ts
import { z } from "zod";
import { Approval, ApprovalDecision, Bot, BotInput, BotPatch, Message, ModelInfo, Provider, ProviderSetup } from "./model.ts";

export const PROTOCOL_VERSION = 1;

const Empty = z.object({});
const Id = z.object({ id: z.string() });

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
  "chat.history": {
    params: z.object({ botId: z.string(), before: z.string().optional(), limit: z.number().int().min(1).max(200).default(50) }),
    result: z.array(Message),
  },
  "chat.send": {
    params: z.object({ botId: z.string(), text: z.string().trim().min(1).max(100_000), files: z.array(z.string()).max(20).default([]) }),
    result: Message,
  },
  "chat.stop": { params: z.object({ botId: z.string() }), result: Empty },
  "chat.retry": { params: z.object({ messageId: z.string() }), result: Message },
  "approvals.respond": { params: z.object({ approvalId: z.string(), decision: ApprovalDecision }), result: Approval },
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

Replace `packages/protocol/src/index.ts` with:

```ts
export * from "./model.ts";
export * from "./wire.ts";
```

- [ ] **Step 6: Run the tests, the type check and the UI build**

Run: `npm test`
Expected: PASS (5 tests).

Run: `npm run typecheck`
Expected: no errors.

Run: `npm run build`
Expected: the Next.js static export still builds, because `packages/ui/app/page.tsx` imports `PROTOCOL_VERSION`. This confirms Next resolves the protocol's `.ts` imports.

- [ ] **Step 7: Commit**

```bash
git add vitest.config.ts package.json package-lock.json packages/protocol
git commit -m "Add protocol messages for the daemon API

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Daemon storage

**Files:**
- Modify: `packages/daemon/package.json`, `tsconfig.json` (root)
- Create: `packages/daemon/src/paths.ts`, `packages/daemon/src/db.ts`
- Test: `packages/daemon/src/db.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `paths.ts`: `DEFAULT_PORT` (`47100`), `type Paths = { root; db; token; bots; accounts; logs }` (all `string`), `dataPaths(root?: string): Paths` (creates the folders), `readOrCreateToken(file: string): string`.
  - `db.ts`: `type Db = DatabaseSync`, `openDb(file: string): Db` (runs migrations), `transaction(db: Db, work: () => void): void`, `newId(): string`, `nowIso(): string`.
  - Tables (later tasks rely on these names and columns): `accounts`, `bots`, `bot_links`, `allow_rules`, `conversations`, `messages`, `work_items`, `sessions`, `approvals`, `usage`.

- [ ] **Step 1: Add zod to the daemon and set up its scripts**

Run (PowerShell, repo root): `npm install zod@^4.6.5 -w @tjommie/daemon`

Then make `packages/daemon/package.json` read exactly:

```json
{
  "name": "@tjommie/daemon",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "start": "node --disable-warning=ExperimentalWarning src/main.ts",
    "signin": "node --disable-warning=ExperimentalWarning scripts/signin.ts",
    "chat": "node --disable-warning=ExperimentalWarning scripts/chat.ts"
  },
  "dependencies": {
    "@tjommie/protocol": "*",
    "zod": "^4.6.5"
  }
}
```

In the root `tsconfig.json`, change `"include"` to:

```json
  "include": ["packages/protocol/src", "packages/daemon/src", "packages/daemon/scripts", "packages/desktop/src"]
```

- [ ] **Step 2: Write the failing test**

Create `packages/daemon/src/db.test.ts`:

```ts
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { dataPaths, readOrCreateToken } from "./paths.ts";

// A root with a space and an accented letter, like a real Windows profile path.
const root = () => mkdtempSync(join(tmpdir(), "tjommie test ñ-"));

describe("openDb", () => {
  it("creates the schema once and keeps data when reopened", () => {
    const file = join(root(), "t.db");
    const db = openDb(file);
    db.prepare("INSERT INTO accounts (id, provider, label, kind, folder, created_at) VALUES ('a', 'claude', 'Claude', 'subscription', 'x', 'now')").run();
    db.close();
    const again = openDb(file);
    expect(again.prepare("SELECT label FROM accounts WHERE id = 'a'").get()).toEqual({ label: "Claude" });
    expect(again.prepare("PRAGMA user_version").get()).toEqual({ user_version: 1 });
  });

  it("enforces foreign keys", () => {
    const db = openDb(join(root(), "t.db"));
    expect(() => db.prepare("INSERT INTO conversations (id, kind, bot_id) VALUES ('c', 'bot', 'missing')").run()).toThrow(/FOREIGN KEY/);
  });
});

describe("dataPaths", () => {
  it("creates the folders under a root with spaces and accents", () => {
    const p = dataPaths(root());
    for (const dir of [p.bots, p.accounts, p.logs]) expect(existsSync(dir)).toBe(true);
  });

  it("creates a token once and returns the same one afterwards", () => {
    const p = dataPaths(root());
    const token = readOrCreateToken(p.token);
    expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(readOrCreateToken(p.token)).toBe(token);
  });
});
```

- [ ] **Step 3: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./db.ts` and `./paths.ts` don't exist.

- [ ] **Step 4: Write paths.ts and db.ts**

Create `packages/daemon/src/paths.ts`:

```ts
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";

export const DEFAULT_PORT = 47100;

export type Paths = { root: string; db: string; token: string; bots: string; accounts: string; logs: string };

const defaultRoot = () => process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");

export function dataPaths(root: string = defaultRoot()): Paths {
  const p = {
    root,
    db: join(root, "tjommie.db"),
    token: join(root, "daemon-token"),
    bots: join(root, "bots"),
    accounts: join(root, "accounts"),
    logs: join(root, "logs"),
  };
  for (const dir of [p.root, p.bots, p.accounts, p.logs]) mkdirSync(dir, { recursive: true });
  return p;
}

// ponytail: on Windows the file mode is ignored; the per-user ACL of %LOCALAPPDATA% protects the token.
export function readOrCreateToken(file: string): string {
  if (existsSync(file)) return readFileSync(file, "utf8").trim();
  const token = randomBytes(32).toString("base64url");
  writeFileSync(file, token, { mode: 0o600 });
  return token;
}
```

Create `packages/daemon/src/db.ts`:

```ts
import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

export type Db = DatabaseSync;
export const newId = (): string => randomUUID();
export const nowIso = (): string => new Date().toISOString();

// Append-only: never edit a shipped migration, add a new one.
const migrations = [
  `
  CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    kind TEXT NOT NULL,
    folder TEXT NOT NULL,
    created_at TEXT NOT NULL
  );
  CREATE TABLE bots (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    shape TEXT NOT NULL,
    instructions TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    autonomy TEXT NOT NULL,
    home_folder TEXT NOT NULL,
    project_folder TEXT,
    browser INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0,
    limited_until TEXT,
    created_at TEXT NOT NULL
  );
  CREATE TABLE bot_links (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    target_bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    PRIMARY KEY (bot_id, target_bot_id)
  );
  CREATE TABLE allow_rules (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    rule TEXT NOT NULL,
    PRIMARY KEY (bot_id, rule)
  );
  CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    bot_id TEXT REFERENCES bots(id) ON DELETE CASCADE
  );
  CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    author TEXT NOT NULL,
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    data TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    seq INTEGER NOT NULL
  );
  CREATE INDEX messages_by_conversation ON messages (conversation_id, seq);
  CREATE TABLE work_items (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    text TEXT NOT NULL,
    message_id TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT
  );
  CREATE INDEX work_items_by_bot ON work_items (bot_id, status);
  CREATE TABLE sessions (
    bot_id TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    account_id TEXT NOT NULL,
    cli_session_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
  );
  CREATE TABLE approvals (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    tool TEXT NOT NULL,
    summary TEXT NOT NULL,
    detail TEXT NOT NULL,
    rule TEXT,
    status TEXT NOT NULL,
    message_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    decided_at TEXT
  );
  CREATE TABLE usage (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input INTEGER NOT NULL,
    cached_input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    reasoning INTEGER NOT NULL,
    cost_usd REAL,
    cost_source TEXT,
    created_at TEXT NOT NULL
  );
  `,
];

export function transaction(db: Db, work: () => void): void {
  db.exec("BEGIN");
  try {
    work();
    db.exec("COMMIT");
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}

export function openDb(file: string): Db {
  const db = new DatabaseSync(file);
  db.exec("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
  const { user_version } = db.prepare("PRAGMA user_version").get() as { user_version: number };
  for (let version = user_version; version < migrations.length; version++) {
    transaction(db, () => {
      db.exec(migrations[version]);
      db.exec(`PRAGMA user_version = ${version + 1}`);
    });
  }
  return db;
}
```

`usage.bot_id` deliberately has no foreign key: usage history outlives deleted bots.

- [ ] **Step 5: Run the tests and the type check**

Run: `npm test`
Expected: PASS (protocol 5, db 4).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add package-lock.json tsconfig.json packages/daemon
git commit -m "Add daemon data folder and SQLite schema

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Local WebSocket server

**Files:**
- Modify: `packages/daemon/package.json` (npm adds ws)
- Create: `packages/daemon/src/server.ts`
- Test: `packages/daemon/src/server.test.ts`

**Interfaces:**
- Consumes: `@tjommie/protocol` (`ClientMessage`, `PROTOCOL_VERSION`, `methods`, and the types `ServerMessage`, `MethodName`, `ParamsOut`, `Result`, `EventName`, `EventData`).
- Produces:
  - `class RpcError extends Error { code: string }` (`new RpcError(code, message)`).
  - `type Handlers = { [M in MethodName]?: (params: ParamsOut<M>) => Result<M> | Promise<Result<M>> }`.
  - `type Broadcast = <E extends EventName>(event: E, data: EventData<E>) => void`.
  - `type Server = { port: number; broadcast: Broadcast; close(): Promise<void> }`.
  - `startServer(opts: { port: number; token: string; handlers: Handlers; allowedOrigins: string[] }): Promise<Server>`. Pass port `0` for a random free port.
  - Wire behavior:
    - The first message must be `hello` with the token and protocol version. The daemon answers `{type:"welcome", protocol: 1}`.
    - Close codes: 4401 for a wrong or missing token or no hello within 5 s; 4003 for a wrong protocol version; 4400 for malformed JSON.
    - Error codes: `unknown_method`, `bad_params` (zod), `internal`, or the `RpcError` code.

- [ ] **Step 1: Add ws**

Run (PowerShell, repo root): `npm install ws@^8.21.3 -w @tjommie/daemon` then `npm install -D @types/ws@^8.18.1 -w @tjommie/daemon`

- [ ] **Step 2: Write the failing test**

Create `packages/daemon/src/server.test.ts`:

```ts
import { afterEach, describe, expect, it } from "vitest";
import WebSocket from "ws";
import { RpcError, startServer, type Server } from "./server.ts";

let server: Server | undefined;
afterEach(async () => {
  await server?.close();
  server = undefined;
});

function connect(port: number, headers: Record<string, string> = {}) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/`, { headers });
  const inbox: any[] = [];
  const waiting: ((message: any) => void)[] = [];
  ws.on("message", (raw) => {
    const message = JSON.parse(String(raw));
    const waiter = waiting.shift();
    if (waiter) waiter(message);
    else inbox.push(message);
  });
  return {
    ws,
    opened: new Promise<void>((resolve, reject) => {
      ws.on("open", () => resolve());
      ws.on("error", reject);
    }),
    closed: new Promise<number>((resolve) => ws.on("close", (code) => resolve(code))),
    next: () => new Promise<any>((resolve) => (inbox.length ? resolve(inbox.shift()) : waiting.push(resolve))),
    send: (message: unknown) => ws.send(JSON.stringify(message)),
  };
}

async function hello(client: ReturnType<typeof connect>, token: string) {
  await client.opened;
  client.send({ type: "hello", protocol: 1, token });
  return client.next();
}

describe("startServer", () => {
  it("answers requests after a valid hello", async () => {
    server = await startServer({ port: 0, token: "secret", allowedOrigins: [], handlers: { "bots.list": () => [] } });
    const client = connect(server.port);
    expect(await hello(client, "secret")).toEqual({ type: "welcome", protocol: 1 });
    client.send({ type: "req", id: 7, method: "bots.list", params: {} });
    expect(await client.next()).toEqual({ type: "res", id: 7, ok: true, result: [] });
    client.ws.close();
  });

  it("closes with 4401 on a wrong token and 4003 on a wrong protocol", async () => {
    server = await startServer({ port: 0, token: "secret", allowedOrigins: [], handlers: {} });
    const wrongToken = connect(server.port);
    await wrongToken.opened;
    wrongToken.send({ type: "hello", protocol: 1, token: "nope" });
    expect(await wrongToken.closed).toBe(4401);
    const wrongProtocol = connect(server.port);
    await wrongProtocol.opened;
    wrongProtocol.send({ type: "hello", protocol: 99, token: "secret" });
    expect(await wrongProtocol.closed).toBe(4003);
  });

  it("reports bad params and handler errors without closing the connection", async () => {
    server = await startServer({
      port: 0,
      token: "t",
      allowedOrigins: [],
      handlers: {
        "bots.delete": () => {
          throw new RpcError("not_found", "No such bot");
        },
      },
    });
    const client = connect(server.port);
    await hello(client, "t");
    client.send({ type: "req", id: 1, method: "bots.delete", params: {} });
    expect(await client.next()).toMatchObject({ id: 1, ok: false, error: { code: "bad_params" } });
    client.send({ type: "req", id: 2, method: "bots.delete", params: { id: "x" } });
    expect(await client.next()).toEqual({ type: "res", id: 2, ok: false, error: { code: "not_found", message: "No such bot" } });
    client.send({ type: "req", id: 3, method: "bots.nope", params: {} });
    expect(await client.next()).toMatchObject({ id: 3, ok: false, error: { code: "unknown_method" } });
    client.ws.close();
  });

  it("refuses a foreign Origin and a foreign Host, and accepts the app origin", async () => {
    server = await startServer({ port: 0, token: "t", allowedOrigins: ["app://tjommie"], handlers: {} });
    await expect(connect(server.port, { Origin: "https://evil.example" }).opened).rejects.toThrow(/403/);
    await expect(connect(server.port, { Host: `evil.example:${server.port}` }).opened).rejects.toThrow(/403/);
    const app = connect(server.port, { Origin: "app://tjommie" });
    await app.opened;
    app.ws.close();
  });

  it("broadcasts events to authenticated clients", async () => {
    server = await startServer({ port: 0, token: "t", allowedOrigins: [], handlers: {} });
    const client = connect(server.port);
    await hello(client, "t");
    server.broadcast("bot.deleted", { id: "b1" });
    expect(await client.next()).toEqual({ type: "event", event: "bot.deleted", data: { id: "b1" } });
    client.ws.close();
  });
});
```

- [ ] **Step 3: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./server.ts` doesn't exist.

- [ ] **Step 4: Write server.ts**

Create `packages/daemon/src/server.ts`:

```ts
import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage } from "node:http";
import { WebSocketServer, type WebSocket } from "ws";
import { ZodError } from "zod";
import {
  ClientMessage,
  PROTOCOL_VERSION,
  methods,
  type EventData,
  type EventName,
  type MethodName,
  type ParamsOut,
  type Result,
  type ServerMessage,
} from "@tjommie/protocol";

export class RpcError extends Error {
  code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

export type Handlers = { [M in MethodName]?: (params: ParamsOut<M>) => Result<M> | Promise<Result<M>> };
export type Broadcast = <E extends EventName>(event: E, data: EventData<E>) => void;
export type Server = { port: number; broadcast: Broadcast; close: () => Promise<void> };

const HELLO_TIMEOUT_MS = 5000;

export async function startServer(opts: { port: number; token: string; handlers: Handlers; allowedOrigins: string[] }): Promise<Server> {
  const clients = new Set<WebSocket>();
  let port = opts.port;

  const http = createServer((req, res) => {
    if (req.url === "/health") {
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ ok: true, protocol: PROTOCOL_VERSION }));
      return;
    }
    res.writeHead(404).end();
  });
  const wss = new WebSocketServer({ noServer: true, maxPayload: 16 * 1024 * 1024 });

  http.on("upgrade", (req, socket, head) => {
    if (!isLocalHost(req, port) || !originAllowed(req, opts.allowedOrigins)) {
      socket.end("HTTP/1.1 403 Forbidden\r\n\r\n");
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => accept(ws));
  });

  const send = (ws: WebSocket, message: ServerMessage) => ws.send(JSON.stringify(message));

  function accept(ws: WebSocket): void {
    let authed = false;
    const timer = setTimeout(() => ws.close(4401, "hello required"), HELLO_TIMEOUT_MS);
    ws.on("close", () => {
      clearTimeout(timer);
      clients.delete(ws);
    });
    ws.on("message", async (raw) => {
      let message: ClientMessage;
      try {
        message = ClientMessage.parse(JSON.parse(String(raw)));
      } catch {
        ws.close(4400, "bad message");
        return;
      }
      if (!authed) {
        if (message.type !== "hello" || !sameToken(message.token, opts.token)) {
          ws.close(4401, "unauthorized");
          return;
        }
        if (message.protocol !== PROTOCOL_VERSION) {
          ws.close(4003, `protocol ${PROTOCOL_VERSION} required`);
          return;
        }
        authed = true;
        clearTimeout(timer);
        clients.add(ws);
        send(ws, { type: "welcome", protocol: PROTOCOL_VERSION });
        return;
      }
      if (message.type !== "req") return;
      try {
        send(ws, { type: "res", id: message.id, ok: true, result: await call(message.method, message.params) });
      } catch (error) {
        const code = error instanceof RpcError ? error.code : error instanceof ZodError ? "bad_params" : "internal";
        send(ws, { type: "res", id: message.id, ok: false, error: { code, message: error instanceof Error ? error.message : String(error) } });
      }
    });
  }

  async function call(method: string, params: unknown): Promise<unknown> {
    // Params are validated below before the call, so the handler table can be called untyped here.
    const handler = Object.hasOwn(methods, method) ? (opts.handlers[method as MethodName] as unknown as ((p: unknown) => unknown) | undefined) : undefined;
    if (!handler) throw new RpcError("unknown_method", `Unknown method ${method}`);
    return handler(methods[method as MethodName].params.parse(params));
  }

  const broadcast: Broadcast = (event, data) => {
    const text = JSON.stringify({ type: "event", event, data });
    for (const ws of clients) ws.send(text);
  };

  await new Promise<void>((resolve, reject) => {
    http.once("error", reject);
    http.listen(opts.port, "127.0.0.1", () => resolve());
  });
  port = (http.address() as { port: number }).port;

  return {
    port,
    broadcast,
    close: () =>
      new Promise<void>((resolve) => {
        for (const ws of clients) ws.terminate();
        wss.close();
        http.close(() => resolve());
      }),
  };
}

function sameToken(given: string, expected: string): boolean {
  const a = Buffer.from(given);
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

// Blocks DNS rebinding: the Host header must name this machine's loopback on our port.
function isLocalHost(req: IncomingMessage, port: number): boolean {
  return req.headers.host === `127.0.0.1:${port}` || req.headers.host === `localhost:${port}`;
}

// Browsers always send Origin, so a web page can't skip this check. Non-browser clients send none.
function originAllowed(req: IncomingMessage, allowed: string[]): boolean {
  return req.headers.origin === undefined || allowed.includes(req.headers.origin);
}
```

- [ ] **Step 5: Run the tests and the type check**

Run: `npm test`
Expected: PASS (server 5 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add package-lock.json packages/daemon
git commit -m "Add the daemon's local WebSocket server with token and origin checks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: CLI helpers

**Files:**
- Create: `packages/daemon/src/cli.ts`
- Test: `packages/daemon/src/cli.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `type CliName = "claude" | "codex"` and `type CliCommand = { file: string; args: string[] }`.
  - `resolveCli(name: CliName, env?: NodeJS.ProcessEnv): CliCommand | null`. A native exe on PATH wins. Otherwise it uses an npm-installed JS entry run with `process.execPath`.
  - `cliEnv(extra: Record<string, string>, base?: NodeJS.ProcessEnv): Record<string, string>`. It strips inherited secrets and config folders (case-insensitively), then applies `extra`.
  - `spawnCli(cmd, args, { cwd, env }): ChildProcessWithoutNullStreams`.
  - `runCli(cmd, args, env, timeoutMs?): Promise<{ code: number | null; stdout: string; stderr: string }>`.
  - `killTree(pid: number | undefined): void`.
  - `openUrl(url: string): void` (https only, no shell).
  - `openConsole(title: string, command: string, env: Record<string, string>): void` (Windows console window).

- [ ] **Step 1: Write the failing test**

Create `packages/daemon/src/cli.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { cliEnv, openUrl, resolveCli, runCli } from "./cli.ts";

const dir = () => mkdtempSync(join(tmpdir(), "tjommie cli ñ-"));

describe("resolveCli", () => {
  it("prefers a native executable on PATH", () => {
    const bin = dir();
    const exe = join(bin, process.platform === "win32" ? "codex.exe" : "codex");
    writeFileSync(exe, "");
    expect(resolveCli("codex", { PATH: bin })).toEqual({ file: exe, args: [] });
  });

  it("runs an npm-installed CLI's JS entry with our own Node (PATH key in any case)", () => {
    const bin = dir();
    const entry = join(bin, "node_modules", "@openai", "codex", "bin", "codex.js");
    mkdirSync(join(entry, ".."), { recursive: true });
    writeFileSync(entry, "");
    expect(resolveCli("codex", { Path: bin })).toEqual({ file: process.execPath, args: [entry] });
  });

  it("returns null when the CLI isn't installed", () => {
    expect(resolveCli("claude", { PATH: dir() })).toBeNull();
  });
});

describe("cliEnv", () => {
  it("strips inherited keys and config folders case-insensitively, then adds extras", () => {
    const env = cliEnv({ CLAUDE_CONFIG_DIR: "C:\\acct" }, { Path: "x", anthropic_api_key: "sk", CODEX_HOME: "h", ClaudeCode: "1", KEEP: "1" });
    expect(env).toEqual({ Path: "x", KEEP: "1", CLAUDE_CONFIG_DIR: "C:\\acct" });
  });
});

describe("runCli", () => {
  it("captures stdout and the exit code", async () => {
    const out = await runCli({ file: process.execPath, args: [] }, ["-e", "console.log('hi'); process.exit(3)"], cliEnv({}));
    expect(out.code).toBe(3);
    expect(out.stdout.trim()).toBe("hi");
  });
});

describe("openUrl", () => {
  it("refuses anything but https", () => {
    expect(() => openUrl("file:///C:/Windows/System32/calc.exe")).toThrow(/https/);
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./cli.ts` doesn't exist.

- [ ] **Step 3: Write cli.ts**

Create `packages/daemon/src/cli.ts`:

```ts
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";

export type CliName = "claude" | "codex";
export type CliCommand = { file: string; args: string[] };

const NPM_ENTRY: Record<CliName, string[]> = {
  claude: ["@anthropic-ai", "claude-code", "cli.js"],
  codex: ["@openai", "codex", "bin", "codex.js"],
};

// Finds a CLI on PATH. A native executable wins. An npm-installed CLI runs its JS entry with our own
// Node, because Windows can't spawn npm's .cmd/.ps1 shims without a shell.
export function resolveCli(name: CliName, env: NodeJS.ProcessEnv = process.env): CliCommand | null {
  const path = Object.entries(env).find(([key]) => key.toUpperCase() === "PATH")?.[1] ?? "";
  const dirs = path.split(delimiter).filter(Boolean);
  const exe = process.platform === "win32" ? `${name}.exe` : name;
  for (const dir of dirs) if (existsSync(join(dir, exe))) return { file: join(dir, exe), args: [] };
  for (const dir of dirs) {
    const entry = join(dir, "node_modules", ...NPM_ENTRY[name]);
    if (existsSync(entry)) return { file: process.execPath, args: [entry] };
  }
  return null;
}

const STRIPPED_ENV = [
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_AUTH_TOKEN",
  "CLAUDE_CODE_OAUTH_TOKEN",
  "CLAUDE_CONFIG_DIR",
  "CLAUDECODE",
  "CLAUDE_CODE_ENTRYPOINT",
  "OPENAI_API_KEY",
  "CODEX_API_KEY",
  "CODEX_HOME",
];

// A CLI must use the login in its Tjommie account folder, never keys or config folders we happen to inherit.
export function cliEnv(extra: Record<string, string>, base: NodeJS.ProcessEnv = process.env): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(base)) {
    if (value !== undefined && !STRIPPED_ENV.includes(key.toUpperCase())) env[key] = value;
  }
  return { ...env, ...extra };
}

export function spawnCli(cmd: CliCommand, args: string[], opts: { cwd: string; env: Record<string, string> }): ChildProcessWithoutNullStreams {
  return spawn(cmd.file, [...cmd.args, ...args], { cwd: opts.cwd, env: opts.env, stdio: "pipe", windowsHide: true });
}

// Runs a short command (version and status checks) and collects its output.
export function runCli(
  cmd: CliCommand,
  args: string[],
  env: Record<string, string>,
  timeoutMs = 20_000,
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(cmd.file, [...cmd.args, ...args], { env, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    const timer = setTimeout(() => killTree(child.pid), timeoutMs);
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ code: null, stdout, stderr: String(error) });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}

// The CLIs start their own children (shells, MCP servers), so stopping one means stopping its whole tree.
export function killTree(pid: number | undefined): void {
  if (pid === undefined) return;
  if (process.platform === "win32") {
    spawn("taskkill", ["/pid", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
    return;
  }
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // already gone
  }
}

// Opens a sign-in URL in the default browser. No shell: URLs contain & which cmd.exe would split on.
export function openUrl(url: string): void {
  if (!url.startsWith("https://")) throw new Error(`Refusing to open a non-https URL: ${url}`);
  const child =
    process.platform === "win32"
      ? spawn("rundll32", ["url.dll,FileProtocolHandler", url], { stdio: "ignore", detached: true })
      : spawn(process.platform === "darwin" ? "open" : "xdg-open", [url], { stdio: "ignore", detached: true });
  child.unref();
}

// Opens a visible console window running a CLI's own interactive command (Claude's sign-in).
// ponytail: Windows only, like the rest of Phase 1; macOS gets its own opener with the macOS build.
export function openConsole(title: string, command: string, env: Record<string, string>): void {
  const child = spawn("cmd.exe", ["/c", "start", `"${title}"`, "cmd.exe", "/k", command], {
    env,
    detached: true,
    stdio: "ignore",
    windowsVerbatimArguments: true,
  });
  child.unref();
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS (cli 6 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/cli.ts packages/daemon/src/cli.test.ts
git commit -m "Add helpers to find, start and stop the official CLIs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 5: Codex app-server client

**Files:**
- Create: `packages/daemon/src/providers/codex-rpc.ts`
- Test: `packages/daemon/src/providers/codex-rpc.test.ts`, `packages/daemon/src/providers/codex-rpc.real.test.ts`

**Interfaces:**
- Consumes: `cli.ts` (`spawnCli`, `killTree`, `cliEnv`, `resolveCli`, `type CliCommand`).
- Produces:
  - `class RpcRemoteError extends Error { code: number }`.
  - `class JsonRpcLines`, constructed with `write: (line: string) => void`. Public members:
    - `onNotification: (method: string, params: any) => void`
    - `onRequest: (id: number | string, method: string, params: any) => void`
    - `request<T = any>(method: string, params?: unknown, timeoutMs?: number): Promise<T>`
    - `notify(method: string, params?: unknown): void`
    - `respond(id: number | string, result: unknown): void`
    - `respondError(id: number | string, code: number, message: string): void`
    - `handleLine(line: string): void`
    - `failAll(error: Error): void`
  - `type CodexServer = { rpc: JsonRpcLines; child: ChildProcessWithoutNullStreams; close(): void }`.
  - `startCodexServer(cmd: CliCommand, env: Record<string, string>, cwd: string): Promise<CodexServer>`. It runs `app-server` and completes `initialize` plus `initialized`.

- [ ] **Step 1: Write the failing fast test**

Create `packages/daemon/src/providers/codex-rpc.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { JsonRpcLines, RpcRemoteError } from "./codex-rpc.ts";

function client() {
  const sent: any[] = [];
  const rpc = new JsonRpcLines((line) => sent.push(JSON.parse(line)));
  return { rpc, sent };
}

describe("JsonRpcLines", () => {
  it("matches responses to requests by id, in any order", async () => {
    const { rpc, sent } = client();
    const a = rpc.request("model/list", {});
    const b = rpc.request("account/read", {});
    rpc.handleLine(JSON.stringify({ id: sent[1].id, result: { account: null } }));
    rpc.handleLine(JSON.stringify({ id: sent[0].id, result: { data: [] } }));
    await expect(a).resolves.toEqual({ data: [] });
    await expect(b).resolves.toEqual({ account: null });
    expect(sent.map((m) => m.method)).toEqual(["model/list", "account/read"]);
  });

  it("rejects with the remote error (real error text from a bad thread config)", async () => {
    const { rpc, sent } = client();
    const pending = rpc.request("thread/start", {});
    rpc.handleLine(JSON.stringify({ id: sent[0].id, error: { code: -32600, message: "failed to load configuration: invalid transport" } }));
    await expect(pending).rejects.toBeInstanceOf(RpcRemoteError);
  });

  it("routes notifications and server requests, and ignores junk (real app-server lines)", () => {
    const { rpc } = client();
    const seen: string[] = [];
    rpc.onNotification = (method) => seen.push(`n:${method}`);
    rpc.onRequest = (id, method) => seen.push(`r:${id}:${method}`);
    rpc.handleLine('{"method":"turn/started","params":{"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turn":{"id":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","items":[],"itemsView":"notLoaded","status":"inProgress","error":null}}}');
    rpc.handleLine('{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"t","turnId":"u","itemId":"i","startedAtMs":1790327515964}}');
    rpc.handleLine("WARNING: proceeding, even though we could not create PATH aliases");
    expect(seen).toEqual(["n:turn/started", "r:0:item/commandExecution/requestApproval"]);
  });

  it("writes notifications, results and errors in the app-server's shape", () => {
    const { rpc, sent } = client();
    rpc.notify("initialized");
    rpc.respond(0, { decision: "accept" });
    rpc.respondError(1, -32601, "Not supported by Tjommie");
    expect(sent).toEqual([
      { method: "initialized" },
      { id: 0, result: { decision: "accept" } },
      { id: 1, error: { code: -32601, message: "Not supported by Tjommie" } },
    ]);
  });

  it("fails every pending request when the process goes away", async () => {
    const { rpc } = client();
    const pending = rpc.request("account/read", {});
    rpc.failAll(new Error("codex app-server exited"));
    await expect(pending).rejects.toThrow("exited");
  });
});
```

- [ ] **Step 2: Write the real-CLI test**

Create `packages/daemon/src/providers/codex-rpc.real.test.ts`:

```ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { cliEnv, resolveCli } from "../cli.ts";
import { startCodexServer } from "./codex-rpc.ts";

it("starts codex app-server in a fresh CODEX_HOME and reads its empty account", async () => {
  const cmd = resolveCli("codex");
  expect(cmd, "the Codex CLI must be installed").not.toBeNull();
  const home = mkdtempSync(join(tmpdir(), "tjommie codex home-"));
  const server = await startCodexServer(cmd!, cliEnv({ CODEX_HOME: home }), home);
  try {
    expect(await server.rpc.request("account/read", {})).toMatchObject({ account: null, requiresOpenaiAuth: true });
  } finally {
    server.close();
  }
});
```

- [ ] **Step 3: Run the fast tests to see them fail**

Run: `npm test`
Expected: FAIL. `./codex-rpc.ts` doesn't exist.

- [ ] **Step 4: Write codex-rpc.ts**

Create `packages/daemon/src/providers/codex-rpc.ts`:

```ts
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import { killTree, spawnCli, type CliCommand } from "../cli.ts";

export class RpcRemoteError extends Error {
  code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}

type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout };

// JSON-RPC over newline-delimited JSON, as spoken by `codex app-server` (it omits the "jsonrpc" field).
export class JsonRpcLines {
  onNotification: (method: string, params: any) => void = () => {};
  onRequest: (id: number | string, method: string, params: any) => void = () => {};
  #nextId = 1;
  #pending = new Map<number, Pending>();
  #write: (line: string) => void;

  constructor(write: (line: string) => void) {
    this.#write = write;
  }

  request<T = any>(method: string, params: unknown = {}, timeoutMs = 120_000): Promise<T> {
    const id = this.#nextId++;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs);
      this.#pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
      this.#write(JSON.stringify({ id, method, params }));
    });
  }

  notify(method: string, params?: unknown): void {
    this.#write(JSON.stringify(params === undefined ? { method } : { method, params }));
  }

  respond(id: number | string, result: unknown): void {
    this.#write(JSON.stringify({ id, result }));
  }

  respondError(id: number | string, code: number, message: string): void {
    this.#write(JSON.stringify({ id, error: { code, message } }));
  }

  handleLine(line: string): void {
    let message: any;
    try {
      message = JSON.parse(line);
    } catch {
      return; // stray log output
    }
    if (message.method !== undefined && message.id !== undefined) {
      this.onRequest(message.id, message.method, message.params);
      return;
    }
    if (message.method !== undefined) {
      this.onNotification(message.method, message.params);
      return;
    }
    const pending = this.#pending.get(message.id);
    if (!pending) return;
    this.#pending.delete(message.id);
    clearTimeout(pending.timer);
    if (message.error) pending.reject(new RpcRemoteError(message.error.code, message.error.message));
    else pending.resolve(message.result);
  }

  failAll(error: Error): void {
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.#pending.clear();
  }
}

export type CodexServer = { rpc: JsonRpcLines; child: ChildProcessWithoutNullStreams; close: () => void };

// Starts `codex app-server` with the given environment (CODEX_HOME = the account folder) and completes
// the initialize handshake.
export async function startCodexServer(cmd: CliCommand, env: Record<string, string>, cwd: string): Promise<CodexServer> {
  const child = spawnCli(cmd, ["app-server"], { cwd, env });
  const rpc = new JsonRpcLines((line) => child.stdin.write(`${line}\n`));
  createInterface({ input: child.stdout, crlfDelay: Infinity }).on("line", (line) => rpc.handleLine(line));
  child.stderr.resume(); // Codex logs to stderr; drain it so a full pipe can never block the process
  child.on("exit", () => rpc.failAll(new Error("codex app-server exited")));
  child.on("error", (error) => rpc.failAll(error));
  const close = () => {
    child.stdin.end();
    setTimeout(() => killTree(child.pid), 3000).unref();
  };
  try {
    await rpc.request("initialize", { clientInfo: { name: "tjommie", title: "Tjommie", version: "0.1.0" } }, 30_000);
    rpc.notify("initialized");
  } catch (error) {
    close();
    throw error;
  }
  return { rpc, child, close };
}
```

- [ ] **Step 5: Run the fast and real tests**

Run: `npm test`
Expected: PASS (codex-rpc 5 tests plus the earlier ones).

Run: `npm run test:real`
Expected: PASS (1 test: `account/read` in a fresh `CODEX_HOME`).

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/providers
git commit -m "Add a JSON-RPC client for the Codex app-server

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Accounts, setup check and one-time sign-in

**Files:**
- Create: `packages/daemon/src/setup.ts`, `packages/daemon/src/testing.ts`, `packages/daemon/scripts/signin.ts`
- Test: `packages/daemon/src/setup.test.ts`, `packages/daemon/src/setup.real.test.ts`

**Interfaces:**
- Consumes: `db.ts` (`Db`, `nowIso`), `cli.ts` (`cliEnv`, `openConsole`, `openUrl`, `resolveCli`, `runCli`), `providers/codex-rpc.ts` (`startCodexServer`), `server.ts` (`RpcError`), `paths.ts` (`dataPaths`), `@tjommie/protocol` (`Provider`, `ProviderSetup`, `ModelInfo`).
- Produces:
  - `setup.ts`:
    - `type Account = { id: string; provider: Provider; label: string; folder: string }`.
    - `ensureAccounts(db, accountsDir): Account[]`. It creates `claude-main` and `codex-main`, each with its folder.
    - `accountFor(db, provider): Account`.
    - `accountEnv(account): Record<string, string>`: `cliEnv` plus `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
    - `parseVersion(text): string | null` and `parseClaudeStatus(json): { signedIn: boolean; account: string | null; plan: string | null }`.
    - `checkSetup(db): Promise<ProviderSetup[]>`, always in the order `[claude, codex]`.
    - `signIn(db, provider, onDone: (ok: boolean) => void): Promise<{ url: string | null }>`.
    - `listModels(db, provider): Promise<ModelInfo[]>` and `CLAUDE_MODELS`.
  - `testing.ts` (helpers for tests only):
    - `tempRoot(): string`: a folder whose path has a space and an accent.
    - `realAccountsDir(): string` and `realAccount(provider): Account`.
    - `waitFor<T>(check, timeoutMs?): Promise<T>`.
    - `firstOf(events, type)`.
  - `npm run signin -w @tjommie/daemon`: signs the real Tjommie accounts in.

- [ ] **Step 1: Write the failing fast test**

Create `packages/daemon/src/setup.test.ts`:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { openDb } from "./db.ts";
import { accountEnv, accountFor, ensureAccounts, parseClaudeStatus, parseVersion } from "./setup.ts";
import { tempRoot } from "./testing.ts";

afterEach(() => vi.unstubAllEnvs());

describe("accounts", () => {
  it("creates one account per provider, once, each with its folder", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const first = ensureAccounts(db, join(root, "accounts"));
    expect(ensureAccounts(db, join(root, "accounts"))).toEqual(first);
    expect(first.map((a) => a.id)).toEqual(["claude-main", "codex-main"]);
    for (const account of first) expect(existsSync(account.folder)).toBe(true);
  });

  it("points each CLI at its account folder and never passes inherited keys", () => {
    vi.stubEnv("ANTHROPIC_API_KEY", "sk-inherited");
    vi.stubEnv("CODEX_HOME", "C:\\somewhere-else");
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const claude = accountEnv(accountFor(db, "claude"));
    expect(claude.CLAUDE_CONFIG_DIR).toBe(join(root, "accounts", "claude-main"));
    expect(claude.ANTHROPIC_API_KEY).toBeUndefined();
    expect(accountEnv(accountFor(db, "codex")).CODEX_HOME).toBe(join(root, "accounts", "codex-main"));
  });
});

describe("parsers", () => {
  it("reads both CLIs' version output (real output)", () => {
    expect(parseVersion("2.1.280 (Claude Code)\n")).toBe("2.1.280");
    expect(parseVersion("codex-cli 0.155.1\n")).toBe("0.155.1");
    expect(parseVersion("command not found")).toBeNull();
  });

  it("reads claude auth status JSON (real output, email replaced)", () => {
    expect(parseClaudeStatus('{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}')).toEqual({ signedIn: false, account: null, plan: null });
    expect(parseClaudeStatus('{"loggedIn": true, "authMethod": "claude.ai", "email": "person@example.com", "subscriptionType": "max"}')).toEqual({
      signedIn: true,
      account: "person@example.com",
      plan: "max",
    });
    expect(parseClaudeStatus("not json")).toEqual({ signedIn: false, account: null, plan: null });
  });
});
```

- [ ] **Step 2: Write the real-CLI test**

Create `packages/daemon/src/setup.real.test.ts`:

```ts
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { checkSetup, ensureAccounts, listModels } from "./setup.ts";
import { realAccountsDir, tempRoot } from "./testing.ts";

describe("setup against the installed CLIs", () => {
  it("finds both CLIs installed and fresh account folders signed out", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const [claude, codex] = await checkSetup(db);
    expect(claude).toMatchObject({ provider: "claude", installed: true, signedIn: false, error: null });
    expect(claude.version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(codex).toMatchObject({ provider: "codex", installed: true, signedIn: false, error: null });
    expect(codex.version).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it("lists Codex models before sign-in, including a default", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const models = await listModels(db, "codex");
    expect(models.length).toBeGreaterThan(0);
    expect(models.some((m) => m.isDefault)).toBe(true);
  });

  it("sees the one-time sign-in from `npm run signin -w @tjommie/daemon`", async () => {
    const db = openDb(join(tempRoot(), "t.db"));
    ensureAccounts(db, realAccountsDir());
    const setup = await checkSetup(db);
    expect(setup.map((s) => [s.provider, s.signedIn])).toEqual([
      ["claude", true],
      ["codex", true],
    ]);
  });
});
```

- [ ] **Step 3: Run the fast tests to see them fail**

Run: `npm test`
Expected: FAIL. `./setup.ts` and `./testing.ts` don't exist.

- [ ] **Step 4: Write testing.ts and setup.ts**

Create `packages/daemon/src/testing.ts`:

```ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Provider } from "@tjommie/protocol";
import { dataPaths } from "./paths.ts";
import type { Account } from "./setup.ts";

// Helpers shared by the *.test.ts and *.real.test.ts files. Not used by the daemon itself.

// A fresh folder whose path has a space and an accented letter, like many Windows profile paths.
export const tempRoot = (): string => mkdtempSync(join(tmpdir(), "tjommie test ñ-"));

// The real Tjommie account folders, signed in once with `npm run signin -w @tjommie/daemon`.
export const realAccountsDir = (): string => dataPaths().accounts;

export const realAccount = (provider: Provider): Account => ({
  id: `${provider}-main`,
  provider,
  label: provider === "claude" ? "Claude" : "Codex",
  folder: join(realAccountsDir(), `${provider}-main`),
});

export async function waitFor<T>(check: () => T | null | undefined | false, timeoutMs = 180_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`waitFor timed out after ${timeoutMs} ms`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

export function firstOf<E extends { type: string }, T extends E["type"]>(events: E[], type: T): Extract<E, { type: T }> | undefined {
  return events.find((event): event is Extract<E, { type: T }> => event.type === type);
}
```

Create `packages/daemon/src/setup.ts`:

```ts
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { ModelInfo, Provider, ProviderSetup } from "@tjommie/protocol";
import { cliEnv, openConsole, openUrl, resolveCli, runCli } from "./cli.ts";
import { nowIso, type Db } from "./db.ts";
import { startCodexServer } from "./providers/codex-rpc.ts";
import { RpcError } from "./server.ts";

export type Account = { id: string; provider: Provider; label: string; folder: string };

const SIGN_IN_TIMEOUT_MS = 10 * 60_000;

// Aliases always resolve to the newest model of each family.
export const CLAUDE_MODELS: ModelInfo[] = [
  { id: "sonnet", label: "Claude Sonnet (latest)", isDefault: true },
  { id: "opus", label: "Claude Opus (latest)", isDefault: false },
  { id: "haiku", label: "Claude Haiku (latest)", isDefault: false },
];

// Phase 1 has one Tjommie-owned account per provider. Its folder is the CLI's config home, so bots never
// load the person's own CLI setup (hooks, plugins, skills, memories, MCP servers).
export function ensureAccounts(db: Db, accountsDir: string): Account[] {
  for (const provider of ["claude", "codex"] as const) {
    const id = `${provider}-main`;
    const folder = join(accountsDir, id);
    mkdirSync(folder, { recursive: true });
    db.prepare("INSERT OR IGNORE INTO accounts (id, provider, label, kind, folder, created_at) VALUES (?, ?, ?, 'subscription', ?, ?)").run(
      id,
      provider,
      provider === "claude" ? "Claude" : "Codex",
      folder,
      nowIso(),
    );
  }
  return db.prepare("SELECT id, provider, label, folder FROM accounts ORDER BY created_at, id").all() as Account[];
}

export function accountFor(db: Db, provider: Provider): Account {
  const account = db.prepare("SELECT id, provider, label, folder FROM accounts WHERE provider = ? ORDER BY created_at, id LIMIT 1").get(provider) as
    | Account
    | undefined;
  if (!account) throw new RpcError("no_account", `No ${provider} account`);
  return account;
}

export function accountEnv(account: Account): Record<string, string> {
  return cliEnv(account.provider === "claude" ? { CLAUDE_CONFIG_DIR: account.folder } : { CODEX_HOME: account.folder });
}

export const parseVersion = (text: string): string | null => text.match(/\d+\.\d+\.\d+/)?.[0] ?? null;

export function parseClaudeStatus(text: string): { signedIn: boolean; account: string | null; plan: string | null } {
  try {
    const status = JSON.parse(text) as { loggedIn?: boolean; email?: string; subscriptionType?: string };
    return { signedIn: status.loggedIn === true, account: status.email ?? null, plan: status.subscriptionType ?? null };
  } catch {
    return { signedIn: false, account: null, plan: null };
  }
}

const notInstalled = (provider: Provider): ProviderSetup => ({
  provider,
  installed: false,
  version: null,
  signedIn: false,
  account: null,
  plan: null,
  error: `${provider === "claude" ? "Claude Code" : "Codex"} is not installed`,
});

async function claudeSetup(account: Account): Promise<ProviderSetup> {
  const cmd = resolveCli("claude");
  if (!cmd) return notInstalled("claude");
  const env = accountEnv(account);
  const [version, status] = await Promise.all([runCli(cmd, ["--version"], env), runCli(cmd, ["auth", "status"], env)]);
  return { provider: "claude", installed: true, version: parseVersion(version.stdout), ...parseClaudeStatus(status.stdout), error: null };
}

async function codexSetup(account: Account): Promise<ProviderSetup> {
  const cmd = resolveCli("codex");
  if (!cmd) return notInstalled("codex");
  const env = accountEnv(account);
  const version = parseVersion((await runCli(cmd, ["--version"], env)).stdout);
  try {
    const server = await startCodexServer(cmd, env, account.folder);
    try {
      const { account: signedIn } = await server.rpc.request<{ account: { email?: string; planType?: string } | null }>("account/read", {});
      return { provider: "codex", installed: true, version, signedIn: signedIn !== null, account: signedIn?.email ?? null, plan: signedIn?.planType ?? null, error: null };
    } finally {
      server.close();
    }
  } catch (error) {
    return { provider: "codex", installed: true, version, signedIn: false, account: null, plan: null, error: String(error) };
  }
}

export function checkSetup(db: Db): Promise<ProviderSetup[]> {
  return Promise.all([claudeSetup(accountFor(db, "claude")), codexSetup(accountFor(db, "codex"))]);
}

// Starts the provider's own sign-in for its Tjommie account and calls onDone once it finishes or times out.
// Claude: `claude auth login` in a console window. Codex: OpenAI's sign-in URL from the app-server, opened
// in the browser. Tjommie never sees a password or token; each CLI stores its login in the account folder.
export async function signIn(db: Db, provider: Provider, onDone: (ok: boolean) => void): Promise<{ url: string | null }> {
  const account = accountFor(db, provider);
  const cmd = resolveCli(provider);
  if (!cmd) throw new RpcError("not_installed", notInstalled(provider).error!);
  const env = accountEnv(account);

  if (provider === "claude") {
    openConsole("Sign in to Claude for Tjommie", "claude auth login", env);
    const deadline = Date.now() + SIGN_IN_TIMEOUT_MS;
    const poll = async (): Promise<void> => {
      if (parseClaudeStatus((await runCli(cmd, ["auth", "status"], env)).stdout).signedIn) return onDone(true);
      if (Date.now() > deadline) return onDone(false);
      setTimeout(() => void poll(), 3000);
    };
    setTimeout(() => void poll(), 3000);
    return { url: null };
  }

  const server = await startCodexServer(cmd, env, account.folder);
  let finished = false;
  const finish = (ok: boolean) => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    server.close();
    onDone(ok);
  };
  const timer = setTimeout(() => finish(false), SIGN_IN_TIMEOUT_MS);
  server.rpc.onNotification = (method, params) => {
    if (method === "account/login/completed") finish(params.success === true);
  };
  try {
    const login = await server.rpc.request<{ authUrl: string }>("account/login/start", { type: "chatgpt" });
    openUrl(login.authUrl);
    return { url: login.authUrl };
  } catch (error) {
    finish(false);
    throw error;
  }
}

export async function listModels(db: Db, provider: Provider): Promise<ModelInfo[]> {
  if (provider === "claude") return CLAUDE_MODELS;
  const cmd = resolveCli("codex");
  if (!cmd) throw new RpcError("not_installed", "Codex is not installed");
  const account = accountFor(db, "codex");
  const server = await startCodexServer(cmd, accountEnv(account), account.folder);
  try {
    const { data } = await server.rpc.request<{ data: { model: string; displayName: string; hidden: boolean; isDefault: boolean }[] }>("model/list", {});
    return data.filter((m) => !m.hidden).map((m) => ({ id: m.model, label: m.displayName, isDefault: m.isDefault }));
  } finally {
    server.close();
  }
}
```

- [ ] **Step 5: Write the sign-in script**

Create `packages/daemon/scripts/signin.ts`:

```ts
// One-time setup: sign Tjommie's own Claude and Codex accounts in.
// The daemon and the real-CLI tests (`npm run test:real`) use these account folders.
import { openDb } from "../src/db.ts";
import { dataPaths } from "../src/paths.ts";
import { checkSetup, ensureAccounts, signIn } from "../src/setup.ts";

const paths = dataPaths();
const db = openDb(paths.db);
ensureAccounts(db, paths.accounts);

for (const setup of await checkSetup(db)) {
  if (!setup.installed) {
    console.log(`${setup.provider}: not installed, skipping.`);
    continue;
  }
  if (setup.signedIn) {
    console.log(`${setup.provider}: already signed in as ${setup.account}.`);
    continue;
  }
  console.log(`${setup.provider}: opening its sign-in. Finish it in the window or browser that opens.`);
  await new Promise<void>((resolve, reject) => {
    signIn(db, setup.provider, (ok) => (ok ? resolve() : reject(new Error(`${setup.provider} sign-in did not finish within 10 minutes`))))
      .then(({ url }) => {
        if (url) console.log(`If the browser didn't open, go to: ${url}`);
      })
      .catch(reject);
  });
}

for (const setup of await checkSetup(db)) {
  console.log(`${setup.provider}: ${setup.signedIn ? `signed in as ${setup.account} (${setup.plan})` : "not signed in"}`);
}
db.close();
```

- [ ] **Step 6: Run the fast tests**

Run: `npm test`
Expected: PASS (setup 4 tests plus the earlier ones).

- [ ] **Step 7: Human step, one-time sign-in**

Ask the user to run this in PowerShell from the repo root and finish both sign-ins: `npm run signin -w @tjommie/daemon`

- **Claude:** a console window opens with `claude auth login`. The user signs in with the Claude account Tjommie should use, then closes the window.
- **Codex:** the browser opens OpenAI's sign-in page. The user signs in with the ChatGPT account Tjommie should use.

Expected final output:
```
claude: signed in as <email> (<plan>)
codex: signed in as <email> (<plan>)
```

If you're a subagent, stop here and report that this step needs the user. Continue once they confirm.

- [ ] **Step 8: Run the real tests**

Run: `npm run test:real`
Expected: PASS (setup 3 tests, codex-rpc 1 test).

- [ ] **Step 9: Commit**

```bash
git add packages/daemon/src/setup.ts packages/daemon/src/setup.test.ts packages/daemon/src/setup.real.test.ts packages/daemon/src/testing.ts packages/daemon/scripts/signin.ts
git commit -m "Add Tjommie-owned CLI accounts, setup check and one-time sign-in

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Bots store

**Files:**
- Create: `packages/daemon/src/bots.ts`
- Test: `packages/daemon/src/bots.test.ts`

**Interfaces:**
- Consumes: `db.ts` (`Db`, `newId`, `nowIso`, `transaction`), `server.ts` (`RpcError`), `@tjommie/protocol` (types `Bot`, `BotInput`, `BotPatch`).
- Produces:
  - `type StoredBot = Omit<Bot, "state">`.
  - Reads: `getStoredBot(db, id): StoredBot` (throws `RpcError("not_found")`) and `listStoredBots(db): StoredBot[]`, oldest first.
  - `createBot(db, botsDir, input: BotInput): StoredBot`. It creates `<botsDir>\<id>\notes.md` and `inbox\`, plus the bot's conversation, whose id is the bot id.
  - `updateBot(db, id, patch: BotPatch): StoredBot`. It clears saved allow rules when the provider changes.
  - `deleteBot(db, id): void`, which removes the home folder but never a project folder.
  - Flags: `setPaused(db, id, paused)` and `setLimitedUntil(db, id, isoOrNull)`.
  - `botPrompt(bot: StoredBot): string`.

- [ ] **Step 1: Write the failing test**

Create `packages/daemon/src/bots.test.ts`:

```ts
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { botPrompt, createBot, deleteBot, getStoredBot, listStoredBots, setPaused, updateBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { tempRoot } from "./testing.ts";

const input = { name: "Researcher", color: "teal", shape: "droplet", instructions: "Find sources.", provider: "claude", model: "sonnet" } as const;
let db: Db;
let root: string;
let botsDir: string;

beforeEach(() => {
  root = tempRoot();
  botsDir = join(root, "bots");
  db = openDb(join(root, "t.db"));
});

describe("bots store", () => {
  it("creates a bot with defaults, a home folder, notes.md and an inbox", () => {
    const bot = createBot(db, botsDir, input);
    expect(bot).toMatchObject({ name: "Researcher", autonomy: "folder", browser: false, paused: false, projectFolder: null, canMessage: [], limitedUntil: null });
    expect(bot.homeFolder).toBe(join(botsDir, bot.id));
    expect(readFileSync(join(bot.homeFolder, "notes.md"), "utf8")).toContain("Researcher");
    expect(existsSync(join(bot.homeFolder, "inbox"))).toBe(true);
  });

  it("keeps names unique regardless of case", () => {
    createBot(db, botsDir, input);
    expect(() => createBot(db, botsDir, { ...input, name: "researcher" })).toThrow(/already a bot/);
  });

  it("accepts only existing absolute project folders, spaces and accents included", () => {
    const project = join(root, "My Project ñ");
    mkdirSync(project);
    expect(createBot(db, botsDir, { ...input, projectFolder: project }).projectFolder).toBe(project);
    expect(() => createBot(db, botsDir, { ...input, name: "B", projectFolder: join(root, "missing") })).toThrow(/existing folder/);
    expect(() => createBot(db, botsDir, { ...input, name: "C", projectFolder: "relative" })).toThrow(/existing folder/);
  });

  it("validates the Can message list", () => {
    const researcher = createBot(db, botsDir, input);
    const writer = createBot(db, botsDir, { ...input, name: "Writer", canMessage: [researcher.id] });
    expect(writer.canMessage).toEqual([researcher.id]);
    expect(() => updateBot(db, researcher.id, { canMessage: [researcher.id] })).toThrow(/itself/);
    expect(() => updateBot(db, researcher.id, { canMessage: ["nope"] })).toThrow(/no such bot/);
  });

  it("changes only the patched fields, and drops saved rules when the provider changes", () => {
    const bot = createBot(db, botsDir, input);
    db.prepare("INSERT INTO allow_rules (bot_id, rule) VALUES (?, 'Write')").run(bot.id);
    expect(updateBot(db, bot.id, { autonomy: "ask" })).toMatchObject({ autonomy: "ask", name: "Researcher", model: "sonnet", instructions: "Find sources." });
    expect(db.prepare("SELECT COUNT(*) AS n FROM allow_rules").get()).toEqual({ n: 1 });
    updateBot(db, bot.id, { provider: "codex", model: "gpt-5.6-sol" });
    expect(db.prepare("SELECT COUNT(*) AS n FROM allow_rules").get()).toEqual({ n: 0 });
  });

  it("pauses, and deletes the bot with its chat and home folder", () => {
    const bot = createBot(db, botsDir, input);
    setPaused(db, bot.id, true);
    expect(getStoredBot(db, bot.id).paused).toBe(true);
    db.prepare("INSERT INTO messages (id, conversation_id, author, kind, body, status, created_at, seq) VALUES ('m', ?, 'you', 'text', 'hi', 'done', 'now', 1)").run(bot.id);
    deleteBot(db, bot.id);
    expect(listStoredBots(db)).toEqual([]);
    expect(existsSync(bot.homeFolder)).toBe(false);
    expect(db.prepare("SELECT COUNT(*) AS n FROM messages").get()).toEqual({ n: 0 });
  });

  it("builds a prompt with the name, home folder and instructions", () => {
    const bot = createBot(db, botsDir, input);
    const prompt = botPrompt(bot);
    expect(prompt).toContain("You are Researcher");
    expect(prompt).toContain(bot.homeFolder);
    expect(prompt).toContain("Find sources.");
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./bots.ts` doesn't exist.

- [ ] **Step 3: Write bots.ts**

Create `packages/daemon/src/bots.ts`:

```ts
import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { Bot, BotInput, BotPatch } from "@tjommie/protocol";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

export type StoredBot = Omit<Bot, "state">;

type BotRow = {
  id: string;
  name: string;
  color: Bot["color"];
  shape: Bot["shape"];
  instructions: string;
  provider: Bot["provider"];
  model: string;
  autonomy: Bot["autonomy"];
  home_folder: string;
  project_folder: string | null;
  browser: number;
  paused: number;
  limited_until: string | null;
  created_at: string;
};

export function getStoredBot(db: Db, id: string): StoredBot {
  const row = db.prepare("SELECT * FROM bots WHERE id = ?").get(id) as BotRow | undefined;
  if (!row) throw new RpcError("not_found", "No such bot");
  const links = db.prepare("SELECT target_bot_id FROM bot_links WHERE bot_id = ? ORDER BY target_bot_id").all(id) as { target_bot_id: string }[];
  return {
    id: row.id,
    name: row.name,
    color: row.color,
    shape: row.shape,
    instructions: row.instructions,
    provider: row.provider,
    model: row.model,
    autonomy: row.autonomy,
    homeFolder: row.home_folder,
    projectFolder: row.project_folder,
    browser: row.browser === 1,
    paused: row.paused === 1,
    canMessage: links.map((link) => link.target_bot_id),
    limitedUntil: row.limited_until,
    createdAt: row.created_at,
  };
}

export function listStoredBots(db: Db): StoredBot[] {
  const rows = db.prepare("SELECT id FROM bots ORDER BY created_at, rowid").all() as { id: string }[];
  return rows.map((row) => getStoredBot(db, row.id));
}

export function createBot(db: Db, botsDir: string, input: BotInput): StoredBot {
  assertNameFree(db, input.name, null);
  const projectFolder = checkProjectFolder(input.projectFolder ?? null);
  const id = newId();
  const home = join(botsDir, id);
  mkdirSync(join(home, "inbox"), { recursive: true });
  writeFileSync(join(home, "notes.md"), `# ${input.name}'s notes\n`);
  try {
    transaction(db, () => {
      db.prepare(
        `INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, project_folder, browser, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      ).run(id, input.name, input.color, input.shape, input.instructions, input.provider, input.model, input.autonomy ?? "folder", home, projectFolder, input.browser ? 1 : 0, nowIso());
      db.prepare("INSERT INTO conversations (id, kind, bot_id) VALUES (?, 'bot', ?)").run(id, id);
      setLinks(db, id, input.canMessage ?? []);
    });
  } catch (error) {
    rmSync(home, { recursive: true, force: true });
    throw error;
  }
  return getStoredBot(db, id);
}

export function updateBot(db: Db, id: string, patch: BotPatch): StoredBot {
  const bot = getStoredBot(db, id);
  if (patch.name !== undefined) assertNameFree(db, patch.name, id);
  const next = { ...bot, ...patch, projectFolder: patch.projectFolder === undefined ? bot.projectFolder : checkProjectFolder(patch.projectFolder) };
  transaction(db, () => {
    db.prepare("UPDATE bots SET name = ?, color = ?, shape = ?, instructions = ?, provider = ?, model = ?, autonomy = ?, project_folder = ?, browser = ? WHERE id = ?").run(
      next.name,
      next.color,
      next.shape,
      next.instructions,
      next.provider,
      next.model,
      next.autonomy,
      next.projectFolder,
      next.browser ? 1 : 0,
      id,
    );
    if (patch.canMessage) setLinks(db, id, patch.canMessage);
    // Saved "Always allow" rules are in one CLI's own format, so they don't carry over to the other CLI.
    if (next.provider !== bot.provider) db.prepare("DELETE FROM allow_rules WHERE bot_id = ?").run(id);
  });
  return getStoredBot(db, id);
}

// Deletes the bot, its chat and its home folder. An attached project folder is never touched.
export function deleteBot(db: Db, id: string): void {
  const bot = getStoredBot(db, id);
  db.prepare("DELETE FROM bots WHERE id = ?").run(id);
  rmSync(bot.homeFolder, { recursive: true, force: true });
}

export function setPaused(db: Db, id: string, paused: boolean): void {
  getStoredBot(db, id);
  db.prepare("UPDATE bots SET paused = ? WHERE id = ?").run(paused ? 1 : 0, id);
}

export function setLimitedUntil(db: Db, id: string, until: string | null): void {
  db.prepare("UPDATE bots SET limited_until = ? WHERE id = ?").run(until, id);
}

// The bot's standing instructions, appended to the CLI's own system prompt.
export function botPrompt(bot: StoredBot): string {
  return [
    `You are ${bot.name}, one of the bots a person runs in Tjommie on their own computer.`,
    `Your home folder is ${bot.homeFolder}.${bot.projectFolder ? ` You work in the project folder ${bot.projectFolder}.` : ""}`,
    "Keep notes.md in your home folder up to date with the person's preferences, important facts and short summaries of finished work, and read it when a new session starts.",
    "Files the person attaches are copied into the inbox folder inside your home folder.",
    "Reply in plain, friendly language, and keep replies short unless the person asks for detail.",
    "",
    "What the person wants from you:",
    bot.instructions.trim() || "No instructions yet. Ask what they need.",
  ].join("\n");
}

function assertNameFree(db: Db, name: string, exceptId: string | null): void {
  // ponytail: SQLite lower() folds ASCII only, so "Ñame" and "ñame" count as different names.
  if (db.prepare("SELECT id FROM bots WHERE lower(name) = lower(?) AND id IS NOT ?").get(name, exceptId)) {
    throw new RpcError("name_taken", `There's already a bot called ${name}`);
  }
}

function checkProjectFolder(folder: string | null): string | null {
  if (folder === null) return null;
  if (!isAbsolute(folder) || !existsSync(folder) || !statSync(folder).isDirectory()) {
    throw new RpcError("bad_params", "The project folder must be an existing folder");
  }
  return folder;
}

function setLinks(db: Db, botId: string, targets: string[]): void {
  db.prepare("DELETE FROM bot_links WHERE bot_id = ?").run(botId);
  for (const target of new Set(targets)) {
    if (target === botId) throw new RpcError("bad_params", "A bot can't message itself");
    if (!db.prepare("SELECT 1 FROM bots WHERE id = ?").get(target)) throw new RpcError("not_found", "Can message: no such bot");
    db.prepare("INSERT INTO bot_links (bot_id, target_bot_id) VALUES (?, ?)").run(botId, target);
  }
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS (bots 7 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/bots.ts packages/daemon/src/bots.test.ts
git commit -m "Add the bots store with home folders and Can message links

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Chat store and session recap

**Files:**
- Create: `packages/daemon/src/chat.ts`
- Test: `packages/daemon/src/chat.test.ts`

**Interfaces:**
- Consumes: `db.ts` (`Db`, `newId`, `nowIso`), `server.ts` (`RpcError`), `bots.ts` (`createBot`, in tests), `@tjommie/protocol` (type `Message`).
- Produces:
  - `addMessage(db, { conversationId, author, kind, body, data?, status }): Message`. Messages get an increasing `seq` per conversation.
  - `getMessage(db, id): Message` (throws `RpcError("not_found")`).
  - `updateMessage(db, id, { body?, data?, status? }): Message`. It replaces `data` only when the patch gives it.
  - `history(db, conversationId, before: string | undefined, limit): Message[]`: the newest `limit` messages before the message with id `before`, oldest first.
  - `buildRecap(messages: Message[], botName: string): string`: `""` when there are no text messages.

- [ ] **Step 1: Write the failing test**

Create `packages/daemon/src/chat.test.ts`:

```ts
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Message } from "@tjommie/protocol";
import { createBot } from "./bots.ts";
import { addMessage, buildRecap, getMessage, history, updateMessage } from "./chat.ts";
import { openDb } from "./db.ts";
import { tempRoot } from "./testing.ts";

function setup() {
  const root = tempRoot();
  const db = openDb(join(root, "t.db"));
  const bot = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" });
  return { db, bot };
}

describe("chat store", () => {
  it("pages through history, newest page first, each page oldest first", () => {
    const { db, bot } = setup();
    const ids = ["one", "two", "three"].map((body) => addMessage(db, { conversationId: bot.id, author: "you", kind: "text", body, status: "done" }).id);
    expect(history(db, bot.id, undefined, 2).map((m) => m.body)).toEqual(["two", "three"]);
    expect(history(db, bot.id, ids[1], 50).map((m) => m.body)).toEqual(["one"]);
  });

  it("updates body, status and data, and keeps data when an update doesn't mention it", () => {
    const { db, bot } = setup();
    const message = addMessage(db, { conversationId: bot.id, author: bot.id, kind: "text", body: "", data: { files: ["a.pdf"] }, status: "streaming" });
    expect(updateMessage(db, message.id, { body: "Done.", status: "done" })).toMatchObject({ body: "Done.", status: "done", data: { files: ["a.pdf"] } });
    expect(updateMessage(db, message.id, { data: null }).data).toBeNull();
    expect(() => getMessage(db, "missing")).toThrow(/No such message/);
  });

  it("builds a recap from text messages only, oldest first", () => {
    const message = (author: string, kind: Message["kind"], body: string): Message => ({ id: body, conversationId: "c", author, kind, body, data: null, status: "done", createdAt: "" });
    const recap = buildRecap([message("you", "text", "Remember the word ZEBRA."), message("bot-1", "activity", "1 step: Read notes.md"), message("bot-1", "text", "Noted.")], "Writer");
    expect(recap).toContain("Person: Remember the word ZEBRA.\nWriter: Noted.");
    expect(recap).not.toContain("1 step");
    expect(buildRecap([], "Writer")).toBe("");
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./chat.ts` doesn't exist.

- [ ] **Step 3: Write chat.ts**

Create `packages/daemon/src/chat.ts`:

```ts
import type { Message } from "@tjommie/protocol";
import { newId, nowIso, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

type MessageRow = {
  id: string;
  conversation_id: string;
  author: string;
  kind: Message["kind"];
  body: string;
  data: string | null;
  status: Message["status"];
  created_at: string;
};

const toMessage = (row: MessageRow): Message => ({
  id: row.id,
  conversationId: row.conversation_id,
  author: row.author,
  kind: row.kind,
  body: row.body,
  data: row.data === null ? null : (JSON.parse(row.data) as Record<string, unknown>),
  status: row.status,
  createdAt: row.created_at,
});

export function addMessage(
  db: Db,
  m: { conversationId: string; author: string; kind: Message["kind"]; body: string; data?: Record<string, unknown> | null; status: Message["status"] },
): Message {
  const id = newId();
  const { seq } = db.prepare("SELECT COALESCE(MAX(seq), 0) + 1 AS seq FROM messages WHERE conversation_id = ?").get(m.conversationId) as { seq: number };
  db.prepare("INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)").run(
    id,
    m.conversationId,
    m.author,
    m.kind,
    m.body,
    m.data ? JSON.stringify(m.data) : null,
    m.status,
    nowIso(),
    seq,
  );
  return getMessage(db, id);
}

export function getMessage(db: Db, id: string): Message {
  const row = db.prepare("SELECT * FROM messages WHERE id = ?").get(id) as MessageRow | undefined;
  if (!row) throw new RpcError("not_found", "No such message");
  return toMessage(row);
}

export function updateMessage(db: Db, id: string, patch: { body?: string; data?: Record<string, unknown> | null; status?: Message["status"] }): Message {
  const current = getMessage(db, id);
  const data = patch.data === undefined ? current.data : patch.data;
  db.prepare("UPDATE messages SET body = ?, data = ?, status = ? WHERE id = ?").run(patch.body ?? current.body, data ? JSON.stringify(data) : null, patch.status ?? current.status, id);
  return getMessage(db, id);
}

// The newest `limit` messages before the message `before`, oldest first.
export function history(db: Db, conversationId: string, before: string | undefined, limit: number): Message[] {
  const cursor = before ? (db.prepare("SELECT seq FROM messages WHERE id = ?").get(before) as { seq: number } | undefined) : undefined;
  const rows = db
    .prepare("SELECT * FROM messages WHERE conversation_id = ? AND seq < ? ORDER BY seq DESC LIMIT ?")
    .all(conversationId, cursor?.seq ?? Number.MAX_SAFE_INTEGER, limit) as MessageRow[];
  return rows.reverse().map(toMessage);
}

// Opening text for a fresh CLI session, so the bot picks up an ongoing chat where it left off.
export function buildRecap(messages: Message[], botName: string): string {
  const lines = messages
    .filter((m) => m.kind === "text" && m.body.trim() !== "")
    .map((m) => `${m.author === "you" ? "Person" : botName}: ${m.body.trim().slice(0, 2000)}`);
  if (lines.length === 0) return "";
  return ["[Tjommie: this is a new session of an ongoing chat. Your notes are in notes.md in your home folder. Recent messages, oldest first:", ...lines, "]"].join("\n");
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS (chat 3 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/chat.ts packages/daemon/src/chat.test.ts
git commit -m "Add the chat store and the recap for fresh sessions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 9: Provider events and the Claude output parser

**Files:**
- Create: `packages/daemon/src/providers/types.ts`, `packages/daemon/src/providers/claude-parse.ts`
- Test: `packages/daemon/src/providers/claude-parse.test.ts`

**Interfaces:**
- Consumes: `@tjommie/protocol` (type `Autonomy`).
- Produces:
  - `types.ts`:
    - `type ApprovalAnswer = "once" | "always" | "deny"`.
    - `type ProviderEvent`, one of:
      - `session {cliSessionId}`
      - `text {delta}` and `text_end`
      - `tool {id, name, summary}` and `tool_done {id, ok}`
      - `approval {id, tool, summary, detail, rule: string | null}`
      - `usage {model, input, cachedInput, output, reasoning, costUsd: number | null}`
      - `limit {state: "ok" | "near" | "reached", usedPercent: number | null, resetsAt: string | null}`
      - `turn_done {ok, error: string | null}` and `exit {error: string | null}`
    - `type StartOptions = { cwd; extraDirs: string[]; model; instructions; autonomy: Autonomy; allowRules: string[]; resumeId: string | null; env: Record<string, string> }`.
    - `interface ProviderSession { start(); send(text); steer(text): Promise<boolean>; interrupt(); answer(approvalId, answer); close(); pid(); onEvent(listener) }`.
  - `claude-parse.ts`: `type ClaudeLine`, `parseClaudeLine(line: string): ClaudeLine`, `toolSummary(name: string, input: Record<string, unknown>): string`.

- [ ] **Step 1: Write the failing test with real output**

Every line below is real `claude` 2.1.280 output from the 2026-09-25 spikes. A line marked *trimmed* had unused fields removed; the rejected rate-limit line is marked *built* because it is the allowed line with its status changed.

Create `packages/daemon/src/providers/claude-parse.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { parseClaudeLine, toolSummary } from "./claude-parse.ts";

const INIT = // trimmed: tools list shortened
  '{"type":"system","subtype":"init","cwd":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\claude-steer","session_id":"f472038e-3221-4314-af40-2fbc326b6209","tools":["Bash","Read","Write"],"mcp_servers":[],"model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none"}';
const TEXT_DELTA =
  '{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The"}},"session_id":"f472038e-3221-4314-af40-2fbc326b6209","parent_tool_use_id":null,"uuid":"39c0c5e1-590b-4623-9a36-397fccd50230"}';
const THINKING_DELTA =
  '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":null}},"session_id":"f472038e-3221-4314-af40-2fbc326b6209","parent_tool_use_id":null,"uuid":"5253cd2b-b1fd-4c18-afb2-69429037b323"}';
const ASSISTANT_TOOL = // trimmed: usage removed
  '{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011CfPso6TEaKSahfBEtfH1m","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_0157kDBqT6btcG5vksNWWWi7","name":"Bash","input":{"command":"sleep 8 && echo first","description":"Sleep for 8 seconds then print \\"first\\""},"caller":{"type":"direct"}}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const ASSISTANT_TEXT = // trimmed: usage removed
  '{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011CfPspSLbBqh1Mi18BxVgT","type":"message","role":"assistant","content":[{"type":"text","text":"The command printed \\"first\\" after the 8-second delay. BANANA"}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const TOOL_RESULT =
  '{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_0157kDBqT6btcG5vksNWWWi7","type":"tool_result","content":"first","is_error":false}]},"parent_tool_use_id":null,"session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const TOOL_DENIED =
  '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"The user denied this.","is_error":true,"tool_use_id":"toolu_01UKHn8SRaUHCyDtvGNiZCEX"}]},"parent_tool_use_id":null,"session_id":"d2b2b825-2396-4a10-a648-27605e94cc1d"}';
const ASK_BASH = // trimmed: one of three suggestions removed
  '{"type":"control_request","request_id":"8e4e2307-ddb2-47b6-a3df-2c08568d70bc","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"mkdir made-by-bash","description":"Create directory named made-by-bash"},"description":"Create directory named made-by-bash","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"mkdir made-by-bash *"}],"behavior":"allow","destination":"localSettings"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"blocked_path":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\claude-approve\\\\made-by-bash","tool_use_id":"toolu_01UKHn8SRaUHCyDtvGNiZCEX"}}';
const ASK_WRITE =
  '{"type":"control_request","request_id":"b3716c47-c14d-4ae1-a6d5-06f95632615b","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\claude-approve\\\\hello.txt","content":"hi"},"description":"hello.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_01QMRGxpAmYkosuW5kse1e2w"}}';
const RATE_LIMIT =
  '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1790341200,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.02,"resetsAt":1790341200},"seven_day":{"utilization":0.07,"resetsAt":1790802000}}},"uuid":"b883d966-1eba-4099-b9b6-229317053327","session_id":"f472038e-3221-4314-af40-2fbc326b6209"}';
const RATE_LIMIT_REJECTED = RATE_LIMIT.replace('"status":"allowed"', '"status":"rejected"').replace('"utilization":0.02', '"utilization":1'); // built from the real line
const RESULT = // trimmed: iterations and subagent stats removed
  '{"type":"result","subtype":"success","is_error":false,"result":"The command printed \\"first\\" after the 8-second delay. BANANA","session_id":"f472038e-3221-4314-af40-2fbc326b6209","total_cost_usd":0.0337045,"usage":{"input_tokens":18,"cache_creation_input_tokens":13868,"cache_read_input_tokens":48655,"output_tokens":217,"output_tokens_details":{"thinking_tokens":107}},"modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":18,"outputTokens":217,"cacheReadInputTokens":48655,"cacheCreationInputTokens":13868,"costUSD":0.0337045}},"terminal_reason":"completed","num_turns":2}';
const RESULT_STOPPED = // trimmed
  '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"","stop_reason":"tool_use","terminal_reason":"aborted_tools","session_id":"59b5f08f-5b89-444d-9fed-75dce5ec727a","total_cost_usd":0.0181357,"usage":{"input_tokens":18,"cache_creation_input_tokens":5839,"cache_read_input_tokens":48647,"output_tokens":315}}';
const CONTROL_RESPONSE = '{"type":"control_response","response":{"subtype":"success","request_id":"int-1","response":{"still_queued":[]}}}';

describe("parseClaudeLine", () => {
  it("reads the session id from each turn's init line", () => {
    expect(parseClaudeLine(INIT)).toEqual({ kind: "init", sessionId: "f472038e-3221-4314-af40-2fbc326b6209" });
  });

  it("streams text deltas and ignores thinking", () => {
    expect(parseClaudeLine(TEXT_DELTA)).toEqual({ kind: "text_delta", text: "The" });
    expect(parseClaudeLine(THINKING_DELTA)).toEqual({ kind: "other" });
  });

  it("splits assistant lines into text blocks and tool uses", () => {
    expect(parseClaudeLine(ASSISTANT_TOOL)).toEqual({
      kind: "assistant",
      texts: [],
      tools: [{ id: "toolu_0157kDBqT6btcG5vksNWWWi7", name: "Bash", input: { command: "sleep 8 && echo first", description: 'Sleep for 8 seconds then print "first"' } }],
    });
    expect(parseClaudeLine(ASSISTANT_TEXT)).toMatchObject({ kind: "assistant", texts: ['The command printed "first" after the 8-second delay. BANANA'], tools: [] });
  });

  it("ignores lines from subagents", () => {
    expect(parseClaudeLine(ASSISTANT_TEXT.replace('"parent_tool_use_id":null', '"parent_tool_use_id":"toolu_x"'))).toEqual({ kind: "other" });
  });

  it("reports tool results, including denied ones", () => {
    expect(parseClaudeLine(TOOL_RESULT)).toEqual({ kind: "tool_results", results: [{ id: "toolu_0157kDBqT6btcG5vksNWWWi7", ok: true }] });
    expect(parseClaudeLine(TOOL_DENIED)).toEqual({ kind: "tool_results", results: [{ id: "toolu_01UKHn8SRaUHCyDtvGNiZCEX", ok: false }] });
  });

  it("turns a permission request into an approval with a reusable rule", () => {
    expect(parseClaudeLine(ASK_BASH)).toMatchObject({
      kind: "permission",
      requestId: "8e4e2307-ddb2-47b6-a3df-2c08568d70bc",
      toolName: "Bash",
      description: "Create directory named made-by-bash",
      rule: "Bash(mkdir made-by-bash *)",
    });
    expect(parseClaudeLine(ASK_WRITE)).toMatchObject({ kind: "permission", toolName: "Write", description: "hello.txt", rule: "Write", suggestion: null });
  });

  it("reads rate limits: state, percent of the current window and reset time", () => {
    expect(parseClaudeLine(RATE_LIMIT)).toEqual({ kind: "rate_limit", state: "ok", usedPercent: 2, resetsAt: new Date(1790341200 * 1000).toISOString() });
    expect(parseClaudeLine(RATE_LIMIT_REJECTED)).toMatchObject({ kind: "rate_limit", state: "reached", usedPercent: 100 });
  });

  it("reads a finished turn's per-turn usage and cumulative cost", () => {
    expect(parseClaudeLine(RESULT)).toEqual({
      kind: "result",
      ok: true,
      error: null,
      totalCostUsd: 0.0337045,
      model: "claude-haiku-4-5-20251001",
      usage: { input: 18 + 13868, cachedInput: 48655, output: 217, reasoning: 107 },
    });
  });

  it("reads a stopped turn as failed, with its terminal reason", () => {
    expect(parseClaudeLine(RESULT_STOPPED)).toMatchObject({ kind: "result", ok: false, error: "aborted_tools", totalCostUsd: 0.0181357 });
  });

  it("ignores control responses and non-JSON output", () => {
    expect(parseClaudeLine(CONTROL_RESPONSE)).toEqual({ kind: "other" });
    expect(parseClaudeLine("Warning: something")).toEqual({ kind: "other" });
  });
});

describe("toolSummary", () => {
  it("prefers the tool's own description, then its main argument, then its name", () => {
    expect(toolSummary("Bash", { command: "npm test", description: "Run the tests" })).toBe("Run the tests");
    expect(toolSummary("Read", { file_path: "C:\\bots\\notes.md" })).toBe("C:\\bots\\notes.md");
    expect(toolSummary("TodoWrite", { todos: [] })).toBe("TodoWrite");
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./claude-parse.ts` doesn't exist.

- [ ] **Step 3: Write types.ts and claude-parse.ts**

Create `packages/daemon/src/providers/types.ts`:

```ts
import type { Autonomy } from "@tjommie/protocol";

export type ApprovalAnswer = "once" | "always" | "deny";

// What every CLI adapter reports, whatever its CLI prints.
export type ProviderEvent =
  | { type: "session"; cliSessionId: string }
  | { type: "text"; delta: string }
  | { type: "text_end" }
  | { type: "tool"; id: string; name: string; summary: string }
  | { type: "tool_done"; id: string; ok: boolean }
  | { type: "approval"; id: string; tool: string; summary: string; detail: string; rule: string | null }
  | { type: "usage"; model: string; input: number; cachedInput: number; output: number; reasoning: number; costUsd: number | null }
  | { type: "limit"; state: "ok" | "near" | "reached"; usedPercent: number | null; resetsAt: string | null }
  | { type: "turn_done"; ok: boolean; error: string | null }
  | { type: "exit"; error: string | null };

export type StartOptions = {
  cwd: string;
  extraDirs: string[];
  model: string;
  instructions: string;
  autonomy: Autonomy;
  allowRules: string[];
  resumeId: string | null;
  env: Record<string, string>;
};

// One running CLI conversation. Claude and Codex each have an adapter that implements this.
export interface ProviderSession {
  start(): Promise<void>;
  send(text: string): Promise<void>;
  steer(text: string): Promise<boolean>; // false: couldn't add it to the current turn, so the caller queues it
  interrupt(): Promise<void>;
  answer(approvalId: string, answer: ApprovalAnswer): void;
  close(): void;
  pid(): number | undefined;
  onEvent(listener: (event: ProviderEvent) => void): void;
}
```

Create `packages/daemon/src/providers/claude-parse.ts`:

```ts
// One line of `claude -p --output-format stream-json` output, reduced to what Tjommie uses.
export type ClaudeLine =
  | { kind: "init"; sessionId: string }
  | { kind: "text_delta"; text: string }
  | { kind: "assistant"; texts: string[]; tools: { id: string; name: string; input: Record<string, unknown> }[] }
  | { kind: "tool_results"; results: { id: string; ok: boolean }[] }
  | {
      kind: "permission";
      requestId: string;
      toolName: string;
      input: Record<string, unknown>;
      description: string;
      rule: string;
      suggestion: Record<string, unknown> | null;
    }
  | { kind: "rate_limit"; state: "ok" | "near" | "reached"; usedPercent: number | null; resetsAt: string | null }
  | {
      kind: "result";
      ok: boolean;
      error: string | null;
      totalCostUsd: number | null;
      model: string | null;
      usage: { input: number; cachedInput: number; output: number; reasoning: number };
    }
  | { kind: "other" };

export function parseClaudeLine(line: string): ClaudeLine {
  let m: any;
  try {
    m = JSON.parse(line);
  } catch {
    return { kind: "other" };
  }
  // Subagent traffic stays inside its tool call and never reaches the chat.
  const fromSubagent = m.parent_tool_use_id !== null && m.parent_tool_use_id !== undefined;
  switch (m.type) {
    case "system":
      return m.subtype === "init" ? { kind: "init", sessionId: m.session_id } : { kind: "other" };
    case "stream_event":
      return !fromSubagent && m.event?.type === "content_block_delta" && m.event.delta?.type === "text_delta"
        ? { kind: "text_delta", text: m.event.delta.text }
        : { kind: "other" };
    case "assistant": {
      if (fromSubagent) return { kind: "other" };
      const content: any[] = m.message?.content ?? [];
      return {
        kind: "assistant",
        texts: content.filter((c) => c.type === "text").map((c) => c.text as string),
        tools: content.filter((c) => c.type === "tool_use").map((c) => ({ id: c.id as string, name: c.name as string, input: c.input ?? {} })),
      };
    }
    case "user": {
      if (fromSubagent || !Array.isArray(m.message?.content)) return { kind: "other" };
      const results = m.message.content
        .filter((c: any) => c.type === "tool_result")
        .map((c: any) => ({ id: c.tool_use_id as string, ok: c.is_error !== true }));
      return results.length ? { kind: "tool_results", results } : { kind: "other" };
    }
    case "control_request": {
      const request = m.request;
      if (request?.subtype !== "can_use_tool") return { kind: "other" };
      const suggestion = (request.permission_suggestions ?? []).find((s: any) => s.type === "addRules" && s.behavior === "allow") ?? null;
      const first = suggestion?.rules?.[0];
      return {
        kind: "permission",
        requestId: m.request_id,
        toolName: request.tool_name,
        input: request.input ?? {},
        description: request.description ?? request.tool_name,
        rule: first ? (first.ruleContent ? `${first.toolName}(${first.ruleContent})` : first.toolName) : request.tool_name,
        suggestion,
      };
    }
    case "rate_limit_event": {
      const info = m.rate_limit_info ?? {};
      const window = info.unifiedWindows?.[info.rateLimitType];
      return {
        kind: "rate_limit",
        state: info.status === "rejected" ? "reached" : info.status === "allowed_warning" ? "near" : "ok",
        usedPercent: typeof window?.utilization === "number" ? Math.round(window.utilization * 100) : null,
        resetsAt: typeof info.resetsAt === "number" ? new Date(info.resetsAt * 1000).toISOString() : null,
      };
    }
    case "result": {
      const usage = m.usage ?? {};
      const failed = m.is_error === true || m.subtype !== "success";
      return {
        kind: "result",
        ok: !failed,
        error: failed ? String(m.result || m.terminal_reason || m.subtype || "The turn failed") : null,
        totalCostUsd: typeof m.total_cost_usd === "number" ? m.total_cost_usd : null,
        model: Object.keys(m.modelUsage ?? {})[0] ?? null,
        usage: {
          input: (usage.input_tokens ?? 0) + (usage.cache_creation_input_tokens ?? 0),
          cachedInput: usage.cache_read_input_tokens ?? 0,
          output: usage.output_tokens ?? 0,
          reasoning: usage.output_tokens_details?.thinking_tokens ?? 0,
        },
      };
    }
    default:
      return { kind: "other" };
  }
}

// A one-line description of a tool call for the chat's activity row.
export function toolSummary(name: string, input: Record<string, unknown>): string {
  for (const key of ["description", "command", "file_path", "url", "pattern", "query"]) {
    const value = input[key];
    if (typeof value === "string" && value.trim()) return value.trim().slice(0, 200);
  }
  return name;
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS (claude-parse 11 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/providers/types.ts packages/daemon/src/providers/claude-parse.ts packages/daemon/src/providers/claude-parse.test.ts
git commit -m "Add provider events and a parser for Claude's stream-json output

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Claude session

**Files:**
- Create: `packages/daemon/src/providers/claude.ts`
- Test: `packages/daemon/src/providers/claude.test.ts`, `packages/daemon/src/providers/claude.real.test.ts`

**Interfaces:**
- Consumes:
  - `cli.ts`: `spawnCli`, `killTree`, `type CliCommand`, and in tests `runCli`, `cliEnv`, `resolveCli`.
  - `claude-parse.ts` and `types.ts`.
  - In tests: `setup.ts` (`accountEnv`) and `testing.ts`.
- Produces:
  - `claudeMode(autonomy): "manual" | "acceptEdits" | "bypassPermissions"` and `claudeArgs(options: StartOptions): string[]`.
  - `class ClaudeSession implements ProviderSession`, constructed with `(cmd: CliCommand, options: StartOptions)`. How it behaves:
    - **Steering:** `steer()` writes to stdin and returns true while the process is running.
    - **Cost:** it reports each turn's own cost (the difference from the cumulative total).
    - **Stop:** `interrupt()` kills the process tree if no `result` arrives within 10 s.
    - **Always allow:** also sends a session rule back to Claude.
    - **Exit:** it emits `exit` once when the process ends.

- [ ] **Step 1: Write the failing fast test**

Create `packages/daemon/src/providers/claude.test.ts`:

```ts
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { cliEnv, runCli } from "../cli.ts";
import { claudeArgs, claudeMode } from "./claude.ts";
import type { StartOptions } from "./types.ts";

const base: StartOptions = {
  cwd: "C:\\bots\\b1",
  extraDirs: [],
  model: "sonnet",
  instructions: "Be brief.",
  autonomy: "folder",
  allowRules: [],
  resumeId: null,
  env: {},
};

describe("claudeArgs", () => {
  it("maps autonomy to Claude's permission modes", () => {
    expect([claudeMode("ask"), claudeMode("folder"), claudeMode("full")]).toEqual(["manual", "acceptEdits", "bypassPermissions"]);
  });

  it("starts a headless stream-json session with approvals over stdio and no outside MCP servers", () => {
    expect(claudeArgs(base)).toEqual([
      "-p",
      "--input-format",
      "stream-json",
      "--output-format",
      "stream-json",
      "--verbose",
      "--include-partial-messages",
      "--permission-prompt-tool",
      "stdio",
      "--permission-mode",
      "acceptEdits",
      "--model",
      "sonnet",
      "--append-system-prompt",
      "Be brief.",
      "--strict-mcp-config",
    ]);
  });

  it("adds extra folders, the session to resume, and saved rules last", () => {
    const args = claudeArgs({ ...base, extraDirs: ["C:\\home"], resumeId: "abc", allowRules: ["Write", "Bash(npm test *)"] });
    expect(args.slice(-7)).toEqual(["--add-dir", "C:\\home", "--resume", "abc", "--allowedTools", "Write", "Bash(npm test *)"]);
  });

  it("passes awkward instructions through process arguments unchanged (argument round trip)", async () => {
    const instructions = `Say "hi" & 'bye'\n100% ^caret <tag> | pipe naïve ✓ C:\\Users\\Thabo Mokoena`;
    const args = claudeArgs({ ...base, instructions });
    const script = join(mkdtempSync(join(tmpdir(), "tjommie args ñ-")), "print-args.js");
    writeFileSync(script, "process.stdout.write(JSON.stringify(process.argv.slice(2)))");
    const out = await runCli({ file: process.execPath, args: [script] }, args, cliEnv({}));
    expect(JSON.parse(out.stdout)).toEqual(args);
  });
});
```

- [ ] **Step 2: Write the real-CLI test**

Create `packages/daemon/src/providers/claude.real.test.ts`:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { resolveCli } from "../cli.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { ClaudeSession } from "./claude.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}) {
  const cwd = tempRoot();
  const claude = new ClaudeSession(resolveCli("claude")!, {
    cwd,
    extraDirs: [],
    model: "haiku",
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("claude")),
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  claude.onEvent((event) => events.push(event));
  return { claude, events, cwd };
}

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
});
```

- [ ] **Step 3: Run the fast tests to see them fail**

Run: `npm test`
Expected: FAIL. `./claude.ts` doesn't exist.

- [ ] **Step 4: Write claude.ts**

Create `packages/daemon/src/providers/claude.ts`:

```ts
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import type { Autonomy } from "@tjommie/protocol";
import { killTree, spawnCli, type CliCommand } from "../cli.ts";
import { parseClaudeLine, toolSummary } from "./claude-parse.ts";
import type { ApprovalAnswer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

const INTERRUPT_GRACE_MS = 10_000;

export function claudeMode(autonomy: Autonomy): "manual" | "acceptEdits" | "bypassPermissions" {
  return autonomy === "ask" ? "manual" : autonomy === "folder" ? "acceptEdits" : "bypassPermissions";
}

export function claudeArgs(options: StartOptions): string[] {
  const args = [
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--permission-prompt-tool",
    "stdio",
    "--permission-mode",
    claudeMode(options.autonomy),
    "--model",
    options.model,
    "--append-system-prompt",
    options.instructions,
    "--strict-mcp-config",
  ];
  for (const dir of options.extraDirs) args.push("--add-dir", dir);
  if (options.resumeId) args.push("--resume", options.resumeId);
  if (options.allowRules.length) args.push("--allowedTools", ...options.allowRules); // takes several values, so it goes last
  return args;
}

type PendingApproval = { input: Record<string, unknown>; toolName: string; suggestion: Record<string, unknown> | null };

// Drives `claude -p` over stream-json: one process per bot session; turns go to stdin as JSON lines.
export class ClaudeSession implements ProviderSession {
  #cmd: CliCommand;
  #options: StartOptions;
  #child: ChildProcessWithoutNullStreams | null = null;
  #listeners: ((event: ProviderEvent) => void)[] = [];
  #pending = new Map<string, PendingApproval>();
  #lastTotalCost = 0;
  #streamed = "";
  #inTurn = false;
  #interruptTimer: NodeJS.Timeout | undefined;
  #controlId = 0;

  constructor(cmd: CliCommand, options: StartOptions) {
    this.#cmd = cmd;
    this.#options = options;
  }

  onEvent(listener: (event: ProviderEvent) => void): void {
    this.#listeners.push(listener);
  }

  pid(): number | undefined {
    return this.#child?.pid;
  }

  async start(): Promise<void> {
    const child = spawnCli(this.#cmd, claudeArgs(this.#options), { cwd: this.#options.cwd, env: this.#options.env });
    this.#child = child;
    let stderr = "";
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => (stderr = (stderr + chunk).slice(-2000)));
    createInterface({ input: child.stdout, crlfDelay: Infinity }).on("line", (line) => this.#onLine(line));
    child.on("error", (error) => this.#exited(error.message));
    child.on("exit", (code) => this.#exited(code === 0 ? null : stderr.trim() || `Claude exited with code ${code}`));
  }

  async send(text: string): Promise<void> {
    this.#write({ type: "user", message: { role: "user", content: text } });
  }

  // Claude reads a message written mid-turn at its next step, inside the same turn.
  async steer(text: string): Promise<boolean> {
    if (!this.#child) return false;
    this.#write({ type: "user", message: { role: "user", content: text } });
    return true;
  }

  async interrupt(): Promise<void> {
    if (!this.#child || !this.#inTurn) return;
    this.#write({ type: "control_request", request_id: `tjommie-${++this.#controlId}`, request: { subtype: "interrupt" } });
    const pid = this.#child.pid;
    this.#interruptTimer = setTimeout(() => {
      if (this.#inTurn) killTree(pid);
    }, INTERRUPT_GRACE_MS);
  }

  answer(approvalId: string, answer: ApprovalAnswer): void {
    const pending = this.#pending.get(approvalId);
    if (!pending) return;
    this.#pending.delete(approvalId);
    const response =
      answer === "deny"
        ? { behavior: "deny", message: "The person declined this in Tjommie." }
        : { behavior: "allow", updatedInput: pending.input, ...(answer === "always" ? { updatedPermissions: [sessionRule(pending)] } : {}) };
    this.#write({ type: "control_response", response: { subtype: "success", request_id: approvalId, response } });
  }

  close(): void {
    const child = this.#child;
    if (!child) return;
    child.stdin.end();
    setTimeout(() => killTree(child.pid), 5000).unref();
  }

  #emit(event: ProviderEvent): void {
    for (const listener of this.#listeners) listener(event);
  }

  #write(message: unknown): void {
    this.#child?.stdin.write(`${JSON.stringify(message)}\n`);
  }

  #exited(error: string | null): void {
    if (!this.#child) return; // already reported
    this.#child = null;
    clearTimeout(this.#interruptTimer);
    this.#emit({ type: "exit", error });
  }

  #onLine(line: string): void {
    const out = parseClaudeLine(line);
    switch (out.kind) {
      case "init":
        this.#inTurn = true;
        this.#emit({ type: "session", cliSessionId: out.sessionId });
        return;
      case "text_delta":
        this.#streamed += out.text;
        this.#emit({ type: "text", delta: out.text });
        return;
      case "assistant":
        if (out.texts.length) {
          // Text normally arrives as deltas first. If it didn't, send the whole block now.
          if (!this.#streamed) this.#emit({ type: "text", delta: out.texts.join("") });
          this.#streamed = "";
          this.#emit({ type: "text_end" });
        }
        for (const tool of out.tools) this.#emit({ type: "tool", id: tool.id, name: tool.name, summary: toolSummary(tool.name, tool.input) });
        return;
      case "tool_results":
        for (const result of out.results) this.#emit({ type: "tool_done", id: result.id, ok: result.ok });
        return;
      case "permission":
        this.#pending.set(out.requestId, { input: out.input, toolName: out.toolName, suggestion: out.suggestion });
        this.#emit({
          type: "approval",
          id: out.requestId,
          tool: out.toolName,
          summary: out.description,
          detail: JSON.stringify(out.input, null, 2).slice(0, 4000),
          rule: out.rule,
        });
        return;
      case "rate_limit":
        this.#emit({ type: "limit", state: out.state, usedPercent: out.usedPercent, resetsAt: out.resetsAt });
        return;
      case "result": {
        clearTimeout(this.#interruptTimer);
        this.#inTurn = false;
        this.#streamed = "";
        // total_cost_usd is cumulative for this process, so one turn's cost is the difference.
        const cost = out.totalCostUsd === null ? null : Math.max(0, out.totalCostUsd - this.#lastTotalCost);
        if (out.totalCostUsd !== null) this.#lastTotalCost = out.totalCostUsd;
        this.#emit({ type: "usage", model: out.model ?? this.#options.model, ...out.usage, costUsd: cost });
        this.#emit({ type: "turn_done", ok: out.ok, error: out.error });
        return;
      }
      case "other":
        return;
    }
  }
}

// "Always allow" also tells this session to stop asking; the daemon saves the rule for later sessions.
function sessionRule(pending: PendingApproval): Record<string, unknown> {
  if (pending.suggestion) return { ...pending.suggestion, destination: "session" };
  return { type: "addRules", rules: [{ toolName: pending.toolName }], behavior: "allow", destination: "session" };
}
```

- [ ] **Step 5: Run the fast tests, the type check and the real tests**

Run: `npm test`
Expected: PASS (claude 4 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

Run: `npm run test:real`
Expected: PASS, including the 3 ClaudeSession tests.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/providers/claude.ts packages/daemon/src/providers/claude.test.ts packages/daemon/src/providers/claude.real.test.ts
git commit -m "Add the Claude session: stream-json turns, steering, stop and approvals

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Codex session

**Files:**
- Create: `packages/daemon/src/providers/codex-parse.ts`, `packages/daemon/src/providers/codex.ts`
- Test: `packages/daemon/src/providers/codex-parse.test.ts`, `packages/daemon/src/providers/codex.real.test.ts`

**Interfaces:**
- Consumes:
  - `codex-rpc.ts`: `startCodexServer`, `type CodexServer`, `type JsonRpcLines`.
  - `types.ts` and `cli.ts` (`type CliCommand`).
  - In tests: `setup.ts`, `testing.ts` and `paths.ts`.
- Produces:
  - `codex-parse.ts`:
    - `mapCodexNotification(method: string, params: any, model: string): ProviderEvent[]`.
    - `mapCodexRequest(id: number | string, method: string, params: any): (ProviderEvent & { type: "approval" }) | null`. A Codex rule is `codex:<JSON of proposedExecpolicyAmendment>`.
  - `codex.ts`:
    - `codexPolicy(autonomy): { approvalPolicy; sandbox }`.
    - `codexConfig(options, platform?): Record<string, unknown>`.
    - `class CodexSession implements ProviderSession`, constructed with `(cmd, options)`. It answers approvals with `accept` or `decline`, and replies with an error to server requests it doesn't know.

- [ ] **Step 1: Write the failing fast test with real output**

These lines are real `codex app-server` 0.155.1 output from the 2026-09-25 spikes, except the three marked *built*. The agent-message delta line was cut off in the spike log, so it was rebuilt from the schema's fields (`threadId`, `turnId`, `itemId`, `delta`) with real ids. The usage-limit turn and the file-change request were built from the protocol schema.

Create `packages/daemon/src/providers/codex-parse.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { codexConfig, codexPolicy } from "./codex.ts";
import { mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { StartOptions } from "./types.ts";

const parse = (line: string) => JSON.parse(line);
const MODEL = "gpt-6-sol";

const DELTA = parse( // built from the schema, real ids
  '{"method":"item/agentMessage/delta","params":{"threadId":"01a0d7cb-a738-7121-bc46-cb16f5896e3c","turnId":"01a0d7cb-b0ed-7a02-bc8c-042988202fa1","itemId":"msg_061da235ca62de87016ab63848eab087d0862ec7c6a83415fe","delta":"I will run"}}',
);
const COMMAND_STARTED = parse( // trimmed: the PowerShell wrapper command, cwd and process fields removed
  '{"method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974","source":"unifiedExecStartup","status":"inProgress","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath \'inside.txt\' -Value \'hi\' -NoNewline; Get-Content -LiteralPath \'inside.txt\' -Raw"}],"aggregatedOutput":null,"exitCode":null,"durationMs":null},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","startedAtMs":1790327506815},"emittedAtMs":1790327506815}',
);
const COMMAND_COMPLETED = parse( // trimmed like COMMAND_STARTED
  '{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974","status":"completed","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath \'inside.txt\' -Value \'hi\' -NoNewline; Get-Content -LiteralPath \'inside.txt\' -Raw"}],"aggregatedOutput":"hi","exitCode":0,"durationMs":398},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","completedAtMs":1790327507214},"emittedAtMs":1790327507311}',
);
const AGENT_COMPLETED = parse(
  '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_04cbf98466447733016ab63af82afc87d092e78611119924a6","text":"1. `inside.txt`: wrote `hi` and read it back successfully.","phase":"final_answer","memoryCitation":null,"delivery":null,"questions":null},"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5"}}',
);
const TOKEN_USAGE = parse(
  '{"method":"thread/tokenUsage/updated","params":{"threadId":"01a0d7cb-a738-7121-bc46-cb16f5896e3c","turnId":"01a0d7cb-b0ed-7a02-bc8c-042988202fa1","tokenUsage":{"total":{"totalTokens":52111,"inputTokens":51830,"cachedInputTokens":25600,"cacheWriteInputTokens":0,"outputTokens":281,"reasoningOutputTokens":35},"last":{"totalTokens":26163,"inputTokens":26025,"cachedInputTokens":25600,"cacheWriteInputTokens":0,"outputTokens":138,"reasoningOutputTokens":9},"modelContextWindow":258400}},"emittedAtMs":1790326878417}',
);
const RATE_LIMITS = parse(
  '{"method":"account/rateLimits/updated","params":{"rateLimits":{"limitId":"codex","limitName":null,"normalModelSlug":null,"primary":{"usedPercent":25,"windowDurationMins":10080,"resetsAt":1790438506},"secondary":null,"credits":{"hasCredits":false,"unlimited":false,"balance":"0"},"individualLimit":null,"spendControlReached":null,"planType":"pro","rateLimitReachedType":null}},"emittedAtMs":1790326867549}',
);
const TURN_COMPLETED = parse(
  '{"method":"turn/completed","params":{"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turn":{"id":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","items":[],"itemsView":"summary","status":"completed","error":null,"startedAt":1790327484,"completedAt":1790327547,"durationMs":62249}},"emittedAtMs":1790327547033}',
);
const TURN_LIMITED = { method: "turn/completed", params: { threadId: "t", turn: { id: "u", items: [], status: "failed", error: { message: "You've hit your usage limit.", codexErrorInfo: "usageLimitExceeded" } } } }; // built from the schema
const APPROVAL = parse( // trimmed: the PowerShell path in "command" shortened to powershell.exe
  '{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turnId":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","itemId":"exec-0d7b2fc4-81aa-4a06-98bf-ef0ccc6c31bf","startedAtMs":1790327515964,"environmentId":"local","reason":"Allow writing and reading outside-netconfig.txt in the parent folder as requested?","command":"powershell.exe -Command \\"Set-Content -LiteralPath \'C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\outside-netconfig.txt\' -Value \'hi\' -NoNewline\\"","cwd":"C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\netconfig","commandActions":[{"type":"unknown","command":"Set-Content -LiteralPath \'C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\outside-netconfig.txt\' -Value \'hi\' -NoNewline"}],"proposedExecpolicyAmendment":["Set-Content","-LiteralPath","C:\\\\Users\\\\tyrel\\\\AppData\\\\Local\\\\Temp\\\\tjommie-spikes\\\\codex\\\\outside-netconfig.txt","-Value","hi","-NoNewline"],"availableDecisions":["accept","cancel"]}}',
);
const FILE_APPROVAL = { reason: null, grantRoot: "C:\\work", itemId: "i", threadId: "t", turnId: "u", startedAtMs: 1 }; // built from the schema

describe("mapCodexNotification", () => {
  it("streams agent text and closes it when the message completes", () => {
    expect(mapCodexNotification(DELTA.method, DELTA.params, MODEL)).toEqual([{ type: "text", delta: "I will run" }]);
    expect(mapCodexNotification(AGENT_COMPLETED.method, AGENT_COMPLETED.params, MODEL)).toEqual([{ type: "text_end" }]);
  });

  it("reports a shell command as a tool, summarized by its inner command", () => {
    expect(mapCodexNotification(COMMAND_STARTED.method, COMMAND_STARTED.params, MODEL)).toEqual([
      { type: "tool", id: "exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974", name: "Shell", summary: "Set-Content -LiteralPath 'inside.txt' -Value 'hi' -NoNewline; Get-Content -LiteralPath 'inside.txt' -Raw" },
    ]);
    expect(mapCodexNotification(COMMAND_COMPLETED.method, COMMAND_COMPLETED.params, MODEL)).toEqual([{ type: "tool_done", id: "exec-3c0027d3-0c5b-41fa-92ac-fb29fa1d3974", ok: true }]);
  });

  it("reports each model call's tokens, uncached input separately", () => {
    expect(mapCodexNotification(TOKEN_USAGE.method, TOKEN_USAGE.params, MODEL)).toEqual([
      { type: "usage", model: MODEL, input: 26025 - 25600, cachedInput: 25600, output: 138, reasoning: 9, costUsd: null },
    ]);
  });

  it("reports rate limits with percent and reset time", () => {
    expect(mapCodexNotification(RATE_LIMITS.method, RATE_LIMITS.params, MODEL)).toEqual([
      { type: "limit", state: "ok", usedPercent: 25, resetsAt: new Date(1790438506 * 1000).toISOString() },
    ]);
  });

  it("ends turns, and reports a usage-limit failure as a reached limit first", () => {
    expect(mapCodexNotification(TURN_COMPLETED.method, TURN_COMPLETED.params, MODEL)).toEqual([{ type: "turn_done", ok: true, error: null }]);
    expect(mapCodexNotification(TURN_LIMITED.method, TURN_LIMITED.params, MODEL)).toEqual([
      { type: "limit", state: "reached", usedPercent: 100, resetsAt: null },
      { type: "turn_done", ok: false, error: "You've hit your usage limit." },
    ]);
  });

  it("ignores notifications Tjommie doesn't use", () => {
    expect(mapCodexNotification("mcpServer/startupStatus/updated", { name: "codex_apps", status: "ready" }, MODEL)).toEqual([]);
  });
});

describe("mapCodexRequest", () => {
  it("turns a command approval into an approval card with a reusable rule", () => {
    expect(mapCodexRequest(APPROVAL.id, APPROVAL.method, APPROVAL.params)).toEqual({
      type: "approval",
      id: "0",
      tool: "Shell",
      summary: "Allow writing and reading outside-netconfig.txt in the parent folder as requested?",
      detail: "Set-Content -LiteralPath 'C:\\Users\\tyrel\\AppData\\Local\\Temp\\tjommie-spikes\\codex\\outside-netconfig.txt' -Value 'hi' -NoNewline",
      rule: `codex:${JSON.stringify(APPROVAL.params.proposedExecpolicyAmendment)}`,
    });
  });

  it("turns a file-change approval into a card without a rule, and refuses unknown requests", () => {
    expect(mapCodexRequest(4, "item/fileChange/requestApproval", FILE_APPROVAL)).toEqual({ type: "approval", id: "4", tool: "Edit", summary: "Change files", detail: "C:\\work", rule: null });
    expect(mapCodexRequest(5, "item/tool/requestUserInput", {})).toBeNull();
  });
});

describe("codex policy and config", () => {
  const options: StartOptions = { cwd: "C:\\p", extraDirs: ["C:\\home"], model: "", instructions: "", autonomy: "folder", allowRules: [], resumeId: null, env: {} };

  it("maps autonomy to approval policy and sandbox", () => {
    expect(codexPolicy("ask")).toEqual({ approvalPolicy: "untrusted", sandbox: "read-only" });
    expect(codexPolicy("folder")).toEqual({ approvalPolicy: "on-request", sandbox: "workspace-write" });
    expect(codexPolicy("full")).toEqual({ approvalPolicy: "never", sandbox: "danger-full-access" });
  });

  it("opens the network and extra folders to the sandbox, unelevated on Windows", () => {
    expect(codexConfig(options, "win32")).toEqual({ sandbox_workspace_write: { network_access: true, writable_roots: ["C:\\home"] }, windows: { sandbox: "unelevated" } });
    expect(codexConfig(options, "darwin")).toEqual({ sandbox_workspace_write: { network_access: true, writable_roots: ["C:\\home"] } });
  });
});
```

- [ ] **Step 2: Write the real-CLI test**

Create `packages/daemon/src/providers/codex.real.test.ts`:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { resolveCli } from "../cli.ts";
import { dataPaths } from "../paths.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { CodexSession } from "./codex.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}) {
  const cwd = tempRoot();
  const codex = new CodexSession(resolveCli("codex")!, {
    cwd,
    extraDirs: [],
    model: "", // the account's default model
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("codex")),
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  codex.onEvent((event) => events.push(event));
  return { codex, events, cwd };
}

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
});
```

- [ ] **Step 3: Run the fast tests to see them fail**

Run: `npm test`
Expected: FAIL. `./codex-parse.ts` and `./codex.ts` don't exist.

- [ ] **Step 4: Write codex-parse.ts and codex.ts**

Create `packages/daemon/src/providers/codex-parse.ts`:

```ts
import type { ProviderEvent } from "./types.ts";

const TOOL_ITEMS = new Set(["commandExecution", "fileChange", "mcpToolCall", "webSearch"]);
const toIso = (seconds: unknown): string | null => (typeof seconds === "number" ? new Date(seconds * 1000).toISOString() : null);

function toolStarted(item: any): ProviderEvent[] {
  switch (item.type) {
    case "commandExecution":
      return [{ type: "tool", id: item.id, name: "Shell", summary: String(item.commandActions?.[0]?.command ?? item.command).slice(0, 200) }];
    case "fileChange":
      return [{ type: "tool", id: item.id, name: "Edit", summary: (item.changes ?? []).map((change: any) => change.path).join(", ").slice(0, 200) || "Change files" }];
    case "mcpToolCall":
      return [{ type: "tool", id: item.id, name: `${item.server}.${item.tool}`, summary: String(item.arguments?.title ?? item.tool).slice(0, 200) }];
    case "webSearch":
      return [{ type: "tool", id: item.id, name: "Web search", summary: String(item.query ?? "Search the web").slice(0, 200) }];
    default:
      return [];
  }
}

// Maps one app-server notification to Tjommie's provider events. `model` is the thread's model.
export function mapCodexNotification(method: string, params: any, model: string): ProviderEvent[] {
  switch (method) {
    case "item/agentMessage/delta":
      return [{ type: "text", delta: params.delta }];
    case "item/started":
      return toolStarted(params.item);
    case "item/completed": {
      const item = params.item;
      if (item.type === "agentMessage") return [{ type: "text_end" }];
      // Web searches carry no status; they only complete when they worked.
      if (TOOL_ITEMS.has(item.type)) return [{ type: "tool_done", id: item.id, ok: item.status === undefined || item.status === "completed" }];
      return [];
    }
    case "thread/tokenUsage/updated": {
      const last = params.tokenUsage.last;
      return [
        {
          type: "usage",
          model,
          input: last.inputTokens - last.cachedInputTokens,
          cachedInput: last.cachedInputTokens,
          output: last.outputTokens,
          reasoning: last.reasoningOutputTokens,
          costUsd: null, // Codex reports no cost; Phase 2 prices tokens from a price list
        },
      ];
    }
    case "account/rateLimits/updated": {
      const limits = params.rateLimits;
      const used: number | null = limits.primary?.usedPercent ?? null;
      return [
        {
          type: "limit",
          state: limits.rateLimitReachedType ? "reached" : used !== null && used >= 90 ? "near" : "ok",
          usedPercent: used,
          resetsAt: toIso(limits.primary?.resetsAt),
        },
      ];
    }
    case "turn/completed": {
      const turn = params.turn;
      const limit: ProviderEvent[] = turn.error?.codexErrorInfo === "usageLimitExceeded" ? [{ type: "limit", state: "reached", usedPercent: 100, resetsAt: null }] : [];
      const ok = turn.status === "completed";
      return [...limit, { type: "turn_done", ok, error: ok ? null : (turn.error?.message ?? `The turn ended: ${turn.status}`) }];
    }
    default:
      return [];
  }
}

// Maps an approval request from the app-server. The session refuses anything this returns null for.
export function mapCodexRequest(id: number | string, method: string, params: any): Extract<ProviderEvent, { type: "approval" }> | null {
  if (method === "item/commandExecution/requestApproval") {
    const command = String(params.commandActions?.[0]?.command ?? params.command ?? "a command");
    const proposal = params.proposedExecpolicyAmendment;
    return {
      type: "approval",
      id: String(id),
      tool: "Shell",
      summary: params.reason ?? `Run ${command}`,
      detail: command,
      rule: Array.isArray(proposal) ? `codex:${JSON.stringify(proposal)}` : null,
    };
  }
  if (method === "item/fileChange/requestApproval") {
    return { type: "approval", id: String(id), tool: "Edit", summary: params.reason ?? "Change files", detail: params.grantRoot ?? "", rule: null };
  }
  return null;
}
```

Create `packages/daemon/src/providers/codex.ts`:

```ts
import type { Autonomy } from "@tjommie/protocol";
import type { CliCommand } from "../cli.ts";
import { startCodexServer, type CodexServer, type JsonRpcLines } from "./codex-rpc.ts";
import { mapCodexNotification, mapCodexRequest } from "./codex-parse.ts";
import type { ApprovalAnswer, ProviderEvent, ProviderSession, StartOptions } from "./types.ts";

type Policy = { approvalPolicy: "untrusted" | "on-request" | "never"; sandbox: "read-only" | "workspace-write" | "danger-full-access" };

export function codexPolicy(autonomy: Autonomy): Policy {
  if (autonomy === "ask") return { approvalPolicy: "untrusted", sandbox: "read-only" };
  if (autonomy === "folder") return { approvalPolicy: "on-request", sandbox: "workspace-write" };
  return { approvalPolicy: "never", sandbox: "danger-full-access" };
}

// Per-thread config overrides. The unelevated Windows sandbox needs no admin rights. Shell commands still
// can't reach the network inside it (TLS fails), so they ask; bots browse with the browser tool instead.
export function codexConfig(options: StartOptions, platform: NodeJS.Platform = process.platform): Record<string, unknown> {
  return {
    sandbox_workspace_write: { network_access: true, writable_roots: options.extraDirs },
    ...(platform === "win32" ? { windows: { sandbox: "unelevated" } } : {}),
  };
}

type ThreadResponse = { thread: { id: string }; model: string };

// Drives `codex app-server`: one process and one thread per bot session.
export class CodexSession implements ProviderSession {
  #cmd: CliCommand;
  #options: StartOptions;
  #server: CodexServer | null = null;
  #listeners: ((event: ProviderEvent) => void)[] = [];
  #approvals = new Map<string, number | string>();
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
      const approval = mapCodexRequest(id, method, params);
      if (!approval) {
        server.rpc.respondError(id, -32601, "Not supported by Tjommie");
        return;
      }
      this.#approvals.set(approval.id, id);
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
    const rpcId = this.#approvals.get(approvalId);
    if (rpcId === undefined || !this.#server) return;
    this.#approvals.delete(approvalId);
    // The daemon remembers "Always allow" per bot and answers matching requests itself, so Codex only
    // ever gets accept or decline.
    this.#server.rpc.respond(rpcId, { decision: answer === "deny" ? "decline" : "accept" });
  }

  close(): void {
    const server = this.#server;
    this.#server = null;
    server?.close();
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

- [ ] **Step 5: Run the fast tests, the type check and the real tests**

Run: `npm test`
Expected: PASS (codex-parse 10 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

Run: `npm run test:real`
Expected: PASS, including the 3 CodexSession tests.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/providers/codex-parse.ts packages/daemon/src/providers/codex.ts packages/daemon/src/providers/codex-parse.test.ts packages/daemon/src/providers/codex.real.test.ts
git commit -m "Add the Codex session over app-server: turns, steering, stop and approvals

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 12: Approvals store

**Files:**
- Create: `packages/daemon/src/approvals.ts`
- Test: `packages/daemon/src/approvals.test.ts`

**Interfaces:**
- Consumes: `db.ts`, `server.ts` (`RpcError`), `@tjommie/protocol` (types `Approval`, `ApprovalDecision`), and in tests `bots.ts`, `chat.ts` and `testing.ts`.
- Produces:
  - `createApproval(db, { id, botId, tool, summary, detail, rule, messageId }): Approval`. It starts `pending`.
  - `getApproval(db, id): Approval`.
  - `resolveApproval(db, id, decision): Approval`.
    - It sets `allowed` or `denied`.
    - `always` with a rule also saves the rule.
    - It throws `RpcError("already_answered")` if the approval isn't pending.
  - `allowRules(db, botId): string[]`.
  - `expirePendingApprovals(db): Approval[]`. It marks every pending approval `denied` and returns them; used at startup.

- [ ] **Step 1: Write the failing test**

Create `packages/daemon/src/approvals.test.ts`:

```ts
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { allowRules, createApproval, expirePendingApprovals, getApproval, resolveApproval } from "./approvals.ts";
import { createBot } from "./bots.ts";
import { addMessage } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let botId: string;
let n = 0;

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" }).id;
});

function approval(rule: string | null = "Write") {
  const message = addMessage(db, { conversationId: botId, author: botId, kind: "approval", body: "hello.txt", status: "done" });
  return createApproval(db, { id: `a${++n}`, botId, tool: "Write", summary: "hello.txt", detail: "{}", rule, messageId: message.id });
}

describe("approvals store", () => {
  it("creates a pending approval", () => {
    expect(approval()).toMatchObject({ botId, tool: "Write", summary: "hello.txt", rule: "Write", status: "pending" });
  });

  it("allows once without saving the rule", () => {
    const a = approval();
    expect(resolveApproval(db, a.id, "once").status).toBe("allowed");
    expect(allowRules(db, botId)).toEqual([]);
  });

  it("saves the rule on Always allow, once per rule", () => {
    resolveApproval(db, approval().id, "always");
    resolveApproval(db, approval().id, "always");
    expect(allowRules(db, botId)).toEqual(["Write"]);
  });

  it("denies, and refuses to answer the same approval twice", () => {
    const a = approval();
    expect(resolveApproval(db, a.id, "deny").status).toBe("denied");
    expect(() => resolveApproval(db, a.id, "once")).toThrow(/already answered/);
  });

  it("closes every pending approval after a restart", () => {
    const pending = approval();
    const answered = approval();
    resolveApproval(db, answered.id, "once");
    expect(expirePendingApprovals(db).map((a) => a.id)).toEqual([pending.id]);
    expect(getApproval(db, pending.id).status).toBe("denied");
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./approvals.ts` doesn't exist.

- [ ] **Step 3: Write approvals.ts**

Create `packages/daemon/src/approvals.ts`:

```ts
import type { Approval, ApprovalDecision } from "@tjommie/protocol";
import { nowIso, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

type ApprovalRow = {
  id: string;
  bot_id: string;
  tool: string;
  summary: string;
  detail: string;
  rule: string | null;
  status: Approval["status"];
  message_id: string;
  created_at: string;
};

const toApproval = (row: ApprovalRow): Approval => ({
  id: row.id,
  botId: row.bot_id,
  tool: row.tool,
  summary: row.summary,
  detail: row.detail,
  rule: row.rule,
  status: row.status,
  messageId: row.message_id,
  createdAt: row.created_at,
});

export function createApproval(
  db: Db,
  a: { id: string; botId: string; tool: string; summary: string; detail: string; rule: string | null; messageId: string },
): Approval {
  db.prepare("INSERT INTO approvals (id, bot_id, tool, summary, detail, rule, status, message_id, created_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?)").run(
    a.id,
    a.botId,
    a.tool,
    a.summary,
    a.detail,
    a.rule,
    a.messageId,
    nowIso(),
  );
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

// After a restart the CLI that asked is gone, so nothing can answer these any more.
export function expirePendingApprovals(db: Db): Approval[] {
  const ids = (db.prepare("SELECT id FROM approvals WHERE status = 'pending' ORDER BY created_at, rowid").all() as { id: string }[]).map((row) => row.id);
  db.prepare("UPDATE approvals SET status = 'denied', decided_at = ? WHERE status = 'pending'").run(nowIso());
  return ids.map((id) => getApproval(db, id));
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS (approvals 5 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/approvals.ts packages/daemon/src/approvals.test.ts
git commit -m "Add the approvals store with per-bot Always allow rules

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Work queue and restart recovery

**Files:**
- Create: `packages/daemon/src/queue.ts`
- Test: `packages/daemon/src/queue.test.ts`

**Interfaces:**
- Consumes: `db.ts`, and in tests `bots.ts` and `testing.ts`.
- Produces:
  - `type WorkSource = "you"` (plan 1B adds more sources).
  - `type WorkStatus = "queued" | "running" | "done" | "failed" | "interrupted"`.
  - `type WorkItem = { id; botId; source: WorkSource; text; messageId: string | null; status: WorkStatus; createdAt }`.
  - `enqueue(db, { botId, source, text, messageId }): WorkItem` and `getWorkItem(db, id): WorkItem`.
  - `nextQueued(db, botId): WorkItem | null`. Oldest first; ties go to the one inserted first.
  - `setStatus(db, id, status): void`.
  - `lastItemForMessage(db, messageId): WorkItem | null`.
  - `recoverAfterRestart(db): WorkItem[]`. It turns `running` into `interrupted` and returns those items.

- [ ] **Step 1: Write the failing test**

Create `packages/daemon/src/queue.test.ts`:

```ts
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, lastItemForMessage, nextQueued, recoverAfterRestart, setStatus } from "./queue.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let botId: string;

beforeEach(() => {
  const root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
});

const add = (text: string, messageId: string | null = null) => enqueue(db, { botId, source: "you", text, messageId });

describe("work queue", () => {
  it("hands out queued work oldest first, even within one millisecond", () => {
    const [a, b, c] = [add("a"), add("b"), add("c")];
    expect(nextQueued(db, botId)?.id).toBe(a.id);
    setStatus(db, a.id, "running");
    expect(nextQueued(db, botId)?.id).toBe(b.id);
    setStatus(db, b.id, "done");
    expect(nextQueued(db, botId)?.id).toBe(c.id);
  });

  it("puts running work back in the queue with its start time cleared", () => {
    const item = add("x");
    setStatus(db, item.id, "running");
    setStatus(db, item.id, "queued");
    expect(getWorkItem(db, item.id).status).toBe("queued");
    expect(db.prepare("SELECT started_at FROM work_items WHERE id = ?").get(item.id)).toEqual({ started_at: null });
  });

  it("finds the latest attempt for a message", () => {
    add("first try", "m1");
    const retry = add("first try", "m1");
    expect(lastItemForMessage(db, "m1")?.id).toBe(retry.id);
    expect(lastItemForMessage(db, "nope")).toBeNull();
  });

  it("marks running work interrupted after a restart and leaves queued work alone (recovery after restart)", () => {
    const running = add("running");
    const queued = add("queued");
    setStatus(db, running.id, "running");
    expect(recoverAfterRestart(db).map((item) => [item.id, item.status])).toEqual([[running.id, "interrupted"]]);
    expect(getWorkItem(db, running.id).status).toBe("interrupted");
    expect(nextQueued(db, botId)?.id).toBe(queued.id);
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npm test`
Expected: FAIL. `./queue.ts` doesn't exist.

- [ ] **Step 3: Write queue.ts**

Create `packages/daemon/src/queue.ts`:

```ts
import { newId, nowIso, type Db } from "./db.ts";

// Where a piece of work came from. Plan 1B adds room posts, requests from other bots and routine runs.
export type WorkSource = "you";
export type WorkStatus = "queued" | "running" | "done" | "failed" | "interrupted";
export type WorkItem = { id: string; botId: string; source: WorkSource; text: string; messageId: string | null; status: WorkStatus; createdAt: string };

type WorkRow = { id: string; bot_id: string; source: WorkSource; text: string; message_id: string | null; status: WorkStatus; created_at: string };

const toItem = (row: WorkRow): WorkItem => ({
  id: row.id,
  botId: row.bot_id,
  source: row.source,
  text: row.text,
  messageId: row.message_id,
  status: row.status,
  createdAt: row.created_at,
});

export function enqueue(db: Db, work: { botId: string; source: WorkSource; text: string; messageId: string | null }): WorkItem {
  const id = newId();
  db.prepare("INSERT INTO work_items (id, bot_id, source, text, message_id, status, created_at) VALUES (?, ?, ?, ?, ?, 'queued', ?)").run(
    id,
    work.botId,
    work.source,
    work.text,
    work.messageId,
    nowIso(),
  );
  return getWorkItem(db, id);
}

export function getWorkItem(db: Db, id: string): WorkItem {
  const row = db.prepare("SELECT * FROM work_items WHERE id = ?").get(id) as WorkRow | undefined;
  if (!row) throw new Error(`No work item ${id}`);
  return toItem(row);
}

// Oldest first. rowid breaks ties between items created in the same millisecond.
export function nextQueued(db: Db, botId: string): WorkItem | null {
  const row = db.prepare("SELECT * FROM work_items WHERE bot_id = ? AND status = 'queued' ORDER BY created_at, rowid LIMIT 1").get(botId) as WorkRow | undefined;
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
```

- [ ] **Step 4: Run the tests and the type check**

Run: `npm test`
Expected: PASS (queue 4 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/queue.ts packages/daemon/src/queue.test.ts
git commit -m "Add the per-bot work queue with restart recovery

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Runner

The runner is the heart of the daemon. It owns each bot's live CLI session and turns queued work into turns. It steers your messages into running turns, turns provider events into chat messages, approval cards, usage rows and bot states, and handles stops, limits, crashes, idle shutdown and restarts.

**Files:**
- Create: `packages/daemon/src/runner.ts`
- Test: `packages/daemon/src/runner.test.ts`, `packages/daemon/src/runner.real.test.ts`

**Interfaces:**
- Consumes (exact names from Tasks 3 to 13):
  - `server.ts`: `Broadcast`, `RpcError`.
  - `bots.ts`: `getStoredBot`, `listStoredBots`, `setLimitedUntil`, `botPrompt`, `type StoredBot`.
  - `chat.ts`: `addMessage`, `getMessage`, `updateMessage`, `history`, `buildRecap`.
  - `approvals.ts`: `allowRules`, `createApproval`, `resolveApproval`, `expirePendingApprovals`.
  - `queue.ts`: `enqueue`, `nextQueued`, `setStatus`, `lastItemForMessage`, `recoverAfterRestart`, `type WorkItem`.
  - `setup.ts`: `accountFor`, `accountEnv`.
  - `cli.ts`: `resolveCli`.
  - Providers: `ClaudeSession`, `CodexSession`, `ProviderEvent`, `ProviderSession`, `StartOptions`.
- Produces:
  - `type SessionFactory = (bot: StoredBot, options: StartOptions) => ProviderSession` and `cliSessionFactory: SessionFactory`.
  - Pure helpers: `botStateOf({ signedIn, paused, limitedUntil, waiting, busy }, nowIso): BotState`, `activityBody(items): string`, `copyIntoInbox(homeFolder, file): string`.
  - `class Runner`, constructed with `{ db, broadcast, isSignedIn: (provider) => boolean, createSession?: SessionFactory }`. Methods:
    - Lifecycle: `start()`, `pump(botId)`, `pumpAll()`, `shutdown()`.
    - Reads: `toBot(stored): Bot` and `processId(botId)`.
    - Chat: `submit(botId, text, files): Message`, `retry(messageId): Message`, `stop(botId): Promise<void>`.
    - Approvals and settings: `answerApproval(approvalId, decision): Approval`, `botChanged(botId, providerChanged)`, `forget(botId)`.
  - Events it broadcasts: `message.added`, `message.updated`, `message.delta`, `bot.updated`, `approval.added`, `approval.resolved`, `notify`.

- [ ] **Step 1: Write the failing fast test**

Create `packages/daemon/src/runner.test.ts`:

```ts
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { activityBody, botStateOf, copyIntoInbox } from "./runner.ts";
import { tempRoot } from "./testing.ts";

describe("botStateOf", () => {
  const base = { signedIn: true, paused: false, limitedUntil: null, waiting: false, busy: false };
  const now = "2026-09-25T10:00:00.000Z";

  it("ranks states: needs setup, paused, limited, waiting, working, idle", () => {
    expect(botStateOf({ ...base, signedIn: false, paused: true }, now)).toBe("needs_setup");
    expect(botStateOf({ ...base, paused: true, busy: true }, now)).toBe("paused");
    expect(botStateOf({ ...base, limitedUntil: "2026-09-25T11:00:00.000Z", waiting: true }, now)).toBe("limited");
    expect(botStateOf({ ...base, limitedUntil: "2026-09-25T09:00:00.000Z", waiting: true, busy: true }, now)).toBe("waiting");
    expect(botStateOf({ ...base, busy: true }, now)).toBe("working");
    expect(botStateOf(base, now)).toBe("idle");
  });
});

describe("activityBody", () => {
  it("counts steps and shows the latest one", () => {
    expect(activityBody([{ summary: "Read notes.md" }])).toBe("1 step: Read notes.md");
    expect(activityBody([{ summary: "Read notes.md" }, { summary: "Run npm test" }])).toBe("2 steps: Run npm test");
  });
});

describe("copyIntoInbox", () => {
  it("copies an attachment without overwriting an earlier one with the same name", () => {
    const root = tempRoot();
    const home = join(root, "home");
    mkdirSync(home);
    const file = join(root, "CV final ñ.pdf");
    writeFileSync(file, "one");
    expect(copyIntoInbox(home, file)).toBe("CV final ñ.pdf");
    writeFileSync(file, "two");
    const second = copyIntoInbox(home, file);
    expect(second).toMatch(/^\d+-CV final ñ\.pdf$/);
    expect(readFileSync(join(home, "inbox", "CV final ñ.pdf"), "utf8")).toBe("one");
    expect(readFileSync(join(home, "inbox", second), "utf8")).toBe("two");
  });
});
```

- [ ] **Step 2: Write the real-CLI test**

Create `packages/daemon/src/runner.real.test.ts`:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { BotInput } from "@tjommie/protocol";
import { createBot, getStoredBot } from "./bots.ts";
import { killTree } from "./cli.ts";
import { openDb, type Db } from "./db.ts";
import { Runner, cliSessionFactory } from "./runner.ts";
import { ensureAccounts, listModels } from "./setup.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

type Sent = { event: string; data: any };
let runners: Runner[] = [];
afterEach(() => {
  for (const runner of runners) runner.shutdown();
  runners = [];
});

// A runner on a fresh database in a spaced, accented folder, using the real signed-in account folders.
function harness(root = tempRoot(), db: Db = openDb(join(root, "t.db"))) {
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
    firstAdded: (botId: string, kind: string) => events.find((e) => e.event === "message.added" && e.data.conversationId === botId && e.data.kind === kind),
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
});
```

- [ ] **Step 3: Run the fast tests to see them fail**

Run: `npm test`
Expected: FAIL. `./runner.ts` doesn't exist.

- [ ] **Step 4: Write runner.ts**

Create `packages/daemon/src/runner.ts`:

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
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, lastItemForMessage, nextQueued, recoverAfterRestart, setStatus, type WorkItem } from "./queue.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { accountEnv, accountFor } from "./setup.ts";

export type SessionFactory = (bot: StoredBot, options: StartOptions) => ProviderSession;

export const cliSessionFactory: SessionFactory = (bot, options) => {
  const cmd = resolveCli(bot.provider);
  if (!cmd) throw new RpcError("not_installed", `${bot.provider === "claude" ? "Claude Code" : "Codex"} is not installed`);
  return bot.provider === "claude" ? new ClaudeSession(cmd, options) : new CodexSession(cmd, options);
};

const IDLE_SHUTDOWN_MS = 10 * 60_000;
const RECAP_MESSAGES = 20;
const UNKNOWN_RESET_MS = 60 * 60_000;
const MAX_TIMER_MS = 2 ** 31 - 1;
const TURN_EVENTS = new Set<ProviderEvent["type"]>(["text", "text_end", "tool", "tool_done", "approval"]);

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
  activity: { id: string; items: ActivityItem[] } | null;
  stopRequested: boolean;
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

// Copies a file the person attached into the bot's inbox, never overwriting an earlier one.
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
  #live = new Map<string, Live>();

  constructor(deps: { db: Db; broadcast: Broadcast; isSignedIn: (provider: Bot["provider"]) => boolean; createSession?: SessionFactory }) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#isSignedIn = deps.isSignedIn;
    this.#createSession = deps.createSession ?? cliSessionFactory;
  }

  // Call once at startup: settle work a restart cut off, then carry on with whatever is queued.
  start(): void {
    for (const item of recoverAfterRestart(this.#db)) {
      if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "interrupted" }));
      this.#notice(item.botId, "error", "Tjommie restarted while this was running.", item.messageId ? { retryMessageId: item.messageId } : null);
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

  submit(botId: string, text: string, files: string[]): Message {
    const bot = getStoredBot(this.#db, botId);
    const attached = files.map((file) => copyIntoInbox(bot.homeFolder, file));
    const message = addMessage(this.#db, { conversationId: botId, author: "you", kind: "text", body: text, data: attached.length ? { files: attached } : null, status: "queued" });
    this.#emitMessage(message, "message.added");
    const prompt = attached.length ? `${text}\n\n[Attached files, now in your inbox folder: ${attached.join(", ")}]` : text;
    enqueue(this.#db, { botId, source: "you", text: prompt, messageId: message.id });
    const live = this.#liveFor(botId);
    if (!live.busy) this.pump(botId);
    else if (live.sent) void this.#steerQueued(botId); // otherwise the turn steers it in as soon as its first message is sent
    return getMessage(this.#db, message.id);
  }

  retry(messageId: string): Message {
    const previous = lastItemForMessage(this.#db, messageId);
    if (!previous) throw new RpcError("not_found", "There's nothing to retry for this message");
    enqueue(this.#db, { botId: previous.botId, source: previous.source, text: previous.text, messageId });
    this.#emitMessage(updateMessage(this.#db, messageId, { status: "queued" }));
    this.pump(previous.botId);
    return getMessage(this.#db, messageId);
  }

  async stop(botId: string): Promise<void> {
    const live = this.#live.get(botId);
    if (!live?.busy || !live.session) return;
    live.stopRequested = true;
    await live.session.interrupt();
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
    else this.#closeSession(live);
  }

  // Call before deleting a bot.
  forget(botId: string): void {
    const live = this.#live.get(botId);
    if (!live) return;
    if (live.idleTimer) clearTimeout(live.idleTimer);
    if (live.limitTimer) clearTimeout(live.limitTimer);
    this.#closeSession(live);
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
      this.#armIdle(live);
      return;
    }
    live.busy = true; // set before any await, so a second pump can't start another turn
    live.sent = false;
    live.item = item;
    live.stopRequested = false;
    live.limitHit = false;
    if (live.idleTimer) {
      clearTimeout(live.idleTimer);
      live.idleTimer = null;
    }
    setStatus(this.#db, item.id, "running");
    if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "sent" }));
    this.#emitBot(botId);
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
        activity: null,
        stopRequested: false,
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
      });
      created.onEvent((event) => this.#onEvent(bot.id, created, event));
      live.session = created;
      live.fresh = resumeId === null;
      live.sawSession = false;
      try {
        await created.start();
      } catch (error) {
        live.session = null;
        created.close();
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

  // Steers queued messages into the running turn, oldest first. One loop at a time keeps the order.
  async #steerQueued(botId: string): Promise<void> {
    const live = this.#liveFor(botId);
    if (live.steering) return;
    live.steering = true;
    try {
      for (let item = nextQueued(this.#db, botId); item && live.busy && live.sent; item = nextQueued(this.#db, botId)) {
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
    if (!live.text) {
      if (live.activity) this.#closeActivity(live);
      const message = addMessage(this.#db, { conversationId: botId, author: botId, kind: "text", body: "", status: "streaming" });
      live.text = { id: message.id, body: "" };
      this.#emitMessage(message, "message.added");
    }
    live.text.body += delta;
    this.#broadcast("message.delta", { id: live.text.id, conversationId: botId, delta });
  }

  #flushText(live: Live, status: Message["status"]): void {
    if (!live.text) return;
    this.#emitMessage(updateMessage(this.#db, live.text.id, { body: live.text.body, status }));
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
    const message = addMessage(this.#db, { conversationId: botId, author: botId, kind: "activity", body: activityBody([item]), data: { items: [item] }, status: "streaming" });
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
    if (item && live.limitHit) {
      setStatus(this.#db, item.id, "queued"); // runs again once the limit resets
      if (item.messageId) this.#emitMessage(updateMessage(this.#db, item.messageId, { status: "queued" }));
    } else if (item) {
      setStatus(this.#db, item.id, outcome);
      if (outcome === "interrupted") this.#notice(botId, "notice", "Stopped.", null);
      if (outcome === "failed") this.#notice(botId, "error", error ?? "Something went wrong.", item.messageId ? { retryMessageId: item.messageId } : null);
    }
    for (const approvalId of live.approvals.keys()) this.#showApprovalStatus(resolveApproval(this.#db, approvalId, "deny")); // the turn that asked is over
    live.approvals.clear();
    live.busy = false;
    live.sent = false;
    live.item = null;
    live.stopRequested = false;
    live.limitHit = false;
    if (live.reload) {
      live.reload = false;
      this.#closeSession(live);
    }
    this.#emitBot(botId);
    this.pump(botId);
  }

  #onExit(botId: string, live: Live, error: string | null): void {
    const resumeFailed = !live.fresh && !live.sawSession;
    live.session = null;
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

  #closeSession(live: Live): void {
    const session = live.session;
    live.session = null; // its exit event is ignored from now on
    session?.close();
  }

  #armIdle(live: Live): void {
    if (!live.session || live.idleTimer) return;
    live.idleTimer = setTimeout(() => {
      live.idleTimer = null;
      if (!live.busy) this.#closeSession(live);
    }, IDLE_SHUTDOWN_MS);
    live.idleTimer.unref();
  }

  #notice(botId: string, kind: "notice" | "error", body: string, data: Record<string, unknown> | null): void {
    this.#emitMessage(addMessage(this.#db, { conversationId: botId, author: "system", kind, body: body.slice(0, 500), data, status: "done" }), "message.added");
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

- [ ] **Step 5: Run the fast tests and the type check**

Run: `npm test`
Expected: PASS (runner 3 tests plus the earlier ones).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 6: Run the real tests**

Run: `npm run test:real`
Expected: PASS, including the 6 Runner tests. They take several minutes because each one drives a real CLI.

- [ ] **Step 7: Commit**

```bash
git add packages/daemon/src/runner.ts packages/daemon/src/runner.test.ts packages/daemon/src/runner.real.test.ts
git commit -m "Add the runner: turns, steering, approvals, limits, crash and restart handling

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Daemon wiring, terminal chat and end-to-end run

**Files:**
- Create: `packages/daemon/src/daemon.ts`, `packages/daemon/scripts/chat.ts`
- Modify: `packages/daemon/src/main.ts` (replace the scaffold), `README.md` (the Develop section)
- Test: `packages/daemon/src/daemon.real.test.ts`

**Interfaces:**
- Consumes everything above.
- Produces:
  - `startDaemon(options?: { root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory }): Promise<Daemon>`, where `type Daemon = { port: number; token: string; runner: Runner; close(): Promise<void> }`.
  - Commands, run from the repo root:
    - `npm start -w @tjommie/daemon` starts the daemon.
    - `npm run chat -w @tjommie/daemon -- "<name>" [claude|codex] [ask|folder|full]` opens the terminal chat.
  - This is the API plan 1D's UI talks to.

- [ ] **Step 1: Write the failing real end-to-end test**

Create `packages/daemon/src/daemon.real.test.ts`:

```ts
import { afterAll, beforeAll, expect, it } from "vitest";
import { startDaemon, type Daemon } from "./daemon.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

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
```

- [ ] **Step 2: Write daemon.ts, main.ts and the chat script**

Create `packages/daemon/src/daemon.ts`:

```ts
import type { ProviderSetup } from "@tjommie/protocol";
import { createBot, deleteBot, getStoredBot, listStoredBots, setPaused, updateBot } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb } from "./db.ts";
import { DEFAULT_PORT, dataPaths, readOrCreateToken } from "./paths.ts";
import { Runner, type SessionFactory } from "./runner.ts";
import { startServer, type Broadcast, type Handlers } from "./server.ts";
import { checkSetup, ensureAccounts, listModels, signIn } from "./setup.ts";

// The desktop app loads the UI from app://tjommie (plan 1D); `npm run dev` serves it on localhost:3000.
const ALLOWED_ORIGINS = ["app://tjommie", "http://localhost:3000"];

export type Daemon = { port: number; token: string; runner: Runner; close: () => Promise<void> };

export async function startDaemon(options: { root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory } = {}): Promise<Daemon> {
  const paths = dataPaths(options.root);
  const db = openDb(paths.db);
  const token = readOrCreateToken(paths.token);
  ensureAccounts(db, options.accountsDir ?? paths.accounts);
  let setup: ProviderSetup[] = await checkSetup(db);
  let broadcast: Broadcast = () => {};
  const runner = new Runner({
    db,
    broadcast: (event, data) => broadcast(event, data),
    isSignedIn: (provider) => setup.some((s) => s.provider === provider && s.signedIn),
    createSession: options.createSession,
  });

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
    "approvals.respond": ({ approvalId, decision }) => runner.answerApproval(approvalId, decision),
  };

  const server = await startServer({ port: options.port ?? DEFAULT_PORT, token, handlers, allowedOrigins: ALLOWED_ORIGINS });
  broadcast = server.broadcast;
  runner.start();
  return {
    port: server.port,
    token,
    runner,
    close: async () => {
      runner.shutdown();
      await server.close();
      db.close();
    },
  };
}
```

Replace `packages/daemon/src/main.ts` with:

```ts
import { startDaemon } from "./daemon.ts";
import { DEFAULT_PORT } from "./paths.ts";

const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
try {
  const daemon = await startDaemon({ port });
  console.log(`Tjommie daemon listening on http://127.0.0.1:${daemon.port}`);
  const stop = () => void daemon.close().finally(() => process.exit(0));
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
} catch (error) {
  if ((error as NodeJS.ErrnoException).code === "EADDRINUSE") {
    console.error(`Tjommie is already running: port ${port} is in use.`);
    process.exit(1);
  }
  throw error;
}
```

Create `packages/daemon/scripts/chat.ts`:

```ts
// Dev tool: chat with a bot through the running daemon before the desktop app exists.
//   npm run chat -w @tjommie/daemon -- "<bot name>" [claude|codex] [ask|folder|full]
// In the chat: /stop interrupts, /allow <id>, /always <id> and /deny <id> answer approvals, Ctrl+C quits.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { PROTOCOL_VERSION } from "@tjommie/protocol";
import { DEFAULT_PORT, dataPaths } from "../src/paths.ts";

const [name = "Tester", provider = "claude", autonomy = "folder"] = process.argv.slice(2);
const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
const token = readFileSync(dataPaths().token, "utf8").trim();
const ws = new WebSocket(`ws://127.0.0.1:${port}`);
const pending = new Map<number, (message: any) => void>();
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
  if (message.event === "message.delta" && d.conversationId === botId) process.stdout.write(d.delta);
  else if (message.event === "message.updated" && d.conversationId === botId && d.kind === "text" && d.author === botId && d.status !== "streaming") process.stdout.write("\n");
  else if (message.event === "message.added" && d.conversationId === botId && d.kind === "activity") console.log(`  [${d.body}]`);
  else if (message.event === "message.added" && d.conversationId === botId && (d.kind === "notice" || d.kind === "error")) console.log(`  (${d.body})`);
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
let bot = bots.find((b) => b.name === name);
if (!bot) {
  const models: any[] = await call("models.list", { provider });
  const model = (models.find((m) => m.isDefault) ?? models[0]).id;
  bot = await call("bots.create", { name, color: "blue", shape: "circle", instructions: "", provider, model, autonomy });
}
botId = bot.id;
console.log(`Chatting with ${bot.name} (${bot.provider}, ${bot.autonomy}, ${bot.state}). /stop interrupts, Ctrl+C quits.`);

const decisions: Record<string, string> = { "/allow": "once", "/always": "always", "/deny": "deny" };
for await (const line of createInterface({ input: process.stdin })) {
  const [command, arg] = line.trim().split(/\s+/, 2);
  try {
    if (command === "/stop") await call("chat.stop", { botId });
    else if (decisions[command] && arg) await call("approvals.respond", { approvalId: arg, decision: decisions[command] });
    else if (line.trim()) await call("chat.send", { botId, text: line });
  } catch (error) {
    console.log(`  (${(error as Error).message})`);
  }
}
```

In `README.md`, replace the code block under "## Develop" with:

```sh
npm install
npm run signin -w @tjommie/daemon    # once: sign Tjommie's own Claude and Codex accounts in
npm start -w @tjommie/daemon         # the daemon, on http://127.0.0.1:47100
npm run chat -w @tjommie/daemon -- "Researcher" claude   # chat with a bot from the terminal
npm run dev        # UI at http://localhost:3000
npm run desktop    # Electron window showing that UI (keep npm run dev running)
npm test           # fast tests
npm run test:real  # tests that drive the real Claude and Codex CLIs (needs the sign-in above)
npm run typecheck
npm run lint
npm run build      # static export to packages/ui/out
```

- [ ] **Step 3: Run the fast tests and the type check**

Run: `npm test`
Expected: PASS (every fast test).

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 4: Run all the real tests**

Run: `npm run test:real`
Expected: PASS, including the new end-to-end test.

- [ ] **Step 5: Manual end-to-end run (with the user)**

This is Plan 1A's demo. In PowerShell from the repo root:

1. **Start the daemon.** Run `npm start -w @tjommie/daemon`. Expected: `Tjommie daemon listening on http://127.0.0.1:47100`.
2. **Check health.** In a second window, run `curl.exe http://127.0.0.1:47100/health`. Expected: `{"ok":true,"protocol":1}`.
3. **Chat with Claude.** Run `npm run chat -w @tjommie/daemon -- "Researcher" claude`.
   - Ask something. The reply streams in.
   - Ask for a long task (for example "list the files in your home folder slowly, one command at a time"), then send "also say BANANA at the end". The reply includes it.
   - Start another long task and type `/stop`. Expected: `(Stopped.)`.
4. **Approvals.** Run `npm run chat -w @tjommie/daemon -- "Careful" claude ask` and ask it to create a file.
   - A "Waiting for you" line appears. Answer with `/always <id>`, and the file is created.
   - Ask for a second file. It's created without asking, because Always allow saved `Write` for this bot.
5. **Chat with Codex.** Run `npm run chat -w @tjommie/daemon -- "Writer" codex` and repeat the chat and steering checks.
6. **Restart.** Stop the daemon (Ctrl+C) while a bot is mid-reply, start it again, and reconnect with the same chat command.
   - The chat shows `(Tjommie restarted while this was running.)`.
   - The next message continues the same conversation.

Record anything that surprised you in the task report. Plan 1B builds on this daemon.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/daemon.ts packages/daemon/src/daemon.real.test.ts packages/daemon/src/main.ts packages/daemon/scripts/chat.ts README.md
git commit -m "Wire the daemon together and add a terminal chat for trying it out

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
