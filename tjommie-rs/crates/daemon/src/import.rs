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

/// `copy_data`, then `copy_secrets`.
pub fn run(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    copy_data(from, to, replace)?;
    copy_secrets(from, to)
}

/// Copies `tjommie.db`, `logins.sqlite` and `bots\`, then detaches the copy from the source install: it forgets the
/// paired phones, this PC's remote identity and the project teams, and bot and account folders under the source root
/// move to `to`.
/// Refuses a target that already has a database unless `replace`, and never writes into the source root.
pub fn copy_data(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let (source, target) = (resolved(from)?, resolved(to)?);
    if is_within(&target, &source) {
        return Err(io::Error::other(
            "Tjommie can't import into the source data root or a folder inside it.",
        ));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA")
        && is_within(&target, &resolved(&PathBuf::from(local).join("Tjommie"))?)
    {
        return Err(io::Error::other(
            "Tjommie won't import into the production data root. Set TJOMMIE_HOME to a dev root.",
        ));
    }
    let (source, target) = (Path::new(&source), Path::new(&target));
    let source_db = source.join("data").join("tjommie.db");
    if !source_db.is_file() {
        return Err(io::Error::other(format!(
            "There is no Tjommie database at {}.",
            source_db.display()
        )));
    }
    if target.join("data").join("tjommie.db").exists() {
        if !replace {
            return Err(io::Error::other(format!(
                "{} already has a Tjommie database. Import again with --replace to overwrite it.",
                target.display()
            )));
        }
        remove_imported(target)?;
    }

    let paths = data_paths(target)?;
    back_up(&source_db, &paths.db)?;
    let source_logins = source.join("data").join("logins.sqlite");
    if source_logins.is_file() {
        back_up(&source_logins, &paths.data.join("logins.sqlite"))?;
    }
    copy_tree(&source.join("bots"), &paths.bots)?;
    detach(
        &paths.db,
        &source.to_string_lossy(),
        &target.to_string_lossy(),
    )
    .map_err(io::Error::other)
}

/// Re-writes the source's `account/` and `login/` secrets under this root's namespace. The `remote-host` and
/// `relay-login` keys and the worker passwords stay behind: the new install makes its own.
pub fn copy_secrets(from: &Path, to: &Path) -> io::Result<()> {
    let (source, target) = (Namespace::new(from)?, Namespace::new(to)?);
    let data = to.join("data");
    let mut targets = Vec::new();
    for id in column(&data.join("tjommie.db"), "SELECT id FROM accounts")? {
        let pair = (source.account_target(&id), target.account_target(&id));
        targets.push((
            pair.0.map_err(io::Error::other)?,
            pair.1.map_err(io::Error::other)?,
        ));
    }
    let logins = data.join("logins.sqlite");
    if logins.is_file() {
        for reference in column(&logins, "SELECT reference FROM logins")? {
            if let (Some(from), Some(to)) = (
                source.login_target(&reference),
                target.login_target(&reference),
            ) {
                targets.push((from, to));
            }
        }
    }
    for (from, to) in targets {
        if let Some(secret) = cred::read(&from)? {
            cred::write(&to, &secret)?;
        }
    }
    Ok(())
}

/// Absolute, without a trailing separator.
fn resolved(path: &Path) -> io::Result<String> {
    let absolute = std::path::absolute(path)?;
    let text = absolute.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    Ok(text.trim_end_matches('\\').to_owned())
}

/// `path` is `root` or inside it. Windows paths ignore case.
fn is_within(path: &str, root: &str) -> bool {
    let (path, root) = (path.to_lowercase(), root.to_lowercase());
    path == root || path.starts_with(&format!("{root}\\"))
}

/// Whatever an earlier import left, so `--replace` starts clean.
fn remove_imported(root: &Path) -> io::Result<()> {
    let ignore_missing = |result: io::Result<()>| match result {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    for name in ["tjommie.db", "logins.sqlite"] {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            ignore_missing(fs::remove_file(
                root.join("data").join(format!("{name}{suffix}")),
            ))?;
        }
    }
    ignore_missing(fs::remove_dir_all(root.join("bots")))
}

/// A read-write connection that never writes. A read-only one would leave behind the `-wal` and `-shm` it creates;
/// this one removes them when it closes. A `-wal` that was already there (the install is running, or crashed) must
/// not be checkpointed into its database on close, so it is left as found. SQLite may still refresh the `-shm` index.
fn back_up(source: &Path, target: &Path) -> io::Result<()> {
    let mut wal = source.as_os_str().to_owned();
    wal.push("-wal");
    let had_wal = Path::new(&wal).exists();
    let db = Connection::open(source).map_err(io::Error::other)?;
    if had_wal {
        db.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(io::Error::other)?;
    }
    db.backup(MAIN_DB, target, NO_PROGRESS)
        .map_err(io::Error::other)
}

/// Folders and regular files, without the Claude sign-in. Links are skipped: they could reach outside the root.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(to)?;
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
    // The daemon uses these folders as stored, so copies left pointing into the source would write there.
    for (table, folder) in [("bots", "home_folder"), ("accounts", "folder")] {
        let rows: Vec<(String, String)> = tx
            .prepare(&format!("SELECT id, {folder} FROM {table}"))?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (id, path) in rows {
            if let Some(moved) = rebase(&path, from, to) {
                tx.execute(
                    &format!("UPDATE {table} SET {folder} = ?1 WHERE id = ?2"),
                    [&moved, &id],
                )?;
            }
        }
    }
    tx.commit()
}

/// `path` with the `from` root swapped for `to`, if it is inside `from`.
fn rebase(path: &str, from: &str, to: &str) -> Option<String> {
    let rest = path.get(from.len()..)?;
    (path[..from.len()].eq_ignore_ascii_case(from) && rest.starts_with('\\'))
        .then(|| format!("{to}{rest}"))
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
