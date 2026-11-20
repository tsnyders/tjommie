//! The login vault's storage: the cases of `packages/desktop/src/login-vault.test.ts` that concern records and secrets,
//! and the cross-implementation test. The TypeScript vault writes a login and Rust reads it, and the reverse.
//! Each test works in a fresh folder under %TEMP%, so its Credential Manager targets live in a namespace of their own.
//! Every target a test creates is deleted when the test ends, even when the test fails.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use desktop_host::owner_safety::test_root;
use desktop_host::spawn;
use desktop_host::vault::credential::{self, LoginCredentials, login_target, namespace};
use desktop_host::vault::storage::{VaultError, VaultRecords, VaultSecret};
use protocol::vault::{LoginEntry, LoginOrigin};
use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tempfile::TempDir;
use uuid::Uuid;

/// A password that no record, log or fixture output may contain.
const SECRET: &str = "Synthetic-Login-Secret-ABSENT-FROM-OUTPUT-7d14";
const TIME: &str = "2026-11-15T10:00:00.000Z";

/// The two node-backed tests each start a PowerShell that compiles the TypeScript side's Credential Manager bridge, and the
/// TypeScript transport kills it after 15 s. Two at once on a busy PC can both run out of time, so they take turns.
static NODE_TURN: Mutex<()> = Mutex::new(());
/// How many TypeScript fixtures run right now: more than one means a test forgot to take its turn.
static NODE_RUNNING: AtomicUsize = AtomicUsize::new(0);

fn node_turn() -> MutexGuard<'static, ()> {
    NODE_TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether this is a GitHub-hosted runner. Its service logons cannot reliably reach Credential Manager, and the two
/// cross-implementation tests need the TypeScript vault to read and write it, so they skip there, as the daemon's credential
/// tests do for the same reason and by the same variable (`ci.yml` sets it for `cargo test`). Everywhere else they run.
fn hosted_ci() -> bool {
    std::env::var("TJOMMIE_CI_HOSTED").as_deref() == Ok("1")
}

/// What a cross-implementation test says when it skips.
const HOSTED_SKIP: &str = "skipped: hosted Windows service logons cannot reliably reach Credential Manager, which the TypeScript vault side of this test uses";

/// Counts one running fixture, and says so if it is not alone.
struct FixtureRun;

impl FixtureRun {
    fn begin() -> Self {
        let before = NODE_RUNNING.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            before, 0,
            "two TypeScript fixtures ran at once: a test did not take its turn (node_turn)"
        );
        Self
    }
}

impl Drop for FixtureRun {
    fn drop(&mut self) {
        NODE_RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A test's folder, its Credential Manager namespace and every reference it created. Dropping it deletes those targets,
/// so a failing test leaves nothing behind in Credential Manager.
struct Sandbox {
    root: TempDir,
    ns: String,
    references: Vec<String>,
}

impl Sandbox {
    fn new() -> Self {
        let root = test_root().expect("a folder under %TEMP%");
        let ns = namespace(root.path()).expect("a namespace");
        Self {
            root,
            ns,
            references: Vec::new(),
        }
    }

    fn records_path(&self) -> PathBuf {
        self.root.path().join("data").join("logins.sqlite")
    }

    fn credentials(&self) -> LoginCredentials {
        LoginCredentials::new(self.root.path()).expect("credentials for this folder")
    }

    fn track(&mut self, reference: &str) {
        self.references.push(reference.to_owned());
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for reference in &self.references {
            if let Some(target) = login_target(&self.ns, reference) {
                // A target the test never reached is not there, which is fine.
                let _ = credential::delete_blob(&target);
            }
        }
    }
}

fn origin() -> LoginOrigin {
    LoginOrigin::new("https://login.invalid").expect("a valid origin")
}

fn login(id: &str, bot: &str, revision: u64) -> LoginEntry {
    LoginEntry {
        id: id.to_owned(),
        name: "Synthetic account".to_owned(),
        origin: origin(),
        bot_id: bot.to_owned(),
        revision,
        created_at: TIME.to_owned(),
        updated_at: TIME.to_owned(),
    }
}

/// What the TypeScript `JSON.stringify(entry)` writes for the same login. Stored rows must match it byte for byte.
fn stored_json(entry: &LoginEntry) -> String {
    format!(
        r#"{{"id":"{}","name":"{}","origin":"{}","botId":"{}","revision":{},"createdAt":"{}","updatedAt":"{}"}}"#,
        entry.id,
        entry.name,
        entry.origin.as_str(),
        entry.bot_id,
        entry.revision,
        entry.created_at,
        entry.updated_at
    )
}

/// What the TypeScript `JSON.stringify({ username, password })` writes. The Credential Manager blob must match it.
fn stored_secret_json(username: &str, password: &str) -> String {
    format!(r#"{{"username":"{username}","password":"{password}"}}"#)
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

fn raw_data(path: &Path, id: &str) -> String {
    Connection::open(path)
        .expect("the records open")
        .query_row("SELECT data FROM logins WHERE id=?1", [id], |row| {
            row.get::<_, String>(0)
        })
        .expect("the row is stored")
}

/// Fills the records with `rows` valid logins written straight to SQL, which is quicker than the entry limit's
/// own O(n) check. Returns the first row's id and reference.
fn fill(path: &Path, rows: usize) -> (String, String) {
    let db = Connection::open(path).expect("the records open");
    let mut first = None;
    for index in 0..rows {
        let entry = login(&Uuid::new_v4().to_string(), "bot", 1);
        let reference = Uuid::new_v4().to_string();
        db.execute(
            "INSERT INTO logins VALUES (?1,?2,?3)",
            (&entry.id, stored_json(&entry), &reference),
        )
        .expect("the row is inserted");
        if index == 0 {
            first = Some((entry.id.clone(), reference));
        }
    }
    first.expect("at least one row")
}

fn node_available() -> bool {
    let mut command = spawn::command("node");
    command
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteRequest<'a> {
    entry: &'a LoginEntry,
    username: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct ReadRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Written {
    reference: String,
    namespace: String,
}

#[derive(serde::Deserialize)]
struct SnapshotRow {
    data: String,
    reference: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    namespace: String,
    rows: Vec<SnapshotRow>,
    pending: Vec<String>,
    secret_matches: bool,
    list: Vec<LoginEntry>,
}

/// Runs `tests/fixtures/vault_crossimpl.ts` with node. The request goes in on stdin, never on the command line.
fn run_fixture<T: DeserializeOwned>(operation: &str, root: &Path, request: &impl Serialize) -> T {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("vault_crossimpl.ts");
    let _running = FixtureRun::begin();
    let mut command = spawn::command("node");
    command
        .arg("--disable-warning=ExperimentalWarning")
        .arg(&script)
        .arg(operation)
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("node starts");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(
            serde_json::to_string(request)
                .expect("the request encodes")
                .as_bytes(),
        )
        .expect("the request is written");
    drop(stdin);
    let output = child.wait_with_output().expect("node finishes");
    // Everything node said, as it said it: a failure here is read from this message alone.
    assert!(
        output.status.success(),
        "the TypeScript fixture `{operation}` ended with {}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).expect("the fixture prints JSON")
}

#[test]
fn records_hold_metadata_and_an_opaque_reference_never_the_password() {
    let mut sandbox = Sandbox::new();
    let id = Uuid::new_v4().to_string();
    let mut records = VaultRecords::open(&sandbox.records_path()).expect("records open");
    let reference = records.prepare().expect("a reference is journalled");
    sandbox.track(&reference);
    sandbox
        .credentials()
        .write(&reference, &VaultSecret::new("alice", SECRET))
        .expect("the secret is written");
    records
        .saved(&login(&id, "bot", 1), &reference, None)
        .expect("the login is saved");
    assert!(records.pending().expect("pending").is_empty());
    assert_eq!(
        records.get(&id, Some(1)).expect("the login").reference,
        reference
    );
    drop(records);

    let path = sandbox.records_path();
    for file in [path.clone(), with_suffix(&path, "-wal")] {
        if let Ok(bytes) = fs::read(&file) {
            assert!(
                !contains(&bytes, SECRET),
                "{} holds the password",
                file.display()
            );
        }
    }
    let secret = sandbox
        .credentials()
        .read(&reference)
        .expect("read")
        .expect("the secret is in Credential Manager");
    assert_eq!(secret.password(), SECRET);
}

#[test]
fn replacing_a_login_keeps_the_old_secret_pending_until_it_is_deleted() {
    let mut sandbox = Sandbox::new();
    let mut records = VaultRecords::open(&sandbox.records_path()).expect("records open");
    let creds = sandbox.credentials();
    let id = Uuid::new_v4().to_string();

    let first = records.prepare().expect("prepare");
    sandbox.track(&first);
    creds
        .write(&first, &VaultSecret::new("alice", SECRET))
        .expect("write");
    records
        .saved(&login(&id, "bot", 1), &first, None)
        .expect("first save");

    let second = records.prepare().expect("prepare");
    sandbox.track(&second);
    creds
        .write(&second, &VaultSecret::new("alice", "second-secret"))
        .expect("write");
    records
        .saved(&login(&id, "bot", 2), &second, Some(1))
        .expect("the edit");

    assert_eq!(records.pending().expect("pending"), vec![first.clone()]);
    assert_eq!(
        records.get(&id, Some(2)).expect("current").reference,
        second
    );
    assert!(matches!(
        records.get(&id, Some(1)),
        Err(VaultError::Changed)
    ));

    creds
        .delete(&first)
        .expect("the replaced secret is deleted");
    assert!(creds.read(&first).expect("read").is_none());
    records
        .deleted(&first)
        .expect("the journal entry is cleared");
    assert!(records.pending().expect("pending").is_empty());
}

#[test]
fn a_pending_deletion_survives_a_restart() {
    let mut sandbox = Sandbox::new();
    let path = sandbox.records_path();
    let id = Uuid::new_v4().to_string();
    let mut records = VaultRecords::open(&path).expect("records open");
    let reference = records.prepare().expect("prepare");
    sandbox.track(&reference);
    sandbox
        .credentials()
        .write(&reference, &VaultSecret::new("alice", SECRET))
        .expect("write");
    records
        .saved(&login(&id, "bot", 1), &reference, None)
        .expect("save");

    records.deleting(&id, 1).expect("the login is marked");
    assert!(records.list(None).expect("list").is_empty());
    assert_eq!(records.pending().expect("pending"), vec![reference.clone()]);
    drop(records);

    let records = VaultRecords::open(&path).expect("reopen after a restart");
    assert_eq!(records.pending().expect("pending"), vec![reference.clone()]);
    sandbox
        .credentials()
        .delete(&reference)
        .expect("the secret is deleted");
    records
        .deleted(&reference)
        .expect("the journal entry clears");
    assert!(records.pending().expect("pending").is_empty());
}

#[test]
fn a_missing_or_changed_login_is_refused_with_the_typescript_wording() {
    let sandbox = Sandbox::new();
    let mut records = VaultRecords::open(&sandbox.records_path()).expect("records open");
    let id = Uuid::new_v4().to_string();

    let missing = records.get(&id, None).expect_err("no such login");
    assert_eq!(missing.to_string(), "This login no longer exists.");

    records
        .saved(&login(&id, "bot", 1), &Uuid::new_v4().to_string(), None)
        .expect("save");
    let changed = records.get(&id, Some(2)).expect_err("the revision moved");
    assert_eq!(
        changed.to_string(),
        "This login changed. Refresh Logins before continuing."
    );
    assert!(matches!(records.deleting(&id, 2), Err(VaultError::Changed)));
}

#[test]
fn list_filters_by_bot_in_insertion_order() {
    let sandbox = Sandbox::new();
    let mut records = VaultRecords::open(&sandbox.records_path()).expect("records open");
    let (first, second, third) = (
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
    );
    records
        .saved(
            &login(&first, "bot-a", 1),
            &Uuid::new_v4().to_string(),
            None,
        )
        .expect("save");
    records
        .saved(
            &login(&second, "bot-b", 1),
            &Uuid::new_v4().to_string(),
            None,
        )
        .expect("save");
    records
        .saved(
            &login(&third, "bot-a", 1),
            &Uuid::new_v4().to_string(),
            None,
        )
        .expect("save");

    let all: Vec<String> = records
        .list(None)
        .expect("list")
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    assert_eq!(all, vec![first.clone(), second, third.clone()]);
    let bot_a: Vec<String> = records
        .list(Some("bot-a"))
        .expect("list")
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    assert_eq!(bot_a, vec![first, third]);
}

#[test]
fn the_1001st_login_is_refused_but_an_existing_one_can_still_be_saved() {
    let sandbox = Sandbox::new();
    let path = sandbox.records_path();
    drop(VaultRecords::open(&path).expect("records open"));
    let (existing_id, existing_reference) = fill(&path, 1000);

    let mut records = VaultRecords::open(&path).expect("reopen");
    let newcomer = Uuid::new_v4().to_string();
    let refused = records
        .saved(
            &login(&newcomer, "bot", 1),
            &Uuid::new_v4().to_string(),
            None,
        )
        .expect_err("the entry limit");
    assert_eq!(refused.to_string(), "Logins has reached its entry limit.");

    let edited = Uuid::new_v4().to_string();
    records
        .saved(&login(&existing_id, "bot", 2), &edited, Some(1))
        .expect("an edit does not add an entry");
    assert_eq!(
        records.pending().expect("pending"),
        vec![existing_reference]
    );
}

#[test]
fn a_stored_login_with_an_extra_key_is_refused_as_the_strict_schema_refuses_it() {
    let sandbox = Sandbox::new();
    let path = sandbox.records_path();
    let id = Uuid::new_v4().to_string();
    drop(VaultRecords::open(&path).expect("records open"));
    let db = Connection::open(&path).expect("open");
    let clean = stored_json(&login(&id, "bot", 1));
    db.execute(
        "INSERT INTO logins VALUES (?1,?2,?3)",
        (&id, &clean, Uuid::new_v4().to_string()),
    )
    .expect("insert");
    let records = VaultRecords::open(&path).expect("reopen");
    assert_eq!(records.list(None).expect("a clean row lists").len(), 1);

    let with_extra = format!(
        r#"{{"id":"{id}","name":"Synthetic account","origin":"https://login.invalid","botId":"bot","revision":1,"createdAt":"{TIME}","updatedAt":"{TIME}","extra":1}}"#
    );
    db.execute("UPDATE logins SET data=?1 WHERE id=?2", (&with_extra, &id))
        .expect("update");
    assert!(matches!(records.list(None), Err(VaultError::InvalidEntry)));
}

#[test]
fn a_records_folder_behind_a_junction_is_refused() {
    let sandbox = Sandbox::new();
    let real = sandbox.root.path().join("real");
    fs::create_dir(&real).expect("the real folder");
    let link = sandbox.root.path().join("linked");
    let mut command = spawn::command("cmd");
    let status = command
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&real)
        .status()
        .expect("cmd runs");
    assert!(status.success(), "mklink /J failed");

    let refused = VaultRecords::open(&link.join("logins.sqlite"))
        .expect_err("a junction in the records path");
    assert!(matches!(refused, VaultError::Storage));
}

#[test]
fn a_secret_with_a_line_break_is_refused_before_credential_manager_is_touched() {
    let mut sandbox = Sandbox::new();
    let creds = sandbox.credentials();
    let reference = Uuid::new_v4().to_string();
    sandbox.track(&reference);

    let refused = creds
        .write(&reference, &VaultSecret::new("alice", "line\nbreak"))
        .expect_err("a line break is refused");
    assert_eq!(
        refused.to_string(),
        "Enter a username and password without line breaks, within the protected storage limit."
    );
    assert!(creds.read(&reference).expect("read").is_none());
}

#[test]
fn credential_manager_keeps_a_website_login_in_its_own_namespace() {
    let mut sandbox = Sandbox::new();
    let creds = sandbox.credentials();
    let reference = Uuid::new_v4().to_string();
    sandbox.track(&reference);
    let password = format!("{SECRET} with spaces");

    creds
        .write(
            &reference,
            &VaultSecret::new("Person With Spaces", password.clone()),
        )
        .expect("write");
    let read = creds.read(&reference).expect("read").expect("present");
    assert_eq!(read.username(), "Person With Spaces");
    assert_eq!(read.password(), password);

    let target = login_target(&sandbox.ns, &reference).expect("a target");
    let parts: Vec<&str> = target.split('/').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], "Tjommie");
    assert_eq!(parts[1], sandbox.ns);
    assert_eq!(parts[2], "login");
    assert_eq!(parts[3], reference);
    assert_eq!(sandbox.ns.len(), 24);
    assert!(
        sandbox
            .ns
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );

    creds.delete(&reference).expect("delete");
    assert!(creds.read(&reference).expect("read").is_none());
}

#[test]
fn a_malformed_reference_fails_closed_without_credential_manager() {
    let sandbox = Sandbox::new();
    let creds = sandbox.credentials();
    let secret = VaultSecret::new("alice", "pw");
    assert!(matches!(creds.read("../escape"), Err(VaultError::Storage)));
    assert!(matches!(
        creds.write("ABCDEF00-0000-4000-8000-000000000123", &secret),
        Err(VaultError::Storage)
    ));
    assert!(matches!(creds.delete("short"), Err(VaultError::Storage)));
}

#[test]
fn the_typescript_vault_writes_a_login_that_rust_reads() {
    if hosted_ci() {
        eprintln!("{HOSTED_SKIP}");
        return;
    }
    if !node_available() {
        eprintln!("skipped: node is not on PATH, so the TypeScript side cannot run");
        return;
    }
    let _turn = node_turn();
    let mut sandbox = Sandbox::new();
    let id = Uuid::new_v4().to_string();
    let entry = login(&id, "bot", 1);
    let written: Written = run_fixture(
        "write",
        sandbox.root.path(),
        &WriteRequest {
            entry: &entry,
            username: "alice",
            password: SECRET,
        },
    );
    sandbox.track(&written.reference);
    assert_eq!(written.namespace, sandbox.ns, "the namespaces agree");

    let path = sandbox.records_path();
    let records = VaultRecords::open(&path).expect("Rust opens the TypeScript records");
    let stored = records.get(&id, Some(1)).expect("the TypeScript's login");
    assert_eq!(stored.entry, entry);
    assert_eq!(stored.reference, written.reference);
    assert_eq!(
        raw_data(&path, &id),
        stored_json(&entry),
        "byte-identical row"
    );

    let secret = sandbox
        .credentials()
        .read(&written.reference)
        .expect("read")
        .expect("the TypeScript's secret");
    assert_eq!((secret.username(), secret.password()), ("alice", SECRET));
    let blob =
        credential::read_blob(&login_target(&sandbox.ns, &written.reference).expect("target"))
            .expect("blob")
            .expect("present");
    assert_eq!(
        blob.as_slice(),
        stored_secret_json("alice", SECRET).as_bytes(),
        "byte-identical blob"
    );
}

#[test]
fn rust_writes_a_login_that_the_typescript_vault_reads() {
    if hosted_ci() {
        eprintln!("{HOSTED_SKIP}");
        return;
    }
    if !node_available() {
        eprintln!("skipped: node is not on PATH, so the TypeScript side cannot run");
        return;
    }
    let _turn = node_turn();
    let mut sandbox = Sandbox::new();
    let id = Uuid::new_v4().to_string();
    let entry = login(&id, "bot", 1);
    let mut records = VaultRecords::open(&sandbox.records_path()).expect("records open");
    let reference = records.prepare().expect("prepare");
    sandbox.track(&reference);
    sandbox
        .credentials()
        .write(&reference, &VaultSecret::new("alice", SECRET))
        .expect("write");
    records.saved(&entry, &reference, None).expect("save");
    drop(records);

    let snapshot: Snapshot = run_fixture(
        "read",
        sandbox.root.path(),
        &ReadRequest {
            username: "alice",
            password: SECRET,
        },
    );
    assert_eq!(snapshot.namespace, sandbox.ns, "the namespaces agree");
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(
        snapshot.rows[0].data,
        stored_json(&entry),
        "byte-identical row"
    );
    assert_eq!(snapshot.rows[0].reference, reference);
    assert!(snapshot.pending.is_empty());
    assert!(
        snapshot.secret_matches,
        "the TypeScript reads the same secret"
    );
    assert_eq!(snapshot.list, vec![entry]);
}
