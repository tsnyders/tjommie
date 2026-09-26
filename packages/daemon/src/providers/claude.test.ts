import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

import { cliEnv, runCli } from "../cli.ts";
import { ClaudeSession, claudeArgs, claudeMode } from "./claude.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

const base: StartOptions = {
  cwd: "C:\\bots\\b1",
  extraDirs: [],
  model: "sonnet",
  instructions: "Be brief.",
  autonomy: "folder",
  allowRules: [],
  resumeId: null,
  env: {},
  mcpServers: [],
};

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

  it("passes awkward instructions through process arguments unchanged (argument round trip)", async () => {
    const instructions = `Say "hi" & 'bye'\n100% ^caret <tag> | pipe naïve ✓ C:\\Users\\Thabo Mokoena`;
    const args = claudeArgs({ ...base, instructions });
    const script = join(mkdtempSync(join(tmpdir(), "tjommie args ñ-")), "print-args.js");
    writeFileSync(script, "process.stdout.write(JSON.stringify(process.argv.slice(2)))");
    const out = await runCli({ file: process.execPath, args: [script] }, args, cliEnv({}));
    expect(JSON.parse(out.stdout)).toEqual(args);
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
