-- Test-only: the TypeScript database schema after all migrations, every table and index from lane A's fixture
-- tests/fixtures/schema.json (sqlite_master of a fresh db.ts database), plus the Team room the migrations insert.
-- When lane A's db.rs merges, test_db opens a real database with daemon::db::open and this file is deleted.
CREATE TABLE account_cap_notifications (
    account_id TEXT NOT NULL REFERENCES accounts(id), cap_revision INTEGER NOT NULL,
    threshold INTEGER NOT NULL CHECK (threshold IN (80, 100)), created_at TEXT NOT NULL,
    PRIMARY KEY (account_id, cap_revision, threshold)
  );
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
CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    kind TEXT NOT NULL,
    folder TEXT NOT NULL,
    created_at TEXT NOT NULL
  , credential_ref TEXT, is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)), is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)), spending_cap_usd REAL CHECK (spending_cap_usd IS NULL OR spending_cap_usd > 0), limited_until TEXT, deleted_at TEXT, cap_revision INTEGER NOT NULL DEFAULT 0);
CREATE TABLE allow_rules (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    rule TEXT NOT NULL,
    PRIMARY KEY (bot_id, rule)
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
  , kind TEXT NOT NULL DEFAULT 'tool', payload TEXT);
CREATE TABLE attachments (
    id TEXT PRIMARY KEY, host_id TEXT NOT NULL, storage_name TEXT NOT NULL UNIQUE,
    original_name TEXT NOT NULL, byte_length INTEGER NOT NULL CHECK(byte_length >= 0 AND byte_length <= 209715200),
    content_type TEXT NOT NULL, sha256 TEXT NOT NULL, created_at TEXT NOT NULL
  );
CREATE TABLE bot_links (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    target_bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    PRIMARY KEY (bot_id, target_bot_id)
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
  , account_id TEXT REFERENCES accounts(id));
CREATE TABLE chains (id TEXT PRIMARY KEY, requests_made INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL);
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    bot_id TEXT REFERENCES bots(id) ON DELETE CASCADE
  , name TEXT, created_at TEXT);
CREATE TABLE devices (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, public_key TEXT NOT NULL UNIQUE,
    paired_at TEXT NOT NULL, last_seen_at TEXT, revoked_at TEXT
  , push_token TEXT, push_kinds TEXT);
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
  , task_attempt_id TEXT, task_revision INTEGER, execution_context_key TEXT);
CREATE TABLE message_attachments (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE, attachment_index INTEGER NOT NULL CHECK(attachment_index >= 0),
    attachment_id TEXT NOT NULL REFERENCES attachments(id), PRIMARY KEY(message_id,attachment_index)
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
CREATE TABLE pairing_codes (
    id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL, expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL, used_at TEXT
  );
CREATE TABLE project_bot_leases (bot_id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, kind TEXT NOT NULL);
CREATE TABLE project_members (project_id TEXT NOT NULL REFERENCES projects(id), bot_id TEXT NOT NULL, active INTEGER NOT NULL CHECK(active IN (0,1)), PRIMARY KEY(project_id,bot_id));
CREATE TABLE project_repositories (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, common_dir TEXT NOT NULL UNIQUE, name TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE project_tasks (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), state TEXT NOT NULL, revision INTEGER NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));
CREATE TABLE projects (id TEXT PRIMARY KEY, repository_id TEXT NOT NULL REFERENCES project_repositories(id), state TEXT NOT NULL, revision INTEGER NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));
CREATE TABLE remote_host (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1), id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL, public_key TEXT NOT NULL, created_at TEXT NOT NULL
  );
CREATE TABLE room_members (
    room_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, bot_id));
CREATE TABLE routine_failure_notifications (
    run_id TEXT PRIMARY KEY REFERENCES routine_runs(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body TEXT NOT NULL
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
CREATE TABLE "sessions" (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    provider TEXT NOT NULL, account_id TEXT NOT NULL, cli_session_id TEXT NOT NULL,
    updated_at TEXT NOT NULL, total_cost_usd REAL, execution_context_key TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (bot_id, provider, account_id)
  );
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE task_attempts (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES project_tasks(id), ordinal INTEGER NOT NULL, state TEXT NOT NULL, bot_id TEXT NOT NULL, worktree_path TEXT NOT NULL UNIQUE, branch TEXT NOT NULL UNIQUE, data TEXT NOT NULL CHECK(json_valid(data)), UNIQUE(task_id,ordinal));
CREATE TABLE task_evaluations (id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL REFERENCES task_attempts(id), revision INTEGER NOT NULL, source_sha TEXT NOT NULL, kind TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));
CREATE TABLE task_integrations (id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL REFERENCES task_attempts(id), repository_id TEXT NOT NULL REFERENCES project_repositories(id), state TEXT NOT NULL, target_ref TEXT NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));
CREATE TABLE task_requests (id TEXT PRIMARY KEY, operation TEXT NOT NULL, task_id TEXT NOT NULL, result_id TEXT NOT NULL);
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
  , account_kind TEXT NOT NULL DEFAULT 'unknown' CHECK (account_kind IN ('subscription', 'api_key', 'unknown')), pricing_revision TEXT);
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
  , conversation_id TEXT, priority INTEGER NOT NULL DEFAULT 0, chain_id TEXT, exchange_id TEXT, routine_run_id TEXT, task_attempt_id TEXT, task_revision INTEGER, execution_context_key TEXT);
CREATE UNIQUE INDEX accounts_default ON accounts(provider) WHERE is_default = 1 AND deleted_at IS NULL;
CREATE UNIQUE INDEX accounts_fallback ON accounts(provider) WHERE is_fallback = 1 AND deleted_at IS NULL;
CREATE INDEX approvals_pending_routine_by_bot ON approvals (bot_id) WHERE kind = 'routine' AND status = 'pending';
CREATE INDEX message_attachments_by_attachment ON message_attachments(attachment_id);
CREATE INDEX messages_by_conversation ON messages (conversation_id, seq);
CREATE UNIQUE INDEX project_bot_binding ON project_members(bot_id) WHERE active=1;
CREATE INDEX project_tasks_by_project ON project_tasks(project_id,state);
CREATE INDEX routine_runs_by_routine ON routine_runs (routine_id, due_at);
CREATE UNIQUE INDEX task_active_bot ON task_attempts(bot_id) WHERE state IN ('provisioning','queued','running');
CREATE INDEX task_attempts_by_task ON task_attempts(task_id,ordinal);
CREATE INDEX task_evaluations_by_attempt ON task_evaluations(attempt_id,revision,source_sha,kind);
CREATE UNIQUE INDEX task_integration_target ON task_integrations(repository_id,target_ref) WHERE state IN ('preparing','checking','ready');
CREATE INDEX task_integrations_by_attempt ON task_integrations(attempt_id);
CREATE INDEX usage_by_account_date ON usage(account_id, created_at);
CREATE INDEX usage_by_bot_date ON usage(bot_id, created_at);
CREATE INDEX usage_by_date ON usage(created_at);
CREATE INDEX usage_by_model_date ON usage(model, created_at);
CREATE INDEX work_items_by_bot ON work_items (bot_id, status);
INSERT INTO conversations (id, kind, bot_id, name) VALUES ('team', 'room', NULL, 'Everyone');
