//! The whole daemon, in process on a temporary data root, answering over the real WebSocket with provider fixtures on: a
//! message to an agent gets the scripted streamed reply and the `message.*` events the chat shows, a tool becomes
//! activity, an error ends the turn as an error message, and an approval card waits for `approvals.respond` and then
//! plays its branch. A fresh data root accepts the first message, because the fixture host makes both providers read as
//! installed and signed in. No CLI, account or network is touched: the fixture host's launch counter ends at 0.
#![cfg(feature = "provider-fixtures")]

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use common::{Client, TempRoot, connect};
use daemon::paths::data_paths;
use daemon::provider_fixtures::Fixtures;
use daemon::startup::{Auth, Daemon, Launch, Mode, start_in_with_fixtures};
use serde_json::{Value, json};

struct Run {
    daemon: Daemon,
    client: Client,
    fixtures: Fixtures,
    _root: TempRoot,
    next_id: i64,
}

impl Run {
    async fn start() -> Run {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider");
        let fixtures = Fixtures::open(&dir).unwrap();
        let root = TempRoot::new();
        // The test hooks stay off: with fixtures on, the daemon itself skips the real account check and the Codex data
        // ACL pass, and a desktop run strips these hooks anyway.
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
            _root: root,
            next_id: 0,
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let response = self.client.call(self.next_id, method, params).await;
        assert_eq!(response["ok"], true, "{method}: {response}");
        response["result"].clone()
    }

    async fn create_bot(&mut self, name: &str) -> Value {
        let bot = self
            .ok(
                "bots.create",
                json!({ "name": name, "color": "blue", "shape": "circle", "provider": "claude", "model": "sonnet",
                    "instructions": "Fixture test", "autonomy": "ask", "browser": false }),
            )
            .await;
        assert_eq!(
            bot["state"], "idle",
            "a fixtures daemon needs no setup: {bot}"
        );
        bot
    }

    /// Reads frames until the events so far satisfy `found`, keeping every event in `client.events`.
    async fn until(&mut self, what: &str, found: impl Fn(&[Value]) -> bool) {
        for _ in 0..500 {
            if found(&self.client.events) {
                return;
            }
            let frame = self.client.next().await;
            if frame["type"] == "event" {
                self.client.events.push(frame);
            }
        }
        panic!("never saw {what}");
    }

    /// Sends a message after forgetting the events so far, so the events kept are this turn's.
    async fn send(&mut self, bot_id: &str, text: &str) {
        self.client.events.clear();
        self.ok("chat.send", json!({ "botId": bot_id, "text": text }))
            .await;
    }

    /// The bot worked and is idle again: the turn is over, and every message of it has been sent.
    async fn idle(&mut self, bot_id: &str) {
        let id = bot_id.to_owned();
        self.until("the bot to work and then rest", move |events| {
            let states: Vec<&str> = events
                .iter()
                .filter(|event| {
                    event["event"] == "bot.updated" && event["data"]["id"] == id.as_str()
                })
                .filter_map(|event| event["data"]["state"].as_str())
                .collect();
            states
                .iter()
                .position(|state| *state != "idle")
                .is_some_and(|worked| states[worked..].contains(&"idle"))
        })
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

    async fn finish(self) {
        let Run {
            daemon,
            client,
            fixtures,
            ..
        } = self;
        client.close().await;
        daemon.close().await;
        assert_eq!(
            fixtures.host().launch_attempts(),
            0,
            "nothing asked to start a provider"
        );
    }
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_owned()
}

/// The finished text messages the bot wrote, in order.
fn finished_texts(run: &Run, bot_id: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for message in run.events("message.updated") {
        if message["author"] == bot_id
            && message["kind"] == "text"
            && message["status"] == "done"
            && !seen.contains(&message["id"])
        {
            seen.push(message["id"].clone());
        }
    }
    seen.iter()
        .map(|id| {
            run.events("message.updated")
                .iter()
                .rev()
                .find(|message| message["id"] == *id)
                .map(|message| message["body"].as_str().unwrap().to_owned())
                .unwrap()
        })
        .collect()
}

#[tokio::test]
async fn a_message_gets_the_scripted_reply_streamed_with_its_chat_events() {
    let mut run = Run::start().await;
    let grace = run.create_bot("Grace").await;
    let id = id_of(&grace);

    // default.json, turn 1: it expects "hello".
    run.send(&id, "hello there").await;
    run.idle(&id).await;

    let user = run
        .events("message.added")
        .into_iter()
        .find(|message| message["author"] == "you")
        .cloned()
        .expect("the person's message was added");
    assert_eq!(user["body"], "hello there");
    let reply = run
        .events("message.added")
        .into_iter()
        .find(|message| message["author"] == id.as_str() && message["kind"] == "text")
        .cloned()
        .expect("the reply started");
    assert_eq!(reply["status"], "streaming");
    let deltas: Vec<&str> = run
        .events("message.delta")
        .iter()
        .filter(|delta| delta["id"] == reply["id"])
        .map(|delta| delta["delta"].as_str().unwrap())
        .collect();
    assert_eq!(deltas, ["Hello", " from the fixture."]);
    assert_eq!(finished_texts(&run, &id), ["Hello from the fixture."]);
    let history = run.ok("chat.history", json!({ "botId": id })).await;
    let bodies: Vec<&str> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["kind"] == "text")
        .map(|message| message["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["hello there", "Hello from the fixture."]);
    // The scripted usage was recorded against the account.
    assert!(
        !run.events("usage.changed").is_empty(),
        "the usage event was recorded"
    );

    // Turn 2: a tool shows as activity, then text.
    run.send(&id, "look at it").await;
    run.idle(&id).await;
    let activity = run
        .events("message.updated")
        .into_iter()
        .rev()
        .find(|message| message["kind"] == "activity")
        .cloned()
        .expect("the tool was shown");
    assert_eq!(activity["status"], "done");
    assert_eq!(
        activity["data"]["items"],
        json!([{ "id": "tool-1", "name": "Reading a file", "summary": "src/lib.rs", "status": "done" }])
    );
    assert_eq!(finished_texts(&run, &id), ["The file looks fine."]);

    // Turn 3: an error ends the turn and shows as an error message with the scripted text.
    run.send(&id, "once more").await;
    run.idle(&id).await;
    let error = run
        .events("message.added")
        .into_iter()
        .find(|message| message["kind"] == "error")
        .cloned()
        .expect("the failure was shown");
    assert_eq!(error["body"], "The provider is down.");

    // Turn 4: default.json has no fourth turn, which says so rather than starting anything.
    run.send(&id, "and again").await;
    run.idle(&id).await;
    let exhausted = run
        .events("message.added")
        .into_iter()
        .find(|message| message["kind"] == "error")
        .cloned()
        .expect("the missing turn was shown");
    assert_eq!(
        exhausted["body"],
        "Provider fixtures: default.json has 3 turns; turn 4 has no fixture."
    );
    run.finish().await;
}

async fn approval_flow(decision: &str, reply: &str) {
    let mut run = Run::start().await;
    let ada = run.create_bot("Ada").await;
    let id = id_of(&ada);

    run.send(&id, "check git status").await;
    run.until("the approval card", |events| {
        events
            .iter()
            .any(|event| event["event"] == "approval.added")
    })
    .await;
    let approval = run.events("approval.added")[0].clone();
    assert_eq!(approval["tool"], "Bash");
    assert_eq!(approval["summary"], "Run git status");
    assert_eq!(approval["status"], "pending");
    // The card is in the chat, and the turn waits on it.
    let card = run
        .events("message.added")
        .into_iter()
        .find(|message| message["kind"] == "approval")
        .cloned()
        .expect("the approval card was added");
    assert_eq!(card["data"]["approvalId"], approval["id"]);
    assert_eq!(card["data"]["status"], "pending");
    assert_eq!(finished_texts(&run, &id), ["I will check the repository."]);
    let waiting = run.ok("bots.list", json!({})).await;
    let state = waiting
        .as_array()
        .unwrap()
        .iter()
        .find(|bot| bot["id"] == id.as_str())
        .unwrap()["state"]
        .clone();
    assert_eq!(state, "waiting");

    let answered = run
        .ok(
            "approvals.respond",
            json!({ "approvalId": approval["id"], "decision": decision }),
        )
        .await;
    assert_eq!(
        answered["status"],
        if decision == "deny" {
            "denied"
        } else {
            "allowed"
        }
    );
    run.idle(&id).await;
    assert!(
        !run.events("approval.resolved").is_empty(),
        "the card was resolved"
    );
    assert_eq!(
        finished_texts(&run, &id),
        ["I will check the repository.", reply]
    );
    // The turn the approval belonged to is over, so the next message plays turn 2.
    run.send(&id, "anything").await;
    run.idle(&id).await;
    assert_eq!(finished_texts(&run, &id), ["Ada here."]);
    run.finish().await;
}

#[tokio::test]
async fn an_approval_card_waits_and_then_plays_the_once_branch() {
    approval_flow("once", "The working tree is clean.").await;
}

#[tokio::test]
async fn an_approval_card_waits_and_then_plays_the_always_branch() {
    approval_flow(
        "always",
        "The working tree is clean, and I will not ask again.",
    )
    .await;
}

#[tokio::test]
async fn an_approval_card_waits_and_then_plays_the_deny_branch() {
    approval_flow("deny", "Understood. I will not run it.").await;
}

#[tokio::test]
async fn a_fresh_data_root_reports_both_providers_ready_and_signing_in_is_refused() {
    let mut run = Run::start().await;
    let setup = run.ok("setup.status", json!({})).await;
    for entry in setup.as_array().unwrap() {
        assert_eq!(entry["installed"], true, "{entry}");
        assert_eq!(entry["signedIn"], true, "{entry}");
        assert_eq!(entry["account"], "fixtures@example.com", "{entry}");
    }
    let models = run.ok("models.list", json!({ "provider": "codex" })).await;
    assert!(!models.as_array().unwrap().is_empty());

    // Sign-in is refused. The handlers answer a failed start with their own plain sentence (the cause stays in the
    // log), and Claude's sign-in, which is a process, is the one that asks the host to launch.
    for provider in ["claude", "codex"] {
        run.next_id += 1;
        let response = run
            .client
            .call(run.next_id, "setup.signIn", json!({ "provider": provider }))
            .await;
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(response["error"]["code"], "bad_state", "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Sign-in could not start"),
            "{response}"
        );
    }
    // Claude's sign-in is a process, so it asked the host once, and was refused.
    assert_eq!(run.fixtures.host().launch_attempts(), 1);
    let bots = run.ok("bots.list", json!({})).await;
    assert_eq!(bots, json!([]));
    // Not `finish`: that would assert no launch was asked for, and the Claude sign-in just did.
    run.client.close().await;
    run.daemon.close().await;
}

#[tokio::test]
async fn a_setup_change_says_both_providers_are_signed_in_on_the_event_as_well_as_in_setup_status()
{
    let mut run = Run::start().await;
    // `setup.status` checks both providers, and its own `setup.changed` carries the same reports.
    let setup = run.ok("setup.status", json!({})).await;
    assert_eq!(setup.as_array().map(Vec::len), Some(2), "{setup}");
    let changed = run.events("setup.changed");
    assert!(!changed.is_empty(), "the setup check sent setup.changed");
    for reports in changed {
        let entries = reports.as_array().unwrap();
        assert_eq!(entries.len(), 2, "{reports}");
        for entry in entries {
            assert_eq!(entry["installed"], true, "{entry}");
            assert_eq!(entry["signedIn"], true, "{entry}");
            assert_eq!(entry["account"], "fixtures@example.com", "{entry}");
        }
    }
    run.finish().await;
}
