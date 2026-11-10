
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
  