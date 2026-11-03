import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { tempRoot } from "./testing.ts";
import { WindowsWorkerBoundary, WorkerLaunchTerminationError, WorkerPolicyBusyError, type WorkerPolicyReceipt } from "./windows-worker.ts";

vi.mock("node:child_process", () => ({ spawn: vi.fn(() => { throw new Error("No native process is permitted in this synthetic fixture."); }) }));
afterEach(() => vi.restoreAllMocks());

const receipt: WorkerPolicyReceipt = Object.freeze({ workerSid: "S-1-5-21-1-2-3-1002", observerSha256: "a".repeat(64), policySha256: "b".repeat(64) });
function fixture() {
  const root = tempRoot(), configurationPath = join(root, "worker.json");
  return { root, configurationPath, worker: new WindowsWorkerBoundary({ configurationPath, exclusive: () => false }) };
}

describe("Windows worker review lease", () => {
  it("rejects policy observation without an exclusive review scope", async () => {
    await expect(fixture().worker.observeReviewPolicy()).rejects.toBeInstanceOf(WorkerPolicyBusyError);
    expect(spawn).not.toHaveBeenCalled();
  });

  it("holds unrelated launches out while permitting the trusted scope and releases after matching observations", async () => {
    const { worker } = fixture();
    const observe = vi.spyOn(worker, "observeReviewPolicy").mockResolvedValue(receipt);
    let entered!: () => void, finish!: () => void;
    const ready = new Promise<void>((done) => { entered = done; }), pending = new Promise<void>((done) => { finish = done; });
    const result = worker.withReviewPolicy(async (proof) => {
      expect(proof).toBe(receipt);
      // It reaches configuration, proving that this scope passed the Busy gate; no process starts.
      await expect(worker.spawn("unused", [], { cwd: "unused", env: {} })).rejects.toThrow("isolated Windows provider");
      entered(); await pending; return "reviewed";
    });
    await ready;
    await expect(worker.spawn("unused", [], { cwd: "unused", env: {} })).rejects.toBeInstanceOf(WorkerPolicyBusyError);
    await expect(worker.withReviewPolicy(async () => "other")).rejects.toBeInstanceOf(WorkerPolicyBusyError);
    finish(); expect(await result).toBe("reviewed");
    expect(observe).toHaveBeenCalledTimes(2);
    expect(await worker.withReviewPolicy(async () => "next")).toBe("next");
    expect(spawn).not.toHaveBeenCalled();
  });

  it("retains quiescence and invalidates future work after an action reports unknown termination", async () => {
    const { worker } = fixture();
    vi.spyOn(worker, "observeReviewPolicy").mockResolvedValue(receipt);
    const unknown = new WorkerLaunchTerminationError();
    await expect(worker.withReviewPolicy(async () => { throw unknown; })).rejects.toBe(unknown);
    await expect(worker.spawn("unused", [], { cwd: "unused", env: {} })).rejects.toBeInstanceOf(WorkerPolicyBusyError);
    await expect(worker.withReviewPolicy(async () => "unsafe")).rejects.toBeInstanceOf(WorkerPolicyBusyError);
    expect(await worker.unavailableReason()).toContain("verified Windows worker account");
    expect(spawn).not.toHaveBeenCalled();
  });

  it("retains quiescence when the post-observer loses its process-tree proof", async () => {
    const { worker } = fixture();
    vi.spyOn(worker, "observeReviewPolicy").mockResolvedValueOnce(receipt).mockRejectedValueOnce(new WorkerLaunchTerminationError());
    await expect(worker.withReviewPolicy(async () => "not accepted")).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
    await expect(worker.withReviewPolicy(async () => "unsafe")).rejects.toBeInstanceOf(WorkerPolicyBusyError);
  });

  it("refuses changed worker policy and permits a later fresh attempt after a settled failure", async () => {
    const { worker } = fixture();
    vi.spyOn(worker, "observeReviewPolicy").mockResolvedValueOnce(receipt).mockResolvedValueOnce({ ...receipt, policySha256: "c".repeat(64) }).mockResolvedValue(receipt);
    await expect(worker.withReviewPolicy(async () => "stale")).rejects.toThrow("review_worker_policy_changed");
    const failure = new Error("ordinary provider failure");
    await expect(worker.withReviewPolicy(async () => { throw failure; })).rejects.toBe(failure);
    expect(await worker.withReviewPolicy(async () => "fresh")).toBe("fresh");
  });
});

describe("Windows worker startup gate", () => {
  it("keeps worker mode required if configuration disappears but its protection marker remains", async () => {
    const { worker, root } = fixture();
    expect(worker.required).toBe(false);
    writeFileSync(join(root, "vault-protection-required"), "1");
    expect(worker.required).toBe(true);
    expect(await worker.unavailableReason()).not.toBeNull();
    await expect(worker.spawn("unused", [], { cwd: root, env: {} })).rejects.toThrow();
    expect(spawn).not.toHaveBeenCalled();
  });

  it("rejects an acceptance receipt that does not match the exact protected configuration", async () => {
    const { root, configurationPath } = fixture();
    const worker = new WindowsWorkerBoundary({ configurationPath, exclusive: () => true });
    const workerRoot = join(root, "worker"), runtimeRoot = join(root, "runtime"), ownerDataRoot = root;
    for (const folder of [workerRoot, runtimeRoot]) mkdirSync(folder);
    writeFileSync(configurationPath, JSON.stringify({ version: 1, ownerSid: "S-1-5-21-1-2-3-1001", workerSid: receipt.workerSid, workerName: "TjWorker-12345678", workerRoot, runtimeRoot, ownerDataRoot, provisionedAt: new Date().toISOString(), credentialTarget: `Tjommie/${"a".repeat(24)}/isolated-worker` }));
    writeFileSync(join(root, "worker-acceptance.json"), JSON.stringify({ version: 1, configurationHash: "0".repeat(64), ownerFileDenied: true, ownerCredentialDenied: true, ownerProcessDenied: true, ownerDesktopDenied: true, workerWriteAllowed: true, codexStarted: true, claudeStarted: true, reviewPolicyVerified: true }));
    expect(worker.required).toBe(true);
    expect(await worker.unavailableReason()).not.toBeNull();
    expect(spawn).not.toHaveBeenCalled();
  });
});
