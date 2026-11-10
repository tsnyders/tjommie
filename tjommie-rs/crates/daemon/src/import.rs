//! `tjommie-daemon import --from <root> [--replace]`: a one-way copy of an install into this build's data root, so a
//! dev build can run against real data (spec §4). The source is only read: SQLite files go through the backup API,
//! which is safe while that install runs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::backup::NO_PROGRESS;
use rusqlite::config::DbConfig;
use rusqlite::{Connection, MAIN_DB};

use crate::paths::data_paths;
use crate::win::cred::{self, Namespace};

/// The Claude CLI's sign-in is never copied: refresh-token rotation would sign one of the two installs out.
const CLAUDE_SIGN_IN: &str = ".credentials.json";

/// The project-team schema (migration 11), children first: its foreign keys have no `ON DELETE CASCADE`.
const PROJECT_TEAM_TABLES: [&str; 9] = [
    "task_evaluations",
    "task_integrations",
    "task_attempts",
    "project_tasks",
    "project_members",
    "projects",
    "project_repositories",
    "project_bot_leases",
    "task_requests",
];

/// `new Date().toISOString()` in SQL.
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

/// What `copy_secrets` re-wrote, so an import that found none can say so.
#[derive(Debug, PartialEq, Eq)]
pub struct Secrets {
    pub account_keys: usize,
    pub logins: usize,
}

/// `copy_data`, then `copy_secrets`.
pub fn run(from: &Path, to: &Path, replace: bool) -> io::Result<Secrets> {
    copy_data(from, to, replace)?;
    copy_secrets(from, to)
}

/// Copies `tjommie.db`, `logins.sqlite` and `bots\`, then detaches the copy from the source install: it forgets the
/// paired phones, this PC's remote identity and the project teams, and bot and account folders under the source root
/// move to `to`.
/// Refuses a target that already has a database unless `replace`, and never writes into the source root.
pub fn copy_data(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    // The guards compare what the paths name: `\\?\`, junction and 8.3 spellings reach the same folders.
    let (source, target_identity) = (identity(from)?, identity(to)?);
    if is_within(&target_identity, &source) {
        return Err(io::Error::other(
            "Tjommie can't import into the source data root or a folder inside it.",
        ));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA")
        && is_within(
            &target_identity,
            &identity(&PathBuf::from(local).join("Tjommie"))?,
        )
    {
        return Err(io::Error::other(
            "Tjommie won't import into the production data root. Set TJOMMIE_HOME to a dev root.",
        ));
    }
    // The copy is written, and its folders named, the way this install spells its own root.
    let target = plain(&std::path::absolute(to)?);
    let (source, target) = (Path::new(&source), Path::new(&target));
    let source_db = source.join("data").join("tjommie.db");
    if !source_db.is_file() {
        return Err(io::Error::other(format!(
            "There is no Tjommie database at {}.",
            source_db.display()
        )));
    }
    if target.join("data").join("tjommie.db").exists() && !replace {
        return Err(io::Error::other(format!(
            "{} already has a Tjommie database. Import again with --replace to overwrite it.",
            target.display()
        )));
    }

    // Everything is staged under `.partial` names first, so a failed import leaves an earlier one as it was.
    let paths = data_paths(target)?;
    let logins = paths.data.join("logins.sqlite");
    let staged = |path: &Path| {
        let mut name = path.as_os_str().to_owned();
        name.push(".partial");
        PathBuf::from(name)
    };
    let (staged_db, staged_logins, staged_bots) =
        (staged(&paths.db), staged(&logins), staged(&paths.bots));
    remove_files(&[&staged_db, &staged_logins])?;
    ignore_missing(fs::remove_dir_all(&staged_bots))?;
    back_up(&source_db, &staged_db)?;
    let source_logins = source.join("data").join("logins.sqlite");
    if source_logins.is_file() {
        back_up(&source_logins, &staged_logins)?;
    }
    copy_tree(&source.join("bots"), &staged_bots)?;
    detach(
        &staged_db,
        &source.to_string_lossy(),
        &target.to_string_lossy(),
    )
    .map_err(io::Error::other)?;

    // The swap. A database's old -wal must go with it, or SQLite would replay it into the new one.
    remove_files(&[&paths.db, &logins])?;
    fs::rename(&staged_db, &paths.db)?;
    if source_logins.is_file() {
        fs::rename(&staged_logins, &logins)?;
    }
    ignore_missing(fs::remove_dir_all(&paths.bots))?;
    fs::rename(&staged_bots, &paths.bots)
}

/// Re-writes the source's `account/` and `login/` secrets under this root's namespace. The `remote-host` and
/// `relay-login` keys and the worker passwords stay behind: the new install makes its own.
pub fn copy_secrets(from: &Path, to: &Path) -> io::Result<Secrets> {
    // The source named its secrets after its root as it spelled it, most likely as given here; an 8.3 or `\\?\`
    // spelling gives another namespace, so the canonical one is tried too. The target is named as this install
    // spells its own root.
    let mut sources = vec![Namespace::new(from)?];
    let canonical = Namespace::new(Path::new(&identity(from)?))?;
    if canonical.short() != sources[0].short() {
        sources.push(canonical);
    }
    let target = Namespace::new(to)?;
    let data = to.join("data");
    let mut secrets = Secrets {
        account_keys: 0,
        logins: 0,
    };
    for id in column(&data.join("tjommie.db"), "SELECT id FROM accounts")? {
        let from = sources
            .iter()
            .map(|source| source.account_target(&id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        let to = target.account_target(&id).map_err(io::Error::other)?;
        secrets.account_keys += usize::from(copy_secret(&from, &to)?);
    }
    let logins = data.join("logins.sqlite");
    if logins.is_file() {
        for reference in column(&logins, "SELECT reference FROM logins")? {
            let from: Vec<String> = sources
                .iter()
                .filter_map(|source| source.login_target(&reference))
                .collect();
            if let Some(to) = target.login_target(&reference) {
                secrets.logins += usize::from(copy_secret(&from, &to)?);
            }
        }
    }
    Ok(secrets)
}

/// Copies the first of `from` that holds a secret. Says whether one did.
fn copy_secret(from: &[String], to: &str) -> io::Result<bool> {
    for target in from {
        if let Some(secret) = cred::read(target)? {
            cred::write(to, &secret)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// What a path names, for comparing roots: `fs::canonicalize` of its deepest existing part, which sees through
/// junctions, 8.3 names and `\\?\`, then the rest as written. `bots.ts` checks project folders with
/// `realpathSync.native` the same way.
fn identity(path: &Path) -> io::Result<String> {
    let absolute = std::path::absolute(path)?;
    let mut existing = absolute.as_path();
    while !existing.exists() {
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return Ok(plain(&absolute)), // Not even the drive exists.
        }
    }
    let rest = absolute
        .strip_prefix(existing)
        .expect("an ancestor is a prefix");
    Ok(plain(&fs::canonicalize(existing)?.join(rest)))
}

/// A path as text, without a `\\?\` prefix or a trailing separator.
fn plain(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    };
    text.trim_end_matches('\\').to_owned()
}

/// What follows `root` in `path` (empty, or starting with `\`), if `path` is `root` or inside it. Windows ignores case.
fn inside<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let mut chars = path.char_indices();
    for expected in root.chars() {
        let (_, actual) = chars.next()?;
        if !actual.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
    }
    let rest = &path[chars.offset()..];
    (rest.is_empty() || rest.starts_with('\\')).then_some(rest)
}

fn is_within(path: &str, root: &str) -> bool {
    inside(path, root).is_some()
}

fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// SQLite databases with their sidecar files.
fn remove_files(databases: &[&Path]) -> io::Result<()> {
    for database in databases {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut file = database.as_os_str().to_owned();
            file.push(suffix);
            ignore_missing(fs::remove_file(file))?;
        }
    }
    Ok(())
}

/// A read-write connection that never writes. A read-only one would leave behind the `-wal` and `-shm` it creates;
/// this one removes them when it closes. A `-wal` that was already there (the install is running, or crashed) must
/// not be checkpointed into its database on close, so it is left as found; then the `-shm` that SQLite built to read
/// it stays too, and goes if it wasn't there before. SQLite may still refresh an `-shm` that was.
fn back_up(source: &Path, target: &Path) -> io::Result<()> {
    let sidecar = |suffix: &str| {
        let mut name = source.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let (wal, shm) = (sidecar("-wal"), sidecar("-shm"));
    let (had_wal, had_shm) = (wal.exists(), shm.exists());
    let copied = (|| {
        let db = Connection::open(source)?;
        if had_wal {
            db.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
        }
        db.backup(MAIN_DB, target, NO_PROGRESS)
    })();
    if !had_shm {
        let _ = fs::remove_file(&shm); // Best effort: the source install may have opened it meanwhile.
    }
    copied.map_err(io::Error::other)
}

/// Folders and regular files, without the Claude sign-in. Links are skipped: they could reach outside the root.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    if !from.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_file() && entry.file_name() != CLAUDE_SIGN_IN {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn detach(db: &Path, from: &str, to: &str) -> rusqlite::Result<()> {
    let mut db = Connection::open(db)?;
    let tx = db.transaction()?;
    tx.execute_batch("DELETE FROM devices; DELETE FROM remote_host; DELETE FROM pairing_codes;")?;
    // Project teams point at git repositories and worktrees outside the root, which a copy would then work in.
    for table in PROJECT_TEAM_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    // Live task work would act on attempts that are gone, so it ends as failed, the way the daemon ends work itself.
    // Finished rows keep their history; sessions keyed by a task's context are never matched again.
    tx.execute_batch(&format!(
        "UPDATE work_items SET status = 'failed', ended_at = {NOW}
           WHERE task_attempt_id IS NOT NULL AND status IN ('queued', 'running');
         UPDATE exchanges SET status = 'failed', reply = 'Project teams were not imported.', replied_at = {NOW}
           WHERE task_attempt_id IS NOT NULL AND status = 'waiting';"
    ))?;
    // The daemon uses these folders as stored, so copies left pointing into the source would write there. They are
    // matched by what they name, so any spelling of the source root (`from` is its identity) moves.
    for (table, folder) in [("bots", "home_folder"), ("accounts", "folder")] {
        let rows: Vec<(String, String)> = tx
            .prepare(&format!("SELECT id, {folder} FROM {table}"))?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (id, path) in rows {
            let named = identity(Path::new(&path))
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.into()))?;
            if let Some(rest) = inside(&named, from) {
                tx.execute(
                    &format!("UPDATE {table} SET {folder} = ?1 WHERE id = ?2"),
                    [format!("{to}{rest}"), id],
                )?;
            }
        }
    }
    tx.commit()
}

fn column(db: &Path, sql: &str) -> io::Result<Vec<String>> {
    let db = Connection::open(db).map_err(io::Error::other)?;
    let mut statement = db.prepare(sql).map_err(io::Error::other)?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(io::Error::other)?
        .collect::<Result<_, _>>()
        .map_err(io::Error::other)
}
