import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cliEnv, installWindowsWorker, launchCli, prepareProviderDirectories, providerAccountFolder, resolveCli, runCli, spawnCli, withWindowsWorker, withWorkerPolicy, workerAccountsRoot, workerCliExclusive, WorkerLaunchTerminationError } from "./cli.ts";
import { tempRoot } from "./testing.ts";

vi.mock("node:child_process", () => ({ spawn: vi.fn(() => { throw new Error("Legacy provider execution is forbidden in this synthetic worker fixture."); }) }));
afterEach(() => vi.restoreAllMocks());
function fixture() {
  const root = tempRoot(), workerRoot = join(root, "worker"), reviewRoot = join(root, "review"), runtimeRoot = join(root, "runtime"), ownerDataRoot = join(root, "owner");
  for (const folder of [workerRoot, reviewRoot, runtimeRoot, ownerDataRoot]) mkdirSync(folder);
  const child = Object.assign(new EventEmitter(), { stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough() }) as unknown as ChildProcessWithoutNullStreams;
  const receipt = { purpose: "review" as const, workerSid: "S-1-5-21-1-2-3-1003", providerSid: "S-1-5-21-1-2-3-1002", configurationSha256: "c".repeat(64), observerSha256: "a".repeat(64), policySha256: "b".repeat(64) };
  const make = (purpose: "provider" | "review"): Parameters<typeof withWindowsWorker>[0] => ({
    purpose, required: true, pairWith: () => {}, receipts: () => [],
    configuration: () => ({ version: 2, purpose, workerRoot: purpose === "provider" ? workerRoot : reviewRoot, peerRoot: purpose === "provider" ? reviewRoot : workerRoot, runtimeRoot, ownerDataRoot, ownerSid: "S-1-5-21-1-2-3-1001", workerSid: purpose === "provider" ? receipt.providerSid : receipt.workerSid, peerSid: purpose === "provider" ? receipt.workerSid : receipt.providerSid, workerName: purpose === "provider" ? "TjWorker-12345678" : "TjReview-12345678", provisionedAt: "2026-11-03T00:00:00.000Z", credentialTarget: "Tjommie/" + "a".repeat(24) + (purpose === "provider" ? "/isolated-worker" : "/isolated-review") }),
    spawn: vi.fn(async () => child), grantDirectory: vi.fn(async () => {}),
    withReviewPolicy: async (action) => action(receipt), observeReviewPolicy: async () => receipt,
  });
  return { root, workerRoot, reviewRoot, runtimeRoot, ownerDataRoot, worker: make("provider"), review: make("review"), child };
}

describe("provider worker launch scope", () => {
  it("maps accounts into fresh worker folders, routes launches and grants, and strips inherited keys", async () => {
    const f = fixture(), ownerAccount = join(f.ownerDataRoot, "accounts", "codex-main");
    await withWindowsWorker(f.worker, async () => {
      const account = providerAccountFolder(ownerAccount);
      expect(account.startsWith(join(f.workerRoot, "accounts"))).toBe(true);
      expect(providerAccountFolder(account)).toBe(account);
      expect(workerAccountsRoot()).toBe(join(f.workerRoot, "accounts"));
      const env = cliEnv({ CODEX_HOME: ownerAccount }, { Path: "bin", OPENAI_API_KEY: "synthetic-inherited-key", TJOMMIE_DAEMON_TOKEN: "synthetic-host-token" });
      expect(await launchCli({ file: join(f.runtimeRoot, "codex.exe"), args: [] }, ["--version"], { cwd: ownerAccount, env })).toBe(f.child);
      expect(f.worker.spawn).toHaveBeenCalledWith(join(f.runtimeRoot, "codex.exe"), ["--version"], { cwd: account, env: { Path: "bin", CODEX_HOME: account } });
      await prepareProviderDirectories(account); expect(f.worker.grantDirectory).toHaveBeenCalledWith(account);
      expect(() => spawnCli({ file: "never.exe", args: [] }, [], { cwd: f.root, env: {} })).toThrow("asynchronous Windows worker");
      await expect(withWorkerPolicy(async () => "unsafe")).rejects.toThrow("review_isolated_identity_unavailable");
    });
    expect(providerAccountFolder(ownerAccount)).toBe(ownerAccount); expect(spawn).not.toHaveBeenCalled();
  });

  it("resolves only protected runtime files instead of using an owner PATH command", () => {
    const f = fixture(), ownerBin = join(f.root, "bin"); mkdirSync(ownerBin); writeFileSync(join(ownerBin, "claude.exe"), "synthetic");
    withWindowsWorker(f.worker, () => {
      expect(resolveCli("claude", { PATH: ownerBin })).toBeNull();
      const command = join(f.runtimeRoot, "claude.exe"); writeFileSync(command, "synthetic");
      expect(resolveCli("claude", { PATH: ownerBin })).toEqual({ file: command, args: [] });
    });
  });

  it("permits only an explicit private Claude API key through the transport", async () => {
    const f = fixture();
    await withWindowsWorker(f.worker, () => launchCli({ file: "protected.exe", args: [] }, [], { cwd: f.root, env: { ANTHROPIC_API_KEY: "inherited", TOKEN: "discard" }, claudeApiKey: "synthetic-explicit-key" }));
    expect(f.worker.spawn).toHaveBeenCalledWith("protected.exe", [], { cwd: f.root, env: { ANTHROPIC_API_KEY: "synthetic-explicit-key", CLAUDE_CODE_SUBPROCESS_ENV_SCRUB: "1", CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST: "1" } });
    expect(spawn).not.toHaveBeenCalled();
  });

  it("does not turn unknown worker termination into an ordinary failed status command", async () => {
    const f = fixture(), failure = new WorkerLaunchTerminationError(); vi.mocked(f.worker.spawn).mockRejectedValueOnce(failure);
    await expect(withWindowsWorker(f.worker, () => runCli({ file: "protected.exe", args: [] }, ["--version"], {}))).rejects.toBe(failure);
  });

  it("keeps normal work on the provider SID and selects only the registered review SID inside policy", async () => {
    const f = fixture(), other = fixture(), ownerAccount = join(f.ownerDataRoot, "accounts", "shared-api-account");
    const dispose = installWindowsWorker(f.worker, f.review);
    try {
      expect(workerCliExclusive(f.worker) && workerCliExclusive(f.review)).toBe(true);
      expect(() => installWindowsWorker(other.worker, other.review)).toThrow("different Windows worker boundary");
      const providerFolder = providerAccountFolder(ownerAccount);
      await launchCli({ file: "protected.exe", args: [] }, [], { cwd: f.workerRoot, env: {} });
      await withWorkerPolicy(async (proof) => {
        expect(proof?.purpose).toBe("review"); expect(proof?.workerSid).toBe(f.review.configuration().workerSid);
        expect(workerAccountsRoot()).toBe(join(f.reviewRoot, "accounts"));
        const reviewFolder = providerAccountFolder(ownerAccount); expect(reviewFolder).not.toBe(providerFolder);
        await launchCli({ file: "protected.exe", args: [] }, [], { cwd: f.reviewRoot, env: cliEnv({ CLAUDE_CONFIG_DIR: ownerAccount }, {}) });
        expect(f.review.spawn).toHaveBeenCalledWith("protected.exe", [], { cwd: f.reviewRoot, env: { CLAUDE_CONFIG_DIR: reviewFolder } });
      });
      expect(workerAccountsRoot()).toBe(join(f.workerRoot, "accounts"));
      expect(f.worker.spawn).toHaveBeenCalledTimes(1); expect(f.review.spawn).toHaveBeenCalledTimes(1);
    } finally { dispose(); }
    expect(workerCliExclusive(f.worker)).toBe(false); expect(workerAccountsRoot()).toBeNull();
  });
});
