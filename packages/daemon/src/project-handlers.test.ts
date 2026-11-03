import { expect, it } from "vitest";
import { projectHandlers } from "./project-handlers.ts";
import { projectFixture } from "./project-test-support.ts";
import { TaskIntegrations } from "./task-integration.ts";

it("requires an authenticated live local context to register host paths and refuses paid starts from a closed connection", () => {
  const h = projectFixture();
  try {
    const handlers = projectHandlers(h.projects, h.tasks, new TaskIntegrations(h.tasks)), controller = new AbortController();
    const local = { connectionId: "local-synthetic", principal: { kind: "local" as const }, signal: controller.signal };
    const remote = { connectionId: "remote-synthetic", principal: { kind: "device" as const, deviceId: "device-synthetic" }, signal: controller.signal };
    expect(() => handlers["projects.registerRepository"]({ path: h.repoPath }, remote)).toThrow(/host PC/);
    expect(() => handlers["projects.registerRepository"]({ path: h.repoPath })).toThrow(/host PC/);
    expect(handlers["projects.registerRepository"]({ path: h.repoPath }, local)).toEqual({ id: h.repository.id, name: h.repository.name, createdAt: h.repository.createdAt });
    const task = h.newTask(); controller.abort();
    expect(() => handlers["tasks.start"]({ id: task.id, expectedRevision: task.revision, requestId: "closed" }, local)).toThrow(/connection closed/);
    expect(h.delivered).toHaveLength(0);
  } finally { h.db.close(); }
}, 30_000);

it("keeps executable check configuration host-local while allowing remote team edits", () => {
  const h = projectFixture();
  try {
    const handlers = projectHandlers(h.projects, h.tasks, new TaskIntegrations(h.tasks));
    const signal = new AbortController().signal;
    const local = { connectionId: "local", principal: { kind: "local" as const }, signal };
    const remote = { connectionId: "remote", principal: { kind: "device" as const, deviceId: "paired" }, signal };
    const changed = [{ ...h.project.checks[0], command: "unaccepted-program" }];
    expect(() => handlers["projects.update"]({ id: h.project.id, expectedRevision: h.project.revision, patch: { checks: changed } }, remote)).toThrow(/host PC/);
    expect(() => handlers["projects.create"]({ ...h.project, name: "Remote copy", checks: changed }, remote)).toThrow(/host PC/);
    const renamed = handlers["projects.update"]({ id: h.project.id, expectedRevision: h.project.revision, patch: { name: "Remote edit", checks: h.project.checks } }, remote);
    expect(renamed).toMatchObject({ name: "Remote edit", checks: h.project.checks });
    expect(handlers["projects.update"]({ id: h.project.id, expectedRevision: h.project.revision + 1, patch: { checks: changed } }, local)).toMatchObject({ checks: changed });
  } finally { h.db.close(); }
}, 30_000);
