import { newId, nowIso, type Db } from "./db.ts";

// Where a piece of work came from.
export type WorkSource = "you" | "room" | "bot_request" | "bot_reply" | "routine";
export type WorkStatus = "queued" | "running" | "done" | "failed" | "interrupted";
export type WorkItem = {
  id: string;
  botId: string;
  source: WorkSource;
  text: string;
  messageId: string | null;
  conversationId: string; // where the turn's output goes: the bot's chat, "team" or an exchange
  chainId: string | null;
  exchangeId: string | null;
  routineRunId: string | null;
  status: WorkStatus;
  createdAt: string;
};
export type NewWork = {
  botId: string;
  source: WorkSource;
  text: string;
  messageId?: string | null;
  conversationId?: string;
  chainId?: string | null;
  exchangeId?: string | null;
  routineRunId?: string | null;
};

// Your messages and replies to a bot's own requests go first; requests from other bots and routine runs wait.
const PRIORITY: Record<WorkSource, number> = { you: 0, room: 0, bot_reply: 0, bot_request: 1, routine: 1 };

type WorkRow = {
  id: string;
  bot_id: string;
  source: WorkSource;
  text: string;
  message_id: string | null;
  conversation_id: string | null;
  chain_id: string | null;
  exchange_id: string | null;
  routine_run_id: string | null;
  status: WorkStatus;
  created_at: string;
};

const toItem = (row: WorkRow): WorkItem => {
  if (row.conversation_id === null) throw new Error(`Work item ${row.id} has a null conversation_id`);
  return {
    id: row.id,
    botId: row.bot_id,
    source: row.source,
    text: row.text,
    messageId: row.message_id,
    conversationId: row.conversation_id,
    chainId: row.chain_id,
    exchangeId: row.exchange_id,
    routineRunId: row.routine_run_id,
    status: row.status,
    createdAt: row.created_at,
  };
};

export function enqueue(db: Db, work: NewWork): WorkItem {
  const id = newId();
  db.prepare(
    `INSERT INTO work_items (id, bot_id, source, text, message_id, conversation_id, priority, chain_id, exchange_id, routine_run_id, status, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?)`,
  ).run(
    id,
    work.botId,
    work.source,
    work.text,
    work.messageId ?? null,
    work.conversationId ?? work.botId,
    PRIORITY[work.source],
    work.chainId ?? null,
    work.exchangeId ?? null,
    work.routineRunId ?? null,
    nowIso(),
  );
  return getWorkItem(db, id);
}

export function getWorkItem(db: Db, id: string): WorkItem {
  const row = db.prepare("SELECT * FROM work_items WHERE id = ?").get(id) as WorkRow | undefined;
  if (!row) throw new Error(`No work item ${id}`);
  return toItem(row);
}

// Highest priority first, then oldest. rowid breaks ties within one millisecond.
export function nextQueued(db: Db, botId: string): WorkItem | null {
  const row = db.prepare("SELECT * FROM work_items WHERE bot_id = ? AND status = 'queued' ORDER BY priority, created_at, rowid LIMIT 1").get(botId) as
    | WorkRow
    | undefined;
  return row ? toItem(row) : null;
}

// Work that may join a turn already writing to `conversationId`: your messages there, and replies to the bot's own requests.
export function nextSteerable(db: Db, botId: string, conversationId: string): WorkItem | null {
  const row = db
    .prepare(
      `SELECT * FROM work_items WHERE bot_id = ? AND status = 'queued' AND source IN ('you', 'bot_reply') AND conversation_id = ?
       ORDER BY priority, created_at, rowid LIMIT 1`,
    )
    .get(botId, conversationId) as WorkRow | undefined;
  return row ? toItem(row) : null;
}

export function setStatus(db: Db, id: string, status: WorkStatus): void {
  if (status === "queued") db.prepare("UPDATE work_items SET status = 'queued', started_at = NULL, ended_at = NULL WHERE id = ?").run(id);
  else if (status === "running") db.prepare("UPDATE work_items SET status = 'running', started_at = ? WHERE id = ?").run(nowIso(), id);
  else db.prepare("UPDATE work_items SET status = ?, ended_at = ? WHERE id = ?").run(status, nowIso(), id);
}

export function lastItemForMessage(db: Db, messageId: string, botId?: string): WorkItem | null {
  const row = (botId === undefined
    ? db.prepare("SELECT * FROM work_items WHERE message_id = ? ORDER BY created_at DESC, rowid DESC LIMIT 1").get(messageId)
    : db.prepare("SELECT * FROM work_items WHERE message_id = ? AND bot_id = ? ORDER BY created_at DESC, rowid DESC LIMIT 1").get(messageId, botId)) as WorkRow | undefined;
  return row ? toItem(row) : null;
}

// After a restart nothing is running any more: mark what was running as interrupted and return it.
export function recoverAfterRestart(db: Db): WorkItem[] {
  const rows = db.prepare("SELECT * FROM work_items WHERE status = 'running' ORDER BY created_at, rowid").all() as WorkRow[];
  db.prepare("UPDATE work_items SET status = 'interrupted', ended_at = ? WHERE status = 'running'").run(nowIso());
  return rows.map((row) => ({ ...toItem(row), status: "interrupted" }));
}

// A chain is one message from you, one room post or one routine run, plus everything it causes.
export function newChain(db: Db): string {
  const id = newId();
  db.prepare("INSERT INTO chains (id, requests_made, created_at) VALUES (?, 0, ?)").run(id, nowIso());
  return id;
}

// Counts one bot-to-bot request against the chain. False once the chain has used `limit` requests.
export function useChainRequest(db: Db, chainId: string, limit: number): boolean {
  const result = db.prepare("UPDATE chains SET requests_made = requests_made + 1 WHERE id = ? AND requests_made < ?").run(chainId, limit);
  return Number(result.changes) === 1;
}
