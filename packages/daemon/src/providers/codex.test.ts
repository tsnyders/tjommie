import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

const cliMocks = vi.hoisted(() => ({ killTreeAndWait: vi.fn() }));
vi.mock("../cli.ts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../cli.ts")>()),
  killTreeAndWait: cliMocks.killTreeAndWait,
}));

vi.mock("./codex-rpc.ts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./codex-rpc.ts")>()),
  startCodexServer: vi.fn(),
}));

import { JsonRpcLines, RpcRemoteError, startCodexServer } from "./codex-rpc.ts";
import { CodexSession } from "./codex.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";
import { createBot, getStoredBot } from "../bots.ts";
import { history } from "../chat.ts";
import { openDb } from "../db.ts";
import { getWorkItem } from "../queue.ts";
import { Runner } from "../runner.ts";
import { ensureAccounts } from "../setup.ts";
import { tempRoot, waitFor } from "../testing.ts";

const options: StartOptions = {
  cwd: "C:\\bots\\b1",
  extraDirs: [],
  model: "",
  instructions: "Be brief.",
  autonomy: "folder",
  allowRules: [],
  resumeId: null,
  env: {},
  mcpServers: [],
};

const elicitation = {
  serverName: "tjommie",
  mode: "form",
  message: "Please authorize the ping operation on this MCP server.",
  requestedSchema: {},
  _meta: { codex_approval_kind: "mcp_tool_call", tool_description: "Ping", tool_params: { text: "hello" } },
};

const cleanups: (() => void)[] = [];
const children = new Map<number, ChildProcessWithoutNullStreams>();
let nextPid = 4000;

function exitChild(child: ChildProcessWithoutNullStreams, code: number): void {
  Object.assign(child, { exitCode: code });
  child.emit("exit", code);
}

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  children.clear();
  vi.clearAllMocks();
});

async function session(autoAllow: boolean, startImmediately = true, resumeId: string | null = null) {
  const sent: { id: number | string; method?: string }[] = [];
  const rpc = new JsonRpcLines((line) => {
    const message = JSON.parse(line) as { id: number | string; method?: string };
    if (message.method === "thread/start" || message.method === "thread/resume") {
      rpc.handleLine(JSON.stringify({ id: message.id, result: { thread: { id: "thread-1" }, model: "test-model" } }));
    } else {
      sent.push(message);
    }
  });
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  const child = Object.assign(new EventEmitter(), { stdin, stdout, stderr, pid: ++nextPid, exitCode: null, signalCode: null }) as unknown as ChildProcessWithoutNullStreams;
  children.set(nextPid, child);
  cliMocks.killTreeAndWait.mockImplementation(async (pid: number) => {
    const target = children.get(pid);
    if (!target) throw new Error("Unknown process");
    exitChild(target, 1);
  });
  const close = () => {
    rpc.failAll(new Error("Test session closed"));
    stdin.destroy();
    stdout.destroy();
    stderr.destroy();
    if (child.exitCode === null) {
      exitChild(child, 0);
    }
  };
  vi.mocked(startCodexServer).mockResolvedValueOnce({ rpc, child, close });
  const codex = new CodexSession({ file: "codex", args: [] }, {
    ...options,
    resumeId,
    mcpServers: [{ name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: {}, autoAllow }],
  });
  cleanups.push(() => codex.close());
  const events: ProviderEvent[] = [];
  codex.onEvent((event) => events.push(event));
  if (startImmediately) await codex.start();
  events.length = 0;
  return { codex, rpc, sent, events, child };
}

describe("CodexSession MCP elicitations", () => {
  it.each([
    { autoAllow: true, action: "accept" },
    { autoAllow: false, action: "decline" },
  ])("answers $action for unexpected wording when autoAllow is $autoAllow", async ({ autoAllow, action }) => {
    const { rpc, sent, events } = await session(autoAllow);

    rpc.handleLine(JSON.stringify({ id: 7, method: "mcpServer/elicitation/request", params: elicitation }));

    expect(sent).toEqual([{ id: 7, result: { action } }]);
    expect(events).toEqual([]);
  });

  it("still declines malformed tool-call elicitations for an autoAllow server", async () => {
    const { rpc, sent, events } = await session(true);
    const malformed: unknown[] = [
      null,
      [],
      { ...elicitation, serverName: 3 },
      { ...elicitation, serverName: "" },
      { ...elicitation, mode: "url" },
      { ...elicitation, message: 3 },
      { ...elicitation, _meta: null },
      { ...elicitation, _meta: [] },
      { ...elicitation, _meta: { ...elicitation._meta, codex_approval_kind: "other" } },
      { ...elicitation, _meta: { ...elicitation._meta, tool_description: 3 } },
      { ...elicitation, _meta: { ...elicitation._meta, tool_params: "bad" } },
      { ...elicitation, _meta: { ...elicitation._meta, tool_params: [] } },
    ];

    malformed.forEach((params, id) => rpc.handleLine(JSON.stringify({ id, method: "mcpServer/elicitation/request", params })));

    expect(sent).toEqual(malformed.map((_params, id) => ({ id, result: { action: "decline" } })));
    expect(events).toEqual([]);
  });

  it("keeps parseable calls from a non-autoAllow server on the approval-card path", async () => {
    const { codex, rpc, sent, events } = await session(false);
    rpc.handleLine(JSON.stringify({
      id: "approval-1",
      method: "mcpServer/elicitation/request",
      params: { ...elicitation, message: 'Allow the tjommie MCP server to run tool "ping"?' },
    }));

    expect(sent).toEqual([]);
    expect(events).toEqual([{
      type: "approval",
      id: "approval-1",
      tool: "tjommie.ping",
      summary: "Ping",
      detail: JSON.stringify({ text: "hello" }, null, 2),
      rule: "codex-mcp:tjommie.ping",
    }]);
    codex.answer("approval-1", "once");
    expect(sent).toEqual([{ id: "approval-1", result: { action: "accept" } }]);
  });
});

describe("CodexSession steering", () => {
  it.each([false, true])("ignores an approved command's late events after Stop (follow-up already queued: %s)", async (alreadyQueued) => {
    const { codex, rpc, sent, child } = await session(true, false);
    const replacement = await session(true, false, "thread-1");
    const terminated = Promise.withResolvers<void>();
    cliMocks.killTreeAndWait.mockImplementationOnce((pid: number) => {
      expect(pid).toBe(child.pid);
      exitChild(child, 1); // parent exit alone must not release the runner's queue
      return terminated.promise;
    });
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const bot = createBot(db, join(root, "bots"), { name: "Codex Stop", color: "blue", shape: "circle", provider: "codex", model: "test-model", autonomy: "ask", instructions: "" });
    const ended: string[] = [];
    const startsWith: StartOptions[] = [];
    const runner = new Runner({ db, broadcast: () => {}, isSignedIn: () => true, createSession: (_bot, options) => {
      startsWith.push(options);
      return startsWith.length === 1 ? codex : replacement.codex;
    }, hooks: { turnEnded: (item) => ended.push(item.id) } });
    cleanups.push(() => { runner.shutdown(); db.close(); });
    const notify = (method: string, params: Record<string, unknown>) => rpc.handleLine(JSON.stringify({ method, params: { threadId: "thread-1", ...params } }));
    const message = runner.submit(bot.id, "Run the foreground timer", []);
    await waitFor(() => sent.some((request) => request.method === "turn/start"), 2_000);
    rpc.handleLine(JSON.stringify({ id: 2, result: { turn: { id: "stopped" } } }));
    notify("turn/started", { turn: { id: "stopped", status: "inProgress" } });
    const command = { turnId: "stopped", item: { id: "timer", type: "commandExecution", command: "node timer", status: "inProgress" } };
    notify("item/started", command);
    rpc.handleLine(JSON.stringify({ id: "approve", method: "item/commandExecution/requestApproval", params: { threadId: "thread-1", turnId: "stopped", itemId: "timer", command: "node timer" } }));
    const approval = db.prepare("SELECT id FROM approvals WHERE status = 'pending'").get() as { id: string };
    runner.answerApproval(approval.id, "once");
    const work = { botId: bot.id, source: "routine" as const, text: "Reply ROUTINE-OK" };
    let followup = alreadyQueued ? runner.deliver(work) : null;
    const stopping = runner.stop(bot.id);
    await waitFor(() => sent.some((request) => request.method === "turn/interrupt"), 2_000);
    const interrupt = sent.find((request) => request.method === "turn/interrupt")!;
    notify("turn/completed", { turn: { id: "stopped", status: "interrupted", error: null } });
    // Captured from the real app-server: approval -> Stop -> turn/completed ->
    // item/started for the same command -> item/completed, with no further turn completion.
    notify("item/started", command);
    notify("item/completed", { ...command, item: { ...command.item, status: "failed" } });
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "running" });
    rpc.handleLine(JSON.stringify({ id: interrupt.id, result: {} }));
    await waitFor(() => cliMocks.killTreeAndWait.mock.calls.length > 0, 2_000);
    expect(ended).toEqual([]);
    expect(startsWith).toHaveLength(1);
    expect(runner.toBot(getStoredBot(db, bot.id)).state).toBe("working");
    terminated.resolve();
    await stopping;
    if (!alreadyQueued) expect(runner.toBot(getStoredBot(db, bot.id)).state).toBe("idle");
    expect(history(db, bot.id, undefined, 50).filter((entry) => entry.status === "streaming")).toEqual([]);
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "interrupted" });
    followup ??= runner.deliver(work);
    expect(getWorkItem(db, followup.id).status).toBe("running");
    await waitFor(() => replacement.sent.some((request) => request.method === "turn/start"), 2_000);
    expect(startsWith.map((options) => options.resumeId)).toEqual([null, "thread-1"]);
    expect(cliMocks.killTreeAndWait).toHaveBeenCalledWith(child.pid);
    const nextNotify = (method: string, params: Record<string, unknown>) => replacement.rpc.handleLine(JSON.stringify({ method, params }));
    replacement.rpc.handleLine(JSON.stringify({ id: 2, result: { turn: { id: "next" } } }));
    nextNotify("turn/started", { turn: { id: "next", status: "inProgress" } });
    nextNotify("item/agentMessage/delta", { turnId: "next", itemId: "answer", delta: "ROUTINE-OK" });
    // A delayed duplicate completion or text from Stop cannot finish or pollute the next turn.
    notify("turn/completed", { turn: { id: "stopped", status: "interrupted", error: null } });
    notify("item/agentMessage/delta", { turnId: "stopped", itemId: "old-answer", delta: "STALE" });
    expect(getWorkItem(db, followup.id).status).toBe("running");
    nextNotify("item/completed", { turnId: "next", item: { id: "answer", type: "agentMessage" } });
    nextNotify("turn/completed", { turn: { id: "next", status: "completed", error: null } });
    expect(getWorkItem(db, followup.id).status).toBe("done");
    expect(runner.toBot(getStoredBot(db, bot.id)).state).toBe("idle");
    expect(history(db, bot.id, undefined, 50).filter((entry) => entry.kind === "text" && entry.author === bot.id).map((entry) => entry.body)).toEqual(["ROUTINE-OK"]);
    expect(ended).toHaveLength(2);
  });

  it.each([false, true])("does not restore a completed turn from a delayed start response (background turn: %s)", async (background) => {
    const { codex, rpc, sent, events } = await session(true);
    const send = codex.send("start");
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "old" } } }));
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "old", status: "completed" } } }));
    if (background) {
      rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "background" } } }));
      rpc.handleLine(JSON.stringify({ method: "item/agentMessage/delta", params: { turnId: "background", delta: "Background reply" } }));
    }
    rpc.handleLine(JSON.stringify({ id: 2, result: { turn: { id: "old" } } }));
    await send;
    const steer = codex.steer("Follow up");
    const request = sent.find((entry) => entry.method === "turn/steer");
    if (request) rpc.handleLine(JSON.stringify({ id: request.id, result: {} }));
    if (background) {
      await expect(steer).resolves.toBe(true);
      expect(request).toMatchObject({ method: "turn/steer", params: { expectedTurnId: "background" } });
      expect(events).toContainEqual({ type: "text", delta: "Background reply" });
    } else {
      await expect(steer).resolves.toBe(false);
      expect(sent.filter((request) => request.method === "turn/steer")).toEqual([]);
    }
  });

  it.each([
    { method: "item/commandExecution/requestApproval", params: { command: "node timer" }, response: { decision: "decline" } },
    { method: "item/fileChange/requestApproval", params: { grantRoot: "C:\\bots\\b1" }, response: { decision: "decline" } },
    { method: "mcpServer/elicitation/request", params: elicitation, response: { action: "decline" } },
  ])("declines a late $method without reopening a completed turn", async ({ method, params, response }) => {
    const { rpc, sent, events } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "stopped" } } }));
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "stopped", status: "interrupted" } } }));
    events.length = 0;
    rpc.handleLine(JSON.stringify({ id: "late", method, params: { ...params, turnId: "stopped" } }));
    expect(sent).toEqual([{ id: "late", result: response }]);
    expect(events).toEqual([]);
  });

  it("keeps late usage while ignoring a completed turn's replayed start and content", async () => {
    const { codex, rpc, sent, events } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "stopped" } } }));
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "stopped", status: "interrupted" } } }));
    events.length = 0;
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "stopped" } } }));
    rpc.handleLine(JSON.stringify({ method: "item/agentMessage/delta", params: { turnId: "stopped", delta: "stale" } }));
    rpc.handleLine(JSON.stringify({ method: "thread/tokenUsage/updated", params: { turnId: "stopped", tokenUsage: { last: { inputTokens: 10, cachedInputTokens: 2, outputTokens: 3, reasoningOutputTokens: 1 } } } }));
    await expect(codex.steer("Follow up")).resolves.toBe(false);
    expect(sent).toEqual([]);
    expect(events).toEqual([{ type: "usage", model: "test-model", input: 8, cachedInput: 2, output: 3, reasoning: 1, costUsd: null }]);
  });

  it("settles the current turn when a completion omits its ID", async () => {
    const { codex, rpc, events } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "current" } } }));
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { status: "completed", error: null } } }));
    rpc.handleLine(JSON.stringify({ method: "item/agentMessage/delta", params: { turnId: "current", delta: "stale" } }));
    expect(events).toEqual([{ type: "turn_done", ok: true, error: null }]);
    await expect(codex.steer("Follow up")).resolves.toBe(false);
  });

  it("still rejects unsupported requests associated with a completed turn", async () => {
    const { rpc, sent, events } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "stopped" } } }));
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "stopped", status: "interrupted" } } }));
    events.length = 0;
    rpc.handleLine(JSON.stringify({ id: "late", method: "unsupported/request", params: { turnId: "stopped" } }));
    expect(sent).toEqual([{ id: "late", error: { code: -32601, message: "Not supported by Tjommie" } }]);
    expect(events).toEqual([]);
  });

  it("reports a failed tree termination and lets Stop retry without publishing a premature exit", async () => {
    const { codex, rpc, sent, events } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "stopped" } } }));
    cliMocks.killTreeAndWait.mockRejectedValueOnce(new Error("tree termination failed"));
    const failed = expect(codex.interrupt()).rejects.toThrow("tree termination failed");
    await waitFor(() => sent.some((request) => request.method === "turn/interrupt"), 2_000);
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "stopped", status: "interrupted" } } }));
    rpc.handleLine(JSON.stringify({ id: 2, result: {} }));
    await failed;
    expect(events).toEqual([]);
    const retry = codex.interrupt();
    await waitFor(() => sent.filter((request) => request.method === "turn/interrupt").length === 2, 2_000);
    rpc.handleLine(JSON.stringify({ id: 3, error: { code: -32600, message: "no active turn to interrupt" } }));
    await expect(retry).resolves.toBe(true);
    expect(events).toEqual([{ type: "exit", error: null }]);
  });

  it("publishes the held exit when a pending send rejects during Stop", async () => {
    const { codex, rpc, events, child } = await session(true);
    const failure = new Error("app-server exited during send");
    const sent = expect(codex.send("start")).rejects.toThrow(failure);
    const stopped = expect(codex.interrupt()).rejects.toThrow(failure);
    rpc.failAll(failure);
    exitChild(child, 1);
    await sent;
    await stopped;
    expect(events).toEqual([{ type: "exit", error: failure.message }]);
  });

  it("reports unverified tree termination when the server exits before its interrupt response", async () => {
    const { codex, rpc, sent, events, child } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "active" } } }));
    const stopped = expect(codex.interrupt()).rejects.toThrow("Codex exited before its process tree could be stopped.");
    await waitFor(() => sent.some((request) => request.method === "turn/interrupt"), 2_000);
    rpc.failAll(new Error("app-server exited"));
    exitChild(child, 1);
    await stopped;
    expect(events).toEqual([{ type: "exit", error: "Codex exited before its process tree could be stopped." }]);
  });

  it("reports an exit immediately after Stop when send has already fulfilled", async () => {
    const { codex, rpc, events, child } = await session(true);
    const sent = codex.send("start");
    rpc.handleLine(JSON.stringify({ id: 2, result: { turn: { id: "active" } } }));
    await sent;
    const stopped = expect(codex.interrupt()).rejects.toThrow("Codex exited before its process tree could be stopped.");
    exitChild(child, 1); // #interrupt is still yielding at await #sending (now null)
    await stopped;
    expect(events).toEqual([{ type: "exit", error: "Codex exited before its process tree could be stopped." }]);
  });

  it.each(["completed", "interrupted"] as const)("settles a held %s completion without a turn ID", async (status) => {
    const { codex, rpc, events } = await session(true);
    const stopped = codex.interrupt();
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { status, error: null } } }));
    await expect(stopped).resolves.toBe(true);
    expect(events).toEqual([{ type: "exit", error: null, ...(status === "completed" ? { completedTurn: { ok: true, error: null } } : {}) }]);
  });

  it("holds a normal completion racing Stop until process-tree termination", async () => {
    const { codex, rpc, sent, events, child } = await session(true);
    const terminated = Promise.withResolvers<void>();
    cliMocks.killTreeAndWait.mockImplementationOnce(() => {
      exitChild(child, 1);
      return terminated.promise;
    });
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "active" } } }));
    const stopped = codex.interrupt();
    await waitFor(() => sent.some((request) => request.method === "turn/interrupt"), 2_000);
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "active", status: "completed", error: null } } }));
    rpc.handleLine(JSON.stringify({ id: 2, result: {} }));
    await waitFor(() => cliMocks.killTreeAndWait.mock.calls.length > 0, 2_000);
    expect(events).toEqual([]);
    terminated.resolve();
    await expect(stopped).resolves.toBe(true);
    expect(events).toEqual([{ type: "exit", error: null, completedTurn: { ok: true, error: null } }]);
  });

  it("publishes a tree-termination failure if the parent exit was already held", async () => {
    const { codex, rpc, sent, events, child } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "active" } } }));
    cliMocks.killTreeAndWait.mockImplementationOnce(async () => {
      exitChild(child, 1);
      throw new Error("tree termination failed after exit");
    });
    const stopped = expect(codex.interrupt()).rejects.toThrow("tree termination failed after exit");
    await waitFor(() => sent.some((request) => request.method === "turn/interrupt"), 2_000);
    rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "active", status: "interrupted" } } }));
    rpc.handleLine(JSON.stringify({ id: 2, result: {} }));
    await stopped;
    expect(events).toEqual([{ type: "exit", error: "tree termination failed after exit" }]);
  });

  it("sends one provider interrupt when the runner stops during turn/start", async () => {
    const { codex, rpc, sent } = await session(true, false);
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const bot = createBot(db, join(root, "bots"), { name: "Codex Stop", color: "blue", shape: "circle", provider: "codex", model: "test-model", instructions: "" });
    const runner = new Runner({ db, broadcast: () => {}, isSignedIn: () => true, createSession: () => codex });
    cleanups.push(() => { runner.shutdown(); db.close(); });
    const message = runner.submit(bot.id, "begin", []);
    await waitFor(() => sent.some((request) => request.method === "turn/start"), 2_000);
    const stopped = runner.stop(bot.id);
    rpc.handleLine(JSON.stringify({ id: 2, result: { turn: { id: "turn-1" } } }));
    await new Promise<void>((resolve) => setImmediate(resolve));
    try {
      expect(sent.filter((request) => request.method === "turn/interrupt")).toHaveLength(1);
      expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "running" });
    } finally {
      for (const request of sent.filter((request) => request.method === "turn/interrupt")) {
        rpc.handleLine(JSON.stringify({ id: request.id, result: {} }));
      }
      await stopped;
      rpc.handleLine(JSON.stringify({ method: "turn/completed", params: { turn: { id: "turn-1", status: "interrupted", error: null } } }));
    }
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(sent.filter((request) => request.method === "turn/interrupt")).toHaveLength(1);
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "interrupted" });
  });

  it("waits for an in-flight turn start before interrupting it", async () => {
    const { codex, rpc, sent } = await session(true);
    const send = codex.send("start");
    const stopped = codex.interrupt();
    rpc.handleLine(JSON.stringify({ id: 2, result: { turn: { id: "turn-1" } } }));
    await send;
    await Promise.resolve();
    expect(sent).toContainEqual({ id: 3, method: "turn/interrupt", params: { threadId: "thread-1", turnId: "turn-1" } });
    rpc.handleLine(JSON.stringify({ id: 3, result: {} }));
    await stopped;
  });

  it("closes a server that finishes initializing after the session was closed", async () => {
    const rpc = new JsonRpcLines(() => {});
    const child = Object.assign(new EventEmitter(), { pid: 42, exitCode: null, signalCode: null }) as unknown as ChildProcessWithoutNullStreams;
    const close = vi.fn();
    let initialized!: (server: { rpc: JsonRpcLines; child: ChildProcessWithoutNullStreams; close: () => void }) => void;
    vi.mocked(startCodexServer).mockReturnValueOnce(new Promise((resolve) => { initialized = resolve; }));
    const codex = new CodexSession({ file: "codex", args: [] }, options);
    const start = codex.start();
    const rejected = expect(start).rejects.toThrow("closed");
    codex.close();
    initialized({ rpc, child, close });
    await rejected;
    expect(close).toHaveBeenCalledOnce();
    child.emit("exit", 0);
  });
  it("does not send a steer request when there is no local active turn", async () => {
    const { codex, sent } = await session(true);
    await expect(codex.steer("Follow up")).resolves.toBe(false);
    expect(sent).toEqual([]);
  });

  it("accepts a successful steer response", async () => {
    const { codex, rpc, sent } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "turn-1" } } }));
    const steered = codex.steer("Follow up");
    expect(sent).toEqual([{ id: 2, method: "turn/steer", params: { threadId: "thread-1", expectedTurnId: "turn-1", input: [{ type: "text", text: "Follow up" }] } }]);
    rpc.handleLine(JSON.stringify({ id: 2, result: { turnId: "turn-1" } }));
    await expect(steered).resolves.toBe(true);
  });

  it.each([
    "no active turn to steer",
    "expected active turn id `turn-1` but found `turn-2`",
    "cannot steer a review turn",
    "cannot steer a compact turn",
  ])("returns false for the expected state rejection: %s", async (message) => {
    const { codex, rpc } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "turn-1" } } }));
    const steered = codex.steer("Follow up");
    rpc.handleLine(JSON.stringify({ id: 2, error: { code: -32600, message } }));
    await expect(steered).resolves.toBe(false);
  });

  it.each([
    { code: -32603, message: "failed to steer turn: connection failed" },
    { code: -32600, message: "thread not found: thread-1" },
    { code: -32603, message: "no active turn to steer" },
  ])("propagates unexpected RPC rejection $code: $message", async ({ code, message }) => {
    const { codex, rpc } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "turn-1" } } }));
    const steered = codex.steer("Follow up");
    rpc.handleLine(JSON.stringify({ id: 2, error: { code, message } }));
    await expect(steered).rejects.toMatchObject({ constructor: RpcRemoteError, code, message });
  });

  it("propagates transport failures even when their text resembles an expected state rejection", async () => {
    const { codex, rpc } = await session(true);
    rpc.handleLine(JSON.stringify({ method: "turn/started", params: { turn: { id: "turn-1" } } }));
    const steered = codex.steer("Follow up");
    const failure = new Error("no active turn to steer");
    rpc.failAll(failure);
    await expect(steered).rejects.toBe(failure);
  });
});
