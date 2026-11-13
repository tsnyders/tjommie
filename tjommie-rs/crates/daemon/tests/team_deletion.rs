//! Ports `team.deletion.test.ts`: the two-phase bot deletion. `bot_deleting` captures what a deletion will settle, and
//! `bot_deleted` settles it once the bot's row is gone. Each test runs in a fresh fixture, as the TypeScript `beforeEach`
//! does, and the observer is a second connection to the same database file.
//!
//! What the TypeScript tests spy on, and what replaces it here:
//! - `vi.mock("node:fs")` with a `renameSync` that throws once (the busy-folder test): a file in the bot's home folder is
//!   held open with no sharing, so the folder rename that `delete_bot` makes really fails. That needs Windows share modes,
//!   so the test is `#[cfg(windows)]`.
//! - `makeTeam`'s `stopItem`: [`checks`] runs before the fixture records each stop. It checks, through the observer, that
//!   the item is still running and its exchange already failed.
//! - `makeTeam(undefined, observe)`: [`observe_broadcasts`] replaces the fixture's team (a public field) with one whose
//!   broadcast runs a check after recording each event.
//! - `stateOf: () => "idle"`: the fixture's host reports every bot idle, so nothing extra is needed.

mod team_support;
use team_support::*;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use daemon::bots::{StoredBot, delete_bot, get_stored_bot};
use daemon::queue::{
    NewWork, WorkItem, WorkSource, WorkStatus, enqueue, get_work_item, new_chain, set_status,
};
use daemon::runner::{TurnOutcome, TurnResult};
use daemon::server::RpcError;
use daemon::team::{HostAccess, Team, TeamDeps, TeamHost, get_exchange};
use protocol::Event;
use protocol::model::{Bot, ExchangeStatus, TEAM_ROOM_ID};
use rusqlite::Connection;
use rusqlite::types::Value as SqlValue;
use serde_json::{Value, json};

/// The hooks the TypeScript `makeTeam` callbacks become: `stopItem`'s checks, run before the fixture records a stop.
#[derive(Clone)]
struct Hooks {
    on_stop: Arc<dyn Fn(&str) + Send + Sync>,
}

/// The fixture's host, with [`Hooks`] applied on the way through.
struct Checked<'a> {
    inner: &'a mut dyn TeamHost,
    hooks: Hooks,
}

impl TeamHost for Checked<'_> {
    fn db(&self) -> &Connection {
        self.inner.db()
    }

    fn deliver(&mut self, work: &WorkItem) -> Result<(), String> {
        self.inner.deliver(work)
    }

    fn stop_item(&mut self, item_id: &str, notice: &str) {
        (*self.hooks.on_stop)(item_id);
        self.inner.stop_item(item_id, notice);
    }

    fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        self.inner.current_work(bot_id)
    }

    fn to_bot(&mut self, bot: StoredBot) -> Bot {
        self.inner.to_bot(bot)
    }

    fn effective_folder(&self, bot_id: &str) -> Result<Option<String>, RpcError> {
        self.inner.effective_folder(bot_id)
    }
}

/// The way the MCP tools reach the fixture's host: through [`Checked`].
struct Access {
    host: Arc<Mutex<FakeHost>>,
    hooks: Hooks,
}

impl HostAccess for Access {
    fn with_host(&self, work: &mut dyn FnMut(&mut dyn TeamHost)) {
        let mut guard = self.host.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = Checked {
            inner: &mut *guard,
            hooks: self.hooks.clone(),
        };
        work(&mut host);
    }
}

/// `tool.run(botId, args)` for the `message_bot` tool, through [`Checked`].
fn call_with(fx: &Fixture, hooks: &Hooks, bot: &str, name: &str, arguments: Value) -> Result<String, String> {
    let access = Access {
        host: fx.host.clone(),
        hooks: hooks.clone(),
    };
    let tools = fx.team.tools(Arc::new(access));
    let tool = tools
        .iter()
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("no tool {name}"));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(tool.call(bot, &arguments))
}

/// Runs `work` with the team and the fixture's host, through [`Checked`].
fn with_team<T>(
    fx: &Fixture,
    hooks: &Hooks,
    work: impl FnOnce(&Team, &mut dyn TeamHost) -> T,
) -> T {
    fx.team_call(|team, host| {
        let mut host = Checked {
            inner: host,
            hooks: hooks.clone(),
        };
        work(team, &mut host)
    })
}

/// Replaces the team with one whose broadcast is `broadcast`. The fixture's logs and shared-files folder stay.
fn team_with(
    fx: &mut Fixture,
    broadcast: impl Fn(Event) -> Result<(), String> + Send + Sync + 'static,
) {
    let logs = fx.logs.clone();
    let shared_files = fx.shared_files();
    fx.team = Arc::new(Team::new(TeamDeps {
        broadcast: Arc::new(broadcast),
        log: Arc::new(move |context: &str, error: &str| {
            recorded(&logs).push((context.to_owned(), error.to_owned()));
        }),
        shared_files,
        attachments: None,
    }));
}

/// `makeTeam(undefined, observe)`: a team that records each event, then runs `observe`, as the TypeScript broadcast does.
fn observe_broadcasts(fx: &mut Fixture, observe: impl Fn() + Send + Sync + 'static) {
    let events = fx.events.clone();
    team_with(fx, move |event| {
        recorded(&events).push(event);
        observe();
        Ok(())
    });
}

/// The TypeScript tests' `observer`: a second connection to the fixture's database file.
fn connect(fx: &Fixture) -> Connection {
    daemon::db::open(&fx.root.path().join("t.db")).unwrap()
}

/// `makeTeam`'s `stopItem` checks: the item is still running and its exchange already failed, as the observer sees them.
/// The fixture's `stop_item` records the stop after the check.
fn checks(fx: &Fixture) -> Hooks {
    let observer = Arc::new(Mutex::new(connect(fx)));
    Hooks {
        on_stop: Arc::new(move |item_id: &str| {
            let db = observer.lock().unwrap_or_else(PoisonError::into_inner);
            let item = get_work_item(&db, item_id).unwrap();
            assert_eq!(item.status, WorkStatus::Running, "the stopped item is running");
            let exchange = get_exchange(&db, exchange_id(&item)).unwrap();
            assert_eq!(exchange.status, ExchangeStatus::Failed, "its exchange is failed");
        }),
    }
}

/// `beforeEach`: writer, researcher and planner. Researcher and planner may message writer, and writer may message
/// researcher.
fn setup() -> (Fixture, StoredBot, StoredBot, StoredBot) {
    let fx = fixture();
    let writer = fx.bot("Writer", "blue", "circle", "Write summaries.", "codex", "gpt-5.6-sol");
    let researcher = fx.bot("Researcher", "teal", "droplet", "Find sources.", "claude", "sonnet");
    let planner = fx.bot("Planner", "blue", "circle", "Make plans.", "codex", "gpt-5.6-sol");
    let researcher = fx.set_can_message(&researcher.id, &[writer.id.as_str()]);
    let planner = fx.set_can_message(&planner.id, &[writer.id.as_str()]);
    let writer = fx.set_can_message(&writer.id, &[researcher.id.as_str()]);
    (fx, writer, researcher, planner)
}

fn exchange_id(work: &WorkItem) -> &str {
    work.exchange_id.as_deref().expect("Missing exchange id")
}

/// `request(from, to, conversationId)`: `from` asks `to` through the tool, and the request as it was delivered comes back.
fn request(
    fx: &Fixture,
    hooks: &Hooks,
    from: &StoredBot,
    to: &StoredBot,
    conversation_id: &str,
) -> WorkItem {
    let work = fx.db(|db| {
        let chain_id = new_chain(db).unwrap();
        enqueue(
            db,
            &NewWork {
                conversation_id: Some(conversation_id.to_owned()),
                chain_id: Some(chain_id),
                ..NewWork::new(&from.id, WorkSource::You, "Ask for help.")
            },
        )
        .unwrap()
    });
    fx.current.lock().unwrap().insert(from.id.clone(), work);
    call_with(
        fx,
        hooks,
        &from.id,
        "message_bot",
        json!({ "bot": to.name.as_str(), "message": format!("Help {}.", from.name) }),
    )
    .unwrap();
    let queued = recorded(&fx.delivered).last().cloned();
    let queued = queued.expect("No exchange request was delivered");
    assert!(queued.exchange_id.is_some(), "No exchange request was delivered");
    queued
}

fn clear_effects(fx: &Fixture) {
    recorded(&fx.delivered).clear();
    fx.clear_events();
}

/// Every row of `sql`, as its column values in order.
fn rows(db: &Connection, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut statement = db.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|index| row.get::<_, SqlValue>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

/// The TypeScript `state()`: every row of the tables a deletion touches, in insertion order.
fn state(db: &Connection) -> Vec<(&'static str, Vec<Vec<SqlValue>>)> {
    ["bots", "chains", "conversations", "exchanges", "messages", "work_items"]
        .into_iter()
        .map(|table| (table, rows(db, &format!("SELECT * FROM {table} ORDER BY rowid"))))
        .collect()
}

#[derive(Debug)]
struct Reply {
    bot_id: String,
    conversation_id: String,
    exchange_id: String,
    text: String,
}

/// The `bot_reply` work items, in insertion order.
fn replies(db: &Connection) -> Vec<Reply> {
    let mut statement = db
        .prepare("SELECT bot_id, conversation_id, exchange_id, text FROM work_items WHERE source = 'bot_reply' ORDER BY rowid")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok(Reply {
                bot_id: row.get(0)?,
                conversation_id: row.get(1)?,
                exchange_id: row.get(2)?,
                text: row.get(3)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn stopped_ids(fx: &Fixture) -> Vec<String> {
    recorded(&fx.stopped)
        .iter()
        .map(|(id, _)| id.clone())
        .collect()
}

mod two_phase_bot_deletion {
    use super::*;

    #[test]
    fn captures_incoming_and_outgoing_exchanges_without_changing_state_or_publishing_effects() {
        let (fx, writer, researcher, _) = setup();
        let hooks = checks(&fx);
        let incoming = request(&fx, &hooks, &researcher, &writer, &researcher.id);
        let outgoing = request(&fx, &hooks, &writer, &researcher, &writer.id);
        clear_effects(&fx);
        let before = fx.db(state);
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        // TypeScript's `toBeDefined`: the call returned a handle, and it is for this bot.
        assert_eq!(handle.bot.id, writer.id);
        assert_eq!(fx.db(state), before);
        assert_eq!(
            fx.db(|db| get_exchange(db, exchange_id(&incoming)).unwrap().status),
            ExchangeStatus::Waiting
        );
        assert_eq!(
            fx.db(|db| get_exchange(db, exchange_id(&outgoing)).unwrap().status),
            ExchangeStatus::Waiting
        );
        assert!(recorded(&fx.delivered).is_empty());
        assert!(fx.event_names().is_empty());
    }

    /// TypeScript makes `renameSync` throw once. Here a file in the bot's home folder stays open with no sharing, so
    /// the folder rename that `delete_bot` makes really fails.
    #[cfg(windows)]
    #[test]
    fn keeps_a_busy_bot_and_its_exchanges_untouched_when_deletion_never_completes() {
        use std::os::windows::fs::OpenOptionsExt;

        let (fx, writer, researcher, _) = setup();
        let hooks = checks(&fx);
        let pending = request(&fx, &hooks, &researcher, &writer, &researcher.id);
        clear_effects(&fx);
        let before = fx.db(state);
        with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        let busy = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(Path::new(&writer.home_folder).join("notes.md"))
            .unwrap();
        let error = fx.db(|db| delete_bot(db, &writer.id)).unwrap_err();
        assert!(error.message.contains("The bot folder is busy"), "{}", error.message);
        assert_eq!(fx.db(|db| get_stored_bot(db, &writer.id).unwrap()), writer);
        assert!(Path::new(&writer.home_folder).exists());
        assert_eq!(
            fx.db(|db| get_exchange(db, exchange_id(&pending)).unwrap().status),
            ExchangeStatus::Waiting
        );
        assert_eq!(fx.db(state), before);
        assert!(recorded(&fx.delivered).is_empty());
        assert!(fx.event_names().is_empty());
        drop(busy);
    }

    #[test]
    fn refuses_to_finalize_a_handle_while_its_bot_still_exists() {
        let (fx, writer, researcher, _) = setup();
        let hooks = checks(&fx);
        request(&fx, &hooks, &researcher, &writer, &researcher.id);
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        clear_effects(&fx);
        let before = fx.db(state);
        assert!(with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).is_err());
        assert_eq!(fx.db(state), before);
        assert!(recorded(&fx.delivered).is_empty());
        assert!(fx.event_names().is_empty());
    }

    #[test]
    fn fails_every_incoming_exchange_after_deletion_and_tells_each_asker_once_across_repeated_handles() {
        let (fx, writer, researcher, planner) = setup();
        let hooks = checks(&fx);
        let first = request(&fx, &hooks, &researcher, &writer, TEAM_ROOM_ID);
        let second = request(&fx, &hooks, &planner, &writer, &planner.id);
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        let second_handle =
            with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        fx.db(|db| delete_bot(db, &writer.id).unwrap());
        clear_effects(&fx);
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap();
        for work in [&first, &second] {
            let exchange = fx.db(|db| get_exchange(db, exchange_id(work)).unwrap());
            assert_eq!(exchange.status, ExchangeStatus::Failed);
            assert_eq!(exchange.reply.as_deref(), Some("Writer was deleted"));
        }
        let reply_rows = fx.db(replies);
        assert_eq!(reply_rows.len(), 2, "{reply_rows:?}");
        assert_eq!(reply_rows[0].bot_id, researcher.id);
        assert_eq!(reply_rows[0].conversation_id, TEAM_ROOM_ID);
        assert_eq!(reply_rows[0].exchange_id, exchange_id(&first));
        assert!(reply_rows[0].text.contains("Writer was deleted"), "{}", reply_rows[0].text);
        assert_eq!(reply_rows[1].bot_id, planner.id);
        assert_eq!(reply_rows[1].conversation_id, planner.id);
        assert_eq!(reply_rows[1].exchange_id, exchange_id(&second));
        assert!(reply_rows[1].text.contains("Writer was deleted"), "{}", reply_rows[1].text);
        assert_eq!(recorded(&fx.delivered).len(), 2);
        let after = fx.db(state);
        let published = fx.event_names().len();
        assert!(published > 0);
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap();
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &second_handle)).unwrap();
        assert_eq!(fx.db(state), after);
        assert_eq!(fx.event_names().len(), published);
        assert_eq!(recorded(&fx.delivered).len(), 2);
    }

    #[test]
    fn recovers_captured_exchanges_after_their_rows_cascade_away_with_the_deleted_bot() {
        let (fx, writer, researcher, _) = setup();
        let hooks = checks(&fx);
        let pending = request(&fx, &hooks, &researcher, &writer, &researcher.id);
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        fx.db(|db| {
            db.execute_batch("CREATE TRIGGER cascade_exchanges AFTER DELETE ON bots BEGIN DELETE FROM exchanges WHERE from_bot_id = OLD.id OR to_bot_id = OLD.id; END")
                .unwrap()
        });
        fx.db(|db| delete_bot(db, &writer.id).unwrap());
        let gone = fx.db(|db| get_exchange(db, exchange_id(&pending))).unwrap_err();
        assert!(gone.message.contains("No such exchange"), "{}", gone.message);
        clear_effects(&fx);
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap();
        let exchange = fx.db(|db| get_exchange(db, exchange_id(&pending)).unwrap());
        assert_eq!(exchange.status, ExchangeStatus::Failed);
        assert_eq!(exchange.reply.as_deref(), Some("Writer was deleted"));
        assert_eq!(exchange.from_bot_id, researcher.id);
        assert_eq!(exchange.to_bot_id, writer.id);
        let reply_rows = fx.db(replies);
        assert_eq!(reply_rows.len(), 1, "{reply_rows:?}");
        assert_eq!(reply_rows[0].bot_id, researcher.id);
        assert_eq!(reply_rows[0].exchange_id, exchange_id(&pending));
        assert!(reply_rows[0].text.contains("Writer was deleted"), "{}", reply_rows[0].text);
        assert_eq!(recorded(&fx.delivered).len(), 1);
    }

    #[test]
    fn fails_queued_requests_and_asks_the_runner_to_stop_running_requests_after_deletion_commits() {
        let (fx, writer, researcher, _) = setup();
        let hooks = checks(&fx);
        let queued = request(&fx, &hooks, &writer, &researcher, &writer.id);
        let running = request(&fx, &hooks, &writer, &researcher, &writer.id);
        fx.db(|db| set_status(db, &running.id, WorkStatus::Running).unwrap());
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        fx.db(|db| delete_bot(db, &writer.id).unwrap());
        clear_effects(&fx);
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap();
        for work in [&queued, &running] {
            assert_eq!(
                fx.db(|db| get_exchange(db, exchange_id(work)).unwrap().status),
                ExchangeStatus::Failed
            );
        }
        assert_eq!(
            fx.db(|db| get_work_item(db, &queued.id).unwrap().status),
            WorkStatus::Failed
        );
        assert_eq!(
            fx.db(|db| get_work_item(db, &running.id).unwrap().status),
            WorkStatus::Running
        );
        assert_eq!(stopped_ids(&fx), std::slice::from_ref(&running.id));
        assert!(fx.db(replies).is_empty());
        assert!(recorded(&fx.delivered).is_empty());
        let after = fx.db(state);
        let published = fx.event_names().len();
        with_team(&fx, &hooks, |team, host| {
            team.turn_ended(
                host,
                &running,
                &TurnResult {
                    outcome: TurnOutcome::Done,
                    text: Some("A late answer.".to_owned()),
                    text_message_id: None,
                    error: None,
                },
            )
        })
        .unwrap();
        assert_eq!(fx.db(state), after);
        assert_eq!(fx.event_names().len(), published);
    }

    #[test]
    fn rolls_back_all_failures_and_notices_when_a_reply_insert_fails_and_keeps_the_handle_reusable() {
        let (fx, writer, researcher, planner) = setup();
        let hooks = checks(&fx);
        let first = request(&fx, &hooks, &researcher, &writer, &researcher.id);
        let second = request(&fx, &hooks, &planner, &writer, &planner.id);
        let running = request(&fx, &hooks, &writer, &researcher, &writer.id);
        fx.db(|db| set_status(db, &running.id, WorkStatus::Running).unwrap());
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        fx.db(|db| delete_bot(db, &writer.id).unwrap());
        clear_effects(&fx);
        let before = fx.db(state);
        fx.db(|db| {
            db.execute_batch(&format!(
                "CREATE TRIGGER reject_deleted_reply BEFORE INSERT ON work_items WHEN NEW.source = 'bot_reply' AND NEW.bot_id = '{}' BEGIN SELECT RAISE(ABORT, 'deletion reply failed'); END",
                planner.id
            ))
            .unwrap()
        });
        let error = with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap_err();
        assert!(error.message.contains("deletion reply failed"), "{}", error.message);
        assert_eq!(fx.db(state), before);
        assert!(fx.event_names().is_empty());
        assert!(recorded(&fx.delivered).is_empty());
        assert!(stopped_ids(&fx).is_empty());
        fx.db(|db| db.execute_batch("DROP TRIGGER reject_deleted_reply").unwrap());
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap();
        for work in [&first, &second] {
            assert_eq!(
                fx.db(|db| get_exchange(db, exchange_id(work)).unwrap().status),
                ExchangeStatus::Failed
            );
        }
        assert_eq!(fx.db(replies).len(), 2);
        assert_eq!(recorded(&fx.delivered).len(), 2);
        assert_eq!(stopped_ids(&fx), std::slice::from_ref(&running.id));
        assert_eq!(
            fx.db(|db| get_work_item(db, &running.id).unwrap().status),
            WorkStatus::Running
        );
    }

    #[test]
    fn broadcasts_deletion_results_only_after_all_failures_and_notifications_commit() {
        let (mut fx, writer, researcher, planner) = setup();
        let hooks = checks(&fx);
        let first = request(&fx, &hooks, &researcher, &writer, &researcher.id);
        let second = request(&fx, &hooks, &planner, &writer, &planner.id);
        let watch = Arc::new(Mutex::new(connect(&fx)));
        let observed: Recorded<(Vec<ExchangeStatus>, usize, i64)> = Arc::default();
        let (seen, first_id, second_id) = (
            observed.clone(),
            exchange_id(&first).to_owned(),
            exchange_id(&second).to_owned(),
        );
        observe_broadcasts(&mut fx, move || {
            let db = watch.lock().unwrap_or_else(PoisonError::into_inner);
            let statuses = vec![
                get_exchange(&db, &first_id).unwrap().status,
                get_exchange(&db, &second_id).unwrap().status,
            ];
            let reply_count = replies(&db).len();
            let notices: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE kind = 'notice' AND conversation_id = ?1",
                    [TEAM_ROOM_ID],
                    |row| row.get(0),
                )
                .unwrap();
            recorded(&seen).push((statuses, reply_count, notices));
        });
        let handle = with_team(&fx, &hooks, |team, host| team.bot_deleting(host, &writer.id)).unwrap();
        fx.db(|db| delete_bot(db, &writer.id).unwrap());
        clear_effects(&fx);
        with_team(&fx, &hooks, |team, host| team.bot_deleted(host, &handle)).unwrap();
        let published = fx.event_names().len();
        assert!(published > 0);
        assert_eq!(
            *recorded(&observed),
            vec![(vec![ExchangeStatus::Failed, ExchangeStatus::Failed], 2, 2); published]
        );
    }
}
