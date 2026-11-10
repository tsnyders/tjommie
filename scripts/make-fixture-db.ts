import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import { migrations, openDb } from "../packages/daemon/src/db.ts";

// The Rust daemon's fixture database test (spec §5 Phase 1):
// - fixture.db: a version 1 chat seeded as in database-upgrade.test.ts, upgraded by the TypeScript openDb, as 0.4.2 leaves it.
// - schema.json: sqlite_master of a fresh TypeScript database.
const fixturesDir = fileURLToPath(new URL("../tjommie-rs/crates/daemon/tests/fixtures", import.meta.url));
export const fixtureFile = join(fixturesDir, "fixture.db");
export const schemaFile = join(fixturesDir, "schema.json");

type SchemaObject = { type: string; name: string; tbl_name: string; sql: string | null };

function withTempDir<T>(work: (dir: string) => T): T {
  const dir = mkdtempSync(join(tmpdir(), "tjommie-fixture-"));
  try { return work(dir); }
  finally { rmSync(dir, { recursive: true, force: true }); }
}

export function freshSchema(): SchemaObject[] {
  return withTempDir((dir) => {
    const db = openDb(join(dir, "fresh.db"));
    try { return db.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name").all() as SchemaObject[]; }
    finally { db.close(); }
  });
}

function writeFixture(file: string): void {
  rmSync(file, { force: true });
  const home = "C:\\Users\\tjommie\\AppData\\Local\\Tjommie\\bots\\legacy-bot";
  const accountFolder = "C:\\Users\\tjommie\\AppData\\Local\\Tjommie\\accounts\\claude-main";
  // The seed from database-upgrade.test.ts: the original schema and only 1A columns, so openDb runs the real upgrade path.
  const legacy = new DatabaseSync(file);
  try {
    legacy.exec(migrations[0]);
    legacy.exec("PRAGMA foreign_keys = ON; PRAGMA user_version = 1;");
    legacy.prepare(`INSERT INTO accounts (id, provider, label, kind, folder, created_at)
      VALUES ('claude-main', 'claude', 'Claude', 'subscription', ?, '2026-09-25T10:00:00.000Z')`).run(accountFolder);
    legacy.prepare(`INSERT INTO bots
      (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
      VALUES ('legacy-bot', 'Writer', 'blue', 'circle', 'Be terse.', 'claude', 'haiku', 'folder', ?, '2026-09-25T10:00:00.000Z')`).run(home);
    legacy.exec(`
      INSERT INTO conversations (id, kind, bot_id) VALUES ('legacy-bot', 'bot', 'legacy-bot');
      INSERT INTO messages (id, conversation_id, author, kind, body, data, status, created_at, seq) VALUES
        ('running-message', 'legacy-bot', 'you', 'text', 'First request', NULL, 'sent', '2026-09-25T10:01:00.000Z', 1),
        ('approval-card', 'legacy-bot', 'legacy-bot', 'approval', 'Run a command', '{"approvalId":"legacy-approval","tool":"Bash","detail":"echo hello","rule":null,"status":"pending"}', 'done', '2026-09-25T10:02:00.000Z', 2),
        ('partial-reply', 'legacy-bot', 'legacy-bot', 'text', 'Unfinished answer', NULL, 'streaming', '2026-09-25T10:03:00.000Z', 3),
        ('queued-message', 'legacy-bot', 'you', 'text', 'Next request', NULL, 'queued', '2026-09-25T10:04:00.000Z', 4);
      INSERT INTO approvals (id, bot_id, tool, summary, detail, rule, status, message_id, created_at)
        VALUES ('legacy-approval', 'legacy-bot', 'Bash', 'Run a command', 'echo hello', NULL, 'pending', 'approval-card', '2026-09-25T10:02:00.000Z');
      INSERT INTO work_items (id, bot_id, source, text, message_id, status, created_at, started_at) VALUES
        ('running-work', 'legacy-bot', 'you', 'First request', 'running-message', 'running', '2026-09-25T10:01:00.000Z', '2026-09-25T10:01:01.000Z'),
        ('queued-work', 'legacy-bot', 'you', 'Next request', 'queued-message', 'queued', '2026-09-25T10:04:00.000Z', NULL);
      INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at)
        VALUES ('legacy-bot', 'claude', 'claude-main', 'saved-session', '2026-09-25T10:01:01.000Z');
    `);
  } finally {
    legacy.close();
  }
  // Closing the last connection checkpoints the WAL into the file, so fixture.db is complete on its own.
  openDb(file).close();
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  mkdirSync(fixturesDir, { recursive: true });
  writeFixture(fixtureFile);
  writeFileSync(schemaFile, `${JSON.stringify(freshSchema(), null, 2)}\n`);
}
