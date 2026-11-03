import { TaskIntegration, type ProjectHandlers } from "../../protocol/src/projects.ts";
import { getStoredBot } from "./bots.ts";
import { newId, nowIso, transaction } from "./db.ts";
import { CheckTerminationError, HostCheckRuntime, type CheckRuntime } from "./project-checks.ts";
import { storedJson } from "./projects.ts";
import { RpcError } from "./server.ts";
import { Tasks } from "./tasks.ts";

export class TaskIntegrations {
  readonly tasks: Tasks;
  readonly #checks?: CheckRuntime;
  readonly #running = new Map<string, AbortController>();
  readonly #settling = new Set<Promise<void>>();
  #closing = false;
  #terminationUnknown = false;
  constructor(tasks: Tasks, checks?: CheckRuntime) { this.tasks = tasks; this.#checks = checks; }
  get(id: string): TaskIntegration { return TaskIntegration.parse(storedJson(this.tasks.db.prepare("SELECT data FROM task_integrations WHERE id=?").get(id) as { data: string } | undefined)); }
  #save(record: TaskIntegration): TaskIntegration {
    TaskIntegration.parse(record);
    this.tasks.db.prepare("UPDATE task_integrations SET state=?,data=? WHERE id=?").run(record.state, JSON.stringify(record), record.id);
    this.tasks.emit("task.integrationUpdated", record); return record;
  }
  #blocked(record: TaskIntegration, reason: string): TaskIntegration { return this.#save({ ...record, state: "blocked", reason, updatedAt: nowIso() }); }
  start(input: Parameters<ProjectHandlers["tasks.integrate"]>[0]): TaskIntegration {
    if (this.#closing) throw new RpcError("bad_state", "The integration service is closing.");
    const repeated = this.tasks.db.prepare("SELECT task_id,result_id,operation FROM task_requests WHERE id=?").get(input.requestId) as { task_id: string; result_id: string; operation: string } | undefined;
    const existingAttempt = this.tasks.attempt(input.attemptId), task = this.tasks.task(existingAttempt.taskId);
    if (repeated) {
      if (repeated.task_id !== task.id || repeated.operation !== "integrate") throw new RpcError("bad_params", "This request ID was already used for another action.");
      return this.get(repeated.result_id);
    }
    const attempt = this.tasks.exact(input.attemptId, input.expectedRevision, input.sourceSha);
    const project = this.tasks.projects.active(task.projectId), repo = this.tasks.projects.repository(project.repositoryId);
    if (this.tasks.db.prepare("SELECT 1 FROM task_integrations WHERE repository_id=? AND target_ref=? AND state IN ('preparing','checking','ready')").get(repo.id, project.targetRef)) throw new RpcError("busy", "Another integration owns this target.");
    const id = newId(), work = this.tasks.projects.worktrees.describe(`integration-${id}`, input.expectedTargetSha), timestamp = nowIso();
    const record: TaskIntegration = { id, attemptId: attempt.id, revision: attempt.revision, projectRevision: project.revision, acceptedSha: input.sourceSha, expectedTargetSha: input.expectedTargetSha,
      targetRef: project.targetRef, candidateSha: null, branch: work.branch, worktreePath: work.path, promotionPath: null, state: "preparing", reason: null, checks: [], checksCompleted: false, createdAt: timestamp, updatedAt: timestamp };
    transaction(this.tasks.db, () => {
      this.tasks.db.prepare("INSERT INTO task_integrations VALUES (?,?,?,?,?,?)").run(id, attempt.id, repo.id, record.state, record.targetRef, JSON.stringify(record));
      this.tasks.db.prepare("INSERT INTO task_requests VALUES (?,?,?,?)").run(input.requestId, "integrate", task.id, id);
    });
    this.tasks.emit("task.integrationUpdated", record);
    const operation = new Promise<void>((resolve) => { setImmediate(() => {
      void this.#prepare(record).catch(() => { this.#blocked(this.get(id), "Integration could not finish. Retained work needs inspection."); }).finally(resolve);
    }); });
    this.#settling.add(operation); void operation.finally(() => this.#settling.delete(operation));
    return record;
  }
  async #prepare(record: TaskIntegration): Promise<void> {
    if (this.#closing) { this.#blocked(record, "Integration stopped before preparation; no project checks were run."); return; }
    const attempt = this.tasks.exact(record.attemptId, record.revision, record.acceptedSha), task = this.tasks.task(attempt.taskId), project = this.tasks.projects.active(task.projectId);
    if (!this.tasks.accepted(attempt)) { this.#blocked(record, "The exact task commit needs passing independent review and configured checks."); return; }
    const repo = this.tasks.projects.repository(project.repositoryId), trees = this.tasks.projects.worktrees;
    if (trees.resolveRef(repo, record.targetRef) !== record.expectedTargetSha) { this.#blocked(record, "target_head_moved"); return; }
    const tester = task.testerId ? getStoredBot(this.tasks.db, task.testerId) : null;
    const checks = this.#checks ?? (tester?.autonomy === "full" ? new HostCheckRuntime() : undefined);
    if (!tester || !checks || !project.checks.length) { this.#blocked(record, "check_isolation_unavailable"); return; }
    this.tasks.lease(tester.id, record.id, "integration");
    const controller = new AbortController(); this.#running.set(record.id, controller);
    let terminationUnknown = false;
    try {
      const work = trees.describe(`integration-${record.id}`, record.expectedTargetSha, record.branch);
      const merged = trees.mergeCandidate(repo, work, record.expectedTargetSha, record.acceptedSha);
      if (merged.conflict) { this.#blocked(record, "conflict: resolve the retained candidate files, then create a new reviewed task commit; no side was selected."); return; }
      record = this.#save({ ...record, candidateSha: merged.sha, state: "checking", updatedAt: nowIso() });
      for (const check of project.checks) {
        const result = await checks.run(check, work.path, controller.signal); record.checks.push(result);
        record = this.#save({ ...record, updatedAt: nowIso() });
        if (result.exitCode !== 0) { this.#blocked(record, "combined_checks_failed"); return; }
      }
      this.tasks.exact(attempt.id, attempt.revision, record.acceptedSha);
      if (controller.signal.aborted) { this.#blocked(record, "Integration stopped; retained candidate is unchanged."); return; }
      if (!record.candidateSha || trees.dirty(repo, work, record.candidateSha, true)) { this.#blocked(record, "Combined checks changed the candidate source."); return; }
      record = this.#save({ ...record, state: "ready", checksCompleted: true, updatedAt: nowIso() });
      this.promote(record.id, record.expectedTargetSha);
    } catch (error) { terminationUnknown = error instanceof CheckTerminationError; this.#terminationUnknown ||= terminationUnknown; throw error; }
    finally { this.#running.delete(record.id); if (!terminationUnknown) this.tasks.release(tester.id, record.id); }
  }
  promote(id: string, expectedTargetSha: string): TaskIntegration {
    if (this.#closing) throw new RpcError("bad_state", "The integration service is closing.");
    const record = this.get(id);
    if (record.state === "integrated") return record;
    if (!["ready", "blocked"].includes(record.state) || !record.candidateSha || !record.checksCompleted || !record.checks.length || !record.checks.every((check) => check.exitCode === 0)) throw new RpcError("bad_state", "This integration has no verified candidate to promote.");
    if (expectedTargetSha !== record.expectedTargetSha) throw new RpcError("bad_state", "Prepare a new integration for the changed target head.");
    const attempt = this.tasks.exact(record.attemptId, record.revision, record.acceptedSha);
    if (!this.tasks.accepted(attempt)) return this.#blocked(record, "Review or tests are stale.");
    const project = this.tasks.projects.active(this.tasks.task(attempt.taskId).projectId), repo = this.tasks.projects.repository(project.repositoryId), trees = this.tasks.projects.worktrees;
    if (project.targetRef !== record.targetRef || project.revision !== record.projectRevision) return this.#blocked(record, "The project's target or check configuration changed.");
    if (record.checks.length !== project.checks.length || project.checks.some((check, index) => {
      const result = record.checks[index];
      return result.checkId !== check.id || result.command !== check.command || JSON.stringify(result.args) !== JSON.stringify(check.args);
    })) throw new RpcError("bad_state", "The candidate has not passed every configured combined check.");
    if (this.tasks.db.prepare("SELECT 1 FROM task_integrations WHERE repository_id=? AND target_ref=? AND id<>? AND state IN ('preparing','checking','ready')").get(repo.id, record.targetRef, record.id)) throw new RpcError("busy", "Another integration owns this target.");
    const work = trees.describe(`integration-${record.id}`, record.expectedTargetSha, record.branch);
    if (trees.head(repo, work) !== record.candidateSha || trees.dirty(repo, work, record.candidateSha, true)) return this.#blocked(record, "The verified candidate changed.");
    const promoted = trees.promote(repo, record.targetRef, record.expectedTargetSha, record.candidateSha);
    if (promoted.reason) return this.#save({ ...record, state: "blocked", promotionPath: promoted.path, reason: promoted.reason, updatedAt: nowIso() });
    return this.#save({ ...record, state: "integrated", promotionPath: promoted.path, reason: null, updatedAt: nowIso() });
  }
  stopAttempt(attemptId: string): void { for (const [id, controller] of this.#running) if (this.get(id).attemptId === attemptId) controller.abort(); }
  recover(): void {
    for (const row of this.tasks.db.prepare("SELECT data FROM task_integrations WHERE state IN ('preparing','checking','ready')").all() as { data: string }[]) {
      const record = TaskIntegration.parse(storedJson(row));
      this.#blocked(record, "Daemon restarted during integration. Inspect retained work and prepare again.");
    }
  }
  async shutdown(): Promise<void> {
    this.#closing = true; for (const controller of this.#running.values()) controller.abort();
    await Promise.allSettled([...this.#settling]);
    if (this.#terminationUnknown) throw new Error("An owned integration check could not be confirmed stopped; retain its state for reconciliation.");
  }
}
