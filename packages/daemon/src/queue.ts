import { newId, nowIso, type Db } from "./db.ts";

// Where a piece of work came from. Plan 1B adds room posts, requests from other bots and routine runs.
export type WorkSource = "you";
export type WorkStatus = "queued" | "running" | "done" | "failed" | "interrupted";
export type WorkItem = { id: string; botId: string; source: WorkSource; text: string; messageId: string | null; status: WorkStatus; createdAt: string };

type WorkRow = { id: string; bot_id: string; source: WorkSource; text: string; message_id: string | null; status: WorkStatus; created_at: string };

const toItem = (row: WorkRow): WorkItem => ({
  id: row.id,
  botId: row.bot_id,
  source: row.source,
  text: row.text,
  messageId: row.message_id,
  status: row.status,
  createdAt: row.created_at,
});

export function enqueue(db: Db, work: { botId: string; source: WorkSource; text: string; messageId: string | null }): WorkItem {
  const id = newId();
  db.prepare("INSERT INTO work_items (id, bot_id, source, text, message_id, status, created_at) VALUES (?, ?, ?, ?, ?, 'queued', ?)").run(
    id,
    work.botId,
    work.source,
    work.text,
    work.messageId,
    nowIso(),
  );
  return getWorkItem(db, id);
}

export function getWorkItem(db: Db, id: string): WorkItem {
  const row = db.prepare("SELECT * FROM work_items WHERE id = ?").get(id) as WorkRow | undefined;
  if (!row) throw new Error(`No work item ${id}`);
  return toItem(row);
}

// Oldest first. rowid breaks ties between items created in the same millisecond.
export function nextQueued(db: Db, botId: string): WorkItem | null {
  const row = db.prepare("SELECT * FROM work_items WHERE bot_id = ? AND status = 'queued' ORDER BY created_at, rowid LIMIT 1").get(botId) as WorkRow | undefined;
  return row ? toItem(row) : null;
}

export function setStatus(db: Db, id: string, status: WorkStatus): void {
  if (status === "queued") db.prepare("UPDATE work_items SET status = 'queued', started_at = NULL, ended_at = NULL WHERE id = ?").run(id);
  else if (status === "running") db.prepare("UPDATE work_items SET status = 'running', started_at = ? WHERE id = ?").run(nowIso(), id);
  else db.prepare("UPDATE work_items SET status = ?, ended_at = ? WHERE id = ?").run(status, nowIso(), id);
}

export function lastItemForMessage(db: Db, messageId: string): WorkItem | null {
  const row = db.prepare("SELECT * FROM work_items WHERE message_id = ? ORDER BY created_at DESC, rowid DESC LIMIT 1").get(messageId) as WorkRow | undefined;
  return row ? toItem(row) : null;
}

// After a restart nothing is running any more: mark what was running as interrupted and return it.
export function recoverAfterRestart(db: Db): WorkItem[] {
  const rows = db.prepare("SELECT * FROM work_items WHERE status = 'running' ORDER BY created_at, rowid").all() as WorkRow[];
  db.prepare("UPDATE work_items SET status = 'interrupted', ended_at = ? WHERE status = 'running'").run(nowIso());
  return rows.map((row) => ({ ...toItem(row), status: "interrupted" }));
}
