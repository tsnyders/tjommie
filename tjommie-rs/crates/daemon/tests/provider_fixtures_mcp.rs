//! An `mcp` fixture event calls one of the agent's own `tjommie` tools over its own MCP server, as the Claude CLI would.
//! The whole daemon runs in process on a temporary data root with provider fixtures on, so the call goes through the
//! real `/mcp` route, the real token check and the real team: Forge asks Scout through `message_bot`, Scout (a second
//! fixture agent, with a file of its own) answers, and the answer reaches Forge as a new message that plays Forge's next
//! turn; Forge shares a file from its own folder through `share_file`. No CLI, account or network is touched.
#![cfg(feature = "provider-fixtures")]

mod common;
mod team_support;

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{Client, TempRoot, connect};
use daemon::bots::StoredBot;
use daemon::paths::data_paths;
use daemon::provider_fixtures::{Fixtures, TJOMMIE_TOOLS, fixture_session_factory};
use daemon::providers::{ProviderEvent, ProviderSession, StartOptions};
use daemon::startup::{Auth, Daemon, Launch, Mode, start_in_with_fixtures};
use protocol::model::{Autonomy, BotColor, BotShape, Provider};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(30);

/// Forge asks Scout, then (on Scout's answer, which arrives as a message) reports it, then shares a file of its own.
const FORGE: &str = r#"{ "version": 1, "turns": [
  { "expect": "ask Scout", "events": [
      { "type": "mcp", "id": "m1", "tool": "message_bot", "arguments": { "bot": "Scout", "message": "Is the build green?" } },
      { "type": "text", "delta": "Asked Scout." }, { "type": "text_end" } ] },
  { "expect": "Yes, it is green.", "events": [
      { "type": "text", "delta": "Scout says the build is green." }, { "type": "text_end" } ] },
  { "expect": "share the summary", "events": [
      { "type": "mcp", "id": "m2", "tool": "share_file", "arguments": { "path": "summary.md", "caption": "Build summary" } },
      { "type": "mcp", "id": "m3", "tool": "share_file", "arguments": { "path": "missing.md" } },
      { "type": "text", "delta": "Shared it." }, { "type": "text_end" } ] } ] }"#;

const SCOUT: &str = r#"{ "version": 1, "turns": [
  { "expect": "Is the build green?", "events": [
      { "type": "text", "delta": "Yes, it is green." }, { "type": "text_end" } ] } ] }"#;

struct Run {
    daemon: Daemon,
    client: Client,
    fixtures: Fixtures,
    root: TempRoot,
    next_id: i64,
}

impl Run {
    async fn start(files: &[(&str, &str)]) -> Run {
        let root = TempRoot::new();
        let dir = root.0.join("provider-fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let fixtures = Fixtures::open(&dir).unwrap();
        let launch = Launch {
            mode: Mode::Development,
            port: 0,
            lan_port: 0,
            lan_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            skip_account_check: false,
            skip_folder_protection: false,
            accounts_dir: None,
        };
        let daemon = start_in_with_fixtures(
            &launch,
            &Auth::Development,
            root.0.clone(),
            Some(fixtures.clone()),
        )
        .await
        .expect("the daemon starts with fixtures on a temporary data root");
        let token = std::fs::read_to_string(data_paths(&root.0).unwrap().token).unwrap();
        let mut client = connect(daemon.port()).await;
        client.hello(token.trim()).await;
        Run {
            daemon,
            client,
            fixtures,
            root,
            next_id: 0,
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let response = self.client.call(self.next_id, method, params).await;
        assert_eq!(response["ok"], true, "{method}: {response}");
        response["result"].clone()
    }

    async fn create_bot(&mut self, name: &str, can_message: &[&str]) -> String {
        let bot = self
            .ok(
                "bots.create",
                json!({ "name": name, "color": "blue", "shape": "circle", "provider": "claude", "model": "sonnet",
                    "instructions": "Fixture test", "autonomy": "ask", "browser": false, "canMessage": can_message }),
            )
            .await;
        bot["id"].as_str().unwrap().to_owned()
    }

    /// The agent's own folder, which `share_file` reads from.
    fn home(&self, bot_id: &str) -> PathBuf {
        data_paths(&self.root.0).unwrap().bots.join(bot_id)
    }

    /// Reads frames until the events kept so far satisfy `found`.
    async fn until(&mut self, what: &str, found: impl Fn(&[Value]) -> bool) {
        let started = std::time::Instant::now();
        while !found(&self.client.events) {
            assert!(started.elapsed() < WAIT, "never saw {what}");
            if let Some(frame) = self.client.try_next(Duration::from_millis(200)).await
                && frame["type"] == "event"
            {
                self.client.events.push(frame);
            }
        }
    }

    async fn send(&mut self, bot_id: &str, text: &str) {
        self.ok("chat.send", json!({ "botId": bot_id, "text": text }))
            .await;
    }

    fn events(&self, name: &str) -> Vec<&Value> {
        self.client
            .events
            .iter()
            .filter(|event| event["event"] == name)
            .map(|event| &event["data"])
            .collect()
    }

    /// Waits until the bot has finished a text message whose body is `body`.
    async fn said(&mut self, bot_id: &str, body: &str) {
        let (id, body) = (bot_id.to_owned(), body.to_owned());
        self.until(&format!("{bot_id} saying {body}"), move |events| {
            events.iter().any(|event| {
                event["event"] == "message.updated"
                    && event["data"]["author"] == id.as_str()
                    && event["data"]["kind"] == "text"
                    && event["data"]["status"] == "done"
                    && event["data"]["body"] == body.as_str()
            })
        })
        .await;
    }

    /// The tool lines every activity message of the bot ended with, as `(name, summary, status)`, newest message last.
    fn tool_lines(&self, bot_id: &str) -> Vec<(String, String, String)> {
        let mut seen: Vec<(String, String, String)> = Vec::new();
        for message in self.events("message.updated") {
            if message["author"] != bot_id || message["kind"] != "activity" {
                continue;
            }
            for item in message["data"]["items"].as_array().into_iter().flatten() {
                let line = (
                    item["name"].as_str().unwrap_or_default().to_owned(),
                    item["summary"].as_str().unwrap_or_default().to_owned(),
                    item["status"].as_str().unwrap_or_default().to_owned(),
                );
                seen.retain(|known| (&known.0, &known.1) != (&line.0, &line.1));
                seen.push(line);
            }
        }
        seen
    }

    async fn finish(self) {
        let Run {
            daemon,
            client,
            fixtures,
            root,
            ..
        } = self;
        client.close().await;
        daemon.close().await;
        assert_eq!(
            fixtures.host().launch_attempts(),
            0,
            "nothing asked to start a provider"
        );
        drop(root);
    }
}

#[tokio::test]
async fn an_agent_asks_a_second_agent_through_message_bot_and_the_answer_plays_its_next_turn() {
    let mut run = Run::start(&[("Forge.json", FORGE), ("Scout.json", SCOUT)]).await;
    let scout = run.create_bot("Scout", &[]).await;
    let forge = run.create_bot("Forge", &[&scout]).await;

    run.send(&forge, "Please ask Scout about the build").await;
    // Forge's own words, then Scout's answer reaching Forge as a message that plays Forge's second turn.
    run.said(&forge, "Asked Scout.").await;
    run.said(&scout, "Yes, it is green.").await;
    run.said(&forge, "Scout says the build is green.").await;

    // The tool line reads as a real session's would, and the call went through: the exchange exists.
    assert_eq!(
        run.tool_lines(&forge),
        [(
            "Messaged Scout".to_owned(),
            "Messaged Scout".to_owned(),
            "done".to_owned()
        )]
    );
    let handoffs: Vec<&Value> = run
        .events("message.added")
        .into_iter()
        .filter(|message| message["kind"] == "handoff")
        .collect();
    assert!(
        handoffs
            .iter()
            .any(|handoff| handoff["data"]["fromBotId"] == forge.as_str()
                && handoff["data"]["toBotId"] == scout.as_str()),
        "a handoff row names the exchange: {handoffs:?}"
    );
    run.finish().await;
}

#[tokio::test]
async fn an_agent_shares_a_file_from_its_own_folder_and_a_missing_file_is_a_failed_line() {
    let mut run = Run::start(&[("Forge.json", FORGE), ("Scout.json", SCOUT)]).await;
    let scout = run.create_bot("Scout", &[]).await;
    let forge = run.create_bot("Forge", &[&scout]).await;
    // Forge's first two turns are the exchange; play them, then share.
    run.send(&forge, "Please ask Scout about the build").await;
    run.said(&forge, "Scout says the build is green.").await;
    std::fs::write(
        run.home(&forge).join("summary.md"),
        "# Build summary\n\nGreen.\n",
    )
    .unwrap();
    run.send(&forge, "Now share the summary").await;
    run.said(&forge, "Shared it.").await;

    let cards: Vec<&Value> = run
        .events("message.added")
        .into_iter()
        .filter(|message| message["kind"] == "file")
        .collect();
    assert_eq!(cards.len(), 1, "one file card: {cards:?}");
    assert_eq!(cards[0]["author"], forge.as_str());
    assert_eq!(cards[0]["body"], "Build summary");
    assert_eq!(cards[0]["data"]["name"], "summary.md");
    let lines = run.tool_lines(&forge);
    assert!(
        lines.contains(&(
            "Shared summary.md".to_owned(),
            "Shared summary.md".to_owned(),
            "done".to_owned()
        )),
        "{lines:?}"
    );
    // The file that is not there fails its line and not the turn: Forge still said "Shared it."
    assert!(
        lines.contains(&(
            "Shared missing.md".to_owned(),
            "Shared missing.md".to_owned(),
            "failed".to_owned()
        )),
        "{lines:?}"
    );
    run.finish().await;
}

#[tokio::test]
async fn the_event_lists_the_tools_the_servers_tools_list_has() {
    let fixture = team_support::fixture();
    let mut served: Vec<String> = fixture.tools().into_iter().map(|tool| tool.name).collect();
    let mut named: Vec<String> = TJOMMIE_TOOLS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    served.sort();
    named.sort();
    assert_eq!(named, served, "TJOMMIE_TOOLS drifted from Team::tools");
}

#[tokio::test]
async fn a_file_that_names_an_unknown_tool_fails_the_turn_with_its_path_and_the_reason() {
    let root = TempRoot::new();
    let dir = root.0.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Ada.json"),
        r#"{ "version": 1, "turns": [ { "events": [ { "type": "mcp", "id": "m1", "tool": "browser_navigate" } ] } ] }"#,
    )
    .unwrap();
    let fixtures = Fixtures::open(&dir).unwrap();
    let error = fixtures.next_turn("Ada", "hello").unwrap_err();
    assert!(
        error.contains("is not a valid fixture file")
            && error.contains("turn 1: mcp names a tool that is not one of Tjommie's own tools"),
        "{error}"
    );
}

#[tokio::test]
async fn an_mcp_event_without_a_tjommie_server_fails_the_turn_with_a_message() {
    let root = TempRoot::new();
    let dir = root.0.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Ada.json"),
        r#"{ "version": 1, "turns": [ { "events": [ { "type": "mcp", "id": "m1", "tool": "list_bots" } ] } ] }"#,
    )
    .unwrap();
    let fixtures = Fixtures::open(&dir).unwrap();
    let bot = StoredBot {
        id: "bot-ada".into(),
        name: "Ada".into(),
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
    };
    let options = StartOptions {
        cwd: String::new(),
        extra_dirs: Vec::new(),
        model: "sonnet".into(),
        instructions: String::new(),
        autonomy: Autonomy::Ask,
        codex_sandbox_ready: false,
        allow_rules: Vec::new(),
        resume_id: None,
        resume_cost_usd: None,
        env: Default::default(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers: Vec::new(),
    };
    let session: Arc<dyn ProviderSession> =
        fixture_session_factory(fixtures)(&bot, options).expect("the factory makes a session");
    let mut events = session.subscribe();
    session.start().await.unwrap();
    session.send("who is there").await.unwrap();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("an event arrives in time")
            .expect("the session still sends events");
        if let ProviderEvent::TurnDone(result) = event {
            assert!(!result.ok);
            assert_eq!(
                result.error.as_deref(),
                Some(
                    "Provider fixtures: this session has no tjommie server, so the mcp event has no tool to call."
                )
            );
            break;
        }
    }
    session.close();
}
