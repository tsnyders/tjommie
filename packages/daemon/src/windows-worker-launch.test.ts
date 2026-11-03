import { spawn } from "node:child_process";
import { runInNewContext } from "node:vm";
import { expect, it } from "vitest";
import { tempRoot } from "./testing.ts";
import { WINDOWS_WORKER_BOOTSTRAP, workerLaunchFrame, workerLaunchEnvironment, windowsWorkerCommandLine, WindowsWorkerBoundary } from "./windows-worker.ts";

it("transports long Windows argv privately while keeping the logon command below its separate limit", () => {
  const file = "C:\\ProgramData\\Tjommie\\Worker-123456789012345678901234\\runtime\\node.exe";
  const args = ["-e", "/* synthetic */" + "x".repeat(5000), "", "quoted \"value\"", "line\nvalue", "C:\\trailing slash\\", "English \ud83e\udd16"];
  const frame = workerLaunchFrame(file, args);
  expect(frame.readUInt32LE()).toBe(frame.length - 4);
  expect(JSON.parse(frame.subarray(4).toString("utf8"))).toEqual({ file, args });
  expect(windowsWorkerCommandLine(file, args).length).toBeGreaterThan(1024);
  expect(windowsWorkerCommandLine(file, ["-e", WINDOWS_WORKER_BOOTSTRAP]).length + 1).toBeLessThanOrEqual(1024);
});

it("counts UTF16 code units and Windows quoting including the final terminator", () => {
  const file = "C:\\runtime\\node.exe";
  const remaining = 32766 - file.length - 1;
  expect(() => workerLaunchFrame(file, ["x".repeat(remaining)])).not.toThrow();
  expect(() => workerLaunchFrame(file, ["x".repeat(remaining + 1)])).toThrow("launch limit");
  expect(windowsWorkerCommandLine(file, ["", 'a "b"', "space\\", "\ud83e\udd16"])).toBe(`${file} "" "a \\\"b\\\"" space\\ \ud83e\udd16`);
  expect(() => workerLaunchFrame(file, ["\ud83e\udd16".repeat(Math.floor(remaining / 2) + 1)])).toThrow("launch limit");
});

it("rejects NUL and injected environment entries before configuration, native ACL or credential access", async () => {
  const worker = new WindowsWorkerBoundary({ configurationPath: "does-not-exist", exclusive: () => false });
  for (const [file, args, env] of [
    ["node.exe\0other", [], {}], ["node.exe", ["hello\0world"], {}],
    ["node.exe", [], { harmless: "x\0NODE_OPTIONS=--require bad" }], ["node.exe", [], { "bad\0key": "x" }],
  ] as const) await expect(worker.spawn(file, [...args], { cwd: "unused", env })).rejects.toThrow("launch input");
  expect(() => workerLaunchEnvironment({ "bad=key": "x" })).toThrow("launch input");
  expect(workerLaunchEnvironment({ NODE_OPTIONS: "--require synthetic", node_path: "synthetic", Node_Compile_Cache: "synthetic", ELECTRON_RUN_AS_NODE: "1", SYNTHETIC_ALLOWED: "yes" })).toEqual({ SYNTHETIC_ALLOWED: "yes" });
});

function interpreted(frame: Buffer, step = 1, spawnFailure = false) {
  let offset = 0, calls = 0;
  const process = { exitCode: 0 };
  runInNewContext(WINDOWS_WORKER_BOOTSTRAP, { Buffer, process, require: (name: string): unknown => {
    if (name === "node:fs") return { readSync: (_fd: number, target: Buffer, start: number, length: number) => {
      const count = Math.min(step, length, frame.length - offset); frame.copy(target, start, offset, offset + count); offset += count; return count;
    } };
    if (name === "node:child_process") return { spawnSync: () => { calls++; if (spawnFailure) throw new Error("synthetic private launch details"); return { status: 17 }; } };
    throw new Error("Unexpected bootstrap dependency.");
  } }, { timeout: 1000 });
  return { offset, calls, status: process.exitCode };
}

it("consumes only the framed launch bytes across partial reads and keeps provider stdin untouched", () => {
  const frame = workerLaunchFrame("C:\\runtime\\node.exe", ["synthetic", "\ud83e\udd16"]), tail = Buffer.from([0, 1, 255, 10, 13, 42]);
  expect(interpreted(Buffer.concat([frame, tail]))).toEqual({ offset: frame.length, calls: 1, status: 17 });
  for (const prefix of [frame.subarray(0, 2), frame.subarray(0, frame.length - 1), Buffer.from([255, 255, 255, 127]), Buffer.from([1, 0, 0, 0, 123])]) {
    expect(interpreted(prefix).calls).toBe(0); expect(interpreted(prefix).status).toBe(71);
  }
  expect(interpreted(frame, 7, true).status).toBe(71);
});

it("runs the actual fixed bootstrap with long synthetic argv, binary stdin, inherited stdio and exit status", async () => {
  const cwd = tempRoot(), args = ["x".repeat(6000), "", 'quoted "value"', "line\nvalue", "\ud83e\udd16"], input = Buffer.from([0, 255, 10, 13, 3, 4, 5]);
  const script = "let b=[];process.stdin.on('data',v=>b.push(v));process.stdin.on('end',()=>{process.stdout.write(JSON.stringify({args:process.argv.slice(1),stdin:Buffer.concat(b).toString('base64'),cwd:process.cwd(),marker:process.env.TJOMMIE_SYNTHETIC}));process.stderr.write('synthetic-stderr');process.exitCode=17})";
  const frame = workerLaunchFrame(process.execPath, ["-e", script, "--", ...args]);
  const child = spawn(process.execPath, ["-e", WINDOWS_WORKER_BOOTSTRAP], { cwd, env: { SystemRoot: process.env.SystemRoot ?? "C:\\Windows", TJOMMIE_SYNTHETIC: "yes" }, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  let stdout = "", stderr = "";
  const closed = new Promise<number | null>((done, reject) => { child.once("error", reject); child.once("close", done); });
  child.stdout.on("data", (value: Buffer) => { stdout += value.toString("utf8"); }); child.stderr.on("data", (value: Buffer) => { stderr += value.toString("utf8"); });
  child.stdin.write(frame.subarray(0, 1)); child.stdin.write(frame.subarray(1, 3)); child.stdin.write(frame.subarray(3)); child.stdin.end(input);
  expect(await closed).toBe(17); expect(stderr).toBe("synthetic-stderr");
  expect(JSON.parse(stdout)).toEqual({ args, stdin: input.toString("base64"), cwd, marker: "yes" });
}, 15_000);
