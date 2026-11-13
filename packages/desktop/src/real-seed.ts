import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

// Rows that the desktop's real tests plant in a disposable data root, beside what the Rust daemon wrote there. The
// statements are the ones the 0.4.2 TypeScript daemon ran (chat.ts, approvals.ts, usage.ts, devices-store.ts and
// accounts-store.ts), kept as plain SQL against the shared schema (user_version 12). They add no validation: a test plants
// well-formed rows, and the Rust daemon reads them back through its own checks. real-seed.test.ts runs every helper against
// the committed fixture database, so a schema change that breaks one fails there, not in a two-minute real test.

export const newId = (): string => randomUUID();
export const nowIso = (): string => new Date().toISOString();

// An already-migrated database: the Rust daemon created it when the app started. Same pragmas as the daemon's own opens.
export function openSeedDb(file: string): DatabaseSync {
  const db = new DatabaseSync(file);
  db.exec("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
  return db;
}

export function addMessage(
  db: DatabaseSync,
  m: { conversationId: string; author: string; kind: string; body: string; data?: Record<string, unknown> | null; status: string },
): { id: string; body: string } {
  const id = newId();
  const { seq } = db.prepare("SELECT COALESCE(MAX(seq), 0) + 1 AS seq FROM messages WHERE conversation_id = ?").get(m.conversationId) as { seq: number };
  db.prepare("INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
    .run(id, m.conversationId, m.author, m.kind, m.body, m.data ? JSON.stringify(m.data) : null, m.status, nowIso(), seq);
  return { id, body: m.body };
}

export function createApproval(
  db: DatabaseSync,
  a: { id: string; botId: string; tool: string; summary: string; detail: string; rule: string | null; messageId: string },
): void {
  db.prepare("INSERT INTO approvals (id, bot_id, kind, tool, summary, detail, rule, payload, status, message_id, created_at) VALUES (?, ?, 'tool', ?, ?, ?, ?, NULL, 'pending', ?, ?)")
    .run(a.id, a.botId, a.tool, a.summary, a.detail, a.rule, a.messageId, nowIso());
}

export function recordUsage(
  db: DatabaseSync,
  u: { botId: string; accountId: string; provider: string; model: string; input: number; cachedInput: number; output: number; reasoning: number; costUsd: number | null; costSource: "reported" | "estimated" | "unknown" },
): void {
  const account = db.prepare("SELECT kind FROM accounts WHERE id = ?").get(u.accountId) as { kind: string } | undefined;
  if (!account) throw new Error(`Usage needs an existing account: ${u.accountId}`);
  db.prepare(`INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at, account_kind, pricing_revision)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)`)
    .run(newId(), u.botId, u.accountId, u.provider, u.model, u.input, u.cachedInput, u.output, u.reasoning, u.costUsd, u.costSource, nowIso(), account.kind);
}

export function addDevice(db: DatabaseSync, input: { publicKey: string; name: string }): { id: string } {
  const id = newId();
  db.prepare("INSERT INTO devices(id,name,public_key,paired_at) VALUES(?,?,?,?)").run(id, input.name, input.publicKey, nowIso());
  return { id };
}

export type SeedLimitWindow = { windowKey: string; durationMinutes: number | null; usedPercent: number | null; resetsAt: string | null; state: "ok" | "near" | "reached" };

// A fresh quota snapshot for one account and limit, and the account's `limited_until`: the latest reset among windows that
// reached their limit, or none when a reached window has no reset time.
export function replaceAccountLimits(db: DatabaseSync, input: { accountId: string; limitId: string; observedAt: string; windows: SeedLimitWindow[] }): void {
  db.exec("BEGIN IMMEDIATE");
  try {
    db.prepare("INSERT INTO account_limit_snapshots VALUES (?, ?, ?) ON CONFLICT(account_id, limit_id) DO UPDATE SET observed_at = excluded.observed_at")
      .run(input.accountId, input.limitId, input.observedAt);
    db.prepare("DELETE FROM account_limit_windows WHERE account_id = ? AND limit_id = ?").run(input.accountId, input.limitId);
    const insert = db.prepare("INSERT INTO account_limit_windows VALUES (?, ?, ?, ?, ?, ?, ?, ?)");
    for (const w of input.windows) insert.run(input.accountId, input.limitId, w.windowKey, w.durationMinutes, w.usedPercent, w.resetsAt, w.state, input.observedAt);
    const resets = (db.prepare("SELECT resets_at FROM account_limit_windows WHERE account_id = ? AND state = 'reached'").all(input.accountId) as { resets_at: string | null }[]).map((w) => w.resets_at);
    const until = resets.length === 0 || resets.includes(null) ? null : (resets as string[]).sort().at(-1)!;
    db.prepare("UPDATE accounts SET limited_until = ? WHERE id = ?").run(until, input.accountId);
    db.exec("COMMIT");
  } catch (error) { db.exec("ROLLBACK"); throw error; }
}
