-- Test-only: the tables T3.2 reads and writes, in their final shape after db.ts migrations 1-12, folded by hand.
-- When lane A's db.rs merges, test_db opens a real database with daemon::db::open and this file is deleted.
CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  label TEXT NOT NULL,
  kind TEXT NOT NULL,
  folder TEXT NOT NULL,
  created_at TEXT NOT NULL,
  credential_ref TEXT,
  is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
  is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
  spending_cap_usd REAL CHECK (spending_cap_usd IS NULL OR spending_cap_usd > 0),
  limited_until TEXT,
  deleted_at TEXT,
  cap_revision INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX accounts_default ON accounts(provider) WHERE is_default = 1 AND deleted_at IS NULL;
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
  created_at TEXT NOT NULL,
  account_id TEXT REFERENCES accounts(id)
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
  bot_id TEXT REFERENCES bots(id) ON DELETE CASCADE,
  name TEXT,
  created_at TEXT
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
  decided_at TEXT,
  kind TEXT NOT NULL DEFAULT 'tool',
  payload TEXT
);
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
