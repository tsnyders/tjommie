import { spawn } from "node:child_process";
import { beforeEach, expect, it, vi } from "vitest";
import { launchCli, observeWorkerPolicy, waitForCliExit, workerAccountsRoot, WorkerLaunchTerminationError } from "./cli.ts";
import { runReviewCli, type ReviewCliOptions } from "./project-review-cli.ts";
import { CheckTerminationError } from "./project-checks.ts";
import { tempRoot } from "./testing.ts";
import type { ProviderEvent } from "./providers/types.ts";
import { reviewMachinePolicyHash } from "./project-review-policy.ts";

vi.mock("./cli.ts", async (original) => ({ ...await original<typeof import("./cli.ts")>(), launchCli: vi.fn(), waitForCliExit: vi.fn(), workerAccountsRoot: vi.fn(), observeWorkerPolicy: vi.fn() }));
beforeEach(() => { vi.mocked(launchCli).mockReset(); vi.mocked(waitForCliExit).mockReset().mockResolvedValue(); vi.mocked(workerAccountsRoot).mockReset().mockReturnValue(null); vi.mocked(observeWorkerPolicy).mockReset().mockResolvedValue(null); });
const options = (events: ProviderEvent[]): ReviewCliOptions => ({ command: { file: process.execPath, args: [] }, args: [], cwd: tempRoot(), env: {}, input: "owned fixture", model: "test", signal: new AbortController().signal, onEvent: (event) => events.push(event) });

it("records unknown spending and retains the lease when a paid worker launch cannot prove retirement", async () => {
  const events: ProviderEvent[] = []; vi.mocked(launchCli).mockRejectedValue(new WorkerLaunchTerminationError());
  await expect(runReviewCli(options(events))).rejects.toBeInstanceOf(CheckTerminationError);
  expect(events).toMatchObject([{ type: "usage", model: "test", costUsd: null }]);
  expect(events[0].type === "usage" && events[0].eventId).toMatch(/^[a-f0-9-]+:unknown-launch$/);
});

it("does not invent a charge when the launcher proves the child never started", async () => {
  const events: ProviderEvent[] = []; vi.mocked(launchCli).mockRejectedValue(new Error("Missing synthetic executable"));
  await expect(runReviewCli(options(events))).rejects.toThrow("Missing synthetic executable"); expect(events).toEqual([]);
});

it("waits for worker job proof, and records unknown spending when output has no final usage", async () => {
  const events: ProviderEvent[] = [], opts = options(events);
  vi.mocked(launchCli).mockImplementation(async () => spawn(process.execPath, ["-e", "process.stdin.resume();process.stdin.on('end',()=>{})"], { cwd: opts.cwd, env: {}, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] }));
  vi.mocked(waitForCliExit).mockRejectedValue(new WorkerLaunchTerminationError());
  await expect(runReviewCli(opts)).rejects.toBeInstanceOf(CheckTerminationError);
  expect(waitForCliExit).toHaveBeenCalledTimes(1); expect(events).toMatchObject([{ type: "usage", costUsd: null }]);
});

it.skipIf(process.platform !== "win32")("does not reuse owner policy evidence for a different worker SID", async () => {
  vi.mocked(workerAccountsRoot).mockReturnValue("C:\\synthetic-worker\\accounts");
  await expect(reviewMachinePolicyHash()).rejects.toThrow("review_worker_policy_observer_unavailable");
});
