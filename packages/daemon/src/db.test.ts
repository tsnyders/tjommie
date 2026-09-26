import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { dataPaths, readOrCreateToken } from "./paths.ts";

// A root with a space and an accented letter, like a real Windows profile path.
const root = () => mkdtempSync(join(tmpdir(), "tjommie test ñ-"));

describe("openDb", () => {
  it("creates the schema once and keeps data when reopened", () => {
    const file = join(root(), "t.db");
    const db = openDb(file);
    db.prepare("INSERT INTO accounts (id, provider, label, kind, folder, created_at) VALUES ('a', 'claude', 'Claude', 'subscription', 'x', 'now')").run();
    db.close();
    const again = openDb(file);
    expect(again.prepare("SELECT label FROM accounts WHERE id = 'a'").get()).toEqual({ label: "Claude" });
    expect(again.prepare("PRAGMA user_version").get()).toEqual({ user_version: 3 });
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
    old.exec(`
      CREATE TABLE sessions (bot_id TEXT PRIMARY KEY, provider TEXT NOT NULL, account_id TEXT NOT NULL,
        cli_session_id TEXT NOT NULL, updated_at TEXT NOT NULL);
      INSERT INTO sessions VALUES ('bot', 'claude', 'claude-main', 'saved', 'then');
      PRAGMA user_version = 2;
    `);
    old.close();

    const db = openDb(file);
    expect(db.prepare("SELECT cli_session_id, total_cost_usd FROM sessions WHERE bot_id = 'bot'").get()).toEqual({
      cli_session_id: "saved",
      total_cost_usd: null,
    });
    expect(db.prepare("PRAGMA user_version").get()).toEqual({ user_version: 3 });
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
