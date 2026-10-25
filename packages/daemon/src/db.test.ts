import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { describe, expect, it } from "vitest";
import { migrations, openDb } from "./db.ts";
import { dataPaths, readOrCreateToken } from "./paths.ts";

// A root with a space and an accented letter, like a real Windows profile path.
const root = () => mkdtempSync(join(tmpdir(), "tjommie test ñ-"));

describe("openDb", () => {
  it("upgrades a version 6 database with Everyone and empty room memberships", () => {
    const file = join(root(), "v6.db");
    const old = new DatabaseSync(file);
    for (const migration of migrations.slice(0, 6)) old.exec(migration);
    old.exec("PRAGMA user_version = 6");
    old.close();
    const db = openDb(file);
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: migrations.length });
    expect(db.prepare("SELECT name, created_at FROM conversations WHERE id = 'team'").get()).toEqual({ name: "Everyone", created_at: null });
    expect(db.prepare("SELECT * FROM room_members").all()).toEqual([]);
    db.close();
  });

  it("adds a nullable push token to the previous device schema without changing devices", () => {
    const file = join(root(), "v7.db");
    const old = new DatabaseSync(file);
    for (const migration of migrations.slice(0, 7)) old.exec(migration);
    old.exec(`INSERT INTO devices(id,name,public_key,paired_at) VALUES('phone','Phone','key','2026-10-25T00:00:00.000Z'); PRAGMA user_version = 7;`);
    old.close();
    const db = openDb(file);
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: migrations.length });
    expect(db.prepare("SELECT id, push_token FROM devices WHERE id = 'phone'").get()).toEqual({ id: "phone", push_token: null });
    db.close();
  });

  it("migrates version 4 accounts, pins, sessions and historical usage without moving identities", () => {
    const file = join(root(), "v4.db");
    const old = new DatabaseSync(file);
    for (const migration of migrations.slice(0, 4)) old.exec(migration);
    old.exec(`INSERT INTO accounts VALUES ('claude-main', 'claude', 'Original', 'subscription', 'unchanged/main', '2026-01-01');
      INSERT INTO accounts VALUES ('claude-second', 'claude', 'Second', 'subscription', 'unchanged/second', '2026-02-01');
      INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
        VALUES ('saved', 'Saved', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'x', 'then'),
        ('unsaved', 'Unsaved', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'y', 'then');
      INSERT INTO sessions VALUES ('saved', 'claude', 'claude-second', 'retained-session', 'then', 1.25);
      INSERT INTO usage VALUES ('known', 'saved', 'claude-main', 'claude', 'haiku', 10, 2, 3, 0, 0.5, 'reported', '2026-10-22T12:00:00Z');
      INSERT INTO usage VALUES ('unknown', 'deleted-bot', 'missing-account', 'codex', 'm', 1, 0, 2, 1, NULL, NULL, '2026-10-22T13:00:00Z');
      PRAGMA user_version = 4;`);
    old.close();
    const db = openDb(file);
    try {
      expect(db.prepare("SELECT id, folder, is_default FROM accounts ORDER BY id").all()).toEqual([
        { id: "claude-main", folder: "unchanged/main", is_default: 1 }, { id: "claude-second", folder: "unchanged/second", is_default: 0 },
      ]);
      expect(db.prepare("SELECT id, account_id FROM bots ORDER BY id").all()).toEqual([
        { id: "saved", account_id: "claude-second" }, { id: "unsaved", account_id: "claude-main" },
      ]);
      expect(db.prepare("SELECT * FROM sessions").get()).toEqual({ bot_id: "saved", provider: "claude", account_id: "claude-second", cli_session_id: "retained-session", updated_at: "then", total_cost_usd: 1.25 });
      db.prepare("INSERT INTO sessions VALUES ('saved', 'claude', 'claude-main', 'other-session', 'then', NULL)").run();
      expect(db.prepare("SELECT COUNT(*) AS n FROM sessions").get()).toEqual({ n: 2 });
      expect(db.prepare("SELECT id, account_kind, cost_usd, cost_source, pricing_revision FROM usage ORDER BY id").all()).toEqual([
        { id: "known", account_kind: "subscription", cost_usd: 0.5, cost_source: "reported", pricing_revision: null },
        { id: "unknown", account_kind: "unknown", cost_usd: null, cost_source: "unknown", pricing_revision: null },
      ]);
      db.prepare("DELETE FROM bots WHERE id = 'saved'").run();
      expect(db.prepare("SELECT COUNT(*) AS n FROM sessions").get()).toEqual({ n: 0 });
      expect(db.prepare("SELECT COUNT(*) AS n FROM usage").get()).toEqual({ n: 2 });
    } finally { db.close(); }
    const again = openDb(file);
    expect(again.prepare("SELECT COUNT(*) AS n FROM accounts").get()).toEqual({ n: 2 });
    again.close();
  });

  it("upgrades version 3 without replaying historical failures", () => {
    const file = join(root(), "v3.db");
    const old = new DatabaseSync(file);
    for (const migration of migrations.slice(0, 3)) old.exec(migration);
    old.exec(`INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
      VALUES ('bot', 'Writer', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'x', 'then');
      INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, created_at)
      VALUES ('routine', 'bot', 'Check', 'Check', '[1]', '["08:00"]', 'UTC', 'then');
      INSERT INTO routine_runs (id, routine_id, due_at, ended_at, status, note)
      VALUES ('run', 'routine', 'then', 'then', 'failed', 'Historical failure');
      PRAGMA user_version = 3;`);
    old.close();
    const db = openDb(file);
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: migrations.length });
    expect(db.prepare("SELECT status, note FROM routine_runs WHERE id = 'run'").get()).toEqual({ status: "failed", note: "Historical failure" });
    expect(db.prepare("SELECT * FROM routine_failure_notifications").all()).toEqual([]);
    expect(() => db.prepare("INSERT INTO routine_failure_notifications VALUES ('missing', 'Failure', 'Restarted')").run()).toThrow(/FOREIGN KEY/);
    db.close();
  });

  it("creates the schema once and keeps data when reopened", () => {
    const file = join(root(), "t.db");
    const db = openDb(file);
    db.prepare("INSERT INTO accounts (id, provider, label, kind, folder, created_at) VALUES ('a', 'claude', 'Claude', 'subscription', 'x', 'now')").run();
    db.close();
    const again = openDb(file);
    expect(again.prepare("SELECT label FROM accounts WHERE id = 'a'").get()).toEqual({ label: "Claude" });
    expect(again.prepare("PRAGMA user_version").get()).toEqual({ user_version: migrations.length });
  });

  it("has the Team room conversation and the routine tables after migration 2", () => {
    const db = openDb(join(root(), "t.db"));
    expect(db.prepare("SELECT kind FROM conversations WHERE id = 'team'").get()).toEqual({ kind: "room" });
    for (const table of ["settings", "chains", "exchanges", "routines", "routine_runs"]) {
      expect(db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?").get(table)).toEqual({ name: table });
    }
  });

  it("enforces foreign keys", () => {
    const db = openDb(join(root(), "t.db"));
    expect(() => db.prepare("INSERT INTO conversations (id, kind, bot_id) VALUES ('c', 'bot', 'missing')").run()).toThrow(/FOREIGN KEY/);
  });

  it("upgrades saved sessions with an unknown cost baseline", () => {
    const file = join(root(), "t.db");
    const old = new DatabaseSync(file);
    for (const migration of migrations.slice(0, 2)) old.exec(migration);
    old.exec(`
      PRAGMA foreign_keys = ON;
      INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
        VALUES ('bot', 'Writer', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'x', 'then');
      INSERT INTO sessions VALUES ('bot', 'claude', 'claude-main', 'saved', 'then');
      PRAGMA user_version = 2;
    `);
    old.close();

    const db = openDb(file);
    expect(db.prepare("SELECT cli_session_id, total_cost_usd FROM sessions WHERE bot_id = 'bot'").get()).toEqual({
      cli_session_id: "saved",
      total_cost_usd: null,
    });
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: migrations.length });
    db.close();
  });
});

describe("dataPaths", () => {
  it("creates the folders under a root with spaces and accents", () => {
    const p = dataPaths(root());
    for (const dir of [p.bots, p.accounts, p.logs]) expect(existsSync(dir)).toBe(true);
  });

  it("creates a token once and returns the same one afterwards", () => {
    const p = dataPaths(root());
    const token = readOrCreateToken(p.token);
    expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(readOrCreateToken(p.token)).toBe(token);
  });
});
