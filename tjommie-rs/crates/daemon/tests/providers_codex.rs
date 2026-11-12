//! Port of `packages/daemon/src/providers/codex.test.ts`. A test-owned `JsonRpcLines` stands in for the app-server,
//! as the mocked `startCodexServer` does in TypeScript.
//!
//! Not ported until T3.5 brings the Runner: "ignores an approved command's late events after Stop" (both rows) and
//! "sends one provider interrupt when the runner stops during turn/start". They drive `Runner`, `db` and `bots`.

mod providers_fakes;

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use daemon::cli::{BoxFuture, ChildExit, CliExit, CliHost, KillTree};
use daemon::providers::codex::CodexSession;
use daemon::providers::codex_rpc::{CodexServer, JsonRpcLines};
use daemon::providers::{
    ApiKey, Approval, ApprovalAnswer, McpServer, ProviderError, ProviderEvent, ProviderSession,
    StartOptions, TokenUsage, TurnResult,
};
use protocol::model::Autonomy;
use providers_fakes::{FakeHost, command, drain, settle};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

const EXITED_BEFORE_STOP: &str = "Codex exited before its process tree could be stopped.";

fn options() -> StartOptions {
    StartOptions {
        cwd: "C:\\bots\\b1".into(),
        extra_dirs: vec![],
        model: String::new(),
        instructions: "Be brief.".into(),
        autonomy: Autonomy::Folder,
        codex_sandbox_ready: false,
        allow_rules: vec![],
        resume_id: None,
        resume_cost_usd: None,
        env: BTreeMap::new(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers: vec![],
    }
}

fn elicitation() -> Value {
    json!({
        "serverName": "tjommie",
        "mode": "form",
        "message": "Please authorize the ping operation on this MCP server.",
        "requestedSchema": {},
        "_meta": { "codex_approval_kind": "mcp_tool_call", "tool_description": "Ping", "tool_params": { "text": "hello" } },
    })
}

fn with(mut value: Value, key: &str, field: Value) -> Value {
    value[key] = field;
    value
}

type Kill = Box<dyn FnOnce() -> BoxFuture<'static, Result<(), String>> + Send>;

/// `killTreeAndWait`: by default the child exits with code 1; `once` overrides the next call.
#[derive(Default)]
struct FakeKill {
    calls: AtomicUsize,
    once: Mutex<VecDeque<Kill>>,
}

impl FakeKill {
    fn once(&self, kill: impl FnOnce() -> BoxFuture<'static, Result<(), String>> + Send + 'static) {
        self.once.lock().unwrap().push_back(Box::new(kill));
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

struct Harness {
    codex: CodexSession,
    rpc: Arc<JsonRpcLines>,
    sent: Arc<Mutex<Vec<Value>>>,
    events: mpsc::UnboundedReceiver<ProviderEvent>,
    exit: ChildExit,
    kill: Arc<FakeKill>,
    host: Arc<FakeHost>,
    closes: Arc<AtomicUsize>,
}

impl Harness {
    fn line(&self, message: Value) {
        self.rpc.handle_line(&message.to_string());
    }

    fn notify(&self, method: &str, params: Value) {
        self.line(json!({ "method": method, "params": params }));
    }

    fn sent(&self) -> Vec<Value> {
        self.sent.lock().unwrap().clone()
    }

    fn requests(&self, method: &str) -> Vec<Value> {
        self.sent()
            .into_iter()
            .filter(|message| message["method"] == method)
            .collect()
    }

    fn events(&mut self) -> Vec<ProviderEvent> {
        drain(&mut self.events)
    }

    fn exit_child(&self, code: i32) {
        self.exit.fire(CliExit::Exited {
            code: Some(code),
            signal: None,
        });
    }
}

fn exit_code(exit: &ChildExit) -> Option<i32> {
    match exit.status() {
        Some(CliExit::Exited { code, .. }) => code,
        _ => None,
    }
}

/// `session()` in TypeScript, before `start`. The app-server answers `thread/start` and `thread/resume` itself, after
/// handling `before_thread_response` if given.
fn build(
    auto_allow: bool,
    resume_id: Option<&str>,
    change: impl FnOnce(&mut StartOptions),
    resolved_model: &str,
    before_thread_response: Option<Value>,
) -> Harness {
    build_with(
        auto_allow,
        resume_id,
        change,
        resolved_model,
        before_thread_response,
        None,
    )
}

fn build_with(
    auto_allow: bool,
    resume_id: Option<&str>,
    change: impl FnOnce(&mut StartOptions),
    resolved_model: &str,
    before_thread_response: Option<Value>,
    retirement: Option<daemon::cli::Retirement>,
) -> Harness {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let rpc = Arc::new_cyclic(|weak: &Weak<JsonRpcLines>| {
        let (weak, sent, model) = (weak.clone(), Arc::clone(&sent), resolved_model.to_string());
        JsonRpcLines::new(move |line| {
            let message: Value = serde_json::from_str(&line).unwrap();
            if matches!(
                message["method"].as_str(),
                Some("thread/start" | "thread/resume")
            ) {
                let rpc = weak.upgrade().unwrap();
                if let Some(line) = &before_thread_response {
                    rpc.handle_line(&line.to_string());
                }
                rpc.handle_line(&json!({ "id": message["id"], "result": { "thread": { "id": "thread-1" }, "model": model } }).to_string());
            } else {
                sent.lock().unwrap().push(message);
            }
            Ok(())
        })
    });
    let exit = ChildExit::new();
    let kill = Arc::new(FakeKill::default());
    let kill_tree: KillTree = {
        let (kill, exit) = (Arc::clone(&kill), exit.clone());
        Arc::new(move || {
            kill.calls.fetch_add(1, Ordering::SeqCst);
            match kill.once.lock().unwrap().pop_front() {
                Some(once) => once(),
                None => {
                    exit.fire(CliExit::Exited {
                        code: Some(1),
                        signal: None,
                    });
                    Box::pin(async { Ok(()) })
                }
            }
        })
    };
    let closes = Arc::new(AtomicUsize::new(0));
    let close = {
        let (rpc, exit, closes) = (Arc::clone(&rpc), exit.clone(), Arc::clone(&closes));
        Arc::new(move || {
            closes.fetch_add(1, Ordering::SeqCst);
            rpc.fail_all(ProviderError::Failed("Test session closed".into()));
            exit.fire(CliExit::Exited {
                code: Some(0),
                signal: None,
            }); // only if it has not exited yet
        })
    };
    let host = Arc::new(FakeHost::default());
    host.push_server(CodexServer {
        rpc: Arc::clone(&rpc),
        pid: Some(4001),
        exit: exit.clone(),
        kill_tree,
        close,
        retirement,
    });
    let mut options = options();
    change(&mut options);
    options.resume_id = resume_id.map(str::to_string);
    options.mcp_servers = vec![McpServer::Http {
        name: "tjommie".into(),
        url: "http://127.0.0.1:47100/mcp".into(),
        headers: BTreeMap::new(),
        auto_allow,
    }];
    let cli_host: Arc<dyn CliHost> = host.clone();
    let codex = CodexSession::new(cli_host, command("codex"), options);
    let events = codex.subscribe();
    Harness {
        codex,
        rpc,
        sent,
        events,
        exit,
        kill,
        host,
        closes,
    }
}

async fn session(auto_allow: bool) -> Harness {
    let mut harness = build(auto_allow, None, |_| {}, "test-model", None);
    harness.codex.start().await.unwrap();
    harness.events();
    harness
}

// describe("CodexSession MCP elicitations")

#[tokio::test]
async fn answers_for_unexpected_wording_by_auto_allow() {
    for (auto_allow, action) in [(true, "accept"), (false, "decline")] {
        let mut h = session(auto_allow).await;
        h.line(
            json!({ "id": 7, "method": "mcpServer/elicitation/request", "params": elicitation() }),
        );
        assert_eq!(
            h.sent(),
            vec![json!({ "id": 7, "result": { "action": action } })],
            "autoAllow {auto_allow}"
        );
        assert_eq!(h.events(), vec![]);
    }
}

#[tokio::test]
async fn still_declines_malformed_tool_call_elicitations_for_an_auto_allow_server() {
    let mut h = session(true).await;
    let meta = elicitation()["_meta"].clone();
    let malformed = [
        Value::Null,
        json!([]),
        with(elicitation(), "serverName", json!(3)),
        with(elicitation(), "serverName", json!("")),
        with(elicitation(), "mode", json!("url")),
        with(elicitation(), "message", json!(3)),
        with(elicitation(), "_meta", Value::Null),
        with(elicitation(), "_meta", json!([])),
        with(
            elicitation(),
            "_meta",
            with(meta.clone(), "codex_approval_kind", json!("other")),
        ),
        with(
            elicitation(),
            "_meta",
            with(meta.clone(), "tool_description", json!(3)),
        ),
        with(
            elicitation(),
            "_meta",
            with(meta.clone(), "tool_params", json!("bad")),
        ),
        with(elicitation(), "_meta", with(meta, "tool_params", json!([]))),
    ];
    for (id, params) in malformed.iter().enumerate() {
        h.line(json!({ "id": id, "method": "mcpServer/elicitation/request", "params": params }));
    }
    let declines: Vec<Value> = (0..malformed.len())
        .map(|id| json!({ "id": id, "result": { "action": "decline" } }))
        .collect();
    assert_eq!(h.sent(), declines);
    assert_eq!(h.events(), vec![]);
}

#[tokio::test]
async fn keeps_parseable_calls_from_a_non_auto_allow_server_on_the_approval_card_path() {
    let mut h = session(false).await;
    let params = with(
        elicitation(),
        "message",
        json!("Allow the tjommie MCP server to run tool \"ping\"?"),
    );
    h.line(
        json!({ "id": "approval-1", "method": "mcpServer/elicitation/request", "params": params }),
    );
    assert!(h.sent().is_empty());
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Approval(Approval {
            id: "approval-1".into(),
            tool: "tjommie.ping".into(),
            summary: "Ping".into(),
            detail: "{\n  \"text\": \"hello\"\n}".into(),
            rule: Some("codex-mcp:tjommie.ping".into()),
        })]
    );
    h.codex.answer("approval-1", ApprovalAnswer::Once);
    assert_eq!(
        h.sent(),
        vec![json!({ "id": "approval-1", "result": { "action": "accept" } })]
    );
}

// describe("CodexSession steering")

#[tokio::test]
async fn does_not_restore_a_completed_turn_from_a_delayed_start_response() {
    for background in [false, true] {
        let mut h = session(true).await;
        let send = h.codex.send("start");
        h.notify("turn/started", json!({ "turn": { "id": "old" } }));
        h.notify(
            "turn/completed",
            json!({ "turn": { "id": "old", "status": "completed" } }),
        );
        if background {
            h.notify("turn/started", json!({ "turn": { "id": "background" } }));
            h.notify(
                "item/agentMessage/delta",
                json!({ "turnId": "background", "delta": "Background reply" }),
            );
        }
        h.line(json!({ "id": 2, "result": { "turn": { "id": "old" } } }));
        send.await.unwrap();
        let steer = h.codex.steer("Follow up");
        let request = h.requests("turn/steer").pop();
        if let Some(request) = &request {
            h.line(json!({ "id": request["id"], "result": {} }));
        }
        if background {
            assert_eq!(steer.await, Ok(true));
            assert_eq!(request.unwrap()["params"]["expectedTurnId"], "background");
            assert!(h.events().contains(&ProviderEvent::Text {
                delta: "Background reply".into()
            }));
        } else {
            assert_eq!(steer.await, Ok(false));
            assert_eq!(h.requests("turn/steer"), Vec::<Value>::new());
        }
    }
}

#[tokio::test]
async fn declines_a_late_request_without_reopening_a_completed_turn() {
    for (method, params, response) in [
        (
            "item/commandExecution/requestApproval",
            json!({ "command": "node timer" }),
            json!({ "decision": "decline" }),
        ),
        (
            "item/fileChange/requestApproval",
            json!({ "grantRoot": "C:\\bots\\b1" }),
            json!({ "decision": "decline" }),
        ),
        (
            "mcpServer/elicitation/request",
            elicitation(),
            json!({ "action": "decline" }),
        ),
    ] {
        let mut h = session(true).await;
        h.notify("turn/started", json!({ "turn": { "id": "stopped" } }));
        h.notify(
            "turn/completed",
            json!({ "turn": { "id": "stopped", "status": "interrupted" } }),
        );
        h.events();
        h.line(json!({ "id": "late", "method": method, "params": with(params, "turnId", json!("stopped")) }));
        assert_eq!(
            h.sent(),
            vec![json!({ "id": "late", "result": response })],
            "{method}"
        );
        assert_eq!(h.events(), vec![], "{method}");
    }
}

#[tokio::test]
async fn keeps_late_usage_while_ignoring_a_completed_turns_replayed_start_and_content() {
    let mut h = session(true).await;
    let sending = h.codex.send("Synthetic turn before Stop");
    let request = h.requests("turn/start").remove(0);
    h.line(json!({ "id": request["id"], "result": { "turn": { "id": "stopped" } } }));
    sending.await.unwrap();
    h.sent.lock().unwrap().clear();
    h.notify("turn/started", json!({ "turn": { "id": "stopped" } }));
    h.notify(
        "turn/completed",
        json!({ "turn": { "id": "stopped", "status": "interrupted" } }),
    );
    h.events();
    h.notify("turn/started", json!({ "turn": { "id": "stopped" } }));
    h.notify(
        "item/agentMessage/delta",
        json!({ "turnId": "stopped", "delta": "stale" }),
    );
    h.notify(
        "thread/tokenUsage/updated",
        json!({ "turnId": "stopped", "tokenUsage": { "last": { "inputTokens": 10, "cachedInputTokens": 2, "outputTokens": 3, "reasoningOutputTokens": 1 } } }),
    );
    assert_eq!(h.codex.steer("Follow up").await, Ok(false));
    assert!(h.sent().is_empty());
    let events = h.events();
    let [ProviderEvent::Usage(usage)] = events.as_slice() else {
        panic!("expected one usage event, got {events:?}")
    };
    assert!(usage.event_id.as_ref().is_some_and(
        |id| id.len() == 64 && id.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f'))
    ));
    assert_eq!(
        (usage.model.as_str(), usage.tokens, usage.cost_usd),
        (
            "test-model",
            TokenUsage {
                input: 8.0,
                cached_input: 2.0,
                output: 3.0,
                reasoning: 1.0
            },
            None
        )
    );
}

#[tokio::test]
async fn settles_the_current_turn_when_a_completion_omits_its_id() {
    let mut h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "current" } }));
    h.notify(
        "turn/completed",
        json!({ "turn": { "status": "completed", "error": null } }),
    );
    h.notify(
        "item/agentMessage/delta",
        json!({ "turnId": "current", "delta": "stale" }),
    );
    assert_eq!(
        h.events(),
        vec![ProviderEvent::TurnDone(TurnResult {
            ok: true,
            error: None
        })]
    );
    assert_eq!(h.codex.steer("Follow up").await, Ok(false));
}

#[tokio::test]
async fn still_rejects_unsupported_requests_associated_with_a_completed_turn() {
    let mut h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "stopped" } }));
    h.notify(
        "turn/completed",
        json!({ "turn": { "id": "stopped", "status": "interrupted" } }),
    );
    h.events();
    h.line(
        json!({ "id": "late", "method": "unsupported/request", "params": { "turnId": "stopped" } }),
    );
    assert_eq!(
        h.sent(),
        vec![
            json!({ "id": "late", "error": { "code": -32601, "message": "Not supported by Tjommie" } })
        ]
    );
    assert_eq!(h.events(), vec![]);
}

#[tokio::test]
async fn reports_a_failed_tree_termination_and_lets_stop_retry_without_publishing_a_premature_exit()
{
    let mut h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "stopped" } }));
    h.kill
        .once(|| Box::pin(async { Err("tree termination failed".to_string()) }));
    let failed = h.codex.interrupt();
    settle().await;
    assert_eq!(h.requests("turn/interrupt").len(), 1);
    h.notify(
        "turn/completed",
        json!({ "turn": { "id": "stopped", "status": "interrupted" } }),
    );
    h.line(json!({ "id": 2, "result": {} }));
    assert_eq!(
        failed.await,
        Err(ProviderError::Failed("tree termination failed".into()))
    );
    assert_eq!(h.events(), vec![]);

    let retry = h.codex.interrupt();
    settle().await;
    assert_eq!(h.requests("turn/interrupt").len(), 2);
    h.line(
        json!({ "id": 3, "error": { "code": -32600, "message": "no active turn to interrupt" } }),
    );
    assert_eq!(retry.await, Ok(true));
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: None,
            completed_turn: None
        }]
    );
}

#[tokio::test]
async fn publishes_the_held_exit_when_a_pending_send_rejects_during_stop() {
    let mut h = session(true).await;
    let failure = ProviderError::Failed("app-server exited during send".into());
    let sent = h.codex.send("start");
    let stopped = h.codex.interrupt();
    h.rpc.fail_all(failure.clone());
    h.exit_child(1);
    assert_eq!(sent.await, Err(failure.clone()));
    assert_eq!(stopped.await, Err(failure.clone()));
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: Some(failure.message().into()),
            completed_turn: None
        }]
    );
}

#[tokio::test]
async fn reports_unverified_tree_termination_when_the_server_exits_before_its_interrupt_response() {
    let mut h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "active" } }));
    let stopped = h.codex.interrupt();
    settle().await;
    assert_eq!(h.requests("turn/interrupt").len(), 1);
    h.rpc
        .fail_all(ProviderError::Failed("app-server exited".into()));
    h.exit_child(1);
    assert_eq!(
        stopped.await,
        Err(ProviderError::Failed(EXITED_BEFORE_STOP.into()))
    );
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: Some(EXITED_BEFORE_STOP.into()),
            completed_turn: None
        }]
    );
}

#[tokio::test]
async fn reports_an_exit_immediately_after_stop_when_send_has_already_fulfilled() {
    let mut h = session(true).await;
    let sent = h.codex.send("start");
    h.line(json!({ "id": 2, "result": { "turn": { "id": "active" } } }));
    sent.await.unwrap();
    let stopped = h.codex.interrupt();
    h.exit_child(1); // the interrupt has not run past its wait for a pending send yet
    assert_eq!(
        stopped.await,
        Err(ProviderError::Failed(EXITED_BEFORE_STOP.into()))
    );
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: Some(EXITED_BEFORE_STOP.into()),
            completed_turn: None
        }]
    );
}

#[tokio::test]
async fn settles_a_held_completion_without_a_turn_id() {
    for status in ["completed", "interrupted"] {
        let mut h = session(true).await;
        let stopped = h.codex.interrupt();
        h.notify(
            "turn/completed",
            json!({ "turn": { "status": status, "error": null } }),
        );
        assert_eq!(stopped.await, Ok(true), "{status}");
        let completed_turn = (status == "completed").then_some(TurnResult {
            ok: true,
            error: None,
        });
        assert_eq!(
            h.events(),
            vec![ProviderEvent::Exit {
                error: None,
                completed_turn
            }],
            "{status}"
        );
    }
}

#[tokio::test]
async fn holds_a_normal_completion_racing_stop_until_process_tree_termination() {
    let mut h = session(true).await;
    let (terminated, termination) = oneshot::channel::<()>();
    let exit = h.exit.clone();
    h.kill.once(move || {
        exit.fire(CliExit::Exited {
            code: Some(1),
            signal: None,
        });
        Box::pin(async move {
            let _ = termination.await;
            Ok(())
        })
    });
    h.notify("turn/started", json!({ "turn": { "id": "active" } }));
    let stopped = h.codex.interrupt();
    settle().await;
    assert_eq!(h.requests("turn/interrupt").len(), 1);
    h.notify(
        "turn/completed",
        json!({ "turn": { "id": "active", "status": "completed", "error": null } }),
    );
    h.line(json!({ "id": 2, "result": {} }));
    settle().await;
    assert!(h.kill.calls() > 0);
    assert_eq!(h.events(), vec![]);
    terminated.send(()).unwrap();
    assert_eq!(stopped.await, Ok(true));
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: None,
            completed_turn: Some(TurnResult {
                ok: true,
                error: None
            })
        }]
    );
}

#[tokio::test]
async fn publishes_a_tree_termination_failure_if_the_parent_exit_was_already_held() {
    let mut h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "active" } }));
    let exit = h.exit.clone();
    h.kill.once(move || {
        exit.fire(CliExit::Exited {
            code: Some(1),
            signal: None,
        });
        Box::pin(async { Err("tree termination failed after exit".to_string()) })
    });
    let stopped = h.codex.interrupt();
    settle().await;
    assert_eq!(h.requests("turn/interrupt").len(), 1);
    h.notify(
        "turn/completed",
        json!({ "turn": { "id": "active", "status": "interrupted" } }),
    );
    h.line(json!({ "id": 2, "result": {} }));
    assert_eq!(
        stopped.await,
        Err(ProviderError::Failed(
            "tree termination failed after exit".into()
        ))
    );
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: Some("tree termination failed after exit".into()),
            completed_turn: None
        }]
    );
}

#[tokio::test]
async fn waits_for_an_in_flight_turn_start_before_interrupting_it() {
    let h = session(true).await;
    let send = h.codex.send("start");
    let stopped = h.codex.interrupt();
    h.line(json!({ "id": 2, "result": { "turn": { "id": "turn-1" } } }));
    send.await.unwrap();
    settle().await;
    assert!(h.sent().contains(&json!({ "id": 3, "method": "turn/interrupt", "params": { "threadId": "thread-1", "turnId": "turn-1" } })));
    h.line(json!({ "id": 3, "result": {} }));
    assert_eq!(stopped.await, Ok(true));
}

#[tokio::test]
async fn closes_a_server_that_finishes_initializing_after_the_session_was_closed() {
    let host = Arc::new(FakeHost::default());
    let initialized = host.push_pending_server();
    let cli_host: Arc<dyn CliHost> = host.clone();
    let codex = CodexSession::new(cli_host, command("codex"), options());
    let start = codex.start();
    settle().await; // start is now waiting for the server, where TypeScript's start() returns
    codex.close();
    let exit = ChildExit::new();
    let closes = Arc::new(AtomicUsize::new(0));
    let close = {
        let closes = Arc::clone(&closes);
        Arc::new(move || {
            closes.fetch_add(1, Ordering::SeqCst);
        })
    };
    let kill_tree: KillTree = Arc::new(|| Box::pin(async { Ok(()) }));
    let rpc = Arc::new(JsonRpcLines::new(|_| Ok(())));
    assert!(
        initialized
            .send(CodexServer {
                rpc,
                pid: Some(42),
                exit: exit.clone(),
                kill_tree,
                close,
                retirement: None,
            })
            .is_ok()
    );
    assert!(start.await.unwrap_err().message().contains("closed"));
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    exit.fire(CliExit::Exited {
        code: Some(0),
        signal: None,
    });
}

#[tokio::test]
async fn does_not_send_a_steer_request_when_there_is_no_local_active_turn() {
    let h = session(true).await;
    assert_eq!(h.codex.steer("Follow up").await, Ok(false));
    assert!(h.sent().is_empty());
}

#[tokio::test]
async fn accepts_a_successful_steer_response() {
    let h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "turn-1" } }));
    let steered = h.codex.steer("Follow up");
    assert_eq!(
        h.sent(),
        vec![
            json!({ "id": 2, "method": "turn/steer", "params": { "threadId": "thread-1", "expectedTurnId": "turn-1", "input": [{ "type": "text", "text": "Follow up" }] } })
        ]
    );
    h.line(json!({ "id": 2, "result": { "turnId": "turn-1" } }));
    assert_eq!(steered.await, Ok(true));
}

#[tokio::test]
async fn returns_false_for_the_expected_state_rejections() {
    for message in [
        "no active turn to steer",
        "expected active turn id `turn-1` but found `turn-2`",
        "cannot steer a review turn",
        "cannot steer a compact turn",
    ] {
        let h = session(true).await;
        h.notify("turn/started", json!({ "turn": { "id": "turn-1" } }));
        let steered = h.codex.steer("Follow up");
        h.line(json!({ "id": 2, "error": { "code": -32600, "message": message } }));
        assert_eq!(steered.await, Ok(false), "{message}");
    }
}

#[tokio::test]
async fn propagates_unexpected_rpc_rejections() {
    for (code, message) in [
        (-32603, "failed to steer turn: connection failed"),
        (-32600, "thread not found: thread-1"),
        (-32603, "no active turn to steer"),
    ] {
        let h = session(true).await;
        h.notify("turn/started", json!({ "turn": { "id": "turn-1" } }));
        let steered = h.codex.steer("Follow up");
        h.line(json!({ "id": 2, "error": { "code": code, "message": message } }));
        assert_eq!(
            steered.await,
            Err(ProviderError::Remote {
                code,
                message: message.into()
            })
        );
    }
}

#[tokio::test]
async fn propagates_transport_failures_even_when_their_text_resembles_an_expected_state_rejection()
{
    let h = session(true).await;
    h.notify("turn/started", json!({ "turn": { "id": "turn-1" } }));
    let steered = h.codex.steer("Follow up");
    let failure = ProviderError::Failed("no active turn to steer".into());
    h.rpc.fail_all(failure.clone());
    assert_eq!(steered.await, Err(failure));
}

// describe("Codex API account transport")

fn usage_events(events: Vec<ProviderEvent>) -> Vec<daemon::providers::Usage> {
    events
        .into_iter()
        .filter_map(|event| {
            if let ProviderEvent::Usage(usage) = event {
                Some(usage)
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test]
async fn does_not_publish_restored_usage_before_the_model_resolves_or_a_new_turn_starts() {
    for resume_id in [None, Some("saved-thread")] {
        let usage = json!({ "method": "thread/tokenUsage/updated", "params": { "threadId": "thread-1", "turnId": "old-turn",
            "tokenUsage": { "last": { "inputTokens": 20, "cachedInputTokens": 1, "outputTokens": 3, "reasoningOutputTokens": 0 } } } });
        let mut h = build(
            true,
            resume_id,
            |_| {},
            "gpt-5.3-codex",
            Some(usage.clone()),
        );
        h.codex.start().await.unwrap();
        assert_eq!(usage_events(h.events()), vec![]);
        h.line(usage.clone());
        assert_eq!(usage_events(h.events()), vec![]);
        let sending = h.codex.send("Synthetic new turn");
        let request = h.requests("turn/start").remove(0);
        h.line(json!({ "id": request["id"], "result": { "turn": { "id": "new-turn" } } }));
        sending.await.unwrap();
        h.line(with(
            usage.clone(),
            "params",
            with(usage["params"].clone(), "turnId", json!("new-turn")),
        ));
        let usages = usage_events(h.events());
        assert_eq!(
            usages
                .iter()
                .map(|usage| (usage.model.as_str(), usage.tokens.input))
                .collect::<Vec<_>>(),
            vec![("gpt-5.3-codex", 19.0)]
        );
    }
}

fn with_key(options: &mut StartOptions) {
    options.api_key = Some(ApiKey("synthetic-private-key".into()));
}

#[tokio::test]
async fn uses_ephemeral_storage_and_sends_the_key_only_over_the_private_login_rpc_before_creating_a_thread()
 {
    let h = build(true, None, with_key, "gpt-5.3-codex", None);
    let starting = h.codex.start();
    settle().await;
    assert_eq!(h.requests("account/login/start").len(), 1);
    assert_eq!(
        *h.host.server_starts.lock().unwrap(),
        vec![(BTreeMap::new(), "C:\\bots\\b1".to_string(), true)]
    );
    let sent = h.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["params"],
        json!({ "type": "apiKey", "apiKey": "synthetic-private-key" })
    );
    h.line(json!({ "id": sent[0]["id"], "result": {} }));
    starting.await.unwrap();
}

#[tokio::test]
async fn rejects_an_unpriced_resolved_api_model_before_dispatching_work() {
    for resume_id in [None, Some("saved-thread")] {
        let mut h = build(
            true,
            resume_id,
            |options| {
                options.model = "gpt-5.3-codex".into();
                with_key(options);
            },
            "unpriced-provider-alias",
            None,
        );
        let starting = h.codex.start();
        settle().await;
        let login = h.requests("account/login/start").remove(0);
        h.line(json!({ "id": login["id"], "result": {} }));
        assert!(
            starting
                .await
                .unwrap_err()
                .message()
                .contains("without supported local pricing")
        );
        assert!(h.codex.send("This must not reach a model").await.is_err());
        assert_eq!(h.requests("turn/start"), Vec::<Value>::new());
        assert!(
            !h.events()
                .iter()
                .any(|event| matches!(event, ProviderEvent::Session { .. }))
        );
        assert_eq!(exit_code(&h.exit), Some(0));
    }
}

#[tokio::test]
async fn redacts_a_rejected_login_payload_and_closes_only_its_owned_server() {
    let mut h = build(true, None, with_key, "test-model", None);
    let starting = h.codex.start();
    settle().await;
    let login = h.requests("account/login/start").remove(0);
    h.line(json!({ "id": login["id"], "error": { "code": 401, "message": "Bad key synthetic-private-key" } }));
    assert!(
        starting
            .await
            .unwrap_err()
            .message()
            .contains("could not configure this API key")
    );
    assert_eq!(exit_code(&h.exit), Some(0));
    assert_eq!(h.closes.load(Ordering::SeqCst), 1);
    assert!(!format!("{:?}", h.events()).contains("synthetic-private-key"));
}

#[tokio::test]
async fn deduplicates_repeated_usage_snapshots_without_dropping_a_later_cumulative_update() {
    let mut h = session(true).await;
    let sending = h.codex.send("Synthetic new turn");
    let request = h.requests("turn/start").remove(0);
    h.line(json!({ "id": request["id"], "result": { "turn": { "id": "turn-1" } } }));
    sending.await.unwrap();
    let mut params = json!({ "threadId": "thread-1", "turnId": "turn-1", "tokenUsage": { "total": { "inputTokens": 20 },
        "last": { "inputTokens": 20, "cachedInputTokens": 1, "outputTokens": 3, "reasoningOutputTokens": 0 } } });
    h.notify("thread/tokenUsage/updated", params.clone());
    h.notify("thread/tokenUsage/updated", params.clone());
    params["tokenUsage"]["total"]["inputTokens"] = json!(40);
    h.notify("thread/tokenUsage/updated", params);
    assert_eq!(usage_events(h.events()).len(), 2);
}

/// Not in TypeScript, which only listens for `exit`: a host that loses the process reports `Failed`, and the runner
/// must still learn that the session ended.
#[tokio::test]
async fn reports_a_lost_app_server_as_an_exit_with_its_error() {
    let mut h = session(true).await;
    h.exit
        .fire(CliExit::Failed("The Codex process was lost".into()));
    assert_eq!(
        h.events(),
        vec![ProviderEvent::Exit {
            error: Some("The Codex process was lost".into()),
            completed_turn: None
        }]
    );
    assert_eq!(h.codex.pid(), None);
}

// Worker boundaries (T3.4; covered in TypeScript by the runner and worker real tests).

#[tokio::test]
async fn grants_the_project_and_extra_folders_before_starting_the_server() {
    let h = build(
        true,
        None,
        |options| options.extra_dirs = vec![r"C:\home".into()],
        "test-model",
        None,
    );
    h.codex.start().await.unwrap();
    assert_eq!(
        *h.host.grants.lock().unwrap(),
        vec![vec![
            std::path::PathBuf::from(r"C:\bots\b1"),
            std::path::PathBuf::from(r"C:\home")
        ]]
    );
}

#[tokio::test]
async fn holds_a_worker_servers_exit_until_its_tree_has_gone_and_reports_an_unconfirmed_tree() {
    for confirmed in [true, false] {
        let (proof, settle_proof) = providers_fakes::retirement();
        let mut h = build_with(true, None, |_| {}, "test-model", None, Some(proof));
        h.codex.start().await.unwrap();
        h.events();
        let exited = tokio::spawn(h.codex.wait_for_exit());
        h.exit_child(1);
        settle().await;
        assert_eq!(h.events(), vec![], "held until the proof");
        assert!(!exited.is_finished());
        settle_proof
            .send(if confirmed {
                Ok(())
            } else {
                Err("the job could not be closed".into())
            })
            .unwrap();
        let exited = exited.await.unwrap();
        settle().await;
        if confirmed {
            assert_eq!(exited, Ok(()));
            assert_eq!(
                h.events(),
                vec![ProviderEvent::Exit {
                    error: Some("Codex exited with code 1".into()),
                    completed_turn: None
                }]
            );
        } else {
            assert!(
                exited
                    .unwrap_err()
                    .message()
                    .contains("could not confirm process-tree termination")
            );
            assert_eq!(
                h.events(),
                vec![ProviderEvent::TurnDone(TurnResult {
                    ok: false,
                    error: Some("The isolated provider could not confirm process-tree exit. Its account stays reserved.".into()),
                })]
            );
        }
    }
}
