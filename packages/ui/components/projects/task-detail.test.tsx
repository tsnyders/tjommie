import { renderToStaticMarkup } from "react-dom/server";
import { expect, it, vi } from "vitest";
import { Project, TaskDetail } from "../../../protocol/src/projects";
import { TaskDetailView } from "./task-detail";

vi.mock("@/lib/connection", () => ({ request: vi.fn() }));
const sourceSha = "a".repeat(40), timestamp = "2026-11-03T00:00:00.000Z";
const project = Project.parse({ id: "p", name: "Project", repositoryId: "r", roomId: null, baseRef: "refs/heads/main", targetRef: "refs/heads/main", members: [{ botId: "dev", roles: ["developer"] }], checks: [], state: "active", revision: 1, createdAt: timestamp, updatedAt: timestamp });
const detail = TaskDetail.parse({ task: { id: "t", projectId: "p", title: "Task", instructions: "Review", assigneeId: "dev", reviewerId: "reviewer", testerId: "tester", state: "review", currentAttemptId: null, revision: 1, createdAt: timestamp, updatedAt: timestamp }, attempts: [], integrations: [],
  evaluations: [{ id: "e", attemptId: "a", revision: 1, projectRevision: 1, sourceSha, kind: "review", botId: "reviewer", worktreePath: null, state: "passed", reason: null, findings: [], checks: [], createdAt: timestamp, updatedAt: timestamp,
    reviewCoverage: { sourceSha, snapshotSha256: "b".repeat(64), diffSha256: "c".repeat(64), complete: true, files: [{ path: "source.ts", bytes: 10, sha256: "d".repeat(64) }] } }] });
const render = (value: TaskDetail) => renderToStaticMarkup(<TaskDetailView project={project} detail={value} diff={null} targetSha={null} busy={false} connected onAction={() => {}} onEdit={() => {}} />);

it("shows the exact supplied coverage manifest and explains paid enforcement acceptance", () => {
  const html = render(detail); expect(html).toContain("Complete snapshot supplied"); expect(html).toContain("source.ts"); expect(html).toContain("b".repeat(64));
  expect(html).toContain("first review verifies"); expect(html).not.toContain("Review remains unavailable");
  const legacy = structuredClone(detail); legacy.evaluations[0].reviewCoverage = null;
  expect(render(legacy)).toContain("Complete source coverage is unavailable"); expect(render(legacy)).not.toContain("Complete snapshot supplied");
});
