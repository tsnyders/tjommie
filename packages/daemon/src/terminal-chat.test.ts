import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  Approval, Bot, ClientMessage, Message, PROTOCOL_VERSION, TEAM_ROOM_ID, events, methods,
  type EventData, type EventName,
} from "@tjommie/protocol";
import { afterEach, expect, it } from "vitest";
import { WebSocketServer, type WebSocket } from "ws";
import type { ClientEvent } from "./client.ts";

const workspace = fileURLToPath(new URL("../../../", import.meta.url));
const script = fileURLToPath(new URL("../scripts/chat.ts", import.meta.url));
const cache = join(workspace, "node_modules", ".cache");
const timestamp = "2026-09-26T10:00:00.000Z";
const tester = Bot.parse({
  id: "tester", name: "Tester", color: "blue", shape: "circle", instructions: "", provider: "claude",
  model: "haiku", autonomy: "folder", homeFolder: "unused", projectFolder: null, browser: false,
  paused: false, canMessage: [], state: "idle", limitedUntil: null, createdAt: timestamp,
});
const writer = { ...tester, id: "writer", name: "Writer" };
const message = (patch: Partial<Message> = {}): Message => Message.parse({
  id: "message", conversationId: tester.id, author: tester.id, kind: "text", body: "Hello",
  data: null, status: "done", createdAt: timestamp, ...patch,
});
const approval = (patch: Partial<Approval> = {}): Approval => Approval.parse({
  id: "proposal", botId: tester.id, kind: "routine", tool: "add_routine", summary: "Add a routine: News",
  detail: "Weekdays at 08:00", rule: null, payload: null, status: "pending", messageId: "approval-message",
  createdAt: timestamp, ...patch,
});

type Request = Extract<ClientMessage, { type: "req" }>;
type Fixture = {
  root: string;
  server: WebSocketServer;
  child?: ChildProcessWithoutNullStreams;
  closed?: Promise<number | null>;
};
const fixtures: Fixture[] = [];

afterEach(async () => {
  for (const fixture of fixtures.splice(0)) {
    if (fixture.child && fixture.child.exitCode === null && fixture.child.signalCode === null) fixture.child.kill();
    for (const socket of fixture.server.clients) socket.terminate();
    await Promise.all([
      fixture.closed && waitForExit(fixture.closed, () => "Terminal fixture cleanup"),
      new Promise<void>((resolve) => fixture.server.close(() => resolve())),
    ]);
    rmSync(fixture.root, { recursive: true, force: true });
  }
});

async function eventually(check: () => boolean, diagnostic: () => string): Promise<void> {
  const deadline = Date.now() + 5_000;
  while (!check()) {
    if (Date.now() >= deadline) throw new Error(`Terminal chat did not reach the expected state. ${diagnostic()}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

async function waitForExit(closed: Promise<number | null>, diagnostic: () => string): Promise<number | null> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      closed,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`Terminal chat did not exit within 5 seconds. ${diagnostic()}`)), 5_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function terminal(options: { beforeWelcome?: { code: number; reason: string }; setupEvents?: ClientEvent[] } = {}) {
  const { beforeWelcome, setupEvents = [] } = options;
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "terminal-chat-"));
  writeFileSync(join(root, "daemon-token"), "terminal-test-token");
  const server = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  const fixture: Fixture = { root, server };
  fixtures.push(fixture);
  const requests: Request[] = [];
  let socket: WebSocket | undefined;
  let hello: ClientMessage | undefined;
  server.on("connection", (connected) => {
    socket = connected;
    connected.on("message", (raw) => {
      const request = ClientMessage.parse(JSON.parse(String(raw)));
      if (request.type === "hello") {
        hello = request;
        if (beforeWelcome) {
          if (beforeWelcome.code === 1005) connected.close();
          else connected.close(beforeWelcome.code, beforeWelcome.reason);
        } else connected.send(JSON.stringify({ type: "welcome", protocol: PROTOCOL_VERSION }));
        return;
      }
      requests.push(request);
      let result: unknown;
      switch (request.method) {
        case "bots.list":
          methods["bots.list"].params.parse(request.params);
          for (const event of setupEvents) {
            connected.send(JSON.stringify({ ...event, data: events[event.event].parse(event.data) }));
          }
          result = methods["bots.list"].result.parse([tester, writer]);
          break;
        case "chat.send": {
          const params = methods["chat.send"].params.parse(request.params);
          result = methods["chat.send"].result.parse(message({ author: "you", body: params.text }));
          break;
        }
        case "room.post": {
          const params = methods["room.post"].params.parse(request.params);
          result = methods["room.post"].result.parse(message({ conversationId: TEAM_ROOM_ID, author: "you", body: params.text }));
          break;
        }
        case "chat.stop":
          methods["chat.stop"].params.parse(request.params);
          result = methods["chat.stop"].result.parse({});
          break;
        case "approvals.respond": {
          const params = methods["approvals.respond"].params.parse(request.params);
          result = methods["approvals.respond"].result.parse(approval({ id: params.approvalId, status: params.decision === "deny" ? "denied" : "allowed" }));
          break;
        }
        default:
          connected.send(JSON.stringify({ type: "res", id: request.id, ok: false, error: { code: "unknown_method", message: request.method } }));
          return;
      }
      connected.send(JSON.stringify({ type: "res", id: request.id, ok: true, result }));
    });
  });
  await new Promise<void>((resolve) => server.once("listening", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Expected a TCP listener");
  const child = spawn(process.execPath, ["--disable-warning=ExperimentalWarning", script], {
    cwd: workspace,
    env: { ...process.env, TJOMMIE_HOME: root, TJOMMIE_PORT: String(address.port) },
    windowsHide: true,
    stdio: "pipe",
  });
  fixture.child = child;
  let output = "";
  let spawnError = "";
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => { output += chunk; });
  child.stderr.setEncoding("utf8").on("data", (chunk: string) => { output += chunk; });
  child.on("error", (error) => { spawnError = error.message; });
  const closed = new Promise<number | null>((resolve) => child.once("close", resolve));
  fixture.closed = closed;
  const diagnostic = () => `${spawnError} Exit: ${child.exitCode}; output: ${output}`;
  return {
    requests,
    waitForExit: () => waitForExit(closed, diagnostic),
    output: () => output,
    hello: () => hello,
    waitFor: (text: string) => eventually(() => output.includes(text), diagnostic),
    waitForRequests: (count: number) => eventually(() => requests.length >= count, diagnostic),
    write: (text: string) => { child.stdin.write(text); },
    endInput: () => { child.stdin.end(); },
    emit: <E extends EventName>(event: E, data: EventData<E>) => {
      if (!socket) throw new Error("No connected terminal");
      socket.send(JSON.stringify({ type: "event", event, data: events[event].parse(data) }));
    },
    sendFrame: (frame: string) => {
      if (!socket) throw new Error("No connected terminal");
      socket.send(frame);
    },
    close: (code = 1000, reason = "Test completed") => {
      if (!socket) throw new Error("No connected terminal");
      if (code === 1005) socket.close();
      else socket.close(code, reason);
    },
  };
}

it("prints Team room posts, bot names, local cards and streaming text", async () => {
  const chat = await terminal();
  await chat.waitFor("Chatting with Tester (claude, folder, idle).");
  expect(chat.output()).toContain("/room <text> posts in the Team room, /stop interrupts, Ctrl+C quits.");
  expect(chat.hello()).toEqual({ type: "hello", protocol: PROTOCOL_VERSION, token: "terminal-test-token" });
  chat.emit("message.added", message({ conversationId: TEAM_ROOM_ID, author: writer.id, body: "Hello team", data: { exchangeId: "exchange", toBotId: tester.id } }));
  chat.emit("bot.updated", { ...writer, name: "Editor" });
  chat.emit("message.updated", message({ conversationId: TEAM_ROOM_ID, author: writer.id, body: "Renamed", data: { exchangeId: "exchange", toBotId: "unknown" } }));
  chat.emit("message.added", message({ conversationId: TEAM_ROOM_ID, author: "system", kind: "notice", body: "Room notice" }));
  chat.emit("message.added", message({ conversationId: TEAM_ROOM_ID, author: "system", kind: "error", body: "Room error" }));
  chat.emit("message.added", message({ conversationId: TEAM_ROOM_ID, author: "unknown", body: "Unknown author" }));
  for (const [index, toBotId] of [4, {}, "", tester.id].entries()) {
    chat.emit("message.added", message({ conversationId: TEAM_ROOM_ID, author: writer.id, body: `Untargeted ${index}`, data: { toBotId } }));
  }
  for (const patch of [
    { author: "you" }, { status: "streaming" as const }, { kind: "activity" as const },
  ]) chat.emit("message.added", message({ conversationId: TEAM_ROOM_ID, body: "Hidden room row", ...patch }));
  chat.emit("message.added", message({ conversationId: writer.id, kind: "notice", body: "Hidden other bot row" }));
  chat.emit("message.delta", { id: "stream", conversationId: writer.id, delta: "Hidden other bot delta" });
  chat.emit("message.delta", { id: "stream", conversationId: tester.id, delta: "Hello" });
  chat.emit("message.delta", { id: "stream", conversationId: tester.id, delta: " world" });
  chat.emit("message.updated", message({ id: "stream", body: "Hello world" }));
  chat.emit("message.added", message({ kind: "activity", body: "Looking up news" }));
  for (const kind of ["notice", "error", "handoff", "routine", "file"] as const) {
    chat.emit("message.added", message({ kind, body: `${kind} card` }));
  }
  chat.emit("approval.added", approval());
  chat.emit("approval.added", approval({ id: "tool-card", kind: "tool", tool: "Write", summary: "Write a file" }));
  chat.emit("approval.added", approval({ botId: writer.id, summary: "Hidden other approval" }));
  chat.emit("approval.resolved", approval({ status: "allowed" }));
  chat.emit("approval.resolved", approval({ id: "denied-card", status: "denied", summary: "Write declined" }));
  chat.emit("approval.resolved", approval({ botId: writer.id, status: "allowed", summary: "Hidden other resolved approval" }));
  chat.emit("notify", { kind: "routine_failed", routineId: "r", runId: "run", botId: writer.id, title: "Routine finished", body: "News checked" });
  chat.emit("notify", { kind: "limit", botId: "unknown", title: "Needs attention", body: "Unknown bot report" });
  chat.emit("notify", { kind: "approval", approvalId: "a", botId: tester.id, title: "Ready", body: "Your bot report" });
  chat.emit("settings.changed", { chainLimit: 5 });
  chat.emit("bot.deleted", { id: "Hidden deleted bot" });
  chat.emit("routine.deleted", { id: "Hidden deleted routine" });
  chat.emit("message.added", message({ kind: "notice", body: "Rendering complete" }));
  await chat.waitFor("(Rendering complete)");
  const output = chat.output();
  for (const line of [
    "  [Team room] Writer to Tester: Hello team",
    "  [Team room] Editor to a bot: Renamed",
    "  [Team room] Tjommie: Room notice",
    "  [Team room] Tjommie: Room error",
    "  [Team room] A bot: Unknown author",
    ...[0, 1, 2, 3].map((index) => `  [Team room] Editor: Untargeted ${index}`),
    "Hello world\n", "  [Looking up news]",
    ...["notice", "error", "handoff", "routine", "file"].map((kind) => `  (${kind} card)`),
    "Waiting for you: Add a routine: News\n  /allow proposal   /always proposal   /deny proposal",
    "Waiting for you: Write a file\n  /allow tool-card   /always tool-card   /deny tool-card",
    "  Approval proposal is allowed: Add a routine: News",
    "  Approval denied-card is denied: Write declined",
    "  [Editor] Routine finished: News checked",
    "  [A bot] Needs attention: Unknown bot report",
    "  [Tester] Ready: Your bot report",
  ]) expect(output).toContain(line);
  expect(output).not.toContain("Hidden");
  chat.close();
  expect(await chat.waitForExit()).toBe(0);
});

it("routes room posts, approval decisions, stop and ordinary chat through typed requests", async () => {
  const chat = await terminal();
  await chat.waitFor("Chatting with Tester");
  chat.write("/room @Writer hello team\n/allow once-card\n/always always-card\n/deny deny-card\n/stop\n  ordinary chat  \n\n");
  await chat.waitForRequests(7);
  expect(chat.requests.map(({ method, params }) => ({ method, params }))).toEqual([
    { method: "bots.list", params: {} },
    { method: "room.post", params: { text: "@Writer hello team" } },
    { method: "approvals.respond", params: { approvalId: "once-card", decision: "once" } },
    { method: "approvals.respond", params: { approvalId: "always-card", decision: "always" } },
    { method: "approvals.respond", params: { approvalId: "deny-card", decision: "deny" } },
    { method: "chat.stop", params: { botId: tester.id } },
    { method: "chat.send", params: { botId: tester.id, text: "  ordinary chat  " } },
  ]);
  chat.close();
  expect(await chat.waitForExit()).toBe(0);
});

it("prints the chat header before replaying events received during bot setup", async () => {
  const chat = await terminal({ setupEvents: [
    { type: "event", event: "bot.updated", data: { ...writer, name: "Editor" } },
    { type: "event", event: "message.added", data: message({ conversationId: TEAM_ROOM_ID, author: writer.id, body: "Setup room post", data: { exchangeId: "exchange", toBotId: tester.id } }) },
    { type: "event", event: "message.added", data: message({ kind: "notice", body: "Setup local notice" }) },
    { type: "event", event: "approval.added", data: approval({ summary: "Setup approval" }) },
    { type: "event", event: "notify", data: { kind: "approval", approvalId: "a", botId: writer.id, title: "Setup notification", body: "Setup complete" } },
  ] });
  await chat.waitFor("[Editor] Setup notification: Setup complete");
  const output = chat.output();
  const headerIndex = output.indexOf("Chatting with Tester");
  expect(headerIndex).toBeGreaterThanOrEqual(0);
  for (const row of [
    "[Team room] Editor to Tester: Setup room post", "(Setup local notice)", "Waiting for you: Setup approval",
    "[Editor] Setup notification: Setup complete",
  ]) {
    expect(output.indexOf(row)).toBeGreaterThan(headerIndex);
    expect(output.split(row)).toHaveLength(2);
  }
  chat.close();
  expect(await chat.waitForExit()).toBe(0);
});

it("prints usage for incomplete commands and continues accepting valid commands", async () => {
  const chat = await terminal();
  await chat.waitFor("Chatting with Tester");
  chat.write("/room\n/room   \n/allow\n/always\n/deny\n/stop\n");
  await chat.waitForRequests(2);
  await chat.waitFor("Usage: /deny <approval id>");
  for (const usage of [
    "Usage: /room <message>", "Usage: /allow <approval id>",
    "Usage: /always <approval id>", "Usage: /deny <approval id>",
  ]) expect(chat.output()).toContain(usage);
  expect(chat.requests.map(({ method, params }) => ({ method, params }))).toEqual([
    { method: "bots.list", params: {} },
    { method: "chat.stop", params: { botId: tester.id } },
  ]);
  chat.close();
  expect(await chat.waitForExit()).toBe(0);
});

it("closes the connection and exits normally when standard input ends", async () => {
  const chat = await terminal();
  await chat.waitFor("Chatting with Tester");
  chat.endInput();
  expect(await chat.waitForExit()).toBe(0);
  expect(chat.output()).toContain("Disconnected from the daemon.");
});

it("identifies an invalid server frame as a client read failure", async () => {
  const chat = await terminal();
  await chat.waitFor("Chatting with Tester");
  chat.sendFrame(JSON.stringify({ type: "event", event: "settings.changed", data: { chainLimit: "invalid" } }));
  expect(await chat.waitForExit()).toBe(1);
  expect(chat.output()).toContain("Connection closed with code 4400.");
  expect(chat.output()).toContain("The chat client could not read a message from the daemon.");
  expect(chat.output()).toContain("Client reason: Invalid server frame:");
  expect(chat.output()).not.toContain("The daemon rejected a message from the chat client.");
  expect(chat.output()).not.toContain("Daemon reason:");
});

for (const phase of ["before welcome", "after startup"] as const) {
  it.each([
    [4401, "Bad token"], [4003, "Protocol mismatch"], [4400, "Bad message"],
  ] as const)(`reports abnormal close %i ${phase} with its reason and exit 1`, async (code, reason) => {
    const chat = await terminal({ beforeWelcome: phase === "before welcome" ? { code, reason } : undefined });
    if (phase === "after startup") {
      await chat.waitFor("Chatting with Tester");
      chat.close(code, reason);
    }
    expect(await chat.waitForExit()).toBe(1);
    expect(chat.output()).toContain(`Connection closed with code ${code}.`);
    expect(chat.output()).toContain(`Daemon reason: ${reason}`);
    if (code === 4400) expect(chat.output()).toContain("The daemon rejected a message from the chat client.");
    expect(chat.output()).not.toContain("Client reason:");
    expect(chat.output()).not.toContain("Could not connect to the daemon");
  });

  it.each([1000, 1005])(`exits normally for close %i ${phase}`, async (code) => {
    const chat = await terminal({ beforeWelcome: phase === "before welcome" ? { code, reason: "Normal close" } : undefined });
    if (phase === "after startup") {
      await chat.waitFor("Chatting with Tester");
      chat.close(code, "Normal close");
    }
    expect(await chat.waitForExit()).toBe(0);
    expect(chat.output()).toContain("Disconnected from the daemon.");
  });
}
