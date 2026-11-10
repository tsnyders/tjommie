//! Port of `packages/daemon/src/providers/codex-rpc.test.ts`, with the same recorded app-server lines.

mod providers_fakes;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::providers::ProviderError;
use daemon::providers::codex_rpc::{JsonRpcLines, RpcId};
use providers_fakes::{FakeChild, FakeHost, command, fake_child, settle};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

fn client() -> (JsonRpcLines, Arc<Mutex<Vec<Value>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let rpc = JsonRpcLines::new({
        let sent = Arc::clone(&sent);
        move |line: String| {
            sent.lock()
                .unwrap()
                .push(serde_json::from_str(&line).unwrap());
            Ok(())
        }
    });
    (rpc, sent)
}

fn sent_id(sent: &Mutex<Vec<Value>>, index: usize) -> Value {
    sent.lock().unwrap()[index]["id"].clone()
}

/// Answers the `initialize` request, then hands the child back.
fn answer_initialize(mut child: FakeChild) -> JoinHandle<FakeChild> {
    tokio::spawn(async move {
        let request: Value = serde_json::from_str(&child.read_line().await).unwrap();
        child
            .print(&json!({ "id": request["id"], "result": {} }).to_string())
            .await;
        child
    })
}

// describe("JsonRpcLines")

#[tokio::test]
async fn matches_responses_to_requests_by_id_in_any_order() {
    let (rpc, sent) = client();
    let a = rpc.request("model/list", json!({}));
    let b = rpc.request("account/read", json!({}));
    rpc.handle_line(&json!({ "id": sent_id(&sent, 1), "result": { "account": null } }).to_string());
    rpc.handle_line(&json!({ "id": sent_id(&sent, 0), "result": { "data": [] } }).to_string());
    assert_eq!(a.await, Ok(json!({ "data": [] })));
    assert_eq!(b.await, Ok(json!({ "account": null })));
    let methods: Vec<Value> = sent
        .lock()
        .unwrap()
        .iter()
        .map(|message| message["method"].clone())
        .collect();
    assert_eq!(methods, vec![json!("model/list"), json!("account/read")]);
}

#[tokio::test]
async fn rejects_with_the_remote_error_real_error_text_from_a_bad_thread_config() {
    let (rpc, sent) = client();
    let pending = rpc.request("thread/start", json!({}));
    rpc.handle_line(
        &json!({ "id": sent_id(&sent, 0), "error": { "code": -32600, "message": "failed to load configuration: invalid transport" } }).to_string(),
    );
    assert_eq!(
        pending.await,
        Err(ProviderError::Remote {
            code: -32600,
            message: "failed to load configuration: invalid transport".into()
        })
    );
}

#[tokio::test]
async fn rejects_every_non_null_error_response_even_when_its_code_or_shape_is_unexpected() {
    let (rpc, sent) = client();
    let missing_code = rpc.request("thread/start", json!({}));
    rpc.handle_line(
        &json!({ "id": sent_id(&sent, 0), "error": { "message": "failed to load configuration" } })
            .to_string(),
    );
    assert_eq!(
        missing_code.await,
        Err(ProviderError::Remote {
            code: 0,
            message: "failed to load configuration".into()
        })
    );

    let string_error = rpc.request("thread/start", json!({}));
    rpc.handle_line(&json!({ "id": sent_id(&sent, 1), "error": "boom" }).to_string());
    assert_eq!(
        string_error.await,
        Err(ProviderError::Remote {
            code: 0,
            message: "\"boom\"".into()
        })
    );
}

#[tokio::test]
async fn routes_notifications_and_server_requests_and_ignores_junk_real_app_server_lines() {
    let (rpc, _) = client();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    rpc.set_on_notification({
        let seen = Arc::clone(&seen);
        move |method, _| seen.lock().unwrap().push(format!("n:{method}"))
    });
    rpc.set_on_request({
        let seen = Arc::clone(&seen);
        move |id, method, _| seen.lock().unwrap().push(format!("r:{id}:{method}"))
    });
    rpc.handle_line(r#"{"method":"turn/started","params":{"threadId":"01a0d7d5-63cc-7c60-9053-51ebed691584","turn":{"id":"01a0d7d5-6f72-7a01-96d4-f8a64437c1b5","items":[],"itemsView":"notLoaded","status":"inProgress","error":null}}}"#);
    rpc.handle_line(r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"t","turnId":"u","itemId":"i","startedAtMs":1790327515964}}"#);
    rpc.handle_line("WARNING: proceeding, even though we could not create PATH aliases");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            "n:turn/started",
            "r:0:item/commandExecution/requestApproval"
        ]
    );
}

#[tokio::test]
async fn ignores_json_primitives_and_null() {
    let (rpc, _) = client();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    rpc.set_on_notification({
        let seen = Arc::clone(&seen);
        move |method, _| seen.lock().unwrap().push(method.into())
    });
    rpc.set_on_request({
        let seen = Arc::clone(&seen);
        move |_, method, _| seen.lock().unwrap().push(method.into())
    });
    for line in ["null", "42", "true", "\"text\""] {
        rpc.handle_line(line);
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn writes_notifications_results_and_errors_in_the_app_servers_shape() {
    let (rpc, sent) = client();
    rpc.notify("initialized", None);
    rpc.respond(&RpcId::from(0), json!({ "decision": "accept" }));
    rpc.respond_error(&RpcId::from(1), -32601, "Not supported by Tjommie");
    assert_eq!(
        *sent.lock().unwrap(),
        vec![
            json!({ "method": "initialized" }),
            json!({ "id": 0, "result": { "decision": "accept" } }),
            json!({ "id": 1, "error": { "code": -32601, "message": "Not supported by Tjommie" } }),
        ]
    );
}

#[tokio::test]
async fn fails_every_pending_request_when_the_process_goes_away() {
    let (rpc, _) = client();
    let pending = rpc.request("account/read", json!({}));
    rpc.fail_all(ProviderError::Failed("codex app-server exited".into()));
    assert!(pending.await.unwrap_err().message().contains("exited"));
}

#[tokio::test(start_paused = true)]
async fn rejects_immediately_and_clears_its_timeout_when_writing_throws() {
    let failure = ProviderError::Failed("stdin write failed".into());
    let rpc = JsonRpcLines::new({
        let failure = failure.clone();
        move |_| Err(failure.clone())
    });
    // With paused time a waiting request would reach its timeout instead.
    assert_eq!(rpc.request("turn/start", json!({})).await, Err(failure));
}

// describe("startCodexServer")

#[tokio::test]
async fn fails_pending_requests_when_the_child_stdin_emits_an_error() {
    let host = FakeHost::default();
    let (child, fake) = fake_child(4321);
    host.push_child(child);
    let answering = answer_initialize(fake);
    let server = daemon::providers::codex_rpc::start_codex_server(
        &host,
        &command("codex"),
        &BTreeMap::new(),
        ".",
        false,
    )
    .await
    .unwrap();
    let fake = answering.await.unwrap();

    let FakeChild { stdin, exit, .. } = fake;
    drop(stdin); // writes to the child now fail, like a stdin "error" event
    let pending = server.rpc.request("account/read", json!({}));
    let error = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .expect("rejected, not timed out")
        .unwrap_err();
    assert!(matches!(error, ProviderError::Failed(_)), "{error:?}");
    exit.fire(daemon::cli::CliExit::Exited {
        code: Some(1),
        signal: None,
    });
}

#[tokio::test(start_paused = true)]
async fn cancels_the_delayed_tree_kill_when_the_child_exits() {
    let host = FakeHost::default();
    let (child, fake) = fake_child(4321);
    host.push_child(child);
    let answering = answer_initialize(fake);
    let server = daemon::providers::codex_rpc::start_codex_server(
        &host,
        &command("codex"),
        &BTreeMap::new(),
        ".",
        false,
    )
    .await
    .unwrap();
    let fake = answering.await.unwrap();

    (server.close)();
    fake.exit_with(Some(0), None);
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert_eq!(fake.kills(), 0);
}

/// The other half of the TypeScript test's timer count: close arms a 3-second tree kill.
#[tokio::test(start_paused = true)]
async fn kills_the_tree_when_the_child_outlives_close_by_three_seconds() {
    let host = FakeHost::default();
    let (child, fake) = fake_child(4321);
    host.push_child(child);
    let answering = answer_initialize(fake);
    let server = daemon::providers::codex_rpc::start_codex_server(
        &host,
        &command("codex"),
        &BTreeMap::new(),
        ".",
        false,
    )
    .await
    .unwrap();
    let mut fake = answering.await.unwrap();

    (server.close)();
    assert_eq!(fake.read_line().await.trim(), r#"{"method":"initialized"}"#);
    assert_eq!(fake.read_line().await, "", "close ends stdin");
    tokio::time::sleep(Duration::from_millis(2999)).await;
    assert_eq!(fake.kills(), 0);
    tokio::time::sleep(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(fake.kills(), 1);
}

#[tokio::test]
async fn starts_api_key_app_server_authentication_in_ephemeral_storage_without_secret_argv_or_env()
{
    let host = FakeHost::default();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let env = BTreeMap::from([("CODEX_HOME".to_string(), "isolated".to_string())]);
    let starting = tokio::spawn(async move {
        let request: Value = serde_json::from_str(&fake.read_line().await).unwrap();
        fake.stdout
            .write_all(format!("{}\n", json!({ "id": request["id"], "result": {} })).as_bytes())
            .await
            .unwrap();
        fake
    });
    let server =
        daemon::providers::codex_rpc::start_codex_server(&host, &command("codex"), &env, ".", true)
            .await
            .unwrap();
    let fake = starting.await.unwrap();
    let (cmd, args, options) = host.last_launch();
    assert_eq!(cmd, command("codex"));
    assert_eq!(
        args,
        vec![
            "-c",
            r#"cli_auth_credentials_store="ephemeral""#,
            "app-server"
        ]
    );
    assert_eq!(
        (options.env, options.cwd.as_str(), options.claude_api_key),
        (env, ".", None)
    );
    (server.close)();
    fake.exit_with(Some(0), None);
}
