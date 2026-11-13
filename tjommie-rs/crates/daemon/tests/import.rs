//! T1.4 `tjommie-daemon import --from <root>` (spec §4). Every source and target is a temp root; nothing here reads
//! or writes the real `%LOCALAPPDATA%\Tjommie`.

use std::fs;
use std::path::{Path, PathBuf};

use daemon::{import, spawn};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fixture.db");
const LOGIN: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
/// An account folder outside the source, on a drive that is usually missing: the import must neither move it nor fail.
const ELSEWHERE: &str = r"Q:\Elsewhere\codex";

/// One row in every project-team table, a queued task turn and a waiting task exchange, plus a finished task turn.
const PROJECT_TEAM: &str = r"
INSERT INTO project_repositories VALUES ('repo', 'C:\Code\app', 'C:\Code\app\.git', 'app', 'now');
INSERT INTO projects VALUES ('project', 'repo', 'active', 1, '{}');
INSERT INTO project_members VALUES ('project', 'legacy-bot', 1);
INSERT INTO project_bot_leases VALUES ('legacy-bot', 'project', 'project');
INSERT INTO project_tasks VALUES ('task', 'project', 'running', 1, '{}');
INSERT INTO task_attempts VALUES ('attempt', 'task', 1, 'running', 'legacy-bot', 'C:\Code\app-worktrees\task', 'tjommie/task', '{}');
INSERT INTO task_evaluations VALUES ('evaluation', 'attempt', 1, 'abc', 'review', 'passed', '{}');
INSERT INTO task_integrations VALUES ('integration', 'attempt', 'repo', 'ready', 'main', '{}');
INSERT INTO task_requests VALUES ('request', 'start', 'task', 'attempt');
INSERT INTO work_items (id, bot_id, source, text, status, created_at, conversation_id, task_attempt_id, task_revision, execution_context_key)
  VALUES ('task-turn', 'legacy-bot', 'task', 'Build it', 'queued', 'now', 'legacy-bot', 'attempt', 1, 'key'),
         ('old-task-turn', 'legacy-bot', 'task', 'Built it', 'done', 'then', 'legacy-bot', 'attempt', 1, 'key');
INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, status, chain_id, reply_to, created_at, task_attempt_id, task_revision, execution_context_key)
  VALUES ('task-exchange', 'legacy-bot', 'legacy-bot', 'Check this', 'waiting', 'chain', 'legacy-bot', 'now', 'attempt', 1, 'key');
";

const PROJECT_TABLES: [&str; 9] = [
    "project_repositories",
    "projects",
    "project_members",
    "project_bot_leases",
    "project_tasks",
    "task_attempts",
    "task_evaluations",
    "task_integrations",
    "task_requests",
];

fn temp(prefix: &str) -> TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().unwrap()
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A 0.4.2 data root: the fixture database with a paired phone, a logins database, a bot folder and an account folder
/// that holds a Claude sign-in.
fn source() -> TempDir {
    let dir = temp("tjommie source ñ-");
    let root = dir.path();
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::copy(FIXTURE, data.join("tjommie.db")).unwrap();
    let home = root.join("bots").join("legacy-bot");
    let account = root.join("accounts").join("claude-main");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&account).unwrap();
    fs::write(home.join("notes.md"), "# Writer's notes\n").unwrap();
    fs::write(home.join(".credentials.json"), "{\"stray\":true}").unwrap();
    fs::write(account.join(".credentials.json"), "{\"claudeAiOauth\":{}}").unwrap();
    {
        let db = Connection::open(data.join("tjommie.db")).unwrap();
        db.execute(
            "UPDATE bots SET home_folder = ?1 WHERE id = 'legacy-bot'",
            [text(&home)],
        )
        .unwrap();
        db.execute(
            "UPDATE accounts SET folder = ?1 WHERE id = 'claude-main'",
            [text(&account)],
        )
        .unwrap();
        db.execute(
            "INSERT INTO accounts (id, provider, label, kind, folder, created_at) VALUES ('codex-elsewhere', 'codex', 'Codex', 'api_key', ?1, 'now')",
            [ELSEWHERE],
        )
        .unwrap();
        db.execute_batch(
            "INSERT INTO devices (id, name, public_key, paired_at) VALUES ('phone', 'Phone', 'key', 'now');
             INSERT INTO remote_host (singleton, id, name, public_key, created_at) VALUES (1, 'host', 'PC', 'key', 'now');
             INSERT INTO pairing_codes (id, secret_hash, expires_at, created_at) VALUES ('code', 'hash', 'later', 'now');",
        )
        .unwrap();
        db.execute_batch(PROJECT_TEAM).unwrap();
    }
    let logins = Connection::open(data.join("logins.sqlite")).unwrap();
    logins
        .execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE logins (id TEXT PRIMARY KEY, data TEXT NOT NULL, reference TEXT NOT NULL UNIQUE);
             CREATE TABLE login_deletions (reference TEXT PRIMARY KEY);",
        )
        .unwrap();
    logins
        .execute("INSERT INTO logins VALUES ('login-1', '{}', ?1)", [LOGIN])
        .unwrap();
    dir
}

/// The 8.3 spelling of an existing path, if the volume makes them.
#[cfg(windows)]
fn short_name(path: &Path) -> Option<PathBuf> {
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows::core::HSTRING;

    let mut buffer = [0u16; 1024];
    // SAFETY: the call writes at most the buffer's length into it.
    let length =
        unsafe { GetShortPathNameW(&HSTRING::from(path.as_os_str()), Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    let short = PathBuf::from(String::from_utf16(&buffer[..length]).ok()?);
    (short != path).then_some(short)
}

/// `C:\x` as the UNC loopback shares `\\localhost\c$\x`, `\\127.0.0.1\c$\x` and `\\?\UNC\localhost\c$\x`, which
/// `canonicalize` keeps in UNC form. Where the admin share is off they can't be opened, and the guards refuse that too.
#[cfg(windows)]
fn loopback_spellings(path: &Path) -> Vec<PathBuf> {
    let text = text(path);
    let (drive, rest) = text.split_once(":\\").unwrap();
    let share = format!("{}$\\{rest}", drive.to_lowercase());
    if !Path::new(&format!(r"\\localhost\{share}")).exists() {
        eprintln!(
            "the c$ admin share is off here: the loopback spellings are refused because they can't be opened"
        );
    }
    [r"\\localhost\", r"\\127.0.0.1\", r"\\?\UNC\localhost\"]
        .map(|host| PathBuf::from(format!("{host}{share}")))
        .to_vec()
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let made = spawn::command("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stdout)
    );
}

fn count(db: &Path, sql: &str) -> i64 {
    Connection::open(db)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

/// The sha256 of every path and file under `root`.
fn tree_hash(root: &Path) -> String {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.push((format!("{name}\\"), Vec::new()));
                walk(root, &path, out);
            } else {
                out.push((name, fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    let mut hash = Sha256::new();
    for (name, bytes) in files {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

fn files_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_named(&path, name));
        } else if path.file_name().is_some_and(|file| file == name) {
            found.push(path);
        }
    }
    found
}

#[test]
fn copies_the_databases_and_bots_without_changing_the_source() {
    let source = source();
    let target = temp("tjommie target ñ-");
    let before = tree_hash(source.path());

    import::copy_data(source.path(), target.path(), false).unwrap();

    assert_eq!(tree_hash(source.path()), before);
    let data = target.path().join("data");
    assert_eq!(
        count(&data.join("tjommie.db"), "SELECT COUNT(*) FROM messages"),
        4
    );
    assert_eq!(
        count(&data.join("logins.sqlite"), "SELECT COUNT(*) FROM logins"),
        1
    );
    let notes = fs::read_to_string(
        target
            .path()
            .join("bots")
            .join("legacy-bot")
            .join("notes.md"),
    )
    .unwrap();
    assert_eq!(notes, "# Writer's notes\n");
}

/// After a crash the source can hold rows that are only in its WAL. The copy reads them, and the source's database and
/// WAL stay as they were: nothing checkpoints into the other install.
#[test]
fn copies_an_unmerged_wal_without_merging_it_into_the_source() {
    let source = source();
    let db = source.path().join("data").join("tjommie.db");
    let wal = source.path().join("data").join("tjommie.db-wal");
    let live = temp("tjommie live-");
    fs::copy(&db, live.path().join("tjommie.db")).unwrap();
    let writer = Connection::open(live.path().join("tjommie.db")).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint = 0; INSERT INTO settings (key, value) VALUES ('crashed', 'yes');")
        .unwrap();
    fs::copy(live.path().join("tjommie.db"), &db).unwrap();
    fs::copy(live.path().join("tjommie.db-wal"), &wal).unwrap();
    drop(writer);
    // A WAL without its -shm: the copy must not leave a new -shm behind either.
    assert!(!source.path().join("data").join("tjommie.db-shm").exists());
    let before = tree_hash(source.path());

    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();

    assert_eq!(
        tree_hash(source.path()),
        before,
        "every file and folder in the source is as it was"
    );
    let copy = target.path().join("data").join("tjommie.db");
    assert_eq!(
        count(&copy, "SELECT COUNT(*) FROM settings WHERE key = 'crashed'"),
        1
    );
}

#[test]
fn clears_devices_remote_host_and_pairing_codes() {
    let source = source();
    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();
    let db = target.path().join("data").join("tjommie.db");
    for table in ["devices", "remote_host", "pairing_codes"] {
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    let source_db = source.path().join("data").join("tjommie.db");
    assert_eq!(count(&source_db, "SELECT COUNT(*) FROM devices"), 1);
}

/// Project teams point at git repositories and worktrees outside the root, so a copy would work in production's.
#[test]
fn leaves_project_teams_behind() {
    let source = source();
    let target = temp("tjommie target ñ-");
    let before = tree_hash(source.path());
    let source_db = source.path().join("data").join("tjommie.db");
    for table in PROJECT_TABLES {
        assert_eq!(
            count(&source_db, &format!("SELECT COUNT(*) FROM {table}")),
            1,
            "{table}"
        );
    }

    import::copy_data(source.path(), target.path(), false).unwrap();

    assert_eq!(tree_hash(source.path()), before);
    let db = target.path().join("data").join("tjommie.db");
    for table in PROJECT_TABLES {
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    let broken: i64 = Connection::open(&db)
        .unwrap()
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count() as i64;
    assert_eq!(broken, 0);
    // Live task work would run against a missing attempt, so it ends as failed; finished history stays as it was.
    let status = |sql: &str| -> String {
        Connection::open(&db)
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(
        status("SELECT status FROM work_items WHERE id = 'task-turn'"),
        "failed"
    );
    assert_eq!(
        status("SELECT status FROM work_items WHERE id = 'old-task-turn'"),
        "done"
    );
    assert_eq!(
        status("SELECT status FROM exchanges WHERE id = 'task-exchange'"),
        "failed"
    );
}

/// Run as a command, the import says that project teams stayed behind. It reads Credential Manager (this source's
/// namespace is new and empty, so nothing is written), which hosted CI can't reach.
#[test]
fn says_that_project_teams_were_not_imported() {
    if std::env::var("TJOMMIE_CI_HOSTED").as_deref() == Ok("1") {
        eprintln!(
            "skipped: hosted Windows service logons cannot reliably reach Credential Manager"
        );
        return;
    }
    let source = source();
    let target = temp("tjommie target ñ-");
    let imported = spawn::command(env!("CARGO_BIN_EXE_tjommie-daemon"))
        .args(["import", "--from", text(source.path())])
        .env("TJOMMIE_HOME", target.path())
        .output()
        .unwrap();
    let (out, err) = (
        String::from_utf8_lossy(&imported.stdout),
        String::from_utf8_lossy(&imported.stderr),
    );
    assert!(imported.status.success(), "{err}");
    assert!(out.contains("Project teams weren't imported"), "{out}");
    assert!(out.contains("0 account keys and 0 logins"), "{out}");
    assert!(out.contains("browser starts signed out"), "{out}");
    assert!(out.contains("Routines are paused"), "{out}");
}

/// A bot's browser profile holds signed-in web sessions (and is locked while that browser runs), so it stays behind.
#[cfg(windows)]
#[test]
fn leaves_browser_profiles_behind() {
    use std::os::windows::fs::OpenOptionsExt;

    let source = source();
    let profile = source
        .path()
        .join("bots")
        .join("legacy-bot")
        .join("chrome-profile");
    fs::create_dir_all(profile.join("Default")).unwrap();
    fs::write(profile.join("Default").join("Cookies"), "session").unwrap();
    let in_use = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(profile.join("Default").join("Cookies"))
        .unwrap();
    let target = temp("tjommie target ñ-");

    import::copy_data(source.path(), target.path(), false).unwrap();
    drop(in_use);

    let bot = target.path().join("bots").join("legacy-bot");
    assert!(bot.join("notes.md").is_file());
    assert!(!bot.join("chrome-profile").exists());
}

/// A file that can't be copied is named in the error.
#[cfg(windows)]
#[test]
fn names_a_file_it_cannot_copy() {
    use std::os::windows::fs::OpenOptionsExt;

    let source = source();
    let notes = source
        .path()
        .join("bots")
        .join("legacy-bot")
        .join("notes.md");
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&notes)
        .unwrap();
    let target = temp("tjommie target ñ-");
    let error = import::copy_data(source.path(), target.path(), false).unwrap_err();
    drop(locked);
    assert!(error.to_string().contains(text(&notes)), "{error}");
}

/// The copy would otherwise run alongside the source install: routines would fire twice and queued turns (paid ones)
/// would run in both. The fixture holds a running and a queued chat turn.
#[test]
fn pauses_routines_and_fails_live_work() {
    let source = source();
    let source_db = source.path().join("data").join("tjommie.db");
    Connection::open(&source_db)
        .unwrap()
        .execute_batch(
            r#"INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, enabled, created_at)
                 VALUES ('daily', 'legacy-bot', 'Daily', 'Check', '[1]', '["08:00"]', 'UTC', 1, 'then');
               INSERT INTO work_items (id, bot_id, source, text, status, created_at, conversation_id)
                 VALUES ('old-turn', 'legacy-bot', 'you', 'Hello', 'done', 'then', 'legacy-bot');"#,
        )
        .unwrap();
    let target = temp("tjommie target ñ-");

    import::copy_data(source.path(), target.path(), false).unwrap();

    let db = target.path().join("data").join("tjommie.db");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM routines"), 1);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM routines WHERE enabled = 1"),
        0
    );
    let failed = "SELECT COUNT(*) FROM work_items WHERE id IN ('running-work', 'queued-work') AND status = 'failed' AND ended_at IS NOT NULL";
    assert_eq!(count(&db, failed), 2);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM work_items WHERE status IN ('queued', 'running')"
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM work_items WHERE id = 'old-turn' AND status = 'done'"
        ),
        1
    );
    assert_eq!(
        count(
            &source_db,
            "SELECT COUNT(*) FROM routines WHERE enabled = 1"
        ),
        1
    );
}

/// A failed reply turn fails its exchange too (`team.ts:399-400`), or the asking bot would wait forever.
#[test]
fn fails_waiting_exchanges_with_the_turns_that_would_answer_them() {
    let source = source();
    Connection::open(source.path().join("data").join("tjommie.db"))
        .unwrap()
        .execute_batch(
            "INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, status, chain_id, reply_to, created_at)
               VALUES ('question', 'legacy-bot', 'legacy-bot', 'What next?', 'waiting', 'chain', 'legacy-bot', 'now');
             INSERT INTO work_items (id, bot_id, source, text, status, created_at, conversation_id, exchange_id)
               VALUES ('answer', 'legacy-bot', 'bot_request', 'What next?', 'queued', 'now', 'legacy-bot', 'question');",
        )
        .unwrap();
    let target = temp("tjommie target ñ-");

    import::copy_data(source.path(), target.path(), false).unwrap();

    let db = target.path().join("data").join("tjommie.db");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM work_items WHERE id = 'answer' AND status = 'failed'"
        ),
        1
    );
    let exchange: (String, String) = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT status, reply FROM exchanges WHERE id = 'question'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        exchange,
        ("failed".into(), "Imported while waiting.".into())
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM exchanges WHERE status = 'waiting'"
        ),
        0
    );
}

/// A dev daemon holding the target database stops the swap; the error names the file and what to close.
#[cfg(windows)]
#[test]
fn names_the_target_database_when_it_is_in_use() {
    let source = source();
    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();
    let db = target.path().join("data").join("tjommie.db");
    let dev = Connection::open(&db).unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM messages"), 4);
    dev.query_row("SELECT COUNT(*) FROM bots", [], |row| row.get::<_, i64>(0))
        .unwrap();

    let error = import::copy_data(source.path(), target.path(), true).unwrap_err();
    drop(dev);

    let message = error.to_string();
    assert!(message.contains(text(&db)), "{message}");
    assert!(message.contains("Close"), "{message}");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM messages"), 4);
}

#[test]
fn copies_no_credentials_json() {
    let source = source();
    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();
    assert_eq!(
        files_named(target.path(), ".credentials.json"),
        Vec::<PathBuf>::new()
    );
}

/// The daemon uses the stored folders as they are, so copies that pointed into the source root would make the new
/// install write there and share its Claude sign-in.
#[test]
fn points_bot_and_account_folders_under_the_source_root_at_the_target() {
    let source = source();
    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();
    let db = Connection::open(target.path().join("data").join("tjommie.db")).unwrap();
    let folder = |sql: &str| -> String { db.query_row(sql, [], |row| row.get(0)).unwrap() };
    let home = target.path().join("bots").join("legacy-bot");
    let account = target.path().join("accounts").join("claude-main");
    assert_eq!(
        folder("SELECT home_folder FROM bots WHERE id = 'legacy-bot'"),
        text(&home)
    );
    assert_eq!(
        folder("SELECT folder FROM accounts WHERE id = 'claude-main'"),
        text(&account)
    );
    assert_eq!(
        folder("SELECT folder FROM accounts WHERE id = 'codex-elsewhere'"),
        ELSEWHERE
    );
}

#[test]
fn refuses_an_existing_target_unless_replace_is_given() {
    let source = source();
    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();
    let db = target.path().join("data").join("tjommie.db");
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO settings (key, value) VALUES ('dev-only', 'kept')",
            [],
        )
        .unwrap();

    let refused = spawn::command(env!("CARGO_BIN_EXE_tjommie-daemon"))
        .args(["import", "--from", text(source.path())])
        .env("TJOMMIE_HOME", target.path())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("--replace"), "{message}");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM settings WHERE key = 'dev-only'"),
        1
    );

    import::copy_data(source.path(), target.path(), true).unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM settings WHERE key = 'dev-only'"),
        0
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM messages"), 4);
}

/// Nothing of the previous import is removed until the new copy is complete, so a failed `--replace` keeps it.
#[cfg(windows)]
#[test]
fn a_failed_replace_keeps_the_previous_import() {
    use std::os::windows::fs::OpenOptionsExt;

    let source = source();
    let target = temp("tjommie target ñ-");
    import::copy_data(source.path(), target.path(), false).unwrap();
    let db = target.path().join("data").join("tjommie.db");
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO settings (key, value) VALUES ('dev-only', 'kept')",
            [],
        )
        .unwrap();
    let notes = source
        .path()
        .join("bots")
        .join("legacy-bot")
        .join("notes.md");
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&notes)
        .unwrap();

    assert!(import::copy_data(source.path(), target.path(), true).is_err());
    drop(locked);

    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM settings WHERE key = 'dev-only'"),
        1
    );
    assert!(
        target
            .path()
            .join("bots")
            .join("legacy-bot")
            .join("notes.md")
            .is_file()
    );
    import::copy_data(source.path(), target.path(), true).unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM settings WHERE key = 'dev-only'"),
        0
    );
    let mut left: Vec<String> = fs::read_dir(target.path().join("data"))
        .unwrap()
        .chain(fs::read_dir(target.path()).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("partial"))
        .collect();
    left.sort();
    assert_eq!(left, Vec::<String>::new());
}

/// `--replace` would delete a database, so the production root (`%LOCALAPPDATA%\Tjommie`, faked here) is never a target.
#[test]
fn refuses_the_production_root() {
    let source = source();
    let local = temp("tjommie local ñ-");
    let production = local.path().join("Tjommie");
    let refused = spawn::command(env!("CARGO_BIN_EXE_tjommie-daemon"))
        .args(["import", "--from", text(source.path()), "--replace"])
        .env("LOCALAPPDATA", local.path())
        .env("TJOMMIE_HOME", &production)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("production"), "{message}");
    assert!(!production.exists());
}

/// The guards compare what paths name, not how they are spelled: `\\?\` and a junction reach the production root too.
#[cfg(windows)]
#[test]
fn refuses_other_spellings_of_the_production_root() {
    let source = source();
    let local = temp("tjommie local ñ-");
    let production = local.path().join("Tjommie");
    let production_db = production.join("data").join("tjommie.db");
    fs::create_dir_all(production.join("data")).unwrap();
    fs::copy(FIXTURE, &production_db).unwrap();
    Connection::open(&production_db)
        .unwrap()
        .execute(
            "INSERT INTO settings (key, value) VALUES ('production-only', 'kept')",
            [],
        )
        .unwrap();
    let links = temp("tjommie links-");
    let link = links.path().join("production");
    junction(&link, &production);

    let verbatim = PathBuf::from(format!(r"\\?\{}", production.display()));
    let mut targets = vec![verbatim, link.clone()];
    targets.extend(loopback_spellings(&production));
    for target in targets {
        let refused = spawn::command(env!("CARGO_BIN_EXE_tjommie-daemon"))
            .args(["import", "--from", text(source.path()), "--replace"])
            .env("LOCALAPPDATA", local.path())
            .env("TJOMMIE_HOME", &target)
            .output()
            .unwrap();
        let message = String::from_utf8_lossy(&refused.stderr);
        assert!(!refused.status.success(), "{}", target.display());
        assert!(message.contains("production"), "{message}");
        assert_eq!(
            count(
                &production_db,
                "SELECT COUNT(*) FROM settings WHERE key = 'production-only'"
            ),
            1
        );
    }
    fs::remove_dir(&link).unwrap();
}

/// A junction to the source, its 8.3 spelling and a `\\?\` source spelling all name the source root.
#[cfg(windows)]
#[test]
fn refuses_other_spellings_of_the_source_root() {
    let source = source();
    let before = tree_hash(source.path());
    let links = temp("tjommie links-");
    let link = links.path().join("source");
    junction(&link, source.path());
    let verbatim = PathBuf::from(format!(r"\\?\{}", source.path().display()));
    let mut cases = vec![
        (source.path().to_path_buf(), link.clone()),
        (verbatim, source.path().join("inner")),
    ];
    for alias in loopback_spellings(source.path()) {
        cases.push((source.path().to_path_buf(), alias));
    }
    match short_name(source.path()) {
        Some(short) => cases.push((source.path().to_path_buf(), short.join("inner"))),
        None => eprintln!("this volume makes no 8.3 names: that spelling is not checked"),
    }
    for (from, to) in cases {
        let error = import::copy_data(&from, &to, true).unwrap_err();
        assert!(
            error.to_string().contains("source"),
            "{} -> {}: {error}",
            from.display(),
            to.display()
        );
    }
    fs::remove_dir(&link).unwrap();
    assert_eq!(tree_hash(source.path()), before);
}

/// Stored folders are matched by what they name, so a source given by its 8.3 spelling still moves them.
#[cfg(windows)]
#[test]
fn rebases_folders_when_the_source_is_spelled_with_short_names() {
    let source = source();
    let Some(short) = short_name(source.path()) else {
        eprintln!("skipped: this volume makes no 8.3 names");
        return;
    };
    let target = temp("tjommie target ñ-");
    import::copy_data(&short, target.path(), false).unwrap();
    let db = Connection::open(target.path().join("data").join("tjommie.db")).unwrap();
    let folder = |sql: &str| -> String { db.query_row(sql, [], |row| row.get(0)).unwrap() };
    let home = target.path().join("bots").join("legacy-bot");
    let account = target.path().join("accounts").join("claude-main");
    assert_eq!(
        folder("SELECT home_folder FROM bots WHERE id = 'legacy-bot'"),
        text(&home)
    );
    assert_eq!(
        folder("SELECT folder FROM accounts WHERE id = 'claude-main'"),
        text(&account)
    );
}

/// A bot folder that is a junction inside the source names a folder elsewhere, but the copy must still not open it
/// through the source: it moves by its spelling and, since links aren't copied, points at a folder that isn't there.
#[cfg(windows)]
#[test]
fn rebases_a_bot_folder_that_is_a_junction_inside_the_source() {
    let source = source();
    let elsewhere = temp("tjommie elsewhere-");
    let link = source.path().join("bots").join("linked-bot");
    junction(&link, elsewhere.path());
    Connection::open(source.path().join("data").join("tjommie.db"))
        .unwrap()
        .execute(
            "INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
             VALUES ('linked-bot', 'Linked', 'red', 'circle', '', 'claude', 'haiku', 'folder', ?1, 'now')",
            [text(&link)],
        )
        .unwrap();
    let target = temp("tjommie target ñ-");

    import::copy_data(source.path(), target.path(), false).unwrap();
    fs::remove_dir(&link).unwrap();

    let moved = target.path().join("bots").join("linked-bot");
    let home: String = Connection::open(target.path().join("data").join("tjommie.db"))
        .unwrap()
        .query_row(
            "SELECT home_folder FROM bots WHERE id = 'linked-bot'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(home, text(&moved));
    assert!(!moved.exists());
}

#[test]
fn never_imports_into_the_source_root() {
    let source = source();
    let before = tree_hash(source.path());
    for target in [source.path().to_path_buf(), source.path().join("inner")] {
        let error = import::copy_data(source.path(), &target, true).unwrap_err();
        assert!(error.to_string().contains("source"), "{error}");
    }
    assert_eq!(tree_hash(source.path()), before);
}

/// Credential Manager: one random namespace per root, both refused if they are production's, cleaned up by namespace,
/// and a failure if any other `Tjommie/*` target changed (the guard from `cred_roundtrip.rs`).
#[cfg(windows)]
#[test]
fn rewrites_account_and_login_secrets_under_the_target_namespace() {
    use daemon::win::cred::{self, Namespace};
    use std::collections::BTreeMap;
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{CREDENTIALW, CredEnumerateW, CredFree};
    use windows::core::HSTRING;

    if std::env::var("TJOMMIE_CI_HOSTED").as_deref() == Ok("1") {
        eprintln!(
            "skipped: hosted Windows service logons cannot reliably reach Credential Manager"
        );
        return;
    }
    fn enumerate(filter: &str) -> BTreeMap<String, u64> {
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: CredEnumerateW fills `count` and `list`; the list is freed once with CredFree.
        unsafe {
            if let Err(error) = CredEnumerateW(&HSTRING::from(filter), None, &mut count, &mut list)
            {
                assert_eq!(
                    error.code(),
                    ERROR_NOT_FOUND.to_hresult(),
                    "Credential Manager is unreachable"
                );
                return BTreeMap::new();
            }
            let found = std::slice::from_raw_parts(list, count as usize)
                .iter()
                .map(|&credential| {
                    let credential = &*credential;
                    let written = (u64::from(credential.LastWritten.dwHighDateTime) << 32)
                        | u64::from(credential.LastWritten.dwLowDateTime);
                    (credential.TargetName.to_string().unwrap(), written)
                })
                .collect();
            CredFree(list as *const _);
            found
        }
    }

    let source = source();
    let target = temp("tjommie target ñ-");
    let from = Namespace::new(source.path()).unwrap();
    let to = Namespace::new(target.path()).unwrap();
    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    let production = Namespace::new(&PathBuf::from(local).join("Tjommie")).unwrap();
    for ns in [&from, &to] {
        assert_ne!(
            ns.short(),
            production.short(),
            "refusing the production namespace"
        );
        assert_ne!(
            ns.long(),
            production.long(),
            "refusing the production namespace"
        );
    }
    let prefixes = [from.short(), from.long(), to.short(), to.long()]
        .map(|prefix| format!("Tjommie/{prefix}/"));
    let mine = |target: &str| prefixes.iter().any(|prefix| target.starts_with(prefix));
    let outside = |all: &BTreeMap<String, u64>| -> BTreeMap<String, u64> {
        all.iter()
            .filter(|(target, _)| !mine(target))
            .map(|(t, w)| (t.clone(), *w))
            .collect()
    };

    let before = enumerate("Tjommie/*");
    let result = catch_unwind(AssertUnwindSafe(|| {
        let account = from.account_target("claude-main").unwrap();
        let login = from.login_target(LOGIN).unwrap();
        cred::write(&account, "sk-test-account-key").unwrap();
        cred::write(&login, "{\"username\":\"me\",\"password\":\"pw\"}").unwrap();
        cred::write(&from.remote_host_target(), "noise-key").unwrap();
        cred::write(&from.relay_login_target(), "relay-key").unwrap();
        let source_secrets = enumerate(&format!("Tjommie/{}*", from.short()));

        // Spelled with 8.3 names where the volume has them: the secrets are still found under the source's namespace.
        let spelled = short_name(source.path()).unwrap_or_else(|| source.path().to_path_buf());
        let secrets = import::run(&spelled, target.path(), false).unwrap();
        assert_eq!(
            secrets,
            import::Secrets {
                account_keys: 1,
                logins: 1
            }
        );

        let copied = |target: &str| cred::read(target).unwrap();
        assert_eq!(
            copied(&to.account_target("claude-main").unwrap()).as_deref(),
            Some("sk-test-account-key")
        );
        assert_eq!(
            copied(&to.login_target(LOGIN).unwrap()).as_deref(),
            Some("{\"username\":\"me\",\"password\":\"pw\"}")
        );
        assert_eq!(copied(&to.remote_host_target()), None);
        assert_eq!(copied(&to.relay_login_target()), None);
        assert_eq!(
            enumerate(&format!("Tjommie/{}*", from.short())),
            source_secrets,
            "the source secrets are untouched"
        );
    }));
    for prefix in &prefixes {
        for name in enumerate(&format!("{prefix}*")).keys() {
            cred::delete(name).unwrap();
        }
    }
    let after = enumerate("Tjommie/*");
    if let Err(panic) = result {
        resume_unwind(panic);
    }
    assert!(
        after.keys().all(|name| !mine(name)),
        "test targets were left behind"
    );
    assert_eq!(
        outside(&before),
        outside(&after),
        "a Tjommie/* target outside the test namespaces changed"
    );
}
