import { existsSync } from "node:fs";
import { ProjectTask, TaskAttempt, TaskEvaluation, TaskInput, TaskPatch, type TaskCleanupPreview, type TaskDetail, type TaskDiff } from "../../protocol/src/projects.ts";
import { getStoredBot, type StoredBot } from "./bots.ts";
import { newId, nowIso, transaction } from "./db.ts";
import { Projects, storedJson } from "./projects.ts";
import { enqueue, getWorkItem, setStatus, type WorkItem } from "./queue.ts";
import type { TurnResult } from "./runner.ts";
import { RpcError } from "./server.ts";
import { executionKey, type TaskExecutionContext } from "./task-execution.ts";
import { frozenReviewContext, reviewCoverage, reviewSnapshot, type ReviewRuntime } from "./project-review.ts";
import { CheckTerminationError, HostCheckRuntime, type CheckRuntime } from "./project-checks.ts";
import { TaskIntegration as IntegrationSchema } from "../../protocol/src/projects.ts";
import type { ManagedWorktree } from "./worktrees.ts";

export type TaskRunner = { deliver(item: WorkItem): WorkItem; currentWork(botId: string): WorkItem | null; stopItem(id: string, notice: string): Promise<void>; retired?(botId: string): Promise<void> };
const activeAttempt = (attempt: TaskAttempt): boolean => ["provisioning", "queued", "running"].includes(attempt.state);
const evaluationData = (row: { data: string } | undefined): TaskEvaluation => { const value = TaskEvaluation.parse(storedJson(row)); return { ...value, reviewCoverage: value.reviewCoverage ?? null }; };

export class Tasks {
  readonly projects: Projects;
  readonly runner: TaskRunner;
  readonly #review?: ReviewRuntime;
  readonly #checks?: CheckRuntime;
  readonly #operations = new Map<string, AbortController>();
  readonly #settling = new Set<Promise<TaskEvaluation>>();
  #closing = false;
  #terminationUnknown = false;
  constructor(projects: Projects, runner: TaskRunner, options: { review?: ReviewRuntime; checks?: CheckRuntime } = {}) {
    this.projects = projects; this.runner = runner; this.#review = options.review; this.#checks = options.checks;
  }
  get db() { return this.projects.db; }
  get emit() { return this.projects.emit; }
  task(id: string): ProjectTask { return ProjectTask.parse(storedJson(this.db.prepare("SELECT data FROM project_tasks WHERE id=?").get(id) as { data: string } | undefined)); }
  attempt(id: string): TaskAttempt { return TaskAttempt.parse(storedJson(this.db.prepare("SELECT data FROM task_attempts WHERE id=?").get(id) as { data: string } | undefined)); }
  list(projectId: string): ProjectTask[] {
    this.projects.get(projectId);
    return (this.db.prepare("SELECT data FROM project_tasks WHERE project_id=? ORDER BY rowid").all(projectId) as { data: string }[]).map((row) => ProjectTask.parse(storedJson(row)));
  }
  detail(id: string): TaskDetail {
    let task = this.task(id);
    if (task.currentAttemptId) { this.refresh(task.currentAttemptId); task = this.task(id); }
    const attempts = (this.db.prepare("SELECT data FROM task_attempts WHERE task_id=? ORDER BY ordinal").all(id) as { data: string }[]).map((row) => TaskAttempt.parse(storedJson(row)));
    const evaluations = (this.db.prepare("SELECT e.data FROM task_evaluations e JOIN task_attempts a ON a.id=e.attempt_id WHERE a.task_id=? ORDER BY e.rowid").all(id) as { data: string }[]).map(evaluationData);
    const integrations = (this.db.prepare("SELECT i.data FROM task_integrations i JOIN task_attempts a ON a.id=i.attempt_id WHERE a.task_id=? ORDER BY i.rowid").all(id) as { data: string }[]).map((row) => IntegrationSchema.parse(storedJson(row)));
    return { task, attempts, evaluations, integrations };
  }
  saveTask(task: ProjectTask): void {
    this.db.prepare("UPDATE project_tasks SET state=?,revision=?,data=? WHERE id=?").run(task.state, task.revision, JSON.stringify(task), task.id);
    this.emit("task.updated", task);
  }
  saveAttempt(attempt: TaskAttempt): void {
    this.db.prepare("UPDATE task_attempts SET state=?,data=? WHERE id=?").run(attempt.state, JSON.stringify(attempt), attempt.id);
    this.emit("task.attemptUpdated", attempt);
  }
  saveEvaluation(evaluation: TaskEvaluation): void {
    evaluation = TaskEvaluation.parse({ reviewCoverage: null, ...evaluation });
    this.db.prepare("INSERT INTO task_evaluations VALUES (?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state,data=excluded.data")
      .run(evaluation.id, evaluation.attemptId, evaluation.revision, evaluation.sourceSha, evaluation.kind, evaluation.state, JSON.stringify(evaluation));
    this.emit("task.evaluationUpdated", evaluation);
  }
  #assignments(task: TaskInput): void {
    const project = this.projects.active(task.projectId);
    for (const [id, role] of [[task.assigneeId, "developer"], [task.reviewerId, "reviewer"], [task.testerId, "tester"]] as const) {
      if (!id) continue;
      getStoredBot(this.db, id);
      if (!project.members.some((member) => member.botId === id && member.roles.includes(role))) throw new RpcError("bad_params", `Assign a project ${role} to this task.`);
    }
  }
  create(input: TaskInput): ProjectTask {
    input = TaskInput.parse(input); this.#assignments(input);
    const timestamp = nowIso();
    const task: ProjectTask = { ...input, id: newId(), state: "todo", currentAttemptId: null, revision: 1, createdAt: timestamp, updatedAt: timestamp };
    this.db.prepare("INSERT INTO project_tasks VALUES (?,?,?,?,?)").run(task.id, task.projectId, task.state, task.revision, JSON.stringify(task));
    this.emit("task.updated", task); return task;
  }
  expected(id: string, revision: number): ProjectTask {
    const task = this.task(id);
    if (task.revision !== revision) throw new RpcError("bad_state", "This task changed. Refresh before continuing.");
    return task;
  }
  update(id: string, revision: number, patch: TaskPatch): ProjectTask {
    const task = this.expected(id, revision);
    if (task.currentAttemptId && activeAttempt(this.attempt(task.currentAttemptId))) throw new RpcError("busy", "Stop the current attempt before editing its task.");
    const next = ProjectTask.parse({ ...task, ...TaskPatch.parse(patch), state: "todo", currentAttemptId: null, revision: task.revision + 1, updatedAt: nowIso() });
    this.#assignments(next); this.saveTask(next); return next;
  }
  work(attempt: TaskAttempt): ManagedWorktree {
    const work = this.projects.worktrees.describe(attempt.id, attempt.baseSha, attempt.branch);
    if (work.path !== attempt.worktreePath) throw new RpcError("bad_state", "The persisted task worktree path changed.");
    return work;
  }
  repository(attempt: TaskAttempt) { return this.projects.repository(this.projects.get(this.task(attempt.taskId).projectId).repositoryId); }
  botBusy(botId: string): boolean { return !!this.db.prepare("SELECT 1 FROM project_bot_leases WHERE bot_id=?").get(botId); }
  lease(botId: string, ownerId: string, kind: string): void {
    this.projects.assertBotIdle(botId);
    if (this.botBusy(botId)) throw new RpcError("busy", "This bot already owns an active task or check.");
    this.db.prepare("INSERT INTO project_bot_leases VALUES (?,?,?)").run(botId, ownerId, kind);
  }
  release(botId: string, ownerId: string): void { this.db.prepare("DELETE FROM project_bot_leases WHERE bot_id=? AND owner_id=?").run(botId, ownerId); }
  #request(id: string, operation: string, taskId: string): string | null {
    const row = this.db.prepare("SELECT operation,task_id,result_id FROM task_requests WHERE id=?").get(id) as { operation: string; task_id: string; result_id: string } | undefined;
    if (!row) return null;
    if (row.operation !== operation || row.task_id !== taskId) throw new RpcError("bad_params", "This request ID was already used for another action.");
    return row.result_id;
  }
  #context(attempt: TaskAttempt): TaskExecutionContext {
    const task = this.task(attempt.taskId), project = this.projects.active(task.projectId);
    const member = project.members.find((candidate) => candidate.botId === attempt.botId);
    if (!member?.roles.includes("developer")) throw new RpcError("bad_state", "The task's developer assignment changed.");
    const feedback = (this.db.prepare("SELECT data FROM task_evaluations WHERE attempt_id=? ORDER BY rowid DESC LIMIT 2").all(attempt.id) as { data: string }[])
      .map(evaluationData).flatMap((evaluation) => [evaluation.reason ?? "", ...evaluation.findings.map((finding) => `${finding.severity} ${finding.path}:${finding.line ?? ""}: ${finding.message}`),
        ...evaluation.checks.filter((check) => check.exitCode !== 0).map((check) => `${check.command}: ${check.output.slice(0, 4000)}`)]).filter(Boolean).join("\n").slice(0, 12_000);
    const instructions = [`Project: ${project.name}. Task: ${task.title}.`, task.instructions,
      ...task.acceptance.map((item) => `Acceptance: ${item}`), member.instructions, feedback ? `Previous independent feedback:\n${feedback}` : "",
      "Work only in the assigned managed worktree. Commit completed task changes on its assigned branch with the existing Git identity. Do not switch or change the target branch, push, rebase, stash or remove worktrees. Report checks accurately; use share_file for deliverables."].filter(Boolean).join("\n");
    const context = { botId: attempt.botId, attemptId: attempt.id, revision: attempt.revision, phase: attempt.phase, cwd: attempt.worktreePath, instructions };
    return Object.freeze({ ...context, key: executionKey(context) });
  }
  #matchesActiveWork(attempt: TaskAttempt, item: WorkItem): boolean {
    const task = this.task(attempt.taskId);
    return task.currentAttemptId === attempt.id && this.projects.get(task.projectId).state === "active"
      && ["queued", "running"].includes(attempt.state) && attempt.botId === item.botId
      && attempt.revision === item.taskRevision && attempt.contextKey === item.executionContextKey
      && !!this.db.prepare("SELECT 1 FROM project_bot_leases WHERE bot_id=? AND owner_id=? AND kind='develop'").get(item.botId, attempt.id);
  }
  #pendingReplies(attempt: TaskAttempt): boolean {
    return !!this.db.prepare("SELECT 1 FROM exchanges WHERE from_bot_id=? AND task_attempt_id=? AND task_revision=? AND execution_context_key=? AND status='waiting' LIMIT 1")
      .get(attempt.botId, attempt.id, attempt.revision, attempt.contextKey)
      || !!this.db.prepare("SELECT 1 FROM work_items WHERE bot_id=? AND task_attempt_id=? AND task_revision=? AND execution_context_key=? AND source='bot_reply' AND status IN ('queued','running') LIMIT 1")
        .get(attempt.botId, attempt.id, attempt.revision, attempt.contextKey);
  }
  executionContext(bot: StoredBot, item: WorkItem): TaskExecutionContext | null {
    if (!item.taskAttemptId) {
      const binding = this.db.prepare("SELECT project_id FROM project_members WHERE bot_id=? AND active=1").get(bot.id) as { project_id: string } | undefined;
      if (!binding) return null;
      const project = this.projects.active(binding.project_id), member = project.members.find((candidate) => candidate.botId === bot.id);
      const context = { botId: bot.id, attemptId: `discussion-${project.id}`, revision: project.revision, phase: "discussion" as const, cwd: bot.homeFolder,
        instructions: `Project team: ${project.name}. This is a discussion turn in your home folder. Propose task drafts and assignments in the room; coding in the repository begins only after the person starts a task. Do not use the user's repository checkout as your working folder.\n${member?.instructions ?? ""}` };
      return Object.freeze({ ...context, key: executionKey(context) });
    }
    const attempt = this.attempt(item.taskAttemptId);
    if (attempt.botId !== bot.id || !this.#matchesActiveWork(attempt, item)) throw new RpcError("bad_state", "This task attempt is no longer active.");
    this.projects.worktrees.assertOwned(this.repository(attempt), this.work(attempt));
    const context = this.#context(attempt);
    if (context.key !== item.executionContextKey || context.key !== attempt.contextKey) throw new RpcError("bad_state", "The task context changed.");
    return context;
  }
  #queue(attempt: TaskAttempt): TaskAttempt {
    const context = this.#context(attempt), task = this.task(attempt.taskId);
    const project = this.projects.active(task.projectId);
    let queued: WorkItem | undefined;
    transaction(this.db, () => {
      queued = enqueue(this.db, { botId: attempt.botId, source: "task", text: context.instructions,
        conversationId: project.roomId ?? attempt.botId, taskAttemptId: attempt.id, taskRevision: attempt.revision, executionContextKey: context.key });
      this.saveAttempt({ ...attempt, state: "queued", contextKey: context.key, workItemId: queued.id, updatedAt: nowIso() });
    });
    this.runner.deliver(queued!); return this.attempt(attempt.id);
  }
  start(id: string, revision: number, requestId: string): TaskAttempt {
    if (this.#closing) throw new RpcError("bad_state", "The task service is closing.");
    const repeated = this.#request(requestId, "start", id); if (repeated) return this.attempt(repeated);
    const task = this.expected(id, revision), project = this.projects.active(task.projectId);
    this.#assignments(task);
    if (task.currentAttemptId && activeAttempt(this.attempt(task.currentAttemptId))) throw new RpcError("busy", "This task already has an active attempt.");
    const repo = this.projects.repository(project.repositoryId), baseSha = this.projects.worktrees.resolveRef(repo, project.baseRef);
    const attemptId = newId(), work = this.projects.worktrees.describe(attemptId, baseSha), timestamp = nowIso();
    const ordinal = Number((this.db.prepare("SELECT COALESCE(MAX(ordinal),0)+1 AS ordinal FROM task_attempts WHERE task_id=?").get(id) as { ordinal: number }).ordinal);
    const attempt: TaskAttempt = { id: attemptId, taskId: id, ordinal, baseSha, headSha: baseSha, branch: work.branch, worktreePath: work.path,
      phase: "develop", state: "provisioning", revision: 1, botId: task.assigneeId, workItemId: null, contextKey: "", reason: null, createdAt: timestamp, updatedAt: timestamp };
    transaction(this.db, () => {
      this.lease(attempt.botId, attempt.id, "develop");
      this.db.prepare("INSERT INTO task_attempts VALUES (?,?,?,?,?,?,?,?)").run(attempt.id, id, ordinal, attempt.state, attempt.botId, attempt.worktreePath, attempt.branch, JSON.stringify(attempt));
      this.db.prepare("INSERT INTO task_requests VALUES (?,?,?,?)").run(requestId, "start", id, attempt.id);
      this.saveTask({ ...task, currentAttemptId: attempt.id, state: "running", revision: task.revision + 1, updatedAt: timestamp });
    });
    try { this.projects.worktrees.create(repo, work); return this.#queue(attempt); }
    catch { this.block(attempt, "Worktree setup did not finish. Retained work must be reconciled before retrying."); return this.attempt(attempt.id); }
  }
  resume(id: string, revision: number, requestId: string): TaskAttempt {
    if (this.#closing) throw new RpcError("bad_state", "The task service is closing.");
    const repeated = this.#request(requestId, "resume", id); if (repeated) return this.attempt(repeated);
    const task = this.expected(id, revision); this.projects.active(task.projectId);
    if (!task.currentAttemptId) throw new RpcError("bad_state", "Start a new task attempt first.");
    const old = this.attempt(task.currentAttemptId);
    if (activeAttempt(old)) throw new RpcError("busy", "This attempt is still active.");
    this.projects.worktrees.assertOwned(this.repository(old), this.work(old));
    const next: TaskAttempt = { ...old, state: "queued", revision: old.revision + 1, reason: null, updatedAt: nowIso() };
    transaction(this.db, () => {
      this.lease(old.botId, old.id, "develop"); this.invalidate(old.id);
      this.saveTask({ ...task, state: "running", revision: task.revision + 1, updatedAt: nowIso() });
      this.db.prepare("INSERT INTO task_requests VALUES (?,?,?,?)").run(requestId, "resume", id, old.id);
    });
    try { return this.#queue(next); } catch { this.block(next, "This task could not resume. Its files were retained."); return this.attempt(next.id); }
  }
  turnStarted(item: WorkItem): void {
    if (!item.taskAttemptId) return;
    const attempt = this.attempt(item.taskAttemptId);
    if (this.#matchesActiveWork(attempt, item) && (attempt.workItemId === item.id || item.source === "bot_reply")) {
      this.saveAttempt({ ...attempt, workItemId: item.id, state: "running", reason: null, updatedAt: nowIso() });
    }
  }
  turnRequeued(item: WorkItem): void {
    if (!item.taskAttemptId) return;
    const attempt = this.attempt(item.taskAttemptId);
    if (this.#matchesActiveWork(attempt, item) && attempt.workItemId === item.id) this.saveAttempt({ ...attempt, state: "queued", reason: "Waiting for the provider/account to become ready.", updatedAt: nowIso() });
  }
  turnEnded(item: WorkItem, result: TurnResult): void {
    if (this.#closing || !item.taskAttemptId) return;
    const current = this.attempt(item.taskAttemptId);
    if (!this.#matchesActiveWork(current, item) || current.workItemId !== item.id) return;
    const settle = async (): Promise<void> => {
      await this.runner.retired?.(item.botId);
      if (this.#closing) return;
      const attempt = this.attempt(item.taskAttemptId!);
      if (!this.#matchesActiveWork(attempt, item) || attempt.workItemId !== item.id) return;
      if (result.outcome !== "done") { this.block(attempt, result.outcome === "interrupted" ? "Stopped. Resume explicitly to continue." : "The developer turn failed. Inspect the retained work.", result.outcome === "interrupted" ? "interrupted" : "failed"); return; }
      if (this.#pendingReplies(attempt)) {
        this.saveAttempt({ ...attempt, state: "running", reason: "Waiting for requested bot replies.", updatedAt: nowIso() }); return;
      }
      try {
        const repo = this.repository(attempt), work = this.work(attempt), headSha = this.projects.worktrees.head(repo, work);
        if (this.projects.worktrees.dirty(repo, work, headSha)) { this.block({ ...attempt, headSha }, "Commit the retained task changes before review."); return; }
        this.saveAttempt({ ...attempt, headSha, state: "completed", updatedAt: nowIso() }); this.release(attempt.botId, attempt.id);
        const task = this.task(attempt.taskId); this.saveTask({ ...task, state: "review", revision: task.revision + 1, updatedAt: nowIso() });
      } catch { this.block(attempt, "The managed worktree changed or is missing. Retained work needs reconciliation."); }
    };
    void settle().catch(() => {
      if (this.#closing) return;
      const attempt = this.attempt(item.taskAttemptId!);
      if (!this.#matchesActiveWork(attempt, item) || attempt.workItemId !== item.id) return;
      this.saveAttempt({ ...attempt, state: "blocked", reason: "Process exit is unconfirmed; the bot lease is retained.", updatedAt: nowIso() });
      const task = this.task(attempt.taskId); this.saveTask({ ...task, state: "blocked", revision: task.revision + 1, updatedAt: nowIso() });
    });
  }
  block(attempt: TaskAttempt, message: string, state: TaskAttempt["state"] = "blocked"): void {
    this.saveAttempt({ ...attempt, state, reason: message, updatedAt: nowIso() }); this.release(attempt.botId, attempt.id);
    const task = this.task(attempt.taskId); this.saveTask({ ...task, state: "blocked", revision: task.revision + 1, updatedAt: nowIso() });
  }
  async stop(id: string, revision: number, cancel = false): Promise<ProjectTask> {
    const task = this.expected(id, revision);
    if (!task.currentAttemptId) { const next: ProjectTask = { ...task, state: cancel ? "cancelled" : "blocked", revision: task.revision + 1, updatedAt: nowIso() }; this.saveTask(next); return next; }
    const attempt = this.attempt(task.currentAttemptId);
    for (const [key, controller] of this.#operations) if (key.startsWith(`${attempt.id}:`)) controller.abort();
    if (attempt.workItemId) {
      const item = getWorkItem(this.db, attempt.workItemId);
      if (item.status === "queued") setStatus(this.db, item.id, "interrupted");
      else if (item.status === "running") { await this.runner.stopItem(item.id, "Task stopped; its worktree is retained."); await this.runner.retired?.(attempt.botId); }
    }
    this.saveAttempt({ ...this.attempt(attempt.id), state: cancel ? "cancelled" : "interrupted", reason: cancel ? "Cancelled; work retained." : "Stopped; resume explicitly.", updatedAt: nowIso() });
    this.release(attempt.botId, attempt.id);
    const current = this.task(id), next: ProjectTask = { ...current, state: cancel ? "cancelled" : "blocked", revision: current.revision + 1, updatedAt: nowIso() };
    this.saveTask(next); return next;
  }
  invalidate(attemptId: string): void {
    const rows = this.db.prepare("SELECT data FROM task_evaluations WHERE attempt_id=? AND state<>'stale'").all(attemptId) as { data: string }[];
    for (const row of rows) this.saveEvaluation({ ...evaluationData(row), state: "stale", reason: "The task source or execution revision changed.", updatedAt: nowIso() });
  }
  refresh(id: string): TaskAttempt {
    const attempt = this.attempt(id);
    if (activeAttempt(attempt) || attempt.state === "cancelled" || attempt.state === "cleaned") return attempt;
    try {
      const headSha = this.projects.worktrees.head(this.repository(attempt), this.work(attempt));
      const dirty = this.projects.worktrees.dirty(this.repository(attempt), this.work(attempt), headSha);
      if (headSha !== attempt.headSha || dirty && attempt.reason !== "Source changed; commit and rerun review and checks.") {
        this.invalidate(id);
        this.block({ ...attempt, headSha, revision: attempt.revision + 1 }, "Source changed; commit and rerun review and checks.");
      }
    } catch { if (attempt.reason !== "Managed worktree is missing or changed.") this.block(attempt, "Managed worktree is missing or changed."); }
    return this.attempt(id);
  }
  exact(id: string, revision: number, sha: string): TaskAttempt {
    const attempt = this.refresh(id);
    const task = this.task(attempt.taskId); this.projects.active(task.projectId);
    if (task.currentAttemptId !== id || attempt.revision !== revision || attempt.headSha !== sha || attempt.state !== "completed") throw new RpcError("bad_state", "The exact task commit is changed, dirty or still running.");
    return attempt;
  }
  diff(id: string): TaskDiff {
    const attempt = this.refresh(id), repo = this.repository(attempt), sha = this.projects.worktrees.head(repo, this.work(attempt));
    return { attemptId: id, revision: attempt.revision, baseSha: attempt.baseSha, sourceSha: sha, ...this.projects.worktrees.diff(repo, attempt.baseSha, sha), dirty: this.projects.worktrees.dirty(repo, this.work(attempt), sha) };
  }
  cleanupPreview(id: string): TaskCleanupPreview {
    const attempt = this.attempt(id);
    if (activeAttempt(attempt) || this.botBusy(attempt.botId)
      || this.db.prepare("SELECT 1 FROM task_evaluations WHERE attempt_id=? AND state='running'").get(id)
      || this.db.prepare("SELECT 1 FROM task_integrations WHERE attempt_id=? AND state IN ('preparing','checking','ready')").get(id)) {
      return { attemptId: id, worktreePath: attempt.worktreePath, headSha: attempt.headSha, files: [], truncated: false, canRemove: false, reason: "Stop active work and confirm its process exit before cleanup." };
    }
    try {
      const repo = this.repository(attempt), work = this.work(attempt), headSha = this.projects.worktrees.head(repo, work), files = this.projects.worktrees.changedFiles(repo, work, headSha);
      return { attemptId: id, worktreePath: work.path, headSha, files: files.slice(0, 200), truncated: files.length > 200, canRemove: !files.length,
        reason: files.length ? "Modified and untracked files are retained. Save them before removing this worktree." : "This clean worktree can be removed. Its branch and task history remain." };
    } catch { return { attemptId: id, worktreePath: attempt.worktreePath, headSha: null, files: [], truncated: false, canRemove: false, reason: "The worktree is missing or its ownership cannot be verified. Nothing will be removed." }; }
  }
  cleanup(id: string, expectedHeadSha: string): TaskAttempt {
    const preview = this.cleanupPreview(id), attempt = this.attempt(id);
    if (!preview.canRemove || preview.headSha !== expectedHeadSha) throw new RpcError("bad_state", "The cleanup preview is stale or contains retained work.");
    this.projects.worktrees.remove(this.repository(attempt), this.work(attempt), expectedHeadSha);
    const cleaned: TaskAttempt = { ...attempt, state: "cleaned", reason: "Worktree removed explicitly. The branch and history are retained.", updatedAt: nowIso() };
    this.saveAttempt(cleaned); return cleaned;
  }
  evaluate(id: string, revision: number, sha: string, kind: "review" | "test"): Promise<TaskEvaluation> {
    if (this.#closing) return Promise.reject(new RpcError("bad_state", "The task service is closing."));
    const operation = this.#evaluate(id, revision, sha, kind); this.#settling.add(operation);
    void operation.finally(() => this.#settling.delete(operation)).catch(() => {}); return operation;
  }
  async #evaluate(id: string, revision: number, sha: string, kind: "review" | "test"): Promise<TaskEvaluation> {
    const attempt = this.exact(id, revision, sha), task = this.task(attempt.taskId), project = this.projects.active(task.projectId);
    const botId = kind === "review" ? task.reviewerId : task.testerId, timestamp = nowIso();
    let evaluation: TaskEvaluation = { id: newId(), attemptId: id, revision, sourceSha: sha, projectRevision: project.revision, kind, botId, worktreePath: null, reviewCoverage: null, state: "running", reason: null, findings: [], checks: [], createdAt: timestamp, updatedAt: timestamp };
    const unavailable = !botId ? `Assign a ${kind === "review" ? "reviewer" : "tester"} first.` : kind === "review" && !this.#review ? "review_isolation_unavailable" : kind === "test" && !project.checks.length ? "Configure and accept project checks first." : null;
    if (unavailable) { evaluation = { ...evaluation, state: "unavailable", reason: unavailable }; this.saveEvaluation(evaluation); return evaluation; }
    const bot = getStoredBot(this.db, botId!);
    const checks = this.#checks ?? (bot.autonomy === "full" ? new HostCheckRuntime() : undefined);
    if (kind === "test" && !checks) { evaluation = { ...evaluation, state: "unavailable", reason: "check_isolation_unavailable: configured host checks require Full auto or an approved isolated check runtime." }; this.saveEvaluation(evaluation); return evaluation; }
    const controller = new AbortController(), key = `${id}:${evaluation.id}`;
    transaction(this.db, () => { this.lease(botId!, evaluation.id, kind); this.saveEvaluation(evaluation); });
    this.#operations.set(key, controller);
    let terminationUnknown = false;
    try {
      if (kind === "review") {
        const snapshot = reviewSnapshot(this.projects.worktrees, this.repository(attempt), attempt.baseSha, sha);
        const context = frozenReviewContext({ botId: botId!, projectId: project.id, taskId: task.id, attemptId: attempt.id, evaluationId: evaluation.id, revision,
          projectRevision: project.revision, taskRevision: task.revision,
          instructions: { reviewer: bot.instructions, role: project.members.find((member) => member.botId === botId && member.roles.includes("reviewer"))?.instructions ?? "", title: task.title, task: task.instructions, acceptance: task.acceptance } });
        const result = await this.#review!.review(snapshot, controller.signal, context);
        evaluation = TaskEvaluation.parse({ ...evaluation, state: result.state, reason: result.reason, findings: result.findings, reviewCoverage: result.coverage ?? null });
        if (["passed", "findings"].includes(evaluation.state) && JSON.stringify(evaluation.reviewCoverage) !== JSON.stringify(reviewCoverage(snapshot))) throw new Error("review_coverage_incomplete");
        if (evaluation.state === "passed" && evaluation.findings.length) evaluation.state = "findings";
        if (!["passed", "findings", "failed", "unavailable"].includes(evaluation.state)) throw new Error("Invalid review result.");
        if (this.task(task.id).revision !== task.revision || this.projects.get(project.id).revision !== project.revision || getStoredBot(this.db, botId!).instructions !== bot.instructions) evaluation = { ...evaluation, state: "stale", reason: "The review criteria changed during evaluation." };
      } else {
        const repo = this.repository(attempt), work = this.projects.worktrees.describe(`test-${evaluation.id}`, sha);
        this.projects.worktrees.create(repo, work); evaluation.worktreePath = work.path;
        for (const check of project.checks) {
          const result = await checks!.run(check, work.path, controller.signal); evaluation.checks.push(result);
          if (result.exitCode !== 0) break;
        }
        evaluation.state = evaluation.checks.length === project.checks.length && evaluation.checks.every((result) => result.exitCode === 0) ? "passed" : "failed";
        if (this.projects.worktrees.dirty(repo, work, sha, true)) { evaluation.state = "failed"; evaluation.reason = "Checks changed the isolated source. Preserve and inspect the check worktree."; }
      }
      const current = this.refresh(id);
      if (controller.signal.aborted || current.revision !== revision || current.headSha !== sha) evaluation = { ...evaluation, state: "stale", reason: "The attempt changed or was stopped during evaluation." };
    } catch (error) { terminationUnknown = error instanceof CheckTerminationError; this.#terminationUnknown ||= terminationUnknown; evaluation = { ...evaluation, reviewCoverage: null, state: kind === "review" ? "unavailable" : "failed", reason: terminationUnknown ? "Evaluation process termination is unconfirmed; its bot lease is retained for host reconciliation." : kind === "review" ? "review_isolation_unavailable or incomplete snapshot coverage" : "The configured check did not complete. Inspect retained check work." }; }
    finally { this.#operations.delete(key); if (!terminationUnknown) this.release(botId!, evaluation.id); }
    evaluation.updatedAt = nowIso(); this.saveEvaluation(evaluation);
    const currentTask = this.task(task.id);
    if (currentTask.currentAttemptId === id && currentTask.state !== "cancelled" && this.attempt(id).revision === revision) {
      if (evaluation.state === "findings" || evaluation.state === "failed") this.saveTask({ ...currentTask, state: "blocked", revision: currentTask.revision + 1, updatedAt: nowIso() });
      else if (evaluation.state === "passed" && this.accepted(attempt)) this.saveTask({ ...currentTask, state: "done", revision: currentTask.revision + 1, updatedAt: nowIso() });
    }
    return evaluation;
  }
  accepted(attempt: TaskAttempt): boolean {
    const task = this.task(attempt.taskId), project = this.projects.get(task.projectId);
    if (!task.reviewerId || !task.testerId || !project.checks.length || !attempt.headSha) return false;
    for (const kind of ["review", "test"] as const) {
      const row = this.db.prepare("SELECT data FROM task_evaluations WHERE attempt_id=? AND revision=? AND source_sha=? AND kind=? ORDER BY rowid DESC LIMIT 1").get(attempt.id, attempt.revision, attempt.headSha, kind) as { data: string } | undefined;
      if (!row) return false;
      const evaluation = evaluationData(row);
      if (evaluation.state !== "passed" || evaluation.projectRevision !== project.revision) return false;
      if (kind === "review" && (!evaluation.reviewCoverage || evaluation.reviewCoverage.sourceSha !== attempt.headSha)) return false;
    }
    return true;
  }
  recover(): void {
    for (const row of this.db.prepare("SELECT data FROM task_attempts WHERE state IN ('provisioning','queued','running')").all() as { data: string }[]) {
      const attempt = TaskAttempt.parse(storedJson(row));
      try {
        if (attempt.state === "queued" && attempt.workItemId && getWorkItem(this.db, attempt.workItemId).status === "queued") {
          this.executionContext(getStoredBot(this.db, attempt.botId), getWorkItem(this.db, attempt.workItemId)); continue;
        }
      } catch { /* Persist the mismatch without touching the worktree. */ }
      this.saveAttempt({ ...attempt, state: "interrupted", reason: existsSync(attempt.worktreePath) ? "Daemon restarted. Confirm the old process has exited before resuming retained work." : "Managed worktree is missing or changed.", updatedAt: nowIso() });
      const task = this.task(attempt.taskId); this.saveTask({ ...task, state: "blocked", revision: task.revision + 1, updatedAt: nowIso() });
    }
    for (const row of this.db.prepare("SELECT data FROM task_evaluations WHERE state='running'").all() as { data: string }[]) {
      const evaluation = evaluationData(row);
      this.saveEvaluation({ ...evaluation, state: "unavailable", reason: "Daemon restarted during evaluation; rerun explicitly.", updatedAt: nowIso() });
      // A persisted PID alone is unsafe to kill after restart; retain the lease until the host proves exit.
    }
  }
  confirmedExited(botId: string, ownerId: string): void {
    if (this.runner.currentWork(botId)) throw new RpcError("busy", "This bot still has a live turn.");
    this.release(botId, ownerId);
  }
  async shutdown(): Promise<void> {
    this.#closing = true; for (const controller of this.#operations.values()) controller.abort();
    await Promise.allSettled([...this.#settling]);
    if (this.#terminationUnknown) throw new Error("An owned check process could not be confirmed stopped; retain its task state for reconciliation.");
  }
}
