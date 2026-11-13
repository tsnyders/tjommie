//! The five cases of `accounts-integration.test.ts` that need a runner: the real accounts runtime, runner and handlers
//! behind the WebSocket, with synthetic provider sessions, probes and API keys. No CLI, account or network is touched.
//! (The runner-free cases are in `account_handlers.rs`.) The fixture wires what `startup::assemble` wires, minus the
//! Windows worker, the real CLI and Credential Manager.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Client, HealthyOps, MemoryKeys, TempRoot, connect, no_cli_setup, test_db};
use daemon::account_handlers::{Accounts, AccountsOptions};
use daemon::bots::StoredBot;
use daemon::context::{Context, Db, Late, LateBotView, handlers};
use daemon::providers::{
    ApprovalAnswer, Limit, LimitState, ProviderError, ProviderEvent, ProviderSession, StartOptions,
    TokenUsage, TurnResult, Usage,
};
use daemon::runner::{Runner, RunnerDeps, SessionFactory, register};
use daemon::server::{Handlers, Server, ServerOptions};
use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

// ---------------------------------------------------------------------------------------------------------------------
// The synthetic provider session

/// What the fixture's sessions do, as `fixture(fixtureOptions)` configures them in TypeScript.
#[derive(Clone, Copy)]
struct Behaviour {
    /// `limitFirstTurn`: the first session's turn reaches the provider's limit instead of answering.
    limit_first_turn: bool,
    /// `usageCostUsd`: what each answered turn costs; `None` is a turn the provider did not price.
    cost: Option<f64>,
    /// `waitForExit`: the owned process stays alive until the test lets it go.
    hold_exit: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Behaviour { limit_first_turn: false, cost: Some(1.0), hold_exit: false }
    }
}

struct Synthetic {
    sequence: usize,
    model: String,
    limited: bool,
    cost: Option<f64>,
    exit: Shared<BoxFuture<'static, ()>>,
    subscribers: Mutex<Vec<mpsc::UnboundedSender<ProviderEvent>>>,
}

impl Synthetic {
    fn emit(&self, event: ProviderEvent) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|sender| sender.send(event.clone()).is_ok());
    }
}

impl ProviderSession for Synthetic {
    fn start(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        self.emit(ProviderEvent::Session {
            cli_session_id: format!("synthetic-session-{}", self.sequence),
        });
        Box::pin(async { Ok(()) })
    }

    fn send(&self, text: &str) -> BoxFuture<'static, Result<(), ProviderError>> {
        if self.limited {
            self.emit(ProviderEvent::Limit(Limit {
                state: LimitState::Reached,
                used_percent: Some(100.0),
                resets_at: None,
                limit_id: None,
                windows: None,
            }));
            self.emit(ProviderEvent::TurnDone(TurnResult {
                ok: false,
                error: Some("Synthetic quota limit".into()),
            }));
        } else {
            self.emit(ProviderEvent::Text { delta: text.into() });
            self.emit(ProviderEvent::TextEnd);
            self.emit(ProviderEvent::Usage(Usage {
                model: self.model.clone(),
                tokens: TokenUsage { input: 10.0, cached_input: 5.0, output: 2.0, reasoning: 1.0 },
                cost_usd: self.cost,
                total_cost_usd: None,
                event_id: None,
            }));
            self.emit(ProviderEvent::TurnDone(TurnResult { ok: true, error: None }));
        }
        Box::pin(async { Ok(()) })
    }

    fn steer(&self, _: &str) -> BoxFuture<'static, Result<bool, ProviderError>> {
        Box::pin(async { Ok(false) })
    }

    fn interrupt(&self) -> BoxFuture<'static, Result<bool, ProviderError>> {
        Box::pin(async { Ok(true) })
    }

    fn answer(&self, _: &str, _: ApprovalAnswer) {}

    fn close(&self) {}

    fn wait_for_exit(&self) -> BoxFuture<'static, Result<(), ProviderError>> {
        let exit = self.exit.clone();
        Box::pin(async move {
            exit.await;
            Ok(())
        })
    }

    fn pid(&self) -> Option<u32> {
        None
    }

    fn subscribe(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.subscribers.lock().unwrap().push(sender);
        receiver
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The fixture

struct Fixture {
    server: Server,
    client: Client,
    runner: Runner,
    context: Context,
    /// Every session's start options, in order (`envs`).
    starts: Arc<Mutex<Vec<StartOptions>>>,
    /// Lets the held process exit.
    exit: Option<oneshot::Sender<()>>,
    next: i64,
    _root: TempRoot,
}

async fn fixture(behaviour: Behaviour) -> Fixture {
    let root = TempRoot::new();
    let server = Server::start(ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    })
    .await
    .unwrap();
    let db: Db = Arc::new(Mutex::new(test_db(&root.0)));
    let setup = no_cli_setup();
    let accounts_dir = root.0.join("accounts");
    setup
        .ensure_accounts(&db.lock().unwrap(), &accounts_dir, &root.0)
        .unwrap();
    let events = server.broadcaster();
    let view = LateBotView::default();
    let accounts = Accounts::new(AccountsOptions {
        db: db.clone(),
        root: root.0.clone(),
        accounts_dir,
        events: events.clone(),
        bot_view: Arc::new(view.clone()),
        setup: setup.clone(),
        ops: Arc::new(HealthyOps),
        credentials: Arc::new(MemoryKeys::default()),
    });

    let (exit, exited) = oneshot::channel::<()>();
    let (exit, process): (Option<oneshot::Sender<()>>, Shared<BoxFuture<'static, ()>>) =
        if behaviour.hold_exit {
            // A sender dropped without sending also lets the process go, so a failed test doesn't hang.
            (Some(exit), async move { let _ = exited.await; }.boxed().shared())
        } else {
            (None, futures_util::future::ready(()).boxed().shared())
        };
    let starts: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let (record, sequence) = (starts.clone(), Arc::new(AtomicUsize::new(0)));
    let create_session: SessionFactory = Arc::new(move |_bot: &StoredBot, options: StartOptions| {
        let number = sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let session = Synthetic {
            sequence: number,
            model: options.model.clone(),
            limited: behaviour.limit_first_turn && number == 1,
            cost: behaviour.cost,
            exit: process.clone(),
            subscribers: Mutex::default(),
        };
        record.lock().unwrap().push(options);
        Ok(Arc::new(session) as Arc<dyn ProviderSession>)
    });

    let late = Late::default();
    let (held, issue) = (late.switching.clone(), late.switching.clone());
    let broadcast = events.clone();
    let deps = RunnerDeps {
        accounts: Some(Arc::new(accounts.runtime.clone())),
        is_held: Arc::new(move |bot_id| held.lock().unwrap().contains(bot_id)),
        setup_issue: Arc::new(move |bot| {
            issue
                .lock()
                .unwrap()
                .contains(&bot.id)
                .then(|| "This bot is changing provider. Try again shortly.".to_owned())
        }),
        kill_tree: Arc::new(|_| {}),
        ..RunnerDeps::new(
            db.clone(),
            Arc::new(move |event| {
                broadcast.send(event);
                Ok(())
            }),
            create_session,
            setup.clone(),
        )
    };
    let runner = Runner::new(deps);
    view.bind(Arc::new(runner.clone()));
    accounts.attach_runner(runner.clone());
    let _ = late.runner.set(runner.clone());
    let context = Context {
        db: db.clone(),
        bots_dir: root.0.join("bots"),
        events,
        bot_view: Arc::new(runner.clone()),
        app_version: None,
        accounts,
        late,
    };
    server
        .activate("t", register(handlers(&context), &context, &runner))
        .unwrap();
    runner.start().unwrap();

    let mut client = connect(server.port()).await;
    client.hello("t").await;
    let mut fixture = Fixture {
        server,
        client,
        runner,
        context,
        starts,
        exit,
        next: 0,
        _root: root,
    };
    fixture.ok("accounts.check", json!({})).await;
    fixture
}

impl Fixture {
    async fn ok(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let response = self.client.call(self.next, method, params).await;
        assert_eq!(response["ok"], true, "{method}: {response}");
        response["result"].clone()
    }

    /// `eventually`: polls `method` every 10 ms, for up to 1.5 s, until `found` picks something out of its result.
    async fn eventually(
        &mut self,
        method: &str,
        params: Value,
        found: impl Fn(&Value) -> Option<Value>,
    ) -> Value {
        for _ in 0..150 {
            let result = self.ok(method, params.clone()).await;
            if let Some(hit) = found(&result) {
                return hit;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("Synthetic account integration did not settle ({method}).");
    }

    async fn idle(&mut self, bot_id: &str) {
        let id = bot_id.to_owned();
        self.eventually("bots.list", json!({}), move |bots| {
            bots.as_array()?
                .iter()
                .find(|bot| bot["id"] == id.as_str() && bot["state"] == "idle")
                .cloned()
        })
        .await;
    }

    async fn state(&mut self, bot_id: &str, state: &str) {
        let (id, state) = (bot_id.to_owned(), state.to_owned());
        self.eventually("bots.list", json!({}), move |bots| {
            bots.as_array()?
                .iter()
                .find(|bot| bot["id"] == id.as_str() && bot["state"] == state.as_str())
                .cloned()
        })
        .await;
    }

    /// The bot's finished text reply containing `text`.
    async fn replied(&mut self, bot_id: &str, text: &str) {
        let (id, text) = (bot_id.to_owned(), text.to_owned());
        self.eventually("chat.history", json!({ "botId": bot_id }), move |history| {
            history["messages"]
                .as_array()?
                .iter()
                .find(|message| {
                    message["author"] == id.as_str()
                        && message["kind"] == "text"
                        && message["status"] == "done"
                        && message["body"].as_str().is_some_and(|body| body.contains(&text))
                })
                .cloned()
        })
        .await;
    }

    async fn send(&mut self, bot_id: &str, text: &str) {
        self.ok("chat.send", json!({ "botId": bot_id, "text": text })).await;
    }

    /// `usage.query` over the last and next minute, narrowed by `filter` (`botId` or `accountId`).
    async fn usage(&mut self, filter: Value) -> Value {
        let mut query = json!({ "from": iso(-60_000), "to": iso(60_000) });
        query
            .as_object_mut()
            .unwrap()
            .extend(filter.as_object().unwrap().clone());
        self.ok("usage.query", query).await
    }

    fn started(&self) -> usize {
        self.starts.lock().unwrap().len()
    }

    fn start_options(&self, index: usize) -> StartOptions {
        self.starts.lock().unwrap()[index].clone()
    }

    /// The titles of every `notify` event the client has seen.
    fn notices(&self) -> Vec<String> {
        self.client
            .events
            .iter()
            .filter(|event| event["event"] == "notify")
            .filter_map(|event| event["data"]["title"].as_str().map(str::to_owned))
            .collect()
    }

    /// Lets the held process exit.
    fn release_exit(&mut self) {
        if let Some(exit) = self.exit.take() {
            let _ = exit.send(());
        }
    }

    /// What daemon teardown does, in its order: the runner waits for its owned processes, then the accounts, then the
    /// server.
    async fn finish(mut self) {
        self.release_exit();
        let _ = self.runner.shutdown().await;
        self.context.accounts.runtime.close().await;
        self.client.close().await;
        self.server.close().await;
    }
}

/// `new Date(Date.now() + offset).toISOString()`.
fn iso(offset_ms: i64) -> String {
    let now = jiff::Timestamp::now().as_millisecond();
    jiff::Timestamp::from_millisecond(now + offset_ms)
        .unwrap()
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn input() -> Value {
    json!({ "name": "Synthetic", "color": "blue", "shape": "circle", "provider": "claude", "model": "sonnet",
        "instructions": "Synthetic account tests", "autonomy": "ask", "browser": false })
}

fn input_on(account_id: &Value) -> Value {
    let mut bot = input();
    bot["accountId"] = account_id.clone();
    bot
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_owned()
}

// ---------------------------------------------------------------------------------------------------------------------
// accounts-integration.test.ts

#[tokio::test]
async fn switches_a_bot_to_another_subscription_and_attributes_each_completed_turn_to_its_actual_account() {
    let mut f = fixture(Behaviour::default()).await;
    let other = f
        .ok("accounts.create", json!({ "provider": "claude", "label": "Other", "kind": "subscription" }))
        .await;
    f.ok("accounts.check", json!({ "id": other["id"] })).await;
    let bot = f.ok("bots.create", input()).await;
    let id = id_of(&bot);
    f.send(&id, "First account").await;
    f.replied(&id, "First account").await;
    f.idle(&id).await;
    f.ok("bots.update", json!({ "id": id, "patch": { "accountId": other["id"] } })).await;
    f.send(&id, "Other account").await;
    f.replied(&id, "Other account").await;
    f.idle(&id).await;

    let usage = f.usage(json!({ "botId": id })).await;
    let mut accounts: Vec<String> = usage["byAccount"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["accountId"].as_str().unwrap().to_owned())
        .collect();
    accounts.sort();
    let mut expected = vec![bot["accountId"].as_str().unwrap().to_owned(), id_of(&other)];
    expected.sort();
    assert_eq!(accounts, expected);
    assert_eq!(usage["summary"]["records"], 2);
    assert_eq!(usage["summary"]["costs"]["apiValue"]["usd"].as_f64(), Some(2.0));
    assert_eq!(usage["summary"]["costs"]["apiSpend"]["knownRecords"], 0);
    assert_eq!(f.started(), 2);
    let (first, second) = (f.start_options(0), f.start_options(1));
    assert_ne!(first.env["CLAUDE_CONFIG_DIR"], second.env["CLAUDE_CONFIG_DIR"]);
    assert_eq!(second.resume_id, None);
    let history = f.ok("chat.history", json!({ "botId": id })).await;
    let bodies: Vec<&str> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["body"].as_str())
        .collect();
    assert!(bodies.iter().any(|body| body.contains("First account")));
    assert!(bodies.iter().any(|body| body.contains("Other account")));
    f.finish().await;
}

#[tokio::test]
async fn blocks_another_api_key_turn_at_its_lifetime_cap_then_resumes_queued_work_after_the_cap_is_raised() {
    let mut f = fixture(Behaviour::default()).await;
    let account = f
        .ok("accounts.create", json!({ "provider": "claude", "label": "Capped synthetic API", "kind": "api_key", "apiKey": "synthetic-cap-only", "spendingCapUsd": 1 }))
        .await;
    f.ok("accounts.check", json!({ "id": account["id"] })).await;
    let bot = f.ok("bots.create", input_on(&account["id"])).await;
    let id = id_of(&bot);
    f.send(&id, "Use the first dollar").await;
    let account_id = id_of(&account);
    f.eventually("accounts.list", json!({}), |accounts| {
        accounts
            .as_array()?
            .iter()
            .find(|entry| entry["id"] == account_id.as_str() && entry["spending"]["blocked"] == true)
            .cloned()
    })
    .await;
    f.send(&id, "Wait for a higher cap").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(f.started(), 1);
    f.ok("accounts.update", json!({ "id": account["id"], "patch": { "spendingCapUsd": 3 } })).await;
    f.replied(&id, "Wait for a higher cap").await;
    assert_eq!(f.started(), 2);
    let report = f.usage(json!({ "accountId": account["id"] })).await;
    assert_eq!(report["summary"]["costs"]["apiSpend"]["usd"].as_f64(), Some(2.0));
    assert_eq!(report["summary"]["costs"]["apiValue"]["knownRecords"], 0);
    f.finish().await;
}

#[tokio::test]
async fn waits_for_an_owned_account_process_to_exit_before_teardown_closes_its_database() {
    let mut f = fixture(Behaviour { hold_exit: true, ..Behaviour::default() }).await;
    let bot = f.ok("bots.create", input()).await;
    let id = id_of(&bot);
    f.send(&id, "Synthetic process still alive").await;
    f.replied(&id, "Synthetic process still alive").await;
    let closed = Arc::new(AtomicBool::new(false));
    let closing = f.runner.shutdown();
    let waiting = {
        let closed = closed.clone();
        tokio::spawn(async move {
            let _ = closing.await;
            closed.store(true, Ordering::SeqCst);
        })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!closed.load(Ordering::SeqCst), "teardown finished while the process was alive");
    f.release_exit();
    waiting.await.unwrap();
    assert!(closed.load(Ordering::SeqCst));
    f.finish().await;
}

#[tokio::test]
async fn reports_unknown_cost_honestly_instead_of_claiming_the_account_spent_100_percent_of_its_cap() {
    let mut f = fixture(Behaviour { cost: None, ..Behaviour::default() }).await;
    let account = f
        .ok("accounts.create", json!({ "provider": "claude", "label": "Unpriced synthetic API", "kind": "api_key", "apiKey": "synthetic-unpriced-only", "spendingCapUsd": 10 }))
        .await;
    f.ok("accounts.check", json!({ "id": account["id"] })).await;
    let bot = f.ok("bots.create", input_on(&account["id"])).await;
    f.send(&id_of(&bot), "Synthetic missing price").await;
    let account_id = id_of(&account);
    let blocked = f
        .eventually("accounts.list", json!({}), |accounts| {
            accounts
                .as_array()?
                .iter()
                .find(|entry| entry["id"] == account_id.as_str() && entry["spending"]["unknownRows"] == 1)
                .cloned()
        })
        .await;
    assert_eq!(blocked["spending"]["blocked"], true);
    // The notice follows the usage row by a task hop; each poll of `accounts.list` also collects what has arrived.
    for _ in 0..150 {
        if f.notices().iter().any(|title| title == "Unpriced synthetic API has unpriced usage") {
            break;
        }
        f.ok("accounts.list", json!({})).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let notices = f.notices();
    assert!(notices.contains(&"Unpriced synthetic API has unpriced usage".to_owned()), "{notices:?}");
    assert!(!notices.iter().any(|title| title.contains("100%")), "{notices:?}");
    f.finish().await;
}

#[tokio::test]
async fn retries_queued_subscription_work_only_after_the_explicit_unknown_reset_action() {
    let mut f = fixture(Behaviour { limit_first_turn: true, ..Behaviour::default() }).await;
    let bot = f.ok("bots.create", input()).await;
    let (id, account_id) = (id_of(&bot), bot["accountId"].as_str().expect("the bot has an account pin").to_owned());
    f.send(&id, "Explicitly retry this queued work").await;
    f.state(&id, "limited").await;
    assert_eq!(f.started(), 1);
    f.ok("accounts.retryLimit", json!({ "id": account_id })).await;
    f.replied(&id, "Explicitly retry this queued work").await;
    let pinned = account_id.clone();
    f.eventually("accounts.list", json!({}), move |accounts| {
        accounts
            .as_array()?
            .iter()
            .find(|entry| {
                entry["id"] == pinned.as_str()
                    && entry["limits"]
                        .as_array()
                        .is_some_and(|windows| windows.iter().all(|window| window["state"] != "reached"))
            })
            .cloned()
    })
    .await;
    assert_eq!(f.started(), 2);
    let report = f.usage(json!({ "botId": id })).await;
    assert_eq!(report["summary"]["costs"]["apiValue"]["usd"].as_f64(), Some(1.0));
    assert_eq!(report["summary"]["costs"]["apiSpend"]["knownRecords"], 0);
    f.finish().await;
}
