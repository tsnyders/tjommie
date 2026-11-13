//! What the method handlers share, and the handler table.
//!
//! Storage is the `rusqlite::Connection` from [`crate::db::open`], and these modules run the same SQL as
//! their TypeScript originals against it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use protocol::model::Bot;
use rusqlite::{Connection, OptionalExtension};

use crate::account_handlers::{self, Accounts};
use crate::bots::{BotStatus, StoredBot, bot_state_of};
use crate::ids::now_iso;
use crate::m5::M5;
use crate::m6::M6;
use crate::remote::Remote;
use crate::runner::Runner;
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

/// A [`BotView`] bound after it is handed out: the runner needs the accounts, and the accounts' feed needs the runner's
/// view. Until bound it is [`StoredState`].
#[derive(Clone, Default)]
pub struct LateBotView(Arc<OnceLock<Arc<dyn BotView>>>);

impl LateBotView {
    /// Binds the view. Set once; a second call is ignored.
    pub fn bind(&self, view: Arc<dyn BotView>) {
        let _ = self.0.set(view);
    }
}

impl BotView for LateBotView {
    fn to_bot(&self, db: &Connection, bot: StoredBot) -> Bot {
        match self.0.get() {
            Some(view) => view.to_bot(db, bot),
            None => StoredState.to_bot(db, bot),
        }
    }
}

/// What startup binds after the handlers' context exists, because each needs the handlers' own services. Unbound (as in
/// the handler tests), each is absent and the handler that would call it does what it does without it.
#[derive(Clone, Default)]
pub struct Late {
    pub runner: Arc<OnceLock<Runner>>,
    pub remote: Arc<OnceLock<Arc<Remote>>>,
    pub m6: Arc<OnceLock<Arc<M6>>>,
    /// The project services: a bot in a project, or with a task running, can't be changed or deleted (`daemon.ts:451`,
    /// `:494`).
    pub m5: Arc<OnceLock<Arc<M5>>>,
    /// Bots whose provider or account is changing: the runner holds their work and reports "changing provider"
    /// (`switchingBots`).
    pub switching: Arc<Mutex<HashSet<String>>>,
    /// Bots being deleted (`deletionRequests`). Projects refuse to take one on.
    pub deleting: Arc<Mutex<HashSet<String>>>,
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
    pub late: Late,
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
