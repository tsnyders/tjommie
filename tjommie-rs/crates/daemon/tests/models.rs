//! `listModels` for Codex (`setup.ts:470-482`): one app-server on the default account, asked `model/list`, then
//! closed. TypeScript covers this only in its real-CLI tests; these drive a test-owned app-server.

mod providers_fakes;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use daemon::cli::{ChildExit, CliExit, CliHost, KillTree};
use daemon::models::list_codex_models;
use daemon::providers::codex_rpc::{CodexServer, JsonRpcLines};
use protocol::ErrorCode;
use protocol::model::ModelInfo;
use providers_fakes::{FakeHost, command};
use serde_json::{Value, json};

/// An app-server that answers `model/list` with `models`, counting its closes.
fn app_server(models: Value) -> (CodexServer, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let rpc = Arc::new_cyclic(|weak: &Weak<JsonRpcLines>| {
        let (weak, sent) = (weak.clone(), Arc::clone(&sent));
        JsonRpcLines::new(move |line| {
            let message: Value = serde_json::from_str(&line).unwrap();
            sent.lock().unwrap().push(message.clone());
            if message["method"] == "model/list" {
                let answer = json!({ "id": message["id"], "result": { "data": models } });
                weak.upgrade().unwrap().handle_line(&answer.to_string());
            }
            Ok(())
        })
    });
    let exit = ChildExit::new();
    let closes = Arc::new(AtomicUsize::new(0));
    let close = {
        let (exit, closes) = (exit.clone(), Arc::clone(&closes));
        Arc::new(move || {
            closes.fetch_add(1, Ordering::SeqCst);
            exit.fire(CliExit::Exited {
                code: Some(0),
                signal: None,
            });
        })
    };
    let kill_tree: KillTree = Arc::new(|| Box::pin(async { Ok(()) }));
    (
        CodexServer {
            rpc,
            pid: Some(4100),
            exit,
            kill_tree,
            close,
        },
        sent,
        closes,
    )
}

#[tokio::test]
async fn lists_the_visible_codex_models_from_the_app_server_and_closes_it() {
    let models = json!([
        { "model": "gpt-5.6-sol", "displayName": "GPT-5.6 Sol", "hidden": false, "isDefault": true },
        { "model": "internal-preview", "displayName": "Preview", "hidden": true, "isDefault": false },
        { "model": "gpt-5.4", "displayName": "GPT-5.4", "hidden": false, "isDefault": false },
    ]);
    let (server, sent, closes) = app_server(models);
    let host = Arc::new(FakeHost::default());
    host.push_server(server);
    let env = BTreeMap::from([(
        "CODEX_HOME".to_owned(),
        r"C:\accounts\codex-main".to_owned(),
    )]);
    let listed = list_codex_models(
        host.as_ref() as &dyn CliHost,
        Some(command("codex")),
        &env,
        r"C:\accounts\codex-main",
    )
    .await
    .unwrap();
    let model = |id: &str, label: &str, is_default| ModelInfo {
        id: id.into(),
        label: label.into(),
        is_default,
    };
    assert_eq!(
        listed,
        [
            model("gpt-5.6-sol", "GPT-5.6 Sol", true),
            model("gpt-5.4", "GPT-5.4", false)
        ]
    );
    let requests: Vec<Value> = sent
        .lock()
        .unwrap()
        .iter()
        .map(|message| message["method"].clone())
        .collect();
    assert_eq!(requests, [json!("model/list")]);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    let start = host.server_starts.lock().unwrap();
    assert_eq!(start.len(), 1);
    assert_eq!(start[0], (env, r"C:\accounts\codex-main".to_owned(), false));
}

#[tokio::test]
async fn reports_codex_as_not_installed_without_its_cli() {
    let host = FakeHost::default();
    let error = list_codex_models(&host, None, &BTreeMap::new(), ".")
        .await
        .unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::NotInstalled, "Codex is not installed")
    );
    assert_eq!(host.server_starts.lock().unwrap().len(), 0);
}
