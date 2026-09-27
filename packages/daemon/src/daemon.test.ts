import { existsSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { BotInput, BotPatch, ProviderSetup } from "@tjommie/protocol";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import WebSocket from "ws";
import { createBot, getStoredBot, setLimitedUntil } from "./bots.ts";
import { showBrowser } from "./browser.ts";
import { history, updateMessage } from "./chat.ts";
import { connect, type Client } from "@tjommie/client";
import { startDaemon, type Daemon } from "./daemon.ts";
import { newId, openDb, type Db } from "./db.ts";
import { dataPaths } from "./paths.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, getWorkItem, newChain, setStatus } from "./queue.ts";
import { addRun, createRoutine, getRun, listRuns } from "./routines.ts";
import { Scheduler } from "./scheduler.ts";
import * as serverModule from "./server.ts";
import { checkSetup, ensureAccounts, prepareCodexSandbox, signIn } from "./setup.ts";
import { protectCodexData, protectCodexDataAsync } from "./windows-acl.ts";
import { Team } from "./team.ts";
import { tempRoot, waitFor } from "./testing.ts";

const removal = vi.hoisted(() => ({
  failPath: "",
  beforeRemove: () => {},
  failTokenPath: "",
  tokenOptions: undefined as Parameters<typeof import("node:fs").rmSync>[1],
}));
vi.mock("node:fs", async (original) => {
  const fs = await original<typeof import("node:fs")>();
  return { ...fs, rmSync: (...args: Parameters<typeof fs.rmSync>) => {
    if (String(args[0]) === removal.failTokenPath) {
      removal.tokenOptions = args[1];
      throw Object.assign(new Error("Forced locked token file"), { code: "EBUSY" });
    }
    return fs.rmSync(...args);
  }, renameSync: (...args: Parameters<typeof fs.renameSync>) => {
    if (String(args[0]) === removal.failPath) {
      removal.beforeRemove();
      throw new Error("Forced locked folder");
    }
    return fs.renameSync(...args);
  } };
});
vi.mock("./setup.ts", async (original) => ({
  ...await original<typeof import("./setup.ts")>(), checkSetup: vi.fn(), signIn: vi.fn(), prepareCodexSandbox: vi.fn(),
}));
vi.mock("./windows-acl.ts", async (original) => ({
  ...await original<typeof import("./windows-acl.ts")>(), protectCodexData: vi.fn(), protectCodexDataAsync: vi.fn(),
}));
vi.mock("./browser.ts", async (original) => ({
  ...await original<typeof import("./browser.ts")>(), showBrowser: vi.fn(),
}));

class Session implements ProviderSession {
  listener: (event: ProviderEvent) => void = () => {};
  sent: string[] = [];
  closed = false;
  interruptions = 0;
  answers: string[] = [];
  async start(): Promise<void> {}
  async send(text: string): Promise<void> { this.sent.push(text); }
  async steer(): Promise<boolean> { return false; }
  async interrupt(): Promise<void> { this.interruptions++; this.emit({ type: "turn_done", ok: false, error: null }); }
  answer(id: string): void { this.answers.push(id); }
  close(): void { this.closed = true; }
  pid(): undefined { return undefined; }
  onEvent(listener: (event: ProviderEvent) => void): void { this.listener = listener; }
  emit(event: ProviderEvent): void { this.listener(event); }
  finish(text: string): void {
    this.emit({ type: "text", delta: text });
    this.emit({ type: "text_end" });
    this.emit({ type: "turn_done", ok: true, error: null });
  }
}

const setup = (signedIn = true, sandbox: "ready" | "notConfigured" | "updateRequired" | "unavailable" = "ready"): ProviderSetup[] => [
  { provider: "claude", installed: true, version: "test", signedIn, account: null, plan: null, error: null },
  { provider: "codex", installed: true, version: "test", signedIn, account: null, plan: null, error: null, sandbox },
];
const input = (name: string, extra: Partial<BotInput> = {}): BotInput => ({
  name, color: "green", shape: "pill", instructions: "Be terse", provider: "claude", model: "haiku", ...extra,
});
const daemons: Daemon[] = [];
const clients: Client[] = [];
const databases: Db[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(checkSetup).mockResolvedValue(setup());
  vi.mocked(protectCodexData).mockReturnValue({ status: "ready", ready: true, reason: null });
  vi.mocked(protectCodexDataAsync).mockResolvedValue({ status: "ready", ready: true, reason: null });
  vi.mocked(prepareCodexSandbox).mockResolvedValue(undefined);
  vi.mocked(showBrowser).mockResolvedValue(undefined);
  removal.failPath = "";
  removal.beforeRemove = () => {};
  removal.failTokenPath = "";
  removal.tokenOptions = undefined;
});
afterEach(async () => {
  for (const client of clients.splice(0)) await client.close();
  for (const daemon of daemons.splice(0)) await daemon.close();
  for (const db of databases.splice(0)) db.close();
  vi.restoreAllMocks();
});

async function harness(root = tempRoot(), auth: { mode: "development" } | { mode: "production"; token: string } = { mode: "development" }) {
  const sessions: { botId: string; options: StartOptions; session: Session }[] = [];
  const daemon = await startDaemon({ root, port: 0, auth, createSession: (bot, options) => {
    const session = new Session();
    sessions.push({ botId: bot.id, options, session });
    return session;
  } });
  daemons.push(daemon);
  const client = await connect(daemon.port, daemon.token);
  clients.push(client);
  const db = openDb(dataPaths(root).db);
  databases.push(db);
  return { daemon, client, db, sessions, root };
}

function appServer(options: StartOptions) {
  const server = options.mcpServers?.find((entry) => entry.name === "tjommie");
  if (!server || !("url" in server)) throw new Error("Missing Tjommie MCP server");
  return server;
}

async function tool(options: StartOptions, name: string, args: Record<string, unknown>) {
  const server = appServer(options);
  const response = await fetch(server.url, {
    method: "POST", headers: { ...server.headers, "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }),
  });
  expect(response.status).toBe(200);
  const result: unknown = await response.json();
  expect(result).not.toHaveProperty("error");
  expect(result).not.toHaveProperty("result.isError", true);
  return result;
}

it("wires settings, routine CRUD and the Team room through the WebSocket methods", async () => {
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Roomy"));
  expect(await client.call("settings.get", {})).toEqual({ chainLimit: 5 });
  expect(await client.call("settings.set", { chainLimit: 7 })).toEqual({ chainLimit: 7 });
  await client.call("room.post", { text: "Hello team" });
  expect(await client.call("room.history", {})).toMatchObject({ messages: [{ body: "Hello team" }], hasMore: false });
  const routine = await client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [1], times: ["08:00"] });
  expect(await client.call("routines.list", { botId: bot.id })).toEqual([routine]);
  expect(await client.call("routines.runs", { routineId: routine.id })).toEqual([]);
  expect(await client.call("routines.update", { id: routine.id, patch: { enabled: false } })).toMatchObject({ enabled: false });
  await client.call("routines.delete", { id: routine.id });
  expect(await client.call("routines.list", { botId: bot.id })).toEqual([]);
  await expect(client.call("routines.runs", { routineId: routine.id })).rejects.toThrow(/not_found/);
  await expect(client.call("routines.list", { botId: "missing" })).rejects.toThrow(/not_found/);
  expect(client.events.some((event) => event.event === "settings.changed")).toBe(true);
  expect(client.events.some((event) => event.event === "routine.deleted")).toBe(true);
});

it.each(["folder", "ask"] as const)("attaches session MCP servers and revokes their token when closing (%s)", async (autonomy) => {
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Browser", { browser: true, autonomy }));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  const first = sessions[0];
  expect(first.options.mcpServers?.map((entry) => [entry.name, entry.autoAllow])).toEqual([["tjommie", true], ["browser", autonomy !== "ask"]]);
  await tool(first.options, "list_bots", {});
  first.session.finish("Done");
  await client.call("bots.update", { id: bot.id, patch: { browser: false } });
  expect(first.session.closed).toBe(true);
  const server = appServer(first.options);
  expect((await fetch(server.url, { method: "POST", headers: server.headers, body: "{}" })).status).toBe(401);
  await client.call("chat.send", { botId: bot.id, text: "Again" });
  expect(sessions[1].options.mcpServers?.map((entry) => entry.name)).toEqual(["tjommie"]);
  expect(appServer(sessions[1].options).headers).not.toEqual(server.headers);
});

it("routes routine proposals to Team and tool approvals to the runner", async () => {
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Planner"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  await tool(sessions[0].options, "add_routine", { name: "Morning check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected approval card");
  await client.call("approvals.respond", { approvalId: card.data.id, decision: "once" });
  expect(await client.call("routines.list", { botId: bot.id })).toMatchObject([{ name: "Morning check", enabled: true }]);
  expect(sessions[0].session.answers).toEqual([]);
  sessions[0].session.emit({ type: "approval", id: "cli-approval", tool: "Write", summary: "Write file", detail: "File", rule: null });
  const approval = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "tool"));
  if (approval.event !== "approval.added") throw new Error("Expected tool approval");
  await client.call("approvals.respond", { approvalId: approval.data.id, decision: "once" });
  expect(sessions[0].session.answers).toEqual(["cli-approval"]);
});

it.each([false, true])("keeps a pending routine proposal waiting and clears it on answer (restart: %s)", async (restart) => {
  let h = await harness();
  const bot = await h.client.call("bots.create", input("Waiting planner"));
  await h.client.call("chat.send", { botId: bot.id, text: "Begin" });
  h.sessions[0].session.finish("Ready");
  await tool(h.sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => h.client.events.find((event) => event.event === "approval.added"));
  if (card.event !== "approval.added") throw new Error("Expected approval card");
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  expect(h.client.events).toContainEqual({ type: "event", event: "bot.updated", data: expect.objectContaining({ id: bot.id, state: "waiting" }) });
  expect(h.client.events).toContainEqual({ type: "event", event: "notify", data: expect.objectContaining({ kind: "approval", botId: bot.id, approvalId: card.data.id }) });
  if (restart) {
    await h.client.close();
    clients.splice(clients.indexOf(h.client), 1);
    await h.daemon.close();
    daemons.splice(daemons.indexOf(h.daemon), 1);
    h = await harness(h.root);
    expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
    expect((await h.client.call("chat.history", { botId: bot.id })).messages).toContainEqual(expect.objectContaining({ kind: "approval", data: expect.objectContaining({ approvalId: card.data.id, status: "pending" }) }));
  }
  await h.client.call("approvals.respond", { approvalId: card.data.id, decision: "deny" });
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
  expect(h.client.events).toContainEqual({ type: "event", event: "bot.updated", data: expect.objectContaining({ id: bot.id, state: "idle" }) });
});

it.each(["once", "deny"] as const)("answers a damaged routine card and clears Waiting through the API (%s)", async (decision) => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Damaged card"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  sessions[0].session.finish("Ready");
  await tool(sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected routine approval card");
  updateMessage(db, card.data.messageId, { data: { corrupt: "missing routine card fields" } });
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  const from = client.events.length;

  const resolved = await client.call("approvals.respond", { approvalId: card.data.id, decision });
  const status = decision === "deny" ? "denied" : "allowed";
  expect(resolved).toMatchObject({ id: card.data.id, status });
  expect((await client.call("chat.history", { botId: bot.id })).messages).toContainEqual(expect.objectContaining({
    id: card.data.messageId, data: { corrupt: "missing routine card fields", status },
  }));
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
  expect(client.events.slice(from)).toContainEqual({ type: "event", event: "bot.updated", data: expect.objectContaining({ id: bot.id, state: "idle" }) });
  expect(await client.call("routines.list", { botId: bot.id })).toHaveLength(decision === "deny" ? 0 : 1);
  await expect(client.call("approvals.respond", { approvalId: card.data.id, decision })).rejects.toThrow(/already_answered/);
});

it.each(["once", "deny"] as const)("runs queued room work while a routine proposal stays pending (%s)", async (decision) => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Planner"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  await tool(sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected routine approval card");
  const post = await client.call("room.post", { text: "@Planner Check the room while the proposal waits" });
  const queued = db.prepare("SELECT id FROM work_items WHERE message_id = ?").get(post.id) as { id: string };
  expect(getWorkItem(db, queued.id).status).toBe("queued");
  expect(sessions[0].session.sent).toHaveLength(1);

  sessions[0].session.finish("Ready");
  expect(getWorkItem(db, queued.id).status).toBe("running");
  expect(daemon.runner.currentWork(bot.id)?.id).toBe(queued.id);
  await waitFor(() => sessions[0].session.sent.length === 2, 2_000);
  expect(sessions[0].session.sent).toHaveLength(2);
  expect(sessions[0].session.sent[1]).toContain("Check the room while the proposal waits");
  expect(db.prepare("SELECT status FROM approvals WHERE id = ?").get(card.data.id)).toMatchObject({ status: "pending" });
  sessions[0].session.finish("Done");
  expect(getWorkItem(db, queued.id).status).toBe("done");
  expect(daemon.runner.currentWork(bot.id)).toBeNull();
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  expect((await client.call("chat.history", { botId: bot.id })).messages).toContainEqual(expect.objectContaining({ kind: "approval", data: expect.objectContaining({ approvalId: card.data.id, status: "pending" }) }));
  const resolved = await client.call("approvals.respond", { approvalId: card.data.id, decision });
  expect(resolved.status).toBe(decision === "deny" ? "denied" : "allowed");
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
});

it("starts recovered queued work while a routine proposal remains pending", async () => {
  let h = await harness();
  const bot = await h.client.call("bots.create", input("Planner"));
  await h.client.call("chat.send", { botId: bot.id, text: "Begin" });
  await tool(h.sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => h.client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected routine approval card");
  const post = await h.client.call("room.post", { text: "@Planner Pick this up after restart" });
  const queued = h.db.prepare("SELECT id FROM work_items WHERE message_id = ?").get(post.id) as { id: string };
  expect(getWorkItem(h.db, queued.id).status).toBe("queued");
  await h.client.close();
  clients.splice(clients.indexOf(h.client), 1);
  await h.daemon.close();
  daemons.splice(daemons.indexOf(h.daemon), 1);

  h = await harness(h.root);
  expect(getWorkItem(h.db, queued.id).status).toBe("running");
  await waitFor(() => h.sessions[0]?.session.sent.length, 2_000);
  expect(h.sessions[0].session.sent[0]).toContain("Pick this up after restart");
  expect(h.db.prepare("SELECT status FROM approvals WHERE id = ?").get(card.data.id)).toMatchObject({ status: "pending" });
  h.sessions[0].session.finish("Done after restart");
  expect(getWorkItem(h.db, queued.id).status).toBe("done");
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  await h.client.call("approvals.respond", { approvalId: card.data.id, decision: "deny" });
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
});

it("lists routines across bots and includes the bot on every run", async () => {
  const { daemon, client, sessions } = await harness();
  const bots = await Promise.all(["First planner", "Second planner"].map((name) => client.call("bots.create", input(name))));
  const routines = await Promise.all(bots.map((bot) => client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["08:48"] })));
  expect(await client.call("routines.list", {})).toEqual(expect.arrayContaining(routines));
  expect(await client.call("routines.list", { botId: bots[0].id })).toEqual([routines[0]]);
  daemon.scheduler.tick(Date.parse(routines[0].nextRunAt!));
  await waitFor(() => sessions.length === 2 && sessions.every(({ session }) => session.sent.length));
  for (const routine of routines) {
    expect(await client.call("routines.runs", { routineId: routine.id })).toMatchObject([{ botId: routine.botId }]);
  }
});

it("retries only the failed bot from a shared Team room post", async () => {
  const { daemon, client, db, sessions } = await harness();
  const first = await client.call("bots.create", input("First"));
  const second = await client.call("bots.create", input("Second"));
  const post = await client.call("room.post", { text: "@First @Second Please check" });
  await waitFor(() => sessions.length === 2 && sessions.every(({ session }) => session.sent.length));
  const failed = sessions.find(({ botId }) => botId === first.id)!.session;
  const successful = sessions.find(({ botId }) => botId === second.id)!.session;
  failed.emit({ type: "turn_done", ok: false, error: "First failed" });
  successful.finish("Second done");
  expect(history(db, "team", undefined, 50).find((message) => message.id === post.id)?.status).toBe("done");
  expect(history(db, "team", undefined, 50)).toContainEqual(expect.objectContaining({ kind: "error", data: { retryMessageId: post.id, retryBotId: first.id } }));
  const old = db.prepare("SELECT id FROM work_items WHERE bot_id = ?").get(first.id) as { id: string };
  await client.call("chat.retry", { messageId: post.id });
  await waitFor(() => failed.sent.length === 2);
  const retry = daemon.runner.currentWork(first.id)!;
  expect(retry).toMatchObject({ source: "room", conversationId: "team", messageId: post.id });
  expect(history(db, "team", undefined, 50).find((message) => message.id === post.id)?.status).toBe("done");
  expect(retry.chainId).not.toBe(getWorkItem(db, old.id).chainId);
  expect(successful.sent).toHaveLength(1);
  await expect(client.call("chat.retry", { messageId: post.id, botId: first.id })).rejects.toThrow(/bad_state/);
  failed.finish("First done");
});

it("requires a bot target when more than one room turn failed", async () => {
  const { client, sessions } = await harness();
  const first = await client.call("bots.create", input("First"));
  await client.call("bots.create", input("Second"));
  const post = await client.call("room.post", { text: "@First @Second Please check" });
  await waitFor(() => sessions.length === 2 && sessions.every(({ session }) => session.sent.length));
  for (const { session } of sessions) session.emit({ type: "turn_done", ok: false, error: "Failed" });
  await expect(client.call("chat.retry", { messageId: post.id })).rejects.toThrow(/bad_params/);
  await client.call("chat.retry", { messageId: post.id, botId: first.id });
  await waitFor(() => sessions.find(({ botId }) => botId === first.id)!.session.sent.length === 2);
  expect(sessions.find(({ botId }) => botId !== first.id)!.session.sent).toHaveLength(1);
});

it("retries a failed routine as a new run with a new label and chain", async () => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Routine retry"));
  const routine = await client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["08:48"] });
  daemon.scheduler.tick(Date.parse(routine.nextRunAt!));
  await waitFor(() => sessions[0]?.session.sent.length);
  const old = daemon.runner.currentWork(bot.id)!;
  sessions[0].session.emit({ type: "turn_done", ok: false, error: "Routine failed" });
  const label = history(db, bot.id, undefined, 50).find((message) => message.kind === "routine")!;
  expect(label.status).toBe("done");
  expect(history(db, bot.id, undefined, 50)).toContainEqual(expect.objectContaining({ kind: "error", data: { retryMessageId: label.id } }));
  const nextLabel = await client.call("chat.retry", { messageId: label.id });
  await waitFor(() => sessions[0].session.sent.length === 2);
  const next = daemon.runner.currentWork(bot.id)!;
  expect(next.routineRunId).toBeTruthy();
  expect(next.routineRunId).not.toBe(old.routineRunId);
  expect(next.chainId).not.toBe(old.chainId);
  expect(next.messageId).toBe(nextLabel.id);
  expect(nextLabel.id).not.toBe(label.id);
  expect(history(db, bot.id, undefined, 50).find((message) => message.id === nextLabel.id)?.status).toBe("done");
  expect(listRuns(db, routine.id)).toEqual(expect.arrayContaining([expect.objectContaining({ id: old.routineRunId, status: "failed" }), expect.objectContaining({ id: next.routineRunId, status: "running" })]));
  await expect(client.call("chat.retry", { messageId: label.id })).rejects.toThrow(/busy/);
  sessions[0].session.finish("Recovered");
  expect(history(db, bot.id, undefined, 50).find((message) => message.id === nextLabel.id)?.data?.status).toBe("done");
});

it("settles a routine and re-arms its lifecycle after a provider limit requeue", async () => {
  const requeued = vi.spyOn(Scheduler.prototype, "runRequeued");
  const started = vi.spyOn(Scheduler.prototype, "runStarted");
  const ended = vi.spyOn(Scheduler.prototype, "runEnded");
  const teamEnded = vi.spyOn(Team.prototype, "turnEnded");
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Clockwork"));
  const routine = await client.call("routines.create", { botId: bot.id, name: "Tick", prompt: "Tick", days: [0, 1, 2, 3, 4, 5, 6], times: ["03:00"], timeLimitMinutes: 1 });
  if (!routine.nextRunAt) throw new Error("Missing next run");
  daemon.scheduler.tick(Date.parse(routine.nextRunAt));
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: new Date(Date.now() + 60_000).toISOString() });
  sessions[0].session.emit({ type: "turn_done", ok: false, error: "Limited" });
  expect(requeued).toHaveBeenCalledOnce();
  expect(ended).not.toHaveBeenCalled();
  setLimitedUntil(db, bot.id, null);
  daemon.runner.pump(bot.id);
  await waitFor(() => sessions[0].session.sent.length === 2);
  expect(started).toHaveBeenCalledTimes(2);
  sessions[0].session.finish("TICK");
  expect(listRuns(db, routine.id)).toMatchObject([{ status: "done" }]);
  expect(ended).toHaveBeenCalledOnce();
  expect(teamEnded).toHaveBeenCalledOnce();
});

it("keeps the bot, queue and exchanges intact when folder removal fails, then deletes successfully", async () => {
  const { daemon, client, db, sessions } = await harness();
  const writer = await client.call("bots.create", input("Writer"));
  const asker = await client.call("bots.create", input("Asker", { canMessage: [writer.id] }));
  await client.call("chat.send", { botId: writer.id, text: "Warm up" });
  sessions[0].session.finish("Ready");
  await client.call("bots.pause", { id: writer.id, paused: true });
  await client.call("chat.send", { botId: asker.id, text: "Ask Writer" });
  await tool(sessions[1].options, "message_bot", { bot: "Writer", message: "Help" });
  const queue = db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(writer.id);
  const exchanges = db.prepare("SELECT * FROM exchanges").all();
  const bot = getStoredBot(db, writer.id);
  removal.failPath = writer.homeFolder;
  removal.beforeRemove = () => expect(sessions[0].session.closed).toBe(true);
  const pump = vi.spyOn(daemon.runner, "pump");
  await expect(client.call("bots.delete", { id: writer.id })).rejects.toThrow(/busy/);
  expect(pump).toHaveBeenCalledWith(writer.id);
  expect(getStoredBot(db, writer.id)).toEqual(bot);
  expect(db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(writer.id)).toEqual(queue);
  expect(db.prepare("SELECT * FROM exchanges").all()).toEqual(exchanges);
  expect(client.events.some((event) => event.event === "bot.deleted")).toBe(false);
  const row = exchanges[0];
  if (typeof row.id !== "string") throw new Error("Missing exchange");
  expect(await client.call("exchanges.get", { id: row.id })).toMatchObject({ exchange: { status: "waiting" } });
  removal.failPath = "";
  await client.call("bots.delete", { id: writer.id });
  expect(existsSync(writer.homeFolder)).toBe(false);
  expect(await client.call("exchanges.get", { id: row.id })).toMatchObject({ exchange: { status: "failed" } });
  expect(client.events.filter((event) => event.event === "bot.deleted")).toHaveLength(1);
});

it("reports queued routine work dropped by routine deletion through both end hooks", async () => {
  const ended = vi.spyOn(Scheduler.prototype, "runEnded");
  const teamEnded = vi.spyOn(Team.prototype, "turnEnded");
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Queued routine"));
  await client.call("chat.send", { botId: bot.id, text: "Keep working" });
  const routine = await client.call("routines.create", { botId: bot.id, name: "Waiting", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["03:00"] });
  if (!routine.nextRunAt) throw new Error("Missing next run");
  daemon.scheduler.tick(Date.parse(routine.nextRunAt));
  const run = listRuns(db, routine.id)[0];
  expect(run.status).toBe("queued");
  await client.call("routines.delete", { id: routine.id });
  expect(ended).toHaveBeenCalledOnce();
  expect(ended.mock.calls[0][0].routineRunId).toBe(run.id);
  expect(teamEnded).toHaveBeenCalledOnce();
  sessions[0].session.finish("Done");
  expect(sessions[0].session.sent).toHaveLength(1);
});

it("recovers an interrupted routine and sweeps stale shares before starting queued work", async () => {
  const root = tempRoot();
  const paths = dataPaths(root);
  const db = openDb(paths.db);
  databases.push(db);
  ensureAccounts(db, paths.accounts);
  const bot = createBot(db, paths.bots, input("Restart"));
  const routine = createRoutine(db, { botId: bot.id, name: "Interrupted", prompt: "Check", days: [1], times: ["08:00"], enabled: false });
  const run = addRun(db, routine.id, new Date().toISOString(), "running");
  const item = enqueue(db, { botId: bot.id, source: "routine", text: "Check", routineRunId: run.id, chainId: newChain(db) });
  setStatus(db, item.id, "running");
  const queued = enqueue(db, { botId: bot.id, source: "you", text: "Queued", chainId: newChain(db) });
  const stale = join(root, "shared-files", "share-old");
  mkdirSync(stale, { recursive: true });
  const deleted = join(paths.bots, `${newId()}.deleting-${newId()}`);
  mkdirSync(deleted);
  const old = new Date(Date.now() - 25 * 60 * 60_000);
  utimesSync(stale, old, old);
  const listen = serverModule.startServer;
  const server = vi.spyOn(serverModule, "startServer").mockImplementation((options) => {
    expect(options.allowedOrigins).toEqual(["app://tjommie", "http://localhost:3000"]);
    expect(getWorkItem(db, item.id).status).toBe("running");
    expect(getRun(db, run.id)?.status).toBe("running");
    expect(getWorkItem(db, queued.id).status).toBe("queued");
    expect(existsSync(stale)).toBe(true);
    expect(existsSync(deleted)).toBe(true);
    return listen(options);
  });
  const { sessions } = await harness(root);
  expect(server).toHaveBeenCalledOnce();
  await waitFor(() => sessions[0]?.session.sent.length);
  expect(getWorkItem(db, item.id).status).toBe("interrupted");
  expect(getRun(db, run.id)?.status).toBe("failed");
  expect(getWorkItem(db, queued.id).status).toBe("running");
  expect(appServer(sessions[0].options).url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  expect(existsSync(stale)).toBe(false);
  expect(existsSync(deleted)).toBe(false);
});

it("leaves the live daemon's work, approvals and streaming messages untouched on a second start", async () => {
  const { daemon, client, db, sessions, root } = await harness();
  const bot = await client.call("bots.create", input("Still working"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  sessions[0].session.emit({ type: "approval", id: "waiting", tool: "Write", summary: "Write", detail: "file", rule: null });
  sessions[0].session.emit({ type: "text", delta: "In progress" });
  const before = {
    work: db.prepare("SELECT * FROM work_items").all(),
    approvals: db.prepare("SELECT * FROM approvals").all(),
    messages: db.prepare("SELECT * FROM messages").all(),
  };
  expect(before.work).toMatchObject([{ status: "running" }]);
  expect(before.approvals).toMatchObject([{ status: "pending" }]);
  expect(before.messages.some((row) => row.status === "streaming")).toBe(true);
  const deleted = join(dataPaths(root).bots, `${newId()}.deleting-${newId()}`);
  mkdirSync(deleted);
  vi.mocked(checkSetup).mockClear();
  await expect(startDaemon({ root, port: daemon.port, auth: { mode: "development" } })).rejects.toMatchObject({ code: "EADDRINUSE" });
  expect(db.prepare("SELECT * FROM work_items").all()).toEqual(before.work);
  expect(db.prepare("SELECT * FROM approvals").all()).toEqual(before.approvals);
  expect(db.prepare("SELECT * FROM messages").all()).toEqual(before.messages);
  expect(checkSetup).not.toHaveBeenCalled();
  expect(existsSync(deleted)).toBe(true);
});

it.each(["development", "production"] as const)("allows the dev origin only in development mode (%s)", async (mode) => {
  const { daemon } = await harness(tempRoot(), mode === "development" ? { mode } : { mode, token: "p".repeat(43) });
  const upgrade = (origin: string) => new Promise<boolean>((resolve) => {
    const socket = new WebSocket(`ws://127.0.0.1:${daemon.port}`, { origin });
    socket.once("open", () => { socket.close(); resolve(true); });
    socket.once("error", () => resolve(false));
  });
  expect(await upgrade("app://tjommie")).toBe(true);
  expect(await upgrade("http://localhost:3000")).toBe(mode === "development");
});

it("uses a supplied production token without creating or keeping a token file", async () => {
  const root = tempRoot();
  const tokenFile = dataPaths(root).token;
  writeFileSync(tokenFile, "old-development-token");
  const { daemon } = await harness(root, { mode: "production", token: "p".repeat(43) });
  expect(daemon.token).toBe("p".repeat(43));
  expect(existsSync(tokenFile)).toBe(false);
});

it("reports a plain error when the old development token file cannot be removed", async () => {
  const cache = join(fileURLToPath(new URL("../../../", import.meta.url)), "node_modules", ".cache");
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "tjommie-token-removal-"));
  const target = resolve(root);
  if (dirname(target) !== resolve(cache) || !target.startsWith(join(resolve(cache), "tjommie-token-removal-"))) {
    throw new Error("Refusing to remove a folder outside the test root.");
  }
  const tokenFile = join(root, "daemon-token");
  try {
    writeFileSync(tokenFile, "old-development-token");
    removal.failTokenPath = tokenFile;
    await expect(startDaemon({ root, port: 0, auth: { mode: "production", token: "p".repeat(43) } }))
      .rejects.toThrow("Could not remove the development daemon-token file. Close programs using it and start Tjommie again.");
    expect(removal.tokenOptions).toMatchObject({ force: true, recursive: true, maxRetries: 3 });
    expect(existsSync(tokenFile)).toBe(true);
  } finally {
    removal.failTokenPath = "";
    rmSync(target, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

it("waits for a delayed session close before capturing exchanges and deleting the bot", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Delayed close"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  sessions[0].session.finish("Done");
  const exited = Promise.withResolvers<void>();
  const closing = Promise.withResolvers<void>();
  vi.spyOn(sessions[0].session, "close").mockImplementation(() => {
    closing.resolve();
  });
  Object.assign(sessions[0].session, { waitForExit: () => exited.promise });
  const captured = vi.spyOn(Team.prototype, "botDeleting");
  const deletion = client.call("bots.delete", { id: bot.id });
  try {
    await closing.promise;
    expect(captured).not.toHaveBeenCalled();
    expect(getStoredBot(db, bot.id).id).toBe(bot.id);
    expect(existsSync(bot.homeFolder)).toBe(true);
  } finally {
    exited.resolve();
    await deletion;
  }
  expect(captured).toHaveBeenCalledExactlyOnceWith(bot.id);
  expect(existsSync(bot.homeFolder)).toBe(false);
  expect(db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(bot.id)).toEqual([]);
});

it("returns busy after fifteen seconds and restores work only when the delayed exit arrives", async () => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Slow close"));
  const message = await client.call("chat.send", { botId: bot.id, text: "Still running" });
  const exited = Promise.withResolvers<void>();
  const closing = Promise.withResolvers<void>();
  vi.spyOn(sessions[0].session, "close").mockImplementation(() => { closing.resolve(); });
  Object.assign(sessions[0].session, { waitForExit: () => exited.promise });
  const captured = vi.spyOn(Team.prototype, "botDeleting");
  let outcome: unknown;
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const deletion = client.call("bots.delete", { id: bot.id }).then(
    (result) => { outcome = result; },
    (error: unknown) => { outcome = error; },
  );
  try {
    await closing.promise;
    await vi.advanceTimersByTimeAsync(14_999);
    expect(outcome).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    await vi.waitFor(() => expect(outcome).toBeInstanceOf(Error), { timeout: 200, interval: 10 });
    expect((outcome as Error).message).toMatch(/^busy: The bot is taking too long to close\./);
    expect(captured).not.toHaveBeenCalled();
    expect(getStoredBot(db, bot.id).id).toBe(bot.id);
    expect(existsSync(bot.homeFolder)).toBe(true);
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "running" });
    daemon.runner.pump(bot.id);
    expect(sessions).toHaveLength(1);
    exited.resolve();
    await vi.waitFor(() => expect(sessions).toHaveLength(2), { timeout: 200, interval: 10 });
    expect(sessions[1].session.sent).toContain("Still running");
    expect(client.events.some((event) => event.event === "bot.deleted" && event.data.id === bot.id)).toBe(false);
    expect(captured).not.toHaveBeenCalled();
  } finally {
    exited.resolve();
    vi.useRealTimers();
    await deletion;
  }
});

it("rejects a second Delete while the first request waits for the same process", async () => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("One deletion"));
  await client.call("chat.send", { botId: bot.id, text: "Running" });
  const exited = Promise.withResolvers<void>();
  const closing = Promise.withResolvers<void>();
  vi.spyOn(sessions[0].session, "close").mockImplementation(() => { closing.resolve(); });
  Object.assign(sessions[0].session, { waitForExit: () => exited.promise });
  const cancelled = vi.spyOn(daemon.runner, "cancelDeletion");
  const deletion = client.call("bots.delete", { id: bot.id });
  await closing.promise;
  let outcome: unknown;
  const duplicate = client.call("bots.delete", { id: bot.id }).then(
    (result) => { outcome = result; },
    (error: unknown) => { outcome = error; },
  );
  try {
    await vi.waitFor(() => expect(outcome).toBeInstanceOf(Error), { timeout: 200, interval: 10 });
    expect((outcome as Error).message).toMatch(/^busy:/);
    expect(cancelled).not.toHaveBeenCalled();
    expect(getStoredBot(db, bot.id).id).toBe(bot.id);
    expect(sessions).toHaveLength(1);
  } finally {
    exited.resolve();
    await deletion;
    await duplicate;
  }
  expect(existsSync(bot.homeFolder)).toBe(false);
  expect(sessions).toHaveLength(1);
});

it("pumps waiting work after unpause and a successful setup refresh", async () => {
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Paused"));
  await client.call("bots.pause", { id: bot.id, paused: true });
  await client.call("chat.send", { botId: bot.id, text: "Queued" });
  expect(sessions).toHaveLength(0);
  await client.call("bots.pause", { id: bot.id, paused: false });
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.finish("Done");
  vi.mocked(checkSetup).mockResolvedValue(setup(false));
  await client.call("setup.status", {});
  await client.call("chat.send", { botId: bot.id, text: "Wait for sign-in" });
  expect(sessions[0].session.sent).toHaveLength(1);
  vi.mocked(checkSetup).mockResolvedValue(setup());
  await client.call("setup.status", {});
  await waitFor(() => sessions[0].session.sent.length === 2);
});

it("moves an existing SQLite database before opening the daemon database", async () => {
  const root = tempRoot();
  const legacy = join(root, "tjommie.db");
  const old = openDb(legacy);
  old.prepare("INSERT INTO settings (key, value) VALUES ('migration-probe', 'kept')").run();
  old.close();
  const { db } = await harness(root);
  expect(existsSync(legacy)).toBe(false);
  expect(existsSync(dataPaths(root).db)).toBe(true);
  expect(db.prepare("SELECT value FROM settings WHERE key = 'migration-probe'").get()).toEqual({ value: "kept" });
});

it("keeps Codex Own folder queued until sandbox setup succeeds and broadcasts the change", async () => {
  vi.mocked(checkSetup).mockResolvedValue(setup(true, "notConfigured"));
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Codex protected", { provider: "codex", model: "test", autonomy: "folder" }));
  expect(bot).toMatchObject({ state: "needs_setup", setupReason: "Set up Codex's sandbox" });
  await client.call("chat.send", { botId: bot.id, text: "Wait" });
  expect(sessions).toHaveLength(0);
  vi.mocked(checkSetup).mockResolvedValue(setup());
  const status = await client.call("setup.prepareCodexSandbox", {});
  expect(prepareCodexSandbox).toHaveBeenCalledOnce();
  expect(status).toMatchObject({ provider: "codex", sandbox: "ready" });
  expect(client.events.some((event) => event.event === "setup.changed" && event.data.some((entry) => entry.provider === "codex" && entry.sandbox === "ready"))).toBe(true);
  expect(client.events.some((event) => event.event === "bot.updated" && event.data.id === bot.id && event.data.state === "idle")).toBe(true);
  await waitFor(() => sessions.length === 1);
});

it("fails Codex Own folder closed when a required ACL cannot be verified", async () => {
  vi.mocked(protectCodexDataAsync).mockResolvedValue({ status: "check_failed", ready: false, reason: "Tjommie could not protect its data" });
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Codex ACL", { provider: "codex", model: "test", autonomy: "folder" }));
  expect(bot).toMatchObject({ state: "needs_setup", setupReason: "Tjommie could not protect its data" });
  await client.call("chat.send", { botId: bot.id, text: "Wait" });
  expect(sessions).toHaveLength(0);
});

it("protects each new Claude bot and reconciles both provider switches before returning", async () => {
  const passes: { id: string; provider: string }[][] = [];
  vi.mocked(protectCodexDataAsync).mockImplementation(async (_paths, homes) => {
    passes.push((homes ?? []).map((home) => ({ id: home.id, provider: home.provider })));
    return { status: "ready", ready: true, reason: null };
  });
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Changing provider"));
  expect(passes.at(-1)).toContainEqual({ id: bot.id, provider: "claude" });
  await client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  expect(passes.at(-1)).toContainEqual({ id: bot.id, provider: "codex" });
  await client.call("bots.update", { id: bot.id, patch: { provider: "claude", model: "haiku" } });
  expect(passes.at(-1)).toContainEqual({ id: bot.id, provider: "claude" });
  const count = passes.length;
  await client.call("bots.delete", { id: bot.id });
  expect(passes).toHaveLength(count);
});

it.each(["create", "switch"] as const)("keeps an unrelated busy Codex Own folder turn running during a Claude bot %s", async (action) => {
  const { client, db, sessions } = await harness();
  const codex = await client.call("bots.create", input("Busy Codex", { provider: "codex", model: "test", autonomy: "folder" }));
  await client.call("chat.send", { botId: codex.id, text: "Keep working" });
  await waitFor(() => sessions[0]?.session.sent.length);
  const other = action === "switch" ? await client.call("bots.create", input("Other Codex", { provider: "codex", model: "test" })) : null;
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { status: "ready", ready: true, reason: null };
  });
  const change = other
    ? client.call("bots.update", { id: other.id, patch: { provider: "claude", model: "haiku" } })
    : client.call("bots.create", input("New Claude"));
  await entered;
  expect(sessions[0].session.interruptions).toBe(0);
  const item = db.prepare("SELECT id FROM work_items WHERE bot_id = ?").get(codex.id) as { id: string };
  expect(getWorkItem(db, item.id).status).toBe("running");
  release?.();
  await change;
  expect(sessions[0].session.interruptions).toBe(0);
  expect(sessions[0].session.closed).toBe(false);
  expect((await client.call("bots.list", {})).find((bot) => bot.id === codex.id)?.state).toBe("working");
});

it("holds a switching bot before its stopped turn can pump queued work on the old provider", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switching"));
  await client.call("chat.send", { botId: bot.id, text: "First" });
  await waitFor(() => sessions[0]?.session.sent.length);
  await client.call("chat.send", { botId: bot.id, text: "Queued" });
  await waitFor(() => db.prepare("SELECT 1 FROM work_items WHERE bot_id = ? AND status = 'queued'").get(bot.id));
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { status: "ready", ready: true, reason: null };
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await entered;
  expect(getStoredBot(db, bot.id).provider).toBe("claude");
  expect(sessions).toHaveLength(1);
  release?.();
  await change;
  await waitFor(() => sessions.length === 2);
  expect(sessions[1].botId).toBe(bot.id);
  expect(getStoredBot(db, bot.id).provider).toBe("codex");
});

it("keeps a provider switch held when an earlier ACL pass finishes", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switching"));
  await client.call("chat.send", { botId: bot.id, text: "First" });
  await waitFor(() => sessions[0]?.session.sent.length);
  await client.call("chat.send", { botId: bot.id, text: "Queued" });
  await waitFor(() => db.prepare("SELECT 1 FROM work_items WHERE bot_id = ? AND status = 'queued'").get(bot.id));
  const releases: Array<() => void> = [];
  const entered: Array<() => void> = [];
  const passes = [0, 1].map(() => new Promise<void>((resolve) => { releases.push(resolve); }));
  const arrivals = [0, 1].map(() => new Promise<void>((resolve) => { entered.push(resolve); }));
  let pass = 0;
  vi.mocked(protectCodexDataAsync).mockImplementation(async () => {
    const index = pass++;
    entered[index]?.();
    await passes[index];
    return { status: "ready", ready: true, reason: null };
  });
  vi.spyOn(Date, "now").mockReturnValue(Date.now() + 61_000);
  const status = client.call("setup.status", {});
  await arrivals[0];
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await waitFor(() => sessions[0].session.interruptions === 1);
  releases[0]();
  await status;
  await arrivals[1];
  expect(getStoredBot(db, bot.id).provider).toBe("claude");
  expect(sessions).toHaveLength(1);
  releases[1]();
  await change;
  await waitFor(() => sessions.length === 2);
  expect(sessions[1].botId).toBe(bot.id);
  expect(getStoredBot(db, bot.id).provider).toBe("codex");
});

it("waits for Claude deny removal before the first Codex turn", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switch guard"));
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { status: "ready", ready: true, reason: null };
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await entered;
  expect(getStoredBot(db, bot.id).provider).toBe("claude");
  expect(sessions).toHaveLength(0);
  release?.();
  const changed = await change;
  expect(changed.state).toBe("idle");
  await client.call("chat.send", { botId: bot.id, text: "Now run" });
  await waitFor(() => sessions.length === 1);
  expect(sessions[0].options.autonomy).toBe("folder");
});

it("allows both provider switch directions while Codex sandbox setup is absent", async () => {
  vi.mocked(checkSetup).mockResolvedValue(setup(true, "notConfigured"));
  const { client, db } = await harness();
  const claude = await client.call("bots.create", input("Claude ask", { autonomy: "ask" }));
  const codex = await client.call("bots.create", input("Codex ask", { provider: "codex", model: "test", autonomy: "ask" }));
  vi.mocked(protectCodexDataAsync).mockResolvedValue({ ready: false, status: "not_configured", reason: "Set up Codex's sandbox" });
  expect(await client.call("bots.update", { id: claude.id, patch: { provider: "codex", model: "test" } })).toMatchObject({ provider: "codex", state: "idle" });
  expect(await client.call("bots.update", { id: codex.id, patch: { provider: "claude", model: "haiku" } })).toMatchObject({ provider: "claude", state: "idle" });
  expect(getStoredBot(db, claude.id).provider).toBe("codex");
  expect(getStoredBot(db, codex.id).provider).toBe("claude");
});

it("rejects invalid provider switches before stopping or dropping the running session", async () => {
  const { client, db, root, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switch validation"));
  const taken = await client.call("bots.create", input("Taken name"));
  await client.call("chat.send", { botId: bot.id, text: "Keep working" });
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.emit({ type: "session", cliSessionId: "fixture-session" });
  const before = getStoredBot(db, bot.id);
  const item = db.prepare("SELECT id FROM work_items WHERE bot_id = ?").get(bot.id) as { id: string };
  const passes = vi.mocked(protectCodexDataAsync).mock.calls.length;
  const invalid: BotPatch[] = [
    { provider: "codex" },
    { provider: "codex", model: "test", name: taken.name },
    { provider: "codex", model: "test", projectFolder: join(root, "missing-project") },
    { provider: "codex", model: "test", canMessage: ["missing-bot"] },
  ];
  for (const patch of invalid) {
    await expect(client.call("bots.update", { id: bot.id, patch })).rejects.toThrow();
    expect(getStoredBot(db, bot.id)).toEqual(before);
    expect(getWorkItem(db, item.id).status).toBe("running");
    expect(sessions[0].session.interruptions).toBe(0);
    expect(sessions[0].session.closed).toBe(false);
    expect(db.prepare("SELECT COUNT(*) AS count FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ count: 1 });
    expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(passes);
  }
});

it("keeps a held bot's stop notice when setup changes during its stop", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Held Claude"));
  await client.call("chat.send", { botId: bot.id, text: "Working" });
  await waitFor(() => sessions[0]?.session.sent.length);
  const session = sessions[0].session;
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  vi.spyOn(session, "interrupt").mockImplementation(async () => {
    session.interruptions++;
    await pending;
    session.emit({ type: "turn_done", ok: false, error: null });
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await waitFor(() => session.interruptions === 1);
  const now = Date.now();
  vi.spyOn(Date, "now").mockReturnValue(now + 61_000);
  await client.call("setup.status", {});
  release?.();
  await change;
  expect(session.interruptions).toBe(1);
  expect(history(db, bot.id, undefined, 50)).toContainEqual(expect.objectContaining({ kind: "notice", body: "Stopped." }));
  expect(history(db, bot.id, undefined, 50).some((message) => message.body.includes("sandbox is no longer ready"))).toBe(false);
});

it("refuses a failed provider switch and releases its hold for old-provider work", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Failed switch"));
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { ready: false, status: "check_failed", reason: "Could not verify the ACL" };
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await entered;
  await client.call("chat.send", { botId: bot.id, text: "Run on Claude after rollback" });
  expect(sessions).toHaveLength(0);
  release?.();
  await expect(change).rejects.toThrow(/bad_state/);
  expect(getStoredBot(db, bot.id)).toMatchObject({ provider: "claude", model: "haiku" });
  await waitFor(() => sessions[0]?.session.sent.length);
  expect(sessions[0].botId).toBe(bot.id);
  expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(4);
});

it("caches setup.status ACL results for 60 seconds", async () => {
  const { client } = await harness();
  expect(vi.mocked(protectCodexData)).toHaveBeenCalledOnce();
  const initial = vi.mocked(protectCodexDataAsync).mock.calls.length;
  expect(initial).toBe(1);
  await client.call("setup.status", {});
  await client.call("setup.status", {});
  expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(initial);
  const now = Date.now();
  vi.spyOn(Date, "now").mockReturnValue(now + 61_000);
  await client.call("setup.status", {});
  expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(initial + 1);
});

it("logs Codex CLI drift from the measured version once", async () => {
  const different = setup().map((entry) => entry.provider === "codex" ? { ...entry, version: "0.157.1" } : entry);
  vi.mocked(checkSetup).mockResolvedValue(different as ProviderSetup[]);
  const info = vi.spyOn(console, "info").mockImplementation(() => {});
  const { client } = await harness();
  await client.call("setup.status", {});
  await client.call("setup.status", {});
  expect(info.mock.calls.filter((call) => String(call[0]).includes("differs from the measured ACL recipe"))).toHaveLength(1);
});

it("logs a rejected background setup refresh from the sign-in callback", async () => {
  const { client } = await harness();
  const failure = new Error("Setup unavailable");
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  vi.mocked(checkSetup).mockRejectedValueOnce(failure);
  vi.mocked(signIn).mockImplementation(async (_db, _provider, onDone) => {
    onDone(true);
    return { url: null };
  });
  await client.call("setup.signIn", { provider: "claude" });
  await waitFor(() => log.mock.calls.length);
  expect(log).toHaveBeenCalledWith("Failed to refresh provider setup.", failure);
});

it("awaits browser launch errors and rejects showing a disabled browser", async () => {
  const { client } = await harness();
  const off = await client.call("bots.create", input("Off"));
  await expect(client.call("bots.showBrowser", { id: off.id })).rejects.toThrow(/bad_params/);
  const on = await client.call("bots.create", input("On", { browser: true }));
  vi.mocked(showBrowser).mockRejectedValueOnce(new Error("Browser could not start"));
  await expect(client.call("bots.showBrowser", { id: on.id })).rejects.toThrow(/Browser could not start/);
  await expect(client.call("bots.showBrowser", { id: on.id })).resolves.toEqual({});
});
