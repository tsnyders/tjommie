# Tjommie: design spec

- **Date:** 2026-09-25
- **Status:** every section was approved in chat on 2026-09-25. This written version is waiting for review.
- **Replaces:** the architecture, AI, phase and open-question sections of `2026-09-24-grokbot-clone-design.md` (rev 2). The wireframes and their spec (`docs/design/wireframe-spec.md`, `docs/wireframes/`) still apply.
- **Repo:** `github.com/TSnyders/tjommie` (not created yet). Publish with the TSnyders GitHub account only.

Bot names in this document ("Researcher", "Writer") and in the wireframes ("Pnet apply", "CV Writer") are examples. Tjommie ships with no bots.

## 1. Summary

Tjommie is a free, open-source desktop and phone app. People create AI bots that chat with them, work on their PC, talk to each other and run routines. It is modelled on Grok Bot (SpaceXAI, beta launched 2026-08-11), with one big difference: the bots run on the user's own computer, powered by the official Claude Code and Codex CLIs, signed in with the user's own accounts.

### 1.1 What was asked for

- An agent harness where bots talk to each other, like Grok Bot.
- Providers through their official CLIs, driven the way Paseo does it: Claude Code and Codex. Antigravity later.
- Several accounts per provider on one install.
- Phones connect by scanning a QR code, other PCs by pasting a long pairing code, from anywhere.
- Phases: 0 UI (done), 1 the app, 2 CLIs and accounts, 2.5 phone UI, 3 remote access. Phase 4 (the computer panel) was added during planning.

### 1.2 Decisions

| Topic | Decision |
|---|---|
| Audience | A product for others. Open source, free. |
| License | Apache-2.0, same as Paseo (default, change if you prefer) |
| Name | Tjommie |
| What bots do | General work (browser, files) and coding (attached project folder) |
| Build | Fresh build. Paseo (Apache-2.0) is the reference; small pieces are copied with credit |
| Providers | Claude Code and Codex CLIs. Antigravity (`agy`) deferred |
| Claude integration | Run the unmodified `claude` program directly. No Claude Agent SDK |
| Accounts | Several per provider; each bot pinned to one (Phase 2) |
| Usage limit reached | Ask before switching accounts; automatic fallback only to API-key accounts |
| Autonomy | Per bot: Always ask, Own folder (default), Full auto |
| Conversations | One ongoing chat per bot |
| Messaging a busy bot | Your message steers its current work; bot requests and routine runs wait in line |
| Bot to bot | Asynchronous `message_bot` through the app; Team room with @mentions; chain limit 5 |
| Computer | Browser per bot in Phase 1; live panel, Take over and Logins vault in Phase 4 |
| Platforms | Windows first, macOS later. Android first, iOS later (needs a Mac) |
| UI stack | One web UI: Next.js static export + shadcn/ui + Tailwind. Electron on desktop, Capacitor on the phone |
| Phone scope | Full parity with the desktop |
| Remote reach | Anywhere, through an end-to-end encrypted relay (public one + self-host); direct on the same Wi-Fi |
| Usage and cost | Token, limit and cost tracking like T3 Code, plus spending caps on API keys |
| Phase split | Phase 1 runs both CLIs with the PC's existing logins; Phase 2 adds multi-account |
| Testing | Real CLIs (no fake CLI), screenshot checks, a manual end-to-end run per phase |

### 1.3 Assumptions

1. Bots live on one host PC. Phones and other PCs are remotes for that host. No syncing between hosts.
2. One person per install. "Accounts" means provider accounts, not app users.
3. The app never handles provider passwords or tokens. Each account is a normal CLI login in its own folder.
4. Routines are time-based only.
5. Bots can message only the bots on their "Can message" list, and can't create bots.

## 2. How people use it

1. **First run.** Install Tjommie on Windows. It checks that Claude Code and Codex are installed and signed in, and says exactly what to do if not. Then it suggests creating a first bot.
2. **Create a bot** (WF3). Pick an avatar and name, write what it should do, choose provider and model, its folder (its own, or a project), autonomy, browser on or off, which bots it can message, and routines. Or start from a Suggestion.
3. **Chat** (WF1). The reply streams in. Tool activity collapses into one line per turn. Approvals appear as "Waiting for you" cards and files as cards. Message the bot while it works to steer it; Stop interrupts it.
4. **Bots work together.** When a bot asks another for help, a one-line row appears in both chats and the full exchange appears in the Team room. You can join in.
5. **Team room.** @mention one or more bots (or @everyone) to ask several at once.
6. **Routines.** For example: "Every weekday at 9:00, do X, stop after 3 hours". Runs show in the bot's chat. You're notified only when a bot needs you, a routine fails, or an account hits its limit.
7. **Accounts** (Phase 2). Add several Claude and Codex accounts (subscription or API key), pin bots to them, see usage and cost, and decide what happens at limits.
8. **Phone** (Phase 3). Scan a QR code and get the same bots and chats, from anywhere, with a push when a bot needs you.
9. **Watch and take over** (Phase 4). See a bot's browser live and take over for PINs and CAPTCHAs.

## 3. Architecture

```
Host PC (Windows)
┌──────────────────────────────────────────────────────────────┐
│ Desktop app: Electron (window, tray, notifications)          │
│   └─ UI: Next.js static export + shadcn/ui + Tailwind        │
│             │ WebSocket on 127.0.0.1 + token                 │
│ Daemon: Node 24 + TypeScript, its own process                │
│   ├─ Store: SQLite (node:sqlite)                             │
│   ├─ Runner: one CLI process per busy bot                    │
│   │     claude -p (stream-json)    codex app-server          │
│   ├─ App MCP server (HTTP): list_bots, message_bot,          │
│   │     share_file, add_routine, Claude approval hook        │
│   ├─ Scheduler: routines                                     │
│   └─ Browser: Playwright MCP, one Chrome profile per bot     │
└──────────────────────────────────────────────────────────────┘
Phase 3: daemon ⇄ relay (Cloudflare Worker) ⇄ phone (Capacitor) or another PC
         daemon ⇄ phone directly on the same Wi-Fi
```

- **Daemon.** Its own process from day one. It owns the database, runs the CLIs, the scheduler and the app MCP server, and serves the WebSocket API. Closing the window leaves it running; the app stays in the tray.
- **Desktop app.** A thin Electron shell: starts the daemon, shows the UI, owns the tray and notifications. The Windows installer is built with electron-builder.
- **UI.** A Next.js static export and a pure client of the daemon API, so the same bundle runs in Electron, in Capacitor and on other PCs. A static export can't have dynamic path segments, so pages take IDs as query parameters (`/chat?bot=...`). The build uses the system font (Segoe UI); the wireframes used Inter. Colors come from `docs/design/brief.md` section 3. Motion follows section 6 of `2026-09-24-grokbot-clone-design.md`.
- **Storage.** SQLite through Node's built-in `node:sqlite`, so there's no native build step.
- **Local security.** The daemon listens on 127.0.0.1 only and requires a random token kept in a file only the Windows user can read. It checks the Host and Origin headers so web pages can't reach it.

### 3.1 Repo (npm workspaces)

```
packages/protocol   zod schemas for every WebSocket message, shared types
packages/daemon     the daemon
packages/ui         Next.js app (static export)
packages/desktop    Electron shell
packages/mobile     Capacitor Android app (Phase 3)
packages/relay      Cloudflare Worker (Phase 3)
```

### 3.2 On disk: `%LOCALAPPDATA%\Tjommie\`

```
tjommie.db
daemon-token
bots\<bot-id>\           the bot's home folder (its working folder unless a project is attached)
    notes.md             the bot's memory
    inbox\               files you attach
    chrome-profile\      the bot's own Chrome profile
accounts\<account-id>\   CLAUDE_CONFIG_DIR or CODEX_HOME for that account (Phase 2)
logs\
```

### 3.3 WebSocket API

- The client sends `hello` with its protocol version. The daemon answers with its version and capabilities. On a mismatch, the UI shows "Update Tjommie on this device" instead of failing in strange ways. (Pattern from Paseo's `packages/protocol/src/messages.ts`.)
- **Requests** (client to daemon) carry an id and get exactly one response. Groups:
  - Phase 1: `bots.*`, `chat.*` (send, stop, history), `approvals.respond`, `room.*`, `exchanges.get`, `routines.*`, `setup.status`, `settings.*`
  - Phase 2: `accounts.*`, `usage.query`, `terminal.*`
  - Phase 3: `devices.*`, `pairing.create`
- **Events** (daemon to client): `message.added`, `message.delta` (streaming text), `message.updated`, `bot.state`, `approval.added`, `approval.resolved`, `routine.run`, `usage.updated`, `setup.changed`.
- Every message is a zod schema in `packages/protocol`, validated at both ends.

## 4. Bots, chats and sessions

### 4.1 A bot

| Field | Notes |
|---|---|
| Name, avatar | 10 colors and 8 blob shapes (`docs/design/brief.md` section 5) |
| Instructions | "What should this bot do?". Passed to the CLI as a system prompt addition; never written into the folder, so a project's own `CLAUDE.md` and `AGENTS.md` stay untouched |
| Provider, model | Claude or Codex. Claude models by alias (`sonnet`, `opus`, `haiku`) so bots get the newest version; Codex models from `model/list` |
| Account | Phase 2. Until then the PC's existing login |
| Folder | The bot's home folder (default) or an attached project folder. With a project, the home folder is added as an extra allowed folder (Claude `--add-dir`, Codex writable roots) |
| Autonomy | Always ask, Own folder (default), Full auto (section 5.4) |
| Can message | The bots it may ask for help |
| Browser | On or off |
| Routines | Section 7 |
| Paused | Its queue holds and its routines skip |

### 4.2 One ongoing chat, many CLI sessions underneath

- **The chat lives in the database, not in the CLI.** Messages, handoff rows, routine runs, approval cards and errors are all records in the bot's chat, so the chat never resets.
- **One session at a time.** A bot keeps one CLI session and resumes it for every piece of work (`--resume` for Claude, `thread/resume` for Codex). Each CLI compacts a long context on its own.
- **When a fresh session starts.** Only when the provider changes, the account changes (Phase 2), or resuming fails. Changing the model keeps the session when the CLI allows it. A fresh session gets the bot's instructions, a pointer to `notes.md`, and a recap of the last 20 chat messages.
- **Memory.** A plain `notes.md` in the bot's home folder. Its instructions tell it to write down preferences and facts worth keeping, and to read the file when a session starts. Users can open and edit it.
- **Idle shutdown.** A CLI process that's been idle for 10 minutes is shut down and resumed on the next piece of work.

### 4.3 Work queue

Each bot does one turn at a time. The queue is stored in the database, so nothing is lost if the app restarts.

| Source | If the bot is busy | The answer goes to |
|---|---|---|
| You, in the bot's chat | Steered into the current turn. If the CLI can't steer, it goes first in line and is labelled "Queued" | The bot's chat |
| You, in the Team room | First in line. Not steered, so the answer lands in the room | The Team room |
| A reply to the bot's own request | Steered into the current turn (same fallback) | The bot's own work |
| A request from another bot | Waits in line | Back to the asking bot; shown in the Team room |
| A routine run | Waits in line | The bot's chat, under a routine label |

"First in line" means ahead of bot requests and routine runs. Stop interrupts the current turn. Pause holds the queue.

### 4.4 Where things show

- **The bot's own chat** shows your conversation with it in full, plus one-line rows for everything else: exchanges with other bots ("Researcher asked Writer to draft the summary · 2 messages ›") and answers it gave in the Team room. Routine runs show in full under a "Routine: Morning check" label, as in WF1.
- **Files.** Files you drop on the composer are copied to `inbox\`. A bot attaches a file to its reply with `share_file`, which shows as a file card (the PDF card in WF1).

### 4.5 States

| State | Meaning |
|---|---|
| Working | A turn is running |
| Waiting for you | An approval, question or limit card is pending |
| Idle | Nothing to do |
| Paused | You paused it |
| Limited | Its account hit a usage limit. Shows the reset time |
| Needs setup | Its CLI is missing or signed out |

## 5. Running the CLIs

### 5.1 Adapters

Each CLI talks in its own format. An adapter is a small translator per CLI. It turns that CLI's output into one shared format, and turns the app's commands ("send", "steer", "stop") into what that CLI expects. Everything else (chat, approvals, routines, bot to bot) sees only the shared format and doesn't care which CLI a bot runs on. Adding a CLI later means one more adapter. Paseo calls these "providers" (section 17).

```ts
interface ProviderSession {
  start(opts: {
    cwd: string; extraDirs: string[]; resumeId?: string; model: string;
    instructions: string; autonomy: Autonomy; allowRules: string[];
    mcpServers: McpServerConfig[]; env: Record<string, string>;
  }): Promise<void>
  send(text: string, files?: string[]): void   // start a turn
  steer(text: string): boolean                   // false: not possible now, caller queues instead
  interrupt(): void
  close(): void
  onEvent(listener: (event: ProviderEvent) => void): void
}

type ProviderEvent =
  | { type: 'session'; cliSessionId: string }
  | { type: 'text'; delta: string }
  | { type: 'activity'; tool: string; summary: string; status: 'started' | 'done' | 'failed' }
  | { type: 'approval'; id: string; summary: string; details: unknown }
  | { type: 'usage'; model: string; tokens: TokenCounts; costUsd?: number }
  | { type: 'limit'; state: 'ok' | 'near' | 'reached'; resetsAt?: string; usedPercent?: number }
  | { type: 'turn_done' }
  | { type: 'error'; message: string; retryable: boolean }
```

### 5.2 Claude

We run the installed, unmodified `claude` program directly. We don't use the Claude Agent SDK (section 10).

```
claude -p --input-format stream-json --output-format stream-json --verbose
  --include-partial-messages
  --model <alias>
  --append-system-prompt <bot instructions + memory rules>
  --mcp-config <per-session JSON: tjommie (HTTP), browser (stdio)>
  --permission-prompt-tool mcp__tjommie__approve
  --permission-mode <per autonomy> [--allowedTools <saved rules>]
  [--add-dir <home folder>] [--resume <session id>]
```

- **Environment.** Remove `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and `CLAUDE_CODE_OAUTH_TOKEN` so the CLI uses its own sign-in. Phase 2 sets `CLAUDE_CONFIG_DIR` per account, or `ANTHROPIC_API_KEY` for API-key accounts.
- **Approvals.** Claude calls our `approve` tool with the tool name and input. If a saved rule matches, we allow it at once. Otherwise we show a card, keep the call open until the user answers, and return allow or deny.
- **Steering.** Write a new user message line to stdin while a turn runs (Phase 1 spike 1).
- **Stop.** Send the stream-json interrupt control message on stdin. If that doesn't work, end the process and resume (spike 2).
- **Usage and limits.** The `result` message carries `usage`, `total_cost_usd` and per-model usage. `rate_limit_event` carries the status (`allowed`, `allowed_warning`, `rejected`), `resetsAt` and the limit type. A quota refusal arrives as an assistant message with `error: "rate_limit"`.

### 5.3 Codex

We run `codex app-server`: JSON-RPC over stdio, the interface OpenAI built for its IDE extension. Verified in codex-cli 0.155.1:

- **Threads and turns.** `thread/start` takes the working folder, model, `approvalPolicy`, `sandbox`, `developerInstructions` (the bot's instructions) and MCP servers. Also `thread/resume`, `turn/start`, `turn/steer` and `turn/interrupt`.
- **Approvals.** The server sends `item/commandExecution/requestApproval` and `item/fileChange/requestApproval` and waits for our answer.
- **Usage and limits.** `thread/tokenUsage/updated` gives input, cached, output and reasoning tokens. `account/rateLimits/updated` gives `usedPercent` and `resetsAt`. A turn fails with `UsageLimitExceeded` at the limit. Codex reports no cost; see section 8.3.
- **Environment.** Remove `OPENAI_API_KEY` and `CODEX_API_KEY`. Phase 2 sets `CODEX_HOME` per account.
- **Windows.** npm installs `codex` as a `.ps1`/`.cmd` shim, so we resolve and spawn the real executable (spike 7).

### 5.4 Autonomy

Each level maps onto the CLI's own permission system. We don't build a sandbox.

| Level | Claude | Codex |
|---|---|---|
| Always ask | `default` mode: every change becomes a card | approval `untrusted`, `read-only` sandbox |
| Own folder (default) | `acceptEdits`: edits inside its folders go through; shell commands and anything outside become a card | approval `on-request`, `workspace-write` sandbox on its folders, network on |
| Full auto | `bypassPermissions` | approval `never`, full access |

Claude has no operating-system sandbox on Windows, so under "Own folder" Claude bots ask about every shell command. Choosing Full auto shows a warning first.

### 5.5 Approval cards

A "Waiting for you" card says exactly what the bot wants to do ("Run `npm install`", "Edit `...\report.docx`") and offers **Allow once**, **Always allow** or **Deny**. Always allow saves a rule on that bot (for example "commands starting with `npm test`"). From then on the daemon answers matching requests itself, for both CLIs; for Claude the rule is also passed as `--allowedTools` (`Bash(npm test:*)`) so it doesn't even ask. Cards never time out. They send a desktop notification now and a phone push from Phase 3.

The same card style carries the other "needs you" moments: a routine proposed from chat (section 7) and an account switch at a limit (section 8).

### 5.6 The app's MCP server

Streamable HTTP at `http://127.0.0.1:<port>/mcp`. Each CLI session gets its own bearer token, which tells the daemon which bot and session is calling (Paseo pattern: `runtime-mcp-config.ts`).

| Tool | What it does |
|---|---|
| `list_bots` | The bots this bot may message: name, one-line description, state |
| `message_bot(bot, message)` | Queues a request for another bot and returns at once: "Sent. Writer will reply." |
| `share_file(path, caption?)` | Attaches a file from the bot's folders to its reply as a file card |
| `add_routine(name, days, times, prompt, time_limit?)` | Proposes a routine as a "Waiting for you" card. Nothing is saved until you approve |
| `approve` | Internal: Claude's permission prompt hook. Not meant for the model |

Bots also keep any MCP servers the user set up in the CLI itself (we don't pass `--strict-mcp-config`). A per-bot Tools picker is listed under Later.

### 5.7 Browser

- **Playwright MCP.** Each bot with the browser on gets Playwright MCP (`@playwright/mcp`, bundled with Tjommie rather than fetched with npx) pointed at `bots\<id>\chrome-profile`. It runs headed: a real Chrome window (Edge if Chrome isn't installed), separate from the user's own Chrome. Sites you sign in to by hand in that window stay signed in.
- **Who starts Chrome** is decided by spike 5: either Playwright MCP starts it, or the daemon does (with a debugging port) and Playwright MCP attaches. The second option lets the daemon show or close the window in Phase 1 and stream it in Phase 4.
- **Right panel.** In Phase 1 it shows a "Show browser window" button in place of the live screen.

### 5.8 Setup check

At startup, and whenever a bot hits a sign-in error, the daemon runs `claude --version`, `claude auth status`, `codex --version` and `codex login status`. Bots whose CLI is missing or signed out show "Needs setup" with the exact command to run (Phase 1) or a "Sign in" button (Phase 2).

## 6. Bots talking to each other

Example: Researcher runs on Claude, Writer runs on Codex.

1. **Ask.** Researcher calls `message_bot("Writer", "Turn these notes into a one-page summary.")`.
2. **Check and deliver.** The daemon checks that Writer is on Researcher's "Can message" list and that the chain is under its limit. It queues the request for Writer and answers Researcher at once: "Sent. Writer will reply." Researcher keeps working. Nobody waits inside a tool call, so two bots asking each other can't freeze each other.
3. **Answer.** The request becomes Writer's next turn: "Message from Researcher: ...". What Writer says at the end of that turn is the reply; no reply tool is needed. Files it shares travel with the reply.
4. **Reply.** The daemon delivers "Writer replied: ..." into Researcher's work, steered into its current turn if it has one.

Neither CLI knows the other exists. Each only sees plain text through the daemon, so Claude and Codex bots talk in either direction. This is Paseo's `send_agent_prompt` idea done our way: requests queue instead of replacing the target's work, the daemon checks permission first, and every exchange is visible.

### 6.1 Safety rails

- A bot can only message bots on its "Can message" list, and never itself.
- **Chains.** Each message from you and each routine run starts a chain, and all work that follows carries its chain id. Each `message_bot` call counts one. At 5 (adjustable in Settings) the tool answers "Limit reached, ask the user".
- Bots can't create bots.
- Errors come back to the bot as plain reasons (not allowed, limit reached, bot not found), and the bot tells you.

### 6.2 Team room

- There's one room, and every bot is a member. It's pinned at the top of the rail.
- **Talking to bots.** You @mention bots (or @everyone). Each mentioned bot gets your post plus the last 20 room messages and answers in the room. A post without a mention gets no reply, and the composer hints "@ to mention a bot".
- **The bot-to-bot feed.** Every exchange also appears in the room as posts ("Researcher to Writer: ..."), so the room is the place to see what the bots are saying to each other.
- **Bots don't react to @mentions in other bots' posts.** Bots reach each other only through `message_bot`, so the chain limit always applies.
- **Exchange view.** Opened from a handoff row, it shows the request, the reply and any files. Its composer posts to the Team room with both bots already @mentioned.

## 7. Routines and notifications

- **A routine** belongs to one bot. It has a name, days of the week, one or more times (in the user's time zone), what to do (plain text), and an optional time limit ("stop after 3 hours"). It's stored as exactly that, with no cron expressions and no cron parser.
- **Creating one.** From the bot's Routines row (WF3, and the WF1 right panel), or by asking the bot in chat. The bot then calls `add_routine`, which shows a card, and nothing is created until you approve.
- **Running.** Every 30 seconds the daemon checks for due runs. A due run joins the bot's queue and shows in its chat under "Routine: <name>".
  - At the time limit the bot is stopped and the run is marked "Stopped at time limit".
  - If the run needs you, it waits and notifies you. The time limit still applies.
  - If the bot is paused, the run is skipped.
  - If the PC was off or asleep, or the app was closed, the run is marked "Missed". Missed runs don't all fire at once later.
- **History.** The last 20 runs per routine, each Done, Stopped, Failed, Missed or Skipped.
- **Suggestions.** The New bot screen offers built-in templates (the three in WF3) that pre-fill the name, avatar, instructions and suggested routines.
- **Notifications.** Desktop now, phone push from Phase 3. Sent only when a bot is waiting for you, a routine failed, an account hit its limit, or (Phase 2) an API key reached 80% of its cap.

## 8. Accounts, limits, usage and cost (Phase 2)

### 8.1 Accounts

- **Where.** Settings → Accounts (WF4's "Models" nav item becomes "Accounts"). Designed in OpenPencil first. Accounts are grouped by provider, each with a name you choose ("Claude (Work)", "Claude (Personal)", "Codex (Main)"), its type, sign-in status and usage.
- **Subscription sign-in** (Claude Pro/Max, ChatGPT plans):
  - **Its own folder.** The app creates `accounts\<id>\` and points that CLI at it: `CLAUDE_CONFIG_DIR` for Claude, `CODEX_HOME` for Codex. It's the same approach as the `kuumba-paseo` fork's provider profiles.
  - **Signing in.** The CLI's own sign-in runs in a small terminal view inside the app (xterm.js in the UI, a pseudo-terminal in the daemon): `claude auth login`, or `codex login` / `codex login --device-auth`, with that account's folder. You open the link on the PC or your phone, approve, and paste back a code if asked.
  - **The app never handles your login.** The CLI stores it in that folder. The app never sees or reads your password or token.
- **API key** (pay per use):
  - **Stored safely.** Kept in Windows Credential Manager.
  - **Claude** gets the key as `ANTHROPIC_API_KEY` in its own process only.
  - **Codex** gets it through `codex login --with-api-key` into the account's folder, with Codex set to keep credentials in the OS keyring.
  - **Fallback.** A key can be marked as the fallback for its provider.
  - **Spending cap.** Each key gets a spending cap, set when the key is added.
- **Bots** are pinned to one account. The "Model" row becomes "Account and model". New bots get the provider's first account.

### 8.2 When an account hits its limit

1. The bot stops where it is and shows "Limited until 3:45 PM".
2. If the provider has an API-key fallback, the bot carries on with it automatically, and a note in the chat says so. It switches back once the limit resets.
3. Otherwise you get a card: "Claude (Work) hit its 5-hour limit, resets 3:45 PM", with **Switch Writer to Claude (Personal)** or **Wait until 3:45 PM**. Waiting resumes the bot by itself at the reset time. Switching changes the bot's pinned account.
4. Switching accounts starts a fresh CLI session (each account keeps its own sessions) with the recap from section 4.2. The chat carries on unchanged.
5. If a login expires, the bot shows "Needs setup" with a "Sign in again" button that opens Accounts.

### 8.3 Usage and cost

- **Recording starts in Phase 1.** The daemon saves one record per turn: bot, account, model, tokens (input, cached, output, reasoning) and cost. The screens ship in Phase 2.
- **Cost.**
  - **Claude** reports its own cost per turn (`total_cost_usd`).
  - **Codex** costs are tokens multiplied by LiteLLM's public per-model price list (the list `ccusage` uses), refreshed daily with a bundled copy as fallback.
- **Screens.**
  - **Usage page** (Settings → Usage): today, this week and this month; tokens and cost by bot, account and model; a daily bar chart; each account's limit status.
  - **Each bot's right panel:** "Today: 1.2M tokens · $3.40".
  - **Accounts screen:** today and this month per account.
- **Limits shown.** Codex shows live numbers ("62% of 5-hour limit, resets 3:45 PM"). Claude shows OK, Near limit or Limit reached, plus the reset time, because that's all its headless output reports.
- **Money, labelled honestly.**
  - API-key accounts show "Spent: $12.40", which is real money.
  - Subscription accounts show "API value: $48.10", which is what the same tokens would cost at API prices.
- **Spending caps** on API keys: a notification at 80%, and at 100% bots stop using that key.

## 9. Phones and remote access (Phases 2.5 and 3)

### 9.1 Phase 2.5: phone screens

- **Design in OpenPencil first:** bot list (home), bot chat, right-panel contents as a pull-up sheet (routines, activity, usage), Team room, exchange view, approval cards, New bot, Accounts with the sign-in terminal, Usage, Settings, Pair / scan, Devices.
- **Build** them into the same web UI as a narrow-screen layout, testable by narrowing the desktop window.

### 9.2 Phase 3: connecting from anywhere

1. **Always on.** The app starts at Windows login, hidden in the tray. The daemon is a child of the Electron main process. Closing the window keeps both running; Quit in the tray stops both. If the PC sleeps, remote devices see "Host offline". The desktop app gets auto-update (electron-updater, GitHub Releases).
2. **Pairing.** On the host, Settings → Devices → Pair a device shows a QR code and the same pairing code as long text with a Copy button. The code works once and expires in 10 minutes.
   - **Contents:** `{ v, relay, hostId, hostKey, secret, lan: [ip:port], expires }`, base64url-encoded. The QR code holds `tjommie://pair#<code>`.
   - **Phone:** open the app and scan.
   - **Another PC:** install Tjommie, choose "Connect to another computer" and paste the code.
3. **Handshake.** Noise IK, with the one-time secret as a pre-shared key for the first connection. The device's key goes into the host's device list and the code is burned. Later connections use Noise IK without the pre-shared key, and the host refuses device keys it doesn't know or has revoked. The Noise implementation is an existing, maintained library, chosen in the Phase 3 plan. We don't write our own crypto.
   - Every connection gets fresh keys, so a later key leak doesn't expose old traffic.
   - Every message is encrypted and numbered, so it can't be replayed.
4. **Keys.** The host's Noise key and relay login key are kept in Windows Credential Manager. On the phone, the device key is kept in the Android Keystore.
5. **Devices list.** Name, date paired and last seen, plus Rename and **Revoke**. Revoking cuts one device off at once and leaves the others alone. Every paired device has full control and can pair or revoke devices. The host's own window always works.
6. **Relay.** A Cloudflare Worker with one Durable Object per host.
   - **Host login.** The host logs in by signing a challenge, so nobody can pretend to be a host or knock it offline.
   - **What it forwards.** Devices connect by host id, and the relay forwards encrypted frames.
   - **What it can see.** Only IP addresses, timing, message sizes and host ids.
   - **Who runs it.** You run a public relay. A guide lets users deploy their own, and the relay address is a setting.
7. **Same Wi-Fi.** The host also listens on the local network (Windows Firewall asks once). Devices try the LAN addresses from the pairing code first, give each one second, then use the relay. The encryption is the same either way.
8. **Android app.** Capacitor wrapping the same UI, with a native barcode scanner, push (Firebase Cloud Messaging) and secure key storage. Ship the APK on GitHub Releases first; the Play Store comes later.
9. **Push.**
   - **How tokens travel.** The device registers its push token with the host over the encrypted connection. The host asks the relay to deliver "A bot needs you" to that token. The relay holds our Firebase credentials but no device tokens.
   - **What a push contains.** No bot content. Tapping it opens the app, which loads the details over the encrypted connection.
10. **Other PCs.** The desktop app switches between hosts ("This PC", "Home desktop") from the menu at the bottom of the rail.

## 10. Provider terms: rules the code must follow

1. **Official CLIs only.** Run the unmodified official CLIs, and never use the Claude Agent SDK. Anthropic allows "an end user … signing in to the unmodified Claude Code binary with their own Claude subscription". It does not allow third-party products built on the Agent SDK to offer Claude subscription login.
2. **Sign-in only through the CLI's own flow** (`claude auth login`, `codex login`). Never read, copy, store, send or proxy CLI credential files or tokens. That also rules out Claude's unofficial usage endpoint, which needs the token.
3. **No automatic switching between subscription accounts.** A bot never moves from one subscription account to another without your tap. Automatic fallback goes only to API-key accounts. Anthropic's limits assume "ordinary, individual usage" and its policy bans coordinating accounts to "circumvent product guardrails". OpenAI bans circumventing "any rate limits".
4. **Clean environments.** Strip API-key variables from each CLI's environment unless the bot is on an API-key account.
5. **No Antigravity for now.** Google's terms call using a third-party tool with an Antigravity login a breach, and `agy` can't run two Google logins at once.

## 11. Data model (SQLite)

| Table | Main columns |
|---|---|
| `bots` | id, name, color, shape, instructions, provider, model, account_id, home_folder, project_folder, autonomy, browser, paused |
| `bot_links` | bot_id, target_bot_id ("Can message") |
| `allow_rules` | bot_id, rule |
| `conversations` | id, kind (bot, room), bot_id |
| `messages` | id, conversation_id, author (you, bot id, system), kind (text, activity, handoff, routine, approval, file, notice, error), body, exchange_id, routine_run_id, created_at |
| `work_items` | id, bot_id, source, text, files, chain_id, exchange_id, routine_run_id, priority, status, created_at, started_at, ended_at |
| `chains` | id, origin (message or routine run), requests_made |
| `exchanges` | id, from_bot_id, to_bot_id, request, reply, status, chain_id |
| `sessions` | id, bot_id, provider, account_id, cli_session_id, started_at, last_active_at, ended_at |
| `approvals` | id, bot_id, session_id, kind (tool, routine, account switch), summary, details, status |
| `routines` | id, bot_id, name, prompt, days, times, time_zone, time_limit_minutes, enabled |
| `routine_runs` | id, routine_id, due_at, started_at, ended_at, status (last 20 kept per routine) |
| `usage` | id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source (reported, price list), created_at |
| `accounts` (Phase 2) | id, provider, label, kind (subscription, api_key), folder, credential_ref, is_fallback, spending_cap_usd, limited_until |
| `devices` (Phase 3) | id, name, public_key, push_token, paired_at, last_seen_at, revoked_at |
| `pairing_codes` (Phase 3) | id, secret_hash, expires_at, used_at |
| `settings` | key, value |

## 12. Phases

Each phase ends with a demo you approve before the next one starts. The spec covers every phase at this level of detail. Each phase gets its own implementation plan when we reach it; the first plan covers Phase 1 only.

### 12.1 Screens by phase

| Phase | Screens |
|---|---|
| 1 | WF1 main chat (dark and light), WF2 Agents overview, WF3 New bot and edit bot, Team room, exchange view, routine editor, first-run setup, Settings (General, Bots) |
| 2 | Accounts, sign-in terminal, Usage; the bot's "Account and model" row |
| 2.5 | Phone layouts of everything above |
| 3 | Devices, Pair a device (QR and code), Connect to another computer, host switcher |
| 4 | Full WF1 right panel (live screen, Take over, Pause, Stop, Chrome profile, Logins), expanded computer view, WF4 Logins, Chrome profiles |

In Phase 1:
- **WF1 right panel** shows the bot summary (provider, model, autonomy, folder), a Browser row with "Show window", Routines and Recent activity.
- **WF2 cards** show the avatar placeholder instead of a live thumbnail.
- **Hidden until later:** the store icon, the share icon, the mic and voice buttons, and the "This PC / Virtual machine" switch.

### 12.2 Phase by phase

| Phase | Work | Demo |
|---|---|---|
| 1 Core app | Spikes (12.3). Design the missing screens in OpenPencil (Team room, exchange view, routine editor, bot settings, first-run setup, Phase 1 right panel). Build the daemon, desktop app, UI, both adapters, approvals, autonomy, bot to bot, Team room, routines, browser, desktop notifications, usage recording, Windows installer | Create a Claude bot and a Codex bot, one of them from a Suggestion. Chat with both; steer a busy bot; Stop; approve with Always allow. The Claude bot asks the Codex bot for something; rows in both chats; full exchange in the Team room; join from the exchange view. A bot uses its own Chrome window, and a site you signed in to by hand stays signed in. A routine runs on schedule with the window closed. A bot proposes a routine from chat and you approve it. The installer installs. Screenshots match WF1 to WF3, apart from the parts that arrive in later phases |
| 2 Accounts | Design Accounts, sign-in terminal and Usage. Build multi-account (subscription folders, API keys), pinning, limit cards, API-key fallback, spending caps, usage and cost | Sign in two Claude accounts and one Codex account in the app. Add an API key with a tiny cap and watch the cap stop it. Pin bots. The Usage page shows tokens and cost by bot, account and model. Limit handling is shown live if a real limit happens, otherwise proven by tests on captured real limit output |
| 2.5 Phone UI | Phone designs in OpenPencil, then the narrow layout | Every screen at phone width matches its design |
| 3 Remote | Start at login, tray, pairing, devices and revoke, relay (public and self-host guide), same-Wi-Fi connections, Noise encryption, Android app, push, host switcher, desktop auto-update | The app starts at login. Pair an Android phone by QR over mobile data. Chat, approve and create a bot from the phone. A push arrives when a bot waits. Pair a second PC with the code. Revoke the phone and it's cut off at once |
| 4 Computer | Live screen of the bot's Chrome, Take over, Pause, Stop, Logins vault, live Agents thumbnails | Watch a bot's Chrome live; take over to type a PIN; Pause and Stop. The vault fills a login, and a test proves the password never reaches the model |

### 12.3 Phase 1 spikes (run first)

1. **Steering Claude.** Does Claude read a message written to stdin in the middle of a turn? If not, Claude bots use "first in line" for your messages.
2. **Stopping Claude.** Does the stream-json interrupt message stop a turn cleanly? If not, end the process and resume.
3. **Claude approvals.** Does `--permission-prompt-tool` reach our MCP server on Windows, and can the call stay open for hours? (This may need a raised `MCP_TOOL_TIMEOUT`.)
4. **Codex sandbox on Windows.** Does the `workspace-write` sandbox work with network on? If not, Codex's "Own folder" asks before every command, like Claude's.
5. **Browser.** Does Playwright MCP work from both CLIs with a per-bot profile? Should Playwright MCP start Chrome, or should the daemon start it with a debugging port?
6. **SQLite.** Is `node:sqlite` available in the Node the daemon ships with (Electron's, run as Node, or a bundled Node)?
7. **Codex executable.** Resolve the real `codex` executable behind npm's shim on Windows.

## 13. When things go wrong

| Situation | What happens |
|---|---|
| CLI missing or signed out | The bot shows "Needs setup" with exactly what to do |
| CLI crashes mid-turn | The turn is marked failed with Retry. The next piece of work resumes the session; if resuming fails, a fresh session starts with the recap |
| App or PC restarts | The queue survives in the database. Turns that were running show "Interrupted" with Retry. Routines don't fire twice |
| A message is sent | It's saved before it goes to a CLI, so it's never lost |
| A bot-to-bot request fails | The tool returns a plain reason; the bot tells you |
| Usage limit | Section 8.2 (Phase 1: "Limited until <time>", then it resumes by itself) |
| An approval goes unanswered | It waits, with a notification |
| Relay unreachable (Phase 3) | Same-Wi-Fi connections still work; remote devices show "Can't reach your PC" |

## 14. Testing

- **Real CLIs.** Flow tests run against the real `claude` and `codex` with your accounts. No fake CLI.
- **Parser tests.** Each adapter's parsing is tested on output captured from real runs. That keeps the tests fast and lets them run many times while developing.
- **Unit tests** for logic that can break without anyone noticing:
  - queue and steering rules;
  - the chain limit;
  - routine timing (next run, missed runs, time limits);
  - cost calculation;
  - pairing codes (single use, expiry);
  - encryption (tampered and replayed messages rejected);
  - revoked devices.
- **Screenshot checks** of built screens against the wireframes and phone designs.
- **A manual end-to-end run per phase:** the demo in section 12.2.
- **Tools:** vitest for unit and flow tests, and Playwright for UI screenshots.

## 15. Risks

| Risk | Mitigation |
|---|---|
| Provider terms change or enforcement tightens | Section 10 rules. Official CLIs only, no token handling, and API-key accounts as a supported path |
| CLI output formats change (both CLIs update weekly) | Adapters isolate the change. Parser tests on captured output, and real-CLI runs before each release |
| Full auto bots can do anything the user can | Own folder is the default; Full auto shows a warning |
| Claude has no OS sandbox on Windows | "Own folder" asks before every shell command for Claude bots |
| Bot-to-bot chains multiply usage | Chain limit, spending caps, usage page |
| Many bots mean many processes and Chrome windows | One turn at a time per bot; idle shutdown after 10 minutes |
| Bots inherit the user's CLI settings, plugins, hooks and MCP servers | Accepted for Phase 1; revisit with the Tools picker |
| Public relay costs | Cloudflare Workers is cheap, the relay only forwards bytes, and a self-host guide exists |
| Windows Firewall prompt for same-Wi-Fi connections | Explained on the pairing screen; the relay still works if it's refused |
| The name | Run a trademark search for "Tjommie" before launch |

## 16. Later

- iOS app (needs a Mac), and a macOS desktop build.
- Antigravity (`agy`), once it supports separate accounts, or with API keys only.
- A Tools picker per bot: attach MCP servers such as Gmail or GitHub, like Grok Bot's connectors.
- Routines triggered by events.
- Shared Skills.
- Several rooms, or group chats with chosen bots.
- A virtual machine target for computer control.
- Opening Tjommie in a plain web browser.
- Voice input, the Share button, and a bot template gallery (the store icon).

## 17. References

### Paseo (reference implementation)

The local fork `F:\Work\kuumba-paseo` of `getpaseo/paseo`, Apache-2.0. Keep its license notice and credit in `NOTICE` for anything copied.

| For | File |
|---|---|
| Shared event model | `packages/server/src/server/agent/agent-sdk-types.ts` |
| Claude output parsing, approvals, resume | `packages/server/src/server/agent/providers/claude/agent.ts` |
| Codex app-server client | `packages/server/src/server/agent/providers/codex-app-server-agent.ts` |
| Tool calls as readable lines | `packages/server/src/server/agent/providers/claude/tool-call-mapper.ts`, `.../providers/codex/tool-call-mapper.ts` |
| The app's MCP server per session | `packages/server/src/server/agent/runtime-mcp-config.ts`, `.../agent/tools/paseo-tools.ts` |
| Agent-to-agent prompts and finish notices | `packages/server/src/server/agent/agent-prompt.ts` |
| Environment per account | `packages/server/src/server/agent/provider-launch-config.ts` |
| Codex usage | `packages/server/src/services/quota-fetcher/` (Codex parts only; not the Claude token-reading part) |
| Host and Origin checks | `packages/server/src/server/websocket-server.ts` |
| Hello and protocol versioning | `packages/protocol/src/messages.ts` |
| Pairing payload, QR flow, host list | `packages/protocol/src/connection-offer.ts`, `packages/app/src/app/pair-scan.tsx`, `packages/app/src/runtime/host-runtime.ts` |
| Relay encryption (reference only; we use Noise) | `packages/relay/src/encrypted-channel.ts`, `packages/relay/src/crypto.ts` |

### Research sources (checked 2026-09-25)

- **Grok Bot:**
  - https://x.ai/news/introducing-grok-bot
  - https://x.ai/news/designing-grok-bot
  - https://docs.x.ai/grok-bot/overview
  - https://docs.x.ai/grok-bot/computer-and-apps
  - https://docs.x.ai/grok-bot/chat-and-collaboration
  - https://docs.x.ai/grok-bot/skills-routines-and-automations
- **Anthropic:**
  - https://code.claude.com/docs/en/legal-and-compliance
  - https://code.claude.com/docs/en/agent-sdk/overview
  - https://code.claude.com/docs/en/authentication
  - https://code.claude.com/docs/en/headless
- **OpenAI Codex:**
  - https://learn.chatgpt.com/docs/auth
  - https://learn.chatgpt.com/docs/app-server.md
- **Google Antigravity:**
  - https://antigravity.google/terms
  - https://github.com/google-antigravity/antigravity-cli
- **Verified on this PC:**
  - `claude` 2.1.280 headless flags;
  - `codex` 0.155.1 app-server schema: `turn/steer`, `turn/interrupt`, `thread/resume`, `developerInstructions`, `mcpServers`, `thread/tokenUsage/updated`, `account/rateLimits/updated`, `UsageLimitExceeded`;
  - `claude auth login`, `codex login --device-auth` and `--with-api-key`;
  - `agy` stream-json print mode, and its single Google login.
