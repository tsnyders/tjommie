import { spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { PassThrough } from "node:stream";
import { describe, expect, it, vi } from "vitest";

const cliMocks = vi.hoisted(() => ({
  killTree: vi.fn(),
  spawnCli: vi.fn(),
}));

vi.mock("../cli.ts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../cli.ts")>()),
  ...cliMocks,
}));

vi.mock("node:child_process", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:child_process")>();
  return { ...actual, spawnSync: vi.fn(actual.spawnSync) };
});

import { cliEnv, runCli } from "../cli.ts";
import { tempRoot } from "../testing.ts";
import { ClaudeSession, claudeArgs, claudeMode } from "./claude.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

const base: StartOptions = {
  cwd: tempRoot(),
  extraDirs: [],
  model: "sonnet",
  instructions: "Be brief.",
  autonomy: "folder",
  allowRules: [],
  resumeId: null,
  env: {},
  mcpServers: [],
};
const files = { instructions: "instructions.txt", mcp: "mcp.json" };

function fakeChild(): {
  child: ChildProcessWithoutNullStreams;
  stdin: PassThrough;
  stdout: PassThrough;
  stderr: PassThrough;
} {
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  const child = Object.assign(new EventEmitter(), {
    stdin,
    stdout,
    stderr,
    pid: 4321,
    exitCode: null,
    signalCode: null,
  }) as unknown as ChildProcessWithoutNullStreams;
  return { child, stdin, stdout, stderr };
}

describe("claudeArgs", () => {
  it("maps autonomy to Claude's permission modes", () => {
    expect([claudeMode("ask"), claudeMode("folder"), claudeMode("full")]).toEqual(["manual", "acceptEdits", "bypassPermissions"]);
  });

  it("starts a headless stream-json session with approvals over stdio and no outside MCP servers", () => {
    expect(claudeArgs(base, files)).toEqual([
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
      "--append-system-prompt-file",
      "instructions.txt",
      "--strict-mcp-config",
    ]);
  });

  it("adds extra folders, the session to resume, and saved rules last", () => {
    const args = claudeArgs({ ...base, extraDirs: ["C:\\home"], resumeId: "abc", allowRules: ["Write", "Bash(npm test *)"] }, files);
    expect(args.slice(-7)).toEqual(["--add-dir", "C:\\home", "--resume", "abc", "--allowedTools", "Write", "Bash(npm test *)"]);
  });

  it("attaches MCP servers, and allows the tools of the ones marked autoAllow", () => {
    const args = claudeArgs({
      ...base,
      allowRules: ["Write"],
      mcpServers: [
        { name: "tjommie", url: "http://127.0.0.1:47100/mcp", headers: { Authorization: "Bearer t0k" }, autoAllow: true },
        { name: "browser", command: "C:\\node.exe", args: ["cli.js", "--headless"], env: {}, autoAllow: false },
      ],
    }, files);
    expect(args[args.indexOf("--mcp-config") + 1]).toBe(files.mcp);
    expect(args.join(" ")).not.toContain("Bearer t0k");
    expect(args.slice(-3)).toEqual(["--allowedTools", "mcp__tjommie", "Write"]);
  });

  it("passes awkward instructions through process arguments unchanged (argument round trip)", async () => {
    const instructions = `Say "hi" & 'bye'\n100% ^caret <tag> | pipe naïve ✓ C:\\Users\\Thabo Mokoena`;
    const args = claudeArgs({ ...base, instructions }, files);
    const script = join(tempRoot(), "print-args.js");
    writeFileSync(script, "process.stdout.write(JSON.stringify(process.argv.slice(2)))");
    const out = await runCli({ file: process.execPath, args: [script] }, args, cliEnv({}));
    expect(JSON.parse(out.stdout)).toEqual(args);
  });
});

describe("ClaudeSession folder permissions", () => {
  it.skipIf(process.platform !== "win32")("looks up the user SID once and applies an ACL to every new session folder", async () => {
    vi.mocked(spawnSync).mockClear();
    const children = [fakeChild(), fakeChild()];
    const sessions = children.map(({ child }) => {
      cliMocks.spawnCli.mockReturnValueOnce(child);
      return new ClaudeSession({ file: "claude", args: [] }, base);
    });
    try {
      for (const claude of sessions) await claude.start();
      const commands = vi.mocked(spawnSync).mock.calls.map(([command]) => command);
      expect(commands.filter((command) => command === "whoami")).toHaveLength(1);
      expect(commands.filter((command) => command === "icacls")).toHaveLength(2);
      for (const [, , options] of vi.mocked(spawnSync).mock.calls) {
        expect(options?.stdio).toEqual(["ignore", "pipe", "pipe"]);
      }
    } finally {
      sessions.forEach((claude) => claude.close());
      children.forEach(({ child }) => child.emit("exit", 0, null));
    }
  });
});

describe("ClaudeSession activity", () => {
  it("emits no activity for tool loading and emits a plain label for the selected tool", async () => {
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);
    const tools: Extract<ProviderEvent, { type: "tool" }>[] = [];
    claude.onEvent((event) => {
      if (event.type === "tool") tools.push(event);
    });

    try {
      await claude.start();
      const assistant = (id: string, name: string, input: Record<string, unknown>) => JSON.stringify({
        type: "assistant", message: { content: [{ type: "tool_use", id, name, input }] },
      });
      stdout.write(`${assistant("search-1", "ToolSearch", { query: "select:mcp__tjommie__add_routine" })}\n`);
      expect(tools).toEqual([]);

      stdout.write(`${assistant("routine-1", "mcp__tjommie__add_routine", { name: "Morning check" })}\n`);
      expect(tools).toEqual([{ type: "tool", id: "routine-1", name: "Proposed a routine: Morning check", summary: "Proposed a routine: Morning check" }]);
    } finally {
      child.emit("exit", 0, null);
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
    }
  });
});

describe("ClaudeSession cost accounting", () => {
  it.each([
    { name: "a fresh session", resumeId: null, resumeCostUsd: undefined, total: 0.0140522, expected: 0.0140522 },
    { name: "a fresh session with an unrelated baseline", resumeId: null, resumeCostUsd: 0.0169623, total: 0.0140522, expected: 0.0140522 },
    { name: "a resumed session with its saved baseline", resumeId: "saved", resumeCostUsd: 0.0169623, total: 0.0198706, expected: 0.0029083 },
    { name: "a legacy resumed session", resumeId: "saved", resumeCostUsd: undefined, total: 0.0198706, expected: null },
    { name: "a resumed session with an unknown baseline", resumeId: "saved", resumeCostUsd: null, total: 0.0198706, expected: null },
    { name: "a replacement CLI session", resumeId: "saved", resumeCostUsd: 0.0169623, total: 0.0140522, expected: 0.0140522, initSessionId: "replacement" },
  ])("reports only new cost for $name", async ({ resumeId, resumeCostUsd, total, expected, initSessionId }) => {
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, { ...base, resumeId, resumeCostUsd });
    const usage: Extract<ProviderEvent, { type: "usage" }>[] = [];
    claude.onEvent((event) => {
      if (event.type === "usage") usage.push(event);
    });

    try {
      await claude.start();
      stdout.write(`${JSON.stringify({ type: "system", subtype: "init", session_id: initSessionId ?? resumeId ?? "fresh" })}\n`);
      stdout.write(`${JSON.stringify({ type: "result", subtype: "success", total_cost_usd: total })}\n`);
      expect(usage).toHaveLength(1);
      if (expected === null) expect(usage[0].costUsd).toBeNull();
      else expect(usage[0].costUsd).toBeCloseTo(expected, 10);
      expect(usage[0].totalCostUsd).toBe(total);

      stdout.write(`${JSON.stringify({ type: "system", subtype: "init", session_id: initSessionId ?? resumeId ?? "fresh" })}\n`);
      stdout.write(`${JSON.stringify({ type: "result", subtype: "success", total_cost_usd: total + 0.003 })}\n`);
      expect(usage).toHaveLength(2);
      expect(usage[1].costUsd).toBeCloseTo(0.003, 10);
      expect(usage[1].totalCostUsd).toBe(total + 0.003);
    } finally {
      child.emit("exit", 0, null);
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
    }
  });
});

describe("ClaudeSession fallback kills", () => {
  it("cancels the delayed close kill when that child exits", async () => {
    vi.useFakeTimers();
    cliMocks.killTree.mockClear();
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);

    try {
      await claude.start();
      claude.close();
      expect(vi.getTimerCount()).toBe(1);

      child.emit("exit", 0, null);
      expect(vi.getTimerCount()).toBe(0);
      await vi.advanceTimersByTimeAsync(5000);

      expect(cliMocks.killTree).not.toHaveBeenCalled();
    } finally {
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
      vi.useRealTimers();
    }
  });

  it("cancels the delayed interrupt kill when that child exits", async () => {
    vi.useFakeTimers();
    cliMocks.killTree.mockClear();
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);

    try {
      await claude.start();
      stdout.write(`${JSON.stringify({ type: "system", subtype: "init", session_id: "session" })}\n`);
      await claude.interrupt();
      expect(vi.getTimerCount()).toBe(1);

      child.emit("exit", 0, null);
      expect(vi.getTimerCount()).toBe(0);
      await vi.advanceTimersByTimeAsync(10_000);

      expect(cliMocks.killTree).not.toHaveBeenCalled();
    } finally {
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
      vi.useRealTimers();
    }
  });
});

describe("ClaudeSession lifecycle", () => {
  it("stores instructions and MCP config in private files and removes them when closing", async () => {
    const { child } = fakeChild();
    cliMocks.spawnCli.mockImplementationOnce((_cmd: unknown, args: string[]) => {
      const promptFile = args[args.indexOf("--append-system-prompt-file") + 1];
      expect(JSON.parse(readFileSync(join(dirname(promptFile), "owner.json"), "utf8"))).toEqual({ version: 1, ownerPid: process.pid, phase: "starting", childPid: null });
      return child;
    });
    const instructions = `Say "hi" & 'bye'\n100% ^caret <tag> | pipe`;
    const claude = new ClaudeSession({ file: "claude", args: [] }, { ...base, instructions, mcpServers: [{ name: "test", url: "http://127.0.0.1:1/mcp", headers: { Authorization: "Bearer test-only-value" }, autoAllow: true }] });
    await claude.start();
    const args = cliMocks.spawnCli.mock.lastCall![1] as string[];
    const promptFile = args[args.indexOf("--append-system-prompt-file") + 1];
    const mcpFile = args[args.indexOf("--mcp-config") + 1];
    expect(readFileSync(promptFile, "utf8")).toBe(instructions);
    expect(JSON.parse(readFileSync(mcpFile, "utf8"))).toMatchObject({ mcpServers: { test: { headers: { Authorization: "Bearer test-only-value" } } } });
    expect(args.join(" ")).not.toContain(instructions);
    expect(args.join(" ")).not.toContain("test-only-value");
    expect(JSON.parse(readFileSync(join(dirname(promptFile), "owner.json"), "utf8"))).toEqual({ version: 1, ownerPid: process.pid, phase: "started", childPid: child.pid });
    claude.close();
    expect(existsSync(promptFile)).toBe(false);
    expect(existsSync(mcpFile)).toBe(false);
    child.emit("exit", 0, null);
  });

  it("keeps managing a live child if recording its PID fails", async () => {
    const { child } = fakeChild();
    let folder = "";
    cliMocks.spawnCli.mockImplementationOnce((_cmd: unknown, args: string[]) => {
      folder = dirname(args[args.indexOf("--append-system-prompt-file") + 1]);
      mkdirSync(join(folder, "owner.json.tmp"));
      return child;
    });
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);
    try {
      await expect(claude.start()).resolves.toBeUndefined();
      expect(claude.pid()).toBe(child.pid);
      expect(JSON.parse(readFileSync(join(folder, "owner.json"), "utf8"))).toMatchObject({ phase: "starting", childPid: null });
      expect(logged).toHaveBeenCalledOnce();
    } finally {
      claude.close();
      child.emit("exit", 0, null);
      logged.mockRestore();
    }
    expect(existsSync(folder)).toBe(false);
  });
  it("does not write after close and reports the intentional exit quietly", async () => {
    cliMocks.killTree.mockClear();
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    const stdinErrors: Error[] = [];
    const exitErrors: (string | null)[] = [];
    stdin.on("error", (error: Error) => stdinErrors.push(error));
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);
    claude.onEvent((event) => {
      if (event.type === "exit") exitErrors.push(event.error);
    });

    try {
      await claude.start();
      claude.close();
      await expect(claude.send("too late")).resolves.toBeUndefined();
      await new Promise<void>((resolve) => setImmediate(resolve));
      expect(stdinErrors).toEqual([]);
      await expect(claude.steer("too late")).resolves.toBe(false);

      child.emit("exit", null, "SIGTERM");
      expect(exitErrors).toEqual([null]);
    } finally {
      child.emit("exit", 0, null);
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
    }
  });

  it("can interrupt immediately after send, before Claude emits init", async () => {
    vi.useFakeTimers();
    cliMocks.killTree.mockClear();
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    const writes: string[] = [];
    stdin.on("data", (chunk: Buffer) => writes.push(chunk.toString()));
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);

    try {
      await claude.start();
      await claude.send("start");
      await claude.interrupt();

      expect(writes.join("")).toContain('"subtype":"interrupt"');
      expect(vi.getTimerCount()).toBe(1);
      child.emit("exit", 0, null);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
      vi.useRealTimers();
    }
  });

  it("reports the signal when an unexpected exit has no code", async () => {
    cliMocks.killTree.mockClear();
    cliMocks.spawnCli.mockReset();
    const { child, stdin, stdout, stderr } = fakeChild();
    const exitErrors: (string | null)[] = [];
    cliMocks.spawnCli.mockReturnValue(child);
    const claude = new ClaudeSession({ file: "claude", args: [] }, base);
    claude.onEvent((event) => {
      if (event.type === "exit") exitErrors.push(event.error);
    });

    try {
      await claude.start();
      child.emit("exit", null, "SIGTERM");
      expect(exitErrors).toEqual(["Claude exited with signal SIGTERM"]);
    } finally {
      stdin.destroy();
      stdout.destroy();
      stderr.destroy();
    }
  });
});


describe("Claude API accounts", () => {
  it("passes only the positive remaining local allowance as a provider estimate threshold", () => {
    expect(claudeArgs({ ...base, maxBudgetUsd: 0.25 }, files)).toContain("--max-budget-usd");
    expect(claudeArgs({ ...base, maxBudgetUsd: 0.25 }, files)).toContain("0.25");
    for (const value of [0, -1, NaN, Infinity]) expect(() => claudeArgs({ ...base, maxBudgetUsd: value }, files)).toThrow("remaining");
  });
  it("keeps API authentication out of argv/files and redacts provider errors while deduplicating results", async () => {
    const { child, stdout, stderr } = fakeChild(); cliMocks.spawnCli.mockReturnValueOnce(child);
    const session = new ClaudeSession({ file: "claude", args: [] }, { ...base, authentication: { kind: "api_key", key: "synthetic-private-key" }, maxBudgetUsd: 1 });
    const events: ProviderEvent[] = []; session.onEvent((event) => events.push(event));
    try {
      await session.start();
      const call = cliMocks.spawnCli.mock.calls.at(-1)!;
      expect(JSON.stringify(call[1])).not.toContain("synthetic-private-key");
      expect(call[2]).toMatchObject({ claudeApiKey: "synthetic-private-key" });
      const result = { type: "result", uuid: "result-one", subtype: "success", total_cost_usd: 0.1 };
      stdout.write(`${JSON.stringify(result)}\n`); stdout.write(`${JSON.stringify(result)}\n`);
      expect(events.filter((event) => event.type === "usage")).toHaveLength(1);
      stderr.write("Rejected synthetic-private-key"); child.emit("exit", 1, null);
      expect(JSON.stringify(events)).not.toContain("synthetic-private-key");
      expect(events.at(-1)).toMatchObject({ type: "exit", error: "Rejected [redacted]" });
    } finally { child.emit("exit", 0, null); session.close(); }
  });
});
