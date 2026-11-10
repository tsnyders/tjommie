CREATE TABLE devices (
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
  );