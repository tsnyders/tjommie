//! Ports `team.atomicity.test.ts`: the Team's transactions, what each one publishes and when, how a failure rolls back,
//! and the chain quota. Each `it.each` row runs in a fresh fixture, and the row's label is in its assertion messages.
//!
//! What the TypeScript tests spy on, and what replaces it here:
//! - `makeTeam`'s `deliver` and `stateOf`: [`Checked`] wraps the fixture's host. `deliver` runs [`Hooks::on_deliver`]
//!   before the fixture records the work, and `to_bot` derives the state the way `stateOf` does (`waiting` while the bot
//!   has an unanswered routine proposal).
//! - `vi.spyOn(db, "prepare")` (nested transactions): [`Hooks::on_to_bot`]. `Team` calls `TeamHost::to_bot` inside its
//!   transaction, so the hook makes the nested `bot_deleted` call there. `to_bot` returns a `Bot`, not a `Result`, so the
//!   refusal can't fail the outer proposal. The test checks the refusal message and that the refused call wrote nothing.
//! - `vi.spyOn(console, "error")`: the fixture's `logs`, which record each `(context, error)` the team logs.
//! - `makeTeam(undefined, observe)`: [`observe_broadcasts`] and [`team_with`] replace the fixture's team (a public field)
//!   with one whose broadcast runs a check after recording each event.
//! - The TypeScript `observer` database is a second connection to the same file, from [`connect`].

mod team_support;
use team_support::*;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use daemon::approvals::get_approval;
use daemon::bots::{StoredBot, delete_bot};
use daemon::chat::get_message;
use daemon::queue::{NewWork, WorkItem, WorkSource, WorkStatus, enqueue, get_work_item, new_chain};
use daemon::routines::list_routines;
use daemon::runner::{TurnOutcome, TurnResult};
use daemon::server::RpcError;
use daemon::settings::set_settings;
use daemon::team::{HostAccess, Team, TeamDeps, TeamHost, get_exchange};
use protocol::Event;
use protocol::model::{
    Approval, ApprovalDecision, ApprovalStatus, Bot, BotState, ExchangeStatus, SettingsPatch,
};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

/// The fixture's host as the TypeScript test's `makeTeam` callbacks see it, plus the hooks a test sets.
#[derive(Clone)]
struct Hooks {
    /// `deliver`, run before the fixture's host records (or refuses) the work.
    on_deliver: Arc<dyn Fn(&WorkItem) + Send + Sync>,
    /// Run when the team asks for a bot's state, inside the team's transaction when it is in one.
    on_to_bot: Arc<dyn Fn() + Send + Sync>,
}

impl Hooks {
    fn none() -> Hooks {
        Hooks {
            on_deliver: Arc::new(|_: &WorkItem| {}),
            on_to_bot: Arc::new(|| {}),
        }
    }
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
        (*self.hooks.on_deliver)(work);
        self.inner.deliver(work)
    }

    fn stop_item(&mut self, item_id: &str, notice: &str) {
        self.inner.stop_item(item_id, notice);
    }

    fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        self.inner.current_work(bot_id)
    }

    /// `stateOf`: `waiting` while the bot has a routine proposal nobody has answered, `idle` otherwise.
    fn to_bot(&mut self, bot: StoredBot) -> Bot {
        (*self.hooks.on_to_bot)();
        let pending: Option<i64> = self
            .inner
            .db()
            .query_row(
                "SELECT 1 FROM approvals WHERE bot_id = ?1 AND kind = 'routine' AND status = 'pending' LIMIT 1",
                [bot.id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let state = if pending.is_some() {
            BotState::Waiting
        } else {
            BotState::Idle
        };
        bot.into_bot(state, None)
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

/// `tool.run(botId, args)`: `Ok` is the text the bot reads, `Err` the message of a failed call.
fn call_with(
    fx: &Fixture,
    hooks: &Hooks,
    bot: &str,
    name: &str,
    arguments: Value,
) -> Result<String, String> {
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

fn call(fx: &Fixture, bot: &str, name: &str, arguments: Value) -> Result<String, String> {
    call_with(fx, &Hooks::none(), bot, name, arguments)
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

/// `stateCounts()`: the row counts the TypeScript test compares.
fn state_counts(db: &Connection) -> BTreeMap<&'static str, i64> {
    [
        "approvals",
        "chains",
        "conversations",
        "exchanges",
        "messages",
        "routines",
        "work_items",
    ]
    .into_iter()
    .map(|table| {
        let count = db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        (table, count)
    })
    .collect()
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

/// `requests_made` of a chain.
fn request_count(db: &Connection, chain_id: Option<&str>) -> i64 {
    let chain = chain_id.expect("Expected a chain");
    db.query_row(
        "SELECT requests_made FROM chains WHERE id = ?1",
        [chain],
        |row| row.get(0),
    )
    .expect("Missing chain")
}

fn set_current(fx: &Fixture, bot_id: &str, work: WorkItem) {
    fx.current.lock().unwrap().insert(bot_id.to_owned(), work);
}

/// `chatTurn()`: a message from the person to the researcher, which is the researcher's current work.
fn chat_turn(fx: &Fixture, researcher: &StoredBot) -> WorkItem {
    let work = fx.db(|db| {
        let chain_id = new_chain(db).unwrap();
        enqueue(
            db,
            &NewWork {
                chain_id: Some(chain_id),
                ..NewWork::new(&researcher.id, WorkSource::You, "Help me write.")
            },
        )
        .unwrap()
    });
    set_current(fx, &researcher.id, work.clone());
    work
}

/// `beforeEach`: a writer and a researcher, each able to message the other.
fn setup() -> (Fixture, StoredBot, StoredBot) {
    let fx = fixture();
    let writer = fx.bot(
        "Writer",
        "blue",
        "circle",
        "Writes summaries.",
        "codex",
        "gpt-5.6-sol",
    );
    let researcher = fx.bot(
        "Researcher",
        "teal",
        "droplet",
        "Finds sources.",
        "claude",
        "sonnet",
    );
    let researcher = fx.set_can_message(&researcher.id, &[writer.id.as_str()]);
    let writer = fx.set_can_message(&writer.id, &[researcher.id.as_str()]);
    (fx, writer, researcher)
}

/// The routine the TypeScript `propose()` sends.
fn propose(fx: &Fixture, writer: &StoredBot) -> Result<String, String> {
    call(
        fx,
        &writer.id,
        "add_routine",
        json!({ "name": "Morning check", "days": ["mon", "wed"], "times": ["08:30"], "prompt": "Summarize the inbox." }),
    )
}

/// `proposal()`: proposes, then reads the approval the team published.
fn proposal(fx: &Fixture, writer: &StoredBot) -> Approval {
    propose(fx, writer).unwrap();
    let data = fx
        .events_named("approval.added")
        .into_iter()
        .next()
        .expect("the proposal publishes its approval");
    serde_json::from_value(data).unwrap()
}

/// Whether a `bot.updated` event has been published for this bot in this state.
fn bot_updated_with(fx: &Fixture, bot_id: &str, state: &str) -> bool {
    fx.events_named("bot.updated")
        .iter()
        .any(|data| data["id"] == bot_id && data["state"] == state)
}

fn answer(
    fx: &Fixture,
    hooks: &Hooks,
    approval_id: &str,
    decision: ApprovalDecision,
) -> Result<Approval, RpcError> {
    with_team(fx, hooks, |team, host| {
        team.answer_routine_proposal(host, approval_id, decision)
    })
}

/// A message's stored data as JSON: `null` when it has none.
fn card_data(db: &Connection, message_id: &str) -> Value {
    serde_json::to_value(get_message(db, message_id).unwrap().data).unwrap()
}

fn malformed_card() -> Value {
    json!({ "approvalId": 7, "detail": false, "retained": "original value" })
}

mod routine_transactions {
    use super::*;

    #[test]
    fn caps_pending_proposals_at_five_per_bot_and_frees_a_slot_when_one_is_answered() {
        let (fx, writer, researcher) = setup();
        let first = proposal(&fx, &writer);
        for _ in 1..5 {
            propose(&fx, &writer).unwrap();
        }
        let before = fx.db(state_counts);
        let published = fx.event_json().len();
        let error = propose(&fx, &writer).unwrap_err();
        assert!(
            error.contains("You already have 5 routine proposals waiting for the person. Wait for them to answer before proposing another."),
            "{error}"
        );
        assert_eq!(fx.db(state_counts), before);
        assert_eq!(fx.event_json().len(), published);
        let other = call(
            &fx,
            &researcher.id,
            "add_routine",
            json!({ "name": "Other bot", "days": ["mon"], "times": ["08:30"], "prompt": "Check" }),
        )
        .unwrap();
        assert!(other.starts_with("Proposed"), "{other}");
        answer(&fx, &Hooks::none(), &first.id, ApprovalDecision::Deny).unwrap();
        let again = propose(&fx, &writer).unwrap();
        assert!(again.starts_with("Proposed"), "{again}");
        let pending: i64 = fx.db(|db| {
            db.query_row(
                "SELECT COUNT(*) FROM approvals WHERE bot_id = ?1 AND kind = 'routine' AND status = 'pending'",
                [writer.id.as_str()],
                |row| row.get(0),
            )
            .unwrap()
        });
        assert_eq!(pending, 5);
    }

    #[test]
    fn rolls_back_the_proposal_card_when_creating_its_approval_fails() {
        let (fx, writer, _) = setup();
        let before = fx.db(state_counts);
        fx.db(|db| {
            db.execute_batch("CREATE TRIGGER reject_proposal BEFORE INSERT ON approvals BEGIN SELECT RAISE(ABORT, 'approval insert failed'); END")
                .unwrap()
        });
        let error = propose(&fx, &writer).unwrap_err();
        assert!(error.contains("approval insert failed"), "{error}");
        assert_eq!(fx.db(state_counts), before);
        assert!(fx.event_names().is_empty());
    }

    #[test]
    fn publishes_the_proposal_only_after_both_card_and_approval_commit() {
        let (mut fx, writer, _) = setup();
        let observer = Arc::new(Mutex::new(connect(&fx)));
        let observed: Recorded<(i64, i64)> = Arc::default();
        let (watch, seen) = (observer.clone(), observed.clone());
        observe_broadcasts(&mut fx, move || {
            let db = watch.lock().unwrap_or_else(PoisonError::into_inner);
            let counts = state_counts(&db);
            recorded(&seen).push((counts["messages"], counts["approvals"]));
        });
        proposal(&fx, &writer);
        assert_eq!(
            fx.event_names(),
            ["message.added", "approval.added", "notify", "bot.updated"]
        );
        assert_eq!(*recorded(&observed), vec![(1, 1); 4]);
    }

    #[test]
    fn rolls_back_approval_routine_and_card_when_a_stage_fails() {
        let stages = [
            (
                "routine creation",
                "CREATE TRIGGER reject_answer BEFORE INSERT ON routines BEGIN SELECT RAISE(ABORT, 'answer failed'); END",
            ),
            (
                "routine scheduling",
                "CREATE TRIGGER reject_answer BEFORE UPDATE ON routines BEGIN SELECT RAISE(ABORT, 'answer failed'); END",
            ),
            (
                "card update",
                "CREATE TRIGGER reject_answer BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'answer failed'); END",
            ),
        ];
        for (stage, trigger) in stages {
            let (fx, writer, _) = setup();
            let pending = proposal(&fx, &writer);
            let before = fx.db(state_counts);
            let card = fx.db(|db| get_message(db, &pending.message_id).unwrap());
            fx.clear_events();
            fx.db(|db| db.execute_batch(trigger).unwrap());
            let error =
                answer(&fx, &Hooks::none(), &pending.id, ApprovalDecision::Once).unwrap_err();
            assert!(
                error.message.contains("answer failed"),
                "{stage}: {}",
                error.message
            );
            assert_eq!(
                fx.db(|db| get_approval(db, &pending.id).unwrap()),
                pending,
                "{stage}"
            );
            assert_eq!(
                fx.db(|db| get_message(db, &pending.message_id).unwrap()),
                card,
                "{stage}"
            );
            assert_eq!(fx.db(state_counts), before, "{stage}");
            assert!(fx.event_names().is_empty(), "{stage}");
        }
    }

    #[test]
    fn rolls_back_denial_if_the_proposal_card_cannot_be_updated() {
        let (fx, writer, _) = setup();
        let pending = proposal(&fx, &writer);
        fx.clear_events();
        fx.db(|db| {
            db.execute_batch("CREATE TRIGGER reject_denial BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'denial failed'); END")
                .unwrap()
        });
        let error = answer(&fx, &Hooks::none(), &pending.id, ApprovalDecision::Deny).unwrap_err();
        assert!(error.message.contains("denial failed"), "{}", error.message);
        assert_eq!(
            fx.db(|db| get_approval(db, &pending.id).unwrap().status),
            ApprovalStatus::Pending
        );
        let card = fx.db(|db| get_message(db, &pending.message_id).unwrap());
        assert_eq!(
            card.data.as_ref().and_then(|data| data.get("status")),
            Some(&json!("pending"))
        );
        assert!(
            fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap())
                .is_empty()
        );
        assert!(fx.event_names().is_empty());
    }

    #[test]
    fn publishes_approval_results_only_after_the_routine_and_card_commit() {
        let (mut fx, writer, _) = setup();
        let pending = proposal(&fx, &writer);
        fx.clear_events();
        let observer = Arc::new(Mutex::new(connect(&fx)));
        let observed: Recorded<(ApprovalStatus, Option<String>, usize)> = Arc::default();
        let (watch, seen) = (observer.clone(), observed.clone());
        let (approval_id, message_id, bot_id) = (
            pending.id.clone(),
            pending.message_id.clone(),
            writer.id.clone(),
        );
        observe_broadcasts(&mut fx, move || {
            let db = watch.lock().unwrap_or_else(PoisonError::into_inner);
            let status = card_data(&db, &message_id)["status"]
                .as_str()
                .map(str::to_owned);
            recorded(&seen).push((
                get_approval(&db, &approval_id).unwrap().status,
                status,
                list_routines(&db, Some(bot_id.as_str())).unwrap().len(),
            ));
        });
        answer(&fx, &Hooks::none(), &pending.id, ApprovalDecision::Once).unwrap();
        let mut names = fx.event_names();
        names.sort();
        assert_eq!(
            names,
            [
                "approval.resolved",
                "bot.updated",
                "message.updated",
                "routine.updated"
            ]
        );
        assert_eq!(
            *recorded(&observed),
            vec![(ApprovalStatus::Allowed, Some("allowed".to_owned()), 1); 4]
        );
        assert!(
            answer(&fx, &Hooks::none(), &pending.id, ApprovalDecision::Once)
                .unwrap_err()
                .message
                .contains("already answered")
        );
        assert_eq!(
            fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap().len()),
            1
        );
    }

    #[test]
    fn resolves_each_decision_with_corrupt_card_data_and_clears_waiting_after_commit() {
        let cases: [(&str, Option<Value>, ApprovalDecision); 4] = [
            ("null", None, ApprovalDecision::Once),
            ("null", None, ApprovalDecision::Deny),
            ("malformed", Some(malformed_card()), ApprovalDecision::Once),
            ("malformed", Some(malformed_card()), ApprovalDecision::Deny),
        ];
        for (corruption, data, decision) in cases {
            let label = format!("{corruption}/{decision:?}");
            let (mut fx, writer, _) = setup();
            let pending = proposal(&fx, &writer);
            assert!(
                bot_updated_with(&fx, &writer.id, "waiting"),
                "{label}: the proposal sets Waiting"
            );
            let stored = data.as_ref().map(Value::to_string);
            fx.db(|db| {
                db.execute(
                    "UPDATE messages SET data = ?1 WHERE id = ?2",
                    params![stored, pending.message_id],
                )
                .unwrap()
            });
            fx.clear_events();
            let observer = Arc::new(Mutex::new(connect(&fx)));
            let observed: Recorded<(ApprovalStatus, Value, usize)> = Arc::default();
            let (watch, seen) = (observer.clone(), observed.clone());
            let (approval_id, message_id, bot_id) = (
                pending.id.clone(),
                pending.message_id.clone(),
                writer.id.clone(),
            );
            observe_broadcasts(&mut fx, move || {
                let db = watch.lock().unwrap_or_else(PoisonError::into_inner);
                recorded(&seen).push((
                    get_approval(&db, &approval_id).unwrap().status,
                    card_data(&db, &message_id),
                    list_routines(&db, Some(bot_id.as_str())).unwrap().len(),
                ));
            });
            let once = decision == ApprovalDecision::Once;
            let status = if once {
                ApprovalStatus::Allowed
            } else {
                ApprovalStatus::Denied
            };
            let word = if once { "allowed" } else { "denied" };
            let result = answer(&fx, &Hooks::none(), &pending.id, decision).unwrap();
            assert_eq!(result.status, status, "{label}");
            assert_eq!(
                fx.db(|db| get_approval(db, &pending.id).unwrap().status),
                status,
                "{label}"
            );
            let mut expected_card = data.unwrap_or_else(|| json!({}));
            expected_card["status"] = json!(word);
            assert_eq!(
                fx.db(|db| card_data(db, &pending.message_id)),
                expected_card,
                "{label}"
            );
            let routines = fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap().len());
            assert_eq!(routines, usize::from(once), "{label}");
            let message = fx.db(|db| get_message(db, &pending.message_id).unwrap());
            assert!(
                fx.events_named("message.updated")
                    .contains(&serde_json::to_value(&message).unwrap()),
                "{label}"
            );
            assert!(
                fx.events_named("approval.resolved")
                    .contains(&serde_json::to_value(&result).unwrap()),
                "{label}"
            );
            assert!(
                bot_updated_with(&fx, &writer.id, "idle"),
                "{label}: the answer clears Waiting"
            );
            let expected_rows = if once { 4 } else { 3 };
            assert_eq!(
                *recorded(&observed),
                vec![(status, expected_card.clone(), routines); expected_rows],
                "{label}"
            );
            assert!(
                answer(&fx, &Hooks::none(), &pending.id, decision)
                    .unwrap_err()
                    .message
                    .contains("already answered"),
                "{label}"
            );
        }
    }

    #[test]
    fn leaves_a_pending_proposal_intact_when_it_has_a_malformed_payload_or_wrong_kind() {
        let cases = [
            (
                "malformed payload",
                "UPDATE approvals SET payload = '{}' WHERE id = ?1",
            ),
            (
                "wrong approval kind",
                "UPDATE approvals SET kind = 'tool' WHERE id = ?1",
            ),
        ];
        for (reason, sql) in cases {
            let (fx, writer, _) = setup();
            let pending = proposal(&fx, &writer);
            fx.db(|db| db.execute(sql, [pending.id.as_str()]).unwrap());
            fx.clear_events();
            assert!(
                answer(&fx, &Hooks::none(), &pending.id, ApprovalDecision::Once).is_err(),
                "{reason}"
            );
            assert_eq!(
                fx.db(|db| get_approval(db, &pending.id).unwrap().status),
                ApprovalStatus::Pending,
                "{reason}"
            );
            let card = fx.db(|db| card_data(db, &pending.message_id));
            assert_eq!(card["status"], "pending", "{reason}");
            assert!(
                fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap())
                    .is_empty(),
                "{reason}"
            );
            assert!(fx.event_names().is_empty(), "{reason}");
        }
    }

    #[test]
    fn rejects_a_proposal_payload_that_belongs_to_another_bot_without_resolving_it() {
        let (fx, writer, researcher) = setup();
        let pending = proposal(&fx, &writer);
        let mut payload = pending
            .payload
            .clone()
            .expect("a routine proposal carries its routine");
        payload.insert("botId".to_owned(), json!(researcher.id));
        let stored = Value::Object(payload).to_string();
        fx.db(|db| {
            db.execute(
                "UPDATE approvals SET payload = ?1 WHERE id = ?2",
                params![stored, pending.id],
            )
            .unwrap()
        });
        fx.clear_events();
        assert!(answer(&fx, &Hooks::none(), &pending.id, ApprovalDecision::Once).is_err());
        assert_eq!(
            fx.db(|db| get_approval(db, &pending.id).unwrap().status),
            ApprovalStatus::Pending
        );
        assert_eq!(fx.db(state_counts)["routines"], 0);
        assert!(fx.event_names().is_empty());
    }
}

mod request_transactions {
    use super::*;

    #[test]
    fn rolls_back_the_exchange_and_handoffs_when_recording_a_failed_turn_cannot_finish() {
        let (fx, _, researcher) = setup();
        chat_turn(&fx, &researcher);
        call(
            &fx,
            &researcher.id,
            "message_bot",
            json!({ "bot": "Writer", "message": "Please summarize." }),
        )
        .unwrap();
        let request = recorded(&fx.delivered)[0].clone();
        let exchange_id = request
            .exchange_id
            .clone()
            .expect("the request has an exchange");
        let exchange = fx.db(|db| get_exchange(db, &exchange_id).unwrap());
        let handoffs = fx.db(|db| rows(db, "SELECT * FROM messages WHERE kind = 'handoff'"));
        fx.clear_events();
        fx.db(|db| {
            db.execute_batch("CREATE TRIGGER reject_failure_notice BEFORE INSERT ON messages WHEN NEW.kind = 'notice' BEGIN SELECT RAISE(ABORT, 'failure notice failed'); END")
                .unwrap()
        });
        let result = TurnResult {
            outcome: TurnOutcome::Failed,
            text: None,
            text_message_id: None,
            error: Some("CLI exited".to_owned()),
        };
        let error = with_team(&fx, &Hooks::none(), |team, host| {
            team.turn_ended(host, &request, &result)
        })
        .unwrap_err();
        assert!(
            error.message.contains("failure notice failed"),
            "{}",
            error.message
        );
        assert_eq!(
            fx.db(|db| get_exchange(db, &exchange_id).unwrap()),
            exchange
        );
        assert_eq!(
            fx.db(|db| rows(db, "SELECT * FROM messages WHERE kind = 'handoff'")),
            handoffs
        );
        assert!(fx.event_names().is_empty());
    }

    #[test]
    fn rejects_nested_transactions_that_have_an_after_commit_action() {
        let (fx, writer, researcher) = setup();
        let handle = with_team(&fx, &Hooks::none(), |team, host| {
            team.bot_deleting(host, &researcher.id)
        })
        .unwrap();
        fx.db(|db| delete_bot(db, &researcher.id).unwrap());
        // TypeScript's spy on `db.prepare` makes the nested call when the proposal inserts its approval, and its throw
        // fails that call. Here a trigger on that insert calls a SQL function, which makes the nested call on a second
        // host with its own connection (so the refused call never touches the proposal's transaction) and fails the
        // insert with the refusal.
        let second = Arc::new(Mutex::new(FakeHost {
            db: connect(&fx),
            delivered: Arc::default(),
            stopped: Arc::default(),
            current: Arc::default(),
            states: Arc::default(),
            effective: Arc::default(),
            deliver_fails: Arc::default(),
        }));
        let team = fx.team.clone();
        fx.db(|db| {
            db.create_scalar_function(
                "nested_after_commit",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| {
                    let mut host = second.lock().unwrap_or_else(PoisonError::into_inner);
                    match team.bot_deleted(&mut *host, &handle) {
                        Ok(()) => Ok(0_i64),
                        Err(error) => Err(rusqlite::Error::UserFunctionError(error.message.into())),
                    }
                },
            )
            .unwrap();
            db.execute_batch(
                "CREATE TRIGGER nested_after_commit_on_approval BEFORE INSERT ON approvals BEGIN SELECT nested_after_commit(); END",
            )
            .unwrap();
        });
        let before = fx.db(state_counts);
        let error = propose(&fx, &writer).unwrap_err();
        assert!(
            error.contains("An after-commit action cannot run inside another Team transaction."),
            "{error}"
        );
        assert_eq!(fx.db(state_counts), before);
        assert!(fx.event_names().is_empty());
    }

    #[test]
    fn logs_a_broadcast_failure_and_still_delivers_the_committed_request() {
        let (mut fx, _, researcher) = setup();
        let work = chat_turn(&fx, &researcher);
        team_with(&mut fx, |_event| Err("broadcast failed".to_owned()));
        assert_eq!(
            call(
                &fx,
                &researcher.id,
                "message_bot",
                json!({ "bot": "Writer", "message": "Please summarize." }),
            )
            .unwrap(),
            "Sent. Writer will reply."
        );
        let delivered = recorded(&fx.delivered).clone();
        assert_eq!(delivered.len(), 1);
        let item = fx.db(|db| get_work_item(db, &delivered[0].id).unwrap());
        assert_eq!(item.status, WorkStatus::Queued);
        let exchange_id = delivered[0]
            .exchange_id
            .clone()
            .expect("the request has an exchange");
        assert_eq!(
            fx.db(|db| get_exchange(db, &exchange_id).unwrap().status),
            ExchangeStatus::Waiting
        );
        assert_eq!(fx.db(|db| request_count(db, work.chain_id.as_deref())), 1);
        assert_eq!(
            *recorded(&fx.logs),
            vec![
                (
                    "Tjommie: a Team broadcast failed".to_owned(),
                    "broadcast failed".to_owned()
                );
                4
            ]
        );
    }

    #[test]
    fn rolls_back_quota_and_all_request_state_when_a_stage_fails() {
        let stages = [
            (
                "exchange insertion",
                "CREATE TRIGGER reject_request BEFORE INSERT ON exchanges BEGIN SELECT RAISE(ABORT, 'request failed'); END",
            ),
            (
                "conversation insertion",
                "CREATE TRIGGER reject_request BEFORE INSERT ON conversations WHEN NEW.kind = 'exchange' BEGIN SELECT RAISE(ABORT, 'request failed'); END",
            ),
            (
                "handoff insertion",
                "CREATE TRIGGER reject_request BEFORE INSERT ON messages WHEN NEW.kind = 'handoff' BEGIN SELECT RAISE(ABORT, 'request failed'); END",
            ),
            (
                "queue insertion",
                "CREATE TRIGGER reject_request BEFORE INSERT ON work_items WHEN NEW.source = 'bot_request' BEGIN SELECT RAISE(ABORT, 'request failed'); END",
            ),
        ];
        for (stage, trigger) in stages {
            let (fx, _, researcher) = setup();
            let work = chat_turn(&fx, &researcher);
            let before = fx.db(state_counts);
            fx.db(|db| db.execute_batch(trigger).unwrap());
            let error = call(
                &fx,
                &researcher.id,
                "message_bot",
                json!({ "bot": "Writer", "message": "Please summarize." }),
            )
            .unwrap_err();
            assert!(error.contains("request failed"), "{stage}: {error}");
            assert_eq!(fx.db(state_counts), before, "{stage}");
            assert_eq!(
                fx.db(|db| request_count(db, work.chain_id.as_deref())),
                0,
                "{stage}"
            );
            assert!(recorded(&fx.delivered).is_empty(), "{stage}");
            assert!(fx.event_names().is_empty(), "{stage}");
        }
    }

    #[test]
    fn publishes_requests_and_wakes_delivery_only_after_every_request_row_commits() {
        let (mut fx, _, researcher) = setup();
        let work = chat_turn(&fx, &researcher);
        let observer = Arc::new(Mutex::new(connect(&fx)));
        let observed: Recorded<BTreeMap<&'static str, i64>> = Arc::default();
        let (watch, seen) = (observer.clone(), observed.clone());
        observe_broadcasts(&mut fx, move || {
            let db = watch.lock().unwrap_or_else(PoisonError::into_inner);
            recorded(&seen).push(state_counts(&db));
        });
        let watch = observer.clone();
        let hooks = Hooks {
            on_deliver: Arc::new(move |item: &WorkItem| {
                let db = watch.lock().unwrap_or_else(PoisonError::into_inner);
                assert_eq!(get_work_item(&db, &item.id).unwrap(), *item);
            }),
            ..Hooks::none()
        };
        assert_eq!(
            call_with(
                &fx,
                &hooks,
                &researcher.id,
                "message_bot",
                json!({ "bot": "Writer", "message": "Please summarize." }),
            )
            .unwrap(),
            "Sent. Writer will reply."
        );
        let expected: BTreeMap<&str, i64> = [
            ("approvals", 0),
            ("chains", 1),
            ("conversations", 4),
            ("exchanges", 1),
            ("messages", 4),
            ("routines", 0),
            ("work_items", 2),
        ]
        .into_iter()
        .collect();
        assert_eq!(fx.db(state_counts), expected);
        assert_eq!(fx.event_names(), ["message.added"; 4]);
        assert_eq!(*recorded(&observed), vec![expected.clone(); 4]);
        assert_eq!(fx.db(|db| request_count(db, work.chain_id.as_deref())), 1);
        assert_eq!(recorded(&fx.delivered).len(), 1);
    }

    #[test]
    fn fails_the_exchange_refunds_quota_and_closes_queued_work_when_delivery_throws() {
        let (fx, _, researcher) = setup();
        let work = chat_turn(&fx, &researcher);
        let attempted: Arc<Mutex<Option<WorkItem>>> = Arc::default();
        let record = attempted.clone();
        let hooks = Hooks {
            on_deliver: Arc::new(move |item: &WorkItem| {
                *record.lock().unwrap_or_else(PoisonError::into_inner) = Some(item.clone());
            }),
            ..Hooks::none()
        };
        *fx.deliver_fails.lock().unwrap() = Some("delivery failed".to_owned());
        let error = call_with(
            &fx,
            &hooks,
            &researcher.id,
            "message_bot",
            json!({ "bot": "Writer", "message": "Please summarize." }),
        )
        .unwrap_err();
        assert!(error.contains("delivery failed"), "{error}");
        let request = attempted
            .lock()
            .unwrap()
            .clone()
            .expect("Delivery was not attempted");
        let exchange_id = request
            .exchange_id
            .clone()
            .expect("Request has no exchange");
        assert_eq!(
            fx.db(|db| get_exchange(db, &exchange_id).unwrap().status),
            ExchangeStatus::Failed
        );
        assert_eq!(fx.db(|db| request_count(db, work.chain_id.as_deref())), 0);
        let open: Vec<String> = fx.db(|db| {
            let mut statement = db
                .prepare("SELECT id FROM work_items WHERE exchange_id = ?1 AND status IN ('queued', 'running')")
                .unwrap();
            statement
                .query_map([exchange_id.as_str()], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<String>>>()
                .unwrap()
        });
        assert!(open.is_empty());
        let statuses: Vec<String> = fx.db(|db| {
            let mut statement = db
                .prepare("SELECT json_extract(data, '$.status') AS status FROM messages WHERE kind = 'handoff' AND json_extract(data, '$.exchangeId') = ?1")
                .unwrap();
            statement
                .query_map([exchange_id.as_str()], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<String>>>()
                .unwrap()
        });
        assert_eq!(statuses, ["failed", "failed"]);
        *fx.deliver_fails.lock().unwrap() = None;
        assert_eq!(
            call(
                &fx,
                &researcher.id,
                "message_bot",
                json!({ "bot": "Writer", "message": "Try again." }),
            )
            .unwrap(),
            "Sent. Writer will reply."
        );
        assert_eq!(fx.db(|db| request_count(db, work.chain_id.as_deref())), 1);
    }

    #[test]
    fn fails_closed_for_an_unknown_current_chain_without_leaving_request_state() {
        let (fx, _, researcher) = setup();
        let work = chat_turn(&fx, &researcher);
        set_current(
            &fx,
            &researcher.id,
            WorkItem {
                chain_id: Some("missing-chain".to_owned()),
                ..work
            },
        );
        let before = fx.db(state_counts);
        let error = call(
            &fx,
            &researcher.id,
            "message_bot",
            json!({ "bot": "Writer", "message": "Please summarize." }),
        )
        .unwrap_err();
        assert!(error.starts_with("Limit reached, ask the user"), "{error}");
        assert_eq!(fx.db(state_counts), before);
        assert!(fx.event_names().is_empty());
        assert!(recorded(&fx.delivered).is_empty());
    }
}

mod autonomous_chains {
    use super::*;

    fn limit(fx: &Fixture, chain_limit: u32) {
        fx.db(|db| {
            set_settings(
                db,
                &SettingsPatch {
                    chain_limit: Some(chain_limit),
                    ..Default::default()
                },
            )
            .unwrap()
        });
    }

    fn message(fx: &Fixture, researcher: &StoredBot, text: &str) -> Result<String, String> {
        call(
            fx,
            &researcher.id,
            "message_bot",
            json!({ "bot": "Writer", "message": text }),
        )
    }

    #[test]
    fn reuses_one_chain_with_no_current_work_and_stops_at_the_chain_limit() {
        let (fx, _, researcher) = setup();
        limit(&fx, 2);
        message(&fx, &researcher, "First.").unwrap();
        message(&fx, &researcher, "Second.").unwrap();
        let delivered = recorded(&fx.delivered).clone();
        assert_eq!(delivered[0].chain_id, delivered[1].chain_id);
        let error = message(&fx, &researcher, "Third.").unwrap_err();
        assert!(error.starts_with("Limit reached, ask the user"), "{error}");
        assert_eq!(recorded(&fx.delivered).len(), 2);
        assert_eq!(fx.db(state_counts)["chains"], 1);
        assert_eq!(
            fx.db(|db| request_count(db, delivered[0].chain_id.as_deref())),
            2
        );
    }

    #[test]
    fn gives_a_closed_autonomous_session_a_fresh_chain() {
        let (fx, _, researcher) = setup();
        limit(&fx, 1);
        message(&fx, &researcher, "First session.").unwrap();
        let error = message(&fx, &researcher, "Over limit.").unwrap_err();
        assert!(error.starts_with("Limit reached"), "{error}");
        fx.team.session_closed(&researcher.id);
        assert_eq!(
            message(&fx, &researcher, "New session.").unwrap(),
            "Sent. Writer will reply."
        );
        let delivered = recorded(&fx.delivered).clone();
        assert_ne!(delivered[0].chain_id, delivered[1].chain_id);
        assert_eq!(
            fx.db(|db| request_count(db, delivered[0].chain_id.as_deref())),
            1
        );
        assert_eq!(
            fx.db(|db| request_count(db, delivered[1].chain_id.as_deref())),
            1
        );
    }

    #[test]
    fn does_not_poison_its_autonomous_chain_when_the_first_request_rolls_back() {
        let (fx, _, researcher) = setup();
        limit(&fx, 1);
        fx.db(|db| {
            db.execute_batch("CREATE TRIGGER reject_first BEFORE INSERT ON work_items WHEN NEW.source = 'bot_request' BEGIN SELECT RAISE(ABORT, 'queue failed'); END")
                .unwrap()
        });
        let error = message(&fx, &researcher, "Will fail.").unwrap_err();
        assert!(error.contains("queue failed"), "{error}");
        assert_eq!(fx.db(state_counts)["chains"], 0);
        assert!(fx.event_names().is_empty());
        fx.db(|db| db.execute_batch("DROP TRIGGER reject_first").unwrap());
        assert_eq!(
            message(&fx, &researcher, "Retry.").unwrap(),
            "Sent. Writer will reply."
        );
        let delivered = recorded(&fx.delivered).clone();
        assert_eq!(delivered.len(), 1);
        assert_eq!(
            fx.db(|db| request_count(db, delivered[0].chain_id.as_deref())),
            1
        );
    }
}
