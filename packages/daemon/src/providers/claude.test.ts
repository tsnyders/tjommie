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

  it("passes awkward instructions through process arguments unchanged (argument round trip)", async () => {
    const instructions = `Say "hi" & 'bye'\n100% ^caret <tag> | pipe naïve ✓ C:\\Users\\Thabo Mokoena`;
    const args = claudeArgs({ ...base, instructions });
    const script = join(mkdtempSync(join(tmpdir(), "tjommie args ñ-")), "print-args.js");
    writeFileSync(script, "process.stdout.write(JSON.stringify(process.argv.slice(2)))");
    const out = await runCli({ file: process.execPath, args: [script] }, args, cliEnv({}));
    expect(JSON.parse(out.stdout)).toEqual(args);
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
