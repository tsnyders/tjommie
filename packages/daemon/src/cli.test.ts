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
