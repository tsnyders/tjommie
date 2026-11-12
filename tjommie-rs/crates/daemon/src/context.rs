//! What the method handlers share, and the handler table.
//!
//! Storage is the `rusqlite::Connection` from [`crate::db::open`], and these modules run the same SQL as
//! their TypeScript originals against it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use protocol::model::Bot;
use rusqlite::{Connection, OptionalExtension};

use crate::account_handlers::{self, Accounts};
use crate::bots::{BotStatus, StoredBot, bot_state_of};
use crate::ids::now_iso;
use crate::server::{Broadcaster, Handlers, RpcError};
use crate::{bots, diagnostics, google, settings};

/// The daemon's database. One connection: SQLite runs one write at a time anyway.
pub type Db = Arc<Mutex<Connection>>;

/// Turns a stored bot into the wire `Bot`, with its state and setup reason. T3.5's runner, with T3.4's accounts, replaces
/// [`StoredState`] (`runner.ts:194-214`).
pub trait BotView: Send + Sync {
    fn to_bot(&self, db: &Connection, bot: StoredBot) -> Bot;
}

/// A bot as the database alone knows it: signed in and never busy, but paused, limited or waiting on a routine proposal.
pub struct StoredState;

impl BotView for StoredState {
    fn to_bot(&self, db: &Connection, bot: StoredBot) -> Bot {
        let waiting = db
            .query_row("SELECT 1 FROM approvals WHERE bot_id = ?1 AND kind = 'routine' AND status = 'pending' LIMIT 1", [&bot.id], |_| Ok(()))
            .optional()
            .is_ok_and(|row| row.is_some());
        let status = BotStatus {
            signed_in: true,
            setup_reason: None,
            paused: bot.paused,
            limited_until: bot.limited_until.clone(),
            waiting,
            busy: false,
        };
        let state = bot_state_of(&status, &now_iso());
        bot.into_bot(state, None)
    }
}

#[derive(Clone)]
pub struct Context {
    pub db: Db,
    /// The data root's `bots` folder, where each bot's home folder lives.
    pub bots_dir: PathBuf,
    pub events: Broadcaster,
    pub bot_view: Arc<dyn BotView>,
    /// The desktop app's version, from the packaged `build-info.json`.
    pub app_version: Option<String>,
    pub accounts: Accounts,
}

impl Context {
    /// Runs `work` with the database. A handler that panicked mid-write left nothing half-applied: SQLite rolled it back.
    pub fn with_db<T>(
        &self,
        work: impl FnOnce(&Connection) -> Result<T, RpcError>,
    ) -> Result<T, RpcError> {
        work(&self.db.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The methods this daemon answers so far. Later tasks register theirs here too.
pub fn handlers(context: &Context) -> Handlers {
    let handlers = settings::register(Handlers::default(), context);
    let handlers = bots::register(handlers, context);
    let handlers = diagnostics::register(handlers, context);
    let handlers = account_handlers::register(handlers, context);
    google::register(handlers)
}
