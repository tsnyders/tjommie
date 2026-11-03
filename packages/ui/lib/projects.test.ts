import { expect, it } from "vitest";
import { Project, TaskDetail } from "../../protocol/src/projects";
import { canIntegrate, latestEvaluation } from "./projects";

const sha = "a".repeat(40), timestamp = "2026-11-03T00:00:00.000Z";
const project = Project.parse({ id: "p", name: "Project", repositoryId: "r", roomId: null, baseRef: "refs/heads/main", targetRef: "refs/heads/main", members: [{ botId: "dev", roles: ["developer"] }],
  checks: [{ id: "test", label: "Test", command: "node", args: ["--test"] }], state: "active", revision: 1, createdAt: timestamp, updatedAt: timestamp });
const detail = TaskDetail.parse({ task: { id: "t", projectId: "p", title: "Task", instructions: "Change", assigneeId: "dev", reviewerId: "review", testerId: "test", state: "done", currentAttemptId: "a", revision: 2, createdAt: timestamp, updatedAt: timestamp },
  attempts: [{ id: "a", taskId: "t", ordinal: 1, baseSha: sha, headSha: sha, branch: "refs/heads/tjommie/a", worktreePath: "synthetic-worktree", phase: "develop", state: "completed", revision: 1, botId: "dev", workItemId: "w", contextKey: "ctx", reason: null, createdAt: timestamp, updatedAt: timestamp }],
  evaluations: ["review", "test"].map((kind) => ({ id: kind, attemptId: "a", revision: 1, projectRevision: 1, sourceSha: sha, kind, botId: kind, worktreePath: null, reviewCoverage: kind === "review" ? { sourceSha: sha, snapshotSha256: "b".repeat(64), diffSha256: "c".repeat(64), complete: true, files: [] } : null, state: "passed", reason: null, findings: [], checks: [], createdAt: timestamp, updatedAt: timestamp })), integrations: [] });

it("enables exact integration only for fresh independent results and a clean completed attempt", () => {
  expect(canIntegrate(project, detail, false)).toBe(true); expect(canIntegrate(project, detail, true)).toBe(false);
  expect(canIntegrate({ ...project, revision: 2 }, detail, false)).toBe(false);
  const stale = structuredClone(detail); stale.attempts[0].revision = 2; expect(canIntegrate(project, stale, false)).toBe(false);
  const unavailable = structuredClone(detail); unavailable.evaluations[0].state = "unavailable";
  expect(latestEvaluation(unavailable, unavailable.attempts[0], "review")?.state).toBe("unavailable"); expect(canIntegrate(project, unavailable, false)).toBe(false);
  const old = structuredClone(detail); old.evaluations[0].reviewCoverage = null; expect(canIntegrate(project, old, false)).toBe(false);
});
