ALTER TABLE accounts ADD COLUMN credential_ref TEXT;
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
  );