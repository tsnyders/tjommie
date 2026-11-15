//! The provider fixtures (src/provider_fixtures.rs): what a session plays, how turns are found and used up, what fails,
//! and that the host the daemon uses with them starts nothing. Nothing here starts a real CLI, touches an account or
//! uses the network.
#![cfg(feature = "provider-fixtures")]

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::TempRoot;
use daemon::bots::StoredBot;
use daemon::cli::{CliCommand, CliHost, LaunchOptions};
use daemon::provider_fixtures::{
    ENV_VAR, FixtureHost, Fixtures, file_stem, fixture_session_factory,
};
use daemon::providers::{
    Approval, ApprovalAnswer, Limit, LimitState, LimitWindow, ProviderEvent, ProviderSession,
    StartOptions, TokenUsage, TurnResult, Usage,
};
use daemon::setup::{Account, Setup};
use protocol::model::{
    Autonomy, BotColor, BotShape, CodexSandboxReadiness, Provider, ProviderSetup,
};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(150);

fn bot(name: &str) -> StoredBot {
    StoredBot {
        id: format!("bot-{name}"),
        name: name.to_owned(),
        color: BotColor::Blue,
        shape: BotShape::Circle,
        instructions: String::new(),
        provider: Provider::Claude,
        model: "sonnet".into(),
        account_id: None,
        autonomy: Autonomy::Ask,
        home_folder: String::new(),
        project_folder: None,
        browser: false,
        paused: false,
        can_message: Vec::new(),
        limited_until: None,
        created_at: "2026-01-01T00:00:00.000Z".into(),
    }
}

fn options(resume_id: Option<&str>) -> StartOptions {
    StartOptions {
        cwd: String::new(),
        extra_dirs: Vec::new(),
        model: "sonnet".into(),
        instructions: String::new(),
        autonomy: Autonomy::Ask,
        codex_sandbox_ready: false,
        allow_rules: Vec::new(),
        resume_id: resume_id.map(str::to_owned),
        resume_cost_usd: None,
        env: Default::default(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers: Vec::new(),
    }
}

/// A folder of fixture files in a temporary root, and the fixtures that read it.
struct Folder {
    root: TempRoot,
    dir: PathBuf,
    fixtures: Fixtures,
}

impl Folder {
    fn new() -> Self {
        let root = TempRoot::new();
        let dir = root.0.join("fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let fixtures = Fixtures::open(&dir).unwrap();
        Folder {
            root,
            dir,
            fixtures,
        }
    }

    fn write(&self, file: &str, contents: &Value) {
        std::fs::write(self.dir.join(file), contents.to_string()).unwrap();
    }

    fn write_text(&self, file: &str, contents: &str) {
        std::fs::write(self.dir.join(file), contents).unwrap();
    }

    /// A started session for the agent named `agent`, as the runner makes one.
    async fn session(
        &self,
        agent: &str,
    ) -> (Arc<dyn ProviderSession>, UnboundedReceiver<ProviderEvent>) {
        self.session_resuming(agent, None).await
    }

    async fn session_resuming(
        &self,
        agent: &str,
        resume_id: Option<&str>,
    ) -> (Arc<dyn ProviderSession>, UnboundedReceiver<ProviderEvent>) {
        let session =
            fixture_session_factory(self.fixtures.clone())(&bot(agent), options(resume_id))
                .expect("the factory makes a session");
        let events = session.subscribe();
        session.start().await.unwrap();
        (session, events)
    }
}

/// Every event up to and including the turn's end.
async fn turn(events: &mut UnboundedReceiver<ProviderEvent>) -> Vec<ProviderEvent> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("an event arrives in time")
            .expect("the session still sends events");
        let ended = matches!(event, ProviderEvent::TurnDone(_));
        seen.push(event);
        if ended {
            return seen;
        }
    }
}

/// The next event, which must come at once.
async fn next(events: &mut UnboundedReceiver<ProviderEvent>) -> ProviderEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("an event arrives in time")
        .expect("the session still sends events")
}

/// Nothing arrives for a moment: the session is waiting.
async fn quiet(events: &mut UnboundedReceiver<ProviderEvent>) {
    assert!(
        tokio::time::timeout(QUIET, events.recv()).await.is_err(),
        "the session should be waiting"
    );
}

fn session(id: &str) -> ProviderEvent {
    ProviderEvent::Session {
        cli_session_id: id.into(),
    }
}

fn text(delta: &str) -> ProviderEvent {
    ProviderEvent::Text {
        delta: delta.into(),
    }
}

fn done(ok: bool, error: Option<&str>) -> ProviderEvent {
    ProviderEvent::TurnDone(TurnResult {
        ok,
        error: error.map(str::to_owned),
    })
}

fn failure(events: &[ProviderEvent]) -> String {
    match events.last() {
        Some(ProviderEvent::TurnDone(TurnResult {
            ok: false,
            error: Some(error),
        })) => error.clone(),
        other => panic!("expected a failed turn, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Playing a turn

#[tokio::test]
async fn replays_a_turn_exactly() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ {
            "expect": "hello",
            "events": [
                { "type": "text", "delta": "Hel" },
                { "type": "text", "delta": "lo." },
                { "type": "text_end" },
                { "type": "tool", "id": "t1", "name": "Reading a file", "summary": "src/lib.rs" },
                { "type": "tool_done", "id": "t1", "ok": true },
                { "type": "tool", "id": "t2", "name": "Running a command", "summary": "cargo test" },
                { "type": "tool_done", "id": "t2", "ok": false },
                { "type": "limit", "state": "near", "used_percent": 91.5, "limit_id": "claude",
                  "resets_at": "2030-01-01T00:00:00.000Z",
                  "windows": [ { "window_key": "five_hour", "state": "near", "used_percent": 91.5,
                                 "duration_minutes": 300, "resets_at": "2030-01-01T00:00:00.000Z" } ] },
                { "type": "usage", "model": "sonnet", "input": 120, "cached_input": 80, "output": 9,
                  "reasoning": 3, "cost_usd": 0.0123 }
            ],
            "result": { "ok": true }
        } ] }),
    );
    let (session, mut events) = folder.session("Ada").await;
    session.send("hello there").await.unwrap();

    assert_eq!(
        turn(&mut events).await,
        vec![
            self::session("fixture-session"),
            text("Hel"),
            text("lo."),
            ProviderEvent::TextEnd,
            ProviderEvent::Tool {
                id: "t1".into(),
                name: "Reading a file".into(),
                summary: "src/lib.rs".into()
            },
            ProviderEvent::ToolDone {
                id: "t1".into(),
                ok: true
            },
            ProviderEvent::Tool {
                id: "t2".into(),
                name: "Running a command".into(),
                summary: "cargo test".into()
            },
            ProviderEvent::ToolDone {
                id: "t2".into(),
                ok: false
            },
            ProviderEvent::Limit(Limit {
                state: LimitState::Near,
                used_percent: Some(91.5),
                resets_at: Some("2030-01-01T00:00:00.000Z".into()),
                limit_id: Some("claude".into()),
                windows: Some(vec![LimitWindow {
                    window_key: "five_hour".into(),
                    duration_minutes: Some(300.0),
                    used_percent: Some(91.5),
                    resets_at: Some("2030-01-01T00:00:00.000Z".into()),
                    state: LimitState::Near,
                }]),
            }),
            ProviderEvent::Usage(Usage {
                model: "sonnet".into(),
                tokens: TokenUsage {
                    input: 120.0,
                    cached_input: 80.0,
                    output: 9.0,
                    reasoning: 3.0
                },
                cost_usd: Some(0.0123),
                total_cost_usd: None,
                event_id: None,
            }),
            done(true, None),
        ]
    );
    assert_eq!(folder.fixtures.host().launch_attempts(), 0);
}

#[tokio::test]
async fn a_failed_result_and_an_error_event_end_the_turn_with_their_error() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [
            { "events": [ { "type": "text", "delta": "Partial" } ],
              "result": { "ok": false, "error": "Out of tokens." } },
            { "events": [ { "type": "text", "delta": "Then" }, { "type": "error", "message": "The provider is down." } ] },
            { "events": [] },
        ] }),
    );
    let (session, mut events) = folder.session("Ada").await;

    session.send("one").await.unwrap();
    assert_eq!(
        turn(&mut events).await,
        vec![
            self::session("fixture-session"),
            text("Partial"),
            done(false, Some("Out of tokens."))
        ]
    );
    session.send("two").await.unwrap();
    assert_eq!(
        turn(&mut events).await,
        vec![text("Then"), done(false, Some("The provider is down."))]
    );
    // An empty turn is an ok turn with nothing in it.
    session.send("three").await.unwrap();
    assert_eq!(turn(&mut events).await, vec![done(true, None)]);
}

#[tokio::test]
async fn the_session_event_chooses_the_id_and_a_resumed_session_keeps_its_own() {
    let folder = Folder::new();
    folder.write(
        "chosen.json",
        &json!({ "version": 1, "turns": [
            { "events": [ { "type": "session", "id": "chosen-1" }, { "type": "text", "delta": "a" } ] },
            { "events": [ { "type": "session", "id": "chosen-2" }, { "type": "text", "delta": "b" } ] },
            { "events": [ { "type": "text", "delta": "c" } ] },
        ] }),
    );
    let (session, mut events) = folder.session("chosen").await;
    for (message, expected) in [
        (
            "1",
            vec![self::session("chosen-1"), text("a"), done(true, None)],
        ),
        // A later turn's own session event plays as written; one session announces itself once.
        (
            "2",
            vec![self::session("chosen-2"), text("b"), done(true, None)],
        ),
        ("3", vec![text("c"), done(true, None)]),
    ] {
        session.send(message).await.unwrap();
        assert_eq!(turn(&mut events).await, expected);
    }

    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ { "events": [] } ] }),
    );
    let (resumed, mut events) = folder.session_resuming("Other", Some("saved-9")).await;
    resumed.send("go").await.unwrap();
    assert_eq!(
        turn(&mut events).await,
        vec![self::session("saved-9"), done(true, None)]
    );
}

fn approval_fixture() -> Value {
    json!({ "version": 1, "turns": [ { "events": [
        { "type": "text", "delta": "Checking." },
        { "type": "text_end" },
        { "type": "approval", "id": "ask-1", "tool": "Bash", "summary": "Run git status",
          "detail": "{ \"command\": \"git status\" }", "rule": "Bash(git status:*)",
          "then": {
              "once": [ { "type": "text", "delta": "Once." } ],
              "always": [ { "type": "text", "delta": "Always." } ],
              "deny": [ { "type": "text", "delta": "Denied." },
                        { "type": "error", "message": "The person declined." } ]
          } },
        { "type": "text", "delta": "After." },
        { "type": "usage", "model": "sonnet", "input": 5, "output": 1 }
    ] } ] })
}

fn asked() -> ProviderEvent {
    ProviderEvent::Approval(Approval {
        id: "ask-1".into(),
        tool: "Bash".into(),
        summary: "Run git status".into(),
        detail: "{ \"command\": \"git status\" }".into(),
        rule: Some("Bash(git status:*)".into()),
    })
}

fn usage(input: f64, output: f64) -> ProviderEvent {
    ProviderEvent::Usage(Usage {
        model: "sonnet".into(),
        tokens: TokenUsage {
            input,
            cached_input: 0.0,
            output,
            reasoning: 0.0,
        },
        cost_usd: None,
        total_cost_usd: None,
        event_id: None,
    })
}

#[tokio::test]
async fn an_approval_waits_for_the_answer_and_then_plays_the_matching_branch() {
    for (answer, branch, rest) in [
        (
            ApprovalAnswer::Once,
            vec![
                text("Once."),
                text("After."),
                usage(5.0, 1.0),
                done(true, None),
            ],
            "once",
        ),
        (
            ApprovalAnswer::Always,
            vec![
                text("Always."),
                text("After."),
                usage(5.0, 1.0),
                done(true, None),
            ],
            "always",
        ),
        (
            // The deny branch ends the turn with an error, so what follows the approval never plays.
            ApprovalAnswer::Deny,
            vec![text("Denied."), done(false, Some("The person declined."))],
            "deny",
        ),
    ] {
        let folder = Folder::new();
        folder.write("default.json", &approval_fixture());
        let (session, mut events) = folder.session("Ada").await;
        session.send("run it").await.unwrap();

        assert_eq!(next(&mut events).await, self::session("fixture-session"));
        assert_eq!(next(&mut events).await, text("Checking."));
        assert_eq!(next(&mut events).await, ProviderEvent::TextEnd);
        assert_eq!(next(&mut events).await, asked());
        // The turn stays open until it is answered, however long that takes. A wrong id changes nothing.
        quiet(&mut events).await;
        session.answer("not-the-id", answer);
        quiet(&mut events).await;

        session.answer("ask-1", answer);
        assert_eq!(turn(&mut events).await, branch, "the {rest} branch");
        // Answered once only.
        session.answer("ask-1", ApprovalAnswer::Deny);
        quiet(&mut events).await;
    }
}

#[tokio::test]
async fn an_unanswered_approval_ends_when_the_turn_is_stopped_and_the_session_goes_on() {
    let folder = Folder::new();
    let mut fixture = approval_fixture();
    fixture["turns"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "events": [ { "type": "text", "delta": "Back." } ] }));
    folder.write("default.json", &fixture);
    let (session, mut events) = folder.session("Ada").await;
    session.send("run it").await.unwrap();
    assert_eq!(next(&mut events).await, self::session("fixture-session"));
    next(&mut events).await;
    next(&mut events).await;
    assert_eq!(next(&mut events).await, asked());

    // Stopping ends the turn here, as Claude's result line does; the call itself reports nothing handled.
    assert!(!session.interrupt().await.unwrap());
    let stopped = turn(&mut events).await;
    assert_eq!(
        stopped,
        vec![done(
            false,
            Some("Provider fixtures: the turn was stopped.")
        )]
    );
    // Stopping with no turn running does nothing.
    assert!(!session.interrupt().await.unwrap());
    quiet(&mut events).await;

    session.send("next").await.unwrap();
    assert_eq!(
        turn(&mut events).await,
        vec![text("Back."), done(true, None)]
    );
}

#[tokio::test]
async fn closing_ends_the_session_and_nothing_plays_after_it() {
    let folder = Folder::new();
    folder.write("default.json", &approval_fixture());
    let (session, mut events) = folder.session("Ada").await;
    session.send("run it").await.unwrap();
    assert_eq!(next(&mut events).await, self::session("fixture-session"));
    next(&mut events).await;
    next(&mut events).await;
    assert_eq!(next(&mut events).await, asked());

    session.close();
    assert_eq!(
        next(&mut events).await,
        ProviderEvent::Exit {
            error: None,
            completed_turn: None
        }
    );
    tokio::time::timeout(WAIT, session.wait_for_exit())
        .await
        .expect("waiting for the exit ends")
        .unwrap();
    session.answer("ask-1", ApprovalAnswer::Once);
    session.send("again").await.unwrap();
    quiet(&mut events).await;
    assert_eq!(session.pid(), None);
    // Closing twice is harmless.
    session.close();
}

#[tokio::test]
async fn steering_is_refused_so_a_message_sent_mid_turn_runs_as_the_next_turn() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ { "events": [] } ] }),
    );
    let (session, _events) = folder.session("Ada").await;
    assert!(!session.steer("more").await.unwrap());
}

#[tokio::test]
async fn delay_pauses_before_each_event() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ { "delay_ms": 60, "events": [
            { "type": "text", "delta": "a" }, { "type": "text", "delta": "b" } ] } ] }),
    );
    let (session, mut events) = folder.session("Ada").await;
    let started = std::time::Instant::now();
    session.send("go").await.unwrap();
    turn(&mut events).await;
    assert!(
        started.elapsed() >= Duration::from_millis(120),
        "two events, 60 ms before each"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Finding turns, and what fails

#[tokio::test]
async fn turns_are_used_in_order_across_sessions_and_each_file_has_its_own_cursor() {
    let folder = Folder::new();
    let reply = |name: &str| {
        json!({ "version": 1, "turns": [
            { "events": [ { "type": "text", "delta": format!("{name} one") } ] },
            { "events": [ { "type": "text", "delta": format!("{name} two") } ] },
        ] })
    };
    folder.write("default.json", &reply("default"));
    folder.write("Ada.json", &reply("Ada"));

    // The runner can close an idle session and make another: the cursor carries on.
    let (first, mut events) = folder.session("Ada").await;
    first.send("1").await.unwrap();
    assert_eq!(turn(&mut events).await[1], text("Ada one"));
    first.close();
    let (second, mut events) = folder.session("Ada").await;
    second.send("2").await.unwrap();
    assert_eq!(turn(&mut events).await[1], text("Ada two"));

    // Agents without a file share default.json's cursor.
    let (grace, mut grace_events) = folder.session("Grace").await;
    grace.send("1").await.unwrap();
    assert_eq!(turn(&mut grace_events).await[1], text("default one"));
    let (alan, mut alan_events) = folder.session("Alan").await;
    alan.send("2").await.unwrap();
    assert_eq!(turn(&mut alan_events).await[1], text("default two"));
}

#[tokio::test]
async fn a_missing_fixture_fails_the_turn_clearly_and_starts_nothing() {
    let folder = Folder::new();
    let (session, mut events) = folder.session("Ada").await;
    session.send("hello").await.unwrap();
    let ended = turn(&mut events).await;
    assert_eq!(
        failure(&ended),
        format!(
            "Provider fixtures: no fixture for agent \"Ada\" in {} (looked for Ada.json and default.json).",
            folder.dir.display()
        )
    );
    assert_eq!(ended.len(), 1, "a turn that can't start says nothing else");
    assert_eq!(folder.fixtures.host().launch_attempts(), 0);

    // The file may show up later: the next message finds it.
    folder.write(
        "Ada.json",
        &json!({ "version": 1, "turns": [ { "events": [ { "type": "text", "delta": "Now." } ] } ] }),
    );
    session.send("hello").await.unwrap();
    assert_eq!(turn(&mut events).await[1], text("Now."));
}

#[tokio::test]
async fn an_agent_with_a_file_of_its_own_never_falls_back_to_default_when_it_is_broken() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ { "events": [ { "type": "text", "delta": "default" } ] } ] }),
    );
    folder.write_text("Ada.json", "{ not json");
    let (session, mut events) = folder.session("Ada").await;
    session.send("hello").await.unwrap();
    let error = failure(&turn(&mut events).await);
    assert!(
        error.starts_with(&format!(
            "Provider fixtures: {} is not a valid fixture file: ",
            folder.dir.join("Ada.json").display()
        )),
        "{error}"
    );
}

#[tokio::test]
async fn too_many_messages_fail_with_the_turn_count() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ { "events": [] }, { "events": [] } ] }),
    );
    let (session, mut events) = folder.session("Ada").await;
    for _ in 0..2 {
        session.send("go").await.unwrap();
        assert_eq!(turn(&mut events).await.last(), Some(&done(true, None)));
    }
    for _ in 0..2 {
        session.send("go").await.unwrap();
        assert_eq!(
            failure(&turn(&mut events).await),
            "Provider fixtures: default.json has 2 turns; turn 3 has no fixture."
        );
    }
}

#[tokio::test]
async fn an_expectation_that_the_message_misses_fails_the_turn_and_uses_it_up() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [
            { "expect": "needle", "events": [ { "type": "text", "delta": "first" } ] },
            { "expect": "needle", "events": [ { "type": "text", "delta": "second" } ] },
        ] }),
    );
    let (session, mut events) = folder.session("Ada").await;

    session.send("no match here").await.unwrap();
    assert_eq!(
        failure(&turn(&mut events).await),
        "Provider fixtures: turn 1 of default.json expects the message to contain \"needle\", but the message was \"no match here\"."
    );
    // The failed turn was used up, so the next message plays the next turn.
    session.send("a needle in it").await.unwrap();
    assert_eq!(turn(&mut events).await[1], text("second"));
}

#[tokio::test]
async fn an_expectation_failure_never_shows_more_than_200_characters_of_the_message() {
    let folder = Folder::new();
    folder.write(
        "default.json",
        &json!({ "version": 1, "turns": [ { "expect": "needle", "events": [] } ] }),
    );
    let (session, mut events) = folder.session("Ada").await;
    let message = format!("{}{}", "x".repeat(400), "TAIL");
    session.send(&message).await.unwrap();
    let error = failure(&turn(&mut events).await);
    assert!(
        error.contains(&format!("...{}TAIL", "x".repeat(196))),
        "{error}"
    );
    assert!(!error.contains(&"x".repeat(197)), "{error}");
}

#[tokio::test]
async fn a_bad_fixture_file_fails_the_turn_with_its_path_and_never_panics() {
    let folder = Folder::new();
    let cases: [(&str, String); 9] = [
        (r#"{ "version": 1, "turns": "#, "EOF while parsing".into()),
        (
            r#"{ "version": 2, "turns": [] }"#,
            "its version is 2; this daemon reads version 1.".into(),
        ),
        (r#"{ "turns": [] }"#, "missing field `version`".into()),
        (
            r#"{ "version": 1, "turns": [], "extra": 1 }"#,
            "unknown field `extra`".into(),
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [], "when": 1 } ] }"#,
            "unknown field `when`".into(),
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [ { "type": "text", "delta": "x", "bold": true } ] } ] }"#,
            "unknown field `bold`".into(),
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [ { "type": "shout" } ] } ] }"#,
            "unknown variant `shout`".into(),
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [ { "type": "error", "message": "a" }, { "type": "text_end" } ] } ] }"#,
            "turn 1: an error event must be the last event of its list.".into(),
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [], "delay_ms": 600000 } ] }"#,
            "turn 1: delay_ms must be at most 60000.".into(),
        ),
    ];
    for (contents, reason) in cases {
        folder.write_text("default.json", contents);
        let fixtures = Fixtures::open(&folder.dir).unwrap();
        let session = fixture_session_factory(fixtures)(&bot("Ada"), options(None)).unwrap();
        let mut events = session.subscribe();
        session.start().await.unwrap();
        session.send("hello").await.unwrap();
        let error = failure(&turn(&mut events).await);
        assert!(
            error.starts_with(&format!(
                "Provider fixtures: {} is not a valid fixture file: ",
                folder.dir.join("default.json").display()
            )),
            "{contents}: {error}"
        );
        assert!(error.contains(&reason), "{contents}: {error}");
    }
}

#[tokio::test]
async fn more_bad_files_and_a_text_with_a_byte_order_mark() {
    let folder = Folder::new();
    for (contents, reason) in [
        (
            r#"{ "version": 1, "turns": [ { "events": [ { "type": "error", "message": "a" } ], "result": { "ok": true } } ] }"#,
            "an error event already ends the turn, so it has no result.",
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [], "result": { "ok": true, "error": "x" } } ] }"#,
            "result has an error but ok is true.",
        ),
        (
            r#"{ "version": 1, "turns": [ { "events": [ { "type": "approval", "id": "a", "tool": "T", "summary": "S", "then": { "once": [ { "type": "error", "message": "m" }, { "type": "text_end" } ] } } ] } ] }"#,
            "an error event must be the last event of its list.",
        ),
    ] {
        folder.write_text("default.json", contents);
        let fixtures = Fixtures::open(&folder.dir).unwrap();
        let session = fixture_session_factory(fixtures)(&bot("Ada"), options(None)).unwrap();
        let mut events = session.subscribe();
        session.start().await.unwrap();
        session.send("hello").await.unwrap();
        let error = failure(&turn(&mut events).await);
        assert!(error.contains(reason), "{contents}: {error}");
    }

    // PowerShell writes a byte-order mark; it must not matter.
    folder.write_text(
        "default.json",
        &format!(
            "\u{feff}{}",
            json!({ "version": 1, "turns": [ { "events": [ { "type": "text", "delta": "bom" } ] } ] })
        ),
    );
    let (session, mut events) = folder.session("Ada").await;
    session.send("hello").await.unwrap();
    assert_eq!(turn(&mut events).await[1], text("bom"));
}

#[test]
fn agent_names_become_safe_file_names() {
    assert_eq!(file_stem("Ada"), "Ada");
    assert_eq!(file_stem("Ada Lovelace"), "Ada Lovelace");
    assert_eq!(file_stem("Q&A bot"), "Q_A bot");
    assert_eq!(file_stem("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
    assert_eq!(file_stem("..\\..\\x"), ".._.._x");
    assert_eq!(file_stem("Zoë"), "Zo_");
    assert_eq!(file_stem("v1.2-final_ok"), "v1.2-final_ok");
    // A device name must never be opened as a file.
    for device in ["CON", "con", "NUL", "aux", "PRN", "COM1", "lpt9", "CON.txt"] {
        assert!(file_stem(device).starts_with('_'), "{device}");
    }
    assert_eq!(file_stem("COM0"), "COM0");
    assert_eq!(file_stem("Console"), "Console");
    assert_eq!(file_stem(&"x".repeat(300)).len(), 100);
}

#[tokio::test]
async fn the_file_for_an_agent_is_found_by_its_safe_name_ignoring_case() {
    let folder = Folder::new();
    for (file, say) in [
        ("Q_A bot.json", "q and a"),
        ("ada lovelace.json", "ada"),
        ("default.json", "other"),
    ] {
        folder.write(
            file,
            &json!({ "version": 1, "turns": [ { "events": [ { "type": "text", "delta": say } ] } ] }),
        );
    }
    for (agent, expected) in [
        ("Q&A bot", "q and a"),
        ("Ada Lovelace", "ada"),
        ("Someone", "other"),
    ] {
        let (session, mut events) = folder.session(agent).await;
        session.send("hello").await.unwrap();
        assert_eq!(turn(&mut events).await[1], text(expected), "{agent}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The switch's variable

fn env(value: Option<&str>) -> impl Fn(&str) -> Option<OsString> {
    let value = value.map(OsString::from);
    move |name| {
        assert_eq!(name, ENV_VAR, "only the fixtures variable is read");
        value.clone()
    }
}

#[test]
fn the_variable_is_off_unless_it_is_set() {
    assert!(Fixtures::from_env(env(None)).unwrap().is_none());
    assert_eq!(ENV_VAR, "TJOMMIE_PROVIDER_FIXTURES");
}

#[test]
fn the_variable_must_be_an_absolute_existing_folder() {
    let root = TempRoot::new();
    let file = root.0.join("a-file.json");
    std::fs::write(&file, "{}").unwrap();
    let missing = root.0.join("nothing-here");
    let relative = "fixtures";

    let refused = |value: &str| match Fixtures::from_env(env(Some(value))) {
        Err(message) => message,
        Ok(_) => panic!("{value:?} should be refused"),
    };
    assert!(refused("").contains("is set but empty"));
    assert!(refused(relative).contains("must be an absolute path"));
    assert!(refused(relative).contains(relative));
    assert!(refused(".").contains("must be an absolute path"));
    assert!(refused(&missing.to_string_lossy()).contains("must name an existing folder"));
    assert!(refused(&file.to_string_lossy()).contains("must name an existing folder"));

    let fixtures = Fixtures::from_env(env(Some(&root.0.to_string_lossy())))
        .unwrap()
        .expect("a good folder turns fixtures on");
    assert_eq!(fixtures.dir(), root.0);
}

#[test]
fn the_shipped_example_files_are_valid_and_play() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider");
    let fixtures = Fixtures::open(&dir).unwrap();
    // default.json: three turns, then none; Ada.json: two.
    let default = |message: &str| fixtures.next_turn("Anyone", message);
    assert!(default("hello there").is_ok());
    assert!(default("anything").is_ok());
    assert!(default("anything").is_ok());
    assert!(
        default("anything")
            .unwrap_err()
            .contains("has 3 turns; turn 4")
    );
    assert!(fixtures.next_turn("Ada", "check git status").is_ok());
    assert!(fixtures.next_turn("Ada", "hi").is_ok());
    assert!(
        fixtures
            .next_turn("Ada", "hi")
            .unwrap_err()
            .contains("Ada.json has 2 turns")
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// The host that starts nothing

#[tokio::test]
async fn the_host_refuses_every_launch_clearly_and_counts_the_asks() {
    let host = FixtureHost::default();
    assert_eq!(host.launch_attempts(), 0);
    let command = host
        .find_cli(Provider::Claude)
        .expect("providers look installed");
    let result = host
        .launch(
            &command,
            vec!["-p".into()],
            LaunchOptions {
                cwd: String::new(),
                env: Default::default(),
                claude_api_key: None,
            },
        )
        .await;
    let error = result.err().expect("nothing launches");
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    assert!(
        error.to_string().starts_with("Provider fixtures: "),
        "{error}"
    );
    assert!(error.to_string().contains("cannot be launched"), "{error}");
    assert_eq!(host.launch_attempts(), 1);

    // Even the real-looking command a review resolves from PATH is refused, never started.
    let real = CliCommand {
        file: "C:\\Users\\owner\\bin\\claude.exe".into(),
        args: Vec::new(),
        codex_package: None,
    };
    assert!(
        host.launch(
            &real,
            Vec::new(),
            LaunchOptions {
                cwd: String::new(),
                env: Default::default(),
                claude_api_key: None
            }
        )
        .await
        .is_err()
    );
    assert_eq!(host.launch_attempts(), 2);
}

#[tokio::test]
async fn providers_look_installed_and_signed_in_without_any_cli_or_network() {
    let folder = Folder::new();
    let host = folder.fixtures.host();
    let setup = Setup::new(host.clone(), folder.fixtures.usage_fetch());
    for provider in [Provider::Claude, Provider::Codex] {
        let command = host.find_cli(provider).unwrap();
        assert!(!Path::new(&command.file).exists(), "a placeholder, no path");
        let account = Account {
            id: "fixtures".into(),
            provider,
            label: "Fixtures".into(),
            folder: folder.dir.to_string_lossy().into_owned(),
        };
        let checked = setup.check_account_setup(&account).await.unwrap();
        let (fields, sandbox) = match checked {
            ProviderSetup::Claude(fields) => (fields, None),
            ProviderSetup::Codex(codex) => (codex.setup, Some(codex.sandbox)),
        };
        assert!(fields.installed, "{provider:?}");
        assert!(fields.signed_in, "{provider:?}");
        assert_eq!(fields.account.as_deref(), Some("fixtures@example.com"));
        assert_eq!(fields.error, None);
        assert!(fields.version.is_some());
        if provider == Provider::Codex {
            assert_eq!(sandbox, Some(CodexSandboxReadiness::Ready));
        }
    }
    // The model lists are canned too, and Codex's comes from the in-memory server.
    let db = common::test_db(&folder.root.0);
    setup
        .ensure_accounts(&db, &folder.root.0.join("accounts"), &folder.root.0)
        .unwrap();
    let claude = setup.list_models(&db, Provider::Claude).await.unwrap();
    assert!(claude.iter().any(|model| model.id == "sonnet"));
    let codex = setup.list_models(&db, Provider::Codex).await.unwrap();
    assert_eq!(codex.iter().filter(|model| model.is_default).count(), 1);
    assert!(!codex.is_empty());
    // Usage never reaches the network: Claude has no credential file here, and Codex's reading is canned.
    let accounts = folder.root.0.join("accounts");
    let usage = |provider| {
        let setup = setup.clone();
        let (accounts, folder) = (accounts.clone(), folder.dir.to_string_lossy().into_owned());
        async move {
            setup
                .check_account_usage(
                    &Account {
                        id: "fixtures".into(),
                        provider,
                        label: "Fixtures".into(),
                        folder,
                    },
                    &accounts,
                )
                .await
        }
    };
    assert!(matches!(
        usage(Provider::Claude).await,
        daemon::account_quota::AccountUsageRead::Unavailable { .. }
    ));
    assert!(matches!(
        usage(Provider::Codex).await,
        daemon::account_quota::AccountUsageRead::Available { .. }
    ));
    assert_eq!(host.launch_attempts(), 0, "setup launched nothing");
}

#[tokio::test]
async fn signing_in_and_preparing_the_sandbox_are_refused_and_a_launch_is_counted() {
    let folder = Folder::new();
    let host = folder.fixtures.host();
    let setup = Setup::new(host.clone(), folder.fixtures.usage_fetch());
    let account = |provider| Account {
        id: "fixtures".into(),
        provider,
        label: "Fixtures".into(),
        folder: folder.dir.to_string_lossy().into_owned(),
    };
    let ended = Arc::new(AtomicUsize::new(0));
    let on_done = |ended: &Arc<AtomicUsize>| -> daemon::setup::OnDone {
        let ended = ended.clone();
        Arc::new(move |_, _| {
            ended.fetch_add(1, Ordering::SeqCst);
        })
    };

    // Codex asks its (in-memory) app server, which refuses.
    let codex = setup
        .sign_in_account(&account(Provider::Codex), on_done(&ended))
        .await
        .expect_err("fixtures never sign in");
    assert!(
        codex.message.contains("Provider fixtures:"),
        "{}",
        codex.message
    );
    assert!(codex.message.contains("refused"), "{}", codex.message);
    let sandbox = setup
        .prepare_account_codex_sandbox(&account(Provider::Codex), Duration::from_secs(1))
        .await
        .expect_err("fixtures never set up a sandbox");
    assert!(sandbox.message.contains("refused"), "{}", sandbox.message);
    assert_eq!(
        host.launch_attempts(),
        0,
        "the in-memory server launches nothing"
    );

    // Claude's sign-in is a process: a path that tries to launch gets a clear error, and the ask is counted.
    let claude = setup
        .sign_in_account(&account(Provider::Claude), on_done(&ended))
        .await
        .expect_err("fixtures never sign in");
    assert!(
        claude.message.contains("Provider fixtures:"),
        "{}",
        claude.message
    );
    assert!(
        claude.message.contains("cannot be launched"),
        "{}",
        claude.message
    );
    assert_eq!(host.launch_attempts(), 1);
    assert_eq!(ended.load(Ordering::SeqCst), 2, "each failed attempt ended");
}
