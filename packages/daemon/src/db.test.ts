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
