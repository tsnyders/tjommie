import type { Approval, ApprovalDecision } from "@tjommie/protocol";
import { nowIso, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

type ApprovalRow = {
  id: string;
  bot_id: string;
  kind: Approval["kind"];
  tool: string;
  summary: string;
  detail: string;
  rule: string | null;
  payload: string | null;
  status: Approval["status"];
  message_id: string;
  created_at: string;
};

const toApproval = (row: ApprovalRow): Approval => ({
  id: row.id,
  botId: row.bot_id,
  kind: row.kind,
  tool: row.tool,
  summary: row.summary,
  detail: row.detail,
  rule: row.rule,
  payload: row.payload === null ? null : (JSON.parse(row.payload) as Record<string, unknown>),
  status: row.status,
  messageId: row.message_id,
  createdAt: row.created_at,
});

export function createApproval(
  db: Db,
  a: {
    id: string;
    botId: string;
    tool: string;
    summary: string;
    detail: string;
    rule: string | null;
    messageId: string;
    kind?: Approval["kind"];
    payload?: Record<string, unknown> | null;
  },
): Approval {
  db.prepare(
    "INSERT INTO approvals (id, bot_id, kind, tool, summary, detail, rule, payload, status, message_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
  ).run(a.id, a.botId, a.kind ?? "tool", a.tool, a.summary, a.detail, a.rule, a.payload ? JSON.stringify(a.payload) : null, a.messageId, nowIso());
  return getApproval(db, a.id);
}

export function getApproval(db: Db, id: string): Approval {
  const row = db.prepare("SELECT * FROM approvals WHERE id = ?").get(id) as ApprovalRow | undefined;
  if (!row) throw new RpcError("not_found", "No such approval");
  return toApproval(row);
}

export function resolveApproval(db: Db, id: string, decision: ApprovalDecision): Approval {
  const approval = getApproval(db, id);
  if (approval.status !== "pending") throw new RpcError("already_answered", "This request was already answered");
  db.prepare("UPDATE approvals SET status = ?, decided_at = ? WHERE id = ?").run(decision === "deny" ? "denied" : "allowed", nowIso(), id);
  if (decision === "always" && approval.rule) db.prepare("INSERT OR IGNORE INTO allow_rules (bot_id, rule) VALUES (?, ?)").run(approval.botId, approval.rule);
  return getApproval(db, id);
}

export function allowRules(db: Db, botId: string): string[] {
  return (db.prepare("SELECT rule FROM allow_rules WHERE bot_id = ? ORDER BY rule").all(botId) as { rule: string }[]).map((row) => row.rule);
}

// After a restart the CLI that asked is gone, so nothing can answer its requests any more.
// Routine proposals don't depend on a CLI, so they stay pending.
export function expirePendingApprovals(db: Db): Approval[] {
  const ids = (db.prepare("SELECT id FROM approvals WHERE status = 'pending' AND kind = 'tool' ORDER BY created_at, rowid").all() as { id: string }[]).map((row) => row.id);
  db.prepare("UPDATE approvals SET status = 'denied', decided_at = ? WHERE status = 'pending' AND kind = 'tool'").run(nowIso());
  return ids.map((id) => getApproval(db, id));
}
