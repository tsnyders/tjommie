import { copyFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { expect, it } from "vitest";
import { addDevice, addMessage, createApproval, newId, nowIso, openSeedDb, recordUsage, replaceAccountLimits } from "./real-seed.ts";
import { tempRoot } from "./testing.ts";

// The fixture database is a 0.4.2 data root's schema (user_version 12) with one account, one bot and the Team room.
const fixture = resolve(import.meta.dirname, "..", "..", "..", "tjommie-rs", "crates", "daemon", "tests", "fixtures", "fixture.db");

function seeded() {
  const file = join(tempRoot(), "tjommie.db");
  copyFileSync(fixture, file);
  const db = openSeedDb(file);
  const account = db.prepare("SELECT id, provider, kind FROM accounts LIMIT 1").get() as { id: string; provider: string; kind: string };
  const bot = db.prepare("SELECT id FROM bots LIMIT 1").get() as { id: string };
  return { db, account, bot };
}

it("plants messages in a bot's chat and the Team room with a rising sequence, and an approval for one", () => {
  const { db, bot } = seeded();
  try {
    const first = addMessage(db, { conversationId: "team", author: bot.id, kind: "text", body: "one", status: "done" });
    const second = addMessage(db, { conversationId: "team", author: bot.id, kind: "text", body: "two", status: "done" });
    const sequence = db.prepare("SELECT seq FROM messages WHERE id IN (?, ?) ORDER BY seq").all(first.id, second.id) as { seq: number }[];
    expect(sequence[1].seq).toBe(sequence[0].seq + 1);
    const approvalId = newId();
    const card = addMessage(db, { conversationId: bot.id, author: "system", kind: "approval", body: "Allow?", data: { approvalId, status: "pending" }, status: "done" });
    createApproval(db, { id: approvalId, botId: bot.id, tool: "Read", summary: card.body, detail: "fixture-only.txt", rule: null, messageId: card.id });
    expect(db.prepare("SELECT status, kind, message_id FROM approvals WHERE id = ?").get(approvalId)).toMatchObject({ status: "pending", kind: "tool", message_id: card.id });
    expect(JSON.parse((db.prepare("SELECT data FROM messages WHERE id = ?").get(card.id) as { data: string }).data)).toMatchObject({ approvalId });
  } finally { db.close(); }
});

it("plants a usage row that carries the account's kind, and refuses an account that does not exist", () => {
  const { db, account, bot } = seeded();
  try {
    recordUsage(db, { botId: bot.id, accountId: account.id, provider: account.provider, model: "sonnet", input: 100, cachedInput: 10, output: 20, reasoning: 5, costUsd: 2.5, costSource: "reported" });
    expect(db.prepare("SELECT account_kind, cost_usd, cost_source, input FROM usage").all()).toEqual([expect.objectContaining({ account_kind: account.kind, cost_usd: 2.5, cost_source: "reported", input: 100 })]);
    expect(() => recordUsage(db, { botId: bot.id, accountId: "nobody", provider: "claude", model: "sonnet", input: 1, cachedInput: 0, output: 1, reasoning: 0, costUsd: null, costSource: "unknown" })).toThrow("existing account");
  } finally { db.close(); }
});

it("plants a device, an exchange with its conversation, and a quota snapshot whose reached window limits the account", () => {
  const { db, account, bot } = seeded();
  try {
    const device = addDevice(db, { name: "Disposable phone", publicKey: Buffer.alloc(32, 3).toString("base64url") });
    expect(db.prepare("SELECT name, revoked_at FROM devices WHERE id = ?").get(device.id)).toMatchObject({ name: "Disposable phone", revoked_at: null });
    // phone-layout.real.test.ts plants an exchange with its own conversation, by hand, with these statements.
    const exchangeId = newId();
    db.prepare("INSERT INTO exchanges (id,from_bot_id,to_bot_id,request,reply,status,chain_id,reply_to,created_at,replied_at) VALUES (?,?,?,?,?,'replied',?,?,?,?)")
      .run(exchangeId, bot.id, bot.id, "Review", "Looks ready", "chain", bot.id, nowIso(), nowIso());
    db.prepare("INSERT INTO conversations (id,kind,bot_id) VALUES (?,'exchange',NULL)").run(exchangeId);
    addMessage(db, { conversationId: exchangeId, author: bot.id, kind: "text", body: "Looks ready", status: "done" });

    const resetsAt = new Date(Date.now() + 3_600_000).toISOString();
    replaceAccountLimits(db, { accountId: account.id, limitId: account.provider, observedAt: nowIso(), windows: [
      { windowKey: "primary", durationMinutes: 300, usedPercent: 100, resetsAt, state: "reached" },
      { windowKey: "secondary", durationMinutes: 10080, usedPercent: 5, resetsAt: null, state: "ok" },
    ] });
    expect(db.prepare("SELECT window_key, state FROM account_limit_windows ORDER BY window_key").all()).toEqual([
      expect.objectContaining({ window_key: "primary", state: "reached" }), expect.objectContaining({ window_key: "secondary", state: "ok" }),
    ]);
    expect(db.prepare("SELECT limited_until FROM accounts WHERE id = ?").get(account.id)).toEqual({ limited_until: resetsAt });
    // A second snapshot replaces the first, and with nothing reached the account is no longer limited.
    replaceAccountLimits(db, { accountId: account.id, limitId: account.provider, observedAt: nowIso(), windows: [{ windowKey: "primary", durationMinutes: 300, usedPercent: 8, resetsAt, state: "ok" }] });
    expect(db.prepare("SELECT count(*) AS total FROM account_limit_windows").get()).toEqual({ total: 1 });
    expect(db.prepare("SELECT limited_until FROM accounts WHERE id = ?").get(account.id)).toEqual({ limited_until: null });
  } finally { db.close(); }
});
