import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

export type Db = DatabaseSync;
export const newId = (): string => randomUUID();
export const nowIso = (): string => new Date().toISOString();

// Append-only: never edit a shipped migration, add a new one.
const migrations = [
  `
  CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    kind TEXT NOT NULL,
    folder TEXT NOT NULL,
    created_at TEXT NOT NULL
  );
  CREATE TABLE bots (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    shape TEXT NOT NULL,
    instructions TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    autonomy TEXT NOT NULL,
    home_folder TEXT NOT NULL,
    project_folder TEXT,
    browser INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0,
    limited_until TEXT,
    created_at TEXT NOT NULL
  );
  CREATE TABLE bot_links (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    target_bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    PRIMARY KEY (bot_id, target_bot_id)
  );
  CREATE TABLE allow_rules (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    rule TEXT NOT NULL,
    PRIMARY KEY (bot_id, rule)
  );
  CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    bot_id TEXT REFERENCES bots(id) ON DELETE CASCADE
  );
  CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    author TEXT NOT NULL,
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    data TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    seq INTEGER NOT NULL
  );
  CREATE INDEX messages_by_conversation ON messages (conversation_id, seq);
  CREATE TABLE work_items (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    text TEXT NOT NULL,
    message_id TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT
  );
  CREATE INDEX work_items_by_bot ON work_items (bot_id, status);
  CREATE TABLE sessions (
    bot_id TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    account_id TEXT NOT NULL,
    cli_session_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
  );
  CREATE TABLE approvals (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    tool TEXT NOT NULL,
    summary TEXT NOT NULL,
    detail TEXT NOT NULL,
    rule TEXT,
    status TEXT NOT NULL,
    message_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    decided_at TEXT
  );
  CREATE TABLE usage (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input INTEGER NOT NULL,
    cached_input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    reasoning INTEGER NOT NULL,
    cost_usd REAL,
    cost_source TEXT,
    created_at TEXT NOT NULL
  );
  `,
  `
  CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
  CREATE TABLE chains (id TEXT PRIMARY KEY, requests_made INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL);
  CREATE TABLE exchanges (
    id TEXT PRIMARY KEY,
    from_bot_id TEXT NOT NULL,
    to_bot_id TEXT NOT NULL,
    request TEXT NOT NULL,
    reply TEXT,
    status TEXT NOT NULL,
    chain_id TEXT NOT NULL,
    reply_to TEXT NOT NULL,
    created_at TEXT NOT NULL,
    replied_at TEXT
  );
  CREATE TABLE routines (
    id TEXT PRIMARY KEY,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    days TEXT NOT NULL,
    times TEXT NOT NULL,
    time_zone TEXT NOT NULL,
    time_limit_minutes INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1,
    next_run_at TEXT,
    created_at TEXT NOT NULL
  );
  CREATE TABLE routine_runs (
    id TEXT PRIMARY KEY,
    routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE,
    due_at TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT NOT NULL,
    note TEXT
  );
  CREATE INDEX routine_runs_by_routine ON routine_runs (routine_id, due_at);
  ALTER TABLE work_items ADD COLUMN conversation_id TEXT;
  ALTER TABLE work_items ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
  ALTER TABLE work_items ADD COLUMN chain_id TEXT;
  ALTER TABLE work_items ADD COLUMN exchange_id TEXT;
  ALTER TABLE work_items ADD COLUMN routine_run_id TEXT;
  UPDATE work_items SET conversation_id = bot_id;
  ALTER TABLE approvals ADD COLUMN kind TEXT NOT NULL DEFAULT 'tool';
  ALTER TABLE approvals ADD COLUMN payload TEXT;
  INSERT INTO conversations (id, kind, bot_id) VALUES ('team', 'room', NULL);
  `,
];

export function transaction(db: Db, work: () => void): void {
  db.exec("BEGIN");
  try {
    work();
    db.exec("COMMIT");
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}

export function openDb(file: string): Db {
  const db = new DatabaseSync(file);
  db.exec("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
  const { user_version } = db.prepare("PRAGMA user_version").get() as { user_version: number };
  for (let version = user_version; version < migrations.length; version++) {
    transaction(db, () => {
      db.exec(migrations[version]);
      db.exec(`PRAGMA user_version = ${version + 1}`);
    });
  }
  return db;
}
