import { rmSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, setPaused } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, nextQueued, recoverAfterRestart, setStatus, type WorkItem } from "./queue.ts";
import { addRun, createRoutine, deleteRoutine, getRoutine, getRun, hasOpenRun, listRuns, updateRoutine } from "./routines.ts";
import { Runner } from "./runner.ts";
import { GRACE_MS, Scheduler, TICK_MS, duration, routinePrompt } from "./scheduler.ts";
import { RpcError } from "./server.ts";
import { tempRoot } from "./testing.ts";

const at = (iso: string) => Date.parse(iso);
let root: string;
let db: Db;
let botId: string;
let delivered: WorkItem[];
let stopped: [string, string][];
let events: { event: string; data: unknown }[];
let schedulers: Scheduler[];

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "t.db"));
  botId = createBot(db, join(root, "bots"), { name: "Night Shift", color: "purple", shape: "cloud", instructions: "", provider: "codex", model: "gpt-5.6-sol" }).id;
  delivered = [];
  stopped = [];
  events = [];
  schedulers = [];
});

afterEach(() => {
  for (const s of schedulers) s.stop();
  vi.useRealTimers();
  vi.restoreAllMocks();
  db.close();
  rmSync(root, { recursive: true, force: true });
});

const scheduler = (deliver: (work: WorkItem) => void = (work) => { delivered.push(work); }) => {
  const s = new Scheduler({
    db,
    broadcast: (event, data) => { events.push({ event, data }); },
    deliver,
    stopItem: (itemId, notice) => { stopped.push([itemId, notice]); },
  });
  schedulers.push(s);
  return s;
};

// Every day at 08:00 and 17:00 UTC; the first run is due at 2026-09-20 08:00.
const digest = (extra: { timeLimitMinutes?: number; enabled?: boolean; days?: number[]; times?: string[] } = {}) =>
  createRoutine(db, { botId, name: "Digest", prompt: "Summarize my inbox", days: [0, 1, 2, 3, 4, 5, 6], times: ["08:00", "17:00"], timeZone: "UTC", ...extra }, at("2026-09-20T00:00:00Z"));

describe("Scheduler", () => {
  it.each(["routine_runs", "routines", "messages", "chains", "work_items"])("rolls back the entire firing when persisting %s fails", (table) => {
    const routine = digest();
    const s = scheduler();
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    db.exec(`CREATE TRIGGER fail_schedule BEFORE ${table === "routines" ? "UPDATE" : "INSERT"} ON ${table}
      BEGIN SELECT RAISE(ABORT, 'simulated persistence failure'); END`);
    expect(() => s.tick(at("2026-09-20T08:00:10Z"))).not.toThrow();
    expect(log).toHaveBeenCalledWith(`Routine scheduler failed for ${routine.id}.`, expect.objectContaining({ message: "simulated persistence failure" }));
    expect(getRoutine(db, routine.id)).toEqual(routine);
    for (const name of ["routine_runs", "messages", "chains", "work_items"]) {
      expect(db.prepare(`SELECT * FROM ${name}`).all()).toEqual([]);
    }
    expect(events).toEqual([]);
    expect(delivered).toEqual([]);
    db.exec("DROP TRIGGER fail_schedule");
    s.tick(at("2026-09-20T08:00:30Z"));
    expect(listRuns(db, routine.id)).toHaveLength(1);
    expect(delivered).toHaveLength(1);
  });

  it("keeps firing later due routines while one routine repeatedly fails to persist", () => {
    const failing = digest();
    const later = digest({ times: ["08:01", "17:01"] });
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const s = scheduler();
    db.exec(`CREATE TRIGGER fail_one_routine BEFORE INSERT ON work_items
      WHEN (SELECT routine_id FROM routine_runs WHERE id = NEW.routine_run_id) = '${failing.id}'
      BEGIN SELECT RAISE(ABORT, 'one routine unavailable'); END`);
    s.tick(at("2026-09-20T08:01:10Z"));
    expect(getRoutine(db, failing.id)).toEqual(failing);
    expect(listRuns(db, failing.id)).toEqual([]);
    expect(listRuns(db, later.id)).toMatchObject([{ status: "queued" }]);
    expect(delivered).toHaveLength(1);
    expect(delivered[0].routineRunId).toBe(listRuns(db, later.id)[0].id);
    expect(db.prepare("SELECT * FROM chains").all()).toHaveLength(1);
    expect(history(db, botId, undefined, 10)).toHaveLength(1);
    s.tick(at("2026-09-20T08:01:30Z"));
    expect(log).toHaveBeenCalledTimes(2);
    expect(getRoutine(db, failing.id)).toEqual(failing);
    expect(listRuns(db, failing.id)).toEqual([]);
    expect(delivered).toHaveLength(1);
    db.exec("DROP TRIGGER fail_one_routine");
    s.tick(at("2026-09-20T08:01:45Z"));
    expect(listRuns(db, failing.id)).toMatchObject([{ status: "queued" }]);
    expect(delivered).toHaveLength(2);
  });

  it("commits the schedule, label and work before broadcasting or waking the runner", () => {
    const routine = digest();
    const reader = openDb(join(root, "t.db"));
    const observe = () => {
      expect(getRoutine(reader, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
      const [run] = listRuns(reader, routine.id);
      const item = nextQueued(reader, botId)!;
      expect(item.routineRunId).toBe(run.id);
      expect(history(reader, botId, undefined, 10)).toMatchObject([{ id: item.messageId, data: { runId: run.id } }]);
      expect(reader.prepare("SELECT id FROM chains").all()).toEqual([{ id: item.chainId }]);
    };
    try {
      const s = new Scheduler({ db, broadcast: observe, deliver: (item) => { observe(); expect(getWorkItem(reader, item.id)).toEqual(item); }, stopItem: () => {} });
      schedulers.push(s);
      s.tick(at("2026-09-20T08:00:10Z"));
      expect(db.prepare("SELECT * FROM work_items").all()).toHaveLength(1);
    } finally {
      reader.close();
    }
  });

  it("retains committed work across a restart before the runner is woken, without firing twice", () => {
    const routine = digest();
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    // Simulate termination after COMMIT, at the first external effect and before deliver.
    const s = new Scheduler({ db, broadcast: () => { throw new Error("process stopped"); }, deliver: () => { throw new Error("must not wake"); }, stopItem: () => {} });
    schedulers.push(s);
    expect(() => s.tick(at("2026-09-20T08:00:10Z"))).not.toThrow();
    expect(log).toHaveBeenCalledWith(`Routine scheduler failed for ${routine.id}.`, expect.objectContaining({ message: "process stopped" }));
    const item = nextQueued(db, botId)!;
    db.close();
    db = openDb(join(root, "t.db"));
    expect(recoverAfterRestart(db)).toEqual([]);
    const restarted = scheduler();
    restarted.recover();
    restarted.tick(at("2026-09-20T08:00:30Z"));
    expect(nextQueued(db, botId)).toEqual(item);
    expect(listRuns(db, routine.id)).toMatchObject([{ id: item.routineRunId, status: "queued" }]);
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it.each(["queued", "running"] as const)("settles a pre-existing orphan %s run on startup and releases future scheduling", (status) => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T07:59:00Z"));
    const routine = digest();
    const orphan = addRun(db, routine.id, "2026-09-19T17:00:00.000Z", status);
    const s = scheduler();
    s.start();
    expect(getRun(db, orphan.id)).toMatchObject({ status: "failed", endedAt: expect.any(String), note: expect.stringContaining("restarted") });
    expect(hasOpenRun(db, routine.id)).toBe(false);
    expect(history(db, botId, undefined, 10)).toMatchObject([{ data: { runId: orphan.id, status: "failed" } }]);
    expect(events.filter(({ event }) => event === "notify")).toHaveLength(1);
    events.length = 0;
    s.recover();
    expect(events).toEqual([]);
    s.tick(at("2026-09-20T08:00:10Z"));
    expect(delivered).toHaveLength(1);
    expect(listRuns(db, routine.id).map((run) => run.status)).toEqual(["queued", "failed"]);
  });

  it.each(["done", "failed", "interrupted"] as const)("settles an open run left behind after its work became %s", (status) => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    const item = delivered[0];
    setStatus(db, item.id, status);
    s.recover();
    expect(getRun(db, item.routineRunId!)?.status).toBe("failed");
    expect(history(db, botId, undefined, 10)).toHaveLength(1);
    expect(history(db, botId, undefined, 10)[0].data?.status).toBe("failed");
    expect(hasOpenRun(db, routine.id)).toBe(false);
  });

  it.each(["queued", "running"] as const)("preserves open routine runs with %s work during reconciliation", (status) => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    const item = delivered[0];
    s.runStarted(item); // running + queued is also valid during a usage-limit retry.
    setStatus(db, item.id, status);
    const before = listRuns(db, routine.id);
    events.length = 0;
    s.recover();
    expect(listRuns(db, routine.id)).toEqual(before);
    expect(getWorkItem(db, item.id).status).toBe(status);
    expect(events).toEqual([]);
  });

  it("hands persisted work to the real runner once and integrates interrupted-run recovery", () => {
    const routine = digest();
    const createSession = vi.fn(() => { throw new Error("must not start a CLI"); });
    const s = scheduler((item) => { runner.deliver(item); delivered.push(item); });
    // A signed-out synthetic bot holds the queue without touching accounts or launching a CLI.
    const runner = new Runner({ db, broadcast: () => {}, isSignedIn: () => false, createSession,
      hooks: { turnEnded: (item, result) => s.runEnded(item, result) } });
    try {
      s.tick(at("2026-09-20T08:00:10Z"));
      expect(db.prepare("SELECT * FROM work_items").all()).toHaveLength(1);
      const item = delivered[0];
      s.runStarted(item);
      setStatus(db, item.id, "running");
      runner.recover();
      s.recover();
      expect(getWorkItem(db, item.id).status).toBe("interrupted");
      expect(listRuns(db, routine.id)).toMatchObject([{ status: "failed", note: "Tjommie restarted while this was running." }]);
      expect(history(db, botId, undefined, 10).filter((message) => message.kind === "routine")).toMatchObject([{ data: { status: "failed" } }]);
      expect(createSession).not.toHaveBeenCalled();
    } finally {
      runner.shutdown();
    }
  });

  it("rolls back a manual retry when queue persistence fails", () => {
    const routine = digest();
    setPaused(db, botId, true);
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    setPaused(db, botId, false);
    const [label] = history(db, botId, undefined, 10);
    const before = listRuns(db, routine.id);
    events.length = 0;
    db.exec("CREATE TRIGGER fail_retry BEFORE INSERT ON work_items BEGIN SELECT RAISE(ABORT, 'queue unavailable'); END");
    expect(() => s.retry(label)).toThrow("queue unavailable");
    expect(listRuns(db, routine.id)).toEqual(before);
    expect(history(db, botId, undefined, 10)).toEqual([label]);
    expect(db.prepare("SELECT * FROM chains").all()).toEqual([]);
    expect(db.prepare("SELECT * FROM work_items").all()).toEqual([]);
    expect(events).toEqual([]);
    expect(delivered).toEqual([]);
    db.exec("DROP TRIGGER fail_retry");
    const retried = s.retry(label);
    expect(delivered).toHaveLength(1);
    expect(getWorkItem(db, delivered[0].id).messageId).toBe(retried.id);
  });

  it.each([
    { reason: "missed", status: "missed", note: "The PC was off or asleep, or Tjommie wasn't running." },
    { reason: "paused", status: "skipped", note: "Night Shift was paused." },
    { reason: "running", status: "skipped", note: "The previous run was still going." },
    { reason: "queued", status: "skipped", note: "The previous run was still going." },
  ] as const)("publishes one final label when a due run is $reason", ({ reason, status, note }) => {
    const routine = digest();
    if (reason === "paused") setPaused(db, botId, true);
    if (reason === "running" || reason === "queued") addRun(db, routine.id, "2026-09-19T17:00:00.000Z", reason);
    scheduler().tick(at(reason === "missed" ? "2026-09-20T08:03:00Z" : "2026-09-20T08:00:10Z"));
    const labels = history(db, botId, undefined, 10);
    expect(labels).toMatchObject([{ kind: "routine", data: { status, note } }]);
    expect(events.filter(({ event }) => event === "message.added" || event === "message.updated")).toEqual([
      { event: "message.added", data: labels[0] },
    ]);
    expect(delivered).toEqual([]);
  });

  it.each([
    { reason: "paused bot", message: "Unpause this bot before retrying its routine" },
    { reason: "disabled routine", message: "Enable this routine before retrying it" },
  ] as const)("refuses a routine retry for a $reason before creating work", ({ reason, message }) => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    setStatus(db, item.id, "failed");
    s.runEnded(item, { outcome: "failed", error: "CLI exited" });
    if (reason === "paused bot") setPaused(db, botId, true);
    else updateRoutine(db, routine.id, { enabled: false });
    const state = () => ({
      runs: listRuns(db, routine.id),
      messages: history(db, botId, undefined, 10),
      chains: db.prepare("SELECT * FROM chains ORDER BY id").all(),
      work: db.prepare("SELECT * FROM work_items ORDER BY id").all(),
      routine: getRoutine(db, routine.id),
    });
    const before = state();
    delivered.length = 0;
    events.length = 0;
    let error: unknown;
    try {
      s.retry(before.messages[0]);
    } catch (caught) {
      error = caught;
    }
    expect(error).toBeInstanceOf(RpcError);
    expect(error).toMatchObject({ code: "bad_state", message });
    expect(state()).toEqual(before);
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it.each(["done", "failed", "interrupted"] as const)("keeps one routine label current through %s", (outcome) => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    const [label] = history(db, botId, undefined, 10);
    const [run] = listRuns(db, routine.id);
    expect(label).toMatchObject({ kind: "routine", data: { runId: run.id, status: "queued" } });
    expect(delivered[0].messageId).toBe(label.id);
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    s.runEnded(item, { outcome, error: outcome === "failed" ? "CLI exited" : null });
    const status = outcome === "interrupted" ? "stopped" : outcome;
    const note = outcome === "interrupted" ? "Stopped by you." : outcome === "failed" ? "CLI exited" : null;
    const messages = history(db, botId, undefined, 10);
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({ id: label.id, data: { status, note } });
    expect(events).toContainEqual({ event: "message.updated", data: messages[0] });
  });
  it("queues a due run for the bot, with a chain of its own", () => {
    const routine = digest();
    scheduler().tick(at("2026-09-20T08:00:10Z"));
    const [run] = listRuns(db, routine.id);
    expect(run).toMatchObject({ status: "queued", dueAt: "2026-09-20T08:00:00.000Z" });
    expect(delivered).toMatchObject([{ botId, source: "routine", text: expect.stringContaining("Summarize my inbox"), messageId: expect.any(String), chainId: expect.any(String), routineRunId: run.id, status: "queued" }]);
    expect(getWorkItem(db, delivered[0].id)).toEqual(delivered[0]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
    expect(db.prepare("SELECT id, requests_made FROM chains").all()).toEqual([{ id: delivered[0].chainId, requests_made: 0 }]);
    expect(history(db, botId, undefined, 10)).toMatchObject([{ kind: "routine", data: { runId: run.id, status: "queued" } }]);
    expect(events).toContainEqual({ event: "routine.run", data: run });
    expect(events).toContainEqual({ event: "routine.updated", data: getRoutine(db, routine.id) });
  });

  it("records one Missed run when the PC was off through several run times, and starts nothing (late tick)", () => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-25T09:00:00Z")); // five days and ten run times later
    expect(listRuns(db, routine.id).map((r) => [r.status, r.dueAt])).toEqual([["missed", "2026-09-20T08:00:00.000Z"]]);
    expect(delivered).toEqual([]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-25T17:00:00.000Z");
    for (const tickAt of ["2026-09-25T09:00:30Z", "2026-09-25T09:01:00Z", "2026-09-25T09:01:30Z"]) {
      s.tick(at(tickAt));
      expect(listRuns(db, routine.id).map((r) => [r.status, r.dueAt])).toEqual([["missed", "2026-09-20T08:00:00.000Z"]]);
      expect(delivered).toEqual([]);
      expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-25T17:00:00.000Z");
    }
    expect(history(db, botId, undefined, 10).map((m) => [m.kind, m.body, m.data?.status])).toEqual([["routine", "Routine: Digest", "missed"]]);
    expect(listRuns(db, routine.id)[0].note).toBe("The PC was off or asleep, or Tjommie wasn't running.");
  });

  it("queues the within-grace 17:00 run on the next tick after recording the missed 08:00 run", () => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T07:59:00Z"));
    expect(listRuns(db, routine.id)).toEqual([]);
    expect(delivered).toEqual([]);

    // The PC sleeps through 08:00 and wakes at 17:01, within the 17:00 run's grace period.
    s.tick(at("2026-09-20T17:01:00Z"));
    expect(listRuns(db, routine.id).map((r) => [r.status, r.dueAt])).toEqual([["missed", "2026-09-20T08:00:00.000Z"]]);
    expect(delivered).toEqual([]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");

    s.tick(at("2026-09-20T17:01:30Z"));
    const runs = listRuns(db, routine.id);
    expect(runs.map((r) => [r.status, r.dueAt])).toEqual([
      ["queued", "2026-09-20T17:00:00.000Z"],
      ["missed", "2026-09-20T08:00:00.000Z"],
    ]);
    expect(delivered).toHaveLength(1);
    expect(delivered[0]).toMatchObject({ botId, source: "routine", routineRunId: runs[0].id });
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-21T08:00:00.000Z");

    s.tick(at("2026-09-20T17:02:00Z"));
    expect(listRuns(db, routine.id)).toEqual(runs);
    expect(delivered).toHaveLength(1);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-21T08:00:00.000Z");
  });

  it("skips a run while the bot is paused, or while the previous run is still going", () => {
    const routine = digest();
    setPaused(db, botId, true);
    scheduler().tick(at("2026-09-20T08:00:10Z"));
    setPaused(db, botId, false);
    addRun(db, routine.id, "2026-09-20T12:00:00.000Z", "running");
    scheduler().tick(at("2026-09-20T17:00:10Z"));
    expect(listRuns(db, routine.id).map((r) => [r.status, r.note])).toEqual([
      ["skipped", "The previous run was still going."],
      ["running", null],
      ["skipped", "Night Shift was paused."],
    ]);
    expect(delivered).toEqual([]);
    expect(history(db, botId, undefined, 10).map((m) => [m.body, m.data?.status, m.data?.note])).toEqual([
      ["Routine: Digest", "skipped", "Night Shift was paused."],
      ["Routine: Digest", "skipped", "The previous run was still going."],
    ]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-21T08:00:00.000Z");
  });

  it("labels a run when it starts, and stops it at the time limit", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:10Z"));
    digest({ timeLimitMinutes: 180 });
    const s = scheduler();
    s.tick();
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    const runId = item.routineRunId!;
    expect(getRun(db, runId)?.status).toBe("running");
    expect(history(db, botId, undefined, 10).at(-1)).toMatchObject({ kind: "routine", body: "Routine: Digest", data: { runId, status: "running" } });
    vi.advanceTimersByTime(180 * 60_000);
    expect(stopped).toEqual([[item.id, "Stopped at the time limit (3 hours)."]]);
    s.runEnded(getWorkItem(db, item.id), { outcome: "interrupted", error: null });
    expect(getRun(db, runId)).toMatchObject({ status: "stopped", note: "Stopped at the time limit." });
  });

  it("notifies the person when a run fails", () => {
    digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:10Z"));
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    s.runEnded(item, { outcome: "failed", error: "Codex exited with code 1" });
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "failed", note: "Codex exited with code 1" });
    expect(events.find((e) => e.event === "notify")?.data).toEqual({ kind: "routine_failed", botId, routineId: getRun(db, item.routineRunId!)!.routineId, runId: item.routineRunId, title: "Night Shift's routine failed", body: "Digest: Codex exited with code 1" });
  });

  it.each(["throw", "reject"] as const)("records the actual outcome when the time limit stop fails by %s", async (failure) => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:10Z"));
    digest({ timeLimitMinutes: 1 });
    const error = new Error("Could not interrupt");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    // Handle the rejected fixture itself so the red test reports the stale outcome only.
    const rejected = Promise.reject(error);
    void rejected.catch(() => {});
    const s = new Scheduler({
      db, broadcast: () => {}, deliver: (work) => { delivered.push(work); },
      stopItem: () => { if (failure === "throw") throw error; return rejected; },
    });
    schedulers.push(s);
    s.tick();
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    await vi.advanceTimersByTimeAsync(60_000);
    s.runEnded(item, { outcome: "done", error: null });
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "done", note: null });
    expect(log).toHaveBeenCalledWith("Routine time limit failed.", error);
  });

  it("waits until the due instant and queues a run exactly two minutes late", () => {
    expect(TICK_MS).toBe(30_000);
    expect(GRACE_MS).toBe(120_000);
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T07:59:59.999Z"));
    expect(listRuns(db, routine.id)).toEqual([]);
    s.tick(at("2026-09-20T08:02:00Z"));
    expect(listRuns(db, routine.id).map((run) => run.status)).toEqual(["queued"]);
    expect(delivered).toHaveLength(1);
  });

  it("marks a run missed one millisecond past the grace period", () => {
    const routine = digest();
    scheduler().tick(at("2026-09-20T08:02:00.001Z"));
    expect(listRuns(db, routine.id).map((run) => run.status)).toEqual(["missed"]);
    expect(delivered).toEqual([]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
  });

  it("skips the next run while the previous run is still queued", () => {
    const routine = digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:00Z"));
    s.tick(at("2026-09-20T17:00:00Z"));
    expect(listRuns(db, routine.id).map((run) => [run.status, run.note])).toEqual([
      ["skipped", "The previous run was still going."],
      ["queued", null],
    ]);
    expect(delivered).toHaveLength(1);
  });

  it("ignores disabled routines and schedules without a next run", () => {
    const disabled = digest({ enabled: false });
    const empty = digest({ times: [] });
    scheduler().tick(at("2026-09-25T09:00:00Z"));
    expect(listRuns(db, disabled.id)).toEqual([]);
    expect(listRuns(db, empty.id)).toEqual([]);
    expect(delivered).toEqual([]);
    expect(events).toEqual([]);
  });

  it("does not duplicate a due run when days and times contain duplicates", () => {
    const routine = digest({ days: [0, 0, 1, 2, 3, 4, 5, 6], times: ["08:00", "08:00", "17:00"] });
    // Stored input is normalized today, but the scheduler must also tolerate older duplicate schedules.
    db.prepare("UPDATE routines SET days = ?, times = ? WHERE id = ?").run(JSON.stringify([0, 0, 1, 2, 3, 4, 5, 6]), JSON.stringify(["08:00", "08:00", "17:00"]), routine.id);
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:00Z"));
    s.tick(at("2026-09-20T08:00:00Z"));
    s.tick(at("2026-09-20T08:00:30Z"));
    expect(listRuns(db, routine.id).map((run) => [run.status, run.dueAt])).toEqual([["queued", "2026-09-20T08:00:00.000Z"]]);
    expect(delivered).toHaveLength(1);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
  });

  it("ticks immediately and every 30 seconds, then stops all timers", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:00Z"));
    digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    const tick = vi.spyOn(s, "tick");
    s.start();
    expect(tick).toHaveBeenCalledTimes(1);
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    vi.advanceTimersByTime(29_999);
    expect(tick).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(tick).toHaveBeenCalledTimes(2);
    s.stop();
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(120_000);
    expect(tick).toHaveBeenCalledTimes(2);
    expect(stopped).toEqual([]);
  });

  it("logs an immediate tick exception and still runs the next interval tick", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:00Z"));
    const routine = digest();
    const s = scheduler();
    const error = new Error("Database temporarily unavailable");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(s, "tick").mockImplementationOnce(() => { throw error; });
    expect(() => s.start()).not.toThrow();
    expect(log.mock.calls.flat()).toContain(error);
    expect(listRuns(db, routine.id)).toEqual([]);
    expect(() => vi.advanceTimersByTime(30_000)).not.toThrow();
    expect(listRuns(db, routine.id).map((run) => run.status)).toEqual(["queued"]);
    expect(delivered).toHaveLength(1);
  });

  it("logs an interval tick exception and still runs the following tick", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T07:59:30Z"));
    const routine = digest();
    const s = scheduler();
    const error = new Error("Database temporarily unavailable");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const tick = vi.spyOn(s, "tick");
    s.start();
    tick.mockImplementationOnce(() => { throw error; });
    expect(() => vi.advanceTimersByTime(30_000)).not.toThrow();
    expect(log.mock.calls.flat()).toContain(error);
    expect(listRuns(db, routine.id)).toEqual([]);
    expect(() => vi.advanceTimersByTime(30_000)).not.toThrow();
    expect(listRuns(db, routine.id).map((run) => run.status)).toEqual(["queued"]);
    expect(delivered).toHaveLength(1);
  });

  it("marks a throwing delivery failed and persists the deterministic next run", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-25T12:00:00Z"));
    const routine = digest();
    const error = new Error("Provider queue failed \u2014 retry \ud83d\udea8");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const deliver = vi.fn<(work: WorkItem) => void>().mockImplementationOnce(() => { throw error; }).mockImplementation((work) => { delivered.push(work); });
    const s = scheduler(deliver);
    expect(() => s.tick(at("2026-09-20T08:00:10Z"))).not.toThrow();
    const [failed] = listRuns(db, routine.id);
    expect(failed).toMatchObject({ status: "failed", dueAt: "2026-09-20T08:00:00.000Z", startedAt: null, endedAt: expect.any(String), note: "Could not queue the routine run." });
    expect(hasOpenRun(db, routine.id)).toBe(false);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
    expect(log.mock.calls.flat()).toContain(error);
    expect(events).toContainEqual({ event: "routine.run", data: failed });
    const [label] = history(db, botId, undefined, 10);
    expect(label).toMatchObject({ data: { runId: failed.id, status: "failed", note: failed.note } });
    expect(events).toContainEqual({ event: "message.updated", data: label });
    s.tick(at("2026-09-20T08:00:30Z"));
    expect(deliver).toHaveBeenCalledTimes(1);
    s.tick(at("2026-09-20T17:00:00Z"));
    expect(deliver).toHaveBeenCalledTimes(2);
    expect(listRuns(db, routine.id).map((run) => run.status)).toEqual(["queued", "failed"]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-21T08:00:00.000Z");
  });

  it("clears the limit while requeued, rearms the full limit on restart, and clears it on completion", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:00Z"));
    const routine = digest({ timeLimitMinutes: 2 });
    const s = scheduler();
    s.tick();
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    const started = getRun(db, item.routineRunId!);
    vi.advanceTimersByTime(60_000);
    s.runRequeued(item);
    expect(vi.getTimerCount()).toBe(0);
    expect(getRun(db, item.routineRunId!)).toEqual(started);
    expect(hasOpenRun(db, routine.id)).toBe(true);
    vi.advanceTimersByTime(300_000);
    expect(stopped).toEqual([]);
    s.runStarted(item);
    expect(vi.getTimerCount()).toBe(1);
    expect(history(db, botId, undefined, 10)).toHaveLength(1);
    vi.advanceTimersByTime(119_999);
    expect(stopped).toEqual([]);
    s.runEnded(item, { outcome: "done", error: null });
    expect(vi.getTimerCount()).toBe(0);
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "done", note: null });
    expect(hasOpenRun(db, routine.id)).toBe(false);
    vi.advanceTimersByTime(120_000);
    expect(stopped).toEqual([]);
  });

  it("stops a resumed run only after its full restarted time limit", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:00Z"));
    digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    s.tick();
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    vi.advanceTimersByTime(59_000);
    s.runRequeued(item);
    vi.advanceTimersByTime(180_000);
    s.runStarted(item);
    vi.advanceTimersByTime(59_999);
    expect(stopped).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(stopped).toEqual([[item.id, "Stopped at the time limit (1 minute)."]]);
    s.runEnded(item, { outcome: "interrupted", error: null });
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "stopped", note: "Stopped at the time limit." });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("settles the label after a queued routine has been deleted", () => {
    vi.useFakeTimers();
    const routine = digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:00Z"));
    const item = getWorkItem(db, delivered[0].id);
    deleteRoutine(db, routine.id);
    events.length = 0;
    expect(() => s.runStarted(item)).not.toThrow();
    expect(() => s.runRequeued(item)).not.toThrow();
    expect(() => s.runEnded(item, { outcome: "failed", error: "Deleted" })).not.toThrow();
    expect(getRun(db, item.routineRunId!)).toBeNull();
    expect(history(db, botId, undefined, 10)).toMatchObject([{ data: { status: "stopped", note: "The routine was deleted." } }]);
    expect(events.map((event) => event.event)).toEqual(["message.updated"]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not stop or notify a running item when its routine was deleted before the limit", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:00Z"));
    const routine = digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    s.tick();
    const item = getWorkItem(db, delivered[0].id);
    setStatus(db, item.id, "running");
    s.runStarted(item);
    deleteRoutine(db, routine.id);
    events.length = 0;
    expect(() => vi.advanceTimersByTime(60_000)).not.toThrow();
    expect(stopped).toEqual([]);
    expect(() => s.runRequeued(item)).not.toThrow();
    expect(() => s.runStarted(item)).not.toThrow();
    expect(() => s.runEnded(item, { outcome: "failed", error: "Deleted" })).not.toThrow();
    expect(events.map((event) => event.event)).toEqual(["message.updated"]);
    expect(history(db, botId, undefined, 10)).toMatchObject([{ data: { status: "stopped", note: "The routine was deleted." } }]);
    expect(getRun(db, item.routineRunId!)).toBeNull();
    expect(getWorkItem(db, item.id).status).toBe("running");
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each([
    { outcome: "done", status: "done", note: null },
    { outcome: "interrupted", status: "stopped", note: "Stopped by you." },
    { outcome: "failed", status: "failed", note: "Something went wrong." },
  ] as const)("records the $outcome outcome and clears its time limit", ({ outcome, status, note }) => {
    vi.useFakeTimers();
    const routine = digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:00Z"));
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    s.runEnded(item, { outcome, error: null });
    const ended = getRun(db, item.routineRunId!);
    expect(ended).toMatchObject({ status, note, endedAt: expect.any(String) });
    expect(events).toContainEqual({ event: "routine.run", data: ended });
    expect(hasOpenRun(db, routine.id)).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(60_000);
    expect(stopped).toEqual([]);
    expect(events.filter((event) => event.event === "notify")).toHaveLength(outcome === "failed" ? 1 : 0);
  });

  it("does not arm a timer for an unlimited routine", () => {
    vi.useFakeTimers();
    digest();
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:00Z"));
    const item = getWorkItem(db, delivered[0].id);
    s.runStarted(item);
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(24 * 60 * 60_000);
    expect(stopped).toEqual([]);
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "running", endedAt: null });
  });

  it("ignores hooks for work that is not from a routine", () => {
    vi.useFakeTimers();
    const s = scheduler();
    const item = enqueue(db, { botId, source: "you", text: "Hello" });
    s.runStarted(item);
    s.runRequeued(item);
    s.runEnded(item, { outcome: "failed", error: "Failed" });
    expect(events).toEqual([]);
    expect(stopped).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("routine prompts", () => {
  it.each([
    [1, "1 minute"],
    [2, "2 minutes"],
    [60, "1 hour"],
    [90, "90 minutes"],
    [180, "3 hours"],
  ] as const)("formats %i minutes as %s", (minutes, expected) => {
    expect(duration(minutes)).toBe(expected);
  });

  it("includes the exact schedule label and time limit in the prompt", () => {
    expect(routinePrompt(digest({ timeLimitMinutes: 180 }))).toBe('[Routine "Digest", started by Tjommie on schedule. You have 3 hours; then Tjommie stops you.]\nSummarize my inbox');
    expect(routinePrompt(digest())).toBe('[Routine "Digest", started by Tjommie on schedule.]\nSummarize my inbox');
  });
});
