import type { Message } from "@tjommie/protocol";
import { newId, nowIso, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

type MessageRow = {
  id: string;
  conversation_id: string;
  author: string;
  kind: Message["kind"];
  body: string;
  data: string | null;
  status: Message["status"];
  created_at: string;
};

const toMessage = (row: MessageRow): Message => ({
  id: row.id,
  conversationId: row.conversation_id,
  author: row.author,
  kind: row.kind,
  body: row.body,
  data: row.data === null ? null : (JSON.parse(row.data) as Record<string, unknown>),
  status: row.status,
  createdAt: row.created_at,
});

export function addMessage(
  db: Db,
  m: { conversationId: string; author: string; kind: Message["kind"]; body: string; data?: Record<string, unknown> | null; status: Message["status"] },
): Message {
  const id = newId();
  const { seq } = db.prepare("SELECT COALESCE(MAX(seq), 0) + 1 AS seq FROM messages WHERE conversation_id = ?").get(m.conversationId) as { seq: number };
  db.prepare("INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)").run(
    id,
    m.conversationId,
    m.author,
    m.kind,
    m.body,
    m.data ? JSON.stringify(m.data) : null,
    m.status,
    nowIso(),
    seq,
  );
  return getMessage(db, id);
}

export function getMessage(db: Db, id: string): Message {
  const row = db.prepare("SELECT * FROM messages WHERE id = ?").get(id) as MessageRow | undefined;
  if (!row) throw new RpcError("not_found", "No such message");
  return toMessage(row);
}

export function updateMessage(db: Db, id: string, patch: { body?: string; data?: Record<string, unknown> | null; status?: Message["status"] }): Message {
  const current = getMessage(db, id);
  const data = patch.data === undefined ? current.data : patch.data;
  db.prepare("UPDATE messages SET body = ?, data = ?, status = ? WHERE id = ?").run(patch.body ?? current.body, data ? JSON.stringify(data) : null, patch.status ?? current.status, id);
  return getMessage(db, id);
}

// The newest `limit` messages before the message `before`, oldest first.
export function history(db: Db, conversationId: string, before: string | undefined, limit: number): Message[] {
  const cursor = before === undefined
    ? undefined
    : (db.prepare("SELECT seq FROM messages WHERE id = ? AND conversation_id = ?").get(before, conversationId) as { seq: number } | undefined);
  if (before !== undefined && cursor === undefined) throw new RpcError("not_found", "No such message");
  const rows = db
    .prepare("SELECT * FROM messages WHERE conversation_id = ? AND seq < ? ORDER BY seq DESC LIMIT ?")
    .all(conversationId, cursor?.seq ?? Number.MAX_SAFE_INTEGER, limit) as MessageRow[];
  return rows.reverse().map(toMessage);
}

// Opening text for a fresh CLI session, so the bot picks up an ongoing chat where it left off.
export function buildRecap(messages: Message[], botName: string): string {
  const lines = messages
    .filter((m) => m.kind === "text" && m.body.trim() !== "")
    .map((m) => `${m.author === "you" ? "Person" : botName}: ${m.body.trim().slice(0, 2000)}`);
  if (lines.length === 0) return "";
  return ["[Tjommie: this is a new session of an ongoing chat. Your notes are in notes.md in your home folder. Recent messages, oldest first:", ...lines, "]"].join("\n");
}
