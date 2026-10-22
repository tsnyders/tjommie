import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { EventEmitter } from "node:events";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { PassThrough } from "node:stream";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cliEnv, killTree, killTreeAndWait, openUrl, resolveCli, runCli, spawnCli } from "./cli.ts";

vi.mock("node:child_process", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:child_process")>();
  return { ...actual, spawn: vi.fn(actual.spawn) };
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(spawn).mockClear();
  vi.useRealTimers();
});

const dir = () => mkdtempSync(join(tmpdir(), "tjommie cli ñ-"));

describe("resolveCli", () => {
  const codexInstall = (arch: "x64" | "arm64", layout: "nested" | "hoisted" | "legacy") => {
    const bin = dir();
    const root = join(bin, "node_modules", "@openai", "codex");
    const entry = join(root, "bin", "codex.js");
    const triple = arch === "x64" ? "x86_64-pc-windows-msvc" : "aarch64-pc-windows-msvc";
    const vendor = layout === "legacy" ? join(root, "vendor") : join(layout === "nested" ? root : bin, "node_modules", "@openai", `codex-win32-${arch}`, "vendor");
    const native = join(vendor, triple, layout === "legacy" ? "codex" : "bin", "codex.exe");
    mkdirSync(join(entry, ".."), { recursive: true });
    mkdirSync(join(native, ".."), { recursive: true });
    writeFileSync(entry, "");
    writeFileSync(native, "");
    if (layout !== "legacy") writeFileSync(join(vendor, "..", "package.json"), "{}");
    return { bin, root, entry, native, vendor, triple };
  };

  it.each(["x64", "arm64"] as const)("runs the npm native Windows %s binary without its console-opening wrapper", (arch) => {
    const install = codexInstall(arch, "nested");
    expect(resolveCli("codex", { Path: install.bin }, { platform: "win32", arch })).toEqual({
      file: install.native, args: [], codexPackage: { root: install.root },
    });
  });

  it("resolves a hoisted optional platform package relative to the npm entry", () => {
    const install = codexInstall("x64", "hoisted");
    expect(resolveCli("codex", { PATH: install.bin }, { platform: "win32", arch: "x64" })?.file).toBe(install.native);
  });

  it("supports the legacy vendor executable and its bundled helper directory", () => {
    const install = codexInstall("x64", "legacy");
    const pathDir = join(install.vendor, install.triple, "path");
    mkdirSync(pathDir);
    expect(resolveCli("codex", { PATH: install.bin }, { platform: "win32", arch: "x64" })).toEqual({
      file: install.native, args: [], codexPackage: { root: install.root, pathDir },
    });
  });

  it.each([{ platform: "win32", arch: "ia32" }, { platform: "linux", arch: "x64" }] as const)("keeps the wrapper for unsupported native resolution on $platform/$arch", (runtime) => {
    const install = codexInstall("x64", "nested");
    expect(resolveCli("codex", { PATH: install.bin }, runtime)).toEqual({ file: process.execPath, args: [install.entry] });
  });

  it("does not run a bundled executable for the wrong architecture", () => {
    const install = codexInstall("arm64", "nested");
    expect(resolveCli("codex", { PATH: install.bin }, { platform: "win32", arch: "x64" })).toEqual({ file: process.execPath, args: [install.entry] });
  });

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
  it.each(["status", "session"])("preserves the native npm environment for %s launches without inherited managed markers or duplicate PATH keys", async (mode) => {
    const cmd = { file: "C:\\native\\codex.exe", args: [], codexPackage: { root: "C:\\npm\\codex", pathDir: "C:\\npm\\vendor\\path" } };
    const env = { Path: "C:\\tools", CODEX_HOME: "C:\\owned-account", CODEX_MANAGED_PACKAGE_ROOT: "unrelated-install", codex_managed_by_bun: "1", CODEX_MANAGED_BY_PNPM: "1", OPENAI_API_KEY: "secret" };
    const child = Object.assign(new EventEmitter(), { stdout: new PassThrough(), stderr: new PassThrough() });
    vi.mocked(spawn).mockReturnValueOnce(child as unknown as ReturnType<typeof spawn>);
    const result = mode === "status" ? runCli(cmd, ["login", "status"], env) : spawnCli(cmd, ["app-server"], { cwd: dir(), env });
    expect(spawn).toHaveBeenLastCalledWith(cmd.file, mode === "status" ? ["login", "status"] : ["app-server"], expect.objectContaining({ windowsHide: true, env: {
      PATH: `C:\\npm\\vendor\\path${delimiter}C:\\tools`, CODEX_HOME: "C:\\owned-account",
      CODEX_MANAGED_PACKAGE_ROOT: "C:\\npm\\codex", CODEX_MANAGED_BY_NPM: "1",
    } }));
    if (mode === "status") {
      child.emit("close", 0);
      await result;
    }
  });

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

describe("killTreeAndWait", () => {
  it("rejects a missing process identity", async () => {
    await expect(killTreeAndWait(undefined)).rejects.toThrow("process ID");
  });

  describe.skipIf(process.platform !== "win32")("nonzero taskkill reports", () => {
    const success = (pid: number, parent: number) => `SUCCESS: The process with PID ${pid} (child process of PID ${parent}) has been terminated.\r\n`;
    const failure = (pid: number, parent: number) => `ERROR: The process with PID ${pid} (child process of PID ${parent}) could not be terminated.\r\nReason: The operation attempted is not supported.\r\r\n`;
    const stdout = success(200, 100) + success(100, process.pid);
    const stderr = failure(400, 300) + failure(300, 200);
    const gone = () => { throw Object.assign(new Error("process gone"), { code: "ESRCH" }); };
    const taskkill = (out = stdout, err = stderr, code: number | null = 128) => {
      const child = Object.assign(new EventEmitter(), { stdout: new PassThrough(), stderr: new PassThrough() });
      vi.mocked(spawn).mockReturnValueOnce(child as unknown as ReturnType<typeof spawn>);
      const stopped = killTreeAndWait(100);
      child.stdout.end(out);
      child.stderr.end(err);
      child.emit("close", code);
      return stopped;
    };

    it.each([128, 255])("verifies every reported PID after unsupported sandbox descendants (exit %s)", async (code) => {
      const probe = vi.spyOn(process, "kill").mockImplementation(gone);
      await expect(taskkill(stdout, stderr, code)).resolves.toBeUndefined();
      expect(new Set(probe.mock.calls.map(([pid]) => pid))).toEqual(new Set([100, 200, 300, 400]));
      expect(probe.mock.calls.every(([, signal]) => signal === 0)).toBe(true);
      expect(spawn).toHaveBeenCalledTimes(1);
    });

    it("waits for the reported descendants to disappear", async () => {
      vi.useFakeTimers();
      let childAlive = true;
      vi.spyOn(process, "kill").mockImplementation((pid) => pid === 400 && childAlive ? true : gone());
      let settled = false;
      const stopped = taskkill().then(() => { settled = true; }, (error: unknown) => error);
      await vi.advanceTimersByTimeAsync(200);
      expect(settled).toBe(false);
      childAlive = false;
      await vi.advanceTimersByTimeAsync(200);
      expect(await stopped).toBeUndefined();
    });

    it.each(["running", "EPERM", "unknown"])("rejects when a reported PID remains %s even though the root is gone", async (state) => {
      vi.useFakeTimers();
      vi.spyOn(process, "kill").mockImplementation((pid) => {
        if (pid !== 400) return gone();
        if (state === "running") return true; // also protects against a reused PID
        throw Object.assign(new Error("probe failed"), state === "EPERM" ? { code: "EPERM" } : {});
      });
      const rejected = expect(taskkill()).rejects.toMatchObject({ cause: { pid: 100, code: 128, stdout, stderr } });
      await vi.runAllTimersAsync();
      await rejected;
    });

    it.each([
      ["omitted root", success(200, 100), stderr],
      ["unowned root", success(999, 900), ""],
      ["wrong root parent", success(200, 100) + success(100, process.pid + 1), stderr],
      ["unknown line", stdout, `${stderr}Unknown termination error\n`],
      ["missing error reason", stdout, "ERROR: The process with PID 400 (child process of PID 300) could not be terminated.\n"],
      ["disconnected child", stdout + success(500, 600), stderr],
      ["cyclic ancestry", stdout + success(500, 600) + success(600, 500), stderr],
      ["contradictory parent", stdout + success(200, 900), stderr],
      ["already absent root without a tree", "", 'ERROR: The process "100" not found.\n'],
      ["failed root without enumeration proof", "", failure(100, process.pid)],
      ["truncated stdout", `${stdout}${"\n".repeat(70_000)}`, stderr],
      ["truncated stderr", stdout, `${stderr}${"\n".repeat(70_000)}`],
    ])("rejects a report with %s", async (_label, out, err) => {
      const probe = vi.spyOn(process, "kill").mockImplementation(gone);
      await expect(taskkill(out, err)).rejects.toThrow("Could not stop");
      expect(probe).not.toHaveBeenCalled();
    });

    it("does not recover when taskkill itself was interrupted", async () => {
      const probe = vi.spyOn(process, "kill").mockImplementation(gone);
      await expect(taskkill(stdout, stderr, null)).rejects.toThrow("Could not stop");
      expect(probe).not.toHaveBeenCalled();
    });

    it("retains a bounded diagnostic prefix and explicit truncation flags", async () => {
      await expect(taskkill(stdout + "\n".repeat(70_000), stderr)).rejects.toMatchObject({ cause: {
        pid: 100, code: 128, stdout: (stdout + "\n".repeat(70_000)).slice(0, 65_536),
        stderr, stdoutTruncated: true, stderrTruncated: false,
      } });
    });

    it("waits for the owned root to finish exiting before probing descendants", async () => {
      vi.useFakeTimers();
      let rootAlive = true;
      const probe = vi.spyOn(process, "kill").mockImplementation((pid) => pid === 100 && rootAlive ? true : gone());
      let settled = false;
      const stopped = taskkill().then(() => { settled = true; }, (error: unknown) => error);
      await vi.advanceTimersByTimeAsync(200);
      expect(settled).toBe(false);
      expect(probe.mock.calls.every(([pid, signal]) => pid === 100 && signal === 0)).toBe(true);
      rootAlive = false;
      await vi.advanceTimersByTimeAsync(100);
      expect(await stopped).toBeUndefined();
      expect(new Set(probe.mock.calls.map(([pid]) => pid))).toEqual(new Set([100, 200, 300, 400]));
    });

    it.each(["running", "EPERM"])("never probes descendants while the owned root remains %s", async (state) => {
      vi.useFakeTimers();
      const probe = vi.spyOn(process, "kill").mockImplementation(() => {
        if (state === "running") return true;
        throw Object.assign(new Error("probe denied"), { code: "EPERM" });
      });
      const rejected = expect(taskkill()).rejects.toThrow("Could not stop");
      await vi.runAllTimersAsync();
      await rejected;
      expect(probe.mock.calls.every(([pid, signal]) => pid === 100 && signal === 0)).toBe(true);
    });

    it("keeps clean exit-zero behavior unchanged", async () => {
      const probe = vi.spyOn(process, "kill").mockReturnValue(true);
      await expect(taskkill("", "", 0)).resolves.toBeUndefined();
      expect(probe).not.toHaveBeenCalled();
    });
  });

  it.skipIf(process.platform !== "win32")("terminates an owned parent and descendant with an empty PATH", async () => {
    const script = "const { spawn } = require('node:child_process'); const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore', windowsHide: true }); console.log(child.pid); setInterval(() => {}, 1000);";
    const parent = spawnCli({ file: process.execPath, args: [] }, ["-e", script], { cwd: dir(), env: cliEnv({}) });
    const exited = new Promise<void>((resolve) => parent.once("exit", () => resolve()));
    try {
      const descendant = await new Promise<number>((resolve, reject) => {
        parent.once("error", reject);
        parent.stdout.once("data", (chunk: Buffer) => resolve(Number(chunk.toString().trim())));
      });
      expect(Number.isSafeInteger(descendant) && descendant > 0).toBe(true);
      const originalPath = process.env.PATH;
      try {
        process.env.PATH = "";
        await killTreeAndWait(parent.pid);
      } finally {
        if (originalPath === undefined) delete process.env.PATH;
        else process.env.PATH = originalPath;
      }
      await exited;
      expect(() => process.kill(parent.pid!, 0)).toThrow();
      expect(() => process.kill(descendant, 0)).toThrow();
    } finally {
      if (parent.exitCode === null && parent.signalCode === null) killTree(parent.pid);
      await exited;
    }
  });
});
