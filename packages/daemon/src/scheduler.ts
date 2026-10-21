import { messageData, type Message, type Routine, type RoutineLabelData, type RoutineRun } from "@tjommie/protocol";
import { getStoredBot } from "./bots.ts";
import { addMessage, getMessage, updateMessage } from "./chat.ts";
import { transaction, type Db } from "./db.ts";
import { enqueue, getWorkItem, newChain, setStatus, type WorkItem } from "./queue.ts";
import { addRun, getRoutine, getRun, hasOpenRun, nextRunAfter, setNextRun, setRunStatus } from "./routines.ts";
import { RpcError, type Broadcast } from "./server.ts";

export const TICK_MS = 30_000;
export const GRACE_MS = 120_000; // a run later than this counts as missed

export type TurnOutcome = { outcome: "done" | "failed" | "interrupted"; error: string | null };

export const duration = (minutes: number): string =>
  minutes % 60 === 0 ? `${minutes / 60} ${minutes === 60 ? "hour" : "hours"}` : `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;

// What the bot gets when its routine runs.
export function routinePrompt(routine: Routine): string {
  const limit = routine.timeLimitMinutes ? ` You have ${duration(routine.timeLimitMinutes)}; then Tjommie stops you.` : "";
  return `[Routine "${routine.name}", started by Tjommie on schedule.${limit}]\n${routine.prompt}`;
}

// Starts routine runs when they're due, and records how each one went.
export class Scheduler {
  #db: Db;
  #broadcast: Broadcast;
  #deliver: (work: WorkItem) => void;
  #stopItem: (itemId: string, notice: string) => void | Promise<void>;
  #timer: NodeJS.Timeout | null = null;
  #limits = new Map<string, NodeJS.Timeout>(); // run id -> its time limit
  #timedOut = new Set<string>();

  constructor(deps: { db: Db; broadcast: Broadcast; deliver: (work: WorkItem) => void; stopItem: (itemId: string, notice: string) => void | Promise<void> }) {
    this.#db = deps.db;
    this.#broadcast = deps.broadcast;
    this.#deliver = deps.deliver;
    this.#stopItem = deps.stopItem;
  }

  // Checks for due runs now and every 30 seconds. Runs that a restart cut off are settled by the runner's
  // restart recovery, which reports them through runEnded (Task 8).
  start(): void {
    if (this.#timer) return;
    this.recover();
    this.#tickSafely();
    this.#timer = setInterval(() => this.#tickSafely(), TICK_MS);
    this.#timer.unref();
  }

  stop(): void {
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = null;
    for (const timer of this.#limits.values()) clearTimeout(timer);
    this.#limits.clear();
  }

  // Runner recovery settles interrupted work first. Preserve live queue entries (including usage
  // retries), but release runs left open by older non-atomic scheduling or interrupted hooks.
  recover(): void {
    const events: (() => void)[] = [];
    const broadcast: Broadcast = (event, data) => { events.push(() => this.#broadcast(event, data)); };
    transaction(this.#db, () => {
      const rows = this.#db.prepare(`SELECT id FROM routine_runs r WHERE status IN ('queued', 'running')
        AND NOT EXISTS (SELECT 1 FROM work_items w WHERE w.routine_run_id = r.id AND w.status IN ('queued', 'running'))`).all() as { id: string }[];
      for (const { id } of rows) {
        const run = setRunStatus(this.#db, id, "failed", "Tjommie restarted before this routine run could finish.");
        if (!run) continue;
        const routine = getRoutine(this.#db, run.routineId);
        this.#label(routine, run, broadcast);
        broadcast("routine.run", run);
        events.push(() => this.#notifyFailure(routine, run, run.note!));
      }
    });
    for (const send of events) send();
  }

  tick(nowMs = Date.now()): void {
    const due = this.#db
      .prepare("SELECT id FROM routines WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ? ORDER BY next_run_at")
      .all(new Date(nowMs).toISOString()) as { id: string }[];
    for (const { id } of due) {
      try {
        this.#fire(getRoutine(this.#db, id), nowMs);
      } catch (error) {
        // The failed transaction leaves this routine due for retry. Keep processing the others.
        console.error(`Routine scheduler failed for ${id}.`, error);
      }
    }
  }

  #tickSafely(): void {
    try {
      this.tick();
    } catch (error) {
      console.error("Routine scheduler tick failed.", error);
    }
  }

  // The runner calls this when a routine run's turn starts.
  runStarted(item: WorkItem): void {
    const id = item.routineRunId;
    if (!id) return;
    const run = getRun(this.#db, id);
    if (!run) {
      this.runRequeued(item); // the routine was deleted; discard its timer too
      return;
    }
    if (run.status !== "queued" && run.status !== "running") return;
    const routine = getRoutine(this.#db, run.routineId);
    if (run.status === "queued") {
      const started = setRunStatus(this.#db, id, "running");
      if (!started) return; // the routine was deleted
      this.#label(routine, started);
      this.#broadcast("routine.run", started);
    }
    this.#clearLimit(id);
    this.#timedOut.delete(id);
    if (!routine.timeLimitMinutes) return;
    // Restart the full budget after a usage limit. A limited routine can run up to about twice its
    // limit overall across the original turn and one retry; time parked in the queue does not count.
    const notice = `Stopped at the time limit (${duration(routine.timeLimitMinutes)}).`;
    const timer = setTimeout(() => {
      this.#limits.delete(id);
      const failed = (error: unknown) => {
        this.#timedOut.delete(id);
        console.error("Routine time limit failed.", error);
      };
      try {
        const current = getRun(this.#db, id);
        if (!current || current.status !== "running") return; // deleted or already finished
        this.#timedOut.add(id);
        void Promise.resolve(this.#stopItem(item.id, notice)).catch(failed);
      } catch (error) {
        failed(error);
      }
    }, routine.timeLimitMinutes * 60_000);
    timer.unref();
    this.#limits.set(id, timer);
  }

  // The runner calls this when a provider usage limit puts the item back in its queue.
  runRequeued(item: WorkItem): void {
    const id = item.routineRunId;
    if (!id) return;
    this.#clearLimit(id);
    this.#timedOut.delete(id);
    // Keep the run open. runStarted will re-arm its full time limit when work resumes.
  }

  // The runner calls this when a routine run's turn ends.
  runEnded(item: WorkItem, result: TurnOutcome): void {
    const id = item.routineRunId;
    if (!id) return;
    this.#clearLimit(id);
    const timedOut = this.#timedOut.delete(id);
    const run = getRun(this.#db, id);
    if (!run) {
      // Deleted routines have no run row; their retained label still needs a final state.
      if (item.messageId && this.#db.prepare("SELECT 1 FROM messages WHERE id = ?").get(item.messageId)) {
        const parsed = messageData(getMessage(this.#db, item.messageId));
        if (parsed.kind === "routine" && (parsed.data.status === "queued" || parsed.data.status === "running")) {
          const data = { ...parsed.data, status: "stopped", note: "The routine was deleted." } satisfies RoutineLabelData;
          this.#broadcast("message.updated", updateMessage(this.#db, item.messageId, { data }));
        }
      }
      return;
    }
    if (run.status !== "queued" && run.status !== "running") return;
    const status = timedOut || result.outcome === "interrupted" ? "stopped" : result.outcome;
    const note = timedOut ? "Stopped at the time limit." : status === "stopped" ? "Stopped by you." : status === "failed" ? (result.error ?? "Something went wrong.") : null;
    const ended = setRunStatus(this.#db, id, status, note);
    if (!ended) return; // the routine was deleted meanwhile
    this.#label(getRoutine(this.#db, run.routineId), ended);
    this.#broadcast("routine.run", ended);
    if (status === "failed") this.#notifyFailure(getRoutine(this.#db, run.routineId), ended, note ?? "Something went wrong.");
  }

  #clearLimit(id: string): void {
    clearTimeout(this.#limits.get(id));
    this.#limits.delete(id);
  }

  #fire(routine: Routine, nowMs: number): void {
    const events: (() => void)[] = [];
    const broadcast: Broadcast = (event, data) => { events.push(() => this.#broadcast(event, data)); };
    let item: WorkItem | undefined;
    transaction(this.#db, () => {
      const dueAt = routine.nextRunAt!;
      const bot = getStoredBot(this.#db, routine.botId);
      let run: RoutineRun;
      if (nowMs - Date.parse(dueAt) > GRACE_MS) run = addRun(this.#db, routine.id, dueAt, "missed", "The PC was off or asleep, or Tjommie wasn't running.");
      else if (bot.paused) run = addRun(this.#db, routine.id, dueAt, "skipped", `${bot.name} was paused.`);
      else if (hasOpenRun(this.#db, routine.id)) run = addRun(this.#db, routine.id, dueAt, "skipped", "The previous run was still going.");
      else run = addRun(this.#db, routine.id, dueAt, "queued");
      // Skip older missed times, preserve times within grace, and advance past this due time to avoid duplicates.
      const next = nextRunAfter(routine, Math.max(nowMs - GRACE_MS, Date.parse(dueAt)));
      setNextRun(this.#db, routine.id, next === null ? null : new Date(next).toISOString());
      broadcast("routine.updated", getRoutine(this.#db, routine.id));
      broadcast("routine.run", run);
      const label = this.#label(routine, run, broadcast);
      if (run.status === "queued") item = this.#queue(routine, run, label);
    });
    for (const send of events) send();
    if (item) this.#wake(item);
  }

  // A manual retry is a new run with its own label and chain, leaving the old result intact.
  retry(message: Message): Message {
    const parsed = messageData(message);
    if (parsed.kind !== "routine") throw new RpcError("bad_params", "This message is not a routine label");
    const routine = getRoutine(this.#db, parsed.data.routineId);
    if (!routine.enabled) throw new RpcError("bad_state", "Enable this routine before retrying it");
    if (getStoredBot(this.#db, routine.botId).paused) throw new RpcError("bad_state", "Unpause this bot before retrying its routine");
    if (hasOpenRun(this.#db, routine.id)) throw new RpcError("busy", "This routine already has a queued or running run");
    let queued: { run: RoutineRun; label: Message; item: WorkItem } | undefined;
    transaction(this.#db, () => {
      const run = addRun(this.#db, routine.id, new Date().toISOString(), "queued");
      const label = this.#label(routine, run, () => {});
      queued = { run, label, item: this.#queue(routine, run, label) };
    });
    const { run, label, item } = queued!;
    this.#broadcast("routine.run", run);
    this.#broadcast("message.added", label);
    this.#wake(item);
    return getMessage(this.#db, label.id);
  }

  // Persist only: callers include this in the run/schedule transaction and wake after commit.
  #queue(routine: Routine, run: RoutineRun, label: Message): WorkItem {
    return enqueue(this.#db, { botId: routine.botId, source: "routine", text: routinePrompt(routine), messageId: label.id, chainId: newChain(this.#db), routineRunId: run.id });
  }

  #wake(item: WorkItem): void {
    try {
      this.#deliver(item);
    } catch (error) {
      console.error("Routine delivery failed.", error);
      // A callback that already started the turn owns its outcome. Never fail live work here.
      if (getWorkItem(this.#db, item.id).status !== "queued") return;
      const run = getRun(this.#db, item.routineRunId!);
      if (!run) return;
      const routine = getRoutine(this.#db, run.routineId);
      this.#clearLimit(run.id);
      this.#timedOut.delete(run.id);
      // The next run was computed from this tick before delivery, so failure cannot retry or burst it.
      const note = "Could not queue the routine run.";
      const result: { failed: RoutineRun | null } = { failed: null };
      transaction(this.#db, () => {
        setStatus(this.#db, item.id, "failed");
        result.failed = setRunStatus(this.#db, run.id, "failed", note);
      });
      const { failed } = result;
      if (failed) {
        this.#broadcast("routine.run", failed);
        this.#label(routine, failed);
        this.#notifyFailure(routine, failed, note);
      }
    }
  }

  #notifyFailure(routine: Routine, run: RoutineRun, note: string): void {
    this.#broadcast("notify", { kind: "routine_failed", botId: routine.botId, routineId: routine.id, runId: run.id, title: `${getStoredBot(this.#db, routine.botId).name}'s routine failed`, body: `${routine.name}: ${note}` });
  }

  // The "Routine: <name>" row in the bot's chat, above the run or saying why it didn't run.
  #label(routine: Routine, run: RoutineRun, broadcast: Broadcast = this.#broadcast): Message {
    const data = { routineId: routine.id, runId: run.id, status: run.status, note: run.note } satisfies RoutineLabelData;
    const existing = this.#db.prepare("SELECT id FROM messages WHERE conversation_id = ? AND kind = 'routine' AND json_extract(data, '$.runId') = ? LIMIT 1").get(routine.botId, run.id) as { id: string } | undefined;
    if (existing) {
      const message = updateMessage(this.#db, existing.id, { data });
      broadcast("message.updated", message);
      return message;
    }
    const message = addMessage(this.#db, {
      conversationId: routine.botId,
      author: "system",
      kind: "routine",
      body: `Routine: ${routine.name}`,
      data,
      status: "done",
    });
    broadcast("message.added", message);
    return message;
  }
}
