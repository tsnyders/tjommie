import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { expect, it } from "vitest";
import { getApproval } from "./approvals.ts";
import { getStoredBot } from "./bots.ts";
import { getMessage } from "./chat.ts";
import { migrations, openDb } from "./db.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { getWorkItem } from "./queue.ts";
import { Runner } from "./runner.ts";
import { tempRoot, waitFor } from "./testing.ts";

it("upgrades and recovers a version 1 chat before resuming its queued work", async () => {
  const root = tempRoot();
  const file = join(root, "legacy.db");
  const home = join(root, "bots", "legacy-bot");
  const accountFolder = join(root, "accounts", "claude-main");
  mkdirSync(home, { recursive: true });
  mkdirSync(accountFolder, { recursive: true });

  // Use the original schema and only 1A columns, so this exercises the real upgrade path.
  const legacy = new DatabaseSync(file);
  try {
    legacy.exec(migrations[0]);
    legacy.exec("PRAGMA foreign_keys = ON; PRAGMA user_version = 1;");
    legacy.prepare(`INSERT INTO accounts (id, provider, label, kind, folder, created_at)
      VALUES ('claude-main', 'claude', 'Claude', 'subscription', ?, '2026-09-25T10:00:00.000Z')`).run(accountFolder);
    legacy.prepare(`INSERT INTO bots
      (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
      VALUES ('legacy-bot', 'Writer', 'blue', 'circle', 'Be terse.', 'claude', 'haiku', 'folder', ?, '2026-09-25T10:00:00.000Z')`).run(home);
    legacy.exec(`
      INSERT INTO conversations (id, kind, bot_id) VALUES ('legacy-bot', 'bot', 'legacy-bot');
      INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES
        ('running-message', 'legacy-bot', 'you', 'text', 'First request', NULL, 'sent', '2026-09-25T10:01:00.000Z', 1),
        ('approval-card', 'legacy-bot', 'system', 'approval', 'Run a command', '{"approvalId":"legacy-approval","status":"pending"}', 'done', '2026-09-25T10:02:00.000Z', 2),
        ('partial-reply', 'legacy-bot', 'legacy-bot', 'text', 'Unfinished answer', NULL, 'streaming', '2026-09-25T10:03:00.000Z', 3),
        ('queued-message', 'legacy-bot', 'you', 'text', 'Next request', NULL, 'queued', '2026-09-25T10:04:00.000Z', 4);
      INSERT INTO approvals (id, bot_id, tool, summary, detail, rule, status, message_id, created_at)
        VALUES ('legacy-approval', 'legacy-bot', 'Bash', 'Run a command', 'echo hello', NULL, 'pending', 'approval-card', '2026-09-25T10:02:00.000Z');
      INSERT INTO work_items (id, bot_id, source, text, message_id, status, created_at, started_at) VALUES
        ('running-work', 'legacy-bot', 'you', 'First request', 'running-message', 'running', '2026-09-25T10:01:00.000Z', '2026-09-25T10:01:01.000Z'),
        ('queued-work', 'legacy-bot', 'you', 'Next request', 'queued-message', 'queued', '2026-09-25T10:04:00.000Z', NULL);
      INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at)
        VALUES ('legacy-bot', 'claude', 'claude-main', 'saved-session', '2026-09-25T10:01:01.000Z');
    `);
    expect(legacy.prepare("PRAGMA user_version").get()).toEqual({ user_version: 1 });
  } finally {
    legacy.close();
  }

  const db = openDb(file);
  const started: StartOptions[] = [];
  const sent: string[] = [];
  let emit: (event: ProviderEvent) => void = () => {};
  const session: ProviderSession = {
    start: async () => {},
    send: async (text) => { sent.push(text); },
    steer: async () => false,
    interrupt: async () => {},
    answer: () => {},
    close: () => {},
    pid: () => undefined,
    onEvent: (listener) => { emit = listener; },
  };
  const runner = new Runner({
    db,
    broadcast: () => {},
    isSignedIn: () => true,
    createSession: (_bot, options) => {
      started.push(options);
      return session;
    },
  });

  try {
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: 3 });
    expect(db.prepare("SELECT id, kind, bot_id FROM conversations ORDER BY id").all()).toEqual([
      { id: "legacy-bot", kind: "bot", bot_id: "legacy-bot" },
      { id: "team", kind: "room", bot_id: null },
    ]);
    expect(db.prepare(`SELECT id, conversation_id, priority, chain_id, exchange_id, routine_run_id
      FROM work_items ORDER BY created_at`).all()).toEqual([
      { id: "running-work", conversation_id: "legacy-bot", priority: 0, chain_id: null, exchange_id: null, routine_run_id: null },
      { id: "queued-work", conversation_id: "legacy-bot", priority: 0, chain_id: null, exchange_id: null, routine_run_id: null },
    ]);
    expect(getStoredBot(db, "legacy-bot")).toMatchObject({ name: "Writer", homeFolder: home });
    expect(getApproval(db, "legacy-approval")).toMatchObject({ kind: "tool", status: "pending", payload: null });
    expect(db.prepare("SELECT cli_session_id, total_cost_usd FROM sessions WHERE bot_id = 'legacy-bot'").get()).toEqual({
      cli_session_id: "saved-session", total_cost_usd: null,
    });

    runner.recover();
    expect(started).toEqual([]);
    expect(getWorkItem(db, "running-work").status).toBe("interrupted");
    expect(getWorkItem(db, "queued-work").status).toBe("queued");
    expect(getMessage(db, "running-message").status).toBe("interrupted");
    expect(getMessage(db, "partial-reply")).toMatchObject({ body: "Unfinished answer", status: "interrupted" });
    expect(getApproval(db, "legacy-approval")).toMatchObject({ kind: "tool", status: "denied", payload: null });
    expect(getMessage(db, "approval-card").data).toEqual({ approvalId: "legacy-approval", status: "denied" });
    const notices = db.prepare("SELECT id FROM messages WHERE conversation_id = 'legacy-bot' AND kind = 'error'").all() as { id: string }[];
    expect(notices).toHaveLength(1);
    expect(getMessage(db, notices[0].id)).toMatchObject({
      body: "Tjommie restarted while this was running.",
      data: { retryMessageId: "running-message" },
    });

    runner.start();
    await waitFor(() => sent.length === 1, 2_000);
    expect(sent).toEqual(["Next request"]);
    expect(started).toHaveLength(1);
    expect(started[0]).toMatchObject({ cwd: home, resumeId: "saved-session", resumeCostUsd: null });
    expect(runner.currentWork("legacy-bot")?.id).toBe("queued-work");
    expect(getWorkItem(db, "queued-work")).toMatchObject({ status: "running", conversationId: "legacy-bot" });
    expect(getWorkItem(db, "running-work").status).toBe("interrupted");
    expect(getMessage(db, "queued-message").status).toBe("sent");
    emit({ type: "turn_done", ok: true, error: null });
    expect(getWorkItem(db, "queued-work").status).toBe("done");
  } finally {
    runner.shutdown();
    db.close();
  }
});

it("creates the pending routine approval index on an existing version 3 database", () => {
  const file = join(tempRoot(), "existing.db");
  const existing = new DatabaseSync(file);
  try {
    for (const migration of migrations) existing.exec(migration);
    existing.exec("PRAGMA user_version = 3;");
  } finally {
    existing.close();
  }

  const db = openDb(file);
  try {
    const plan = db.prepare(`EXPLAIN QUERY PLAN SELECT id FROM approvals
      WHERE bot_id = ? AND kind = 'routine' AND status = 'pending' LIMIT 1`).all("bot");
    expect(plan).toEqual(expect.arrayContaining([
      expect.objectContaining({ detail: expect.stringContaining("USING INDEX approvals_pending_routine_by_bot") }),
    ]));
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: 3 });
  } finally {
    db.close();
  }
});
