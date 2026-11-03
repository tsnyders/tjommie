import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { deleteBot, getStoredBot, updateBot } from "./bots.ts";
import { projectFixture } from "./project-test-support.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { getWorkItem, type WorkItem } from "./queue.ts";
import { Runner } from "./runner.ts";
import { Tasks } from "./tasks.ts";
import { getExchange, Team } from "./team.ts";

class SyntheticSession implements ProviderSession {
  listener: (event: ProviderEvent) => void = () => {};
  sent: string[] = [];
  steered: string[] = [];
  canSteer = false;
  steerResult: Promise<boolean> | null = null;
  releaseSteer = (): void => {};
  exit = Promise.resolve();
  releaseExit = (): void => {};
  async start(): Promise<void> {}
  async send(text: string): Promise<void> { this.sent.push(text); }
  async steer(text: string): Promise<boolean> { if (this.canSteer || this.steerResult) this.steered.push(text); return this.steerResult ?? this.canSteer; }
  async interrupt(): Promise<void> { this.listener({ type: "turn_done", ok: false, error: null }); }
  answer(): void {}
  close(): void {}
  waitForExit(): Promise<void> { return this.exit; }
  pid(): undefined { return undefined; }
  onEvent(listener: (event: ProviderEvent) => void): void { this.listener = listener; }
  holdExit(): void { this.exit = new Promise<void>((resolve) => { this.releaseExit = resolve; }); }
  finish(text: string): void {
    this.listener({ type: "text", delta: text }); this.listener({ type: "text_end" });
    this.listener({ type: "turn_done", ok: true, error: null });
  }
}

const open: { db: ReturnType<typeof projectFixture>["db"]; tasks: Tasks; runner: Runner; sessions: { session: SyntheticSession }[] }[] = [];
afterEach(async () => {
  for (const h of open.splice(0)) {
    for (const entry of h.sessions) { entry.session.releaseExit(); entry.session.releaseSteer(); }
    await h.tasks.shutdown(); await h.runner.shutdown();
    await new Promise<void>((resolve) => setImmediate(resolve)); h.db.close();
  }
  vi.restoreAllMocks();
});
function fixture(options: { helperOutsideProject?: boolean } = {}) {
  const h = projectFixture(), developer = h.bots[0], helper = h.bots[1];
  const project = options.helperOutsideProject
    ? h.projects.update(h.project.id, h.project.revision, { members: h.project.members.filter((member) => member.botId !== helper.id) })
    : h.project;
  updateBot(h.db, developer.id, { canMessage: [helper.id] });
  const sessions: { botId: string; options: StartOptions; session: SyntheticSession }[] = [];
  let tasks!: Tasks, team!: Team;
  const runner = new Runner({ db: h.db, broadcast: () => {}, isSignedIn: () => true,
    createSession: (bot, options) => { const session = new SyntheticSession(); sessions.push({ botId: bot.id, options, session }); return session; },
    hooks: { executionContext: (bot, item) => tasks.executionContext(bot, item), turnStarted: (item) => tasks.turnStarted(item),
      turnEnded: (item, result) => { tasks.turnEnded(item, result); team.turnEnded(item, result); }, turnRequeued: (item) => tasks.turnRequeued(item), sessionClosed: (id) => team.sessionClosed(id) },
  });
  tasks = new Tasks(h.projects, { deliver: (item) => runner.deliver(item), currentWork: (id) => runner.currentWork(id), stopItem: (id, notice) => runner.stopItem(id, notice), retired: (id) => runner.retired(id) });
  team = new Team({ db: h.db, broadcast: () => {}, deliver: (item) => { runner.deliver(item); }, currentWork: (id) => runner.currentWork(id),
    effectiveFolder: (id) => runner.effectiveFolder(id), stateOf: (bot) => runner.toBot(bot).state, stopItem: (id, notice) => runner.stopItem(id, notice) });
  runner.start();
  const task = tasks.create({ projectId: h.project.id, title: "Use helper feedback", instructions: "Ask the other developer, then finish in this task worktree.", acceptance: ["Use the helper reply"], assigneeId: developer.id, reviewerId: h.bots[2].id, testerId: h.bots[3].id });
  const attempt = tasks.start(task.id, task.revision, "synthetic-start");
  const own = () => sessions.filter((entry) => entry.botId === developer.id);
  const asking = async () => {
    await vi.waitFor(() => expect(own()[0]?.session.sent).toHaveLength(1));
    const tool = team.tools().find((entry) => entry.name === "message_bot")!;
    tool.run(developer.id, tool.input.parse({ bot: helper.name, message: "Give me synthetic feedback." }));
    await vi.waitFor(() => expect(sessions.find((entry) => entry.botId === helper.id)?.session.sent).toHaveLength(1));
    return sessions.find((entry) => entry.botId === helper.id)!.session;
  };
  const reply = (): WorkItem => getWorkItem(h.db, (h.db.prepare("SELECT id FROM work_items WHERE bot_id=? AND source='bot_reply' ORDER BY rowid DESC LIMIT 1").get(developer.id) as { id: string }).id);
  const result = { ...h, project, developer, helper, tasks, runner, team, sessions, task, attempt, own, asking, reply };
  open.push(result); return result;
}

it("retains the attempt/lease after an asking turn and completes only after the real helper continuation", async () => {
  const h = fixture(), ownerBefore = h.owner(), helper = await h.asking();
  h.own()[0].session.finish("The helper will answer later.");
  await h.runner.retired(h.developer.id);
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("running"));
  expect(h.tasks.botBusy(h.developer.id)).toBe(true);
  helper.finish("Synthetic feedback: change the task file.");
  await vi.waitFor(() => expect(h.own()[1]?.session.sent).toHaveLength(1));
  expect(h.own()[1].session.sent[0]).toContain("Synthetic feedback: change the task file.");
  expect(h.own()[1].options.cwd).toBe(h.attempt.worktreePath);
  expect(h.runner.effectiveFolder(h.developer.id)).toBe(h.attempt.worktreePath);
  expect(h.tasks.attempt(h.attempt.id)).toMatchObject({ workItemId: h.reply().id, state: "running", revision: h.attempt.revision });
  writeFileSync(join(h.attempt.worktreePath, "src/a.txt"), "Used synthetic feedback\n");
  h.git(h.attempt.worktreePath, "add", "--", "src/a.txt"); h.git(h.attempt.worktreePath, "commit", "-m", "Synthetic continuation result");
  h.own()[1].session.finish("Committed the task after reading the helper reply.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
  expect(h.tasks.task(h.task.id).state).toBe("review"); expect(h.tasks.botBusy(h.developer.id)).toBe(false);
  expect(h.tasks.attempt(h.attempt.id).headSha).toBe(h.git(h.attempt.worktreePath, "rev-parse", "HEAD"));
  expect(h.owner()).toEqual(ownerBefore);
});

it("settles the active reply item when its answer arrives before the original process retires", async () => {
  const h = fixture(), helper = await h.asking(), first = h.own()[0].session;
  first.holdExit(); first.finish("Waiting for helper."); helper.finish("Immediate synthetic feedback.");
  expect(h.reply().status).toBe("queued"); expect(h.tasks.botBusy(h.developer.id)).toBe(true);
  first.releaseExit(); await vi.waitFor(() => expect(h.own()[1]?.session.sent).toHaveLength(1));
  expect(h.own()[1].session.sent[0]).toContain("Immediate synthetic feedback.");
  expect(h.tasks.attempt(h.attempt.id)).toMatchObject({ workItemId: h.reply().id, state: "running" });
  h.own()[1].session.finish("Used the reply.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
  expect(h.tasks.task(h.task.id).state).toBe("review"); expect(h.tasks.botBusy(h.developer.id)).toBe(false);
});

it.each(["retained", "cascaded"] as const)("continues in the task worktree after deleting an external helper with its exchange %s", async (exchangeState) => {
  const h = fixture({ helperOutsideProject: true }), ownerBefore = h.owner();
  await h.asking();
  expect(h.projects.botInUse(h.helper.id)).toBe(false); expect(h.tasks.botBusy(h.helper.id)).toBe(false);
  expect(h.sessions.find((entry) => entry.botId === h.helper.id)?.options.cwd).toBe(h.helper.homeFolder);
  const request = h.runner.currentWork(h.helper.id)!;
  const exchangeId = request.exchangeId!;
  h.own()[0].session.finish("The external helper will answer later.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).reason).toBe("Waiting for requested bot replies."));
  expect(h.tasks.botBusy(h.developer.id)).toBe(true);

  await h.runner.closeForDeletion(h.helper.id);
  const handle = h.team.botDeleting(h.helper.id);
  if (exchangeState === "cascaded") h.db.exec("CREATE TRIGGER cascade_exchanges AFTER DELETE ON bots BEGIN DELETE FROM exchanges WHERE from_bot_id = OLD.id OR to_bot_id = OLD.id; END");
  deleteBot(h.db, h.helper.id);
  expect(() => getStoredBot(h.db, h.helper.id)).toThrow();
  if (exchangeState === "cascaded") expect(() => getExchange(h.db, exchangeId)).toThrow("No such exchange");
  h.team.botDeleted(handle); h.runner.forget(h.helper.id);

  await vi.waitFor(() => expect(h.own()[1]?.session.sent).toHaveLength(1));
  const reply = h.reply();
  expect(h.own()[1].session.sent[0]).toContain(`${h.helper.name} was deleted`);
  expect(h.own()[1].options.cwd).toBe(h.attempt.worktreePath);
  expect(h.runner.effectiveFolder(h.developer.id)).toBe(h.attempt.worktreePath);
  expect(reply).toMatchObject({ source: "bot_reply", status: "running", exchangeId, taskAttemptId: h.attempt.id, taskRevision: h.attempt.revision, executionContextKey: h.attempt.contextKey });
  expect(h.tasks.attempt(h.attempt.id)).toMatchObject({ state: "running", workItemId: reply.id, revision: h.attempt.revision, contextKey: h.attempt.contextKey });
  expect(h.tasks.botBusy(h.developer.id)).toBe(true);
  expect(getExchange(h.db, exchangeId)).toMatchObject({ status: "failed", reply: `${h.helper.name} was deleted` });
  expect(h.db.prepare("SELECT task_attempt_id,task_revision,execution_context_key FROM exchanges WHERE id=?").get(exchangeId))
    .toEqual({ task_attempt_id: h.attempt.id, task_revision: h.attempt.revision, execution_context_key: h.attempt.contextKey });

  h.own()[1].session.finish("Handled the deleted helper and finished the task.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
  expect(h.tasks.attempt(h.attempt.id).workItemId).toBe(reply.id); expect(h.reply().status).toBe("done");
  expect(h.tasks.task(h.task.id).state).toBe("review"); expect(h.tasks.botBusy(h.developer.id)).toBe(false);
  expect(h.tasks.attempt(h.attempt.id).headSha).toBe(h.git(h.attempt.worktreePath, "rev-parse", "HEAD"));
  expect(h.owner()).toEqual(ownerBefore);
});

it("does not create another turn when a helper reply was already steered into the task turn", async () => {
  const h = fixture(), helper = await h.asking(), first = h.own()[0].session;
  first.canSteer = true; helper.finish("Synthetic feedback during the original turn.");
  await vi.waitFor(() => expect(first.steered).toHaveLength(1));
  expect(h.reply().status).toBe("done"); expect(h.tasks.attempt(h.attempt.id).workItemId).toBe(h.attempt.workItemId);
  first.finish("Used the steered helper reply.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
  expect(h.own()).toHaveLength(1); expect(h.tasks.botBusy(h.developer.id)).toBe(false);
});

it.each(["declined", "rejected", "accepted"] as const)("retains the task while a delayed steer is %s after the original process retires", async (outcome) => {
  const h = fixture(), helper = await h.asking(), first = h.own()[0].session;
  let answer!: (value: boolean) => void, reject!: (error: Error) => void;
  first.steerResult = new Promise<boolean>((resolve, fail) => { answer = resolve; reject = fail; });
  first.releaseSteer = () => answer(false);
  const logged = vi.spyOn(console, "error").mockImplementation(() => {});
  first.holdExit(); helper.finish("Feedback whose steering acknowledgment is delayed.");
  await vi.waitFor(() => expect(first.steered).toHaveLength(1));
  first.finish("The original turn ended before steering resolved."); first.releaseExit();
  await new Promise<void>((resolve) => setImmediate(resolve));
  expect(h.tasks.attempt(h.attempt.id).state).toBe("running"); expect(h.tasks.botBusy(h.developer.id)).toBe(true);
  if (outcome === "rejected") reject(new Error("Synthetic delayed steering rejection")); else answer(outcome === "accepted");
  if (outcome === "accepted") {
    await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
    expect(h.reply().status).toBe("done"); expect(h.own()).toHaveLength(1); expect(h.tasks.botBusy(h.developer.id)).toBe(false);
    expect(logged).not.toHaveBeenCalled(); return;
  }
  await vi.waitFor(() => expect(h.own()[1]?.session.sent).toHaveLength(1));
  expect(h.own()[1].options.cwd).toBe(h.attempt.worktreePath);
  expect(h.own()[1].session.sent[0]).toContain("Feedback whose steering acknowledgment is delayed.");
  expect(h.tasks.attempt(h.attempt.id)).toMatchObject({ state: "running", workItemId: h.reply().id });
  h.own()[1].session.finish("Completed after the delayed helper reply.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
  expect(h.tasks.botBusy(h.developer.id)).toBe(false);
  if (outcome === "rejected") expect(logged).toHaveBeenCalledWith("Tjommie: steering a work item failed", expect.any(Error));
  else expect(logged).not.toHaveBeenCalled();
});

it.each(["cancel", "resume", "edit", "archive"] as const)("fences a late task helper reply after explicit %s without using the home folder", async (operation) => {
  const h = fixture(), helper = await h.asking();
  h.own()[0].session.finish("The helper reply will arrive later.");
  await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).reason).toBe("Waiting for requested bot replies."));
  const stopped = await h.tasks.stop(h.task.id, h.tasks.task(h.task.id).revision, operation === "cancel" || operation === "archive");
  if (operation === "resume") {
    const resumed = h.tasks.resume(h.task.id, stopped.revision, "explicit-resume");
    expect(resumed.revision).toBe(h.attempt.revision + 1);
    await vi.waitFor(() => expect(h.own()[1]?.session.sent).toHaveLength(1));
  } else if (operation === "edit") h.tasks.update(h.task.id, stopped.revision, { title: "Revised task" });
  else if (operation === "archive") h.projects.archive(h.project.id, h.project.revision);
  helper.finish("This answer belongs to the old immutable task context.");
  if (operation === "resume") {
    expect(h.reply().status).toBe("queued");
    h.own()[1].session.finish("Completed the explicitly resumed task.");
    await vi.waitFor(() => expect(h.tasks.attempt(h.attempt.id).state).toBe("completed"));
  }
  await vi.waitFor(() => expect(h.reply().status).toBe("failed"));
  expect(h.own()).toHaveLength(operation === "resume" ? 2 : 1);
  expect(h.own().every((entry) => entry.options.cwd === h.attempt.worktreePath)).toBe(true);
  expect(h.reply()).toMatchObject({ taskAttemptId: h.attempt.id, taskRevision: h.attempt.revision, executionContextKey: h.attempt.contextKey });
  expect(h.tasks.attempt(h.attempt.id).state).toBe(operation === "resume" ? "completed" : operation === "edit" ? "interrupted" : "cancelled");
  expect(h.tasks.task(h.task.id).state).toBe(operation === "resume" ? "review" : operation === "edit" ? "todo" : "cancelled");
});
