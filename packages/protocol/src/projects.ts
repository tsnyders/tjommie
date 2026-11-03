import { z } from "zod";

const Id = z.string().min(1).max(128);
export const CommitSha = z.string().regex(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/);
const Instant = z.string().datetime({ offset: true });
const Revision = z.number().int().positive();
const Ref = z.string().min(12).max(200).regex(/^refs\/heads\/[A-Za-z0-9][A-Za-z0-9/._-]*$/);
export const ProjectRole = z.enum(["lead", "developer", "reviewer", "tester"]);
export const ProjectMember = z.strictObject({ botId: Id, roles: z.array(ProjectRole).min(1).max(4), instructions: z.string().max(20_000).default("") });
export type ProjectMember = z.infer<typeof ProjectMember>;
export const ProjectCheck = z.strictObject({ id: Id, label: z.string().trim().min(1).max(120), command: z.string().min(1).max(2000),
  args: z.array(z.string().max(4000)).max(100), timeoutMs: z.number().int().min(1000).max(3_600_000).default(120_000) });
export type ProjectCheck = z.infer<typeof ProjectCheck>;
export const ProjectRepository = z.strictObject({ id: Id, name: z.string(), createdAt: Instant });
export type ProjectRepository = z.infer<typeof ProjectRepository>;
export const ProjectInput = z.strictObject({ name: z.string().trim().min(1).max(80), repositoryId: Id, baseRef: Ref, targetRef: Ref,
  roomId: Id.nullable().default(null), members: z.array(ProjectMember).min(1).max(40), checks: z.array(ProjectCheck).max(20).default([]) });
export type ProjectInput = z.output<typeof ProjectInput>;
export const Project = ProjectInput.extend({ id: Id, state: z.enum(["active", "archived"]), revision: Revision, createdAt: Instant, updatedAt: Instant });
export type Project = z.infer<typeof Project>;
export const ProjectPatch = z.strictObject({ name: ProjectInput.shape.name.optional(), baseRef: Ref.optional(), targetRef: Ref.optional(),
  roomId: Id.nullable().optional(), members: z.array(ProjectMember).min(1).max(40).optional(), checks: z.array(ProjectCheck).max(20).optional() });
export type ProjectPatch = z.infer<typeof ProjectPatch>;

export const TaskState = z.enum(["todo", "running", "review", "done", "blocked", "cancelled"]);
export const TaskInput = z.strictObject({ projectId: Id, title: z.string().trim().min(1).max(200), instructions: z.string().trim().min(1).max(100_000),
  acceptance: z.array(z.string().trim().min(1).max(2000)).max(50).default([]), assigneeId: Id, reviewerId: Id.nullable().default(null), testerId: Id.nullable().default(null) });
export type TaskInput = z.output<typeof TaskInput>;
export const ProjectTask = TaskInput.extend({ id: Id, state: TaskState, currentAttemptId: Id.nullable(), revision: Revision, createdAt: Instant, updatedAt: Instant });
export type ProjectTask = z.infer<typeof ProjectTask>;
export const TaskPatch = z.strictObject({ title: TaskInput.shape.title.optional(), instructions: TaskInput.shape.instructions.optional(),
  acceptance: z.array(z.string().trim().min(1).max(2000)).max(50).optional(), assigneeId: Id.optional(), reviewerId: Id.nullable().optional(), testerId: Id.nullable().optional() });
export type TaskPatch = z.infer<typeof TaskPatch>;
export const TaskPhase = z.enum(["develop", "review", "test"]);
export type TaskPhase = z.infer<typeof TaskPhase>;
export const TaskAttempt = z.strictObject({ id: Id, taskId: Id, ordinal: Revision, baseSha: CommitSha, headSha: CommitSha.nullable(),
  branch: Ref, worktreePath: z.string().min(1), phase: TaskPhase, state: z.enum(["provisioning", "queued", "running", "interrupted", "completed", "failed", "blocked", "cancelled", "cleaned"]),
  revision: Revision, botId: Id, workItemId: Id.nullable(), contextKey: z.string(), reason: z.string().max(2000).nullable(), createdAt: Instant, updatedAt: Instant });
export type TaskAttempt = z.infer<typeof TaskAttempt>;
export const TaskFinding = z.strictObject({ severity: z.enum(["critical", "high", "medium", "low"]), path: z.string().max(2000),
  line: z.number().int().positive().nullable(), message: z.string().min(1).max(4000) });
const Sha256 = z.string().regex(/^[a-f0-9]{64}$/);
export const ReviewCoverage = z.strictObject({ sourceSha: CommitSha, snapshotSha256: Sha256, diffSha256: Sha256, complete: z.literal(true),
  files: z.array(z.strictObject({ path: z.string().min(1).max(2000), sha256: Sha256, bytes: z.number().int().min(0).max(262_144) })).max(2048) });
export type ReviewCoverage = z.infer<typeof ReviewCoverage>;
export const TaskEvaluation = z.strictObject({ id: Id, attemptId: Id, revision: Revision, sourceSha: CommitSha,
  projectRevision: Revision,
  kind: z.enum(["review", "test"]), botId: Id.nullable(), state: z.enum(["running", "passed", "findings", "failed", "unavailable", "stale"]),
  worktreePath: z.string().nullable(),
  reviewCoverage: ReviewCoverage.nullable().optional(),
  reason: z.string().max(2000).nullable(), findings: z.array(TaskFinding).max(100), checks: z.array(z.strictObject({ checkId: Id,
    command: z.string(), args: z.array(z.string()), exitCode: z.number().int().nullable(), output: z.string().max(262_144), truncated: z.boolean() })).max(20),
  createdAt: Instant, updatedAt: Instant });
export type TaskEvaluation = z.infer<typeof TaskEvaluation>;
export const TaskIntegration = z.strictObject({ id: Id, attemptId: Id, revision: Revision, acceptedSha: CommitSha, expectedTargetSha: CommitSha,
  checksCompleted: z.boolean().default(false),
  projectRevision: Revision,
  targetRef: Ref, candidateSha: CommitSha.nullable(), branch: Ref, worktreePath: z.string(), promotionPath: z.string().nullable(),
  state: z.enum(["preparing", "checking", "ready", "integrated", "blocked"]), reason: z.string().max(2000).nullable(),
  checks: z.array(TaskEvaluation.shape.checks.element).max(20), createdAt: Instant, updatedAt: Instant });
export type TaskIntegration = z.infer<typeof TaskIntegration>;
export const TaskDetail = z.strictObject({ task: ProjectTask, attempts: z.array(TaskAttempt), evaluations: z.array(TaskEvaluation), integrations: z.array(TaskIntegration) });
export type TaskDetail = z.infer<typeof TaskDetail>;
export const TaskDiff = z.strictObject({ attemptId: Id, revision: Revision, sourceSha: CommitSha, baseSha: CommitSha, text: z.string().max(524_288), truncated: z.boolean(), dirty: z.boolean() });
export type TaskDiff = z.infer<typeof TaskDiff>;
export const TaskCleanupPreview = z.strictObject({ attemptId: Id, worktreePath: z.string(), headSha: CommitSha.nullable(), files: z.array(z.string()).max(200),
  truncated: z.boolean(), canRemove: z.boolean(), reason: z.string() });
export type TaskCleanupPreview = z.infer<typeof TaskCleanupPreview>;

const Empty = z.strictObject({});
const ExpectedTask = { id: Id, expectedRevision: Revision };
const ExactAttempt = { attemptId: Id, expectedRevision: Revision, sourceSha: CommitSha };
export const projectMethods = {
  "projects.repositories": { params: Empty, result: z.array(ProjectRepository) },
  "projects.registerRepository": { params: z.strictObject({ path: z.string().min(1).max(4000) }), result: ProjectRepository },
  "projects.list": { params: Empty, result: z.array(Project) },
  "projects.get": { params: z.strictObject({ id: Id }), result: Project },
  "projects.heads": { params: z.strictObject({ id: Id }), result: z.strictObject({ baseSha: CommitSha, targetSha: CommitSha }) },
  "projects.create": { params: ProjectInput, result: Project },
  "projects.update": { params: z.strictObject({ id: Id, expectedRevision: Revision, patch: ProjectPatch }), result: Project },
  "projects.archive": { params: z.strictObject({ id: Id, expectedRevision: Revision }), result: Project },
  "tasks.list": { params: z.strictObject({ projectId: Id }), result: z.array(ProjectTask) },
  "tasks.create": { params: TaskInput, result: ProjectTask },
  "tasks.update": { params: z.strictObject({ ...ExpectedTask, patch: TaskPatch }), result: ProjectTask },
  "tasks.get": { params: z.strictObject({ id: Id }), result: TaskDetail },
  "tasks.start": { params: z.strictObject({ ...ExpectedTask, requestId: Id }), result: TaskAttempt },
  "tasks.resume": { params: z.strictObject({ ...ExpectedTask, requestId: Id }), result: TaskAttempt },
  "tasks.stop": { params: z.strictObject(ExpectedTask), result: ProjectTask },
  "tasks.cancel": { params: z.strictObject(ExpectedTask), result: ProjectTask },
  "tasks.diff": { params: z.strictObject({ attemptId: Id }), result: TaskDiff },
  "tasks.cleanupPreview": { params: z.strictObject({ attemptId: Id }), result: TaskCleanupPreview },
  "tasks.cleanup": { params: z.strictObject({ attemptId: Id, expectedHeadSha: CommitSha, confirm: z.literal("remove_clean_worktree") }), result: TaskAttempt },
  "tasks.evaluate": { params: z.strictObject({ ...ExactAttempt, kind: z.enum(["review", "test"]) }), result: TaskEvaluation },
  "tasks.integrate": { params: z.strictObject({ ...ExactAttempt, expectedTargetSha: CommitSha, requestId: Id }), result: TaskIntegration },
  "tasks.promote": { params: z.strictObject({ id: Id, expectedTargetSha: CommitSha }), result: TaskIntegration },
} as const;
export const projectEvents = { "project.updated": Project, "task.updated": ProjectTask, "task.attemptUpdated": TaskAttempt,
  "task.evaluationUpdated": TaskEvaluation, "task.integrationUpdated": TaskIntegration } as const;
export type ProjectMethod = keyof typeof projectMethods;
export type ProjectHandlers = { [M in ProjectMethod]: (params: z.output<(typeof projectMethods)[M]["params"]>) => z.output<(typeof projectMethods)[M]["result"]> | Promise<z.output<(typeof projectMethods)[M]["result"]>> };
export type ProjectBroadcast = <E extends keyof typeof projectEvents>(event: E, data: z.output<(typeof projectEvents)[E]>) => void;
