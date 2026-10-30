import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

export type Db = DatabaseSync;
export const newId = (): string => randomUUID();
export const nowIso = (): string => new Date().toISOString();

// Append-only: never edit a shipped migration, add a new one.
export const migrations: readonly string[] = [
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
  `ALTER TABLE sessions ADD COLUMN total_cost_usd REAL;`,
  `CREATE TABLE routine_failure_notifications (
    run_id TEXT PRIMARY KEY REFERENCES routine_runs(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body TEXT NOT NULL
  );`,
  `ALTER TABLE accounts ADD COLUMN credential_ref TEXT;
  ALTER TABLE accounts ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1));
  ALTER TABLE accounts ADD COLUMN is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1));
  ALTER TABLE accounts ADD COLUMN spending_cap_usd REAL CHECK (spending_cap_usd IS NULL OR spending_cap_usd > 0);
  ALTER TABLE accounts ADD COLUMN limited_until TEXT;
  ALTER TABLE accounts ADD COLUMN deleted_at TEXT;
  ALTER TABLE accounts ADD COLUMN cap_revision INTEGER NOT NULL DEFAULT 0;
  UPDATE accounts SET is_default = 1 WHERE id = (SELECT a.id FROM accounts a WHERE a.provider = accounts.provider ORDER BY a.created_at, a.id LIMIT 1);
  CREATE UNIQUE INDEX accounts_default ON accounts(provider) WHERE is_default = 1 AND deleted_at IS NULL;
  CREATE UNIQUE INDEX accounts_fallback ON accounts(provider) WHERE is_fallback = 1 AND deleted_at IS NULL;
  ALTER TABLE bots ADD COLUMN account_id TEXT REFERENCES accounts(id);
  UPDATE bots SET account_id = COALESCE(
    (SELECT a.id FROM sessions s JOIN accounts a ON a.id = s.account_id WHERE s.bot_id = bots.id AND s.provider = bots.provider AND a.provider = bots.provider),
    (SELECT a.id FROM accounts a WHERE a.provider = bots.provider ORDER BY a.created_at, a.id LIMIT 1));
  UPDATE accounts SET limited_until = (SELECT MAX(b.limited_until) FROM bots b WHERE b.account_id = accounts.id);
  CREATE TABLE sessions_v5 (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    provider TEXT NOT NULL, account_id TEXT NOT NULL, cli_session_id TEXT NOT NULL,
    updated_at TEXT NOT NULL, total_cost_usd REAL,
    PRIMARY KEY (bot_id, provider, account_id)
  );
  INSERT INTO sessions_v5 SELECT bot_id, provider, account_id, cli_session_id, updated_at, total_cost_usd FROM sessions;
  DROP TABLE sessions;
  ALTER TABLE sessions_v5 RENAME TO sessions;
  ALTER TABLE usage ADD COLUMN account_kind TEXT NOT NULL DEFAULT 'unknown' CHECK (account_kind IN ('subscription', 'api_key', 'unknown'));
  ALTER TABLE usage ADD COLUMN pricing_revision TEXT;
  UPDATE usage SET account_kind = COALESCE((SELECT kind FROM accounts WHERE id = usage.account_id AND kind IN ('subscription', 'api_key')), 'unknown');
  UPDATE usage SET cost_source = 'unknown' WHERE cost_source IS NULL;
  CREATE INDEX usage_by_date ON usage(created_at);
  CREATE INDEX usage_by_bot_date ON usage(bot_id, created_at);
  CREATE INDEX usage_by_account_date ON usage(account_id, created_at);
  CREATE INDEX usage_by_model_date ON usage(model, created_at);
  CREATE TABLE account_limit_snapshots (
    account_id TEXT NOT NULL REFERENCES accounts(id), limit_id TEXT NOT NULL, observed_at TEXT NOT NULL,
    PRIMARY KEY (account_id, limit_id)
  );
  CREATE TABLE account_limit_windows (
    account_id TEXT NOT NULL REFERENCES accounts(id), limit_id TEXT NOT NULL, window_key TEXT NOT NULL,
    duration_minutes REAL, used_percent REAL, resets_at TEXT,
    state TEXT NOT NULL CHECK (state IN ('ok', 'near', 'reached')), observed_at TEXT NOT NULL,
    PRIMARY KEY (account_id, limit_id, window_key)
  );
  CREATE TABLE account_cap_notifications (
    account_id TEXT NOT NULL REFERENCES accounts(id), cap_revision INTEGER NOT NULL,
    threshold INTEGER NOT NULL CHECK (threshold IN (80, 100)), created_at TEXT NOT NULL,
    PRIMARY KEY (account_id, cap_revision, threshold)
  );`,
  `CREATE TABLE devices (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, public_key TEXT NOT NULL UNIQUE,
    paired_at TEXT NOT NULL, last_seen_at TEXT, revoked_at TEXT
  );
  CREATE TABLE remote_host (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1), id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL, public_key TEXT NOT NULL, created_at TEXT NOT NULL
  );
  CREATE TABLE pairing_codes (
    id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL, expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL, used_at TEXT
  );`,
  `ALTER TABLE conversations ADD COLUMN name TEXT;
  ALTER TABLE conversations ADD COLUMN created_at TEXT;
  UPDATE conversations SET name = 'Everyone' WHERE id = 'team';
  CREATE TABLE room_members (
    room_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, bot_id));`,
  `ALTER TABLE devices ADD COLUMN push_token TEXT;`,
  `ALTER TABLE devices ADD COLUMN push_kinds TEXT;`,
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
  // This lookup index also belongs on existing version 3 databases without changing stored data.
  db.exec("CREATE INDEX IF NOT EXISTS approvals_pending_routine_by_bot ON approvals (bot_id) WHERE kind = 'routine' AND status = 'pending';");
  return db;
}
