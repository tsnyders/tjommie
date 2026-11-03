import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import type { CheckRuntime } from "./project-checks.ts";
import { projectFixture } from "./project-test-support.ts";
import { reviewCoverage, type ReviewContext, type ReviewRuntime } from "./project-review.ts";
import { enqueue, getWorkItem, setStatus } from "./queue.ts";

const open: ReturnType<typeof projectFixture>[] = [];
vi.setConfig({ testTimeout: 30_000 });
afterEach(() => { for (const h of open.splice(0)) h.db.close(); });
const fixture = (options: ConstructorParameters<typeof import("./tasks.ts").Tasks>[2] = {}) => { const h = projectFixture(options); open.push(h); return h; };
export const passingChecks: CheckRuntime = { async run(check, cwd) {
  writeFileSync(join(cwd, "check-output.txt"), "synthetic output\n");
  return { checkId: check.id, command: check.command, args: check.args, exitCode: 0, output: "Synthetic check passed", truncated: false };
} };
export const passingReview: ReviewRuntime = { async review(snapshot) {
  snapshot.read("src/a.txt"); return { state: "passed", reason: null, findings: [], coverage: reviewCoverage(snapshot) };
} };

it("creates independent persisted task contexts only after explicit Start and rejects busy-bot dispatch", () => {
  const h = fixture(), first = h.newTask(0), second = h.newTask(1);
  const before = h.owner(), a = h.tasks.start(first.id, first.revision, "a"), b = h.tasks.start(second.id, second.revision, "b");
  expect(a.worktreePath).not.toBe(b.worktreePath); expect(a.contextKey).not.toBe(b.contextKey);
  expect(h.tasks.start(first.id, first.revision, "a").id).toBe(a.id); expect(h.delivered).toHaveLength(2);
  expect(h.tasks.executionContext(h.bots[0], h.delivered[0])?.cwd).toBe(a.worktreePath);
  expect(() => h.tasks.executionContext(h.bots[1], h.delivered[0])).toThrow(/no longer active/);
  const third = h.newTask(0); expect(() => h.tasks.start(third.id, third.revision, "busy")).toThrow(/queued or running|owns/);
  expect(h.owner()).toEqual(before);
});

it("keeps project room and bot-request discussions in the bot home instead of the user repository checkout", () => {
  const h = fixture(); h.db.prepare("UPDATE bots SET project_folder=? WHERE id=?").run(h.repoPath, h.bots[0].id);
  for (const source of ["room", "bot_request"] as const) {
    const item = enqueue(h.db, { botId: h.bots[0].id, source, text: "Discuss the project", conversationId: h.project.roomId! });
    const context = h.tasks.executionContext({ ...h.bots[0], projectFolder: h.repoPath }, item);
    expect(context?.phase).toBe("discussion"); expect(context?.cwd).toBe(h.bots[0].homeFolder); expect(context?.cwd).not.toBe(h.repoPath);
  }
  expect(h.db.prepare("SELECT project_folder FROM bots WHERE id=?").get(h.bots[0].id)?.project_folder).toBe(h.repoPath); expect(h.delivered).toHaveLength(0);
});

it("persists reviewer unavailable without launching a normal provider or accepting the task", async () => {
  const h = fixture(), task = h.newTask(); const a = await h.complete(h.tasks.start(task.id, task.revision, "start"), "src/a.txt", "A1\n");
  const review = await h.tasks.evaluate(a.id, a.revision, a.headSha!, "review");
  expect(review.state).toBe("unavailable"); expect(review.reason).toBe("review_isolation_unavailable");
  expect(h.tasks.accepted(a)).toBe(false); expect(h.delivered).toHaveLength(1); expect(h.tasks.task(task.id).state).toBe("review");
});

it("refuses a passing review without complete coverage, and hydrates legacy coverage as null", async () => {
  const h = fixture({ review: { async review() { return { state: "passed", reason: null, findings: [] }; } } }), task = h.newTask();
  const attempt = await h.complete(h.tasks.start(task.id, task.revision, "missing-coverage"), "src/a.txt", "A1\n");
  const result = await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review");
  expect(result.state).toBe("unavailable"); expect(h.tasks.accepted(attempt)).toBe(false);
  h.db.prepare("UPDATE task_evaluations SET data=json_remove(data,'$.reviewCoverage') WHERE id=?").run(result.id);
  expect(h.tasks.detail(task.id).evaluations[0].reviewCoverage).toBeNull();
});

it("supplies frozen server-owned reviewer, role, task and checklist criteria with their revisions", async () => {
  let observed: ReviewContext | undefined;
  const h = fixture({ review: { async review(snapshot, _signal, context) { observed = context; return { state: "passed", reason: null, findings: [], coverage: reviewCoverage(snapshot) }; } } });
  h.db.prepare("UPDATE bots SET instructions=? WHERE id=?").run("Check error handling", h.bots[2].id);
  const project = h.projects.update(h.project.id, h.project.revision, { members: h.project.members.map((member) => ({ ...member, instructions: member.botId === h.bots[2].id ? "Review compatibility" : member.instructions })) });
  const task = h.newTask(), attempt = await h.complete(h.tasks.start(task.id, task.revision, "criteria"), "src/a.txt", "A1\n"), currentTask = h.tasks.task(task.id);
  expect((await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review")).state).toBe("passed");
  expect(observed).toMatchObject({ botId: h.bots[2].id, projectRevision: project.revision, taskRevision: currentTask.revision, revision: attempt.revision,
    instructions: { reviewer: "Check error handling", role: "Review compatibility", title: task.title, task: task.instructions, acceptance: task.acceptance } });
  expect(Object.isFrozen(observed)).toBe(true); expect(Object.isFrozen(observed!.instructions)).toBe(true); expect(Object.isFrozen(observed!.instructions.acceptance)).toBe(true);
});

it("keeps a completed model reply stale if trusted reviewer instructions changed during review", async () => {
  let release: (() => void) | undefined, started: (() => void) | undefined;
  const ready = new Promise<void>((resolve) => { started = resolve; }), waiting = new Promise<void>((resolve) => { release = resolve; });
  const h = fixture({ review: { async review(snapshot) { started!(); await waiting; return { state: "passed", reason: null, findings: [], coverage: reviewCoverage(snapshot) }; } } }), task = h.newTask();
  const attempt = await h.complete(h.tasks.start(task.id, task.revision, "criteria-drift"), "src/a.txt", "A1\n");
  const operation = h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review"); await ready;
  h.db.prepare("UPDATE bots SET instructions=? WHERE id=?").run("New acceptance requirements", h.bots[2].id); release!();
  expect(await operation).toMatchObject({ state: "stale", reason: "The review criteria changed during evaluation." });
  expect(h.tasks.detail(task.id).evaluations[0].state).toBe("stale"); expect(h.tasks.accepted(attempt)).toBe(false); expect(h.tasks.botBusy(h.bots[2].id)).toBe(false);
});

it("binds review and isolated checks to exact commit/revision and invalidates them when code changes", async () => {
  const h = fixture({ checks: passingChecks, review: passingReview }), task = h.newTask();
  const a = await h.complete(h.tasks.start(task.id, task.revision, "start"), "src/a.txt", "A1\n");
  expect((await h.tasks.evaluate(a.id, a.revision, a.headSha!, "review")).state).toBe("passed");
  const tests = await h.tasks.evaluate(a.id, a.revision, a.headSha!, "test");
  expect(tests.state).toBe("passed"); expect(tests.worktreePath).not.toBe(a.worktreePath);
  expect(readFileSync(join(tests.worktreePath!, "check-output.txt"), "utf8")).toContain("synthetic");
  expect(h.tasks.accepted(a)).toBe(true); expect(h.tasks.task(task.id).state).toBe("done");
  writeFileSync(join(a.worktreePath, "src/a.txt"), "uncommitted fix\n"); h.tasks.detail(task.id);
  expect(h.tasks.attempt(a.id).revision).toBe(a.revision + 1);
  expect(h.tasks.detail(task.id).evaluations.every((evaluation) => evaluation.state === "stale")).toBe(true);
  expect(() => h.tasks.exact(a.id, a.revision, a.headSha!)).toThrow();
});

it("Stop preserves edits and only explicit Resume requeues an attempt", async () => {
  const h = fixture(), task = h.newTask(), a = h.tasks.start(task.id, task.revision, "start");
  writeFileSync(join(a.worktreePath, "src/a.txt"), "retained working edits\n");
  const stopped = await h.tasks.stop(task.id, h.tasks.task(task.id).revision);
  expect(h.tasks.attempt(a.id).state).toBe("interrupted"); expect(readFileSync(join(a.worktreePath, "src/a.txt"), "utf8")).toContain("retained");
  const resumed = h.tasks.resume(task.id, stopped.revision, "resume"); expect(resumed.id).toBe(a.id); expect(resumed.revision).toBe(a.revision + 1);
  expect(h.delivered).toHaveLength(2);
});

it("blocks an interrupted running attempt on recovery and never reruns completed work", async () => {
  const h = fixture(), task = h.newTask(), a = h.tasks.start(task.id, task.revision, "start");
  setStatus(h.db, a.workItemId!, "interrupted"); h.tasks.recover();
  expect(h.tasks.attempt(a.id).state).toBe("interrupted"); expect(h.delivered).toHaveLength(1);
  expect(getWorkItem(h.db, a.workItemId!).status).toBe("interrupted");
});

it("previews and refuses cleanup of all untracked files, including ignored output, and retains branches after clean cleanup", async () => {
  const h = fixture(), task = h.newTask(), attempt = await h.complete(h.tasks.start(task.id, task.revision, "start"), "src/a.txt", "A1\n");
  writeFileSync(join(attempt.worktreePath, "check-output.txt"), "ignored bytes to retain\n");
  const preview = h.tasks.cleanupPreview(attempt.id); expect(preview.canRemove).toBe(false); expect(preview.files).toContain("check-output.txt");
  expect(() => h.tasks.cleanup(attempt.id, attempt.headSha!)).toThrow(/retained work/);
  expect(readFileSync(join(attempt.worktreePath, "check-output.txt"), "utf8")).toContain("retain");
  rmSync(join(attempt.worktreePath, "check-output.txt"));
  expect(h.tasks.cleanupPreview(attempt.id).canRemove).toBe(true); expect(h.tasks.cleanup(attempt.id, attempt.headSha!).state).toBe("cleaned");
  expect(h.git(h.repoPath, "rev-parse", attempt.branch)).toBe(attempt.headSha); expect(h.tasks.detail(task.id).attempts[0].state).toBe("cleaned");
});

it("does not wait for or interrupt a different live task when cancelling completed work", async () => {
  const h = fixture(), oldTask = h.newTask(), old = await h.complete(h.tasks.start(oldTask.id, oldTask.revision, "old"), "src/a.txt", "A1\n");
  const nextTask = h.newTask(), next = h.tasks.start(nextTask.id, nextTask.revision, "next"); h.runner.retired = () => new Promise<void>(() => {});
  const cancelled = await h.tasks.stop(oldTask.id, h.tasks.task(oldTask.id).revision, true);
  expect(cancelled.state).toBe("cancelled"); expect(h.tasks.attempt(next.id).state).toBe("running"); expect(getWorkItem(h.db, next.workItemId!).status).toBe("running");
  expect(h.tasks.attempt(old.id).state).toBe("cancelled");
});

it("cancels and awaits owned evaluations during shutdown before releasing their bot lease", async () => {
  let started: (() => void) | undefined; const ready = new Promise<void>((resolve) => { started = resolve; });
  const checks: CheckRuntime = { run(check, _cwd, signal) { started!(); return new Promise((resolve) => signal.addEventListener("abort", () => resolve({ checkId: check.id, command: check.command, args: check.args, exitCode: null, output: "cancelled", truncated: false }), { once: true })); } };
  const h = fixture({ checks }), task = h.newTask(), attempt = await h.complete(h.tasks.start(task.id, task.revision, "start"), "src/a.txt", "A1\n");
  const result = h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "test"); await ready;
  expect(h.tasks.botBusy(h.bots[3].id)).toBe(true); await h.tasks.shutdown();
  expect((await result).state).toBe("stale"); expect(h.tasks.botBusy(h.bots[3].id)).toBe(false);
  expect(() => h.tasks.start(task.id, h.tasks.task(task.id).revision, "closed")).toThrow(/closing/);
});
