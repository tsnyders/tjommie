import { rmSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, setPaused } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, setStatus, type NewWork } from "./queue.ts";
import { addRun, createRoutine, deleteRoutine, getRoutine, getRun, hasOpenRun, listRuns } from "./routines.ts";
import { GRACE_MS, Scheduler, TICK_MS, duration, routinePrompt } from "./scheduler.ts";
import { tempRoot } from "./testing.ts";

const at = (iso: string) => Date.parse(iso);
let root: string;
let db: Db;
let botId: string;
let delivered: NewWork[];
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

const scheduler = (deliver: (work: NewWork) => void = (work) => { delivered.push(work); }) => {
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
  it("queues a due run for the bot, with a chain of its own", () => {
    const routine = digest();
    scheduler().tick(at("2026-09-20T08:00:10Z"));
    const [run] = listRuns(db, routine.id);
    expect(run).toMatchObject({ status: "queued", dueAt: "2026-09-20T08:00:00.000Z" });
    expect(delivered).toEqual([{ botId, source: "routine", text: expect.stringContaining("Summarize my inbox"), chainId: expect.any(String), routineRunId: run.id }]);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
    expect(db.prepare("SELECT id, requests_made FROM chains").all()).toEqual([{ id: delivered[0].chainId, requests_made: 0 }]);
    expect(history(db, botId, undefined, 10)).toEqual([]);
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
    s.tick(at("2026-09-25T09:00:30Z"));
    expect(listRuns(db, routine.id)).toHaveLength(1);
    expect(history(db, botId, undefined, 10).map((m) => [m.kind, m.body, m.data?.status])).toEqual([["routine", "Routine: Digest", "missed"]]);
    expect(listRuns(db, routine.id)[0].note).toBe("The PC was off or asleep, or Tjommie wasn't running.");
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
    const item = enqueue(db, delivered[0]);
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
    const item = enqueue(db, delivered[0]);
    s.runStarted(item);
    s.runEnded(item, { outcome: "failed", error: "Codex exited with code 1" });
    expect(getRun(db, item.routineRunId!)).toMatchObject({ status: "failed", note: "Codex exited with code 1" });
    expect(events.find((e) => e.event === "notify")?.data).toEqual({ botId, title: "Night Shift's routine failed", body: "Digest: Codex exited with code 1" });
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
    const item = enqueue(db, delivered[0]);
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
    const deliver = vi.fn<(work: NewWork) => void>().mockImplementationOnce(() => { throw error; }).mockImplementation((work) => { delivered.push(work); });
    const s = scheduler(deliver);
    expect(() => s.tick(at("2026-09-20T08:00:10Z"))).not.toThrow();
    const [failed] = listRuns(db, routine.id);
    expect(failed).toMatchObject({ status: "failed", dueAt: "2026-09-20T08:00:00.000Z", startedAt: null, endedAt: expect.any(String), note: "Could not queue the routine run." });
    expect(hasOpenRun(db, routine.id)).toBe(false);
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-20T17:00:00.000Z");
    expect(log.mock.calls.flat()).toContain(error);
    expect(events).toContainEqual({ event: "routine.run", data: failed });
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
    const item = enqueue(db, delivered[0]);
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
    const item = enqueue(db, delivered[0]);
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

  it("ignores runner hooks after a queued routine has been deleted", () => {
    vi.useFakeTimers();
    const routine = digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    s.tick(at("2026-09-20T08:00:00Z"));
    const item = enqueue(db, delivered[0]);
    deleteRoutine(db, routine.id);
    events.length = 0;
    expect(() => s.runStarted(item)).not.toThrow();
    expect(() => s.runRequeued(item)).not.toThrow();
    expect(() => s.runEnded(item, { outcome: "failed", error: "Deleted" })).not.toThrow();
    expect(getRun(db, item.routineRunId!)).toBeNull();
    expect(events).toEqual([]);
    expect(history(db, botId, undefined, 10)).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not stop or notify a running item when its routine was deleted before the limit", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-20T08:00:00Z"));
    const routine = digest({ timeLimitMinutes: 1 });
    const s = scheduler();
    s.tick();
    const item = enqueue(db, delivered[0]);
    setStatus(db, item.id, "running");
    s.runStarted(item);
    deleteRoutine(db, routine.id);
    events.length = 0;
    expect(() => vi.advanceTimersByTime(60_000)).not.toThrow();
    expect(stopped).toEqual([]);
    expect(() => s.runRequeued(item)).not.toThrow();
    expect(() => s.runStarted(item)).not.toThrow();
    expect(() => s.runEnded(item, { outcome: "failed", error: "Deleted" })).not.toThrow();
    expect(events).toEqual([]);
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
    const item = enqueue(db, delivered[0]);
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
    const item = enqueue(db, delivered[0]);
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
