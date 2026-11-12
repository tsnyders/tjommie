//! `chat.history`, `chat.send`, `chat.stop`, `chat.retry` and `approvals.respond` over the WebSocket (`daemon.ts`
//! handlers), on the runner harness's fake sessions. The client checks every result against the manifest.

use super::*;
use common::{Client, connect};
use daemon::context::{Context, handlers};
use daemon::server::{Handlers, Server, ServerOptions};

struct Api {
    _server: Server,
    client: Client,
}

async fn api(h: &Harness) -> Api {
    let options = ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    };
    let server = Server::start(options).await.unwrap();
    let setup = common::no_cli_setup();
    let accounts = daemon::account_handlers::Accounts::new(daemon::account_handlers::AccountsOptions {
        db: h.db.clone(),
        root: h.root.0.clone(),
        accounts_dir: h.root.0.join("accounts"),
        events: server.broadcaster(),
        bot_view: Arc::new(h.runner.clone()),
        setup: setup.clone(),
        ops: Arc::new(setup),
        credentials: Arc::new(common::MemoryKeys::default()),
    });
    let context = Context {
        db: h.db.clone(),
        bots_dir: h.root.0.join("bots"),
        events: server.broadcaster(),
        bot_view: Arc::new(h.runner.clone()),
        app_version: None,
        accounts,
        late: daemon::context::Late::default(),
    };
    server
        .activate(
            "t",
            daemon::runner::register(handlers(&context), &context, &h.runner),
        )
        .unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    Api {
        _server: server,
        client,
    }
}

fn error_of(response: &Value) -> (String, String) {
    (
        response["error"]["code"].as_str().unwrap().to_owned(),
        response["error"]["message"].as_str().unwrap().to_owned(),
    )
}

#[tokio::test]
async fn sends_a_message_into_a_turn_and_pages_the_chat_history() {
    let h = harness();
    let bot = h.bot(json!({}));
    let mut api = api(&h).await;
    let sent = api
        .client
        .call(1, "chat.send", json!({ "botId": bot.id, "text": "Begin" }))
        .await;
    assert_eq!(
        (
            sent["result"]["body"].clone(),
            sent["result"]["status"].clone()
        ),
        (json!("Begin"), json!("sent")),
        "{sent}"
    );
    wait_for(|| h.sent(0) > 0).await;
    assert_eq!(h.session(0).sent()[0], "Begin");
    h.session(0).emit(text("Hello"));
    h.session(0).emit(turn_done(true, None));
    settle().await;
    let page = api
        .client
        .call(2, "chat.history", json!({ "botId": bot.id, "limit": 1 }))
        .await;
    assert_eq!(page["result"]["hasMore"], json!(true));
    assert_eq!(page["result"]["messages"][0]["body"], json!("Hello"));
    let before = page["result"]["messages"][0]["id"].clone();
    let older = api
        .client
        .call(
            3,
            "chat.history",
            json!({ "botId": bot.id, "before": before }),
        )
        .await;
    assert_eq!(older["result"]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(older["result"]["messages"][0]["body"], json!("Begin"));
    let missing = api
        .client
        .call(4, "chat.history", json!({ "botId": "nobody" }))
        .await;
    assert_eq!(
        error_of(&missing),
        ("not_found".to_owned(), "No such bot".to_owned())
    );
}

#[tokio::test]
async fn stops_a_turn_and_retries_the_interrupted_message() {
    let h = harness();
    let bot = h.bot(json!({}));
    let mut api = api(&h).await;
    let sent = api
        .client
        .call(1, "chat.send", json!({ "botId": bot.id, "text": "Begin" }))
        .await;
    let message_id = sent["result"]["id"].as_str().unwrap().to_owned();
    wait_for(|| h.sent(0) > 0).await;
    let stopped = api
        .client
        .call(2, "chat.stop", json!({ "botId": bot.id }))
        .await;
    assert_eq!(stopped["result"], json!({}));
    assert_eq!(h.session(0).interrupts(), 1);
    h.session(0).emit(turn_done(false, None));
    settle().await;
    assert_eq!(
        h.item_for_message(&message_id).status,
        WorkStatus::Interrupted
    );
    let retried = api
        .client
        .call(3, "chat.retry", json!({ "messageId": message_id }))
        .await;
    assert_eq!(retried["result"]["id"], json!(message_id));
    wait_for(|| h.session(0).sent().len() == 2).await;
    assert_eq!(h.item_for_message(&message_id).status, WorkStatus::Running);
}

#[tokio::test]
async fn answers_a_tool_approval_through_the_runner_and_its_session() {
    let h = harness();
    let bot = h.bot(json!({}));
    let mut api = api(&h).await;
    api.client
        .call(1, "chat.send", json!({ "botId": bot.id, "text": "Begin" }))
        .await;
    wait_for(|| h.sent(0) > 0).await;
    h.session(0)
        .emit(approval("cli-approval", "Write file", None));
    settle().await;
    let added = h.added_approvals().remove(0);
    let resolved = api
        .client
        .call(
            2,
            "approvals.respond",
            json!({ "approvalId": added.id, "decision": "once" }),
        )
        .await;
    assert_eq!(
        (
            resolved["result"]["id"].clone(),
            resolved["result"]["status"].clone()
        ),
        (json!(added.id), json!("allowed"))
    );
    assert_eq!(
        h.session(0).answers(),
        [("cli-approval".to_owned(), ApprovalAnswer::Once)]
    );
    let again = api
        .client
        .call(
            3,
            "approvals.respond",
            json!({ "approvalId": added.id, "decision": "once" }),
        )
        .await;
    assert_eq!(error_of(&again).0, "already_answered");
}
