import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./codex-rpc.ts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./codex-rpc.ts")>()),
  startCodexServer: vi.fn(),
}));

import { JsonRpcLines, RpcRemoteError, startCodexServer } from "./codex-rpc.ts";
import { CodexSession } from "./codex.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

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

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  vi.clearAllMocks();
});

async function session(autoAllow: boolean) {
  const sent: unknown[] = [];
  const rpc = new JsonRpcLines((line) => {
    const message = JSON.parse(line) as { id: number | string; method?: string };
    if (message.method === "thread/start") {
      rpc.handleLine(JSON.stringify({ id: message.id, result: { thread: { id: "thread-1" }, model: "test-model" } }));
    } else {
      sent.push(message);
    }
  });
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  const child = Object.assign(new EventEmitter(), { stdin, stdout, stderr }) as unknown as ChildProcessWithoutNullStreams;
  const close = () => {
    rpc.failAll(new Error("Test session closed"));
    stdin.destroy();
    stdout.destroy();
    stderr.destroy();
  };
  vi.mocked(startCodexServer).mockResolvedValueOnce({ rpc, child, close });
  const codex = new CodexSession({ file: "codex", args: [] }, {
    ...options,
    mcpServers: [{ name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: {}, autoAllow }],
  });
  cleanups.push(() => codex.close());
  const events: ProviderEvent[] = [];
  codex.onEvent((event) => events.push(event));
  await codex.start();
  events.length = 0;
  return { codex, rpc, sent, events };
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
