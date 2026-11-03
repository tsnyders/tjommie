import { existsSync } from "node:fs";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { withWindowsWorker, WorkerLaunchTerminationError } from "./cli.ts";
import { CheckTerminationError, HostCheckRuntime } from "./project-checks.ts";
import { tempRoot } from "./testing.ts";

it("runs an explicit argument array with a scrubbed environment and bounded output", async () => {
  const result = await new HostCheckRuntime().run({ id: "check", label: "Synthetic check", command: process.execPath,
    args: ["-e", "if(process.env.NODE_OPTIONS || process.env.ANTHROPIC_API_KEY)process.exit(3);process.stdout.write('x'.repeat(300000))"], timeoutMs: 2000 }, tempRoot(), new AbortController().signal);
  expect(result.exitCode).toBe(0); expect(result.truncated).toBe(true); expect(result.output.length).toBeLessThanOrEqual(262_144);
});

it("never runs an already-cancelled check and reports a missing executable without echoing arguments", async () => {
  const controller = new AbortController(); controller.abort();
  const check = { id: "check", label: "Synthetic check", command: "tjommie-missing-synthetic-check", args: ["synthetic-private-argument"], timeoutMs: 1000 };
  await expect(new HostCheckRuntime().run(check, tempRoot(), controller.signal)).rejects.toThrow(/cancelled/);
  await expect(new HostCheckRuntime().run(check, tempRoot(), new AbortController().signal)).rejects.toThrow("The configured check could not start.");
});

it.each([false, true])("never falls back to owner execution when the worker launch fails (retirement unknown: %s)", async (unknown) => {
  const root = tempRoot(), marker = join(root, "owner-process-must-not-write");
  const spawn = vi.fn(async () => { throw unknown ? new WorkerLaunchTerminationError() : new Error("synthetic worker refusal"); });
  const worker = {
    purpose: "provider" as const, receipts: () => [], pairWith() {},
    required: true, spawn, configuration: () => { throw new Error("unused"); }, grantDirectory: async () => {},
    withReviewPolicy: async () => { throw new Error("unused"); }, observeReviewPolicy: async () => { throw new Error("unused"); },
  };
  const run = withWindowsWorker(worker, () => new HostCheckRuntime().run({ id: "check", label: "Synthetic check", command: process.execPath,
    args: ["-e", "require('node:fs').writeFileSync(process.argv[1], 'owner bypass')", marker], timeoutMs: 2000 }, root, new AbortController().signal));
  if (unknown) await expect(run).rejects.toBeInstanceOf(CheckTerminationError);
  else await expect(run).rejects.toThrow("protected worker runtime");
  expect(spawn).toHaveBeenCalledOnce(); expect(existsSync(marker)).toBe(false);
});
