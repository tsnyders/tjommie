//! `bots.ts`: bots in the database and their home folders, plus `botStateOf` from `runner.ts` and the `bots.*`
//! handlers from `daemon.ts`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use protocol::model::{Autonomy, Bot, BotColor, BotInput, BotPatch, BotShape, BotState, Provider};
use protocol::wire::{Empty, Id, Notes, method};
use protocol::{ErrorCode, Event};
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::context::Context;
use crate::ids::{new_id, now_iso};
use crate::server::{Handlers, RpcError};

/// A bot as stored: everything but its live state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBot {
    pub id: String,
    pub name: String,
    pub color: BotColor,
    pub shape: BotShape,
    pub instructions: String,
    pub provider: Provider,
    pub model: String,
    pub account_id: Option<String>,
    pub autonomy: Autonomy,
    pub home_folder: String,
    pub project_folder: Option<String>,
    pub browser: bool,
    pub paused: bool,
    pub can_message: Vec<String>,
    pub limited_until: Option<String>,
    pub created_at: String,
}

impl StoredBot {
    pub fn into_bot(self, state: BotState, setup_reason: Option<String>) -> Bot {
        Bot {
            id: self.id,
            name: self.name,
            color: self.color,
            shape: self.shape,
            instructions: self.instructions,
            provider: self.provider,
            model: self.model,
            account_id: Some(self.account_id),
            effective_account_id: None,
            using_fallback: None,
            autonomy: self.autonomy,
            home_folder: self.home_folder,
            project_folder: self.project_folder,
            browser: self.browser,
            paused: self.paused,
            can_message: self.can_message,
            state,
            setup_reason,
            limited_until: self.limited_until,
            created_at: self.created_at,
        }
    }
}

/// What decides a bot's state (`botStateOf` in `runner.ts`).
pub struct BotStatus {
    pub signed_in: bool,
    pub setup_reason: Option<String>,
    pub paused: bool,
    pub limited_until: Option<String>,
    pub waiting: bool,
    pub busy: bool,
}

/// `now` and `limited_until` are ISO times, which compare as text.
pub fn bot_state_of(status: &BotStatus, now: &str) -> BotState {
    if !status.signed_in || status.setup_reason.is_some() {
        BotState::NeedsSetup
    } else if status.paused {
        BotState::Paused
    } else if status
        .limited_until
        .as_deref()
        .is_some_and(|until| until > now)
    {
        BotState::Limited
    } else if status.waiting {
        BotState::Waiting
    } else if status.busy {
        BotState::Working
    } else {
        BotState::Idle
    }
}

fn bad_params(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadParams, message)
}

/// An enum's wire name, as stored.
fn text<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .expect("enums serialize")
        .as_str()
        .expect("enums serialize as text")
        .to_owned()
}

fn from_text<T: DeserializeOwned>(row: &Row<'_>, column: usize) -> rusqlite::Result<T> {
    let value: String = row.get(column)?;
    serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn read_bot(row: &Row<'_>) -> rusqlite::Result<StoredBot> {
    Ok(StoredBot {
        id: row.get(0)?,
        name: row.get(1)?,
        color: from_text(row, 2)?,
        shape: from_text(row, 3)?,
        instructions: row.get(4)?,
        provider: from_text(row, 5)?,
        model: row.get(6)?,
        account_id: row.get(7)?,
        autonomy: from_text(row, 8)?,
        home_folder: row.get(9)?,
        project_folder: row.get(10)?,
        browser: row.get::<_, i64>(11)? == 1,
        paused: row.get::<_, i64>(12)? == 1,
        can_message: Vec::new(),
        limited_until: row.get(13)?,
        created_at: row.get(14)?,
    })
}

pub fn get_stored_bot(db: &Connection, id: &str) -> Result<StoredBot, RpcError> {
    let bot = db
        .query_row(
            "SELECT id, name, color, shape, instructions, provider, model, account_id, autonomy, home_folder, project_folder, browser, paused,
                    limited_until, created_at FROM bots WHERE id = ?1",
            [id],
            read_bot,
        )
        .optional()?
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such bot"))?;
    let mut links =
        db.prepare("SELECT target_bot_id FROM bot_links WHERE bot_id = ?1 ORDER BY target_bot_id")?;
    let can_message = links
        .query_map([id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(StoredBot { can_message, ..bot })
}

pub fn list_stored_bots(db: &Connection) -> Result<Vec<StoredBot>, RpcError> {
    let mut statement = db.prepare("SELECT id FROM bots ORDER BY created_at, rowid")?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| get_stored_bot(db, id)).collect()
}

/// Makes the home folder with `notes.md` and an `inbox`, then the row, its chat and its links in one transaction.
pub fn create_bot(
    db: &Connection,
    bots_dir: &Path,
    input: &BotInput,
) -> Result<StoredBot, RpcError> {
    assert_name_free(db, &input.name, None)?;
    let account_id = account_pin(db, input.provider, input.account_id.clone().flatten())?;
    let project_folder = check_project_folder(input.project_folder.clone().flatten(), bots_dir)?;
    let id = new_id();
    let home = bots_dir.join(&id);
    fs::create_dir_all(home.join("inbox"))?;
    fs::write(home.join("notes.md"), format!("# {}'s notes\n", input.name))?;
    let stored = (|| -> Result<(), RpcError> {
        let transaction = db.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO bots (id, name, color, shape, instructions, provider, model, account_id, autonomy, home_folder, project_folder, browser, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                input.name,
                text(&input.color),
                text(&input.shape),
                input.instructions,
                text(&input.provider),
                input.model,
                account_id,
                text(&input.autonomy.unwrap_or(Autonomy::Folder)),
                home.to_string_lossy(),
                project_folder,
                input.browser.unwrap_or(false),
                now_iso(),
            ],
        )?;
        transaction.execute(
            "INSERT INTO conversations (id, kind, bot_id) VALUES (?1, 'bot', ?1)",
            [&id],
        )?;
        set_links(
            &transaction,
            &id,
            input.can_message.as_deref().unwrap_or_default(),
        )?;
        transaction.commit()?;
        Ok(())
    })();
    if let Err(error) = stored {
        let _ = fs::remove_dir_all(&home);
        return Err(error);
    }
    get_stored_bot(db, &id)
}

fn home_parent(bot: &StoredBot) -> &Path {
    Path::new(&bot.home_folder)
        .parent()
        .unwrap_or(Path::new(&bot.home_folder))
}

/// Changes only the patched fields. A new provider or account clears the old limit; a new provider drops the saved
/// "Always allow" rules, which are in the old CLI's format.
pub fn update_bot(db: &Connection, id: &str, patch: &BotPatch) -> Result<StoredBot, RpcError> {
    let bot = get_stored_bot(db, id)?;
    validate_bot_patch(db, &bot, patch)?;
    let project_folder = match &patch.project_folder {
        None => bot.project_folder.clone(),
        Some(folder) => check_project_folder(folder.clone(), home_parent(&bot))?,
    };
    let provider = patch.provider.unwrap_or(bot.provider);
    let account_id = if patch.account_id.is_some() || provider != bot.provider {
        account_pin(db, provider, patch.account_id.clone().flatten())?
    } else {
        bot.account_id.clone()
    };
    let limited_until = if account_id != bot.account_id || provider != bot.provider {
        None
    } else {
        bot.limited_until.clone()
    };
    let transaction = db.unchecked_transaction()?;
    transaction.execute(
        "UPDATE bots SET name = ?1, color = ?2, shape = ?3, instructions = ?4, provider = ?5, model = ?6, account_id = ?7, autonomy = ?8,
         project_folder = ?9, browser = ?10, limited_until = ?11 WHERE id = ?12",
        params![
            patch.name.as_ref().unwrap_or(&bot.name),
            text(&patch.color.unwrap_or(bot.color)),
            text(&patch.shape.unwrap_or(bot.shape)),
            patch.instructions.as_ref().unwrap_or(&bot.instructions),
            text(&provider),
            patch.model.as_ref().unwrap_or(&bot.model),
            account_id,
            text(&patch.autonomy.unwrap_or(bot.autonomy)),
            project_folder,
            patch.browser.unwrap_or(bot.browser),
            limited_until,
            id,
        ],
    )?;
    if let Some(targets) = &patch.can_message {
        set_links(&transaction, id, targets)?;
    }
    if provider != bot.provider {
        transaction.execute("DELETE FROM allow_rules WHERE bot_id = ?1", [id])?;
    }
    transaction.commit()?;
    get_stored_bot(db, id)
}

pub fn validate_bot_patch(
    db: &Connection,
    bot: &StoredBot,
    patch: &BotPatch,
) -> Result<(), RpcError> {
    if patch
        .provider
        .is_some_and(|provider| provider != bot.provider)
        && patch.model.is_none()
    {
        return Err(bad_params("A model is required when changing provider"));
    }
    if let Some(name) = &patch.name {
        assert_name_free(db, name, Some(&bot.id))?;
    }
    if let Some(folder) = &patch.project_folder {
        check_project_folder(folder.clone(), home_parent(bot))?;
    }
    if let Some(targets) = &patch.can_message {
        validate_links(db, &bot.id, targets)?;
    }
    if patch.account_id.is_some() || patch.provider.is_some() {
        account_pin(
            db,
            patch.provider.unwrap_or(bot.provider),
            patch.account_id.clone().flatten(),
        )?;
    }
    Ok(())
}

/// The requested account, or the provider's default account when none is requested.
fn account_pin(
    db: &Connection,
    provider: Provider,
    requested: Option<String>,
) -> Result<Option<String>, RpcError> {
    let read = |row: &Row<'_>| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
    let row = match &requested {
        None => db
            .query_row(
                "SELECT id, provider FROM accounts WHERE provider = ?1 AND deleted_at IS NULL ORDER BY is_default DESC, created_at, id LIMIT 1",
                [text(&provider)],
                read,
            )
            .optional()?,
        Some(id) => db.query_row("SELECT id, provider FROM accounts WHERE id = ?1 AND deleted_at IS NULL", [id], read).optional()?,
    };
    let Some((id, account_provider)) = row else {
        if requested.is_some() {
            return Err(bad_params("Choose an existing account for this provider."));
        }
        return Ok(None); // Legacy unit fixtures can have no accounts; the daemon makes them before creating bots.
    };
    if account_provider != text(&provider) {
        return Err(bad_params("The account must use the bot's provider."));
    }
    Ok(Some(id))
}

/// `rmSync(..., { maxRetries: 5, retryDelay: 200 })`: Windows briefly locks files that an indexer or antivirus opens.
fn remove_with_retries(folder: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::remove_dir_all(folder) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(200 * attempt));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            result => return result,
        }
    }
}

/// Deletes the bot, its chat and its home folder. An attached project folder is never touched.
pub fn delete_bot(db: &Connection, id: &str) -> Result<(), RpcError> {
    let bot = get_stored_bot(db, id)?;
    let home = PathBuf::from(&bot.home_folder);
    let removed = PathBuf::from(format!("{}.deleting-{}", bot.home_folder, new_id()));
    let had_folder = home.exists();
    // A locked folder must fail before any of its contents are removed.
    if had_folder && fs::rename(&home, &removed).is_err() {
        return Err(RpcError::new(ErrorCode::Busy, "The bot folder is busy"));
    }
    if let Err(error) = db.execute("DELETE FROM bots WHERE id = ?1", [id]) {
        if had_folder {
            let _ = fs::rename(&removed, &home);
        }
        return Err(error.into());
    }
    // The bot is deleted. Never restore a folder after a partial recursive removal; the next sweep retries it.
    if had_folder && let Err(error) = remove_with_retries(&removed) {
        eprintln!("Could not finish removing a deleted bot folder. {error}");
    }
    Ok(())
}

static LEFTOVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})\.deleting-[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$")
        .unwrap()
});

/// A path for comparing: absolute, and lowercase because Windows paths ignore case.
fn path_key(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

/// Retries folders left behind after a committed deletion. A crash before the row was deleted can leave the same rename
/// behind, so a live bot's folder is never removed.
pub fn sweep_deleted_bot_folders(db: &Connection, bots_dir: &Path) {
    if !bots_dir.exists() {
        return;
    }
    if let Err(error) = sweep(db, bots_dir) {
        eprintln!("Could not check for deleted bot folders. {error}");
    }
}

fn sweep(db: &Connection, bots_dir: &Path) -> Result<(), RpcError> {
    let mut statement = db.prepare("SELECT id, home_folder FROM bots")?;
    let live = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let ids: HashSet<String> = live.iter().map(|(id, _)| id.to_lowercase()).collect();
    let homes: Vec<String> = live
        .iter()
        .map(|(_, home)| path_key(Path::new(home)))
        .collect();
    for entry in fs::read_dir(bots_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(owner) = LEFTOVER
            .captures(&name)
            .map(|found| found[1].to_lowercase())
        else {
            continue;
        };
        // Directory links are not deletion leftovers, and removing a real folder unlinks nested junctions without
        // following them.
        if !entry.file_type()?.is_dir() || ids.contains(&owner) {
            continue;
        }
        let folder = bots_dir.join(&name);
        let candidate = path_key(&folder);
        if homes.iter().any(|home| {
            *home == candidate
                || home.starts_with(&format!("{candidate}{}", std::path::MAIN_SEPARATOR))
        }) {
            continue;
        }
        if let Err(error) = remove_with_retries(&folder) {
            eprintln!("Could not finish removing a deleted bot folder. {error}");
        }
    }
    Ok(())
}

pub fn set_paused(db: &Connection, id: &str, paused: bool) -> Result<(), RpcError> {
    get_stored_bot(db, id)?;
    db.execute(
        "UPDATE bots SET paused = ?1 WHERE id = ?2",
        params![paused, id],
    )?;
    Ok(())
}

pub fn set_limited_until(db: &Connection, id: &str, until: Option<&str>) -> Result<(), RpcError> {
    db.execute(
        "UPDATE bots SET limited_until = ?1 WHERE id = ?2",
        params![until, id],
    )?;
    Ok(())
}

/// The bot's standing instructions, appended to the CLI's own system prompt.
pub fn bot_prompt(bot: &StoredBot) -> String {
    let project = bot
        .project_folder
        .as_ref()
        .map(|folder| format!(" You work in the project folder {folder}."))
        .unwrap_or_default();
    let mut lines = vec![
        format!("You are {}, one of the bots a person runs in Tjommie on their own computer.", bot.name),
        format!("Your home folder is {}.{project}", bot.home_folder),
        "Keep notes.md in your home folder up to date with the person's preferences, important facts and short summaries of finished work, and read it when a new session starts.".to_owned(),
        "Files the person attaches are copied into the inbox folder inside your home folder.".to_owned(),
        "Tjommie's tools: list_bots shows the bots you may ask for help; message_bot sends one a request, and its answer arrives later as a new message, so don't wait for it; share_file shows the person a file; add_routine proposes work on a schedule.".to_owned(),
        "Call share_file for each file you create or deliver so the person can preview and download it. A file path in chat alone does not deliver the file.".to_owned(),
    ];
    if bot.browser {
        lines.push("Your browser tools share a browser with the person through the browser panel in Tjommie. Sites they sign in to there stay signed in. When they take over, ask them to choose Resume bot when finished. The panel has no native file picker; after they resume, use browser_file_upload to supply files or paths: [] to cancel a pending file chooser. Never open a separate browser window or use their personal browser profile.".to_owned());
    }
    let instructions = bot.instructions.trim();
    lines.extend([
        "Reply in plain, friendly language, and keep replies short unless the person asks for detail.".to_owned(),
        String::new(),
        "What the person wants from you:".to_owned(),
        if instructions.is_empty() { "No instructions yet. Ask what they need.".to_owned() } else { instructions.to_owned() },
    ]);
    lines.join("\n")
}

/// ponytail: SQLite lower() folds ASCII only, so "Ñame" and "ñame" count as different names (as in bots.ts).
fn assert_name_free(db: &Connection, name: &str, except: Option<&str>) -> Result<(), RpcError> {
    let taken = db
        .query_row(
            "SELECT id FROM bots WHERE lower(name) = lower(?1) AND id IS NOT ?2",
            params![name, except],
            |_| Ok(()),
        )
        .optional()?;
    match taken {
        Some(()) => Err(RpcError::new(
            ErrorCode::NameTaken,
            format!("There's already a bot called {name}"),
        )),
        None => Ok(()),
    }
}

/// At or inside `base`, comparing whole path parts without regard to case, as `path.relative` does on Windows.
fn at_or_inside(base: &Path, target: &Path) -> bool {
    let parts = |path: &Path| {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
    };
    parts(target).starts_with(&parts(base))
}

/// An existing absolute folder outside the managed bots folder that doesn't contain the data root, checked by its
/// configured path and by its real path, so junction aliases count too.
fn check_project_folder(
    folder: Option<String>,
    bots_dir: &Path,
) -> Result<Option<String>, RpcError> {
    let Some(folder) = folder else {
        return Ok(None);
    };
    let path = Path::new(&folder);
    if !path.is_absolute() || !path.is_dir() {
        return Err(bad_params("The project folder must be an existing folder"));
    }
    let real = fs::canonicalize(path)?;
    let managed = at_or_inside(&std::path::absolute(bots_dir)?, &std::path::absolute(path)?)
        || (bots_dir.exists() && at_or_inside(&fs::canonicalize(bots_dir)?, &real));
    if managed {
        return Err(bad_params(
            "The project folder must be outside the managed bots folder",
        ));
    }
    let data_root = bots_dir.parent().unwrap_or(bots_dir);
    if at_or_inside(
        &std::path::absolute(path)?,
        &std::path::absolute(data_root)?,
    ) || at_or_inside(&real, &fs::canonicalize(data_root)?)
    {
        return Err(bad_params(
            "Pick a folder that doesn't contain Tjommie's own data folder.",
        ));
    }
    Ok(Some(folder))
}

/// Each target once, in the order given.
fn unique(targets: &[String]) -> Vec<&String> {
    let mut seen = HashSet::new();
    targets
        .iter()
        .filter(|target| seen.insert(*target))
        .collect()
}

fn set_links(db: &Connection, bot_id: &str, targets: &[String]) -> Result<(), RpcError> {
    validate_links(db, bot_id, targets)?;
    db.execute("DELETE FROM bot_links WHERE bot_id = ?1", [bot_id])?;
    for target in unique(targets) {
        db.execute(
            "INSERT INTO bot_links (bot_id, target_bot_id) VALUES (?1, ?2)",
            [bot_id, target],
        )?;
    }
    Ok(())
}

fn validate_links(db: &Connection, bot_id: &str, targets: &[String]) -> Result<(), RpcError> {
    for target in unique(targets) {
        if target == bot_id {
            return Err(bad_params("A bot can't message itself"));
        }
        if db
            .query_row("SELECT 1 FROM bots WHERE id = ?1", [target], |_| Ok(()))
            .optional()?
            .is_none()
        {
            return Err(RpcError::new(
                ErrorCode::NotFound,
                "Can message: no such bot",
            ));
        }
    }
    Ok(())
}

/// Bot settings shows the start of the bot's own notes: the first 4000 UTF-16 units, as `slice(0, 4000)` keeps.
fn notes_preview(home: &str) -> String {
    let Ok(bytes) = fs::read(Path::new(home).join("notes.md")) else {
        return String::new();
    };
    let mut units = 0;
    String::from_utf8_lossy(&bytes)
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 4000
        })
        .collect()
}

/// The `bots.*` methods. Steps that need later tasks are noted where they go (daemon.ts:437-550).
pub fn register(handlers: Handlers, context: &Context) -> Handlers {
    let deleting: Arc<Mutex<HashSet<String>>> = Arc::default();
    let (list, create, update, delete, pause, show, notes) = (
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
        context.clone(),
    );
    handlers
        .on::<method::BotsList, _>(move |_, _| {
            let bots = list.with_db(|db| {
                Ok(list_stored_bots(db)?
                    .into_iter()
                    .map(|bot| list.bot_view.to_bot(db, bot))
                    .collect::<Vec<_>>())
            });
            async move { bots }
        })
        .on::<method::BotsCreate, _>(move |input, _| {
            // Afterwards T2.2 refreshes folder protection and T3.5 tells the runner setup changed.
            let bot = create.with_db(|db| {
                let requested = input.account_id.clone().flatten();
                create
                    .accounts
                    .check_available(db, input.provider, requested.as_deref())?;
                let stored = create_bot(db, &create.bots_dir, &input)?;
                Ok(create.bot_view.to_bot(db, stored))
            });
            if let Ok(bot) = &bot {
                create.events.send(Event::BotUpdated(bot.clone()));
            }
            async move { bot }
        })
        .on::<method::BotsUpdate, _>(move |params, _| {
            // M5 refuses while the bot has a project task running. On a provider or account change, T3.5 stops and holds
            // the bot and T2.2 refreshes its folder protection first; M6 closes its browser when the browser is turned off.
            let bot = update.with_db(|db| {
                let (before, patch) = (get_stored_bot(db, &params.id)?, &params.patch);
                let provider_changed = patch
                    .provider
                    .is_some_and(|provider| provider != before.provider);
                let account_changed = patch
                    .account_id
                    .as_ref()
                    .is_some_and(|account| *account != before.account_id);
                if provider_changed || account_changed {
                    let kept = if provider_changed {
                        None
                    } else {
                        before.account_id
                    };
                    let requested = patch.account_id.clone().flatten().or(kept);
                    let provider = patch.provider.unwrap_or(before.provider);
                    update
                        .accounts
                        .check_available(db, provider, requested.as_deref())?;
                }
                let stored = update_bot(db, &params.id, &params.patch)?;
                Ok(update.bot_view.to_bot(db, stored))
            });
            if let Ok(bot) = &bot {
                update.events.send(Event::BotUpdated(bot.clone()));
            }
            async move { bot }
        })
        .on::<method::BotsDelete, _>(move |Id { id }, _| {
            // M5 refuses while the bot is in a project or task. Before the row goes, T3.5 closes its session, M6 deletes its
            // browser profile and M4 captures its exchanges; afterwards M6 cleans up unused shared files.
            let deleted = (|| {
                delete.with_db(|db| get_stored_bot(db, &id))?;
                if !deleting
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id.clone())
                {
                    return Err(RpcError::new(
                        ErrorCode::Busy,
                        "The bot is already being deleted. Try again shortly.",
                    ));
                }
                let deleted = delete.with_db(|db| delete_bot(db, &id));
                deleting
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                deleted?;
                delete.events.send(Event::BotDeleted(Id { id }));
                Ok(Empty {})
            })();
            async move { deleted }
        })
        .on::<method::BotsPause, _>(move |params, _| {
            // M6 cancels the bot's browser tools when it pauses; T3.5 runs its queued work when it resumes.
            let bot = pause.with_db(|db| {
                set_paused(db, &params.id, params.paused)?;
                let stored = get_stored_bot(db, &params.id)?;
                Ok(pause.bot_view.to_bot(db, stored))
            });
            if let Ok(bot) = &bot {
                pause.events.send(Event::BotUpdated(bot.clone()));
            }
            async move { bot }
        })
        .on::<method::BotsShowBrowser, _>(move |Id { id }, _| {
            let shown = show.with_db(|db| get_stored_bot(db, &id)).and_then(|bot| {
                if !bot.browser {
                    return Err(bad_params("This bot's browser is off"));
                }
                // M6: the desktop host opens the native browser.
                Err::<Empty, _>(RpcError::new(
                    ErrorCode::BadState,
                    "The native browser needs the Tjommie desktop app.",
                ))
            });
            async move { shown }
        })
        .on::<method::BotsNotes, _>(move |Id { id }, _| {
            let text = notes
                .with_db(|db| get_stored_bot(db, &id))
                .map(|bot| Notes {
                    text: notes_preview(&bot.home_folder),
                });
            async move { text }
        })
}
