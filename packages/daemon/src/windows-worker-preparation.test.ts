import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { afterEach, expect, it, vi } from "vitest";
import { tempRoot } from "./testing.ts";
import { WindowsWorkerBoundary, WorkerLaunchTerminationError, WorkerPreparation, workerConfigurationHash, workerProcessRetired, type WindowsWorkerConfiguration } from "./windows-worker.ts";

vi.mock("node:child_process", () => ({ spawn: vi.fn(() => { throw new Error("Native execution is forbidden in the synthetic preparation fixture."); }) }));
afterEach(() => { vi.restoreAllMocks(); vi.clearAllMocks(); });

type Request = { mode?: "verify"; config: WindowsWorkerConfiguration; receiptPath?: string; jobName?: string; allowedJobs: unknown[] };
function fixture() {
  const root = tempRoot(), owner = join(root, "owner"), runtime = join(root, "runtime"), work = join(root, "work"), review = join(root, "review");
  for (const folder of [owner, runtime, work, review]) mkdirSync(folder);
  const file = join(runtime, "node.exe"); writeFileSync(file, "synthetic non-executable");
  const config = (purpose: "provider" | "review"): WindowsWorkerConfiguration => ({ version: 2, purpose, ownerSid: "S-1-5-21-1-2-3-1000", workerSid: purpose === "provider" ? "S-1-5-21-1-2-3-1001" : "S-1-5-21-1-2-3-1002", peerSid: purpose === "provider" ? "S-1-5-21-1-2-3-1002" : "S-1-5-21-1-2-3-1001", workerName: purpose === "provider" ? "TjWorker-12345678" : "TjReview-12345678", ownerDataRoot: owner, workerRoot: purpose === "provider" ? work : review, peerRoot: purpose === "provider" ? review : work, runtimeRoot: runtime, provisionedAt: "2026-11-03T00:00:00.000Z", credentialTarget: "Tjommie/" + "a".repeat(24) + (purpose === "provider" ? "/isolated-worker" : "/isolated-review") });
  const providerConfig = config("provider"), reviewConfig = config("review");
  writeFileSync(join(owner, "worker.json"), JSON.stringify(providerConfig)); writeFileSync(join(owner, "review-worker.json"), JSON.stringify(reviewConfig));
  for (const [current, peer] of [[providerConfig, reviewConfig], [reviewConfig, providerConfig]]) {
    writeFileSync(join(owner, current!.purpose === "provider" ? "worker-acceptance.json" : "review-worker-acceptance.json"), JSON.stringify({ version: 2, purpose: current!.purpose, workerSid: current!.workerSid, peerSid: current!.peerSid, configurationHash: workerConfigurationHash(current!), peerConfigurationHash: workerConfigurationHash(peer!), ownerFileDenied: true, ownerCredentialDenied: true, ownerProcessDenied: true, ownerDesktopDenied: true, workerWriteAllowed: true, peerFileDenied: true, peerWriteDenied: true, peerRegistryDenied: true, peerProcessDenied: true, codexStarted: true, claudeStarted: true, reviewPolicyVerified: true }));
  }
  let credential: ((current: WindowsWorkerConfiguration) => Promise<string>) | undefined;
  const boundary = (current: WindowsWorkerConfiguration) => new WindowsWorkerBoundary({ configurationPath: join(owner, current.purpose === "provider" ? "worker.json" : "review-worker.json"), expectedPurpose: current.purpose, exclusive: () => true, transport: async () => credential ? credential(current) : JSON.stringify({ username: current.workerName, password: "synthetic-not-an-os-account" }) });
  const provider = boundary(providerConfig), reviewer = boundary(reviewConfig); provider.pairWith(reviewer); reviewer.pairWith(provider);
  // OS ACL mutation and OS credential access are deliberately not part of this fixture.
  vi.spyOn(provider, "grantDirectory").mockResolvedValue(); vi.spyOn(reviewer, "grantDirectory").mockResolvedValue();
  let nextPid = 10000;
  const requests: { request: Request; child: ChildProcessWithoutNullStreams; acknowledge(): void; retire(stopped: boolean): void; continues: number }[] = [];
  let verify: ((request: Request) => boolean) | undefined;
  vi.mocked(spawn).mockImplementation(() => {
    const emitter = new EventEmitter(), stdin = new PassThrough(), stdout = new PassThrough(), stderr = new PassThrough();
    const synthetic = Object.assign(emitter, { pid: nextPid++, exitCode: null as number | null, signalCode: null, stdin, stdout, stderr, kill: () => true });
    const child = synthetic as unknown as ChildProcessWithoutNullStreams;
    let pending = "";
    stdin.on("data", (bytes: Buffer) => {
      pending += bytes.toString("utf8");
      for (let at = pending.indexOf("\n"); at >= 0; at = pending.indexOf("\n")) {
        const line = pending.slice(0, at); pending = pending.slice(at + 1);
        if (line === "continue") { requests.find((entry) => entry.child === child)!.continues++; continue; }
        const request = JSON.parse(line) as Request;
        if (request.mode === "verify") {
          const pass = verify?.(request) ?? true;
          queueMicrotask(() => { if (pass) stdout.write("verified"); synthetic.exitCode = pass ? 0 : 71; child.emit("close", synthetic.exitCode); });
          continue;
        }
        requests.push({ request, child, continues: 0, acknowledge() {
          writeFileSync(request.receiptPath!, JSON.stringify({ version: 3, purpose: request.config.purpose, ownerSid: request.config.ownerSid, workerSid: request.config.workerSid, wrapperPid: child.pid, workerPid: child.pid! + 10000, jobName: request.jobName, jobBound: true, elevated: false, privileged: false }));
        }, retire(stopped) {
          writeFileSync(request.receiptPath! + ".closed", JSON.stringify({ version: 1, wrapperPid: child.pid, stopped }));
          synthetic.exitCode = 0; child.emit("exit", 0); child.emit("close", 0);
        } });
      }
    });
    return child;
  });
  return { provider, reviewer, file, work, requests, setVerify: (value: (request: Request) => boolean) => { verify = value; }, setCredential: (value: (current: WindowsWorkerConfiguration) => Promise<string>) => { credential = value; }, launch: () => provider.spawn(file, ["--synthetic"], { cwd: work, env: {} }) };
}

it("serializes paired status verification behind unreceipted preparation, then retries only a confirmed retired Job", async () => {
  const f = fixture(), first = f.launch();
  await vi.waitFor(() => expect(f.requests).toHaveLength(1));
  const before = vi.mocked(spawn).mock.calls.length, status = f.reviewer.unavailableReason();
  await Promise.resolve(); expect(vi.mocked(spawn).mock.calls).toHaveLength(before);
  let checks = 0;
  f.setVerify((request) => {
    checks++; expect(request.allowedJobs).toHaveLength(checks === 1 ? 1 : 0);
    if (checks === 1) { f.requests[0]!.retire(true); return false; } return true;
  });
  f.requests[0]!.acknowledge(); const child = await first;
  expect(await status).toBeNull(); expect(checks).toBe(2); expect(f.provider.receipts()).toEqual([]); await workerProcessRetired(child);
});

it("holds queued preparations out and poisons both identities promptly after unknown retirement", async () => {
  const f = fixture(), first = f.launch();
  await vi.waitFor(() => expect(f.requests).toHaveLength(1));
  const failed = expect(first).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  const status = f.reviewer.unavailableReason(), next = expect(f.launch()).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  const before = vi.mocked(spawn).mock.calls.length;
  f.requests[0]!.retire(false);
  await failed; await next; expect(await status).not.toBeNull(); expect(vi.mocked(spawn).mock.calls).toHaveLength(before);
  await expect(f.launch()).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
});

it("retries a pre-ack launch only after its stopped proof and a peer's confirmed retirement", async () => {
  const f = fixture(), first = f.launch(); await vi.waitFor(() => expect(f.requests).toHaveLength(1));
  f.requests[0]!.acknowledge(); const original = await first;
  const next = f.launch(); await vi.waitFor(() => expect(f.requests).toHaveLength(2));
  expect(f.requests[1]!.continues).toBe(0); f.requests[0]!.retire(true); f.requests[1]!.retire(true);
  await vi.waitFor(() => expect(f.requests).toHaveLength(3)); f.requests[2]!.acknowledge(); const continued = await next;
  expect(f.requests[1]!.continues).toBe(0); expect(f.requests[2]!.continues).toBe(1);
  f.requests[2]!.retire(true); await workerProcessRetired(original); await workerProcessRetired(continued);
});

it("rejects every queued waiter on poisoning without waiting for a blocked preparation to finish", async () => {
  const gate = new WorkerPreparation(); let entered!: () => void, finish!: () => void;
  const ready = new Promise<void>((done) => { entered = done; }), blocked = new Promise<void>((done) => { finish = done; });
  const running = gate.run(async () => { entered(); await blocked; }); await ready;
  const queued = expect(gate.run(async () => { throw new Error("must not enter"); })).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  gate.poison(); await queued; await expect(gate.run(async () => {})).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  finish(); await running;
});

it("refuses a held verification result after a live Job loses retirement proof", async () => {
  const f = fixture(), first = f.launch(); await vi.waitFor(() => expect(f.requests).toHaveLength(1));
  f.requests[0]!.acknowledge(); const original = await first;
  const lost = expect(workerProcessRetired(original)).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  f.setVerify(() => { f.requests[0]!.retire(false); return true; });
  await expect(f.launch()).rejects.toBeInstanceOf(WorkerLaunchTerminationError); await lost;
  expect(f.requests).toHaveLength(1);
});

it("refuses a credential result after poisoning without creating another native worker", async () => {
  const f = fixture(), first = f.launch(); await vi.waitFor(() => expect(f.requests).toHaveLength(1));
  f.requests[0]!.acknowledge(); const original = await first;
  let entered!: () => void, finish!: (value: string) => void;
  const ready = new Promise<void>((done) => { entered = done; }), pending = new Promise<string>((done) => { finish = done; });
  f.setCredential(async () => { entered(); return pending; });
  const next = expect(f.launch()).rejects.toBeInstanceOf(WorkerLaunchTerminationError); await ready;
  const lost = expect(workerProcessRetired(original)).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  f.requests[0]!.retire(false); finish(JSON.stringify({ username: "TjWorker-12345678", password: "synthetic-not-an-os-account" }));
  await next; await lost; expect(f.requests).toHaveLength(1);
});

it("settles an already-created pre-ack child after poisoning without sending continue", async () => {
  const f = fixture(), first = f.launch(); await vi.waitFor(() => expect(f.requests).toHaveLength(1));
  f.requests[0]!.acknowledge(); const original = await first;
  const next = expect(f.launch()).rejects.toBeInstanceOf(WorkerLaunchTerminationError); await vi.waitFor(() => expect(f.requests).toHaveLength(2));
  const lost = expect(workerProcessRetired(original)).rejects.toBeInstanceOf(WorkerLaunchTerminationError);
  f.requests[0]!.retire(false); f.requests[1]!.acknowledge(); f.requests[1]!.retire(true);
  await next; await lost; expect(f.requests[1]!.continues).toBe(0);
});
