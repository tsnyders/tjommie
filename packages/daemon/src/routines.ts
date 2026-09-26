import type { Routine, RoutineInput, RoutinePatch, RoutineRun, RoutineRunStatus } from "@tjommie/protocol";
import { getStoredBot } from "./bots.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

export const RUNS_KEPT = 20;
const DAY_MS = 86_400_000;

export const hostTimeZone = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone;

type RoutineRow = {
  id: string;
  bot_id: string;
  name: string;
  prompt: string;
  days: string;
  times: string;
  time_zone: string;
  time_limit_minutes: number | null;
  enabled: number;
  next_run_at: string | null;
  created_at: string;
};
type RunRow = { id: string; bot_id: string; routine_id: string; due_at: string; started_at: string | null; ended_at: string | null; status: RoutineRunStatus; note: string | null };

const toRoutine = (row: RoutineRow): Routine => ({
  id: row.id,
  botId: row.bot_id,
  name: row.name,
  prompt: row.prompt,
  days: JSON.parse(row.days) as number[],
  times: JSON.parse(row.times) as string[],
  timeZone: row.time_zone,
  timeLimitMinutes: row.time_limit_minutes,
  enabled: row.enabled === 1,
  nextRunAt: row.next_run_at,
  createdAt: row.created_at,
});

const toRun = (row: RunRow): RoutineRun => ({
  id: row.id,
  botId: row.bot_id,
  routineId: row.routine_id,
  dueAt: row.due_at,
  startedAt: row.started_at,
  endedAt: row.ended_at,
  status: row.status,
  note: row.note,
});

const formatters = new Map<string, Intl.DateTimeFormat>();

// Milliseconds to add to a UTC instant to get the wall-clock time in `timeZone` at that instant.
function offsetAt(utcMs: number, timeZone: string): number {
  let format = formatters.get(timeZone);
  if (!format) {
    format = new Intl.DateTimeFormat("en-US", { timeZone, hourCycle: "h23", year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric" });
    formatters.set(timeZone, format);
  }
  const p = Object.fromEntries(format.formatToParts(new Date(utcMs)).map((part) => [part.type, Number(part.value)]));
  return Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second) - Math.floor(utcMs / 1000) * 1000;
}

// The UTC instant of a wall-clock time in `timeZone`. A time that a clock change skips moves forward by the
// gap (02:30 becomes 03:30); a time that happens twice when clocks go back uses the first one.
export function wallTimeToUtc(year: number, month: number, day: number, hour: number, minute: number, timeZone: string): number {
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  const candidates = [offsetAt(wall - DAY_MS, timeZone), offsetAt(wall + DAY_MS, timeZone)].map((offset) => wall - offset);
  const exact = candidates.filter((at) => at + offsetAt(at, timeZone) === wall);
  return exact.length ? Math.min(...exact) : Math.max(...candidates);
}

// The first run time strictly after `afterMs`.
export function nextRunAfter(r: { days: number[]; times: string[]; timeZone: string }, afterMs: number): number | null {
  const local = new Date(afterMs + offsetAt(afterMs, r.timeZone));
  let next: number | null = null;
  // A spring gap can move yesterday's scheduled time into today (for example in Nuuk).
  // Compare across dates because that shifted run can come after an ordinary time today.
  for (let i = -1; i <= 7; i++) {
    const date = new Date(Date.UTC(local.getUTCFullYear(), local.getUTCMonth(), local.getUTCDate() + i));
    if (!r.days.includes(date.getUTCDay())) continue;
    const upcoming = r.times
      .map((time) => wallTimeToUtc(date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), Number(time.slice(0, 2)), Number(time.slice(3)), r.timeZone))
      .filter((at) => at > afterMs);
    if (upcoming.length) {
      const candidate = Math.min(...upcoming);
      next = next === null ? candidate : Math.min(next, candidate);
    }
  }
  return next;
}

function checkTimeZone(timeZone: string): string {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone });
    return timeZone;
  } catch {
    throw new RpcError("bad_params", `Unknown time zone ${timeZone}`);
  }
}

const nextRunIso = (r: Routine, nowMs: number): string | null => {
  const next = r.enabled ? nextRunAfter(r, nowMs) : null;
  return next === null ? null : new Date(next).toISOString();
};

export function getRoutine(db: Db, id: string): Routine {
  const row = db.prepare("SELECT * FROM routines WHERE id = ?").get(id) as RoutineRow | undefined;
  if (!row) throw new RpcError("not_found", "No such routine");
  return toRoutine(row);
}

export function listRoutines(db: Db, botId?: string): Routine[] {
  const rows = botId === undefined
    ? db.prepare("SELECT * FROM routines ORDER BY created_at, rowid").all()
    : db.prepare("SELECT * FROM routines WHERE bot_id = ? ORDER BY created_at, rowid").all(botId);
  return (rows as RoutineRow[]).map(toRoutine);
}

export function createRoutine(db: Db, input: RoutineInput, nowMs = Date.now()): Routine {
  getStoredBot(db, input.botId);
  const id = newId();
  const days = [...new Set(input.days)].sort((a, b) => a - b);
  const times = [...new Set(input.times)].sort();
  db.prepare(
    `INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, time_limit_minutes, enabled, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    id,
    input.botId,
    input.name,
    input.prompt,
    JSON.stringify(days),
    JSON.stringify(times),
    checkTimeZone(input.timeZone ?? hostTimeZone()),
    input.timeLimitMinutes ?? null,
    input.enabled === false ? 0 : 1,
    nowIso(),
  );
  const routine = getRoutine(db, id);
  setNextRun(db, id, nextRunIso(routine, nowMs));
  return getRoutine(db, id);
}

export function updateRoutine(db: Db, id: string, patch: RoutinePatch, nowMs = Date.now()): Routine {
  const previous = getRoutine(db, id);
  const next = { ...previous, ...patch };
  next.days = [...new Set(next.days)].sort((a, b) => a - b);
  next.times = [...new Set(next.times)].sort();
  db.prepare("UPDATE routines SET name = ?, prompt = ?, days = ?, times = ?, time_zone = ?, time_limit_minutes = ?, enabled = ? WHERE id = ?").run(
    next.name,
    next.prompt,
    JSON.stringify(next.days),
    JSON.stringify(next.times),
    checkTimeZone(next.timeZone),
    next.timeLimitMinutes,
    next.enabled ? 1 : 0,
    id,
  );
  if (next.enabled !== previous.enabled || next.timeZone !== previous.timeZone || JSON.stringify(next.days) !== JSON.stringify(previous.days) || JSON.stringify(next.times) !== JSON.stringify(previous.times)) {
    setNextRun(db, id, nextRunIso(next, nowMs));
  }
  return getRoutine(db, id);
}

// Deletes the routine, its run history and any run still waiting in the bot's queue.
export function deleteRoutine(db: Db, id: string): void {
  getRoutine(db, id);
  transaction(db, () => {
    db.prepare("DELETE FROM work_items WHERE status = 'queued' AND routine_run_id IN (SELECT id FROM routine_runs WHERE routine_id = ?)").run(id);
    db.prepare("DELETE FROM routines WHERE id = ?").run(id);
  });
}

export function setNextRun(db: Db, id: string, nextRunAt: string | null): void {
  db.prepare("UPDATE routines SET next_run_at = ? WHERE id = ?").run(nextRunAt, id);
}

export function getRun(db: Db, id: string): RoutineRun | null {
  const row = db.prepare("SELECT routine_runs.*, routines.bot_id FROM routine_runs JOIN routines ON routines.id = routine_runs.routine_id WHERE routine_runs.id = ?").get(id) as RunRow | undefined;
  return row ? toRun(row) : null;
}

// All open runs and the newest RUNS_KEPT finished runs, newest first.
export function listRuns(db: Db, routineId: string): RoutineRun[] {
  return (db.prepare("SELECT routine_runs.*, routines.bot_id FROM routine_runs JOIN routines ON routines.id = routine_runs.routine_id WHERE routine_id = ? ORDER BY due_at DESC, routine_runs.rowid DESC").all(routineId) as RunRow[]).map(toRun);
}

function pruneFinishedRuns(db: Db, routineId: string): void {
  db.prepare(
    `DELETE FROM routine_runs WHERE routine_id = ? AND status NOT IN ('queued', 'running') AND id NOT IN
       (SELECT id FROM routine_runs WHERE routine_id = ? AND status NOT IN ('queued', 'running')
        ORDER BY due_at DESC, rowid DESC LIMIT ?)`,
  ).run(routineId, routineId, RUNS_KEPT);
}

// Records a run, preserving every open run plus the newest RUNS_KEPT finished runs.
export function addRun(db: Db, routineId: string, dueAt: string, status: RoutineRunStatus, note: string | null = null): RoutineRun {
  const id = newId();
  const ended = status === "missed" || status === "skipped" ? nowIso() : null;
  db.prepare("INSERT INTO routine_runs (id, routine_id, due_at, ended_at, status, note) VALUES (?, ?, ?, ?, ?, ?)").run(id, routineId, dueAt, ended, status, note);
  pruneFinishedRuns(db, routineId);
  // An older due time can put the new record outside the retained history immediately.
  return { id, botId: getRoutine(db, routineId).botId, routineId, dueAt, startedAt: null, endedAt: ended, status, note };
}

export function setRunStatus(db: Db, id: string, status: RoutineRunStatus, note: string | null = null): RoutineRun | null {
  if (status === "running") db.prepare("UPDATE routine_runs SET status = 'running', started_at = ? WHERE id = ?").run(nowIso(), id);
  else db.prepare("UPDATE routine_runs SET status = ?, note = ?, ended_at = ? WHERE id = ?").run(status, note, nowIso(), id);
  // Return the updated run even if finishing it makes it too old for retained history.
  const run = getRun(db, id);
  if (run && status !== "queued" && status !== "running") pruneFinishedRuns(db, run.routineId);
  return run;
}

// True while an earlier run of the routine is still waiting or running.
export function hasOpenRun(db: Db, routineId: string): boolean {
  return db.prepare("SELECT 1 FROM routine_runs WHERE routine_id = ? AND status IN ('queued', 'running')").get(routineId) !== undefined;
}
