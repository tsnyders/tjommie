# Tjommie Phase 1D: Desktop Shell and Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tjommie opens as a Windows desktop app. It starts its own daemon with a token that never touches the disk, lists the bots in the rail, and shows a bot's chat with every message and card state from WF1 and WF11. It also shows the bot panel, keeps running in the tray, and sends desktop notifications.

**Architecture:**
- **Electron main process** (`packages/desktop`):
  - Starts the daemon as a child process with `--token-stdin`: Electron's own binary, run as Node.
  - Serves the UI's static export from `app://tjommie`.
  - Owns the window, the tray and the notifications.
  - Hands the token to the UI through a sandboxed preload bridge.
- **UI** (`packages/ui`, Next.js static export):
  - A pure client of the daemon.
  - It uses the typed client that Phase 1 already built, moved into its own package, `packages/client`.
  - It keeps one app state that daemon events update.
- **Daemon:** gets two small changes. It stops when the desktop app closes its stdin, and it has a test hook for real account folders.

**Tech Stack:**
- Electron 44.4.5, Next.js 16.3.6 (static export), React 19.2, Tailwind CSS 4, Base UI 1.8 (`@base-ui/react`), lucide-react.
- react-markdown 10 with remark-gfm 4.
- vitest 5, and playwright-core (Electron driver) for the real end-to-end tests.

**Spec:** `docs/superpowers/specs/2026-09-25-tjommie-design.md`: §3 (architecture, local security), §4.4 and §4.5 (where things show, states), §5.5 (approval cards), §5.7 (browser), §7 (notifications), §12.1 (Phase 1 screens). The screens are in `docs/design/wireframe-spec.md` "Phase 1 screens (plan 1C)" and `docs/wireframes/`.

## Where this plan sits

| Plan | Delivers | Status |
|---|---|---|
| 1A Daemon core | Bots, chat, both CLIs, steering, Stop, approvals, sign-in | Built, on this branch |
| 1B Bots working together | MCP server, exchanges, Team room, routines, browser | Built, on this branch |
| 1C Screen designs | Every Phase 1 frame, in `docs/wireframes/` | Done |
| **1D Desktop shell and chat (this plan)** | Electron shell, daemon link, rail, the chat with every message and card state, the bot panel, tray and notifications | |
| 1E Other screens, installer and demo | New bot and Bot settings, Team room, exchange view, routine editor, first run, Settings, Agents overview, installer, screenshot checks against the wireframes (taken from the running app with real bots), the Phase 1 demo | Written after this plan |

This plan builds on branch `implement-phase-1-multi-agent` at `40415be` or later. All paths are relative to that branch's root.

## Facts this plan relies on

Every item was checked on 2026-09-27 against this branch and its installed packages.

**The daemon, from the final review of Phase 1** (`.superpowers/sdd/2026-09-26-tjommie-phase1ab-final-review/`: token-pipe-report.md, fix-B-report.md, codex-contain-report.md, spec-drift-for-1d.md)
- **Starting it:**
  - Production start is `main.ts --token-stdin` with `TJOMMIE_DEV` unset. The first stdin line is a token of at least 43 base64url characters, which must arrive within 5 seconds.
  - When ready, the daemon prints `Tjommie daemon listening on http://127.0.0.1:<port>`, and it honors `TJOMMIE_PORT=0`.
  - Production mode allows only the `app://tjommie` origin. Development mode (`--dev`, or `TJOMMIE_DEV=1`) writes `daemon-token` and also allows `http://localhost:3000`.
- **Failure messages:** exit 1 with plain stderr lines, for example `Tjommie is already running: port <port> is in use.`
- **Today the daemon stays up when stdin closes.** Task 1 changes that.
- **The typed client:**
  - It's `packages/daemon/src/client.ts` and uses only the global `WebSocket`.
  - `connect(port, token)` returns `{ call, events, subscribe, disconnected, close }`. Errors are `ClientRpcError` (`code`) and `ClientConnectionError` (`code`, where 4003 means a protocol mismatch).
  - Its `events` array keeps every event for the connection's life (a deferred minor).
- **History:** `chat.history` and `room.history` return `{ messages, hasMore }`. The next cursor is the first message's id.
- **Message payloads:** read them with `messageData(message)` from `@tjommie/protocol`.
- **Approvals:**
  - There's no `approvals.list`. Pending cards are the approval messages whose `data.status` is `"pending"`.
  - Claude's approval `detail` is the tool input as pretty-printed JSON. Codex's is the command, or the folder.
- **Text streams** as `message.delta` events. The daemon saves the text only when the message ends, so a history page taken mid-reply holds an empty body.
- **`notify` events** always carry `botId`, `title` and `body`.
- **Setup:** `setup.status` re-runs both CLIs, so it's called once per connection and never polled.
- **Bot fields and setup status:**
  - `Bot.setupReason` explains `needs_setup`.
  - Codex's `ProviderSetup.sandbox` is `ready | notConfigured | updateRequired | unavailable`.
  - `setup.prepareCodexSandbox` sets up that sandbox.

**Next.js 16.3.6** (read in `node_modules/next/dist/docs/`, as `packages/ui/AGENTS.md` asks)
- **`output: "export"` with `trailingSlash: true`** writes each page to `<route>/index.html`.
- **Search params:**
  - `useSearchParams` works in a static export only inside a `<Suspense>` boundary; without one, the build fails.
  - The part that uses it renders on the client only.
- **Workspace packages** that export TypeScript source need `transpilePackages`.

**Electron 44.4.5** (`node_modules/electron/electron.d.ts`)
- **APIs used, all present:** `protocol.registerSchemesAsPrivileged`, `protocol.handle`, `net.fetch`, `BrowserWindow` `titleBarOverlay` and `setTitleBarOverlay`, `webUtils.getPathForFile`, `app.requestSingleInstanceLock`, `app.setAppUserModelId`, `shell.showItemInFolder`, `webContents.setWindowOpenHandler`.
- **Preload format:** a sandboxed preload can't be an ES module or TypeScript. It's a small CommonJS file.
- **Type stripping:** Electron's main process runs `.ts` files through Node's type stripping, which is how the scaffold's `src/main.ts` already runs. Node refuses type stripping under `node_modules`, so the packaged app (plan 1E) bundles the daemon and the main process instead.
- **The daemon under Electron's Node:** checked with Electron 44.4.5 run as Node (`ELECTRON_RUN_AS_NODE=1`). It's Node 24.21.0, and `node:sqlite` works (SQLite 3.53.4).
- **The binary in this worktree:** `node_modules/electron/dist` is missing here, because the binary download didn't run. Before Task 4, run `node node_modules/electron/install.js` once from the repo root. Expected: `node_modules/electron/dist/electron.exe` exists.

**Playwright:** `playwright-core` exports `_electron`, which launches and drives the Electron app. No browser download is needed.

## Global Constraints

- **Plan 1A's constraints still apply.** That covers erasable TypeScript in the Node packages, npm and npx from the repo root in PowerShell only, the provider rules, and visible text without em dashes, en dashes or emoji. So do the commit style and trailer, and the rule against pushing.
- **Dependencies:** add exactly these, nothing else:
  - `packages/ui`: `react-markdown@^10.1.0`, `remark-gfm@^4.0.1` and the workspace package `@tjommie/client`.
  - `packages/desktop`: `@tjommie/client`.
  - `packages/daemon`: `@tjommie/client`.
  - Root dev dependency: `playwright-core@^1.63.0`.

  No state library, no CSS-in-JS, no shadcn CLI runs. Base UI primitives (`@base-ui/react`, already installed) cover the menu.
- **Where the token goes:**
  - Electron's main process makes a fresh token per daemon start.
  - It's written only to the daemon's stdin, and held only in memory by the main process and the UI.
  - It never goes into process arguments, environment variables, files, logs or URLs.
- **Renderer security:**
  - The window runs with `contextIsolation: true`, `sandbox: true` and `nodeIntegration: false`, and loads only `app://tjommie/` (production) or `http://localhost:3000/` (development).
  - Any other navigation is blocked. `http`/`https` links open in the default browser.
  - Production responses carry a Content-Security-Policy that allows WebSocket connections only to `ws://127.0.0.1:*`.
- **Two desktop modes:**
  - **Development** (`npm run desktop`): the development daemon and `npm run dev` run separately, and Electron reads the development token file.
  - **Production** (`TJOMMIE_DESKTOP=production`): Electron starts the daemon from `packages/daemon/src/main.ts` and serves `packages/ui/out`.
  - Packaged builds (paths under `resources`, the installer, the app id) are plan 1E.
- **Lifetimes:**
  - Closing the window hides it; Tjommie keeps running in the tray.
  - Quitting from the tray stops the daemon, and with it the bots and routines.
  - The daemon stops when its stdin closes (Task 1), so a crashed desktop app never leaves a daemon holding the port.
- **Look:**
  - Colors come only from the token table in `docs/design/wireframe-spec.md` and its approved exceptions. The light theme maps them with `design/openpencil/recolor-light.js`. Dark is the default.
  - The font is Segoe UI (spec §3), using the wireframe spec's sizes and weights: body 14/21, helper text 12/18.
  - Icons are lucide-react by name. Radius: bubbles 18, cards 12, inputs and rows 10, popovers 16, pills fully round.
- **Motion:**
  - Nothing animates on keyboard actions or actions repeated all day.
  - Transitions are 150 to 250 ms, with `cubic-bezier(0.23, 1, 0.32, 1)`.
  - Pressed buttons scale to 0.97.
  - `prefers-reduced-motion` turns motion off.
- **Copy:** app-formatted times use "9:14 AM" (en-US). Day labels are "Today", "Yesterday", a weekday within the last week, otherwise "21 September". An unknown bot id reads "A deleted bot".
- **1E's pages:** plan 1E adds the pages these link to. Until then they show Next's not-found page.
  - The rail links to `/room/`, `/agents/`, `/bots/new/` and `/settings/`.
  - The bot panel and the chat's More menu link to `/bots/settings/?bot=<id>`.
- **Tests:**
  - `npm test` runs the fast tests, including the UI's `lib` and `components` tests.
  - `npm run test:real` also runs `packages/desktop/src/desktop.real.test.ts`. It builds the UI first, and needs the one-time sign-in from plan 1A (`npm run signin -w @tjommie/daemon`).
  - `npm run typecheck` and `npm run lint` must pass after every task that touches their files.

## Review Focus

These are the five things most likely to go wrong for a person using the desktop app, most likely first. Each has a test in the task that owns the code.

1. **A daemon left running after the app quits or crashes.** Nobody else knows its token, and it holds port 47100, so the next start fails. It must stop with the app. *Tests: Task 1, "stops when the desktop app closes its stdin"; Task 13, "quitting Tjommie stops its daemon".*
2. **The daemon dies while the window is open.** The app must start a new one, and the UI must reconnect without the person restarting anything. *Test: Task 13, "reconnects after the daemon dies".*
3. **Opening a chat while the bot is mid-reply.** Text that streamed while the history loaded must be neither lost nor doubled. *Test: Task 6, "opened mid-stream".*
4. **Links and HTML in a bot's reply.** Raw HTML must never render, `javascript:` links must be dropped, and a web link must open the default browser instead of replacing the app window. *Tests: Task 8, "markdown safety"; Task 3, "navigation stays in the app".*
5. **The token leaking.** It must never appear in the daemon's output, arguments, environment or logs. *Test: Task 3, "token stays in memory".*

---

### Task 1: The daemon follows the desktop app, with a test hook for account folders

**Files:**
- Modify: `packages/daemon/src/main.ts` (replace the file), `packages/daemon/src/main.test.ts`, `README.md`

**Interfaces:**
- Consumes: `startDaemon({ port, auth, accountsDir })` from `packages/daemon/src/daemon.ts` (unchanged).
- Produces:
  - In production mode (`--token-stdin`) the daemon keeps reading stdin after the token line. When stdin ends, it closes (bots stop cleanly) and exits 0.
  - `TJOMMIE_ACCOUNTS_DIR`, when set, becomes `startDaemon`'s `accountsDir`. It's a test hook: the desktop app's real-CLI tests pair a disposable data root with the signed-in account folders.
  - Development mode is unchanged.

- [ ] **Step 1: Write the failing tests**

In `packages/daemon/src/main.test.ts`:

1. Give `launch` an `env` option. Replace its signature and `env:` line with:

```ts
function launch(args: string[], options: { root?: string; dev?: string; env?: Record<string, string> } = {}): Fixture {
```

```ts
    env: { ...inheritedEnv, PATH: "", TJOMMIE_HOME: root, TJOMMIE_PORT: "0", TJOMMIE_DEV: options.dev ?? "", ...options.env },
```

2. Replace the first test, "authenticates a piped production token, removes a stale file (%s), and stays up after stdin closes", with:

```ts
it.each([false, true])("authenticates a piped production token and removes a stale file (%s)", async (stale) => {
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "tjommie-token-pipe-"));
  const tokenFile = join(root, "daemon-token");
  if (stale) writeFileSync(tokenFile, randomBytes(32).toString("base64url"));
  const token = randomBytes(32).toString("base64url");
  const fixture = launch(["--token-stdin"], { root });
  fixture.child.stdin.write(`${token}\nignored second line\n`);
  const port = await ready(fixture);
  expect(await originAllowed(port, "http://localhost:3000")).toBe(false);
  expect(existsSync(tokenFile)).toBe(false);
  if (stale) expect(fixture.stdout() + fixture.stderr()).toContain("Removed a leftover development daemon-token file.");
  const client = await connect(port, token);
  try {
    expect(await client.call("bots.list", {})).toEqual([]);
    await expect(connect(port, randomBytes(32).toString("base64url"))).rejects.toMatchObject({ code: 4401 });
  } finally {
    await client.close();
  }
  expect(fixture.child.exitCode).toBeNull();
  expect(existsSync(tokenFile)).toBe(false);
}, 30_000);

it("stops when the desktop app closes its stdin", async () => {
  const token = randomBytes(32).toString("base64url");
  const fixture = launch(["--token-stdin"]);
  fixture.child.stdin.write(`${token}\n`);
  const port = await ready(fixture);
  const client = await connect(port, token);
  fixture.child.stdin.end();
  expect(await exited(fixture)).toBe(0);
  await client.disconnected;
}, 30_000);

it("keeps the account folders where TJOMMIE_ACCOUNTS_DIR points", async () => {
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "tjommie-token-pipe-"));
  const accounts = join(root, "signed-in-accounts");
  const token = randomBytes(32).toString("base64url");
  const fixture = launch(["--token-stdin"], { root, env: { TJOMMIE_ACCOUNTS_DIR: accounts } });
  fixture.child.stdin.write(`${token}\n`);
  await ready(fixture);
  expect(existsSync(join(accounts, "claude-main"))).toBe(true);
  expect(existsSync(join(accounts, "codex-main"))).toBe(true);
}, 30_000);
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/daemon/src/main.test.ts`
Expected: FAIL.
- "stops when the desktop app closes its stdin" times out, because today's daemon stays up.
- "keeps the account folders where TJOMMIE_ACCOUNTS_DIR points" fails, because the folders are created under the data root instead.

- [ ] **Step 3: Replace main.ts**

Replace `packages/daemon/src/main.ts` with:

```ts
import { createInterface } from "node:readline";
import { PlainStartupError, startDaemon } from "./daemon.ts";
import { DEFAULT_PORT } from "./paths.ts";

const noModeMessage = "Start Tjommie from the desktop app, or run npm start -w @tjommie/daemon for development.";

function tokenFromStdin(): Promise<string> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
    let done = false;
    const finish = (line?: string, missingMessage = "No daemon token arrived on stdin within 5 seconds."): void => {
      if (done) return;
      done = true;
      clearTimeout(timeout);
      lines.close();
      process.stdin.pause();
      if (line === undefined) reject(new PlainStartupError(missingMessage));
      else resolve(line);
    };
    const timeout = setTimeout(() => finish(), 5_000);
    lines.once("line", finish);
    lines.once("close", () => finish(undefined, "stdin closed before a daemon token arrived."));
  });
}

const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
try {
  const args = process.argv.slice(2);
  const development = process.env.TJOMMIE_DEV === "1" || args.includes("--dev");
  const production = args.includes("--token-stdin");
  const unknown = args.find((arg) => arg !== "--dev" && arg !== "--token-stdin");
  if (unknown !== undefined) throw new PlainStartupError(`Unknown argument: ${unknown}`);
  if (development === production) throw new PlainStartupError(development ? "Choose either --dev or --token-stdin." : noModeMessage);
  const auth = development
    ? { mode: "development" as const }
    : { mode: "production" as const, token: await tokenFromStdin() };
  // Test hook: the desktop app's real-CLI tests pair a disposable data root with the signed-in account folders.
  const accountsDir = process.env.TJOMMIE_ACCOUNTS_DIR || undefined;
  const daemon = await startDaemon({ port, auth, accountsDir });
  console.log(`Tjommie daemon listening on http://127.0.0.1:${daemon.port}`);
  let stopping = false;
  const stop = (): void => {
    if (stopping) return;
    stopping = true;
    void daemon
      .close()
      .catch((error: unknown) => console.error("Failed to stop the Tjommie daemon cleanly.", error))
      .finally(() => process.exit(0));
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  if (production) {
    // The desktop app keeps the other end of stdin open for as long as it runs. When it quits or crashes the
    // pipe closes and the daemon stops too, so no daemon is left holding the port with a token nobody has.
    process.stdin.once("end", stop);
    process.stdin.resume();
    if (process.stdin.readableEnded) stop(); // it closed while the daemon was starting
  }
} catch (error) {
  if (error && typeof error === "object" && "code" in error && error.code === "EADDRINUSE") {
    console.error(`Tjommie is already running: port ${port} is in use.`);
    process.exit(1);
  }
  if (error instanceof PlainStartupError) console.error(error.message);
  else console.error(error);
  process.exit(1);
}
```

- [ ] **Step 4: Update the README**

In `README.md`, replace the paragraph that starts "The installed desktop app starts the daemon with `--token-stdin`." with:

```markdown
The installed desktop app starts the daemon with `--token-stdin`. It must start the daemon with `TJOMMIE_DEV` unset. It passes a fresh token as the first line of a pipe to the daemon's standard input and keeps that pipe open. The daemon requires at least 43 base64url characters within 5 seconds, never saves this token to a file, deletes an old `daemon-token` file left by development mode, and stops when the pipe closes, so quitting or crashing the desktop app never leaves a daemon running. The terminal chat only works with a development daemon. Starting the daemon with neither mode exits with instructions to use the desktop app or the development command. Tests can set `TJOMMIE_ACCOUNTS_DIR` to use signed-in account folders with a disposable data root.
```

- [ ] **Step 5: Run the tests**

Run: `npx vitest run packages/daemon/src/main.test.ts`
Expected: PASS, all tests in the file.

Run: `npm test` and `npm run typecheck`
Expected: PASS, no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/main.ts packages/daemon/src/main.test.ts README.md
git commit -m "Stop the production daemon when the desktop app closes its stdin

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The shared client package

**Files:**
- Create: `packages/client/package.json`, `packages/client/src/index.ts`, `packages/client/src/client.ts`
- Delete: `packages/daemon/src/client.ts`
- Modify: `packages/daemon/package.json`, `tsconfig.json` (root), `README.md`, and the imports in `packages/daemon/src/client.test.ts`, `packages/daemon/src/daemon.test.ts`, `packages/daemon/src/daemon.real.test.ts`, `packages/daemon/src/main.test.ts`, `packages/daemon/src/terminal-chat.test.ts` and `packages/daemon/scripts/chat.ts`
- Test: `packages/daemon/src/client.test.ts` (add a test). It stays in the daemon package, because it runs the client against the daemon's own server.

**Interfaces:**
- Consumes: `packages/daemon/src/client.ts` as it is today.
- Produces:
  - `@tjommie/client` exports what `client.ts` exported: `connect`, `Client`, `ClientEvent`, `ClientRpcError` and `ClientConnectionError`.
  - `connect(port, token, options?)` takes `options.recordEvents` (default `true`). With `false`, `client.events` stays empty but subscribers still get every event. The desktop app and the UI use that.

- [ ] **Step 1: Write the failing test**

Add this test inside the `describe("typed daemon client", ...)` block in `packages/daemon/src/client.test.ts`:

```ts
  it("can leave events unrecorded for a long-lived client and still notify subscribers", async () => {
    const port = await peer((socket, message) => {
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
      if (message.type === "req") {
        socket.send(JSON.stringify({ type: "event", event: "settings.changed", data: { chainLimit: 4 } }));
        socket.send(JSON.stringify({ type: "res", id: message.id, ok: true, result: { chainLimit: 4 } }));
      }
    });
    const client = await connect(port, "test-token", { recordEvents: false });
    clients.push(client);
    const seen: string[] = [];
    client.subscribe((event) => seen.push(event.event));
    expect(await client.call("settings.get", {})).toEqual({ chainLimit: 4 });
    expect(seen).toEqual(["settings.changed"]);
    expect(client.events).toEqual([]);
  });
```

Then change that file's import of the client to:

```ts
import { ClientRpcError, connect, type Client } from "@tjommie/client";
```

- [ ] **Step 2: Run it to see it fail**

Run: `npx vitest run packages/daemon/src/client.test.ts`
Expected: FAIL. `@tjommie/client` can't be resolved.

- [ ] **Step 3: Create the package**

Create `packages/client/package.json`:

```json
{
  "name": "@tjommie/client",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "exports": "./src/index.ts",
  "dependencies": {
    "@tjommie/protocol": "*"
  }
}
```

Create `packages/client/src/index.ts`:

```ts
export * from "./client.ts";
```

Create `packages/client/src/client.ts`:

```ts
import { ErrorCode, PROTOCOL_VERSION, events, methods, type EventData, type EventName, type MethodName, type ParamsIn, type Result } from "@tjommie/protocol";

export type ClientEvent = { [E in EventName]: { type: "event"; event: E; data: EventData<E> } }[EventName];
export type Client = {
  call: <M extends MethodName>(method: M, params: ParamsIn<M>) => Promise<Result<M>>;
  // Every event since connecting, unless the client was opened with recordEvents: false.
  events: ClientEvent[];
  subscribe: (listener: (event: ClientEvent) => void) => () => void;
  disconnected: Promise<ClientConnectionError>;
  close: () => Promise<void>;
};

export type ConnectOptions = {
  // Tests and the terminal chat read `events`. Long-lived clients (the desktop app and the UI) turn this
  // off so the list doesn't grow for the life of the connection; subscribers still get every event.
  recordEvents?: boolean;
};

type PendingCall = { accept: (result: unknown) => void; reject: (error: Error) => void };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export class ClientRpcError extends Error {
  readonly code: ErrorCode;

  constructor(code: ErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "ClientRpcError";
    this.code = code;
  }
}

export class ClientConnectionError extends Error {
  readonly code: number;
  readonly reason: string;
  readonly opened: boolean;

  constructor(code: number, reason: string, opened: boolean) {
    super(`WebSocket closed (code ${code}, reason: ${reason || "none"})`);
    this.name = "ClientConnectionError";
    this.code = code;
    this.reason = reason;
    this.opened = opened;
  }
}

// Uses the global WebSocket, so the same client runs in Node 24, in Electron's main process and in the UI.
export async function connect(port: number, token: string, options: ConnectOptions = {}): Promise<Client> {
  const recordEvents = options.recordEvents ?? true;
  const socket = new WebSocket(`ws://127.0.0.1:${port}`);
  const received: ClientEvent[] = [];
  const subscribers = new Set<(event: ClientEvent) => void>();
  const pending = new Map<number, PendingCall>();
  let nextId = 1;
  let opened = false;
  let welcomed = false;
  let failure: ClientConnectionError | undefined;
  let resolveWelcome: () => void = () => {};
  let rejectWelcome: (error: Error) => void = () => {};
  const welcome = new Promise<void>((resolve, reject) => {
    resolveWelcome = resolve;
    rejectWelcome = reject;
  });
  let resolveClosed: () => void = () => {};
  const closed = new Promise<void>((resolve) => { resolveClosed = resolve; });
  let resolveDisconnected: (error: ClientConnectionError) => void = () => {};
  const disconnected = new Promise<ClientConnectionError>((resolve) => { resolveDisconnected = resolve; });

  function connectionError(code: number, reason: string): ClientConnectionError {
    return new ClientConnectionError(code, reason, opened);
  }

  function fail(error: ClientConnectionError): void {
    if (failure) return;
    failure = error;
    clearTimeout(helloTimer);
    resolveDisconnected(error);
    rejectWelcome(error);
    for (const call of pending.values()) call.reject(error);
    pending.clear();
  }

  function invalidFrame(reason: string): void {
    fail(connectionError(4400, `Invalid server frame: ${reason}`));
    socket.close(4400, "Invalid server frame");
  }

  const helloTimer = setTimeout(() => {
    fail(connectionError(4408, "Welcome timed out"));
    socket.close();
  }, 5000);

  socket.addEventListener("open", () => {
    opened = true;
    if (failure) return;
    try {
      socket.send(JSON.stringify({ type: "hello", protocol: PROTOCOL_VERSION, token }));
    } catch {
      fail(connectionError(1006, "WebSocket error sending hello"));
      socket.close();
    }
  });
  socket.addEventListener("close", (event) => {
    fail(connectionError(event.code, event.reason));
    resolveClosed();
  });
  socket.addEventListener("error", () => {
    fail(connectionError(1006, "WebSocket error"));
  });
  socket.addEventListener("message", (event) => {
    if (failure) return;
    let frame: unknown;
    try {
      if (typeof event.data !== "string") throw new Error("Expected a text frame");
      frame = JSON.parse(event.data);
      if (!isRecord(frame)) throw new Error("Expected an envelope");
      if (frame.type === "welcome") {
        if (welcomed || frame.protocol !== PROTOCOL_VERSION) throw new Error("Unexpected welcome protocol");
        welcomed = true;
        clearTimeout(helloTimer);
        resolveWelcome();
        return;
      }
      if (!welcomed) throw new Error("Expected welcome first");
      if (frame.type === "res") {
        if (typeof frame.id !== "number" || !Number.isSafeInteger(frame.id) || typeof frame.ok !== "boolean") throw new Error("Invalid response envelope");
        const call = pending.get(frame.id);
        if (frame.ok) {
          if (!("result" in frame)) throw new Error("Missing response result");
          call?.accept(frame.result);
        } else {
          if (!isRecord(frame.error) || typeof frame.error.code !== "string" || typeof frame.error.message !== "string") throw new Error("Invalid response error");
          call?.reject(new ClientRpcError(ErrorCode.parse(frame.error.code), frame.error.message));
        }
        pending.delete(frame.id);
        return;
      }
      if (frame.type === "event") {
        if (typeof frame.event !== "string" || !Object.hasOwn(events, frame.event) || !("data" in frame)) throw new Error("Invalid event envelope");
        const name = frame.event as EventName;
        // The name selects the matching schema, preserving the event/data pairing.
        const message = { type: "event", event: name, data: events[name].parse(frame.data) } as ClientEvent;
        if (recordEvents) received.push(message);
        for (const subscriber of subscribers) {
          try { subscriber(message); }
          catch (error: unknown) { console.error(error); }
        }
        return;
      }
      throw new Error("Unknown envelope type");
    } catch (error: unknown) {
      invalidFrame(error instanceof Error ? error.message : "Could not parse message");
    }
  });

  await welcome;

  function call<M extends MethodName>(method: M, params: ParamsIn<M>): Promise<Result<M>> {
    return new Promise<Result<M>>((resolve, reject) => {
      if (failure) { reject(failure); return; }
      if (socket.readyState !== WebSocket.OPEN) { reject(connectionError(1006, "Socket is not open")); return; }
      const id = nextId++;
      pending.set(id, {
        // This schema is selected by the method captured for this pending call.
        accept: (result) => resolve(methods[method].result.parse(result) as Result<M>),
        reject,
      });
      try {
        socket.send(JSON.stringify({ type: "req", id, method, params }));
      } catch (error: unknown) {
        pending.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  return {
    call,
    events: received,
    disconnected,
    subscribe: (listener) => {
      subscribers.add(listener);
      return () => { subscribers.delete(listener); };
    },
    close: async () => {
      if (socket.readyState === WebSocket.CLOSED) return;
      fail(connectionError(1000, "Client closed"));
      socket.close(1000, "Client closed");
      await closed;
    },
  };
}
```

- [ ] **Step 4: Point the daemon at the package**

1. Delete `packages/daemon/src/client.ts`.
2. In `packages/daemon/package.json`, add `"@tjommie/client": "*"` to `dependencies`, before `"@tjommie/protocol"`.
3. In the root `tsconfig.json`, add `"packages/client/src"` to `include`, and also `"packages/desktop/scripts"` (Task 4 adds a script there). It reads:

```json
  "include": ["packages/protocol/src", "packages/client/src", "packages/daemon/src", "packages/daemon/scripts", "packages/desktop/src", "packages/desktop/scripts"]
```

4. Change the client imports:
   - `packages/daemon/src/daemon.test.ts` and `packages/daemon/src/daemon.real.test.ts`: `import { connect, type Client } from "@tjommie/client";`
   - `packages/daemon/src/main.test.ts`: `import { connect } from "@tjommie/client";`
   - `packages/daemon/src/terminal-chat.test.ts`: `import type { ClientEvent } from "@tjommie/client";`
   - `packages/daemon/scripts/chat.ts`: `import { ClientConnectionError, ClientRpcError, connect, type Client, type ClientEvent } from "@tjommie/client";`
5. Run `npm install` from the repo root, so npm links the new workspace package.
6. In `README.md`'s package table, add this row after the `packages/protocol` row:

```markdown
| `packages/client` | The typed WebSocket client for the daemon, shared by the UI, the desktop app and the terminal chat |
```

- [ ] **Step 5: Run the tests**

Run: `npm test` and `npm run typecheck`
Expected: PASS, including the new client test.

Run: `Select-String -Path packages\daemon\src\*.ts,packages\daemon\scripts\*.ts -Pattern 'client\.ts"'`
Expected: no matches.

- [ ] **Step 6: Commit**

```bash
git add packages/client packages/daemon tsconfig.json package-lock.json README.md
git commit -m "Move the typed daemon client into its own package

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Desktop: the daemon process and the app protocol

**Files:**
- Create: `packages/desktop/src/daemon-process.ts`, `packages/desktop/src/app-protocol.ts`
- Modify: `packages/desktop/package.json`
- Test: `packages/desktop/src/daemon-process.test.ts`, `packages/desktop/src/app-protocol.test.ts`

**Interfaces:**
- Consumes: Task 1's daemon, and `@tjommie/client` (tests only).
- Produces, in `daemon-process.ts`:
  - Types: `DaemonConnection = { port, token }`, `DaemonLaunch = { execPath, args, env }` and `RunningDaemon = { connection, pid, exited, stop }`.
  - `newToken(): string` returns 43 base64url characters.
  - `listeningPort(output): number | null`.
  - `daemonEnv(base)` removes `TJOMMIE_DEV` and sets `ELECTRON_RUN_AS_NODE=1`.
  - `startDaemonProcess(launch, onOutput?, timeoutMs?)`:
    - It resolves once the daemon reports its port.
    - It rejects with the daemon's own last stderr line if the daemon exits first.
    - `stop()` closes stdin and waits; it kills the process after 10 s.
- Produces, in `app-protocol.ts`:
  - `uiFile(uiDir, url): string | null`, which maps an `app://tjommie` URL to a file in the static export.
  - `navigationFor(url, production): "stay" | "browser" | "block"`.

- [ ] **Step 1: Write the failing tests**

Create `packages/desktop/src/daemon-process.test.ts`:

```ts
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@tjommie/client";
import { afterEach, describe, expect, it } from "vitest";
import { daemonEnv, listeningPort, newToken, startDaemonProcess, type DaemonLaunch, type RunningDaemon } from "./daemon-process.ts";

const entry = fileURLToPath(new URL("../../daemon/src/main.ts", import.meta.url));
const roots: string[] = [];
const running: RunningDaemon[] = [];

afterEach(async () => {
  for (const daemon of running.splice(0)) await daemon.stop();
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
});

// The daemon from source under this test's Node, on a disposable data root and a free port. TJOMMIE_DEV is set
// on purpose: daemonEnv must remove it, or the daemon refuses to mix development and production modes. PATH is
// emptied, whatever its case, so the daemon finds no CLIs and starts fast (as in the daemon's main.test.ts).
function launch(extraEnv: Record<string, string> = {}): DaemonLaunch {
  const root = mkdtempSync(join(tmpdir(), "tjommie desktop ñ-"));
  roots.push(root);
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PATH"));
  return {
    execPath: process.execPath,
    args: ["--disable-warning=ExperimentalWarning", entry],
    env: { ...inherited, PATH: "", TJOMMIE_HOME: root, TJOMMIE_PORT: "0", TJOMMIE_DEV: "1", ...extraEnv },
  };
}

describe("daemon process", () => {
  it("makes fresh 43-character tokens and finds the port in the daemon's output", () => {
    expect(newToken()).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(newToken()).not.toBe(newToken());
    expect(listeningPort("Removed a leftover development daemon-token file.\nTjommie daemon listening on http://127.0.0.1:52311\n")).toBe(52311);
    expect(listeningPort("starting")).toBeNull();
  });

  it("runs the daemon in production mode as plain Node", () => {
    expect(daemonEnv({ TJOMMIE_DEV: "1", TJOMMIE_HOME: "C:\\x" })).toEqual({ TJOMMIE_HOME: "C:\\x", ELECTRON_RUN_AS_NODE: "1" });
  });

  it("pipes the token in, keeps it out of arguments, environment and output, and stops the daemon by closing stdin (token stays in memory)", async () => {
    const options = launch();
    let output = "";
    const daemon = await startDaemonProcess(options, (text) => { output += text; });
    running.push(daemon);
    const { port, token } = daemon.connection;
    expect(options.args.join(" ")).not.toContain(token);
    expect(JSON.stringify(daemonEnv(options.env))).not.toContain(token);
    const client = await connect(port, token);
    expect(await client.call("bots.list", {})).toEqual([]);
    expect(output).toContain(`listening on http://127.0.0.1:${port}`);
    expect(output).not.toContain(token);
    await daemon.stop();
    expect((await daemon.exited).code).toBe(0);
    await client.disconnected;
  }, 60_000);

  it("reports the daemon's own reason when it can't start", async () => {
    const first = await startDaemonProcess(launch());
    running.push(first);
    const port = first.connection.port;
    await expect(startDaemonProcess(launch({ TJOMMIE_PORT: String(port) }))).rejects.toThrow(`Tjommie is already running: port ${port} is in use.`);
  }, 60_000);
});
```

Create `packages/desktop/src/app-protocol.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, expect, it } from "vitest";
import { navigationFor, uiFile } from "./app-protocol.ts";

function exportFolder(): string {
  const ui = mkdtempSync(join(tmpdir(), "tjommie ui ñ-"));
  for (const file of ["index.html", "404.html", "chat/index.html", "chat/index.txt", "_next/static/chunks/app.js"]) {
    mkdirSync(dirname(join(ui, file)), { recursive: true });
    writeFileSync(join(ui, file), file);
  }
  return ui;
}

describe("uiFile", () => {
  const ui = exportFolder();

  it("maps pages, page data and assets to files in the static export", () => {
    expect(uiFile(ui, "app://tjommie/")).toBe(join(ui, "index.html"));
    expect(uiFile(ui, "app://tjommie/chat/?bot=b1")).toBe(join(ui, "chat", "index.html"));
    expect(uiFile(ui, "app://tjommie/chat")).toBe(join(ui, "chat", "index.html"));
    expect(uiFile(ui, "app://tjommie/chat/index.txt?_rsc=1x2y")).toBe(join(ui, "chat", "index.txt"));
    expect(uiFile(ui, "app://tjommie/_next/static/chunks/app.js")).toBe(join(ui, "_next", "static", "chunks", "app.js"));
  });

  it("refuses paths that leave the export, and missing files", () => {
    expect(uiFile(ui, "app://tjommie/..%5C..%5Csecret.txt")).toBeNull();
    expect(uiFile(ui, "app://tjommie/%E0%A4%A")).toBeNull();
    expect(uiFile(ui, "app://tjommie/missing/")).toBeNull();
  });
});

describe("navigationFor", () => {
  it("keeps Tjommie's own pages in the window and sends web links to the browser (navigation stays in the app)", () => {
    expect(navigationFor("app://tjommie/chat/?bot=b1", true)).toBe("stay");
    expect(navigationFor("https://pnet.co.za/jobs", true)).toBe("browser");
    expect(navigationFor("http://localhost:3000/chat/", true)).toBe("browser");
    expect(navigationFor("http://localhost:3000/chat/", false)).toBe("stay");
    expect(navigationFor("app://tjommie/chat/", false)).toBe("block");
    expect(navigationFor("file:///C:/Windows/win.ini", true)).toBe("block");
    expect(navigationFor("javascript:alert(1)", true)).toBe("block");
    expect(navigationFor("app://evil/", true)).toBe("block");
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/desktop/src`
Expected: FAIL. `./daemon-process.ts` and `./app-protocol.ts` can't be found.

- [ ] **Step 3: Write daemon-process.ts**

Create `packages/desktop/src/daemon-process.ts`:

```ts
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";

export type DaemonConnection = { port: number; token: string };
export type DaemonLaunch = { execPath: string; args: string[]; env: NodeJS.ProcessEnv };
export type RunningDaemon = {
  connection: DaemonConnection;
  pid: number | undefined;
  exited: Promise<{ code: number | null }>;
  stop: () => Promise<void>;
};

const LISTENING = /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/;
const STOP_GRACE_MS = 10_000;

// A fresh token for every daemon start: 32 random bytes, 43 base64url characters.
export const newToken = (): string => randomBytes(32).toString("base64url");

export function listeningPort(output: string): number | null {
  const match = LISTENING.exec(output);
  return match ? Number(match[1]) : null;
}

// Production mode only: TJOMMIE_DEV would make the daemon write a token file. ELECTRON_RUN_AS_NODE makes
// Electron's own binary run the daemon as plain Node.
export function daemonEnv(base: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...base, ELECTRON_RUN_AS_NODE: "1" };
  delete env.TJOMMIE_DEV;
  return env;
}

const lastLine = (text: string): string => text.trim().split(/\r?\n/).at(-1) ?? "";

// Starts the daemon and hands it the token as the first line of stdin. Stdin stays open: the daemon stops
// when it closes, so it never outlives this process. The token never goes into arguments, the environment,
// a file or the output. Resolves once the daemon reports its port.
export function startDaemonProcess(launch: DaemonLaunch, onOutput: (text: string) => void = () => {}, timeoutMs = 60_000): Promise<RunningDaemon> {
  const token = newToken();
  const child = spawn(launch.execPath, [...launch.args, "--token-stdin"], { env: daemonEnv(launch.env), stdio: "pipe", windowsHide: true });
  let stdout = "";
  let stderr = "";
  const exited = new Promise<{ code: number | null }>((resolve) => child.once("close", (code) => resolve({ code })));
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
    onOutput(chunk);
    if (stdout.length < 8_000) stdout += chunk; // only the start matters: the "listening" line
  });
  child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
    onOutput(chunk);
    stderr = (stderr + chunk).slice(-4_000);
  });
  child.stdin.on("error", () => {}); // the daemon may exit before reading; its exit is reported below
  child.stdin.write(`${token}\n`);

  const stop = async (): Promise<void> => {
    if (child.exitCode !== null || child.signalCode !== null) return;
    child.stdin.end(); // the daemon closes its bots and exits
    const timer = setTimeout(() => child.kill(), STOP_GRACE_MS);
    await exited;
    clearTimeout(timer);
  };

  return new Promise<RunningDaemon>((resolve, reject) => {
    let settled = false;
    const fail = (error: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill();
      reject(error);
    };
    const timer = setTimeout(() => fail(new Error("Tjommie's background service didn't start in time.")), timeoutMs);
    child.once("error", fail);
    child.stdout.on("data", () => {
      if (settled) return;
      const port = listeningPort(stdout);
      if (port === null) return;
      settled = true;
      clearTimeout(timer);
      resolve({ connection: { port, token }, pid: child.pid, exited, stop });
    });
    void exited.then(({ code }) => fail(new Error(lastLine(stderr) || `Tjommie's background service stopped (exit code ${code}).`)));
  });
}
```

- [ ] **Step 4: Write app-protocol.ts**

Create `packages/desktop/src/app-protocol.ts`:

```ts
import { existsSync, statSync } from "node:fs";
import { extname, isAbsolute, join, normalize, relative } from "node:path";

export const DEV_ORIGIN = "http://localhost:3000";

// The static export uses trailingSlash, so "/chat/" is "chat/index.html". Next's page data requests
// ("/chat/index.txt?_rsc=...") are plain files; the query string is ignored.
export function uiFile(uiDir: string, url: string): string | null {
  let path: string;
  try {
    path = decodeURIComponent(new URL(url).pathname);
  } catch {
    return null;
  }
  if (path.endsWith("/")) path += "index.html";
  else if (!extname(path)) path += "/index.html";
  const file = normalize(join(uiDir, path));
  const inside = relative(uiDir, file);
  if (inside.startsWith("..") || isAbsolute(inside)) return null;
  return existsSync(file) && statSync(file).isFile() ? file : null;
}

// Only Tjommie's own pages load in the window. Web links open in the default browser; anything else is blocked.
export function navigationFor(url: string, production: boolean): "stay" | "browser" | "block" {
  let target: URL;
  try {
    target = new URL(url);
  } catch {
    return "block";
  }
  const inApp = production ? target.protocol === "app:" && target.host === "tjommie" : target.origin === DEV_ORIGIN;
  if (inApp) return "stay";
  return target.protocol === "https:" || target.protocol === "http:" ? "browser" : "block";
}
```

- [ ] **Step 5: Add the client to the desktop package**

Replace `packages/desktop/package.json` with:

```json
{
  "name": "@tjommie/desktop",
  "productName": "Tjommie",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "main": "src/main.ts",
  "scripts": {
    "start": "electron ."
  },
  "dependencies": {
    "@tjommie/client": "*"
  },
  "devDependencies": {
    "electron": "^44.4.5"
  }
}
```

Run `npm install` from the repo root.

- [ ] **Step 6: Run the tests**

Run: `npx vitest run packages/desktop/src`
Expected: PASS, 7 tests.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add packages/desktop package-lock.json
git commit -m "Add the desktop app's daemon launcher and app protocol mapping

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Desktop: the Electron app

**Files:**
- Modify: `packages/desktop/src/main.ts` (replace the scaffold)
- Create: `packages/desktop/src/preload.cjs`, `packages/desktop/assets/icon.svg`, `packages/desktop/scripts/render-icons.ts`, `packages/desktop/assets/icon-256.png`, `packages/desktop/assets/tray-32.png` (the last two generated)

**Interfaces:**
- Consumes: Task 3's `startDaemonProcess`, `uiFile` and `navigationFor`; `@tjommie/client`.
- Produces the bridge the UI uses, `window.tjommie` (Task 5 types it):
  - `connection(): Promise<{ ok: true; port; token } | { ok: false; error }>`
  - `restartDaemon(): Promise<void>`
  - `onConnection(listener)`, `onDaemonFailed(listener)` and `onNavigate(listener)`; each returns an unsubscribe function.
  - `pickFiles(): Promise<string[]>`
  - `pathForFile(file): string`
  - `showFile(path)`
  - `openFolder(path): Promise<void>`
  - `setTheme("dark" | "light")`
  - `info(): Promise<{ username }>`
- Environment:
  - `TJOMMIE_DESKTOP=production` selects production mode.
  - `TJOMMIE_HOME`, `TJOMMIE_PORT` and `TJOMMIE_ACCOUNTS_DIR` pass through to the daemon.
  - `TJOMMIE_USER_DATA` (tests) moves Electron's own profile, and with it the single-instance lock.

- [ ] **Step 1: Draw the app icon**

The icon is the Team room mark from the rail (the user's choice): three blobs on the rail color.

Create `packages/desktop/assets/icon.svg`:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="14" fill="#111111"/>
  <g transform="translate(7 7) scale(0.6)">
    <path d="M20 2 C29.94 2 38 10.06 38 20 C38 29.94 29.94 38 20 38 C10.06 38 2 29.94 2 20 C2 10.06 10.06 2 20 2 Z" fill="#8C5BF3"/>
    <path d="M17 14 L16 20 M24.5 14 L23.5 20" stroke="#FFFFFF" stroke-width="3" stroke-linecap="round" fill="none"/>
  </g>
  <g transform="translate(33 9) scale(0.6)">
    <path d="M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z" fill="#EC712E"/>
    <path d="M17.5 19 L16.7 25 M24.5 19 L23.7 25" stroke="#FFFFFF" stroke-width="3" stroke-linecap="round" fill="none"/>
  </g>
  <g transform="translate(20 33) scale(0.6)">
    <path d="M11 4 H29 C32.9 4 36 7.1 36 11 V29 C36 32.9 32.9 36 29 36 H11 C7.1 36 4 32.9 4 29 V11 C4 7.1 7.1 4 11 4 Z" fill="#5BC57A"/>
    <path d="M17 14 L16 20 M24.5 14 L23.5 20" stroke="#FFFFFF" stroke-width="3" stroke-linecap="round" fill="none"/>
  </g>
</svg>
```

Create `packages/desktop/scripts/render-icons.ts`:

```ts
// Renders assets/icon.svg into the PNGs the window, taskbar, tray and notifications use. Windows can't load
// SVG icons. Run it after changing the SVG: npx electron packages/desktop/scripts/render-icons.ts
import { app, BrowserWindow } from "electron";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const assets = join(import.meta.dirname, "..", "assets");
const outputs = [
  { size: 256, file: "icon-256.png" },
  { size: 32, file: "tray-32.png" },
];

app.whenReady().then(async () => {
  const svg = readFileSync(join(assets, "icon.svg"), "utf8");
  for (const { size, file } of outputs) {
    const window = new BrowserWindow({ width: size, height: size, show: false, frame: false, transparent: true, webPreferences: { offscreen: true } });
    const html = `<!doctype html><html><body style="margin:0;background:transparent">${svg.replace("<svg ", `<svg width="${size}" height="${size}" `)}</body></html>`;
    await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`);
    await new Promise((resolve) => setTimeout(resolve, 200)); // let the offscreen frame paint
    const image = await window.webContents.capturePage({ x: 0, y: 0, width: size, height: size });
    writeFileSync(join(assets, file), image.toPNG());
    window.destroy();
  }
  app.quit();
});
```

Run: `npx electron packages/desktop/scripts/render-icons.ts`
Expected: `packages/desktop/assets/icon-256.png` (256×256) and `tray-32.png` (32×32) exist. Open `icon-256.png` with the Read tool: three blobs (purple circle, orange cloud, green rounded square) on a dark rounded square.

- [ ] **Step 2: Write the preload bridge**

Create `packages/desktop/src/preload.cjs`:

```js
// Runs in the sandboxed renderer before the UI loads. The UI reaches Electron only through window.tjommie.
// A sandboxed preload can't be an ES module or TypeScript, so this one file is plain CommonJS.
const { contextBridge, ipcRenderer, webUtils } = require("electron");

function listen(channel, listener) {
  const handler = (_event, value) => listener(value);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
}

contextBridge.exposeInMainWorld("tjommie", {
  connection: () => ipcRenderer.invoke("daemon:connection"),
  restartDaemon: () => ipcRenderer.invoke("daemon:restart"),
  onConnection: (listener) => listen("daemon:connection", listener),
  onDaemonFailed: (listener) => listen("daemon:failed", listener),
  onNavigate: (listener) => listen("navigate", listener),
  pickFiles: () => ipcRenderer.invoke("files:pick"),
  pathForFile: (file) => webUtils.getPathForFile(file),
  showFile: (path) => ipcRenderer.send("file:show", path),
  openFolder: (path) => ipcRenderer.invoke("folder:open", path),
  setTheme: (theme) => ipcRenderer.send("theme:set", theme),
  info: () => ipcRenderer.invoke("app:info"),
});
```

- [ ] **Step 3: Replace main.ts**

Replace `packages/desktop/src/main.ts` with:

```ts
import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, net, Notification, protocol, shell, Tray } from "electron";
import { appendFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir, userInfo } from "node:os";
import { isAbsolute, join } from "node:path";
import { pathToFileURL } from "node:url";
import { connect, type Client } from "@tjommie/client";
import { navigationFor, uiFile } from "./app-protocol.ts";
import { startDaemonProcess, type DaemonConnection, type RunningDaemon } from "./daemon-process.ts";

const here = import.meta.dirname;
// Development: the development daemon and `npm run dev` run separately. Production: this process starts the
// daemon and serves the static export. Packaged builds (resources paths, the app id) are plan 1E.
const production = process.env.TJOMMIE_DESKTOP === "production";
const dataRoot = process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");
const assets = join(here, "..", "assets");
const uiDir = join(here, "..", "..", "ui", "out");
const daemonEntry = join(here, "..", "..", "daemon", "src", "main.ts");
const appIcon = join(assets, "icon-256.png");
const CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self' ws://127.0.0.1:*; object-src 'none'; base-uri 'none'; frame-src 'none'";
const THEMES = { dark: { color: "#070707", symbolColor: "#ACACAC" }, light: { color: "#FCFCFC", symbolColor: "#5C5C5C" } } as const;
const TITLE_BAR_HEIGHT = 44;

type ConnectionResult = ({ ok: true } & DaemonConnection) | { ok: false; error: string };

let win: BrowserWindow | null = null;
let tray: Tray | null = null;
let quitting = false;
let daemon: RunningDaemon | null = null;
let current: Promise<DaemonConnection> | null = null;
let notifier: Client | null = null;
let restarts: number[] = [];
const notifications = new Set<Notification>(); // kept alive until closed, so their click handlers fire

function log(text: string): void {
  try {
    mkdirSync(join(dataRoot, "logs"), { recursive: true });
    appendFileSync(join(dataRoot, "logs", "desktop.log"), text.endsWith("\n") ? text : `${text}\n`);
  } catch {
    // logging must never stop the app
  }
}

app.setName("Tjommie");
if (process.env.TJOMMIE_USER_DATA) app.setPath("userData", process.env.TJOMMIE_USER_DATA); // tests: their own profile and lock
protocol.registerSchemesAsPrivileged([{ scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  // An unpackaged Electron needs its own id for Windows to show its notifications.
  app.setAppUserModelId(process.execPath);
  app.on("second-instance", showWindow);
  app.on("before-quit", () => {
    quitting = true;
  });
  app.on("will-quit", (event) => {
    if (!daemon) return;
    event.preventDefault(); // stop the daemon first, so its bots close cleanly
    const running = daemon;
    daemon = null;
    void notifier?.close().catch(() => {});
    void running.stop().finally(() => app.quit());
  });
  app.on("window-all-closed", () => {}); // the window only hides; Tjommie keeps running in the tray
  // No top-level await here: Electron emits "ready" only after this module finishes evaluating.
  app
    .whenReady()
    .then(start)
    .catch((error: unknown) => {
      log(`Startup failed: ${String(error)}`);
      dialog.showErrorBox("Tjommie couldn't start", String(error));
      app.quit();
    });
}

function start(): void {
  if (production) protocol.handle("app", serveUi);
  registerIpc();
  createWindow();
  createTray();
  reconnect(false); // the UI asks for this first connection itself
}

async function serveUi(request: Request): Promise<Response> {
  const file = uiFile(uiDir, request.url);
  // Anything missing gets Next's not-found page, which keeps the rail, so the window (which has no Back button)
  // never strands the person. Pages plan 1E adds show it until then.
  const served = file ?? uiFile(uiDir, "app://tjommie/404.html");
  if (!served) return new Response("Not found", { status: 404, headers: { "content-type": "text/plain" } });
  const response = await net.fetch(pathToFileURL(served).toString());
  const headers = new Headers(response.headers);
  headers.set("content-security-policy", CSP);
  return new Response(response.body, { status: file ? response.status : 404, headers });
}

// Opens a new connection: a new daemon with a new token in production, the development daemon otherwise.
// After a restart the UI gets it pushed when it's ready, or the reason it failed.
function reconnect(push = true): void {
  const next = openConnection();
  current = next;
  next.then(
    (connection) => {
      if (push) win?.webContents.send("daemon:connection", connection);
    },
    (error: unknown) => {
      const reason = error instanceof Error ? error.message : String(error);
      log(`The daemon didn't start: ${reason}`);
      if (push) win?.webContents.send("daemon:failed", reason);
    },
  );
}

async function openConnection(): Promise<DaemonConnection> {
  const connection = production ? await launchDaemon() : developmentConnection();
  void listenForNotifications(connection);
  return connection;
}

function developmentConnection(): DaemonConnection {
  const tokenFile = join(dataRoot, "daemon-token");
  if (!existsSync(tokenFile)) throw new Error("Start the development daemon first: npm start -w @tjommie/daemon");
  return { port: Number(process.env.TJOMMIE_PORT ?? 47100), token: readFileSync(tokenFile, "utf8").trim() };
}

async function launchDaemon(): Promise<DaemonConnection> {
  const running = await startDaemonProcess({ execPath: process.execPath, args: ["--disable-warning=ExperimentalWarning", daemonEntry], env: process.env }, log);
  daemon = running;
  void running.exited.then(({ code }) => onDaemonExit(running, code));
  return running.connection;
}

// The daemon stopped on its own. Start a new one with a new token, at most three times a minute.
function onDaemonExit(running: RunningDaemon, code: number | null): void {
  if (quitting || daemon !== running) return;
  daemon = null;
  log(`The daemon stopped (exit code ${code}).`);
  const now = Date.now();
  restarts = restarts.filter((at) => now - at < 60_000);
  if (restarts.length >= 3) {
    const reason = "Tjommie's background service keeps stopping.";
    current = Promise.reject(new Error(reason));
    current.catch(() => {});
    win?.webContents.send("daemon:failed", reason);
    return;
  }
  restarts.push(now);
  reconnect();
}

async function listenForNotifications(connection: DaemonConnection): Promise<void> {
  const previous = notifier;
  notifier = null;
  await previous?.close().catch(() => {});
  try {
    const client = await connect(connection.port, connection.token, { recordEvents: false });
    notifier = client;
    client.subscribe((event) => {
      if (event.event !== "notify" || !Notification.isSupported()) return;
      if (win?.isVisible() && win.isFocused()) return; // the person is already looking at Tjommie
      const note = new Notification({ title: event.data.title, body: event.data.body, icon: appIcon });
      notifications.add(note);
      note.on("close", () => notifications.delete(note));
      note.on("click", () => {
        notifications.delete(note);
        showWindow();
        win?.webContents.send("navigate", `/chat/?bot=${encodeURIComponent(event.data.botId)}`);
      });
      note.show();
    });
  } catch (error) {
    log(`Notifications are off until the next restart: ${String(error)}`);
  }
}

function registerIpc(): void {
  ipcMain.handle("daemon:connection", async (): Promise<ConnectionResult> => {
    try {
      if (!current) throw new Error("Tjommie is still starting.");
      return { ok: true, ...(await current) };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  });
  ipcMain.handle("daemon:restart", () => {
    if (daemon) return; // it's running; nothing to restart
    restarts = [];
    reconnect();
  });
  ipcMain.handle("files:pick", async () => {
    const options: Electron.OpenDialogOptions = { properties: ["openFile", "multiSelections"] };
    const result = win ? await dialog.showOpenDialog(win, options) : await dialog.showOpenDialog(options);
    return result.canceled ? [] : result.filePaths;
  });
  ipcMain.handle("folder:open", async (_event, path: unknown) => {
    if (typeof path !== "string" || !isAbsolute(path) || !existsSync(path) || !statSync(path).isDirectory()) return;
    await shell.openPath(path);
  });
  ipcMain.on("file:show", (_event, path: unknown) => {
    if (typeof path === "string" && isAbsolute(path)) shell.showItemInFolder(path);
  });
  ipcMain.on("theme:set", (_event, theme: unknown) => {
    const colors = theme === "light" ? THEMES.light : THEMES.dark;
    win?.setTitleBarOverlay({ ...colors, height: TITLE_BAR_HEIGHT });
    win?.setBackgroundColor(colors.color);
  });
  ipcMain.handle("app:info", () => ({ username: userInfo().username }));
}

function createWindow(): void {
  const window = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 960,
    minHeight: 600,
    show: false,
    title: "Tjommie",
    icon: appIcon,
    backgroundColor: THEMES.dark.color,
    titleBarStyle: "hidden",
    titleBarOverlay: { ...THEMES.dark, height: TITLE_BAR_HEIGHT },
    webPreferences: { preload: join(here, "preload.cjs"), contextIsolation: true, sandbox: true, nodeIntegration: false },
  });
  win = window;
  window.once("ready-to-show", () => window.show());
  window.on("close", (event) => {
    if (quitting) return;
    event.preventDefault();
    window.hide();
    hintTray();
  });
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (navigationFor(url, production) === "browser") void shell.openExternal(url);
    return { action: "deny" };
  });
  window.webContents.on("will-navigate", (event, url) => {
    const decision = navigationFor(url, production);
    if (decision === "stay") return;
    event.preventDefault();
    if (decision === "browser") void shell.openExternal(url);
  });
  void window.loadURL(production ? "app://tjommie/" : "http://localhost:3000/");
}

function createTray(): void {
  tray = new Tray(nativeImage.createFromPath(join(assets, "tray-32.png")));
  tray.setToolTip("Tjommie");
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: "Open Tjommie", click: showWindow },
      { type: "separator" },
      { label: "Quit Tjommie", click: () => app.quit() },
    ]),
  );
  tray.on("click", showWindow);
}

function showWindow(): void {
  if (!win) return;
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

// The first time the window closes, say that Tjommie keeps running in the tray.
function hintTray(): void {
  const marker = join(app.getPath("userData"), "tray-hint-shown");
  if (existsSync(marker) || !Notification.isSupported()) return;
  mkdirSync(app.getPath("userData"), { recursive: true });
  writeFileSync(marker, "");
  new Notification({ title: "Tjommie is still running", body: "Your bots and routines keep working. Quit from the Tjommie icon in the taskbar tray.", icon: appIcon }).show();
}
```

- [ ] **Step 4: Check it by hand**

1. Development mode, in three PowerShell windows from the repo root:
   - `npm start -w @tjommie/daemon`
   - `npm run dev`
   - `npm run desktop`

   The window opens with the Windows caption buttons drawn over the dark top-right corner. It shows the scaffold page, which Task 7 replaces.
2. Close the window. It hides, a "Tjommie is still running" notification appears once, and the tray icon shows the three blobs.
3. Tray, then Open Tjommie: the window comes back. Tray, then Quit Tjommie: Electron exits. Stop the other two windows with Ctrl+C.
4. Production mode:
   - Run `npm run build`, then `$env:TJOMMIE_DESKTOP = "production"; npm run desktop`.
   - The window loads from `app://tjommie/`.
   - `Get-Content "$env:LOCALAPPDATA\Tjommie\logs\desktop.log" -Tail 5` shows the daemon's "listening" line and no token.
   - Quit from the tray.
   - `Get-NetTCPConnection -LocalPort 47100 -State Listen -ErrorAction SilentlyContinue` returns nothing, because the daemon stopped with the app.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/desktop
git commit -m "Start the daemon from the desktop app and add the window, tray and notifications

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: UI foundation: tokens, theme, formatting and avatars

**Files:**
- Replace: `packages/ui/next.config.ts`, `packages/ui/app/globals.css`, `vitest.config.ts` (root)
- Create: `packages/ui/lib/desktop.ts`, `packages/ui/lib/theme.ts`, `packages/ui/lib/format.ts`, `packages/ui/lib/labels.ts`, `packages/ui/components/bot-avatar.tsx`
- Delete: `packages/ui/components/ui/button.tsx` (unused; it styled with shadcn's default tokens, which this task removes)
- Modify: `packages/ui/package.json` (npm adds dependencies)
- Test: `packages/ui/lib/format.test.ts`

**Interfaces:**
- Consumes: Task 4's preload bridge (`window.tjommie`).
- Produces:
  - `lib/desktop.ts`: `DesktopApi`, `DesktopConnection`, `ConnectionResult` and `desktop(): DesktopApi | null`.
  - `lib/theme.ts`: `ThemeChoice`, `themeScript` (inline in `<head>`), `themeChoice()`, `applyTheme(choice)` (plan 1E's Settings calls it) and `followTheme(): () => void`.
  - `lib/format.ts`:
    - `clock(value)` gives "9:14 AM"; `dayName(value, now?)`; `stamp(value, now?)` gives "Today 9:14 AM".
    - `fileSize(bytes)`.
    - `schedule(days, times)` gives "Weekdays at 8:48 AM"; `routineTitle(name, limitMinutes)` gives "Pnet apply 9am (3h)".
  - `lib/labels.ts`: `DELETED_BOT`, `botName(bots, id)`, `AUTONOMY_LINE`, `AUTONOMY_HINT`, `modelLabel(bot)`, `stateLabel(bot)` and `errorText(error)`.
  - `components/bot-avatar.tsx`: `BOT_COLORS`, `BotAvatar({ color, shape, size?, className? })`, `MissingAvatar({ size? })` and `TeamMark({ size?, dimmed? })`.
  - **Tailwind color utilities**, one per token, for example `bg-canvas`, `bg-rail`, `bg-bubble`, `text-secondary`, `text-caption`, `border-divider`, `bg-waiting-fill` and `ring-rail`. The full list is the `@theme inline` block below.
  - **CSS helper classes:**
    - `.drag` for header strips that drag the window.
    - `.press` for the 0.97 press.
    - `.markdown` for bot replies.

- [ ] **Step 1: Add the dependencies and the test settings**

In `packages/ui/package.json`, add `"@tjommie/client": "*"` to `dependencies`, before `"@tjommie/protocol"` (a workspace package, so it's written by hand as in Task 2). Then:

Run: `npm install react-markdown@^10.1.0 remark-gfm@^4.0.1 -w @tjommie/ui`
Expected: `packages/ui/package.json` lists all three under `dependencies`, and `node_modules/@tjommie/client` links to `packages/client`.

Replace the root `vitest.config.ts` with:

```ts
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// `npm test` runs fast tests. `npm run test:real` (mode "real") runs the tests that drive the real CLIs.
export default defineConfig(({ mode }) => {
  const real = mode === "real";
  return {
    // The UI's tests render React components and import "@/..." paths the way the UI does.
    oxc: { jsx: { runtime: "automatic" } },
    resolve: { alias: { "@": fileURLToPath(new URL("./packages/ui", import.meta.url)) } },
    test: {
      execArgv: ["--disable-warning=ExperimentalWarning"],
      include: real ? ["packages/*/src/**/*.real.test.ts"] : ["packages/*/src/**/*.test.ts", "packages/ui/{lib,components}/**/*.test.{ts,tsx}"],
      exclude: real ? ["**/node_modules/**"] : ["**/node_modules/**", "**/*.real.test.ts"],
      testTimeout: real ? 600_000 : 10_000,
      hookTimeout: real ? 600_000 : 10_000,
      fileParallelism: !real,
    },
  };
});
```

Replace `packages/ui/next.config.ts` with:

```ts
import type { NextConfig } from "next";

// A static export that the desktop app serves from app://tjommie. trailingSlash gives every page its own
// folder with an index.html, so the app protocol maps paths without guessing. The workspace packages ship
// TypeScript source, so Next compiles them.
const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  transpilePackages: ["@tjommie/protocol", "@tjommie/client"],
  devIndicators: false,
};

export default nextConfig;
```

Delete `packages/ui/components/ui/button.tsx`.

- [ ] **Step 2: Write the failing tests**

Create `packages/ui/lib/format.test.ts`:

```ts
import type { Bot } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { clock, dayName, fileSize, routineTitle, schedule, stamp } from "./format";
import { botName, errorText, modelLabel, stateLabel } from "./labels";

const now = new Date(2026, 8, 27, 14, 0); // Sunday 27 September 2026, 2:00 PM local time

describe("format", () => {
  it("writes times the way the wireframes do", () => {
    expect(clock(new Date(2026, 8, 27, 9, 14))).toBe("9:14 AM");
    expect(clock(new Date(2026, 8, 27, 17, 0))).toBe("5:00 PM");
    expect(stamp(new Date(2026, 8, 27, 9, 14), now)).toBe("Today 9:14 AM");
  });

  it("names days: today, yesterday, a weekday this week, else the date", () => {
    expect(dayName(new Date(2026, 8, 26, 22, 0), now)).toBe("Yesterday");
    expect(dayName(new Date(2026, 8, 22, 9, 0), now)).toBe("Tuesday");
    expect(dayName(new Date(2026, 8, 18, 9, 0), now)).toBe("18 September");
  });

  it("sizes files and describes routine schedules", () => {
    expect(fileSize(512)).toBe("512 bytes");
    expect(fileSize(184 * 1024)).toBe("184 KB");
    expect(fileSize(1.5 * 1024 * 1024)).toBe("1.5 MB");
    expect(schedule([1, 2, 3, 4, 5], ["08:48"])).toBe("Weekdays at 8:48 AM");
    expect(schedule([0, 1, 2, 3, 4, 5, 6], ["07:00", "17:00"])).toBe("Every day at 7:00 AM, 5:00 PM");
    expect(schedule([3, 1], ["09:00"])).toBe("Mon, Wed at 9:00 AM");
    expect(routineTitle("Pnet apply 9am", 180)).toBe("Pnet apply 9am (3h)");
    expect(routineTitle("Night digest", 90)).toBe("Night digest (90m)");
    expect(routineTitle("Night digest", null)).toBe("Night digest");
  });
});

describe("labels", () => {
  it("names bots, models and states, and says when a bot is gone", () => {
    const bots = [{ id: "b1", name: "Pnet apply" }] as Bot[];
    expect(botName(bots, "b1")).toBe("Pnet apply");
    expect(botName(bots, "gone")).toBe("A deleted bot");
    expect(modelLabel({ provider: "claude", model: "sonnet" })).toBe("Claude · Sonnet (latest)");
    expect(modelLabel({ provider: "codex", model: "gpt-6-astra" })).toBe("Codex · gpt-6-astra");
    expect(stateLabel({ state: "limited", limitedUntil: new Date(2026, 8, 27, 15, 45).toISOString() })).toBe("Limited until 3:45 PM");
    expect(stateLabel({ state: "waiting", limitedUntil: null })).toBe("Waiting for you");
    expect(errorText(new Error("bad_params: This bot's browser is off"))).toBe("This bot's browser is off");
  });
});
```

- [ ] **Step 3: Run them to see them fail**

Run: `npx vitest run packages/ui/lib`
Expected: FAIL. `./format` and `./labels` can't be found.

- [ ] **Step 4: Write the helpers**

Create `packages/ui/lib/desktop.ts`:

```ts
export type DesktopConnection = { port: number; token: string };
export type ConnectionResult = ({ ok: true } & DesktopConnection) | { ok: false; error: string };

// The bridge the desktop app's preload script exposes (packages/desktop/src/preload.cjs).
export type DesktopApi = {
  connection(): Promise<ConnectionResult>;
  restartDaemon(): Promise<void>;
  onConnection(listener: (connection: DesktopConnection) => void): () => void;
  onDaemonFailed(listener: (reason: string) => void): () => void;
  onNavigate(listener: (path: string) => void): () => void;
  pickFiles(): Promise<string[]>;
  pathForFile(file: File): string;
  showFile(path: string): void;
  openFolder(path: string): Promise<void>;
  setTheme(theme: "dark" | "light"): void;
  info(): Promise<{ username: string }>;
};

declare global {
  interface Window {
    tjommie?: DesktopApi;
  }
}

// The desktop app's bridge, or null when the page runs anywhere else.
export function desktop(): DesktopApi | null {
  return typeof window === "undefined" ? null : (window.tjommie ?? null);
}
```

Create `packages/ui/lib/theme.ts`:

```ts
import { desktop } from "./desktop";

export type ThemeChoice = "dark" | "light" | "system";
const KEY = "tjommie.theme";

// Runs in <head> before the first paint, so the page never flashes the wrong theme. Dark is the default.
export const themeScript = `(function(){try{var c=localStorage.getItem("${KEY}");var light=c==="light"||(c==="system"&&matchMedia("(prefers-color-scheme: light)").matches);if(!light)document.documentElement.classList.add("dark")}catch(e){document.documentElement.classList.add("dark")}})()`;

export function themeChoice(): ThemeChoice {
  const stored = localStorage.getItem(KEY);
  return stored === "light" || stored === "system" ? stored : "dark";
}

const resolved = (choice: ThemeChoice): "dark" | "light" =>
  choice === "system" ? (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark") : choice;

// Saves the choice, recolors the page, and tells the desktop app so the window's caption buttons match.
export function applyTheme(choice: ThemeChoice): void {
  localStorage.setItem(KEY, choice);
  const theme = resolved(choice);
  document.documentElement.classList.toggle("dark", theme === "dark");
  desktop()?.setTheme(theme);
}

// Applies the saved choice now, and again when Windows switches between light and dark while it's "system".
export function followTheme(): () => void {
  applyTheme(themeChoice());
  const media = matchMedia("(prefers-color-scheme: light)");
  const onChange = (): void => {
    if (themeChoice() === "system") applyTheme("system");
  };
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}
```

Create `packages/ui/lib/format.ts`:

```ts
const CLOCK = new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit" });
const WEEKDAY = new Intl.DateTimeFormat("en-US", { weekday: "long" });
const DAY_MONTH = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "long" });
const DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DAY_MS = 86_400_000;

// "9:14 AM". Newer ICU puts a narrow no-break space before AM; the wireframes use a plain space.
export const clock = (value: string | Date): string => CLOCK.format(new Date(value)).replace(/\u202f/g, " ");

const startOfDay = (date: Date): number => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

// "Today", "Yesterday", a weekday within the last week, otherwise "21 September".
export function dayName(value: string | Date, now: Date = new Date()): string {
  const date = new Date(value);
  const days = Math.round((startOfDay(now) - startOfDay(date)) / DAY_MS);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days > 1 && days < 7) return WEEKDAY.format(date);
  return DAY_MONTH.format(date);
}

// "Today 9:14 AM": the time labels between messages.
export const stamp = (value: string | Date, now?: Date): string => `${dayName(value, now)} ${clock(value)}`;

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

// "Weekdays at 8:48 AM", "Every day at 7:00 AM, 5:00 PM", "Mon, Wed at 9:00 AM".
export function schedule(days: number[], times: string[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const key = sorted.join("");
  const when = key === "0123456" ? "Every day" : key === "12345" ? "Weekdays" : key === "06" ? "Weekends" : sorted.map((day) => DAY_SHORT[day]).join(", ");
  const at = times.map((time) => clock(new Date(2000, 0, 1, Number(time.slice(0, 2)), Number(time.slice(3))))).join(", ");
  return `${when} at ${at}`;
}

// "Pnet apply 9am (3h)": a routine's name with its time limit.
export function routineTitle(name: string, limitMinutes: number | null): string {
  if (!limitMinutes) return name;
  return `${name} (${limitMinutes % 60 === 0 ? `${limitMinutes / 60}h` : `${limitMinutes}m`})`;
}
```

Create `packages/ui/lib/labels.ts`:

```ts
import type { Autonomy, Bot } from "@tjommie/protocol";
import { clock } from "./format";

export const DELETED_BOT = "A deleted bot";

export function botName(bots: Pick<Bot, "id" | "name">[], id: string): string {
  return bots.find((bot) => bot.id === id)?.name ?? DELETED_BOT;
}

// The bot summary's autonomy line.
export const AUTONOMY_LINE: Record<Autonomy, string> = {
  ask: "Always ask: asks before every change",
  folder: "Own folder: works freely in its folder, asks before anything else",
  full: "Full auto: never asks",
};

// The empty chat's second sentence.
export const AUTONOMY_HINT: Record<Autonomy, string> = {
  ask: "It asks before every change.",
  folder: "It works in its own folder and asks before touching anything else.",
  full: "It works without asking first.",
};

const CLAUDE_MODELS: Record<string, string> = { sonnet: "Sonnet (latest)", opus: "Opus (latest)", haiku: "Haiku (latest)" };

export function modelLabel(bot: Pick<Bot, "provider" | "model">): string {
  return bot.provider === "claude" ? `Claude · ${CLAUDE_MODELS[bot.model] ?? bot.model}` : `Codex · ${bot.model}`;
}

export function stateLabel(bot: Pick<Bot, "state" | "limitedUntil">): string {
  switch (bot.state) {
    case "working":
      return "Working";
    case "waiting":
      return "Waiting for you";
    case "paused":
      return "Paused";
    case "limited":
      return bot.limitedUntil ? `Limited until ${clock(bot.limitedUntil)}` : "Limited";
    case "needs_setup":
      return "Needs setup";
    case "idle":
      return "Idle";
  }
}

// A daemon error as a sentence. The client's errors start with their code ("bad_params: ...").
export function errorText(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.replace(/^[a-z_]+: /, "");
}
```

- [ ] **Step 5: Write the avatars**

Create `packages/ui/components/bot-avatar.tsx`:

```tsx
import type { BotColor, BotShape } from "@tjommie/protocol";

// Bot colors are the only decorative color in the app (docs/design/brief.md section 3).
export const BOT_COLORS: Record<BotColor, string> = {
  brown: "#8D6541",
  red: "#EA4144",
  orange: "#EC712E",
  amber: "#F19C3F",
  green: "#5BC57A",
  teal: "#57B8A7",
  blue: "#3B82F6",
  purple: "#8C5BF3",
  pink: "#EC4698",
  gray: "#777777",
};

// One path per shape and two eye strokes, in a 40 x 40 box (docs/design/brief.md section 5).
const SHAPES: Record<BotShape, { body: string; eyes: string }> = {
  circle: {
    body: "M20 2 C29.94 2 38 10.06 38 20 C38 29.94 29.94 38 20 38 C10.06 38 2 29.94 2 20 C2 10.06 10.06 2 20 2 Z",
    eyes: "M17 14 L16 20 M24.5 14 L23.5 20",
  },
  blob: {
    body: "M4 22 C4 12 11.5 6 21 6 C30.5 6 37 12.5 37 21 C37 29.5 30.5 35 21 35 C11 35 4 30.5 4 22 Z",
    eyes: "M18 15 L17 21 M25.5 15 L24.5 21",
  },
  "rounded-square": {
    body: "M11 4 H29 C32.9 4 36 7.1 36 11 V29 C36 32.9 32.9 36 29 36 H11 C7.1 36 4 32.9 4 29 V11 C4 7.1 7.1 4 11 4 Z",
    eyes: "M17 14 L16 20 M24.5 14 L23.5 20",
  },
  pill: {
    body: "M12 10 H28 C33.5 10 38 14.5 38 20 C38 25.5 33.5 30 28 30 H12 C6.5 30 2 25.5 2 20 C2 14.5 6.5 10 12 10 Z",
    eyes: "M18 15.5 L17.3 21.5 M24.5 15.5 L23.8 21.5",
  },
  triangle: {
    body: "M20 4 C21.6 4 22.8 4.8 23.7 6.3 L37 29.8 C38.6 32.6 36.6 35 33.5 35 H6.5 C3.4 35 1.4 32.6 3 29.8 L16.3 6.3 C17.2 4.8 18.4 4 20 4 Z",
    eyes: "M17.8 20 L17.1 26 M23.8 20 L23.1 26",
  },
  hexagon: {
    body: "M17 3.2 C18.9 2.1 21.1 2.1 23 3.2 L33.5 9.3 C35.4 10.4 36.5 12.4 36.5 14.6 V25.4 C36.5 27.6 35.4 29.6 33.5 30.7 L23 36.8 C21.1 37.9 18.9 37.9 17 36.8 L6.5 30.7 C4.6 29.6 3.5 27.6 3.5 25.4 V14.6 C3.5 12.4 4.6 10.4 6.5 9.3 Z",
    eyes: "M17 15 L16 21 M24.5 15 L23.5 21",
  },
  cloud: {
    body: "M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z",
    eyes: "M17.5 19 L16.7 25 M24.5 19 L23.7 25",
  },
  droplet: {
    body: "M20 3 C20 3 33 16 33 25 C33 32.2 27.2 37 20 37 C12.8 37 7 32.2 7 25 C7 16 20 3 20 3 Z",
    eyes: "M17.5 21 L16.8 27 M24 21 L23.3 27",
  },
};

function Shape({ color, shape }: { color: BotColor; shape: BotShape }) {
  const { body, eyes } = SHAPES[shape];
  return (
    <>
      <path d={body} fill={BOT_COLORS[color]} />
      <path d={eyes} stroke="#FFFFFF" strokeWidth={3} strokeLinecap="round" fill="none" />
    </>
  );
}

export function BotAvatar({ color, shape, size = 40, className }: { color: BotColor; shape: BotShape; size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 40 40" width={size} height={size} className={className} aria-hidden="true">
      <Shape color={color} shape={shape} />
    </svg>
  );
}

// A bot that no longer exists, where its avatar would be.
export function MissingAvatar({ size = 18 }: { size?: number }) {
  return <span aria-hidden="true" className="inline-block shrink-0 rounded-full bg-selected" style={{ width: size, height: size }} />;
}

// The Team room mark: the rail's first item, and the app icon (packages/desktop/assets/icon.svg).
export function TeamMark({ size = 40, dimmed = false }: { size?: number; dimmed?: boolean }) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} aria-hidden="true" style={{ opacity: dimmed ? 0.4 : 1 }}>
      <rect width="64" height="64" rx="14" fill="var(--chip)" />
      <g transform="translate(7 7) scale(0.6)">
        <Shape color="purple" shape="circle" />
      </g>
      <g transform="translate(33 9) scale(0.6)">
        <Shape color="orange" shape="cloud" />
      </g>
      <g transform="translate(20 33) scale(0.6)">
        <Shape color="green" shape="rounded-square" />
      </g>
    </svg>
  );
}
```

- [ ] **Step 6: Replace the stylesheet**

Replace `packages/ui/app/globals.css` with:

```css
@import "tailwindcss";

/* Tjommie's colors: the token table in docs/design/wireframe-spec.md ("Colors and type") and its approved
   exceptions. Light values follow design/openpencil/recolor-light.js, the map WF1 light was drawn with; the
   light composer's 1 px #EAEAEA stroke is from WF1 light (dark draws none, so its stroke is the fill).
   --highlight is a popover's highlighted row: the map turns both #1A1A1A and #262626 into #EFEFEF, so light
   mode uses the selected color there instead.
   Components use these names only; bot colors and the Chrome logo are the only raw colors in components. */
:root {
  --canvas: #fcfcfc;
  --rail: #f5f5f5;
  --rail-edge: #ebebeb;
  --card: #f5f5f5;
  --card-stroke: #ebebeb;
  --divider: #e4e4e4;
  --bubble: #efefef;
  --pill: #efefef;
  --chip: #f0f0f0;
  --composer: #ffffff;
  --composer-stroke: #eaeaea;
  --selected: #e1e1e1;
  --popover: #efefef;
  --highlight: #e1e1e1;
  --input-stroke: #d6d6d6;
  --disabled: #f0f0f0;
  --code: #e4e4e4;
  --user-bubble: #0a0a0a;
  --user-text: #ffffff;
  --text: #0a0a0a;
  --title: #0a0a0a;
  --body: #404040;
  --secondary: #6b6b6b;
  --caption: #6b6b6b;
  --label: #6b6b6b;
  --placeholder: #767676;
  --icon: #5c5c5c;
  --accent: #2563eb;
  --working: #17804f;
  --danger: #dc2626;
  --waiting-fill: #eef4ff;
  --waiting-stroke: #c7dbff;
  --mention: #459fff;
  --file-tile: #fdecec;
  --user-avatar: #777777;
}

.dark {
  --canvas: #070707;
  --rail: #111111;
  --rail-edge: #080808;
  --card: #111111;
  --card-stroke: #1c1c1c;
  --divider: #252525;
  --bubble: #262626;
  --pill: #262626;
  --chip: #181818;
  --composer: #2f2f2f;
  --composer-stroke: #2f2f2f;
  --selected: #303030;
  --popover: #1a1a1a;
  --highlight: #262626;
  --input-stroke: #2a2a2a;
  --disabled: #3a3a3a;
  --code: #1a1a1a;
  --user-bubble: #f5f5f5;
  --user-text: #070707;
  --text: #f5f5f5;
  --title: #fcfcfc;
  --body: #bdbdbd;
  --secondary: #989898;
  --caption: #909090;
  --label: #9c9c9c;
  --placeholder: #6e6e6e;
  --icon: #acacac;
  --accent: #459fff;
  --working: #3fa37a;
  --danger: #ea4144;
  --waiting-fill: #101722;
  --waiting-stroke: #1e3a5f;
  --mention: #1d6fe0;
  --file-tile: #3a2020;
  --user-avatar: #777777;
}

@theme inline {
  --font-sans: "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
  --color-canvas: var(--canvas);
  --color-rail: var(--rail);
  --color-rail-edge: var(--rail-edge);
  --color-card: var(--card);
  --color-card-stroke: var(--card-stroke);
  --color-divider: var(--divider);
  --color-bubble: var(--bubble);
  --color-pill: var(--pill);
  --color-chip: var(--chip);
  --color-composer: var(--composer);
  --color-composer-stroke: var(--composer-stroke);
  --color-selected: var(--selected);
  --color-popover: var(--popover);
  --color-highlight: var(--highlight);
  --color-input-stroke: var(--input-stroke);
  --color-disabled: var(--disabled);
  --color-user-bubble: var(--user-bubble);
  --color-user-text: var(--user-text);
  --color-text: var(--text);
  --color-title: var(--title);
  --color-body: var(--body);
  --color-secondary: var(--secondary);
  --color-caption: var(--caption);
  --color-label: var(--label);
  --color-placeholder: var(--placeholder);
  --color-icon: var(--icon);
  --color-accent: var(--accent);
  --color-working: var(--working);
  --color-danger: var(--danger);
  --color-waiting-fill: var(--waiting-fill);
  --color-waiting-stroke: var(--waiting-stroke);
  --color-mention: var(--mention);
  --color-file-tile: var(--file-tile);
  --color-user-avatar: var(--user-avatar);
}

@layer base {
  html,
  body {
    height: 100%;
  }
  body {
    overflow: hidden;
    background: var(--canvas);
    color: var(--text);
    font-family: var(--font-sans);
    font-size: 14px;
    line-height: 21px;
    -webkit-font-smoothing: antialiased;
  }
  * {
    scrollbar-width: thin;
    scrollbar-color: var(--selected) transparent;
  }
  /* A 2 px accent ring: outside buttons, inside inputs (it replaces their border). */
  :focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  input:focus-visible,
  textarea:focus-visible {
    outline-offset: -2px;
  }
}

/* Header strips drag the window; the controls in them stay clickable. */
.drag {
  -webkit-app-region: drag;
}
.drag :where(a, button, input, textarea, select, [role="button"], [role="menu"]) {
  -webkit-app-region: no-drag;
}

/* Buttons press in slightly. Keyboard actions and actions repeated all day don't animate. */
.press {
  transition:
    transform 150ms cubic-bezier(0.23, 1, 0.32, 1),
    background-color 150ms cubic-bezier(0.23, 1, 0.32, 1);
}
.press:active:not(:disabled) {
  transform: scale(0.97);
}

/* Bot replies are Markdown. */
.markdown {
  overflow-wrap: anywhere;
}
.markdown > * + * {
  margin-top: 10px;
}
.markdown ul {
  list-style: disc;
  padding-left: 20px;
}
.markdown ol {
  list-style: decimal;
  padding-left: 20px;
}
.markdown li + li {
  margin-top: 4px;
}
.markdown a {
  color: var(--accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.markdown strong {
  font-weight: 600;
}
.markdown h1,
.markdown h2,
.markdown h3,
.markdown h4 {
  font-size: 15px;
  font-weight: 600;
}
.markdown code {
  border-radius: 6px;
  background: var(--code);
  padding: 1px 5px;
  font-family: "Cascadia Mono", Consolas, monospace;
  font-size: 13px;
}
.markdown pre {
  overflow-x: auto;
  border-radius: 10px;
  background: var(--code);
  padding: 12px;
}
.markdown pre code {
  background: none;
  padding: 0;
}
.markdown blockquote {
  border-left: 2px solid var(--divider);
  padding-left: 12px;
  color: var(--body);
}
.markdown table {
  display: block;
  overflow-x: auto;
  border-collapse: collapse;
}
.markdown th,
.markdown td {
  border: 1px solid var(--divider);
  padding: 4px 8px;
  text-align: left;
}

@media (prefers-reduced-motion: reduce) {
  *,
  ::before,
  ::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```

- [ ] **Step 7: Run the tests and checks**

Run: `npx vitest run packages/ui/lib`
Expected: PASS, 4 tests.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. The scaffold page still builds.

- [ ] **Step 8: Commit**

```bash
git add vitest.config.ts package-lock.json packages/ui
git commit -m "Add the UI's tokens, theme, formatting and bot avatars

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: UI: the app state and the daemon link

**Files:**
- Create: `packages/ui/lib/state.ts`, `packages/ui/lib/store.ts`, `packages/ui/lib/connection.ts`
- Test: `packages/ui/lib/state.test.ts`

**Interfaces:**
- Consumes: `@tjommie/client` (`connect`, `Client`, `ClientEvent`) and Task 5's `desktop()`.
- Produces, in `lib/state.ts` (pure functions, no React):
  - Types:
    - `Conversation = { status: "loading" | "ready"; messages; hasMore; pending: ClientEvent[] }`.
    - `Connection`: a union over `status` of `connecting`, `ready`, `reconnecting`, `update`, `failed` (which has `reason`) and `outside`.
    - `AppState = { connection; bots; setup; settings; routines; conversations }`.
  - `initialState`.
  - `applyEvent(state, event)`, which applies one daemon event.
  - `startLoading(state, id)`, `finishLoading(state, id, page)`, `addOlder(state, id, page)` and `forgetConversation(state, id)`.
  - `connected(state, { bots, settings, routines })`, which resets for a new connection.
- Produces, in `lib/store.ts`: `getState()`, `update(change)` and `useApp(select)`.
- Produces, in `lib/connection.ts`:
  - `startConnection()`, `restartDaemon()` and `daemon(): Client`.
  - `botHistory(botId)`, `loadConversation(id, fetchPage)` and `loadOlder(id, fetchPage)`.

How a chat's history and live events meet:
- The daemon writes a message and then broadcasts its event, and it answers `chat.history` in a single step. So a history page already holds every added or updated message whose event arrived before it.
- The exception is text still streaming: the daemon saves it only when the message ends.
- So events that arrive while a chat's first page loads are kept and replayed on top of the page. Added messages are skipped when already present, updates replace, and deltas append.

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/state.test.ts`:

```ts
import type { ClientEvent } from "@tjommie/client";
import type { Bot, Message, Routine } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { addOlder, applyEvent, connected, finishLoading, initialState, startLoading, type AppState } from "./state";

const bot = (id: string, createdAt: string): Bot => ({
  id,
  name: id,
  color: "blue",
  shape: "circle",
  instructions: "",
  provider: "claude",
  model: "sonnet",
  autonomy: "folder",
  homeFolder: `C:\\bots\\${id}`,
  projectFolder: null,
  browser: false,
  paused: false,
  canMessage: [],
  state: "idle",
  setupReason: null,
  limitedUntil: null,
  createdAt,
});
const message = (id: string, extra: Partial<Message> = {}): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind: "text",
  body: "",
  data: null,
  status: "done",
  createdAt: "2026-09-27T09:00:00.000Z",
  ...extra,
});
const routine = (id: string, botId: string): Routine => ({
  id,
  botId,
  name: "Morning check",
  prompt: "Check pnet.co.za",
  days: [1, 2, 3, 4, 5],
  times: ["08:48"],
  timeZone: "Africa/Johannesburg",
  timeLimitMinutes: null,
  enabled: true,
  nextRunAt: null,
  createdAt: "2026-09-27T08:00:00.000Z",
});
const added = (data: Message): ClientEvent => ({ type: "event", event: "message.added", data });
const updated = (data: Message): ClientEvent => ({ type: "event", event: "message.updated", data });
const delta = (id: string, text: string): ClientEvent => ({ type: "event", event: "message.delta", data: { id, conversationId: "b1", delta: text } });
const ready = (messages: Message[]): AppState => finishLoading(startLoading(initialState, "b1"), "b1", { messages, hasMore: false });

describe("app state", () => {
  it("keeps bots in creation order and forgets a deleted bot's chat and routines", () => {
    let state = connected(initialState, { bots: [bot("b2", "2026-09-27T10:00:00.000Z"), bot("b1", "2026-09-27T09:00:00.000Z")], settings: { chainLimit: 5 }, routines: [routine("r1", "b1")] });
    expect(state.bots.map((b) => b.id)).toEqual(["b1", "b2"]);
    state = applyEvent(state, { type: "event", event: "bot.updated", data: { ...bot("b1", "2026-09-27T09:00:00.000Z"), state: "working" } });
    expect(state.bots[0].state).toBe("working");
    state = startLoading(state, "b1");
    state = applyEvent(state, { type: "event", event: "bot.deleted", data: { id: "b1" } });
    expect(state.bots.map((b) => b.id)).toEqual(["b2"]);
    expect(state.routines).toEqual([]);
    expect(state.conversations.b1).toBeUndefined();
  });

  it("applies new, updated and streaming messages to an open chat, and ignores chats that aren't open", () => {
    let state = ready([message("m1", { body: "Hi" })]);
    state = applyEvent(state, added(message("m2", { status: "streaming" })));
    state = applyEvent(state, delta("m2", "Wor"));
    state = applyEvent(state, delta("m2", "king"));
    expect(state.conversations.b1.messages.map((m) => m.body)).toEqual(["Hi", "Working"]);
    state = applyEvent(state, updated(message("m2", { body: "Working on it.", status: "done" })));
    expect(state.conversations.b1.messages[1]).toMatchObject({ body: "Working on it.", status: "done" });
    const other = applyEvent(state, added(message("x1", { conversationId: "b9" })));
    expect(other).toBe(state);
  });

  it("rebuilds a reply that was streaming while its chat opened (opened mid-stream)", () => {
    let state = startLoading(initialState, "b1");
    state = applyEvent(state, delta("m1", "Hel"));
    state = applyEvent(state, delta("m1", "lo"));
    state = finishLoading(state, "b1", { messages: [message("m1", { status: "streaming", body: "" })], hasMore: false });
    expect(state.conversations.b1.messages[0].body).toBe("Hello");
    state = applyEvent(state, delta("m1", " there"));
    expect(state.conversations.b1.messages[0].body).toBe("Hello there");
    state = applyEvent(state, updated(message("m1", { status: "done", body: "Hello there!" })));
    expect(state.conversations.b1.messages[0]).toMatchObject({ status: "done", body: "Hello there!" });
  });

  it("shows a message that arrived while its chat was loading exactly once", () => {
    let state = startLoading(initialState, "b1");
    state = applyEvent(state, added(message("m2", { body: "New" })));
    state = finishLoading(state, "b1", { messages: [message("m1"), message("m2", { body: "New" })], hasMore: true });
    expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m1", "m2"]);
    expect(state.conversations.b1.hasMore).toBe(true);
  });

  it("adds older pages above without duplicates, and starts over on a new connection", () => {
    let state = ready([message("m3"), message("m4")]);
    state = addOlder(state, "b1", { messages: [message("m1"), message("m2"), message("m3")], hasMore: false });
    expect(state.conversations.b1.messages.map((m) => m.id)).toEqual(["m1", "m2", "m3", "m4"]);
    state = connected(state, { bots: [], settings: { chainLimit: 5 }, routines: [] });
    expect(state.conversations).toEqual({});
    expect(state.connection).toEqual({ status: "ready" });
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/state.test.ts`
Expected: FAIL. `./state` can't be found.

- [ ] **Step 3: Write state.ts**

Create `packages/ui/lib/state.ts`:

```ts
import type { ClientEvent } from "@tjommie/client";
import type { Bot, Message, ProviderSetup, Routine, Settings } from "@tjommie/protocol";

export type Conversation = {
  status: "loading" | "ready";
  messages: Message[];
  hasMore: boolean;
  pending: ClientEvent[]; // message events that arrived while the first page was loading
};

export type Connection =
  | { status: "connecting" }
  | { status: "ready" }
  | { status: "reconnecting" }
  | { status: "update" }
  | { status: "failed"; reason: string }
  | { status: "outside" };

export type AppState = {
  connection: Connection;
  bots: Bot[];
  setup: ProviderSetup[] | null;
  settings: Settings | null;
  routines: Routine[];
  conversations: Record<string, Conversation>;
};

export const initialState: AppState = { connection: { status: "connecting" }, bots: [], setup: null, settings: null, routines: [], conversations: {} };

type Page = { messages: Message[]; hasMore: boolean };
type MessageEvent = Extract<ClientEvent, { event: "message.added" | "message.updated" | "message.delta" }>;

const byCreated = <T extends { createdAt: string }>(a: T, b: T): number => a.createdAt.localeCompare(b.createdAt);

function upsert<T extends { id: string; createdAt: string }>(list: T[], item: T): T[] {
  return [...list.filter((existing) => existing.id !== item.id), item].sort(byCreated);
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  return Object.fromEntries(Object.entries(record).filter(([name]) => name !== key));
}

// An update or delta for a message outside the loaded pages (an old card being answered) changes nothing.
function applyMessageEvent(messages: Message[], event: MessageEvent): Message[] {
  switch (event.event) {
    case "message.added":
      return messages.some((m) => m.id === event.data.id) ? messages : [...messages, event.data];
    case "message.updated":
      return messages.map((m) => (m.id === event.data.id ? event.data : m));
    case "message.delta":
      return messages.map((m) => (m.id === event.data.id ? { ...m, body: m.body + event.data.delta } : m));
  }
}

export function applyEvent(state: AppState, event: ClientEvent): AppState {
  switch (event.event) {
    case "bot.updated":
      return { ...state, bots: upsert(state.bots, event.data) };
    case "bot.deleted":
      return {
        ...state,
        bots: state.bots.filter((bot) => bot.id !== event.data.id),
        routines: state.routines.filter((routine) => routine.botId !== event.data.id),
        conversations: without(state.conversations, event.data.id),
      };
    case "setup.changed":
      return { ...state, setup: event.data };
    case "settings.changed":
      return { ...state, settings: event.data };
    case "routine.updated":
      return { ...state, routines: upsert(state.routines, event.data) };
    case "routine.deleted":
      return { ...state, routines: state.routines.filter((routine) => routine.id !== event.data.id) };
    case "message.added":
    case "message.updated":
    case "message.delta": {
      const id = event.data.conversationId;
      const conversation = state.conversations[id];
      if (!conversation) return state; // not open: its history loads when it opens
      const next: Conversation =
        conversation.status === "loading"
          ? { ...conversation, pending: [...conversation.pending, event] }
          : { ...conversation, messages: applyMessageEvent(conversation.messages, event) };
      return { ...state, conversations: { ...state.conversations, [id]: next } };
    }
    default:
      return state; // approvals arrive as messages, and the desktop app shows notifications
  }
}

export function startLoading(state: AppState, conversationId: string): AppState {
  return { ...state, conversations: { ...state.conversations, [conversationId]: { status: "loading", messages: [], hasMore: false, pending: [] } } };
}

// The first page arrived. Events that came in meanwhile are replayed on top: the page already holds added
// and updated messages, but not text that is still streaming, which the daemon only saves when it ends.
export function finishLoading(state: AppState, conversationId: string, page: Page): AppState {
  const conversation = state.conversations[conversationId];
  if (!conversation || conversation.status !== "loading") return state;
  const messages = conversation.pending.reduce((list, event) => applyMessageEvent(list, event as MessageEvent), page.messages);
  return { ...state, conversations: { ...state.conversations, [conversationId]: { status: "ready", messages, hasMore: page.hasMore, pending: [] } } };
}

export function addOlder(state: AppState, conversationId: string, page: Page): AppState {
  const conversation = state.conversations[conversationId];
  if (!conversation || conversation.status !== "ready") return state;
  const known = new Set(conversation.messages.map((m) => m.id));
  const older = page.messages.filter((m) => !known.has(m.id));
  return { ...state, conversations: { ...state.conversations, [conversationId]: { ...conversation, messages: [...older, ...conversation.messages], hasMore: page.hasMore } } };
}

export function forgetConversation(state: AppState, conversationId: string): AppState {
  return { ...state, conversations: without(state.conversations, conversationId) };
}

// A new connection: everything is fetched again, and open chats reload their history.
export function connected(state: AppState, snapshot: { bots: Bot[]; settings: Settings; routines: Routine[] }): AppState {
  return {
    ...state,
    connection: { status: "ready" },
    bots: [...snapshot.bots].sort(byCreated),
    settings: snapshot.settings,
    routines: [...snapshot.routines].sort(byCreated),
    conversations: {},
  };
}
```

- [ ] **Step 4: Write the store and the connection**

Create `packages/ui/lib/store.ts`:

```ts
import { useSyncExternalStore } from "react";
import { initialState, type AppState } from "./state";

let state = initialState;
const listeners = new Set<() => void>();

export function getState(): AppState {
  return state;
}

export function update(change: (current: AppState) => AppState): void {
  const next = change(state);
  if (next === state) return;
  state = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Reads part of the app state. The selector must return something that's already in the state (never a new
// array or object), or React re-renders forever; derive new values with useMemo in the component.
export function useApp<T>(select: (current: AppState) => T): T {
  return useSyncExternalStore(subscribe, () => select(state), () => select(initialState));
}
```

Create `packages/ui/lib/connection.ts`:

```ts
import { connect, type Client, type ClientEvent } from "@tjommie/client";
import type { Message } from "@tjommie/protocol";
import { desktop, type DesktopConnection } from "./desktop";
import { addOlder, applyEvent, connected, finishLoading, forgetConversation, startLoading } from "./state";
import { getState, update } from "./store";

const PAGE = 50;
let client: Client | null = null;
let latest: DesktopConnection | null = null;
let retry: ReturnType<typeof setTimeout> | undefined;
let started = false;

type Page = { messages: Message[]; hasMore: boolean };

// The live daemon client. Screens call it only while the connection is ready.
export function daemon(): Client {
  if (!client) throw new Error("Tjommie isn't connected right now.");
  return client;
}

// Connects once for the life of the window, and again whenever the desktop app restarts the daemon.
export function startConnection(): void {
  if (started) return;
  started = true;
  const api = desktop();
  if (!api) {
    update((state) => ({ ...state, connection: { status: "outside" } }));
    return;
  }
  api.onConnection((next) => open(next));
  api.onDaemonFailed(failed);
  void api.connection().then((result) => {
    if (result.ok) open({ port: result.port, token: result.token });
    else failed(result.error);
  });
}

// The desktop app couldn't start the daemon. Stop retrying the old connection; "Try again" asks for a new one.
function failed(reason: string): void {
  clearTimeout(retry);
  latest = null;
  update((state) => ({ ...state, connection: { status: "failed", reason } }));
}

// Asks the desktop app to start the daemon again after it failed.
export function restartDaemon(): void {
  update((state) => ({ ...state, connection: { status: "connecting" } }));
  void desktop()?.restartDaemon();
}

function open(connection: DesktopConnection): void {
  if (latest?.token === connection.token) return; // already connecting with this one
  latest = connection;
  void attempt(connection);
}

async function attempt(connection: DesktopConnection): Promise<void> {
  clearTimeout(retry);
  const previous = client;
  client = null;
  await previous?.close().catch(() => {});
  let next: Client;
  try {
    next = await connect(connection.port, connection.token, { recordEvents: false });
  } catch (error) {
    lost(connection, error);
    return;
  }
  if (latest !== connection) {
    void next.close();
    return;
  }
  // Events that arrive while the lists load are applied on top of them, in order. The daemon sends events and
  // replies in the order things happen, so the last word on each bot is always the newest.
  let early: ClientEvent[] | null = [];
  next.subscribe((event) => {
    if (early) early.push(event);
    else update((state) => applyEvent(state, event));
  });
  try {
    const [bots, settings, routines] = await Promise.all([next.call("bots.list", {}), next.call("settings.get", {}), next.call("routines.list", {})]);
    if (latest !== connection) {
      void next.close();
      return;
    }
    const pending = early;
    early = null;
    client = next;
    update((state) => pending.reduce((current, event) => applyEvent(current, event), connected(state, { bots, settings, routines })));
  } catch (error) {
    void next.close();
    lost(connection, error);
    return;
  }
  // setup.status runs both CLIs' checks, so it's asked once per connection and never polled.
  next.call("setup.status", {}).then(
    (setup) => update((state) => ({ ...state, setup })),
    () => {},
  );
  void next.disconnected.then((error) => {
    if (client === next) lost(connection, error);
  });
}

function lost(connection: DesktopConnection, error: unknown): void {
  client = null;
  if (typeof error === "object" && error !== null && "code" in error && error.code === 4003) {
    update((state) => ({ ...state, connection: { status: "update" } }));
    return;
  }
  update((state) => (state.connection.status === "failed" ? state : { ...state, connection: { status: "reconnecting" } }));
  // If the daemon restarts, the desktop app sends a new connection. Until then, keep trying this one in
  // case only the socket dropped.
  retry = setTimeout(() => {
    if (latest === connection) void attempt(connection);
  }, 2_000);
}

// A bot chat's pages, newest first.
export const botHistory =
  (botId: string) =>
  (before?: string): Promise<Page> =>
    daemon().call("chat.history", { botId, before, limit: PAGE });

// Loads a conversation's newest page once; events keep it current from then on.
export async function loadConversation(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  if (getState().conversations[conversationId]) return;
  update((state) => startLoading(state, conversationId));
  try {
    const page = await fetchPage();
    update((state) => finishLoading(state, conversationId, page));
  } catch {
    update((state) => forgetConversation(state, conversationId));
  }
}

// Loads the page before the oldest loaded message.
export async function loadOlder(conversationId: string, fetchPage: (before?: string) => Promise<Page>): Promise<void> {
  const conversation = getState().conversations[conversationId];
  if (!conversation || conversation.status !== "ready" || !conversation.hasMore || conversation.messages.length === 0) return;
  const page = await fetchPage(conversation.messages[0].id);
  update((state) => addOlder(state, conversationId, page));
}
```

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/ui/lib`
Expected: PASS, 9 tests (4 from Task 5, 5 new).

Run: `npm run typecheck` and `npm run lint`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/lib
git commit -m "Keep the UI's app state in step with the daemon

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: UI: the app shell and the rail

**Files:**
- Replace: `packages/ui/app/layout.tsx`, `packages/ui/app/page.tsx`
- Create: `packages/ui/components/app-shell.tsx`, `packages/ui/components/rail.tsx`, `packages/ui/components/connection-screen.tsx`, `packages/ui/components/pill.tsx`, `packages/ui/components/icon-button.tsx`

**Interfaces:**
- Consumes: Tasks 5 and 6.
- Produces:
  - `AppShell` wraps every page. It connects to the daemon and shows the rail. Until the connection is ready it shows `ConnectionScreen`. While reconnecting it shows the pages with a "Reconnecting to Tjommie" pill.
  - `Rail` shows:
    - the Team room mark (`/room/`);
    - the bots in creation order, each with its state mark from the wireframe spec's rail rules. Each links to `/chat/?bot=<id>` with the label "<name>, <state>".
    - Agents (`/agents/`), New bot (`/bots/new/`) and Settings (`/settings/`);
    - the user's initial.
  - `Pill({ primary? })` is the wireframes' pill button. `IconButton({ label, active? })` is a 32 px icon button.
  - The home page (`/`) opens the first bot's chat, or says "No bots yet.". Plan 1E replaces that with first-run setup.

- [ ] **Step 1: Write the small controls**

Create `packages/ui/components/pill.tsx`:

```tsx
import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

// Pill buttons from the wireframes: light for the main action, dark for the others.
export function Pill({ primary = false, className, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { primary?: boolean }) {
  return (
    <button
      type="button"
      className={cn(
        "press inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full px-3.5 text-[13px] font-medium whitespace-nowrap disabled:opacity-50",
        primary ? "bg-user-bubble text-user-text" : "bg-pill text-text",
        className,
      )}
      {...props}
    />
  );
}
```

Create `packages/ui/components/icon-button.tsx`:

```tsx
import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

// A 32 px icon button. Its label is also its tooltip.
export function IconButton({ label, active = false, className, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active || undefined}
      className={cn(
        "press flex size-8 shrink-0 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text [&_svg]:size-4",
        active && "bg-selected text-text",
        className,
      )}
      {...props}
    />
  );
}
```

- [ ] **Step 2: Write the rail**

Create `packages/ui/components/rail.tsx`:

```tsx
"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useState, type ReactNode } from "react";
import { Hourglass, LayoutGrid, Plug, Plus, Settings } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { desktop } from "@/lib/desktop";
import { stateLabel } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { BotAvatar, TeamMark } from "./bot-avatar";

export function Rail() {
  const bots = useApp((state) => state.bots);
  const pathname = usePathname();
  const params = useSearchParams();
  const openBot = pathname.startsWith("/chat") ? params.get("bot") : null;
  const [initial, setInitial] = useState("");
  useEffect(() => {
    void desktop()
      ?.info()
      .then((info) => setInitial(info.username.slice(0, 1).toUpperCase()));
  }, []);
  return (
    <nav aria-label="Tjommie" className="flex w-[88px] shrink-0 flex-col items-center border-r border-rail-edge bg-rail py-4">
      <RailLink href="/room/" label="Team room" selected={pathname.startsWith("/room")}>
        <TeamMark size={40} dimmed={bots.length === 0} />
      </RailLink>
      <div className="my-2 h-px w-10 shrink-0 bg-divider" />
      <div className="flex min-h-0 w-full flex-1 flex-col items-center overflow-y-auto">
        {bots.map((bot) => (
          <BotLink key={bot.id} bot={bot} selected={bot.id === openBot} />
        ))}
      </div>
      <div className="mt-2 flex shrink-0 flex-col items-center">
        <RailLink href="/agents/" label="Agents" selected={pathname.startsWith("/agents")} compact>
          <LayoutGrid className="size-5 text-icon" aria-hidden />
        </RailLink>
        <RailLink href="/bots/new/" label="New bot" selected={pathname.startsWith("/bots/new")} compact>
          <Plus className="size-5 text-icon" aria-hidden />
        </RailLink>
        <RailLink href="/settings/" label="Settings" selected={pathname.startsWith("/settings")} compact>
          <Settings className="size-5 text-icon" aria-hidden />
        </RailLink>
        <span aria-hidden className="mt-[14px] flex size-8 items-center justify-center rounded-full bg-user-avatar text-[13px] font-semibold text-white">
          {initial}
        </span>
      </div>
    </nav>
  );
}

// The selected item sits on a 52 px square. The bottom items are 44 px apart, so their squares overlap a little.
function RailLink({ href, label, selected, compact = false, children }: { href: string; label: string; selected: boolean; compact?: boolean; children: ReactNode }) {
  return (
    <Link
      href={href}
      aria-label={label}
      title={label}
      aria-current={selected ? "page" : undefined}
      className={cn("flex size-[52px] shrink-0 items-center justify-center rounded-[12px] hover:bg-pill", compact && "-my-1", selected && "bg-selected hover:bg-selected")}
    >
      {children}
    </Link>
  );
}

// State marks (wireframe spec, "Rail and header"): Working is a green dot at the bottom right, Waiting a blue
// dot at the top right, Paused is 50% opacity, Limited is 50% plus an hourglass badge, Needs setup a plug badge.
function BotLink({ bot, selected }: { bot: Bot; selected: boolean }) {
  const dimmed = bot.state === "paused" || bot.state === "limited";
  const ring = selected ? "ring-selected" : "ring-rail";
  return (
    <Link
      href={`/chat/?bot=${encodeURIComponent(bot.id)}`}
      aria-label={`${bot.name}, ${stateLabel(bot)}`}
      title={bot.name}
      aria-current={selected ? "page" : undefined}
      className={cn("relative flex size-[52px] shrink-0 items-center justify-center rounded-[12px] hover:bg-pill", selected && "bg-selected hover:bg-selected")}
    >
      <BotAvatar color={bot.color} shape={bot.shape} size={40} className={cn(dimmed && "opacity-50")} />
      {bot.state === "working" && <span className={cn("absolute left-[39px] top-[39px] size-2.5 rounded-full bg-working ring-2", ring)} />}
      {bot.state === "waiting" && <span className={cn("absolute left-[38px] top-[5px] size-2.5 rounded-full bg-accent ring-2", ring)} />}
      {(bot.state === "limited" || bot.state === "needs_setup") && (
        <span className={cn("absolute left-[35px] top-[35px] flex size-4 items-center justify-center rounded-full bg-pill ring-2", ring)}>
          {bot.state === "limited" ? <Hourglass className="size-2.5 text-icon" aria-hidden /> : <Plug className="size-2.5 text-icon" aria-hidden />}
        </span>
      )}
    </Link>
  );
}
```

- [ ] **Step 3: Write the shell and the connection screen**

Create `packages/ui/components/connection-screen.tsx`:

```tsx
"use client";

import { Loader } from "lucide-react";
import { restartDaemon } from "@/lib/connection";
import type { Connection } from "@/lib/state";
import { Pill } from "./pill";

type Blocking = Exclude<Connection, { status: "ready" } | { status: "reconnecting" }>;

const COPY: Record<Exclude<Blocking["status"], "failed">, { title: string; body: string | null }> = {
  connecting: { title: "Starting Tjommie", body: null },
  update: {
    title: "Update Tjommie on this device",
    body: "This window and Tjommie's background service are different versions. Install the latest Tjommie, then open it again.",
  },
  outside: {
    title: "Open Tjommie from the desktop app",
    body: "This page talks to Tjommie's background service, which only the desktop app can reach.",
  },
};

// Shown in place of the pages until the daemon connection is ready.
export function ConnectionScreen({ connection }: { connection: Blocking }) {
  return (
    <div className="flex flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 pb-11 text-center">
        {connection.status === "connecting" && <Loader className="size-5 animate-spin text-icon" aria-hidden />}
        {connection.status === "failed" ? (
          <>
            <h1 className="text-[17px] font-semibold text-title">Tjommie&apos;s background service didn&apos;t start</h1>
            <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">{connection.reason}</p>
            <Pill primary onClick={restartDaemon}>
              Try again
            </Pill>
          </>
        ) : (
          <>
            <h1 className="text-[17px] font-semibold text-title">{COPY[connection.status].title}</h1>
            {COPY[connection.status].body && <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">{COPY[connection.status].body}</p>}
          </>
        )}
      </div>
    </div>
  );
}
```

Create `packages/ui/components/app-shell.tsx`:

```tsx
"use client";

import { Suspense, useEffect, type ReactNode } from "react";
import { useRouter } from "next/navigation";
import { Loader } from "lucide-react";
import { startConnection } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { useApp } from "@/lib/store";
import { followTheme } from "@/lib/theme";
import { ConnectionScreen } from "./connection-screen";
import { Rail } from "./rail";

export function AppShell({ children }: { children: ReactNode }) {
  const router = useRouter();
  const connection = useApp((state) => state.connection);
  useEffect(() => startConnection(), []);
  useEffect(() => followTheme(), []);
  useEffect(() => desktop()?.onNavigate((path) => router.push(path)), [router]);
  return (
    <div className="flex h-screen">
      <Suspense fallback={<div className="w-[88px] shrink-0 border-r border-rail-edge bg-rail" />}>
        <Rail />
      </Suspense>
      <main className="relative flex min-w-0 flex-1 flex-col">
        {connection.status === "ready" || connection.status === "reconnecting" ? children : <ConnectionScreen connection={connection} />}
        {connection.status === "reconnecting" && (
          <div role="status" className="absolute left-1/2 top-14 z-10 flex -translate-x-1/2 items-center gap-2 rounded-full bg-popover px-3 py-1.5 text-[12px] text-secondary">
            <Loader className="size-3.5 animate-spin" aria-hidden />
            Reconnecting to Tjommie
          </div>
        )}
      </main>
    </div>
  );
}
```

- [ ] **Step 4: Replace the layout and the home page**

Replace `packages/ui/app/layout.tsx` with:

```tsx
import type { Metadata } from "next";
import { AppShell } from "@/components/app-shell";
import { themeScript } from "@/lib/theme";
import "./globals.css";

export const metadata: Metadata = {
  title: "Tjommie",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript }} />
      </head>
      <body>
        <AppShell>{children}</AppShell>
      </body>
    </html>
  );
}
```

Replace `packages/ui/app/page.tsx` with:

```tsx
"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { useApp } from "@/lib/store";

// Opens the first bot's chat. First-run setup and the New bot screen replace the empty state later.
export default function Home() {
  const router = useRouter();
  const first = useApp((state) => state.bots[0]?.id);
  useEffect(() => {
    if (first) router.replace(`/chat/?bot=${encodeURIComponent(first)}`);
  }, [first, router]);
  if (first) return null;
  return (
    <div className="flex flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <p className="m-auto text-[13px] text-secondary">No bots yet.</p>
    </div>
  );
}
```

- [ ] **Step 5: Check it**

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass.

In development mode (Task 4 Step 4, with a bot created by `npm run chat -w @tjommie/daemon -- "Pnet apply" claude`):
- The rail shows the Team room mark, a divider, Pnet apply, and at the bottom Agents, New bot, Settings and your initial.
- Opening the bot selects it on a 52 px square. `/chat/` itself is Task 12, so for now it shows a not-found page.
- Stop the development daemon: the window shows "Reconnecting to Tjommie". Start it again: the pill disappears within a few seconds.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/app packages/ui/components
git commit -m "Add the app shell, the rail and the connection screens

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Chat: bubbles, Markdown, notices and time labels

**Files:**
- Create: `packages/ui/components/chat/markdown.tsx`, `packages/ui/components/chat/bubbles.tsx`, `packages/ui/components/chat/notices.tsx`, `packages/ui/lib/timeline.ts`
- Test: `packages/ui/components/chat/markdown.test.tsx`, `packages/ui/lib/timeline.test.ts`

**Interfaces:**
- Consumes: Tasks 5 to 7.
- Produces:
  - `MarkdownText({ text })`.
  - `BotBubble({ message })`: while the message streams it shows plain text with a caret; once done, Markdown. It carries `data-author="bot"`.
  - `UserBubble({ message, files })`: plain text, attachment chips and "Queued". It carries `data-author="you"`.
  - `Timestamp({ label })`, `Notice({ text })`, `ErrorNotice({ text, retry })` and `RoutineLabel({ text, data })`.
  - `timeline(messages, now?)` returns `{ type: "stamp", key, label } | { type: "message", key, message }` items.

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/components/chat/markdown.test.tsx`:

```tsx
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MarkdownText } from "./markdown";

describe("MarkdownText (markdown safety)", () => {
  it("formats Markdown", () => {
    const html = renderToStaticMarkup(<MarkdownText text={"**Done.** Two roles:\n\n- Sabenza IT\n- Takealot"} />);
    expect(html).toContain("<strong>Done.</strong>");
    expect(html).toContain("<li>Sabenza IT</li>");
  });

  it("never renders HTML from a reply, and drops script links", () => {
    const html = renderToStaticMarkup(<MarkdownText text={'<img src=x onerror="alert(1)"> [click](javascript:alert(1)) <script>alert(2)</script>'} />);
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("javascript:");
  });

  it("opens links outside the app window", () => {
    const html = renderToStaticMarkup(<MarkdownText text="See https://pnet.co.za/jobs" />);
    expect(html).toContain('href="https://pnet.co.za/jobs"');
    expect(html).toContain('target="_blank"');
  });
});
```

Create `packages/ui/lib/timeline.test.ts`:

```ts
import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { timeline } from "./timeline";

const at = (hour: number, minute: number, day = 27) => new Date(2026, 8, day, hour, minute).toISOString();
const message = (id: string, createdAt: string): Message => ({ id, conversationId: "b1", author: "b1", kind: "text", body: id, data: null, status: "done", createdAt });

describe("timeline", () => {
  it("labels the first message, gaps of 15 minutes or more, and each new day", () => {
    const now = new Date(2026, 8, 27, 12, 0);
    const items = timeline([message("a", at(9, 14, 26)), message("b", at(9, 14)), message("c", at(9, 20)), message("d", at(9, 40))], now);
    expect(items.map((item) => (item.type === "stamp" ? item.label : item.message.id))).toEqual(["Yesterday 9:14 AM", "a", "Today 9:14 AM", "b", "c", "Today 9:40 AM", "d"]);
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/components packages/ui/lib/timeline.test.ts`
Expected: FAIL. `./markdown` and `./timeline` can't be found.

- [ ] **Step 3: Write the pieces**

Create `packages/ui/lib/timeline.ts`:

```ts
import type { Message } from "@tjommie/protocol";
import { stamp } from "./format";

export type TimelineItem = { type: "stamp"; key: string; label: string } | { type: "message"; key: string; message: Message };

const GAP_MS = 15 * 60_000;

// A chat's messages with a time label before the first one, after a gap of 15 minutes or more, and at each new day.
export function timeline(messages: Message[], now: Date = new Date()): TimelineItem[] {
  const items: TimelineItem[] = [];
  let previous: Date | null = null;
  for (const message of messages) {
    const at = new Date(message.createdAt);
    if (!previous || at.getTime() - previous.getTime() >= GAP_MS || at.toDateString() !== previous.toDateString()) {
      items.push({ type: "stamp", key: `stamp-${message.id}`, label: stamp(at, now) });
    }
    items.push({ type: "message", key: message.id, message });
    previous = at;
  }
  return items;
}
```

Create `packages/ui/components/chat/markdown.tsx`:

```tsx
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

// Bot replies are Markdown. Raw HTML never renders, unsafe link targets are dropped, and links open in the
// default browser (the desktop app sends every new window there).
export function MarkdownText({ text }: { text: string }) {
  return (
    <div className="markdown">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {text}
      </Markdown>
    </div>
  );
}
```

Create `packages/ui/components/chat/bubbles.tsx`:

```tsx
import { Paperclip } from "lucide-react";
import type { Message } from "@tjommie/protocol";
import { MarkdownText } from "./markdown";

// While the text streams it stays plain, with a caret; Markdown renders once it's done.
export function BotBubble({ message }: { message: Message }) {
  const streaming = message.status === "streaming";
  return (
    <div data-author="bot" className="w-fit max-w-[600px] rounded-[18px] bg-bubble px-4 py-[14px] text-[14px] leading-[21px] text-text">
      {streaming ? (
        <p className="whitespace-pre-wrap [overflow-wrap:anywhere]">
          {message.body}
          <span aria-hidden className="ml-0.5 inline-block h-4 w-0.5 translate-y-[3px] bg-text" />
        </p>
      ) : (
        <MarkdownText text={message.body} />
      )}
    </div>
  );
}

// The person's own text stays exactly as typed.
export function UserBubble({ message, files }: { message: Message; files: string[] }) {
  return (
    <div className="flex max-w-[600px] flex-col items-end gap-1 self-end">
      {files.length > 0 && (
        <div className="flex flex-wrap justify-end gap-1.5">
          {files.map((name) => (
            <span key={name} className="inline-flex h-7 max-w-[240px] items-center gap-1.5 rounded-full bg-pill px-2.5 text-[12px] text-text">
              <Paperclip className="size-3 shrink-0 text-icon" aria-hidden />
              <span className="truncate">{name}</span>
            </span>
          ))}
        </div>
      )}
      <div data-author="you" className="whitespace-pre-wrap rounded-[18px] bg-user-bubble px-4 py-3 text-[14px] leading-[21px] text-user-text [overflow-wrap:anywhere]">
        {message.body}
      </div>
      {message.status === "queued" && <span className="text-[11px] text-caption">Queued</span>}
    </div>
  );
}
```

Create `packages/ui/components/chat/notices.tsx`:

```tsx
"use client";

import { useState } from "react";
import { CircleAlert, Clock, Hourglass, Info, RotateCcw, Square, TimerOff } from "lucide-react";
import type { ErrorData, RoutineLabelData } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { Pill } from "../pill";

export function Timestamp({ label }: { label: string }) {
  return <div className="self-stretch py-2 text-center text-[12px] leading-[18px] text-caption">{label}</div>;
}

// The daemon's notices: "Stopped.", "Stopped at the time limit (3 hours).", a usage limit, and the like.
export function Notice({ text }: { text: string }) {
  const Icon = /time limit/i.test(text) ? TimerOff : /usage limit/i.test(text) ? Hourglass : /^stopped/i.test(text) ? Square : Info;
  return (
    <div className="flex items-center justify-center gap-2 self-stretch py-1 text-[12px] leading-[18px] text-secondary">
      <Icon className="size-3.5 shrink-0" aria-hidden />
      <span>{text}</span>
    </div>
  );
}

// An error, with Retry when the daemon can run that work again.
export function ErrorNotice({ text, retry }: { text: string; retry: ErrorData | null }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  async function run(target: ErrorData): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await daemon().call("chat.retry", { messageId: target.retryMessageId, botId: target.retryBotId });
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="flex w-full max-w-[600px] items-center gap-3 rounded-[12px] border border-card-stroke bg-card px-4 py-3">
      <CircleAlert className="size-4 shrink-0 text-secondary" aria-hidden />
      <div className="min-w-0 flex-1 text-[13px] leading-[18px]">
        <p className="text-text">{text}</p>
        {problem && <p className="text-danger">{problem}</p>}
      </div>
      {retry && (
        <Pill disabled={busy} onClick={() => void run(retry)}>
          <RotateCcw className="size-3.5" aria-hidden />
          Retry
        </Pill>
      )}
    </div>
  );
}

// "Routine: <name>" above a run. A missed or skipped run says why, with a gray clock.
export function RoutineLabel({ text, data }: { text: string; data: RoutineLabelData }) {
  const quiet = data.status === "missed" || data.status === "skipped";
  const why = data.status === "missed" ? "Missed, the PC was off" : data.status === "skipped" ? `Skipped: ${(data.note ?? "").replace(/\.$/, "")}` : null;
  return (
    <div className="flex items-center gap-2 py-1 text-[12px] font-medium leading-[18px] text-caption">
      <Clock className={cn("size-3.5 shrink-0", quiet ? "text-secondary" : "text-working")} aria-hidden />
      <span>
        {text}
        {why ? ` · ${why}` : ""}
      </span>
    </div>
  );
}
```

- [ ] **Step 4: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 3 Markdown tests and the timeline test.

Run: `npm run typecheck` and `npm run lint`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/ui/components/chat packages/ui/lib/timeline.ts packages/ui/lib/timeline.test.ts
git commit -m "Add chat bubbles, safe Markdown, notices and time labels

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Chat: cards and rows

**Files:**
- Create: `packages/ui/components/chat/activity-row.tsx`, `packages/ui/components/chat/handoff-row.tsx`, `packages/ui/components/chat/file-card.tsx`, `packages/ui/components/chat/approval-card.tsx`, `packages/ui/components/chat/message-view.tsx`, `packages/ui/lib/approval.ts`
- Test: `packages/ui/lib/approval.test.ts`

**Interfaces:**
- Consumes: Task 8's pieces; `messageData` and the payload types from `@tjommie/protocol`.
- Produces:
  - `ActivityRow({ summary, items })`: collapsed "N steps: …" with a chevron, or expanded with one row per step.
  - `HandoffRow({ body, data, bots, conversationId })` in five variants:
    - pending (spinner, "waiting for a reply");
    - replied (message count);
    - failed ("no reply");
    - incoming, in the asked bot's chat ("<bot> asked: …", one avatar);
    - room answer ("Answered in the Team room").

    Long text truncates; the trailing part stays visible. (Plan 1E makes the rows open the exchange view.)
  - `FileCard({ file })`: name, size, and a button that shows the file in Explorer.
  - `ApprovalCard({ summary, data, bot })`:
    - Tool cards have Allow once, Always allow, Deny and the "Always allow saves" note. Routine proposals have Add routine and Dismiss.
    - An answered card collapses to one line.
  - `approvalDetail(detail): string`.
  - `MessageView({ message, bot, bots, conversationId })` draws any chat message by its kind.

- [ ] **Step 1: Write the failing test**

Create `packages/ui/lib/approval.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { approvalDetail } from "./approval";

describe("approvalDetail", () => {
  it("picks the path, command or address out of Claude's tool input", () => {
    const path = "C:\\Users\\tyrel\\Documents\\Job applications\\CV-Sabenza-Angular.pdf";
    expect(approvalDetail(JSON.stringify({ file_path: path, content: "..." }, null, 2))).toBe(path);
    expect(approvalDetail(JSON.stringify({ command: "npm test", description: "Run the tests" }, null, 2))).toBe("npm test");
    expect(approvalDetail(JSON.stringify({ url: "https://pnet.co.za/jobs" }, null, 2))).toBe("https://pnet.co.za/jobs");
  });

  it("keeps Codex's command text on one line, and shortens very long details", () => {
    expect(approvalDetail("Set-Content -LiteralPath 'notes.txt'\n  -Value 'hi'")).toBe("Set-Content -LiteralPath 'notes.txt' -Value 'hi'");
    expect(approvalDetail("x".repeat(500))).toHaveLength(240);
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npx vitest run packages/ui/lib/approval.test.ts`
Expected: FAIL. `./approval` can't be found.

- [ ] **Step 3: Write the detail helper and the rows**

Create `packages/ui/lib/approval.ts`:

```ts
const oneLine = (text: string): string => text.replace(/\s+/g, " ").trim().slice(0, 240);

// A short line for what an approval card asks about. Claude sends the tool's input as JSON; Codex sends the
// command or folder as text.
export function approvalDetail(detail: string): string {
  try {
    const input: unknown = JSON.parse(detail);
    if (input && typeof input === "object" && !Array.isArray(input)) {
      const record = input as Record<string, unknown>;
      for (const key of ["file_path", "path", "notebook_path", "command", "url", "pattern", "query"]) {
        const value = record[key];
        if (typeof value === "string") return oneLine(value);
      }
      const firstText = Object.values(record).find((value): value is string => typeof value === "string");
      if (firstText) return oneLine(firstText);
    }
  } catch {
    // not JSON: already plain text
  }
  return oneLine(detail);
}
```

Create `packages/ui/components/chat/activity-row.tsx`:

```tsx
"use client";

import { useState } from "react";
import { Check, ChevronDown, ChevronUp, ListChecks, Loader, X } from "lucide-react";
import type { ActivityItem } from "@tjommie/protocol";

// One line per turn of tool work; it expands to one row per step.
export function ActivityRow({ summary, items }: { summary: string; items: ActivityItem[] }) {
  const [open, setOpen] = useState(false);
  const header = (
    <button
      type="button"
      onClick={() => setOpen(!open)}
      aria-expanded={open}
      className="inline-flex h-8 max-w-full items-center gap-2 rounded-[10px] px-3 text-left text-[13px] text-secondary hover:text-body"
    >
      <ListChecks className="size-3.5 shrink-0" aria-hidden />
      <span className="min-w-0 truncate">{summary}</span>
      {open ? <ChevronUp className="size-3.5 shrink-0" aria-hidden /> : <ChevronDown className="size-3.5 shrink-0" aria-hidden />}
    </button>
  );
  if (!open) return header;
  return (
    <div className="w-full max-w-[600px] rounded-[12px] border border-card-stroke py-1">
      {header}
      <ul className="px-3 pb-2">
        {items.map((item) => (
          <li key={item.id} className="flex items-center gap-2 py-1 text-[13px] text-body">
            {item.status === "done" ? (
              <Check className="size-3.5 shrink-0 text-working" aria-label="Done" />
            ) : item.status === "running" ? (
              <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-label="Running" />
            ) : (
              <X className="size-3.5 shrink-0 text-secondary" aria-label="Failed" />
            )}
            <span className="min-w-0 truncate">{item.summary}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
```

Create `packages/ui/components/chat/handoff-row.tsx`:

```tsx
import { ArrowRight, Loader, Users } from "lucide-react";
import type { Bot, HandoffData } from "@tjommie/protocol";
import { DELETED_BOT } from "@/lib/labels";
import { BotAvatar, MissingAvatar } from "../bot-avatar";

const ROW = "flex max-w-full items-center gap-2 rounded-[10px] border border-divider px-3 py-2 text-[13px]";

function Avatar({ bot }: { bot: Bot | undefined }) {
  return bot ? <BotAvatar color={bot.color} shape={bot.shape} size={18} className="shrink-0" /> : <MissingAvatar size={18} />;
}

// A one-line row for work with another bot: an exchange, or an answer given in the Team room.
export function HandoffRow({ body, data, bots, conversationId }: { body: string; data: HandoffData; bots: Bot[]; conversationId: string }) {
  if ("roomMessageId" in data) {
    return (
      <div className={ROW}>
        <Users className="size-3.5 shrink-0 text-secondary" aria-hidden />
        <span className="min-w-0 truncate text-body">Answered in the Team room</span>
      </div>
    );
  }
  const from = bots.find((bot) => bot.id === data.fromBotId);
  const to = bots.find((bot) => bot.id === data.toBotId);
  const incoming = conversationId === data.toBotId;
  const fromName = from?.name ?? DELETED_BOT;
  const text = incoming ? `${fromName} asked: ${body}` : `${fromName} asked ${to?.name ?? DELETED_BOT}: ${body}`;
  return (
    <div className={ROW}>
      <Avatar bot={from} />
      {!incoming && (
        <>
          <ArrowRight className="size-3 shrink-0 text-secondary" aria-hidden />
          <Avatar bot={to} />
        </>
      )}
      <span className="min-w-0 truncate text-body">{text}</span>
      <span className="flex shrink-0 items-center gap-1 text-caption">
        {data.status === "waiting" ? (
          <>
            <Loader className="size-3 animate-spin text-icon" aria-hidden />
            waiting for a reply
          </>
        ) : data.status === "failed" ? (
          "no reply"
        ) : (
          `${data.messages} ${data.messages === 1 ? "message" : "messages"}`
        )}
      </span>
    </div>
  );
}
```

Create `packages/ui/components/chat/file-card.tsx`:

```tsx
"use client";

import { Download, FileText } from "lucide-react";
import type { FileData } from "@tjommie/protocol";
import { desktop } from "@/lib/desktop";
import { fileSize } from "@/lib/format";

// A file a bot shared. The button shows it selected in Explorer; the app never runs it.
export function FileCard({ file }: { file: FileData }) {
  return (
    <div className="flex w-[264px] items-center gap-3 rounded-[12px] bg-bubble p-3">
      <span className="flex size-8 shrink-0 items-center justify-center rounded-[8px] bg-file-tile">
        <FileText className="size-4 text-danger" aria-hidden />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-semibold leading-[18px] text-title">{file.name}</span>
        <span className="block text-[12px] leading-[18px] text-caption">{fileSize(file.size)}</span>
      </span>
      <button
        type="button"
        aria-label={`Show ${file.name} in its folder`}
        title="Show in folder"
        onClick={() => desktop()?.showFile(file.path)}
        className="press flex size-8 shrink-0 items-center justify-center rounded-full text-icon hover:bg-pill"
      >
        <Download className="size-4" aria-hidden />
      </button>
    </div>
  );
}
```

Create `packages/ui/components/chat/approval-card.tsx`:

```tsx
"use client";

import { useState } from "react";
import { Check, CircleAlert, X } from "lucide-react";
import type { ApprovalData, ApprovalDecision, Bot } from "@tjommie/protocol";
import { approvalDetail } from "@/lib/approval";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { Pill } from "../pill";

// Answers given in this window, so an answered card can say "Allowed once" or "Always allowed".
const decided = new Map<string, ApprovalDecision>();

// A "Waiting for you" card. The buttons lock after one click; the card updates when the daemon records it.
export function ApprovalCard({ summary, data, bot }: { summary: string; data: ApprovalData; bot: Bot | undefined }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  if (data.status !== "pending") return <AnsweredApproval summary={summary} data={data} />;
  const routine = data.kind === "routine";
  async function answer(decision: ApprovalDecision): Promise<void> {
    setBusy(true);
    setProblem(null);
    decided.set(data.approvalId, decision);
    try {
      await daemon().call("approvals.respond", { approvalId: data.approvalId, decision });
    } catch (error) {
      decided.delete(data.approvalId);
      setBusy(false);
      setProblem(errorText(error));
    }
  }
  return (
    <div role="group" aria-label="Waiting for you" className="w-full max-w-[600px] rounded-[14px] border border-waiting-stroke bg-waiting-fill p-4">
      <p className="flex items-center gap-2 text-[12px] font-semibold leading-[18px] text-accent">
        <CircleAlert className="size-3.5" aria-hidden />
        Waiting for you
      </p>
      <p className="mt-2 text-[14px] leading-[21px] text-text">{summary}</p>
      <p className={routine ? "mt-1 line-clamp-2 text-[12px] leading-[18px] text-secondary" : "mt-1 truncate text-[12px] leading-[18px] text-secondary"}>
        {routine ? `It will: ${data.detail}` : `${data.tool} · ${approvalDetail(data.detail)}`}
      </p>
      <div className="mt-3 flex flex-wrap gap-2">
        {routine ? (
          <>
            <Pill primary disabled={busy} onClick={() => void answer("once")}>
              Add routine
            </Pill>
            <Pill disabled={busy} onClick={() => void answer("deny")}>
              Dismiss
            </Pill>
          </>
        ) : (
          <>
            <Pill primary disabled={busy} onClick={() => void answer("once")}>
              Allow once
            </Pill>
            <Pill disabled={busy} onClick={() => void answer("always")}>
              Always allow
            </Pill>
            <Pill disabled={busy} onClick={() => void answer("deny")}>
              Deny
            </Pill>
          </>
        )}
      </div>
      {data.kind !== "routine" && data.rule && bot && (
        <p className="mt-2 text-[12px] leading-[18px] text-placeholder">
          Always allow saves: {data.rule}, for {bot.name} only
        </p>
      )}
      {problem && <p className="mt-2 text-[12px] leading-[18px] text-danger">{problem}</p>}
    </div>
  );
}

function AnsweredApproval({ summary, data }: { summary: string; data: ApprovalData }) {
  const allowed = data.status === "allowed";
  const decision = decided.get(data.approvalId);
  const label =
    data.kind === "routine"
      ? allowed
        ? "Routine added"
        : "Dismissed"
      : !allowed
        ? "Denied"
        : decision === "always"
          ? "Always allowed"
          : decision === "once"
            ? "Allowed once"
            : "Allowed";
  return (
    <div className="inline-flex max-w-full items-center gap-2 rounded-[10px] border border-divider bg-canvas px-3 py-2 text-[13px] text-secondary">
      {allowed ? <Check className="size-3.5 shrink-0 text-working" aria-hidden /> : <X className="size-3.5 shrink-0" aria-hidden />}
      <span className="min-w-0 truncate">
        {label}: {summary}
      </span>
    </div>
  );
}
```

Create `packages/ui/components/chat/message-view.tsx`:

```tsx
import { MessageData, type Bot, type Message } from "@tjommie/protocol";
import { ActivityRow } from "./activity-row";
import { ApprovalCard } from "./approval-card";
import { BotBubble, UserBubble } from "./bubbles";
import { FileCard } from "./file-card";
import { HandoffRow } from "./handoff-row";
import { ErrorNotice, Notice, RoutineLabel } from "./notices";

// One chat message, drawn by its kind. A payload that doesn't match its schema shows as plain text.
// (safeParse is messageData() without the throw; React's lint rules reject JSX inside try/catch.)
export function MessageView({ message, bot, bots, conversationId }: { message: Message; bot: Bot | undefined; bots: Bot[]; conversationId: string }) {
  const parsed = MessageData.safeParse(message);
  if (!parsed.success) return <Notice text={message.body} />;
  const payload = parsed.data;
  switch (payload.kind) {
    case "text": {
      if (message.author !== "you") return <BotBubble message={message} />;
      const files = payload.data && !("exchangeId" in payload.data) ? payload.data.files : [];
      return <UserBubble message={message} files={files} />;
    }
    case "activity":
      return <ActivityRow summary={message.body} items={payload.data.items} />;
    case "approval":
      return <ApprovalCard summary={message.body} data={payload.data} bot={bot} />;
    case "notice":
      return <Notice text={message.body} />;
    case "error":
      return <ErrorNotice text={message.body} retry={payload.data} />;
    case "handoff":
      return <HandoffRow body={message.body} data={payload.data} bots={bots} conversationId={conversationId} />;
    case "routine":
      return <RoutineLabel text={message.body} data={payload.data} />;
    case "file":
      return <FileCard file={payload.data} />;
  }
}
```

- [ ] **Step 4: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 2 approval tests.

Run: `npm run typecheck` and `npm run lint`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add packages/ui/components/chat packages/ui/lib/approval.ts packages/ui/lib/approval.test.ts
git commit -m "Add approval cards, activity and handoff rows, and file cards

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Chat: the composer

**Files:**
- Create: `packages/ui/lib/attachments.ts`, `packages/ui/components/chat/composer.tsx`
- Test: `packages/ui/lib/attachments.test.ts`

**Interfaces:**
- Consumes: `daemon()` (Task 6), `desktop()` (Task 5), `errorText` (Task 5).
- Produces:
  - `addFiles(current, incoming): string[] | null`, `fileName(path)` and `MAX_FILES` (20, the daemon's limit for `chat.send`).
  - `Composer({ bot })`. Its textarea is labeled "Message <bot>". Its round button is labeled "Send" or "Stop":
    - disabled Send while the field is empty and the bot isn't working;
    - white Send once there's text, which steers a working bot;
    - white Stop while the field is empty and the bot is working or waiting.

    Enter sends and Shift+Enter adds a line. Files come from the + button or a drop onto the composer.

- [ ] **Step 1: Write the failing test**

Create `packages/ui/lib/attachments.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { addFiles, fileName, MAX_FILES } from "./attachments";

describe("attachments", () => {
  it("adds picked or dropped files once each, and skips dropped items that aren't files", () => {
    expect(addFiles(["C:\\a.pdf"], ["C:\\b.pdf", "C:\\a.pdf", ""])).toEqual(["C:\\a.pdf", "C:\\b.pdf"]);
  });

  it("refuses to go over the daemon's limit of 20 files", () => {
    const twenty = Array.from({ length: MAX_FILES }, (_, index) => `C:\\f${index}.txt`);
    expect(addFiles(twenty.slice(0, 19), ["C:\\f19.txt"])).toEqual(twenty);
    expect(addFiles(twenty, ["C:\\extra.txt"])).toBeNull();
  });

  it("shows a file's name from its Windows path", () => {
    expect(fileName("C:\\Users\\tyrel\\Documents\\CV final.pdf")).toBe("CV final.pdf");
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npx vitest run packages/ui/lib/attachments.test.ts`
Expected: FAIL. `./attachments` can't be found.

- [ ] **Step 3: Write the helpers**

Create `packages/ui/lib/attachments.ts`:

```ts
export const MAX_FILES = 20; // chat.send takes at most 20 files

// Adds files the person picked or dropped, once each. webUtils.getPathForFile gives "" for a dropped item
// that isn't a file on disk. Returns null when the message would carry too many files.
export function addFiles(current: string[], incoming: string[]): string[] | null {
  const next = [...current];
  for (const path of incoming) if (path && !next.includes(path)) next.push(path);
  return next.length > MAX_FILES ? null : next;
}

export const fileName = (path: string): string => path.split(/[\\/]/).pop() ?? path;
```

- [ ] **Step 4: Write the composer**

Create `packages/ui/components/chat/composer.tsx`:

```tsx
"use client";

import { useState, type DragEvent, type KeyboardEvent } from "react";
import { ArrowUp, Paperclip, Plus, Square, X } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { addFiles, fileName, MAX_FILES } from "@/lib/attachments";
import { daemon } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { errorText } from "@/lib/labels";
import { cn } from "@/lib/utils";

// The message box. Its round button sends (a working bot gets the message right away, as steering), or stops
// a working bot while the field is empty. Files go with a message: chat.send needs text, so Send stays off
// until there is some. Sending is done all day, so nothing here animates.
export function Composer({ bot }: { bot: Bot }) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const hasText = text.trim().length > 0;
  const stop = !hasText && (bot.state === "working" || bot.state === "waiting");

  function attach(paths: string[]): void {
    const next = addFiles(files, paths);
    if (next) {
      setFiles(next);
      setProblem(null);
    } else {
      setProblem(`A message can carry up to ${MAX_FILES} files.`);
    }
  }

  async function run(action: () => Promise<unknown>, done?: () => void): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
      done?.();
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  function submit(): void {
    if (busy) return;
    if (hasText) {
      void run(
        () => daemon().call("chat.send", { botId: bot.id, text, files }),
        () => {
          setText("");
          setFiles([]);
        },
      );
    } else if (stop) {
      void run(() => daemon().call("chat.stop", { botId: bot.id }));
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (hasText) submit(); // Enter never stops a bot; Stop takes a click
  }

  async function pick(): Promise<void> {
    const picked = await desktop()?.pickFiles();
    if (picked?.length) attach(picked);
  }

  function onDrop(event: DragEvent<HTMLFormElement>): void {
    event.preventDefault();
    const api = desktop();
    if (api) attach(Array.from(event.dataTransfer.files, (file) => api.pathForFile(file)));
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
      onDragOver={(event) => event.preventDefault()}
      onDrop={onDrop}
      className="shrink-0 px-4 pb-4"
    >
      {files.length > 0 && (
        <ul aria-label="Attached files" className="mb-2 flex flex-wrap gap-1.5">
          {files.map((path) => (
            <li key={path} className="inline-flex h-7 max-w-[240px] items-center gap-1.5 rounded-full bg-pill pl-2.5 pr-1 text-[12px] text-text">
              <Paperclip className="size-3 shrink-0 text-icon" aria-hidden />
              <span className="truncate">{fileName(path)}</span>
              <button
                type="button"
                aria-label={`Remove ${fileName(path)}`}
                onClick={() => setFiles(files.filter((file) => file !== path))}
                className="flex size-5 shrink-0 items-center justify-center rounded-full text-icon hover:text-text"
              >
                <X className="size-3" aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex items-end gap-2 rounded-[22px] border border-composer-stroke bg-composer py-[5px] pl-2 pr-[5px]">
        <button
          type="button"
          aria-label="Attach files"
          title="Attach files"
          onClick={() => void pick()}
          className="mb-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-disabled text-icon hover:text-text"
        >
          <Plus className="size-4" aria-hidden />
        </button>
        {/* The caret shows focus here: WF11 draws the typing composer without a ring. */}
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
          rows={1}
          aria-label={`Message ${bot.name}`}
          placeholder={`Message ${bot.name}`}
          className="max-h-[200px] min-h-8 flex-1 resize-none bg-transparent py-[5.5px] text-[14px] leading-[21px] text-text outline-none [field-sizing:content] placeholder:text-placeholder"
        />
        <button
          type="submit"
          disabled={busy || (!hasText && !stop)}
          aria-label={stop ? "Stop" : "Send"}
          title={stop ? "Stop" : "Send"}
          className={cn(
            "flex size-8 shrink-0 items-center justify-center rounded-full",
            hasText || stop ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder",
          )}
        >
          {stop ? <Square className="size-3.5" aria-hidden /> : <ArrowUp className="size-4" aria-hidden />}
        </button>
      </div>
      {problem && (
        <p role="alert" className="mt-1.5 px-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </form>
  );
}
```

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 3 attachment tests.

Run: `npm run typecheck` and `npm run lint`
Expected: no errors. The composer shows up in Task 12's chat screen, where it's checked by hand.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/lib/attachments.ts packages/ui/lib/attachments.test.ts packages/ui/components/chat/composer.tsx
git commit -m "Add the chat composer with Send, Stop and attachments

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Chat: the bot panel

**Files:**
- Create: `packages/ui/lib/activity.ts`, `packages/ui/components/chat/bot-panel.tsx`
- Test: `packages/ui/lib/activity.test.ts`

**Interfaces:**
- Consumes: Tasks 5 to 7; `ActivityData` from `@tjommie/protocol`.
- Produces:
  - `recentActivity(messages, limit = 8): RecentStep[]`, where `RecentStep = { key, summary, status, at }`.
  - `BotPanel({ bot, messages, onHide })`: the 460 px right panel (WF1 Phase 1). It holds:
    - a top bar with Bot settings (a link to `/bots/settings/?bot=<id>`, which plan 1E adds) and "Hide the bot panel";
    - the summary card (model, autonomy, folders with Open);
    - the Browser card with Show window, only for bots with the browser on;
    - Routines, and Recent activity.

    Routines gets its Add link in plan 1E, with the routine editor.

- [ ] **Step 1: Write the failing test**

Create `packages/ui/lib/activity.test.ts`:

```ts
import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { recentActivity } from "./activity";

type Step = { id: string; summary: string; status: "running" | "done" | "failed" };
const activity = (id: string, createdAt: string, steps: Step[]): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind: "activity",
  body: `${steps.length} steps`,
  data: { items: steps.map((step) => ({ ...step, name: "Tool" })) },
  status: "done",
  createdAt,
});
const text = (id: string): Message => ({ id, conversationId: "b1", author: "b1", kind: "text", body: "Hi", data: null, status: "done", createdAt: "2026-09-27T09:30:00.000Z" });

describe("recentActivity", () => {
  it("lists the newest steps first, each with its turn's time, up to the limit", () => {
    const messages = [
      activity("a1", "2026-09-27T09:15:00.000Z", [{ id: "s1", summary: "Searched pnet.co.za for Full Stack roles", status: "done" }]),
      text("t1"),
      activity("a2", "2026-09-27T09:31:00.000Z", [
        { id: "s2", summary: "Read listing PN-48213", status: "done" },
        { id: "s3", summary: "Filling the Apply form on pnet.co.za", status: "running" },
      ]),
    ];
    expect(recentActivity(messages).map((step) => [step.summary, step.status, step.at])).toEqual([
      ["Filling the Apply form on pnet.co.za", "running", "2026-09-27T09:31:00.000Z"],
      ["Read listing PN-48213", "done", "2026-09-27T09:31:00.000Z"],
      ["Searched pnet.co.za for Full Stack roles", "done", "2026-09-27T09:15:00.000Z"],
    ]);
    expect(recentActivity(messages, 2)).toHaveLength(2);
  });

  it("skips activity rows whose payload doesn't match", () => {
    expect(recentActivity([{ ...text("x"), kind: "activity", data: { steps: 3 } }])).toEqual([]);
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `npx vitest run packages/ui/lib/activity.test.ts`
Expected: FAIL. `./activity` can't be found.

- [ ] **Step 3: Write recentActivity**

Create `packages/ui/lib/activity.ts`:

```ts
import { ActivityData, type ActivityItem, type Message } from "@tjommie/protocol";

export type RecentStep = { key: string; summary: string; status: ActivityItem["status"]; at: string };

// The bot panel's Recent activity: the newest steps from the chat's activity rows, newest first. Each step
// shows the time its turn's row started.
export function recentActivity(messages: Message[], limit = 8): RecentStep[] {
  const steps: RecentStep[] = [];
  for (let index = messages.length - 1; index >= 0 && steps.length < limit; index--) {
    const message = messages[index];
    if (message.kind !== "activity") continue;
    const parsed = ActivityData.safeParse(message.data);
    if (!parsed.success) continue;
    for (const item of [...parsed.data.items].reverse()) {
      if (steps.length === limit) break;
      steps.push({ key: `${message.id}:${item.id}`, summary: item.summary, status: item.status, at: message.createdAt });
    }
  }
  return steps;
}
```

- [ ] **Step 4: Write the panel**

Create `packages/ui/components/chat/bot-panel.tsx`:

```tsx
"use client";

import Link from "next/link";
import { useMemo, useState, type ReactNode } from "react";
import { Check, ChevronsRight, Clock, Folder, Loader, Pause, Settings, Shield, Sparkles, X, type LucideIcon } from "lucide-react";
import type { Bot, Message } from "@tjommie/protocol";
import { recentActivity } from "@/lib/activity";
import { daemon } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { clock, routineTitle, schedule } from "@/lib/format";
import { AUTONOMY_LINE, errorText, modelLabel, stateLabel } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { IconButton } from "../icon-button";

// The right panel beside a bot's chat (WF1 Phase 1). 16 px between cards, 24 px above a section label.
export function BotPanel({ bot, messages, onHide }: { bot: Bot; messages: Message[]; onHide: () => void }) {
  const allRoutines = useApp((state) => state.routines);
  const routines = useMemo(() => allRoutines.filter((routine) => routine.botId === bot.id), [allRoutines, bot.id]);
  const steps = useMemo(() => recentActivity(messages), [messages]);
  return (
    <aside aria-label={`About ${bot.name}`} className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      {/* Windows draws the caption buttons over this bar's right end. */}
      <div className="drag flex h-11 shrink-0 items-center gap-1 px-3">
        <Link
          href={`/bots/settings/?bot=${encodeURIComponent(bot.id)}`}
          aria-label="Bot settings"
          title="Bot settings"
          className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text"
        >
          <Settings className="size-4" aria-hidden />
        </Link>
        <IconButton label="Hide the bot panel" onClick={onHide}>
          <ChevronsRight aria-hidden />
        </IconButton>
      </div>
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-4">
        <Summary bot={bot} />
        {bot.browser && <BrowserCard bot={bot} />}
        <SectionLabel>Routines</SectionLabel>
        {routines.length === 0 ? (
          <p className="text-[13px] leading-[18px] text-secondary">No routines yet.</p>
        ) : (
          <ul className="flex flex-col gap-3">
            {routines.map((routine) => (
              <li key={routine.id} className="flex items-start gap-2.5">
                {routine.enabled ? <Clock className="mt-0.5 size-4 shrink-0 text-working" aria-hidden /> : <Pause className="mt-0.5 size-4 shrink-0 text-secondary" aria-hidden />}
                <div className="min-w-0">
                  <p className="truncate text-[13px] leading-[18px] text-title">{routineTitle(routine.name, routine.timeLimitMinutes)}</p>
                  <p className="text-[12px] leading-[18px] text-secondary">{routine.enabled ? schedule(routine.days, routine.times) : "Paused"}</p>
                </div>
              </li>
            ))}
          </ul>
        )}
        <SectionLabel>Recent activity</SectionLabel>
        {steps.length === 0 ? (
          <p className="text-[13px] leading-[18px] text-secondary">No recent activity.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {steps.map((step) => (
              <li key={step.key} className="flex items-center gap-2.5 text-[13px] leading-[18px]">
                {step.status === "running" ? (
                  <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-label="Running" />
                ) : step.status === "done" ? (
                  <Check className="size-3.5 shrink-0 text-icon" aria-label="Done" />
                ) : (
                  <X className="size-3.5 shrink-0 text-secondary" aria-label="Failed" />
                )}
                <span className="min-w-0 flex-1 truncate text-body">{step.summary}</span>
                <span className="shrink-0 text-[12px] text-caption">{step.status === "running" ? "now" : clock(step.at)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </aside>
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return <h3 className="mb-2 mt-6 text-[13px] font-semibold leading-[18px] text-label">{children}</h3>;
}

function Row({ icon: Icon, children, action }: { icon: LucideIcon; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex items-start gap-2">
      <Icon className="mt-0.5 size-3.5 shrink-0 text-icon" aria-hidden />
      <span className="min-w-0 flex-1">{children}</span>
      {action}
    </div>
  );
}

function OpenFolder({ path }: { path: string }) {
  return (
    <button type="button" onClick={() => void desktop()?.openFolder(path)} className="shrink-0 text-accent hover:underline">
      Open
    </button>
  );
}

function Summary({ bot }: { bot: Bot }) {
  return (
    <section className="mt-3 rounded-[12px] bg-card p-4">
      <div className="flex items-center gap-3">
        <BotAvatar color={bot.color} shape={bot.shape} size={40} className="shrink-0" />
        <h2 className="min-w-0 flex-1 truncate text-[17px] font-semibold text-title">{bot.name}</h2>
        {bot.state !== "idle" && (
          <span className="flex shrink-0 items-center gap-1.5 rounded-full bg-pill px-2.5 py-1 text-[12px] font-medium text-text">
            {bot.state === "working" && <Loader className="size-3 animate-spin" aria-hidden />}
            {stateLabel(bot)}
          </span>
        )}
      </div>
      <div className="mt-3 flex flex-col gap-2 pl-[52px] text-[13px] leading-[18px] text-body">
        <Row icon={Sparkles}>{modelLabel(bot)}</Row>
        <Row icon={Shield}>{AUTONOMY_LINE[bot.autonomy]}</Row>
        <Row icon={Folder} action={<OpenFolder path={bot.homeFolder} />}>
          Home folder
        </Row>
        {bot.projectFolder && (
          <Row icon={Folder} action={<OpenFolder path={bot.projectFolder} />}>
            <span className="block truncate">Project folder: {bot.projectFolder}</span>
          </Row>
        )}
      </div>
    </section>
  );
}

function BrowserCard({ bot }: { bot: Bot }) {
  const [problem, setProblem] = useState<string | null>(null);
  async function show(): Promise<void> {
    setProblem(null);
    try {
      await daemon().call("bots.showBrowser", { id: bot.id });
    } catch (error) {
      setProblem(errorText(error));
    }
  }
  return (
    <section className="mt-4 flex gap-3 rounded-[12px] bg-card p-3">
      <ChromeLogo />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-[13px] font-semibold leading-[18px] text-title">Browser</h3>
          <button type="button" onClick={() => void show()} className="shrink-0 text-[13px] leading-[18px] text-accent hover:underline">
            Show window
          </button>
        </div>
        <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">Its own Chrome window, separate from yours. Sites you sign in to there stay signed in.</p>
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
    </section>
  );
}

// The Chrome logo in its brand colors: an approved exception to the token palette.
function ChromeLogo() {
  return (
    <svg viewBox="0 0 48 48" width={20} height={20} aria-hidden="true" className="mt-0.5 shrink-0">
      <path d="M24 24 6.68 14A20 20 0 0 1 41.32 14Z" fill="#E33B2E" />
      <path d="M24 24 41.32 14A20 20 0 0 1 24 44Z" fill="#FBC116" />
      <path d="M24 24 24 44A20 20 0 0 1 6.68 14Z" fill="#229342" />
      <circle cx="24" cy="24" r="9.5" fill="#FFFFFF" />
      <circle cx="24" cy="24" r="7.5" fill="#1A73E8" />
    </svg>
  );
}
```

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 2 activity tests.

Run: `npm run typecheck` and `npm run lint`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/lib/activity.ts packages/ui/lib/activity.test.ts packages/ui/components/chat/bot-panel.tsx
git commit -m "Add the bot panel: summary, browser, routines and recent activity

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Chat: the chat screen

**Files:**
- Create: `packages/ui/app/chat/page.tsx`, `packages/ui/components/chat/chat-screen.tsx`, `packages/ui/components/chat/chat-header.tsx`, `packages/ui/components/chat/message-list.tsx`, `packages/ui/components/chat/setup-banner.tsx`

**Interfaces:**
- Consumes: Tasks 5 to 11; `Menu` from `@base-ui/react/menu`.
- Produces:
  - `/chat/?bot=<id>`, the page the rail and notifications open. An unknown or deleted bot goes back to `/`.
  - `ChatScreen({ botId })` is made of:
    - `ChatHeader`: the chip with the state in words, the bot panel toggle, and More (Pause or Resume, Show browser window, Open home folder, Bot settings);
    - `SetupBanner`, for Needs setup;
    - `MessageList`, or the empty chat;
    - `Composer`;
    - `BotPanel`.
  - The panel's open or closed choice is kept in `localStorage` under `tjommie.panel`.

How the list scrolls:
- It follows the newest message while the person is at the bottom, and jumps there when they send one.
- It loads the page before the oldest loaded message when the person nears the top. The messages they were reading stay put: the list turns off the browser's own scroll anchoring and moves by the height added above.

- [ ] **Step 1: Write the header and the setup banner**

Create `packages/ui/components/chat/chat-header.tsx`:

```tsx
"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { Menu } from "@base-ui/react/menu";
import { Ellipsis, FolderOpen, Globe, Hourglass, PanelRight, Pause, Play, Plug, Settings } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { errorText, stateLabel } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { IconButton } from "../icon-button";

// Windows draws its caption buttons over the window's top-right 138 x 44 px. With the bot panel hidden they sit
// over this header, so its buttons move 154 px in. The chip stays centered, with room for the buttons both sides.
export function ChatHeader({ bot, panelOpen, onTogglePanel }: { bot: Bot; panelOpen: boolean; onTogglePanel: () => void }) {
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
      <div className={cn("flex h-10 min-w-0 items-center gap-2 rounded-full bg-chip px-3.5", panelOpen ? "max-w-[calc(100%-176px)]" : "max-w-[calc(100%-452px)]")}>
        <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
        <h1 className="min-w-0 truncate text-[15px] font-semibold text-title">{bot.name}</h1>
        <HeaderState bot={bot} />
      </div>
      <div className={cn("absolute top-3 flex items-center gap-2", panelOpen ? "right-4" : "right-[154px]")}>
        <IconButton label="Bot panel" active={panelOpen} onClick={onTogglePanel}>
          <PanelRight aria-hidden />
        </IconButton>
        <MoreMenu bot={bot} onProblem={setProblem} />
      </div>
      {problem && (
        <p role="alert" className="absolute left-1/2 top-full z-10 -translate-x-1/2 rounded-full bg-popover px-3 py-1.5 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </header>
  );
}

// The state in words. Working is green and Waiting blue, each with a dot; the others are gray with an icon.
function HeaderState({ bot }: { bot: Bot }) {
  if (bot.state === "idle") return null;
  const label = stateLabel(bot);
  if (bot.state === "working" || bot.state === "waiting") {
    return (
      <span className={cn("flex shrink-0 items-center gap-1.5 text-[13px]", bot.state === "working" ? "text-working" : "text-accent")}>
        <span aria-hidden className="size-1.5 rounded-full bg-current" />
        {label}
      </span>
    );
  }
  const Icon = bot.state === "limited" ? Hourglass : bot.state === "paused" ? Pause : Plug;
  return (
    <span className="flex shrink-0 items-center gap-1.5 text-[13px] text-secondary">
      <Icon className="size-3.5" aria-hidden />
      {label}
    </span>
  );
}

const ITEM = "flex h-9 cursor-default items-center gap-2.5 rounded-[8px] px-3 text-[13px] text-text outline-none data-[highlighted]:bg-highlight [&_svg]:size-4 [&_svg]:text-icon";

function MoreMenu({ bot, onProblem }: { bot: Bot; onProblem: (problem: string | null) => void }) {
  const router = useRouter();
  async function run(action: () => Promise<unknown>): Promise<void> {
    onProblem(null);
    try {
      await action();
    } catch (error) {
      onProblem(errorText(error));
    }
  }
  return (
    <Menu.Root>
      <Menu.Trigger aria-label="More" title="More" className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text data-[popup-open]:bg-selected">
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className="flex min-w-[220px] flex-col gap-1 rounded-[16px] border border-divider bg-popover p-1.5">
            <Menu.Item className={ITEM} onClick={() => void run(() => daemon().call("bots.pause", { id: bot.id, paused: !bot.paused }))}>
              {bot.paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {bot.paused ? "Resume bot" : "Pause bot"}
            </Menu.Item>
            {bot.browser && (
              <Menu.Item className={ITEM} onClick={() => void run(() => daemon().call("bots.showBrowser", { id: bot.id }))}>
                <Globe aria-hidden />
                Show browser window
              </Menu.Item>
            )}
            <Menu.Item className={ITEM} onClick={() => void run(async () => desktop()?.openFolder(bot.homeFolder))}>
              <FolderOpen aria-hidden />
              Open home folder
            </Menu.Item>
            <Menu.Item className={ITEM} onClick={() => router.push(`/bots/settings/?bot=${encodeURIComponent(bot.id)}`)}>
              <Settings aria-hidden />
              Bot settings
            </Menu.Item>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
```

Create `packages/ui/components/chat/setup-banner.tsx`:

```tsx
"use client";

import { useState } from "react";
import { Plug } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { Pill } from "../pill";

const SANDBOX = "Set up Codex's sandbox"; // the daemon's setupReason for a Codex bot at Own folder

// The Needs setup banner (WF11): waiting-card colors, a plug, what's missing, and the fix when there is one.
// When setup finishes, the daemon updates the bot and the banner goes away.
export function SetupBanner({ bot }: { bot: Bot }) {
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const reason = bot.setupReason ?? "";
  const signIn = reason.startsWith("Sign in to");
  const sandbox = reason === SANDBOX;
  const provider = bot.provider === "claude" ? "Claude" : "Codex";
  const text = signIn ? `${provider} isn't signed in for Tjommie yet.` : sandbox ? "Codex's sandbox isn't set up yet. Windows asks for admin permission once." : reason;

  async function fix(): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      if (signIn) {
        await daemon().call("setup.signIn", { provider: bot.provider });
        setNote(bot.provider === "claude" ? "Finish signing in in the window that opened." : "Finish signing in in your browser.");
      } else {
        await daemon().call("setup.prepareCodexSandbox", {});
      }
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-6 mt-2 flex max-w-[600px] items-center gap-3 rounded-[12px] border border-waiting-stroke bg-waiting-fill px-4 py-3">
      <Plug className="size-4 shrink-0 text-accent" aria-hidden />
      <div className="min-w-0 flex-1 text-[14px] leading-[21px] text-text">
        <p>{text}</p>
        {note && <p className="text-[12px] leading-[18px] text-secondary">{note}</p>}
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
      {(signIn || sandbox) && (
        <Pill primary disabled={busy} onClick={() => void fix()}>
          {signIn ? "Sign in" : "Set up"}
        </Pill>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Write the message list**

Create `packages/ui/components/chat/message-list.tsx`:

```tsx
"use client";

import { useEffect, useLayoutEffect, useMemo, useRef } from "react";
import type { Bot, Message } from "@tjommie/protocol";
import { timeline } from "@/lib/timeline";
import { MessageView } from "./message-view";
import { Timestamp } from "./notices";

type Props = { messages: Message[]; hasMore: boolean; bot: Bot; bots: Bot[]; conversationId: string; onOlder: () => Promise<void> };

// Follows the newest message while the person is at the bottom. Near the top it loads the older page, and the
// messages in view stay put: the browser's own scroll anchoring is off, and the list moves by what was added.
export function MessageList({ messages, hasMore, bot, bots, conversationId, onOlder }: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const firstShown = useRef<{ id: string; top: number } | null>(null);
  const lastId = useRef<string | undefined>(undefined);
  const items = useMemo(() => timeline(messages), [messages]);

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const last = messages.at(-1);
    const justSent = last !== undefined && last.id !== lastId.current && last.author === "you";
    lastId.current = last?.id;
    const before = firstShown.current;
    const kept = before && before.id !== messages[0]?.id ? element.querySelector<HTMLElement>(`[data-message="${before.id}"]`) : null;
    if (before && kept) {
      element.scrollTop += kept.offsetTop - before.top; // an older page was added above
    } else if (pinned.current || justSent) {
      element.scrollTop = element.scrollHeight; // your own message always comes into view
      pinned.current = true;
    }
    const first = element.querySelector<HTMLElement>("[data-message]");
    firstShown.current = first?.dataset.message ? { id: first.dataset.message, top: first.offsetTop } : null;
  }, [messages]);

  useEffect(() => {
    const root = scroller.current;
    const target = sentinel.current;
    if (!root || !target || !hasMore) return;
    let loading = false;
    const observer = new IntersectionObserver(
      (entries) => {
        if (loading || !entries.some((entry) => entry.isIntersecting)) return;
        loading = true;
        void onOlder()
          .catch(() => {})
          .finally(() => {
            loading = false;
          });
      },
      { root, rootMargin: "300px 0px 0px 0px" },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [hasMore, onOlder]);

  return (
    <div
      ref={scroller}
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
      }}
      className="relative min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]"
    >
      <div ref={sentinel} aria-hidden className="h-px" />
      <ol className="flex flex-col gap-3 px-6 pb-4 pt-2">
        {items.map((item) =>
          item.type === "stamp" ? (
            <li key={item.key}>
              <Timestamp label={item.label} />
            </li>
          ) : (
            <li key={item.key} data-message={item.message.id} className="flex flex-col items-start">
              <MessageView message={item.message} bot={bot} bots={bots} conversationId={conversationId} />
            </li>
          ),
        )}
      </ol>
    </div>
  );
}
```

- [ ] **Step 3: Write the screen and the page**

Create `packages/ui/components/chat/chat-screen.tsx`:

```tsx
"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { Bot, Message } from "@tjommie/protocol";
import { botHistory, loadConversation, loadOlder } from "@/lib/connection";
import { AUTONOMY_HINT } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { BotPanel } from "./bot-panel";
import { ChatHeader } from "./chat-header";
import { Composer } from "./composer";
import { MessageList } from "./message-list";
import { SetupBanner } from "./setup-banner";

const PANEL_KEY = "tjommie.panel";
const NO_MESSAGES: Message[] = [];

// A bot's chat (WF1 Phase 1): the header, the setup banner, the messages, the composer and the bot panel.
export function ChatScreen({ botId }: { botId: string }) {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const bot = useMemo(() => bots.find((candidate) => candidate.id === botId), [bots, botId]);
  const conversation = useApp((state) => state.conversations[botId]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [panel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");

  // Loads the newest page when the chat opens, and again when a new connection clears it.
  useEffect(() => {
    if (ready && bot && !conversation) void loadConversation(botId, botHistory(botId));
  }, [ready, bot, conversation, botId]);
  // An unknown or deleted bot has no chat.
  useEffect(() => {
    if (!bot) router.replace("/");
  }, [bot, router]);
  const older = useCallback(() => loadOlder(botId, botHistory(botId)), [botId]);

  if (!bot) return null;
  const loaded = conversation?.status === "ready" ? conversation : null;
  const messages = loaded?.messages ?? NO_MESSAGES;
  const showPanel = (open: boolean): void => {
    localStorage.setItem(PANEL_KEY, open ? "open" : "closed");
    setPanel(open);
  };
  return (
    <div className="flex min-h-0 flex-1">
      <section aria-label={`Chat with ${bot.name}`} className="flex min-w-0 flex-1 flex-col">
        <ChatHeader bot={bot} panelOpen={panel} onTogglePanel={() => showPanel(!panel)} />
        {bot.state === "needs_setup" && <SetupBanner bot={bot} />}
        {!loaded ? (
          <div className="flex-1" />
        ) : messages.length === 0 ? (
          <EmptyChat bot={bot} />
        ) : (
          <MessageList messages={messages} hasMore={loaded.hasMore} bot={bot} bots={bots} conversationId={botId} onOlder={older} />
        )}
        <Composer bot={bot} />
      </section>
      {panel && <BotPanel bot={bot} messages={messages} onHide={() => showPanel(false)} />}
    </div>
  );
}

function EmptyChat({ bot }: { bot: Bot }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 px-8 text-center">
      <BotAvatar color={bot.color} shape={bot.shape} size={40} className="opacity-35" />
      <p className="max-w-[360px] text-[14px] leading-[21px] text-body">
        Say hi to {bot.name}. {AUTONOMY_HINT[bot.autonomy]}
      </p>
    </div>
  );
}
```

Create `packages/ui/app/chat/page.tsx`:

```tsx
"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { ChatScreen } from "@/components/chat/chat-screen";

function Chat() {
  const botId = useSearchParams().get("bot") ?? "";
  // key: each bot's chat starts fresh, with its own draft and attachments.
  return <ChatScreen key={botId} botId={botId} />;
}

// A static export can't have /chat/<id>, so the bot comes as ?bot=<id>. useSearchParams needs the Suspense.
export default function ChatPage() {
  return (
    <Suspense>
      <Chat />
    </Suspense>
  );
}
```

- [ ] **Step 4: Run the checks**

Run: `npm test`, `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. `packages/ui/out/chat/index.html` exists.

- [ ] **Step 5: Check it by hand**

Use development mode (Task 4 Step 4) with a Claude bot. Create one with `npm run chat -w @tjommie/daemon -- "Pnet apply" claude`, then quit the terminal chat. Check each of these against `docs/wireframes/wf1-phase1-main-chat-dark.png` and `wf11-chat-states-dark.png`:

1. The rail opens the chat. The header chip shows the avatar and the name. The empty chat says "Say hi to Pnet apply." and the autonomy hint.
2. Send "Write three short lines about Johannesburg, as a Markdown list."
   - Your bubble shows at once.
   - The reply streams as plain text with a caret, then becomes a list.
   - The header shows "Working" in green while it runs.
   - The rail's green dot comes and goes with it.
3. Send "Use the Bash tool to run: node -e \"setTimeout(() => {}, 30000)\"". While it runs, the round button is a white Stop. Click it: "Stopped." appears.
4. Attach a file with + and another by dragging it onto the composer. Both show as chips, and × removes one. Send stays disabled until you type. After sending, your bubble lists the file names.
5. The panel toggle hides the panel. The header's buttons move clear of the caption buttons, and the choice survives a reload (Ctrl+R).
6. More opens the menu:
   - Pause bot makes the rail avatar 50% and the chip "Paused".
   - Resume bot brings it back.
   - Open home folder opens Explorer.
7. Older pages:
   - Set `PAGE` in `lib/connection.ts` to 4 for a moment, and reload.
   - In a chat with ten or more messages, scroll to the top. Older messages load, and the view doesn't jump.
   - Send a message while scrolled up: the list jumps to it.
   - Set `PAGE` back to 50.
8. Theme:
   - In the window's developer tools (Ctrl+Shift+I), run `localStorage.setItem("tjommie.theme", "light")`, then reload.
   - The app is light, and the caption buttons are dark on light.
   - Set it back to `"dark"`.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/app/chat packages/ui/components/chat
git commit -m "Add the chat screen: header, messages, setup banner and bot panel

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: End-to-end with real bots

**Files:**
- Create: `packages/desktop/src/desktop.real.test.ts`
- Modify: `package.json` (root; npm adds the dependency), `README.md`

**Interfaces:**
- Consumes:
  - the whole app;
  - `realAccountsDir` and `tempRoot` from `packages/daemon/src/testing.ts`;
  - the preload bridge's `connection()`, which the test uses to reach the daemon the app started.
- Produces: `npm run test:real` drives the real desktop app. It builds the UI, launches Electron in production mode with a disposable data root, and chats with real Claude bots.

- [ ] **Step 1: Add Playwright's Electron driver**

Run: `npm install -D playwright-core@^1.63.0`
Expected: the root `package.json` lists `playwright-core` under `devDependencies`. No browser downloads: the tests drive Electron's own Chromium.

- [ ] **Step 2: Write the tests**

Create `packages/desktop/src/desktop.real.test.ts`:

```ts
import { execFileSync, execSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connect, type Client } from "@tjommie/client";
import type { Bot, BotInput } from "@tjommie/protocol";
import { _electron, type ElectronApplication, type Page } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { realAccountsDir, tempRoot } from "../../daemon/src/testing.ts";

// The real desktop app: Electron, the daemon it starts, the UI's static export and real Claude bots. It uses the
// signed-in account folders (npm run signin -w @tjommie/daemon) with a disposable data root and Electron profile.
const repo = resolve(import.meta.dirname, "..", "..", "..");
const electronPath = createRequire(import.meta.url)("electron") as string;
const root = tempRoot();
const userData = mkdtempSync(join(tmpdir(), "tjommie-electron-"));

type Bridge = { tjommie: { connection(): Promise<{ ok: true; port: number; token: string } | { ok: false; error: string }> } };

let electronApp: ElectronApplication | undefined;
let page: Page;
let client: Client | undefined;
let quit = false;

beforeAll(async () => {
  execSync("npm run build", { cwd: repo, stdio: "inherit" }); // the static export the app serves
  const env = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined && entry[0] !== "TJOMMIE_DEV"));
  electronApp = await _electron.launch({
    executablePath: electronPath,
    args: [resolve(import.meta.dirname, "..")],
    env: {
      ...env,
      TJOMMIE_DESKTOP: "production",
      TJOMMIE_HOME: root,
      TJOMMIE_PORT: "0",
      TJOMMIE_ACCOUNTS_DIR: realAccountsDir(),
      TJOMMIE_USER_DATA: userData,
      TJOMMIE_BROWSER_HEADLESS: "1",
    },
  });
  page = await electronApp.firstWindow();
  await page.getByText("No bots yet.").waitFor({ timeout: 120_000 });
  const { port, token } = await daemonConnection();
  client = await connect(port, token);
});

afterAll(async () => {
  await client?.close().catch(() => {});
  if (electronApp && !quit) await electronApp.close().catch(() => {});
  for (const folder of [root, userData]) await remove(folder);
});

// The connection the app handed its UI, read through the same bridge.
async function daemonConnection(): Promise<{ port: number; token: string }> {
  const result = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.connection());
  if (!result.ok) throw new Error(result.error);
  return { port: result.port, token: result.token };
}

function daemonClient(): Client {
  if (!client) throw new Error("The test couldn't reach the app's daemon.");
  return client;
}

function runningApp(): ElectronApplication {
  if (!electronApp) throw new Error("The desktop app didn't start.");
  return electronApp;
}

const claudeBot = (name: string, extra: Partial<BotInput> = {}): Promise<Bot> =>
  daemonClient().call("bots.create", {
    name,
    color: "green",
    shape: "pill",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...extra,
  });

async function openChat(name: string): Promise<void> {
  await page.getByRole("link", { name: new RegExp(`^${name},`) }).click();
  await page.getByRole("heading", { level: 1, name }).waitFor();
}

async function send(name: string, text: string): Promise<void> {
  const box = page.getByRole("textbox", { name: `Message ${name}` });
  await box.fill(text);
  await box.press("Enter");
  await page.locator('[data-author="you"]', { hasText: text }).waitFor();
}

const botReply = (text: string) => page.locator('[data-author="bot"]', { hasText: text });

function listeningPid(port: number): number {
  const output = execFileSync("powershell.exe", ["-NoProfile", "-Command", `(Get-NetTCPConnection -LocalPort ${port} -State Listen).OwningProcess`], { encoding: "utf8" });
  return Number(output.trim().split(/\s+/)[0]);
}

function refused(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      done(false);
    });
    socket.once("error", () => done(true));
  });
}

async function eventually<T>(check: () => Promise<T | undefined>, timeoutMs = 60_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check().catch(() => undefined);
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`Timed out after ${timeoutMs} ms`);
    await new Promise((done) => setTimeout(done, 500));
  }
}

// Windows can hold a file for a moment after its process exits.
async function remove(folder: string): Promise<void> {
  for (let attempt = 0; attempt < 20 && existsSync(folder); attempt++) {
    try {
      rmSync(folder, { recursive: true, force: true });
    } catch {
      await new Promise((done) => setTimeout(done, 500));
    }
  }
}

describe("the desktop app with real bots", () => {
  it("sends a message and shows the reply", async () => {
    await claudeBot("Echo");
    await openChat("Echo");
    await send("Echo", "Reply with exactly: PONG");
    await botReply("PONG").waitFor({ timeout: 180_000 });
  });

  it("asks before a change and carries on after Allow once", async () => {
    const bot = await claudeBot("Careful", { autonomy: "ask" });
    await openChat("Careful");
    await send("Careful", "Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const card = page.getByRole("group", { name: "Waiting for you" });
    await card.waitFor({ timeout: 180_000 });
    await card.getByRole("button", { name: "Allow once" }).click();
    await page.getByText(/^Allowed once: /).waitFor();
    await botReply("DONE").waitFor({ timeout: 180_000 });
    expect(existsSync(join(bot.homeFolder, "hello.txt"))).toBe(true);
  });

  it("stops a bot that's working", async () => {
    await claudeBot("Sleeper", { autonomy: "full" });
    await openChat("Sleeper");
    await send("Sleeper", `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('done'), 60000)". Then tell me what it printed.`);
    await page.getByRole("button", { name: /^\d+ steps?: / }).waitFor({ timeout: 180_000 }); // the command is running
    await page.getByRole("button", { name: "Stop" }).click();
    await page.getByText("Stopped.", { exact: true }).waitFor({ timeout: 30_000 });
    await page.getByRole("link", { name: "Sleeper, Idle" }).waitFor({ timeout: 30_000 });
  });

  it("reconnects after the daemon dies", async () => {
    const before = await daemonConnection();
    const pid = listeningPid(before.port);
    process.kill(pid);
    // The app starts a new daemon with a new token, and hands the UI the new connection.
    const after = await eventually(async () => {
      const next = await daemonConnection();
      return next.token === before.token ? undefined : next;
    });
    expect(listeningPid(after.port)).not.toBe(pid);
    await page.getByText("Reconnecting to Tjommie").waitFor({ state: "hidden", timeout: 60_000 });
    // The UI's calls and events now go to the new daemon.
    await openChat("Echo");
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Pause bot" }).click();
    await page.getByRole("link", { name: "Echo, Paused" }).waitFor();
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Resume bot" }).click();
    await page.getByRole("link", { name: "Echo, Idle" }).waitFor();
  });

  it("quitting Tjommie stops its daemon", async () => {
    const { port } = await daemonConnection();
    const closed = runningApp().waitForEvent("close");
    await runningApp()
      .evaluate(({ app }) => app.quit())
      .catch(() => {}); // the app may exit before it answers
    await closed;
    quit = true;
    expect(await refused(port)).toBe(true);
  });
});
```

- [ ] **Step 3: Run them**

Run: `npm run typecheck`
Expected: no errors.

Run: `npx vitest run --mode real packages/desktop/src/desktop.real.test.ts`
Expected: PASS, 5 tests, in a few minutes. If `beforeAll` times out on "No bots yet.", read `logs\desktop.log` in the test's data root: it holds the daemon's own reason.

- [ ] **Step 4: Update the README**

In `README.md`, replace this line in the command list:

```
npm run desktop    # Electron window showing that UI (keep npm run dev running)
```

with:

```
npm run desktop    # the desktop app, on that UI and the development daemon (keep both running)
```

Then add this paragraph after the one that starts "The installed desktop app starts the daemon with `--token-stdin`.":

```markdown
The desktop app has two modes. By default it reads the development daemon's `daemon-token` file and shows the UI from `http://localhost:3000`, so both must be running. With `$env:TJOMMIE_DESKTOP = 'production'` it starts its own daemon with `--token-stdin` and serves the static export in `packages/ui/out` (run `npm run build` first), the way the installed app will. Closing the window keeps Tjommie running in the tray; Quit Tjommie in the tray menu stops the daemon, and with it the bots and routines. `npm run test:real` also drives the desktop app in production mode.
```

- [ ] **Step 5: Run everything**

Run: `npm test`, `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass.

Run: `npm run test:real`
Expected: PASS, the daemon's real tests plus the 5 desktop tests.

- [ ] **Step 6: Commit**

```bash
git add package.json package-lock.json packages/desktop/src/desktop.real.test.ts README.md
git commit -m "Drive the real desktop app with real bots in the end-to-end tests

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Not measured yet

No code in this plan was compiled or run while writing it. Each task's typecheck, lint and tests are the first check. These points are the likeliest to need a change on first contact; the implementer settles each one and records any change in that task's commit:
- **Playwright and Electron:** that `playwright-core` 1.63's `_electron.launch` starts Electron 44 with an ES module `.ts` main file. If it doesn't, launch through `npm run desktop` instead, with the environment passed through.
- **The UI's WebSocket from `app://tjommie`:** the scheme is registered as secure, and a secure page may still open `ws://127.0.0.1`, because Chromium treats loopback as trustworthy. If Chromium blocks it as mixed content, drop `secure: true` from the scheme's privileges and rerun Task 13.
- **Next's static export:** that it requests page data at paths `uiFile` maps (Task 3's test fixes the shapes it knows). The first production run shows any 404 in `desktop.log`, and the fix goes in `uiFile` and its test.
- **Base UI:** that `Menu.Trigger` sets `data-popup-open` while open (it's only a style).
- **Style:** the exact Tailwind classes against the wireframes. Plan 1E's screenshot checks compare the running app with every frame and fix what's off.