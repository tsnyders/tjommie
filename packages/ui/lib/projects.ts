import type { Project, TaskAttempt, TaskDetail, TaskEvaluation } from "../../protocol/src/projects";

export function latestEvaluation(detail: TaskDetail, attempt: TaskAttempt, kind: "review" | "test"): TaskEvaluation | undefined {
  return detail.evaluations.filter((evaluation) => evaluation.attemptId === attempt.id && evaluation.revision === attempt.revision && evaluation.sourceSha === attempt.headSha && evaluation.kind === kind).at(-1);
}
export function canIntegrate(project: Project, detail: TaskDetail, dirty: boolean): boolean {
  const attempt = detail.attempts.find((candidate) => candidate.id === detail.task.currentAttemptId);
  return project.state === "active" && !dirty && attempt?.state === "completed" && !!detail.task.reviewerId && !!detail.task.testerId && project.checks.length > 0
    && ["review", "test"].every((kind) => {
      const result = latestEvaluation(detail, attempt, kind as "review" | "test");
      return result?.state === "passed" && result.projectRevision === project.revision
        && (kind !== "review" || result.reviewCoverage?.sourceSha === attempt.headSha);
    });
}
export function shortSha(sha: string | null): string { return sha?.slice(0, 12) ?? "Not recorded"; }
