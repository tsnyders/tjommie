import { EventEmitter } from "node:events";
import { dirname, join } from "node:path";
import { expect, it, vi } from "vitest";
import { windowsAclDriver } from "./windows-acl.ts";

const process = vi.hoisted(() => ({
  inputs: [] as string[],
  commands: [] as Array<{ file: string; env: NodeJS.ProcessEnv | undefined }>,
  results: [] as Array<{ status: number | null; stdout: string; stderr: string; error?: Error }>,
  spawnChild: null as (() => unknown) | null,
}));

vi.mock("node:child_process", () => ({
  spawnSync: (file: string, _args: string[], options: { input: string; env?: NodeJS.ProcessEnv }) => {
    process.commands.push({ file, env: options.env });
    process.inputs.push(options.input);
    return process.results.shift();
  },
  spawn: (file: string, _args: string[], options: { env?: NodeJS.ProcessEnv }) => {
    process.commands.push({ file, env: options.env });
    if (!process.spawnChild) throw new Error("Async spawn is not configured in this test");
    return process.spawnChild();
  },
}));

it("uses Windows PowerShell's built-in modules for both passes despite an inherited incompatible module path", async () => {
  const originalModulePath = globalThis.process.env.PSModulePath;
  vi.stubEnv("PSModulePath", "C:\\PowerShell7\\Modules");
  vi.stubEnv("TJOMMIE_ACL_ENV_SENTINEL", "preserved");
  const inheritedModulePath = globalThis.process.env.PSModulePath;
  const result = { sid: null, groupMissing: true, failures: [], applied: 0 };
  const child = new EventEmitter();
  const stdout = Object.assign(new EventEmitter(), { setEncoding: () => stdout });
  const stderr = Object.assign(new EventEmitter(), { setEncoding: () => stderr });
  Object.assign(child, {
    stdout, stderr,
    stdin: { end: () => { stdout.emit("data", JSON.stringify(result)); child.emit("close", 0); } },
    kill: () => true,
  });
  process.commands.length = 0;
  process.results.push({ status: 0, stdout: JSON.stringify(result), stderr: "" });
  process.spawnChild = () => child;
  try {
    expect(windowsAclDriver.runSync([])).toEqual(result);
    expect(await windowsAclDriver.run([])).toEqual(result);
    expect(process.commands).toHaveLength(2);
    for (const command of process.commands) {
      expect(command.env).toBeDefined();
      expect(command.env?.PSModulePath).toBe(join(dirname(command.file), "Modules"));
      expect(Object.keys(command.env ?? {}).filter((key) => key.toLowerCase() === "psmodulepath")).toEqual(["PSModulePath"]);
      expect(command.env?.TJOMMIE_ACL_ENV_SENTINEL).toBe("preserved");
    }
    expect(globalThis.process.env.PSModulePath).toBe(inheritedModulePath);
  } finally {
    process.spawnChild = null;
    vi.unstubAllEnvs();
  }
  expect(globalThis.process.env.PSModulePath).toBe(originalModulePath);
});

it("resolves the live sandbox group SID on every ACL pass", () => {
  process.inputs.length = 0;
  process.results.push(
    { status: 0, stdout: JSON.stringify({ sid: "S-1-5-21-1-2-3-1001", groupMissing: false, failures: [], applied: 0 }), stderr: "" },
    { status: 0, stdout: JSON.stringify({ sid: "S-1-5-21-1-2-3-2002", groupMissing: false, failures: [], applied: 0 }), stderr: "" },
  );
  expect(windowsAclDriver.runSync([]).sid).toBe("S-1-5-21-1-2-3-1001");
  expect(windowsAclDriver.runSync([]).sid).toBe("S-1-5-21-1-2-3-2002");
  expect(process.inputs.map((input) => JSON.parse(input) as Record<string, unknown>)).toEqual([
    expect.not.objectContaining({ sid: expect.any(String) }),
    expect.not.objectContaining({ sid: expect.any(String) }),
  ]);
});

it("reports a synchronous ACL pass timeout as a timeout", () => {
  process.results.push({ status: null, stdout: "", stderr: "", error: Object.assign(new Error("Process timed out"), { code: "ETIMEDOUT" }) });
  expect(() => windowsAclDriver.runSync([])).toThrow("PowerShell ACL pass timed out");
});

it("reports an asynchronous ACL pass timeout as a timeout", async () => {
  const child = new EventEmitter();
  const stdout = Object.assign(new EventEmitter(), { setEncoding: () => stdout });
  const stderr = Object.assign(new EventEmitter(), { setEncoding: () => stderr });
  Object.assign(child, {
    stdout, stderr, stdin: { end: () => {} },
    kill: () => { child.emit("close", null); return true; },
  });
  process.spawnChild = () => child;
  vi.useFakeTimers();
  try {
    const result = windowsAclDriver.run([]).then(() => null, (error: unknown) => error);
    await vi.advanceTimersByTimeAsync(60_000);
    const error = await result;
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toContain("PowerShell ACL pass timed out");
  } finally {
    process.spawnChild = null;
    vi.useRealTimers();
  }
});
