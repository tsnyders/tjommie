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
  const root = tempRoot(), workerRoot = join(root, "worker"), runtimeRoot = join(root, "runtime"), ownerDataRoot = join(root, "owner");
  for (const folder of [workerRoot, runtimeRoot, ownerDataRoot]) mkdirSync(folder);
  const child = Object.assign(new EventEmitter(), { stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough() }) as unknown as ChildProcessWithoutNullStreams;
  const worker: Parameters<typeof withWindowsWorker>[0] = {
    required: true,
    configuration: () => ({ version: 1, workerRoot, runtimeRoot, ownerDataRoot, ownerSid: "S-1-5-21-1-2-3-1001", workerSid: "S-1-5-21-1-2-3-1002", workerName: "TjWorker-12345678", provisionedAt: new Date().toISOString(), credentialTarget: `Tjommie/${"a".repeat(24)}/isolated-worker` }),
    spawn: vi.fn(async () => child), grantDirectory: vi.fn(async () => {}),
    withReviewPolicy: async (action) => action({ workerSid: "S-1-5-21-1-2-3-1002", observerSha256: "a".repeat(64), policySha256: "b".repeat(64) }),
    observeReviewPolicy: async () => ({ workerSid: "S-1-5-21-1-2-3-1002", observerSha256: "a".repeat(64), policySha256: "b".repeat(64) }),
  };
  return { root, workerRoot, runtimeRoot, ownerDataRoot, worker, child };
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
      const child = await launchCli({ file: join(f.runtimeRoot, "codex.exe"), args: [] }, ["--version"], { cwd: ownerAccount, env });
      expect(child).toBe(f.child);
      expect(f.worker.spawn).toHaveBeenCalledWith(join(f.runtimeRoot, "codex.exe"), ["--version"], { cwd: account, env: { Path: "bin", CODEX_HOME: account } });
      await prepareProviderDirectories(account);
      expect(f.worker.grantDirectory).toHaveBeenCalledWith(account);
      expect(() => spawnCli({ file: "never.exe", args: [] }, [], { cwd: f.root, env: {} })).toThrow("asynchronous Windows worker");
    });
    expect(providerAccountFolder(ownerAccount)).toBe(ownerAccount);
    expect(spawn).not.toHaveBeenCalled();
  });

  it("resolves only protected runtime files and fails closed instead of using an owner PATH command", () => {
    const f = fixture(), ownerBin = join(f.root, "bin"); mkdirSync(ownerBin); writeFileSync(join(ownerBin, "claude.exe"), "synthetic");
    withWindowsWorker(f.worker, () => {
      expect(resolveCli("claude", { PATH: ownerBin })).toBeNull();
      const command = join(f.runtimeRoot, "claude.exe"); writeFileSync(command, "synthetic");
      expect(resolveCli("claude", { PATH: ownerBin })).toEqual({ file: command, args: [] });
    });
  });

  it("permits only an explicit private Claude API key through the worker transport", async () => {
    const f = fixture();
    await withWindowsWorker(f.worker, () => launchCli({ file: "protected.exe", args: [] }, [], { cwd: f.root, env: { ANTHROPIC_API_KEY: "inherited", TOKEN: "discard" }, claudeApiKey: "synthetic-explicit-key" }));
    expect(f.worker.spawn).toHaveBeenCalledWith("protected.exe", [], { cwd: f.root, env: { ANTHROPIC_API_KEY: "synthetic-explicit-key", CLAUDE_CODE_SUBPROCESS_ENV_SCRUB: "1", CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST: "1" } });
    expect(spawn).not.toHaveBeenCalled();
  });

  it("does not turn unknown worker termination into an ordinary failed status command", async () => {
    const f = fixture(), failure = new WorkerLaunchTerminationError(); vi.mocked(f.worker.spawn).mockRejectedValueOnce(failure);
    await expect(withWindowsWorker(f.worker, () => runCli({ file: "protected.exe", args: [] }, ["--version"], {}))).rejects.toBe(failure);
  });

  it("installs one daemon-wide boundary with no concurrent replacement and removes only that instance", async () => {
    const f = fixture(), other = fixture();
    const dispose = installWindowsWorker(f.worker);
    try {
      expect(workerCliExclusive(f.worker)).toBe(true);
      expect(() => installWindowsWorker(other.worker)).toThrow("different Windows worker boundary");
      await withWorkerPolicy(async (proof) => { expect(proof?.workerSid).toBe(f.worker.configuration().workerSid); });
      await launchCli({ file: "protected.exe", args: [] }, [], { cwd: f.root, env: {} });
      expect(f.worker.spawn).toHaveBeenCalledTimes(1);
    } finally { dispose(); }
    expect(workerCliExclusive(f.worker)).toBe(false);
    expect(workerAccountsRoot()).toBeNull();
  });
});
