import { randomBytes } from "node:crypto";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { cliEnv, openUrl, resolveCli, runCli, spawnCli } from "./cli.ts";

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
  it("strips endpoint overrides, secret suffixes and cloud variables while preserving ordinary CLI environment", () => {
    const denied = ["ANTHROPIC_BASE_URL", "ANTHROPIC_CUSTOM_HEADERS", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "OPENAI_BASE_URL", "OPENAI_API_BASE", "gh_token", "NPM_SECRET", "DB_PASSWORD", "DB_PASSWD", "CREDENTIAL", "CREDENTIALS", "PRIVATE_KEY", "API_KEY", "ACCESS_KEY", "THING_KEY", "AWS_REGION", "azure_location", "GOOGLE_PROJECT", "GCP_PROJECT"];
    const base = { Path: "bin", TEMP: "tmp", USERPROFILE: "profile", KEEP: "1", ...Object.fromEntries(denied.map((name) => [name, "discard"])) };
    expect(cliEnv({ CODEX_HOME: "account" }, base)).toEqual({ Path: "bin", TEMP: "tmp", USERPROFILE: "profile", KEEP: "1", CODEX_HOME: "account" });
  });
  it("strips inherited keys and config folders case-insensitively, then adds extras", () => {
    const env = cliEnv({ CLAUDE_CONFIG_DIR: "C:\\acct" }, { Path: "x", anthropic_api_key: "sk", CODEX_HOME: "h", ClaudeCode: "1", KEEP: "1" });
    expect(env).toEqual({ Path: "x", KEEP: "1", CLAUDE_CONFIG_DIR: "C:\\acct" });
  });
  it("removes daemon token variables before building a CLI environment", () => {
    const token = randomBytes(32).toString("base64url");
    const env = cliEnv({ CODEX_HOME: "account", TJOMMIE_DAEMON_TOKEN: token }, {
      PATH: "bin", TJOMMIE_DAEMON_TOKEN: token, tjommie_token: token, SESSION_SECRET: token,
    });
    expect(env).toEqual({ PATH: "bin", CODEX_HOME: "account" });
    expect(Object.values(env)).not.toContain(token);
  });
});

describe("runCli", () => {
  it("runs short commands and session commands as Node when hosted by Electron", async () => {
    Object.defineProperty(process.versions, "electron", { value: "44.4.5", configurable: true });
    try {
      const args = ["-e", "process.stdout.write(process.env.ELECTRON_RUN_AS_NODE ?? 'missing')"];
      expect((await runCli({ file: process.execPath, args: [] }, args, {})).stdout).toBe("1");
      const child = spawnCli({ file: process.execPath, args: [] }, args, { cwd: process.cwd(), env: {} });
      let output = "";
      child.stdout.on("data", (chunk: Buffer) => { output += chunk.toString(); });
      await new Promise<void>((resolve) => child.on("close", () => resolve()));
      expect(output).toBe("1");
    } finally {
      Reflect.deleteProperty(process.versions, "electron");
    }
  });
  it("captures stdout and the exit code", async () => {
    const out = await runCli({ file: process.execPath, args: [] }, ["-e", "console.log('hi'); process.exit(3)"], cliEnv({}));
    expect(out.code).toBe(3);
    expect(out.stdout.trim()).toBe("hi");
  });
  it("gives a session CLI its own stdin pipe", async () => {
    const token = randomBytes(32).toString("base64url");
    const child = spawnCli(
      { file: process.execPath, args: [] },
      ["-e", "let input = ''; process.stdin.on('data', chunk => input += chunk); process.stdin.on('end', () => process.stdout.write(JSON.stringify({ input, hasToken: 'TJOMMIE_DAEMON_TOKEN' in process.env })));"],
      { cwd: process.cwd(), env: { TJOMMIE_DAEMON_TOKEN: token } },
    );
    let output = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => { output += chunk; });
    child.stdin.end("CLI protocol only\n");
    const code = await new Promise<number | null>((resolve) => child.once("close", resolve));
    expect(code).toBe(0);
    expect(JSON.parse(output)).toEqual({ input: "CLI protocol only\n", hasToken: false });
    expect(output).not.toContain(token);
  });
});

describe("openUrl", () => {
  it("refuses anything but https", () => {
    expect(() => openUrl("file:///C:/Windows/System32/calc.exe")).toThrow(/https/);
  });
});
