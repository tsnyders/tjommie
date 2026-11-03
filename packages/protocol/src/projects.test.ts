import { expect, it } from "vitest";
import { projectMethods, ProjectInput, ProjectPatch, TaskInput, TaskPatch } from "./projects.ts";

it("rejects remote-style arbitrary execution fields and invalid exact-commit/ref inputs", () => {
  expect(projectMethods["tasks.evaluate"].params.safeParse({ attemptId: "a", expectedRevision: 1, sourceSha: "a".repeat(40), kind: "test", command: "injected" }).success).toBe(false);
  expect(projectMethods["tasks.integrate"].params.safeParse({ attemptId: "a", expectedRevision: 1, sourceSha: "HEAD~1", expectedTargetSha: "a".repeat(40), requestId: "r" }).success).toBe(false);
  expect(ProjectInput.safeParse({ name: "Project", repositoryId: "r", baseRef: "--upload-pack=bad", targetRef: "refs/heads/main", members: [] }).success).toBe(false);
  expect(TaskInput.safeParse({ projectId: "p", title: "", instructions: "x", assigneeId: "b" }).success).toBe(false);
});

it("does not apply create defaults when patching unrelated metadata", () => {
  expect(ProjectPatch.parse({ targetRef: "refs/heads/delivery" })).toEqual({ targetRef: "refs/heads/delivery" });
  expect(TaskPatch.parse({ title: "New title" })).toEqual({ title: "New title" });
});
