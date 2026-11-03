// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Project, ProjectTask, TaskDetail } from "../../../protocol/src/projects";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import ProjectsPage from "@/app/projects/page";

const fixture = vi.hoisted(() => ({ request: vi.fn<(method: string, params: Record<string, unknown>) => Promise<unknown>>(), notify: () => {} }));
vi.mock("@/lib/connection", () => ({ request: fixture.request, daemon: () => ({ subscribe: (listener: (event: { event: string }) => void) => { fixture.notify = () => listener({ event: "project.updated" }); return () => {}; } }) }));
vi.mock("@/lib/native-capability", () => ({ phoneCaptionClear: () => "" }));
vi.mock("@/components/projects/project-editor", () => ({ ProjectEditor: () => null }));
vi.mock("@/components/projects/task-editor", () => ({ TaskEditor: () => null }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);

const timestamp = "2026-11-03T00:00:00.000Z";
const project = Project.parse({ id: "project", name: "UI team", repositoryId: "repo", roomId: null, baseRef: "refs/heads/main", targetRef: "refs/heads/phase4", members: [{ botId: "dev", roles: ["developer"] }], checks: [], state: "active", revision: 7, createdAt: timestamp, updatedAt: timestamp });
const tasks = ["First task", "Second task"].map((title, index) => ProjectTask.parse({ id: `task-${index}`, projectId: project.id, title, instructions: "Review UI", assigneeId: "dev", reviewerId: "review", testerId: "test", state: "todo", currentAttemptId: null, revision: 1, createdAt: timestamp, updatedAt: timestamp }));
let root: Root;
const scroll = vi.fn();
const button = (label: string, parent: ParentNode = document): HTMLButtonElement => {
  const found = [...parent.querySelectorAll("button")].find((item) => item.textContent === label);
  if (!found) throw new Error(`Missing button: ${label}`);
  return found;
};
const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
async function click(target: HTMLButtonElement) { await act(async () => target.click()); await settle(); }

beforeEach(async () => {
  window.history.replaceState(null, "", "/projects/?project=project");
  update(() => ({ ...initialState, connection: { status: "ready" } }));
  scroll.mockClear();
  vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(scroll);
  fixture.request.mockReset().mockImplementation(async (method, params) => {
    switch (method) {
      case "projects.list": return [project];
      case "projects.repositories": return [];
      case "projects.heads": return { targetSha: null };
      case "tasks.list": return tasks;
      case "tasks.get": return TaskDetail.parse({ task: tasks.find((task) => task.id === params.id), attempts: [], evaluations: [], integrations: [] });
      case "projects.archive": return { ...project, state: "archived" };
      default: throw new Error(`Unexpected request: ${method}`);
    }
  });
  root = createRoot(document.body.appendChild(document.createElement("div")));
  await act(async () => root.render(<ProjectsPage />));
  await settle();
});
afterEach(async () => {
  await act(async () => root.unmount());
  document.body.replaceChildren();
  update(() => initialState);
  vi.restoreAllMocks();
});

it("requires a separate confirmation to archive the selected team and preserves its revision", async () => {
  await click(button("Archive team"));
  expect(fixture.request.mock.calls.filter(([method]) => method === "projects.archive")).toHaveLength(0);
  const dialog = document.querySelector('[role="dialog"]')!;
  expect(dialog.textContent).toContain("Existing branches and files are kept");
  await click(button("Cancel", dialog));
  expect(fixture.request.mock.calls.filter(([method]) => method === "projects.archive")).toHaveLength(0);
  await click(button("Archive team"));
  await click(button("Archive team", document.querySelector('[role="dialog"]')!));
  expect(fixture.request.mock.calls.filter(([method]) => method === "projects.archive")).toEqual([["projects.archive", { id: project.id, expectedRevision: 7 }]]);
});

it("brings a selected task into view and focus once, without stealing focus on background refresh", async () => {
  const taskButton = document.querySelector<HTMLButtonElement>('[aria-label="Project tasks"] li:nth-child(2) button')!;
  await click(taskButton);
  expect(scroll).toHaveBeenCalledOnce();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Task: Second task");
  taskButton.focus();
  await act(async () => fixture.notify()); await settle();
  expect(scroll).toHaveBeenCalledOnce();
  expect(document.activeElement).toBe(taskButton);
  await click(taskButton);
  expect(scroll).toHaveBeenCalledTimes(2);
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Task: Second task");
  expect(document.querySelector('[aria-label="Task: Second task"]')).not.toBeNull();
});
