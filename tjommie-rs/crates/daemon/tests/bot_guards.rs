//! The bot handlers' M5 guards and folder-protection calls (`daemon.ts:439-483`, `:493-494`), through `Server::dispatch`.
//! The protection pass is a fake that records what it was asked, so no test touches a real ACL (`windows-acl.test.ts`
//! injects its runner the same way).

mod common;
mod project_support;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::account_handlers::{Accounts, AccountsOptions};
use daemon::bots;
use daemon::context::{Context, Late, StoredState};
use daemon::m5::{M5, bot_available};
use daemon::protect::{FolderProtection, ListAccounts};
use daemon::server::{Handlers, Principal, RequestContext, Server, ServerOptions};
use daemon::task_integration::TaskIntegrations;
use daemon::tasks::TasksOptions;
use daemon::win::acl::{BotHome, ProtectedPaths, Protection};
use project_support::{Fixture, fixture};
use protocol::ErrorCode;
use protocol::model::Provider;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// A protection pass that records the bot homes it was given and answers `result`.
struct Passes {
    homes: Mutex<Vec<Vec<BotHome>>>,
    result: Mutex<Protection>,
}

impl Passes {
    fn calls(&self) -> usize {
        self.homes.lock().unwrap().len()
    }

    fn set(&self, result: Protection) {
        *self.result.lock().unwrap() = result;
    }
}

fn protection(h: &Fixture) -> (Arc<FolderProtection>, Arc<Passes>) {
    let passes = Arc::new(Passes {
        homes: Mutex::default(),
        result: Mutex::new(Protection::Ready),
    });
    let recorder = passes.clone();
    let list_accounts: ListAccounts = Arc::new(|_| Ok(Vec::new()));
    let protection = FolderProtection::with_pass(
        ProtectedPaths::new(&h.root.0),
        h.db.clone(),
        list_accounts,
        Protection::Ready,
        Arc::new(move |_, homes, _| {
            recorder.homes.lock().unwrap().push(homes.to_vec());
            *recorder.result.lock().unwrap()
        }),
    );
    (protection, passes)
}

fn local() -> RequestContext {
    RequestContext {
        connection_id: "synthetic".into(),
        principal: Principal::Local,
        signal: CancellationToken::new(),
    }
}

/// The bot handlers over the fixture's own database, with its project services as M5.
async fn serve(h: &Fixture, protection: Arc<FolderProtection>) -> (Server, Context) {
    let server = Server::start(ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    })
    .await
    .unwrap();
    let accounts = Accounts::new(AccountsOptions {
        db: h.db.clone(),
        root: h.root.0.clone(),
        accounts_dir: h.root.0.join("accounts"),
        events: server.broadcaster(),
        bot_view: Arc::new(StoredState),
        setup: common::no_cli_setup(),
        ops: Arc::new(common::no_cli_setup()),
        credentials: Arc::new(common::MemoryKeys::default()),
    });
    accounts.attach_protection(protection);
    let context = Context {
        db: h.db.clone(),
        bots_dir: h.root.0.join("bots"),
        events: server.broadcaster(),
        bot_view: Arc::new(StoredState),
        app_version: None,
        accounts,
        late: Late::default(),
    };
    let _ = context.late.m5.set(Arc::new(M5 {
        projects: h.projects.clone(),
        tasks: h.tasks.clone(),
        integrations: TaskIntegrations::new(h.tasks.clone(), None, None),
    }));
    server
        .activate("t", bots::register(Handlers::default(), &context))
        .unwrap();
    (server, context)
}

async fn call(
    server: &Server,
    method: &str,
    params: Value,
) -> Result<Value, daemon::server::RpcError> {
    server.dispatch(method, params, Some(local())).await
}

#[tokio::test]
async fn refuses_to_delete_a_bot_in_a_project_and_to_change_a_bot_with_a_running_task() {
    let h = fixture(TasksOptions::default());
    let (protection, _) = protection(&h);
    let (server, context) = serve(&h, protection).await;
    let developer = h.bots[0].id.clone();

    // `projects.botInUse`: every fixture bot is a member of the active project.
    let error = call(&server, "bots.delete", json!({ "id": developer }))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    assert_eq!(
        error.message,
        "Remove this agent from its project and finish its task or check before deleting it."
    );

    // A member without a task can still be changed; one that holds a task lease cannot (`tasks.botBusy`).
    let idle = h.bots[1].id.clone();
    let renamed = call(
        &server,
        "bots.update",
        json!({ "id": idle, "patch": { "name": "Developer B2" } }),
    )
    .await
    .unwrap();
    assert_eq!(renamed["name"], "Developer B2");
    let task = h.new_task(0);
    h.tasks.start(&task.id, task.revision, "guard").unwrap();
    let error = call(
        &server,
        "bots.update",
        json!({ "id": developer, "patch": { "name": "Busy" } }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    assert_eq!(
        error.message,
        "Stop this agent's project task or check before changing its settings."
    );
    drop(context);
    server.close().await;
}

#[tokio::test]
async fn deletes_a_bot_that_is_in_no_project() {
    let h = fixture(TasksOptions::default());
    let (protection, _) = protection(&h);
    let (server, _context) = serve(&h, protection).await;
    let outsider = {
        let db = h.db.lock().unwrap();
        common::new_bot(&db, &h.root.0.join("bots"), "Outsider", "claude").id
    };
    call(&server, "bots.delete", json!({ "id": outsider }))
        .await
        .unwrap();
    assert!(
        bots::get_stored_bot(&h.db.lock().unwrap(), &outsider).is_err(),
        "the bot is gone"
    );
    server.close().await;
}

#[test]
fn counts_a_bot_being_switched_or_deleted_as_unavailable_to_projects() {
    let (switching, deleting) = (
        Arc::new(Mutex::new(HashSet::new())),
        Arc::new(Mutex::new(HashSet::new())),
    );
    let available = bot_available(switching.clone(), deleting.clone());
    available("bot").unwrap();
    for set in [&switching, &deleting] {
        set.lock().unwrap().insert("bot".to_string());
        let error = available("bot").unwrap_err();
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(
            error.message,
            "This agent's settings or lifecycle is changing. Try again shortly."
        );
        available("other").unwrap();
        set.lock().unwrap().clear();
    }
    available("bot").unwrap();
}

#[tokio::test]
async fn refreshes_the_folder_protection_after_a_bot_is_made() {
    let h = fixture(TasksOptions::default());
    let (protection, passes) = protection(&h);
    let (server, _context) = serve(&h, protection).await;
    let created = call(
        &server,
        "bots.create",
        json!({ "name": "Fresh", "color": "blue", "shape": "circle", "instructions": "", "provider": "claude", "model": "sonnet" }),
    )
    .await
    .unwrap();
    assert_eq!(created["name"], "Fresh");
    assert_eq!(passes.calls(), 1);
    // The pass saw the new bot's home among the bots it protects.
    {
        let homes = passes.homes.lock().unwrap();
        assert!(
            homes[0]
                .iter()
                .any(|home| home.id == created["id"].as_str().unwrap())
        );
    }
    server.close().await;
}

#[tokio::test]
async fn protects_a_bot_as_its_new_provider_before_saving_the_change_and_refuses_when_the_check_fails()
 {
    let h = fixture(TasksOptions::default());
    {
        let db = h.db.lock().unwrap();
        let folder = h.root.0.join("accounts").join("codex-main");
        std::fs::create_dir_all(&folder).unwrap();
        common::insert_account(&db, "codex-main", "codex", &folder);
    }
    let (protection, passes) = protection(&h);
    let (server, _context) = serve(&h, protection).await;
    let bot = {
        let db = h.db.lock().unwrap();
        common::new_bot(&db, &h.root.0.join("bots"), "Switcher", "claude")
    };
    let switch = json!({ "id": bot.id, "patch": { "provider": "codex", "model": "gpt-5.6-sol" } });

    // The check fails: the bot stays on its old provider, and the pass runs again without the override.
    passes.set(Protection::CheckFailed);
    let error = call(&server, "bots.update", switch.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadState);
    assert_eq!(
        error.message,
        "Could not update the agent folder protection. Try again after checking Codex's sandbox setup."
    );
    assert_eq!(
        bots::get_stored_bot(&h.db.lock().unwrap(), &bot.id)
            .unwrap()
            .provider,
        Provider::Claude
    );
    assert_eq!(passes.calls(), 2);
    {
        let homes = passes.homes.lock().unwrap();
        let first = homes[0].iter().find(|home| home.id == bot.id).unwrap();
        assert_eq!(
            first.provider,
            Provider::Codex,
            "the first pass saw the new provider"
        );
        let second = homes[1].iter().find(|home| home.id == bot.id).unwrap();
        assert_eq!(
            second.provider,
            Provider::Claude,
            "the retry saw what is saved"
        );
    }

    // The check passes: the change is saved after one pass.
    passes.set(Protection::Ready);
    let changed = call(&server, "bots.update", switch).await.unwrap();
    assert_eq!(changed["provider"], "codex");
    assert_eq!(passes.calls(), 3);

    // A change that keeps the provider needs no pass.
    call(
        &server,
        "bots.update",
        json!({ "id": bot.id, "patch": { "name": "Switcher 2" } }),
    )
    .await
    .unwrap();
    assert_eq!(passes.calls(), 3);
    server.close().await;
}

/// The lock order is the database, then the leaf sets (`switching`, `deleting`): the runner (`is_held`) and the projects'
/// `bot_available` take them that way. A bot change that held `switching` while it took the database would lock against
/// them and freeze the daemon. One thread takes the database and then asks `bot_available`, over and over, as the runner
/// does; another changes a bot's account over and over. On the old code the two lock each other within a few rounds.
/// Both run on plain threads and the test waits with a timeout, so a deadlock fails it instead of hanging the run.
#[tokio::test]
async fn changing_a_bots_account_never_waits_for_the_database_while_holding_the_switching_set() {
    let h = fixture(TasksOptions::default());
    let bot = {
        let db = h.db.lock().unwrap();
        let folder = h.root.0.join("accounts").join("claude-second");
        std::fs::create_dir_all(&folder).unwrap();
        common::insert_account(&db, "claude-second", "claude", &folder);
        common::new_bot(&db, &h.root.0.join("bots"), "Switcher", "claude")
    };
    let (protection, _) = protection(&h);
    let (server, context) = serve(&h, protection).await;
    let server = Arc::new(server);
    let available = bot_available(
        context.late.switching.clone(),
        context.late.deleting.clone(),
    );
    let stop = Arc::new(AtomicBool::new(false));

    let runner_order = {
        let (db, stop) = (h.db.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                {
                    let _db = db.lock().unwrap();
                    available("another bot").unwrap();
                }
                std::thread::yield_now();
            }
        })
    };
    let (done, finished) = std::sync::mpsc::channel();
    let changes = {
        let (server, id, stop) = (server.clone(), bot.id.clone(), stop.clone());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            for round in 0..50 {
                let account = if round % 2 == 0 {
                    "claude-second"
                } else {
                    "claude-main"
                };
                let change = json!({ "id": id, "patch": { "accountId": account } });
                runtime
                    .block_on(server.dispatch("bots.update", change, Some(local())))
                    .unwrap();
            }
            stop.store(true, Ordering::SeqCst);
            let _ = done.send(());
        })
    };
    finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the account changes finished while another thread took the database, then the switching set");
    runner_order.join().unwrap();
    changes.join().unwrap();
    Arc::into_inner(server)
        .expect("both threads are done with the server")
        .close()
        .await;
}
