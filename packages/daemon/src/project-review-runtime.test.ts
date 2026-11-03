import { spawn } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { createProjectReviewRuntime, type ReviewAccount } from "./project-review-runtime.ts";
import { reviewCoverage, reviewInstructionsHash, type ReviewContext, type ReviewSnapshot } from "./project-review.ts";
import { reviewMachinePolicyHash } from "./project-review-policy.ts";
import { withWindowsWorker, WorkerLaunchTerminationError } from "./cli.ts";
import { CheckTerminationError } from "./project-checks.ts";
import { WorkerPolicyBusyError, type WorkerPolicyReceipt } from "./windows-worker.ts";
import type { ProviderEvent } from "./providers/types.ts";
import { tempRoot } from "./testing.ts";
import { projectFixture } from "./project-test-support.ts";
import { TaskIntegrations } from "./task-integration.ts";

const context: ReviewContext = { botId: "reviewer", projectId: "project", taskId: "task", attemptId: "attempt", evaluationId: "evaluation", revision: 1,
  projectRevision: 1, taskRevision: 1, instructions: { reviewer: "Review correctness", role: "Review the assigned task", title: "Synthetic task", task: "Change the source", acceptance: ["Export one constant"] } };
const snapshot: ReviewSnapshot = { sourceSha: "a".repeat(40), files: ["source.ts"], diff: "+return 1;", read() { return "export const source = 1;\n"; } };
export function syntheticReviewRuntime(mode = "good") {
  const root = tempRoot(), script = join(root, `synthetic-claude-${mode}.mjs`), log = join(root, "turns.jsonl");
  writeFileSync(script, `
import {appendFileSync,writeFileSync} from 'node:fs';import {dirname,join} from 'node:path';import {fileURLToPath} from 'node:url';
if(process.argv.includes('--version')){console.log('2.1.314 (Claude Code)');process.exit(0)}
let text='';for await(const chunk of process.stdin)text+=chunk;const input=JSON.parse(text);const mode=${JSON.stringify(mode)};
const args=process.argv.slice(2);const budget=args[args.indexOf('--max-budget-usd')+1];
if(!args.includes('--bare')||!args.includes('--safe-mode')||args[args.indexOf('--tools')+1]!==''||process.env.NODE_OPTIONS)process.exit(2);
appendFileSync(join(dirname(fileURLToPath(import.meta.url)),'turns.jsonl'),JSON.stringify({acceptance:!!input.requests,budget,cwd:process.cwd(),criteria:input.criteria,instructionsSha256:input.instructionsSha256})+'\\n');
console.log(JSON.stringify({type:'system',subtype:'init',tools:mode==='tools'?['Bash']:[],mcp_servers:[],plugins:[]}));
if(mode==='tools'||mode==='wait'){setInterval(()=>{},1000)}else{
if(mode==='mutate')writeFileSync(join(dirname(process.cwd()),'forbidden-mutation'),'mutated');
let result=input.requests?{nonce:input.nonce,attempts:input.output.attempts}:{sourceSha:input.coverage.sourceSha,snapshotSha256:input.coverage.snapshotSha256,instructionsSha256:input.instructionsSha256,complete:true,reviewedFiles:input.snapshot.files.map(f=>f.path),state:'passed',reason:null,findings:[]};
if(mode==='incomplete'&&input.requests)result.attempts.fill(result.attempts[0]);
console.log(JSON.stringify({type:'result',subtype:'success',is_error:false,result:JSON.stringify(result),uuid:'result:1',total_cost_usd:0.01,usage:{input_tokens:10,output_tokens:5},modelUsage:{test:{}}}));}
`);
  const events: ProviderEvent[] = [], released = vi.fn(), budgets: number[] = [];
  let spent = 0;
  const account: ReviewAccount = { provider: "claude", kind: "api_key", model: "test", authentication: { kind: "api_key", key: "synthetic-api-key-not-real" },
    getMaxBudgetUsd() { const remaining = 0.1 - spent; budgets.push(remaining); return remaining; }, assertCurrent() {},
    onEvent(event) { events.push(event); if (event.type === "usage") spent += event.costUsd ?? 0; }, release: released };
  const options = { root: join(root, "reviews"), resolve: async () => account, cli: () => ({ file: process.execPath, args: [script] }), machinePolicyHash: () => "0".repeat(64) };
  const runtime = createProjectReviewRuntime(options);
  return { root, log, account, options, runtime, events, released, budgets };
}

function syntheticWorker(root: string) {
  const state = { held: false, observations: 0, failPaidStart: false, failAfter: false,
    receipt: { workerSid: "S-1-5-21-1-2-3-1001", observerSha256: "a".repeat(64), policySha256: "b".repeat(64) } as WorkerPolicyReceipt };
  const config = { version: 1 as const, ownerSid: "S-1-5-21-1-2-3-1000", workerSid: state.receipt.workerSid, workerName: "TjWorker-12345678", workerRoot: join(root, "worker"), runtimeRoot: join(root, "runtime"), ownerDataRoot: join(root, "owner"), provisionedAt: "2026-11-03T00:00:00.000Z", credentialTarget: `Tjommie/${"a".repeat(24)}/isolated-worker` };
  const boundary: Parameters<typeof withWindowsWorker>[0] = {
    required: true, configuration: () => config, async grantDirectory() {},
    async spawn(file, args, options) {
      if (!state.held) throw new WorkerPolicyBusyError();
      if (state.failPaidStart && args.includes("--output-format")) throw new WorkerLaunchTerminationError();
      return spawn(file, args, { ...options, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
    },
    async observeReviewPolicy() {
      if (!state.held) throw new WorkerPolicyBusyError(); state.observations++;
      return Object.freeze({ ...state.receipt });
    },
    async withReviewPolicy<T>(action: (receipt: WorkerPolicyReceipt) => Promise<T>): Promise<T> {
      if (state.held) throw new WorkerPolicyBusyError(); state.held = true; let unknown = false;
      try {
        const before = await boundary.observeReviewPolicy(), result = await action(before);
        if (state.failAfter) throw new WorkerLaunchTerminationError();
        if (JSON.stringify(await boundary.observeReviewPolicy()) !== JSON.stringify(before)) throw new Error("review_worker_policy_changed");
        return result;
      } catch (error) { unknown = error instanceof WorkerLaunchTerminationError; throw error; }
      finally { if (!unknown) state.held = false; }
    },
  };
  return { boundary, state, config };
}

it("copies the server criteria into the frozen prompt before resolving an account", async () => {
  const h = syntheticReviewRuntime(), acceptance = ["Require the exported constant"], instructions = { ...context.instructions, task: "Original trusted task", acceptance };
  const supplied = { ...context, instructions }, digest = reviewInstructionsHash(supplied);
  const runtime = createProjectReviewRuntime({ ...h.options, async resolve() { instructions.task = "Late mutation"; acceptance.push("Late criterion"); return h.account; } });
  expect((await runtime.review(snapshot, new AbortController().signal, supplied)).state).toBe("passed");
  const turns = readFileSync(h.log, "utf8").trim().split("\n").map((line) => JSON.parse(line) as { criteria?: ReviewContext["instructions"]; instructionsSha256?: string });
  expect(turns[1]).toMatchObject({ criteria: { ...context.instructions, task: "Original trusted task", acceptance: ["Require the exported constant"] }, instructionsSha256: digest });
}, 30_000);

it.skipIf(process.platform !== "win32")("requires fresh worker evidence before each paid turn and releases only after the final policy observation", async () => {
  const h = syntheticReviewRuntime(), worker = syntheticWorker(h.root), released = h.account.release;
  h.account.release = () => { expect(worker.state.held).toBe(false); return released(); };
  const runtime = createProjectReviewRuntime({ ...h.options, machinePolicyHash: reviewMachinePolicyHash });
  await withWindowsWorker(worker.boundary, async () => {
    expect((await runtime.review(snapshot, new AbortController().signal, context)).state).toBe("passed");
  });
  expect(worker.state.observations).toBeGreaterThanOrEqual(7); expect(worker.state.held).toBe(false); expect(h.released).toHaveBeenCalledTimes(1);
  const names = readdirSync(join(h.options.root, "receipts")), receipt = JSON.parse(readFileSync(join(h.options.root, "receipts", names[0]), "utf8")) as { identity: { workerPolicy: WorkerPolicyReceipt } };
  expect(receipt.identity.workerPolicy).toEqual(worker.state.receipt);
  expect(readdirSync(join(worker.config.workerRoot, "accounts", "reviewer-scratch"))).toEqual([]);
}, 30_000);

it.skipIf(process.platform !== "win32")("blocks the review turn when fresh worker evidence changes after acceptance", async () => {
  const h = syntheticReviewRuntime(), worker = syntheticWorker(h.root), onEvent = h.account.onEvent;
  h.account.onEvent = (event) => { onEvent(event); if (event.type === "usage") worker.state.receipt = { ...worker.state.receipt, policySha256: "c".repeat(64) }; };
  const runtime = createProjectReviewRuntime({ ...h.options, machinePolicyHash: reviewMachinePolicyHash });
  const result = await withWindowsWorker(worker.boundary, () => runtime.review(snapshot, new AbortController().signal, context));
  expect(result.state).toBe("unavailable"); expect(result.reason).toContain("review_policy_changed"); expect(h.events.filter((event) => event.type === "usage")).toHaveLength(1);
  expect(existsSync(join(h.options.root, "receipts"))).toBe(false); expect(worker.state.held).toBe(false); expect(h.released).toHaveBeenCalledTimes(1);
}, 30_000);

it.skipIf(process.platform !== "win32")("retains worker, account and scratch leases after unconfirmed paid-start or final-observer retirement", async () => {
  for (const failure of ["paidStart", "after"] as const) {
    const h = syntheticReviewRuntime(), worker = syntheticWorker(h.root); worker.state.failPaidStart = failure === "paidStart"; worker.state.failAfter = failure === "after";
    const runtime = createProjectReviewRuntime({ ...h.options, machinePolicyHash: reviewMachinePolicyHash });
    await withWindowsWorker(worker.boundary, async () => {
      await expect(runtime.review(snapshot, new AbortController().signal, context)).rejects.toBeInstanceOf(CheckTerminationError);
      expect((await runtime.review(snapshot, new AbortController().signal, context)).reason).toContain("review_worker_busy");
    });
    expect(worker.state.held).toBe(true); expect(h.released).not.toHaveBeenCalled();
    expect(readdirSync(join(worker.config.workerRoot, "accounts", "reviewer-scratch"))).toHaveLength(1);
    expect(existsSync(join(h.options.root, "evaluations"))).toBe(false);
    const usage = h.events.filter((event) => event.type === "usage");
    if (failure === "paidStart") expect(usage).toMatchObject([{ costUsd: null }]);
    else expect(usage).toHaveLength(2);
  }
}, 45_000);

it("derives its own acceptance, charges both real subprocess turns, refreshes the budget and retains exact coverage", async () => {
  const h = syntheticReviewRuntime();
  expect(await h.runtime.review(snapshot, new AbortController().signal, context)).toMatchObject({ state: "passed", coverage: reviewCoverage(snapshot) });
  expect(h.events.filter((event) => event.type === "usage")).toHaveLength(2); expect(h.budgets).toHaveLength(2); expect(h.budgets[0]).toBe(0.1); expect(h.budgets[1]).toBeCloseTo(0.09); expect(h.released).toHaveBeenCalledTimes(1);
  const charged = h.events.filter((event) => event.type === "usage"); expect(new Set(charged.map((event) => event.eventId)).size).toBe(2);
  const receipts = readdirSync(join(h.options.root, "receipts")); expect(receipts).toHaveLength(1);
  const receipt = JSON.parse(readFileSync(join(h.options.root, "receipts", receipts[0]), "utf8")) as { identity: { version: string }; attempts: string[] };
  expect(receipt.identity.version).toBe("2.1.314"); expect(receipt.attempts).toHaveLength(6);
  expect(readdirSync(h.options.root).some((name) => name.startsWith(".review-"))).toBe(false);
  expect(readFileSync(h.log, "utf8")).not.toContain("synthetic-api-key-not-real");
  await h.runtime.review(snapshot, new AbortController().signal, { ...context, evaluationId: "second" });
  expect(h.events.filter((event) => event.type === "usage")).toHaveLength(3);
  // A persisted/forged JSON file cannot enable a new daemon process: acceptance runs again.
  await createProjectReviewRuntime(h.options).review(snapshot, new AbortController().signal, { ...context, evaluationId: "restart" });
  expect(h.events.filter((event) => event.type === "usage")).toHaveLength(5);
}, 30_000);

it.each(["mutate", "incomplete"])("never mints a passing acceptance receipt when the fixture %s", async (mode) => {
  const h = syntheticReviewRuntime(mode), result = await h.runtime.review(snapshot, new AbortController().signal, context);
  expect(result.state).toBe("unavailable"); expect(result.coverage).toBeNull(); expect(existsSync(join(h.options.root, "receipts"))).toBe(false);
  expect(h.events.filter((event) => event.type === "usage")).toHaveLength(1); expect(h.released).toHaveBeenCalledTimes(1);
}, 20_000);

it("stops exposed tools and a cancelled owned process before releasing its lease and profile", async () => {
  const exposed = syntheticReviewRuntime("tools");
  expect((await exposed.runtime.review(snapshot, new AbortController().signal, context)).state).toBe("unavailable");
  expect(exposed.released).toHaveBeenCalledTimes(1); expect(existsSync(join(exposed.options.root, "receipts"))).toBe(false);
  const waiting = syntheticReviewRuntime("wait"), controller = new AbortController(), operation = waiting.runtime.review(snapshot, controller.signal, context);
  await vi.waitFor(() => expect(existsSync(waiting.log)).toBe(true), { timeout: 10_000 }); controller.abort();
  expect((await operation).state).toBe("unavailable"); expect(waiting.released).toHaveBeenCalledTimes(1);
  expect(waiting.events.filter((event) => event.type === "usage")).toMatchObject([{ model: "test", costUsd: null, input: 0, output: 0 }]);
  expect(readdirSync(waiting.options.root).some((name) => name.startsWith(".review-"))).toBe(false);
}, 30_000);

it("denies unsupported subscription/Codex accounts before any CLI turn and stops when acceptance consumes the remaining allowance", async () => {
  for (const override of [{ provider: "codex" as const }, { kind: "subscription" as const }]) {
    const h = syntheticReviewRuntime(); Object.assign(h.account, override);
    expect((await h.runtime.review(snapshot, new AbortController().signal, context)).state).toBe("unavailable");
    expect(existsSync(h.log)).toBe(false); expect(h.released).toHaveBeenCalledTimes(1);
  }
  const h = syntheticReviewRuntime(); h.account.getMaxBudgetUsd = () => h.events.length ? 0 : 0.01;
  expect((await h.runtime.review(snapshot, new AbortController().signal, context)).state).toBe("unavailable");
  expect(h.events.filter((event) => event.type === "usage")).toHaveLength(1); expect(h.released).toHaveBeenCalledTimes(1);
}, 20_000);

it("invalidates an acceptance if machine policy changes during the paid turn", async () => {
  const h = syntheticReviewRuntime(); let policy = "a".repeat(64); const onEvent = h.account.onEvent;
  h.account.onEvent = (event) => { onEvent(event); if (event.type === "usage") policy = "b".repeat(64); };
  const runtime = createProjectReviewRuntime({ ...h.options, machinePolicyHash: () => policy });
  expect((await runtime.review(snapshot, new AbortController().signal, context)).reason).toContain("review_policy_changed");
  expect(existsSync(join(h.options.root, "receipts"))).toBe(false);
}, 20_000);

it("runs the synthetic developer, enforced CLI reviewer, real host check and exact integration through the task services", async () => {
  const runtime = syntheticReviewRuntime(), h = projectFixture({ review: runtime.runtime }); let integrations: TaskIntegrations | undefined;
  try {
    h.git(h.repoPath, "branch", "delivery", h.base); h.projects.update(h.project.id, 1, { targetRef: "refs/heads/delivery" });
    writeFileSync(join(h.repoPath, "src/b.txt"), "owner working bytes\n"); const before = h.owner();
    const task = h.newTask(), attempt = await h.complete(h.tasks.start(task.id, task.revision, "synthetic-develop"), "src/a.txt", "A1\n");
    const reviewed = await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review");
    expect(reviewed.state).toBe("passed"); expect(reviewed.reviewCoverage?.sourceSha).toBe(attempt.headSha);
    expect((await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "test")).state).toBe("passed");
    expect(h.tasks.accepted(attempt)).toBe(true);
    integrations = new TaskIntegrations(h.tasks);
    const integration = integrations.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: h.base, requestId: "synthetic-integrate" });
    await vi.waitFor(() => expect(integrations!.get(integration.id).state).toBe("integrated"), { timeout: 60_000 });
    const result = integrations.get(integration.id); expect(result.checksCompleted).toBe(true);
    expect(h.git(h.repoPath, "rev-parse", "delivery")).toBe(result.candidateSha); expect(h.owner()).toEqual(before);
  } finally { await integrations?.shutdown(); await h.tasks.shutdown(); h.db.close(); }
}, 120_000);
