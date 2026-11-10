//! The T3.2 methods over the WebSocket: settings, bots, models, diagnostics and Google, with the `daemon.test.ts` cases
//! for them. Every result and event is checked against the protocol manifest by the test client.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{Client, TempRoot, connect};
use daemon::context::handlers;
use daemon::server::{Handlers, Server, ServerOptions};
use protocol::relay::DEFAULT_RELAY_URL;
use serde_json::{Value, json};

struct Harness {
    server: Server,
    client: Client,
    root: TempRoot,
}

async fn harness() -> Harness {
    let root = TempRoot::new();
    let options = ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    };
    let server = Server::start(options).await.unwrap();
    let context = common::test_context(
        &root.0,
        server.broadcaster(),
        Arc::new(common::no_cli_setup()),
        Arc::new(common::MemoryKeys::default()),
    );
    server.activate("t", handlers(&context)).unwrap();
    let mut client = connect(server.port()).await;
    client.hello("t").await;
    Harness {
        server,
        client,
        root,
    }
}

fn bot(name: &str) -> Value {
    json!({ "name": name, "color": "blue", "shape": "circle", "instructions": "", "provider": "claude", "model": "sonnet" })
}

fn events_named<'a>(client: &'a Client, name: &str) -> Vec<&'a Value> {
    client
        .events
        .iter()
        .filter(|event| event["event"] == name)
        .collect()
}

#[tokio::test]
async fn reads_and_changes_settings_and_announces_the_change() {
    let mut h = harness().await;
    assert_eq!(
        h.client.call(1, "settings.get", json!({})).await["result"],
        json!({ "chainLimit": 5, "relayUrl": DEFAULT_RELAY_URL })
    );
    assert_eq!(
        h.client
            .call(2, "settings.set", json!({ "chainLimit": 7 }))
            .await["result"],
        json!({ "chainLimit": 7, "relayUrl": DEFAULT_RELAY_URL })
    );
    assert_eq!(
        h.client
            .call(3, "settings.set", json!({ "relayUrl": null }))
            .await["result"],
        json!({ "chainLimit": 7, "relayUrl": null })
    );
    assert_eq!(
        h.client
            .call(4, "settings.set", json!({ "chainLimit": 0 }))
            .await["error"]["code"],
        "bad_params"
    );
    let changed = events_named(&h.client, "settings.changed");
    assert_eq!(changed.len(), 2);
    assert_eq!(
        changed[1]["data"],
        json!({ "chainLimit": 7, "relayUrl": null })
    );
    h.server.close().await;
}

#[tokio::test]
async fn creates_lists_changes_pauses_and_deletes_bots_and_announces_each_change() {
    let mut h = harness().await;
    let created = h.client.call(1, "bots.create", bot("Writer")).await["result"].clone();
    assert_eq!(
        (
            created["state"].clone(),
            created["setupReason"].clone(),
            created["autonomy"].clone()
        ),
        (json!("idle"), Value::Null, json!("folder"))
    );
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        h.client.call(2, "bots.list", json!({})).await["result"],
        json!([created])
    );
    let changed = h
        .client
        .call(
            3,
            "bots.update",
            json!({ "id": id, "patch": { "autonomy": "ask" } }),
        )
        .await["result"]
        .clone();
    assert_eq!(changed["autonomy"], "ask");
    assert_eq!(
        h.client
            .call(4, "bots.pause", json!({ "id": id, "paused": true }))
            .await["result"]["state"],
        "paused"
    );
    assert_eq!(
        h.client
            .call(5, "bots.pause", json!({ "id": id, "paused": false }))
            .await["result"]["state"],
        "idle"
    );
    let taken = h.client.call(6, "bots.create", bot("writer")).await;
    assert_eq!(taken["error"]["code"], "name_taken");
    assert_eq!(
        h.client.call(7, "bots.delete", json!({ "id": id })).await["result"],
        json!({})
    );
    assert_eq!(
        h.client.call(8, "bots.list", json!({})).await["result"],
        json!([])
    );
    assert_eq!(
        h.client.call(9, "bots.delete", json!({ "id": id })).await["error"]["code"],
        "not_found"
    );
    let updated: Vec<&Value> = events_named(&h.client, "bot.updated");
    assert_eq!(
        updated
            .iter()
            .map(|event| event["data"]["autonomy"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["folder", "ask", "ask", "ask"]
    );
    assert_eq!(
        events_named(&h.client, "bot.deleted")
            .iter()
            .map(|event| event["data"].clone())
            .collect::<Vec<_>>(),
        [json!({ "id": id })]
    );
    h.server.close().await;
}

#[tokio::test]
async fn opens_the_browser_only_for_a_bot_that_has_one() {
    let mut h = harness().await;
    let without = h.client.call(1, "bots.create", bot("Plain")).await["result"]["id"].clone();
    let response = h
        .client
        .call(2, "bots.showBrowser", json!({ "id": without }))
        .await;
    assert_eq!(
        response["error"],
        json!({ "code": "bad_params", "message": "This bot's browser is off" })
    );
    let mut browsing = bot("Browsing");
    browsing["browser"] = json!(true);
    let with = h.client.call(3, "bots.create", browsing).await["result"]["id"].clone();
    // M6 brings the native browser broker; until then there is no desktop host to open it.
    assert_eq!(
        h.client
            .call(4, "bots.showBrowser", json!({ "id": with }))
            .await["error"]["code"],
        "bad_state"
    );
    h.server.close().await;
}

#[tokio::test]
async fn reads_the_start_of_a_bots_notes_md_for_bot_settings() {
    let mut h = harness().await;
    let created = h.client.call(1, "bots.create", bot("Noter")).await["result"].clone();
    let (id, home) = (
        created["id"].clone(),
        created["homeFolder"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        h.client.call(2, "bots.notes", json!({ "id": id })).await["result"],
        json!({ "text": "# Noter's notes\n" })
    );
    std::fs::write(Path::new(&home).join("notes.md"), "x".repeat(5000)).unwrap();
    assert_eq!(
        h.client.call(3, "bots.notes", json!({ "id": id })).await["result"]["text"]
            .as_str()
            .unwrap()
            .len(),
        4000
    );
    std::fs::remove_file(Path::new(&home).join("notes.md")).unwrap();
    assert_eq!(
        h.client.call(4, "bots.notes", json!({ "id": id })).await["result"],
        json!({ "text": "" })
    );
    assert_eq!(
        h.client
            .call(5, "bots.notes", json!({ "id": "missing" }))
            .await["error"]["code"],
        "not_found"
    );
    h.server.close().await;
}

#[tokio::test]
async fn lists_the_claude_models() {
    let mut h = harness().await;
    let models = h
        .client
        .call(1, "models.list", json!({ "provider": "claude" }))
        .await["result"]
        .clone();
    assert_eq!(
        models,
        json!([
            { "id": "sonnet", "label": "Claude Sonnet (latest)", "isDefault": true },
            { "id": "opus", "label": "Claude Opus (latest)", "isDefault": false },
            { "id": "haiku", "label": "Claude Haiku (latest)", "isDefault": false },
        ])
    );
    // Codex lists its models through its app server with the default account; this host has no Codex CLI.
    assert_eq!(
        h.client
            .call(2, "models.list", json!({ "provider": "codex" }))
            .await["error"]["code"],
        "not_installed"
    );
    h.server.close().await;
}

#[tokio::test]
async fn serves_bounded_diagnostics_through_the_running_daemon() {
    let mut h = harness().await;
    let report = h.client.call(1, "diagnostics.get", json!({})).await["result"].clone();
    assert_eq!(report["daemonVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        (
            report["appVersion"].clone(),
            report["storage"].clone(),
            report["relay"].clone()
        ),
        (Value::Null, json!("ok"), json!("off"))
    );
    assert!(
        report["providers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|provider| provider["version"].is_null())
    );
    assert!(
        !report
            .to_string()
            .contains(&h.root.0.display().to_string().replace('\\', "\\\\"))
    );
    h.server.close().await;
}

#[tokio::test]
async fn answers_every_google_method_that_the_separate_google_api_is_not_enabled() {
    let mut h = harness().await;
    let calls = [
        ("google.status", json!({})),
        (
            "google.configure",
            json!({ "clientId": "123-test.apps.googleusercontent.com", "clientSecret": "synthetic" }),
        ),
        ("google.begin", json!({ "services": ["gmail"] })),
        ("google.cancel", json!({ "attempt": "a" })),
        ("google.disconnect", json!({})),
        ("google.grant", json!({ "botId": "b", "enabled": true })),
    ];
    for (id, (method, params)) in (1..).zip(calls) {
        let error = h.client.call(id, method, params).await["error"].clone();
        assert_eq!(
            error,
            json!({ "code": "bad_state", "message": "The separate Google API connection is not enabled in this version. Use the native browser." }),
            "{method}"
        );
    }
    h.server.close().await;
}
