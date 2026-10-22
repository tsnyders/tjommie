import { rmSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, getWorkItem, setStatus } from "./queue.ts";
import {
  RUNS_KEPT, addRun, createRoutine, deleteRoutine, getRoutine, getRun, hasOpenRun, hostTimeZone,
  listRoutines, listRuns, nextRunAfter, setNextRun, setRunStatus, updateRoutine, wallTimeToUtc,
} from "./routines.ts";
import { tempRoot } from "./testing.ts";

const at = (iso: string) => Date.parse(iso);
const iso = (ms: number | null) => (ms === null ? null : new Date(ms).toISOString());
const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];

describe("nextRunAfter", () => {
  const weekdays = { days: [1, 2, 3, 4, 5], times: ["08:48", "12:47"], timeZone: "Africa/Johannesburg" };

  it("finds the next time today, or the next matching day (2026-09-25 is a Friday)", () => {
    expect(iso(nextRunAfter(weekdays, at("2026-09-25T07:00:00Z")))).toBe("2026-09-25T10:47:00.000Z"); // 09:00 in Johannesburg
    expect(iso(nextRunAfter(weekdays, at("2026-09-25T11:00:00Z")))).toBe("2026-09-28T06:48:00.000Z"); // Monday 08:48
    expect(iso(nextRunAfter({ ...weekdays, days: [5], times: ["08:00"] }, at("2026-09-25T07:00:00Z")))).toBe("2026-10-02T06:00:00.000Z");
  });

  it("moves a time skipped by the spring clock change forward by the gap", () => {
    const routine = { days: EVERY_DAY, times: ["02:30"], timeZone: "America/New_York" };
    expect(iso(nextRunAfter(routine, at("2026-03-08T05:00:00Z")))).toBe("2026-03-08T07:30:00.000Z"); // 03:30 EDT
  });

  it("runs a time that happens twice in the autumn only once", () => {
    const routine = { days: EVERY_DAY, times: ["01:30"], timeZone: "America/New_York" };
    const first = nextRunAfter(routine, at("2026-11-01T04:00:00Z"));
    expect(iso(first)).toBe("2026-11-01T05:30:00.000Z"); // 01:30 EDT, the first of the two
    expect(iso(nextRunAfter(routine, first!))).toBe("2026-11-02T06:30:00.000Z"); // the next day, not 01:30 EST
  });

  it("uses Sunday as day zero and excludes the exact boundary down to the millisecond", () => {
    const routine = { days: [0], times: ["00:00"], timeZone: "Africa/Johannesburg" };
    const due = at("2026-09-26T22:00:00Z");
    expect(nextRunAfter(routine, due - 1)).toBe(due);
    expect(iso(nextRunAfter(routine, due))).toBe("2026-10-03T22:00:00.000Z");
  });

  it("returns null for an empty schedule", () => {
    expect(nextRunAfter({ ...weekdays, days: [] }, at("2026-09-25T07:00:00Z"))).toBeNull();
    expect(nextRunAfter({ ...weekdays, times: [] }, at("2026-09-25T07:00:00Z"))).toBeNull();
  });

  it("selects the earliest instant when a gap reorders or merges scheduled times", () => {
    const routine = { days: EVERY_DAY, times: ["02:30", "03:15", "03:30"], timeZone: "America/New_York" };
    const first = nextRunAfter(routine, at("2026-03-08T05:00:00Z"));
    expect(iso(first)).toBe("2026-03-08T07:15:00.000Z");
    const shifted = nextRunAfter(routine, first!);
    expect(iso(shifted)).toBe("2026-03-08T07:30:00.000Z");
    expect(iso(nextRunAfter(routine, shifted!))).toBe("2026-03-09T06:30:00.000Z");
  });

  it("skips the second occurrence even when called between the two autumn occurrences", () => {
    const routine = { days: EVERY_DAY, times: ["01:30"], timeZone: "America/New_York" };
    expect(iso(nextRunAfter(routine, at("2026-11-01T06:00:00Z")))).toBe("2026-11-02T06:30:00.000Z");
  });

  it("handles Lord Howe's half-hour spring gap and autumn fold", () => {
    const routine = { days: EVERY_DAY, times: ["02:15"], timeZone: "Australia/Lord_Howe" };
    expect(iso(nextRunAfter(routine, at("2026-10-03T14:00:00Z")))).toBe("2026-10-03T15:45:00.000Z");
    const autumn = { ...routine, times: ["01:45"] };
    const first = nextRunAfter(autumn, at("2026-04-04T13:00:00Z"));
    expect(iso(first)).toBe("2026-04-04T14:45:00.000Z");
    expect(iso(nextRunAfter(autumn, first!))).toBe("2026-04-05T15:15:00.000Z");
  });

  it("finds a previous day's run shifted past midnight by a spring gap", () => {
    const routine = { days: [6], times: ["23:30"], timeZone: "America/Nuuk" };
    expect(iso(nextRunAfter(routine, at("2026-03-29T01:10:00Z")))).toBe("2026-03-29T01:30:00.000Z");
    expect(iso(nextRunAfter(routine, at("2026-03-29T01:30:00Z")))).toBe("2026-04-05T00:30:00.000Z");
  });

  it("orders a previous day's shifted run against today's scheduled times", () => {
    const routine = { days: [0, 6], times: ["00:15", "23:30"], timeZone: "America/Nuuk" };
    expect(iso(nextRunAfter(routine, at("2026-03-29T01:10:00Z")))).toBe("2026-03-29T01:15:00.000Z");
  });

  it("handles a fractional offset and a year boundary without DST", () => {
    const routine = { days: EVERY_DAY, times: ["00:15"], timeZone: "Asia/Kathmandu" };
    expect(iso(nextRunAfter(routine, at("2026-12-31T18:31:00Z")))).toBe("2027-01-01T18:30:00.000Z");
  });
});

describe("wallTimeToUtc", () => {
  it.each([
    [2026, 9, 25, 8, 48, "Africa/Johannesburg", "2026-09-25T06:48:00.000Z"],
    [2026, 9, 25, 8, 15, "Asia/Kathmandu", "2026-09-25T02:30:00.000Z"],
    [2026, 3, 8, 2, 30, "America/New_York", "2026-03-08T07:30:00.000Z"],
    [2026, 11, 1, 1, 30, "America/New_York", "2026-11-01T05:30:00.000Z"],
    [2026, 10, 4, 2, 15, "Australia/Lord_Howe", "2026-10-03T15:45:00.000Z"],
    [2026, 4, 5, 1, 45, "Australia/Lord_Howe", "2026-04-04T14:45:00.000Z"],
    [2026, 9, 6, 0, 30, "America/Santiago", "2026-09-06T04:30:00.000Z"],
    [2026, 3, 28, 23, 30, "America/Nuuk", "2026-03-29T01:30:00.000Z"],
  ] as const)("converts %i-%i-%i %i:%i in %s", (year, month, day, hour, minute, zone, expected) => {
    expect(iso(wallTimeToUtc(year, month, day, hour, minute, zone))).toBe(expected);
  });
});

describe("routines store", () => {
  let db: Db;
  let botId: string;
  beforeEach(() => {
    const root = tempRoot();
    db = openDb(join(root, "t.db"));
    botId = createBot(db, join(root, "bots"), { name: "Pnet apply", color: "green", shape: "blob", instructions: "", provider: "claude", model: "sonnet" }).id;
    return () => {
      vi.useRealTimers();
      db.close();
      rmSync(root, { recursive: true, force: true });
    };
  });
  const input = () => ({ botId, name: "Morning check", prompt: "Check pnet.co.za", days: [5, 1, 1], times: ["12:47", "08:48"], timeZone: "Africa/Johannesburg" });

  it("stores days and times sorted and computes the next run", () => {
    const routine = createRoutine(db, input(), at("2026-09-25T07:00:00Z"));
    expect(routine).toMatchObject({ days: [1, 5], times: ["08:48", "12:47"], enabled: true, timeLimitMinutes: null, nextRunAt: "2026-09-25T10:47:00.000Z" });
  });

  it("clears the next run while disabled, and refuses an unknown time zone", () => {
    const routine = createRoutine(db, input(), at("2026-09-25T07:00:00Z"));
    expect(updateRoutine(db, routine.id, { enabled: false }).nextRunAt).toBeNull();
    expect(updateRoutine(db, routine.id, { enabled: true }, at("2026-09-25T11:00:00Z")).nextRunAt).toBe("2026-09-28T06:48:00.000Z");
    expect(() => createRoutine(db, { ...input(), timeZone: "Mars/Olympus" })).toThrow(/time zone/);
  });

  it.each([
    { name: "Renamed" },
    { prompt: "Updated instructions" },
    { timeLimitMinutes: 15 },
    { days: [5, 1, 5], times: ["12:47", "08:48"], timeZone: "Africa/Johannesburg", enabled: true },
  ])("preserves an already due occurrence when the schedule has not changed: %j", (patch) => {
    const routine = createRoutine(db, input(), at("2026-09-25T07:00:00Z"));
    const updated = updateRoutine(db, routine.id, patch, at("2026-09-25T10:47:15Z"));
    expect(updated.nextRunAt).toBe(routine.nextRunAt);
  });

  it("keeps the newest 20 runs", () => {
    const routine = createRoutine(db, input());
    for (let day = 1; day <= 25; day++) addRun(db, routine.id, `2026-08-${String(day).padStart(2, "0")}T06:48:00.000Z`, "done");
    const runs = listRuns(db, routine.id);
    expect(runs).toHaveLength(20);
    expect(runs[0].dueAt).toBe("2026-08-25T06:48:00.000Z");
  });

  it("deletes a routine with its runs and any run still waiting in the queue", () => {
    const routine = createRoutine(db, input());
    const run = addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "queued");
    enqueue(db, { botId, source: "routine", text: "Check pnet.co.za", routineRunId: run.id });
    deleteRoutine(db, routine.id);
    expect(db.prepare("SELECT COUNT(*) AS n FROM work_items").get()).toEqual({ n: 0 });
    expect(db.prepare("SELECT COUNT(*) AS n FROM routine_runs").get()).toEqual({ n: 0 });
  });

  it("defaults to the host's zone and persists explicit options", () => {
    expect(hostTimeZone()).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone);
    const { timeZone: _timeZone, ...withoutZone } = input();
    const routine = createRoutine(db, { ...withoutZone, enabled: false, timeLimitMinutes: 25 });
    expect(getRoutine(db, routine.id)).toEqual(routine);
    expect(routine).toMatchObject({ timeZone: hostTimeZone(), enabled: false, timeLimitMinutes: 25, nextRunAt: null });
  });

  it("normalizes duplicate times on creation and updates the schedule and fields together", () => {
    const routine = createRoutine(db, { ...input(), times: ["12:47", "08:48", "12:47"] });
    expect(routine.times).toEqual(["08:48", "12:47"]);
    const changed = updateRoutine(db, routine.id, {
      name: "Evening check", prompt: "Check again", days: [6, 0, 6], times: ["19:00", "08:00", "19:00"],
      timeZone: "America/New_York", timeLimitMinutes: 15,
    }, at("2026-09-25T07:00:00Z"));
    expect(changed).toEqual({
      ...routine, name: "Evening check", prompt: "Check again", days: [0, 6], times: ["08:00", "19:00"],
      timeZone: "America/New_York", timeLimitMinutes: 15, nextRunAt: "2026-09-26T12:00:00.000Z",
    });
    expect(updateRoutine(db, routine.id, { timeLimitMinutes: null }).timeLimitMinutes).toBeNull();
  });

  it("rejects unknown zones on update without changing the stored routine", () => {
    const routine = createRoutine(db, input());
    expect(() => updateRoutine(db, routine.id, { name: "Bad update", timeZone: "Mars/Olympus" })).toThrow(/time zone/);
    expect(getRoutine(db, routine.id)).toEqual(routine);
    expect(() => createRoutine(db, { ...input(), timeZone: "Mars/Olympus" })).toThrow(/time zone/);
    expect(listRoutines(db, botId)).toEqual([routine]);
  });

  it("lists only the bot's routines in insertion order when creation times tie", () => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-25T07:00:00Z"));
    const first = createRoutine(db, input());
    const second = createRoutine(db, { ...input(), name: "Second" });
    expect(second.createdAt).toBe(first.createdAt);
    expect(listRoutines(db, botId)).toEqual([first, second]);
    expect(listRoutines(db, "no-such-bot")).toEqual([]);
  });

  it("reports missing bots and routines, and returns null for missing runs", () => {
    expect(() => createRoutine(db, { ...input(), botId: "no-such-bot" })).toThrow("No such bot");
    expect(() => getRoutine(db, "missing")).toThrow("No such routine");
    expect(() => updateRoutine(db, "missing", {})).toThrow("No such routine");
    expect(() => deleteRoutine(db, "missing")).toThrow("No such routine");
    expect(getRun(db, "missing")).toBeNull();
    expect(setRunStatus(db, "missing", "done")).toBeNull();
    expect(hasOpenRun(db, "missing")).toBe(false);
  });

  it("lets the scheduler set and clear the next run", () => {
    const routine = createRoutine(db, input());
    setNextRun(db, routine.id, "2026-09-28T06:48:00.000Z");
    expect(getRoutine(db, routine.id).nextRunAt).toBe("2026-09-28T06:48:00.000Z");
    setNextRun(db, routine.id, null);
    expect(getRoutine(db, routine.id).nextRunAt).toBeNull();
  });

  it("records run lifecycle timestamps, notes and open status", () => {
    vi.useFakeTimers();
    const queuedAt = at("2026-09-25T06:48:00Z");
    vi.setSystemTime(queuedAt);
    const routine = createRoutine(db, input());
    const other = createRoutine(db, { ...input(), name: "Other" });
    expect(hasOpenRun(db, routine.id)).toBe(false);
    const run = addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "queued");
    expect(getRun(db, run.id)).toEqual({ ...run, startedAt: null, endedAt: null, note: null, status: "queued" });
    expect(hasOpenRun(db, routine.id)).toBe(true);
    expect(hasOpenRun(db, other.id)).toBe(false);
    vi.setSystemTime(queuedAt + 1000);
    expect(setRunStatus(db, run.id, "running")).toEqual({ ...run, status: "running", startedAt: "2026-09-25T06:48:01.000Z" });
    expect(hasOpenRun(db, routine.id)).toBe(true);
    vi.setSystemTime(queuedAt + 5000);
    expect(setRunStatus(db, run.id, "done", "Checked")).toEqual({
      ...run, status: "done", startedAt: "2026-09-25T06:48:01.000Z", endedAt: "2026-09-25T06:48:05.000Z", note: "Checked",
    });
    expect(hasOpenRun(db, routine.id)).toBe(false);
  });

  it.each(["missed", "skipped"] as const)("records %s runs as ended without starting them", (status) => {
    vi.useFakeTimers();
    vi.setSystemTime(at("2026-09-25T07:00:00Z"));
    const routine = createRoutine(db, input());
    const run = addRun(db, routine.id, "2026-09-25T06:48:00.000Z", status, "Not run");
    expect(run).toMatchObject({ status, note: "Not run", startedAt: null, endedAt: "2026-09-25T07:00:00.000Z" });
    expect(hasOpenRun(db, routine.id)).toBe(false);
  });

  it("retains exactly 20 newest runs per routine and breaks due-time ties by insertion order", () => {
    expect(RUNS_KEPT).toBe(20);
    const routine = createRoutine(db, input());
    const other = createRoutine(db, { ...input(), name: "Other" });
    const independent = addRun(db, other.id, "2026-08-01T06:48:00.000Z", "done");
    const runs = Array.from({ length: 25 }, () => addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "done"));
    expect(listRuns(db, routine.id)).toEqual(runs.slice(5).reverse());
    expect(getRun(db, runs[0].id)).toBeNull();
    expect(listRuns(db, other.id)).toEqual([independent]);
  });

  it("preserves the oldest queued run while pruning finished history, then deletes its queued work", () => {
    const routine = createRoutine(db, input());
    const queued = addRun(db, routine.id, "2026-08-01T06:48:00.000Z", "queued");
    const work = enqueue(db, { botId, source: "routine", text: "Waiting", routineRunId: queued.id });
    const finished = Array.from({ length: RUNS_KEPT }, () => addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "done"));
    expect(listRuns(db, routine.id)).toHaveLength(RUNS_KEPT + 1);
    const newest = addRun(db, routine.id, "2026-09-26T06:48:00.000Z", "done");
    expect(listRuns(db, routine.id)).toEqual([newest, ...finished.slice(1).reverse(), queued]);
    expect(getRun(db, finished[0].id)).toBeNull();
    expect(getRun(db, queued.id)).toEqual(queued);
    expect(hasOpenRun(db, routine.id)).toBe(true);
    expect(getWorkItem(db, work.id).routineRunId).toBe(queued.id);
    deleteRoutine(db, routine.id);
    expect(listRuns(db, routine.id)).toEqual([]);
    expect(() => getWorkItem(db, work.id)).toThrow();
  });

  it("preserves multiple queued and running runs independently of the finished-history cap", () => {
    const routine = createRoutine(db, input());
    const queued = addRun(db, routine.id, "2026-08-01T06:48:00.000Z", "queued");
    const running = addRun(db, routine.id, "2026-08-02T06:48:00.000Z", "running");
    const alsoQueued = addRun(db, routine.id, "2026-08-03T06:48:00.000Z", "queued");
    const finished = Array.from({ length: RUNS_KEPT + 1 }, () => addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "done"));
    expect(listRuns(db, routine.id)).toEqual([...finished.slice(1).reverse(), alsoQueued, running, queued]);
    expect(getRun(db, running.id)).toEqual(running);
    expect(hasOpenRun(db, routine.id)).toBe(true);
  });

  it.each(["done", "stopped", "failed", "missed", "skipped"] as const)("enforces the finished-history cap when an open run becomes %s", (status) => {
    const routine = createRoutine(db, input());
    const finished = Array.from({ length: RUNS_KEPT }, () => addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "done"));
    const open = addRun(db, routine.id, "2026-09-26T06:48:00.000Z", "running");
    const ended = setRunStatus(db, open.id, status, "Finished");
    expect(ended).toMatchObject({ id: open.id, status, note: "Finished", endedAt: expect.any(String) });
    expect(listRuns(db, routine.id)).toEqual([ended, ...finished.slice(1).reverse()]);
    expect(getRun(db, finished[0].id)).toBeNull();
    expect(hasOpenRun(db, routine.id)).toBe(false);
  });

  it("returns the finished snapshot when an older open run falls outside retained history", () => {
    const routine = createRoutine(db, input());
    const open = addRun(db, routine.id, "2026-08-01T06:48:00.000Z", "queued");
    const finished = Array.from({ length: RUNS_KEPT }, () => addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "done"));
    expect(setRunStatus(db, open.id, "stopped", "Cancelled")).toMatchObject({ id: open.id, status: "stopped", note: "Cancelled" });
    expect(listRuns(db, routine.id)).toEqual(finished.reverse());
    expect(getRun(db, open.id)).toBeNull();
    expect(hasOpenRun(db, routine.id)).toBe(false);
  });

  it("returns a recorded run even when its older due time puts it outside the retained history", () => {
    const routine = createRoutine(db, input());
    const retained = Array.from({ length: 20 }, () => addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "done"));
    const older = addRun(db, routine.id, "2026-08-01T06:48:00.000Z", "missed", "Late record");
    expect(older).toMatchObject({ routineId: routine.id, dueAt: "2026-08-01T06:48:00.000Z", status: "missed", note: "Late record" });
    expect(listRuns(db, routine.id)).toEqual(retained.reverse());
    expect(getRun(db, older.id)).toBeNull();
  });

  it("deletes only the routine's queued work, leaving active and unrelated work", () => {
    const routine = createRoutine(db, input());
    const other = createRoutine(db, { ...input(), name: "Other" });
    const run = addRun(db, routine.id, "2026-09-25T06:48:00.000Z", "running");
    const waitingRun = addRun(db, routine.id, "2026-09-25T10:47:00.000Z", "queued");
    const otherRun = addRun(db, other.id, "2026-09-25T06:48:00.000Z", "queued");
    const active = enqueue(db, { botId, source: "routine", text: "Active", routineRunId: run.id });
    setStatus(db, active.id, "running");
    const queued = enqueue(db, { botId, source: "routine", text: "Waiting", routineRunId: waitingRun.id });
    const unrelated = enqueue(db, { botId, source: "routine", text: "Other", routineRunId: otherRun.id });
    const userWork = enqueue(db, { botId, source: "you", text: "Hello" });
    deleteRoutine(db, routine.id);
    expect(() => getRoutine(db, routine.id)).toThrow("No such routine");
    expect(listRuns(db, routine.id)).toEqual([]);
    expect(() => getWorkItem(db, queued.id)).toThrow();
    expect(getWorkItem(db, active.id).status).toBe("running");
    expect(getWorkItem(db, unrelated.id)).toEqual(unrelated);
    expect(getWorkItem(db, userWork.id)).toEqual(userWork);
    expect(getRoutine(db, other.id)).toEqual(other);
    expect(getRun(db, otherRun.id)).toEqual(otherRun);
  });
});
