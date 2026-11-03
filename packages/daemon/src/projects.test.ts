import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { projectFixture } from "./project-test-support.ts";
import { Tasks } from "./tasks.ts";
import { Projects } from "./projects.ts";
import { deleteRoom, getRoom } from "./rooms.ts";

const open: ReturnType<typeof projectFixture>[] = [];
vi.setConfig({ testTimeout: 30_000 });
afterEach(() => { for (const fixture of open.splice(0)) fixture.db.close(); });
const fixture = () => { const result = projectFixture(); open.push(result); return result; };

it("retains an idle active project's room until its project is archived or unlinked", () => {
  const h = fixture(), roomId = h.project.roomId!;
  expect(() => deleteRoom(h.db, roomId)).toThrow(/Archive or unlink/);
  expect(getRoom(h.db, roomId).id).toBe(roomId);
  h.projects.archive(h.project.id, h.project.revision);
  deleteRoom(h.db, roomId);
  expect(() => getRoom(h.db, roomId)).toThrow();
});

it("blocks membership and task leases while a bot lifecycle change holds it", () => {
  const h = fixture();
  let held = true;
  const projects = new Projects(h.db, h.worktrees, () => {}, () => { if (held) throw new Error("Bot lifecycle is changing"); });
  const tasks = new Tasks(projects, h.runner), task = h.newTask();
  expect(() => projects.update(h.project.id, h.project.revision, { name: "Changed" })).toThrow(/lifecycle/);
  expect(() => tasks.start(task.id, task.revision, "held")).toThrow(/lifecycle/);
  expect(h.delivered).toHaveLength(0);
  held = false;
  expect(projects.update(h.project.id, h.project.revision, { name: "Changed" }).name).toBe("Changed");
});

it("registers canonical host repositories once, creates a normal room, and starts no work on project/task creation", () => {
  const h = fixture();
  expect(h.projects.register(h.repoPath).id).toBe(h.repository.id);
  expect(h.projects.repositories()).toHaveLength(1);
  expect(h.project.roomId).toBeTruthy(); h.newTask();
  expect(h.delivered).toHaveLength(0);
  expect(h.db.prepare("SELECT COUNT(*) AS n FROM work_items").get()?.n).toBe(0);
  expect(() => h.projects.register(join(h.repoPath, "src"))).toThrow(/root folder/);
});

it("rejects duplicate membership, assignment outside a role, stale edits and busy project changes", () => {
  const h = fixture(), task = h.newTask();
  expect(() => h.projects.create({ name: "Other project", repositoryId: h.repository.id, baseRef: h.project.baseRef, targetRef: h.project.targetRef,
    members: h.project.members, checks: h.project.checks, roomId: h.project.roomId })).toThrow(/another active project/);
  expect(() => h.tasks.create({ projectId: task.projectId, title: "Wrong role", instructions: "Check", acceptance: [], assigneeId: h.bots[2].id, reviewerId: null, testerId: null })).toThrow(/developer/);
  expect(() => h.projects.update(h.project.id, 9, { name: "Changed" })).toThrow(/changed/);
  h.tasks.start(task.id, task.revision, "start");
  expect(() => h.projects.update(h.project.id, 1, { name: "Busy" })).toThrow(/Stop project/);
  expect(() => h.projects.archive(h.project.id, 1)).toThrow(/Stop project/);
});

it("archives a team without deleting its repository, tasks, room or worktree history", () => {
  const h = fixture(), task = h.newTask();
  const archived = h.projects.archive(h.project.id, h.project.revision);
  expect(archived.state).toBe("archived"); expect(h.tasks.task(task.id)).toEqual(task);
  expect(h.projects.repository(h.repository.id).path).toBe(h.repoPath);
  expect(h.db.prepare("SELECT id FROM conversations WHERE id=?").get(h.project.roomId!)).toBeTruthy();
  expect(() => h.tasks.start(task.id, task.revision, "after-archive")).toThrow(/archived/);
});

it("retains persistent tasks after a new service instance without rerunning settled work", async () => {
  const h = fixture(), task = h.newTask();
  const attempt = h.tasks.start(task.id, task.revision, "start");
  const complete = await h.complete(attempt, "src/a.txt", "A1\n");
  const next = new Tasks(h.projects, h.runner); next.recover();
  expect(next.attempt(complete.id).state).toBe("completed"); expect(h.delivered).toHaveLength(1);
  expect(next.task(task.id).state).toBe("review");
});

it("does not let a removed member's new project lease block its former project", () => {
  const h = fixture(), moved = h.project.members[0];
  const previous = h.projects.update(h.project.id, h.project.revision, { members: h.project.members.slice(1) });
  const next = h.projects.create({ name: "Next project", repositoryId: h.repository.id, baseRef: h.project.baseRef, targetRef: h.project.targetRef, roomId: null, members: [moved], checks: [] });
  const task = h.tasks.create({ projectId: next.id, title: "New project task", instructions: "Work here", acceptance: [], assigneeId: moved.botId, reviewerId: null, testerId: null });
  h.tasks.start(task.id, task.revision, "start-moved-member");
  expect(h.projects.busy(next.id)).toBe(true);
  expect(h.projects.busy(previous.id)).toBe(false);
  expect(h.projects.archive(previous.id, previous.revision).state).toBe("archived");
});
