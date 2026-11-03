import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import type { CheckRuntime } from "./project-checks.ts";
import { projectFixture } from "./project-test-support.ts";
import { reviewCoverage, type ReviewRuntime } from "./project-review.ts";
import { TaskIntegrations } from "./task-integration.ts";

const checks: CheckRuntime = { async run(check) { return { checkId: check.id, command: check.command, args: check.args, exitCode: 0, output: "synthetic pass", truncated: false }; } };
const review: ReviewRuntime = { async review(snapshot) { snapshot.read("src/a.txt"); return { state: "passed", reason: null, findings: [], coverage: reviewCoverage(snapshot) }; } };
const open: ReturnType<typeof projectFixture>[] = [];
const integrations: TaskIntegrations[] = [];
vi.setConfig({ testTimeout: 30_000 });
const track = (service: TaskIntegrations): TaskIntegrations => { integrations.push(service); return service; };
afterEach(async () => {
  for (const service of integrations.splice(0)) await service.shutdown();
  for (const h of open.splice(0)) { await h.tasks.shutdown(); h.db.close(); }
});
const fixture = () => { const h = projectFixture({ checks, review }); open.push(h); return h; };
async function accepted(h: ReturnType<typeof projectFixture>, index = 0) {
  const task = h.newTask(index), attempt = await h.complete(h.tasks.start(task.id, task.revision, `start-${index}`), index === 0 ? "src/a.txt" : "src/b.txt", index === 0 ? "A1\n" : "B1\n");
  await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review"); await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "test"); return attempt;
}
async function finished(service: TaskIntegrations, id: string) {
  await vi.waitFor(() => expect(["integrated", "blocked"]).toContain(service.get(id).state), { timeout: 20_000, interval: 20 }); return service.get(id);
}

it("refuses to promote a passing prefix after a later combined check fails to return", async () => {
  const h = fixture();
  h.git(h.repoPath, "branch", "delivery", h.base);
  h.projects.update(h.project.id, 1, { targetRef: "refs/heads/delivery", checks: [h.project.checks[0], { ...h.project.checks[0], id: "second" }] });
  const attempt = await accepted(h);
  let count = 0;
  const interrupted: CheckRuntime = { async run(check) {
    if (++count === 2) throw new Error("Synthetic interruption before check result");
    return checks.run(check, "", new AbortController().signal);
  } };
  const service = track(new TaskIntegrations(h.tasks, interrupted));
  const record = service.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: h.base, requestId: "partial-checks" });
  const blocked = await finished(service, record.id);
  expect(blocked.checks).toHaveLength(1);
  expect(blocked.checks[0].exitCode).toBe(0);
  expect(blocked.checksCompleted).toBe(false);
  service.recover();
  expect(() => service.promote(record.id, h.base)).toThrow(/verified candidate/);
  expect(h.git(h.repoPath, "rev-parse", "delivery")).toBe(h.base);
});

it("integrates two exact accepted tasks with combined checks while preserving a dirty user checkout", async () => {
  const h = fixture(); h.git(h.repoPath, "branch", "delivery", h.base); h.projects.update(h.project.id, 1, { targetRef: "refs/heads/delivery" });
  writeFileSync(join(h.repoPath, "src/b.txt"), "owner staged\n"); h.git(h.repoPath, "add", "src/b.txt"); writeFileSync(join(h.repoPath, "src/b.txt"), "owner staged and unstaged\n"); writeFileSync(join(h.repoPath, "owner-untracked"), "retain\n");
  const before = h.owner(), first = await accepted(h, 0), second = await accepted(h, 1), service = track(new TaskIntegrations(h.tasks, checks));
  const a = service.start({ attemptId: first.id, expectedRevision: first.revision, sourceSha: first.headSha!, expectedTargetSha: h.base, requestId: "integrate-a" });
  const integratedA = await finished(service, a.id); expect(integratedA.reason).toBeNull(); expect(integratedA.state).toBe("integrated");
  const b = service.start({ attemptId: second.id, expectedRevision: second.revision, sourceSha: second.headSha!, expectedTargetSha: integratedA.candidateSha!, requestId: "integrate-b" });
  const integratedB = await finished(service, b.id); expect(integratedB.reason).toBeNull(); expect(integratedB.state).toBe("integrated");
  expect(h.git(h.repoPath, "rev-parse", "delivery")).toBe(integratedB.candidateSha);
  expect(h.git(h.repoPath, "merge-base", "--is-ancestor", first.headSha!, integratedB.candidateSha!)).toBe("");
  expect(h.git(h.repoPath, "merge-base", "--is-ancestor", second.headSha!, integratedB.candidateSha!)).toBe(""); expect(h.owner()).toEqual(before);
}, 60_000);

it("retains a verified candidate when the user has the target checked out and rejects stale source promotion", async () => {
  const h = fixture(), attempt = await accepted(h), before = h.owner(), service = track(new TaskIntegrations(h.tasks, checks));
  const record = service.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: h.base, requestId: "integrate" });
  const blocked = await finished(service, record.id); expect(blocked.reason).toBe("target_checked_out"); expect(blocked.candidateSha).toBeTruthy(); expect(h.owner()).toEqual(before);
  writeFileSync(join(attempt.worktreePath, "src/a.txt"), "later source edit\n");
  expect(() => service.promote(record.id, h.base)).toThrow(/changed|dirty/);
  expect(existsSync(blocked.worktreePath)).toBe(true);
}, 30_000);

it("blocks failed combined checks and target-head drift without promoting or discarding candidates", async () => {
  const h = fixture(), attempt = await accepted(h);
  const failing: CheckRuntime = { async run(check) { return { checkId: check.id, command: check.command, args: check.args, exitCode: 1, output: "synthetic failure", truncated: false }; } };
  const service = track(new TaskIntegrations(h.tasks, failing)), failed = service.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: h.base, requestId: "failed" });
  const result = await finished(service, failed.id); expect(result.reason).toBe("combined_checks_failed"); expect(existsSync(result.worktreePath)).toBe(true); expect(h.git(h.repoPath, "rev-parse", "main")).toBe(h.base);
  const drift = service.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: attempt.headSha!, requestId: "drift" });
  expect((await finished(service, drift.id)).reason).toBe("target_head_moved");
}, 30_000);

it("stops a queued integration during shutdown before running configured project code", async () => {
  const h = fixture(), attempt = await accepted(h); let runs = 0;
  const runtime: CheckRuntime = { async run(check) { runs++; return checks.run(check, h.repoPath, new AbortController().signal); } };
  const service = track(new TaskIntegrations(h.tasks, runtime)), record = service.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: h.base, requestId: "close" });
  await service.shutdown(); expect(service.get(record.id).reason).toContain("stopped before preparation"); expect(runs).toBe(0); expect(existsSync(record.worktreePath)).toBe(false);
}, 30_000);
