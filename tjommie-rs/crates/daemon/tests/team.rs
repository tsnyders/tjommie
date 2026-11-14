//! Ported from `packages/daemon/src/team.test.ts`: the `Team tools`, `Team room` and `Shared file snapshots` describes, in
//! the same order. `beforeEach` is [`setup`]. Junctions are made with `mklink /J` (see [`make_junction`]), so the cases
//! that need one are `#[cfg(windows)]`.

mod team_support;

use team_support::*;

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use daemon::approvals::get_approval;
use daemon::bots::{StoredBot, delete_bot, set_paused, update_bot};
use daemon::chat::{NewMessage, add_message, history};
use daemon::queue::{NewWork, WorkItem, WorkSource, WorkStatus, enqueue, new_chain, set_status};
use daemon::rooms::create_room;
use daemon::routines::list_routines;
use daemon::runner::{TurnOutcome, TurnResult};
use daemon::server::RpcError;
use daemon::settings::set_settings;
use daemon::team::{file_inside, get_exchange};
use protocol::model::{
    ApprovalDecision, ApprovalStatus, BotPatch, Exchange, ExchangeStatus, Message, MessageKind,
    MessageStatus, Room, RoomInput, SettingsPatch, TEAM_ROOM_ID,
};
use serde_json::{Value, json};

/// The share limit, plus one byte: a source this big is refused.
const OVER_LIMIT: u64 = 200 * 1024 * 1024 + 1;

/// `beforeEach`: Writer may message Researcher, and Researcher may message Writer.
fn setup() -> (Fixture, StoredBot, StoredBot) {
    let fx = fixture();
    let writer = fx.bot(
        "Writer",
        "blue",
        "circle",
        "Writes one-page summaries.\nMore detail here.",
        "codex",
        "gpt-5.6-sol",
    );
    let researcher = fx.bot(
        "Researcher",
        "teal",
        "droplet",
        "Finds sources.",
        "claude",
        "sonnet",
    );
    let researcher = fx.set_can_message(&researcher.id, &[writer.id.as_str()]);
    let writer = fx.set_can_message(&writer.id, &[researcher.id.as_str()]);
    (fx, writer, researcher)
}

/// `tool.run(botId, args)` on a refused call: the message the bot reads.
fn refused(result: Result<String, String>) -> String {
    match result {
        Ok(text) => panic!("the call should have been refused, but it returned: {text}"),
        Err(message) => message,
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `history(db, conversation, undefined, limit)`.
fn chat(fx: &Fixture, conversation: &str, limit: u32) -> Vec<Message> {
    fx.db(|db| history(db, conversation, None, limit).unwrap())
}

/// `history(...).at(-1)`: the newest message in the conversation.
fn newest(fx: &Fixture, conversation: &str) -> Message {
    chat(fx, conversation, 1).remove(0)
}

fn bodies(fx: &Fixture, conversation: &str, limit: u32) -> Vec<String> {
    chat(fx, conversation, limit)
        .into_iter()
        .map(|message| message.body)
        .collect()
}

/// A message's `data` as JSON; `Null` when it has none.
fn data_of(message: &Message) -> Value {
    message.data.clone().map_or(Value::Null, Value::Object)
}

fn exchange_of(fx: &Fixture, id: &str) -> Exchange {
    fx.db(|db| get_exchange(db, id).unwrap())
}

fn delivered_items(fx: &Fixture) -> Vec<WorkItem> {
    recorded(&fx.delivered).clone()
}

fn set_current(fx: &Fixture, bot_id: &str, item: WorkItem) {
    fx.current.lock().unwrap().insert(bot_id.to_owned(), item);
}

/// `chatTurn`: a turn the person started in the bot's own chat, with a chain of its own.
fn chat_turn(fx: &Fixture, bot_id: &str) -> WorkItem {
    let item = fx.db(|db| {
        let mut work = NewWork::new(bot_id, WorkSource::You, "hi");
        work.chain_id = Some(new_chain(db).unwrap());
        enqueue(db, &work).unwrap()
    });
    set_current(fx, bot_id, item.clone());
    item
}

/// `done(text, textMessageId)`.
fn done(text: &str, text_message_id: Option<&str>) -> TurnResult {
    TurnResult {
        outcome: TurnOutcome::Done,
        text: Some(text.to_owned()),
        text_message_id: text_message_id.map(str::to_owned),
        error: None,
    }
}

fn failed(error: &str) -> TurnResult {
    TurnResult {
        outcome: TurnOutcome::Failed,
        text: None,
        text_message_id: None,
        error: Some(error.to_owned()),
    }
}

/// `team.turnEnded(item, result)`.
fn finish(fx: &Fixture, item: &WorkItem, result: TurnResult) -> Result<(), RpcError> {
    fx.team_call(|team, host| team.turn_ended(host, item, &result))
}

/// `addMessage(db, { ... kind: "text" ... })`, straight into the database.
fn add_text(fx: &Fixture, conversation: &str, author: &str, body: &str) -> Message {
    fx.db(|db| {
        add_message(
            db,
            &NewMessage {
                conversation_id: conversation.to_owned(),
                author: author.to_owned(),
                kind: MessageKind::Text,
                body: body.to_owned(),
                data: None,
                status: MessageStatus::Done,
            },
        )
        .unwrap()
    })
}

/// `updateBot(db, id, patch)`.
fn patch_bot(fx: &Fixture, bot_id: &str, patch: BotPatch) -> StoredBot {
    fx.db(|db| update_bot(db, bot_id, &patch).unwrap())
}

/// `createRoom(db, { name, botIds })`.
fn make_room(fx: &Fixture, name: &str, bot_ids: &[&str]) -> Room {
    let input = RoomInput {
        name: name.to_owned(),
        bot_ids: bot_ids.iter().map(|id| (*id).to_owned()).collect(),
    };
    fx.db(|db| create_room(db, &input).unwrap())
}

/// `team.postToRoom(roomId, text)`.
fn post(fx: &Fixture, room_id: &str, text: &str) -> Message {
    fx.team_call(|team, host| team.post_to_room(host, room_id, text).unwrap())
}

fn set_chain_limit(fx: &Fixture, limit: u32) {
    fx.db(|db| {
        set_settings(
            db,
            &SettingsPatch {
                chain_limit: Some(limit),
                ..Default::default()
            },
        )
        .unwrap();
    });
}

/// `SELECT status FROM work_items WHERE id = ?`.
fn status_of(fx: &Fixture, item_id: &str) -> String {
    fx.db(|db| {
        db.query_row(
            "SELECT status FROM work_items WHERE id = ?1",
            [item_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
    })
}

/// The shared-file tests' `request()`: Researcher asks Writer for a file, and Writer's turn is that request.
fn request(fx: &Fixture, researcher_id: &str, writer_id: &str) -> WorkItem {
    chat_turn(fx, researcher_id);
    fx.run_tool(
        "message_bot",
        researcher_id,
        json!({ "bot": "Writer", "message": "Send a file." }),
    )
    .unwrap();
    let item = delivered_items(fx).remove(0);
    set_current(fx, writer_id, item.clone());
    item
}

/// Makes a directory junction (`mklink /J`) at `link` pointing to `target`. False when the OS refuses.
#[cfg(windows)]
fn make_junction(link: &Path, target: &Path) -> bool {
    daemon::spawn::command("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .is_ok_and(|status| status.success())
}

/// `truncateSync(path, length)`: sets the file's length without writing its content.
fn grow_to(path: &Path, length: u64) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(length)
        .unwrap();
}

mod team_tools {
    use super::*;

    #[test]
    fn records_a_named_login_request_in_its_current_conversation_without_accessing_a_vault() {
        let (fx, writer, _researcher) = setup();
        let writer = patch_bot(
            &fx,
            &writer.id,
            BotPatch {
                browser: Some(true),
                ..Default::default()
            },
        );
        let work = chat_turn(&fx, &writer.id);
        let reply = fx
            .run_tool(
                "request_login",
                &writer.id,
                json!({ "name": "Example website" }),
            )
            .unwrap();
        assert!(reply.contains("No credential was read or filled"));
        let message = newest(&fx, &work.conversation_id);
        assert!(message.body.contains("Login requested: Example website"));
        assert!(message.data.is_none());
        assert!(fx.event_names().iter().any(|name| name == "message.added"));
    }

    #[test]
    fn refuses_login_requests_from_paused_bots_or_with_secret_bearing_parameters() {
        let (fx, writer, _researcher) = setup();
        patch_bot(
            &fx,
            &writer.id,
            BotPatch {
                browser: Some(true),
                ..Default::default()
            },
        );
        // `toThrow()`: any refusal, here an unknown key.
        fx.run_tool(
            "request_login",
            &writer.id,
            json!({ "name": "Example", "password": "synthetic-forbidden" }),
        )
        .unwrap_err();
        fx.db(|db| set_paused(db, &writer.id, true).unwrap());
        let message =
            refused(fx.run_tool("request_login", &writer.id, json!({ "name": "Example" })));
        assert!(message.contains("Resume this agent"));
    }

    #[test]
    fn lists_only_the_bots_on_the_callers_can_message_list() {
        let (fx, _writer, researcher) = setup();
        assert_eq!(
            fx.run_tool("list_bots", &researcher.id, json!({})).unwrap(),
            "- Writer (idle): Writes one-page summaries."
        );
        let loner = fx.bot("Loner", "gray", "pill", "", "claude", "haiku");
        let text = fx.run_tool("list_bots", &loner.id, json!({})).unwrap();
        assert!(text.contains("can't message any bots yet"));
    }

    #[test]
    fn delivers_a_request_shows_it_in_both_chats_and_the_room_and_sends_the_answer_and_files_back()
    {
        let (fx, writer, researcher) = setup();
        let chat_item = chat_turn(&fx, &researcher.id);
        assert_eq!(
            fx.run_tool(
                "message_bot",
                &researcher.id,
                json!({ "bot": "@writer", "message": "Turn these notes into a summary." }),
            )
            .unwrap(),
            "Sent. Writer will reply."
        );
        let request = delivered_items(&fx).remove(0);
        let exchange_id = request.exchange_id.clone().unwrap();
        assert_eq!(request.bot_id, writer.id);
        assert_eq!(request.source, WorkSource::BotRequest);
        assert_eq!(request.conversation_id, exchange_id);
        assert_eq!(request.chain_id, chat_item.chain_id);
        assert!(request.text.contains("Message from Researcher"));
        for bot_id in [&researcher.id, &writer.id] {
            let last = newest(&fx, bot_id);
            assert_eq!(last.kind, MessageKind::Handoff);
            assert_eq!(last.body, "Turn these notes into a summary.");
            assert_eq!(data_of(&last)["exchangeId"], exchange_id);
            assert_eq!(data_of(&last)["messages"], 1);
            assert_eq!(data_of(&last)["status"], "waiting");
        }
        let room = chat(&fx, TEAM_ROOM_ID, 5);
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].author, researcher.id);
        assert_eq!(room[0].body, "Turn these notes into a summary.");
        assert_eq!(data_of(&room[0])["exchangeId"], exchange_id);
        assert_eq!(data_of(&room[0])["toBotId"], writer.id);

        // Writer's turn: it shares a file, then answers. The runner writes the answer into the exchange.
        set_current(&fx, &writer.id, request.clone());
        fs::write(
            Path::new(&writer.home_folder).join("summary.md"),
            "# Summary",
        )
        .unwrap();
        assert_eq!(
            fx.run_tool("share_file", &writer.id, json!({ "path": "summary.md" }))
                .unwrap(),
            "Shared summary.md."
        );
        add_text(&fx, &exchange_id, &writer.id, "Here's the summary.");
        finish(&fx, &request, done("Here's the summary.", None)).unwrap();

        let exchange = exchange_of(&fx, &exchange_id);
        assert_eq!(exchange.status, ExchangeStatus::Replied);
        assert_eq!(exchange.reply.as_deref(), Some("Here's the summary."));
        let items = delivered_items(&fx);
        let reply = &items[1];
        assert_eq!(reply.bot_id, researcher.id);
        assert_eq!(reply.source, WorkSource::BotReply);
        assert_eq!(reply.conversation_id, researcher.id);
        assert_eq!(reply.exchange_id.as_deref(), Some(exchange_id.as_str()));
        assert!(reply.text.contains("Writer replied"));
        assert!(reply.text.contains("Here's the summary."));
        assert!(reply.text.contains("summary.md"));
        let inbox_copy = Path::new(&researcher.home_folder)
            .join("inbox")
            .join("summary.md");
        assert_eq!(fs::read_to_string(inbox_copy).unwrap(), "# Summary");
        let handoff = chat(&fx, &researcher.id, 5)
            .into_iter()
            .find(|message| message.kind == MessageKind::Handoff)
            .unwrap();
        assert_eq!(data_of(&handoff)["messages"], 2);
        assert_eq!(data_of(&handoff)["status"], "replied");
        let last = newest(&fx, TEAM_ROOM_ID);
        assert_eq!(last.author, writer.id);
        assert_eq!(last.body, "Here's the summary.");
        assert_eq!(data_of(&last)["toBotId"], researcher.id);
        assert_eq!(data_of(&last)["files"][0]["name"], "summary.md");
    }

    #[test]
    fn refuses_unknown_bots_bots_off_the_list_and_the_caller_itself() {
        let (fx, _writer, researcher) = setup();
        fx.bot("Loner", "gray", "pill", "", "claude", "haiku");
        let message = refused(fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Nobody", "message": "hi" }),
        ));
        assert!(message.contains("no bot called Nobody"));
        let message = refused(fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Loner", "message": "hi" }),
        ));
        assert!(message.contains("not allowed to message Loner"));
        let message = refused(fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Researcher", "message": "hi" }),
        ));
        assert!(message.contains("can't message yourself"));
        assert!(delivered_items(&fx).is_empty());
    }

    #[test]
    fn stops_two_bots_asking_each_other_in_a_loop_at_the_chain_limit_chain_limit() {
        let (fx, writer, researcher) = setup();
        set_chain_limit(&fx, 2);
        chat_turn(&fx, &researcher.id);
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Ask me something back." }),
        )
        .unwrap();
        let first = delivered_items(&fx).remove(0);
        set_current(&fx, &writer.id, first.clone());
        fx.run_tool(
            "message_bot",
            &writer.id,
            json!({ "bot": "Researcher", "message": "What do you need?" }),
        )
        .unwrap();
        assert_eq!(delivered_items(&fx)[1].chain_id, first.chain_id);
        let message = refused(fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Again." }),
        ));
        assert!(message.starts_with("Limit reached, ask the user"));
        // The person's next message starts a new chain.
        chat_turn(&fx, &researcher.id);
        assert_eq!(
            fx.run_tool(
                "message_bot",
                &researcher.id,
                json!({ "bot": "Writer", "message": "Fresh start." }),
            )
            .unwrap(),
            "Sent. Writer will reply."
        );
    }

    #[test]
    fn tells_the_asking_bot_when_the_other_bot_couldnt_answer() {
        let (fx, _writer, researcher) = setup();
        chat_turn(&fx, &researcher.id);
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Summarize." }),
        )
        .unwrap();
        let asked = delivered_items(&fx).remove(0);
        finish(&fx, &asked, failed("Codex exited with code 1")).unwrap();
        let items = delivered_items(&fx);
        assert_eq!(items[1].bot_id, researcher.id);
        assert_eq!(items[1].source, WorkSource::BotReply);
        assert!(
            items[1]
                .text
                .contains("Writer couldn't finish your request")
        );
        assert!(items[1].text.contains("Codex exited with code 1"));
        assert_eq!(
            exchange_of(&fx, asked.exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
    }

    #[test]
    fn ends_a_deleted_askers_exchanges_without_errors_requester_deleted() {
        let (fx, writer, researcher) = setup();
        chat_turn(&fx, &researcher.id);
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "First." }),
        )
        .unwrap();
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Second." }),
        )
        .unwrap();
        let items = delivered_items(&fx);
        let (started, waiting) = (items[0].clone(), items[1].clone());
        fx.db(|db| set_status(db, &started.id, WorkStatus::Running).unwrap());
        let deleting = fx.team_call(|team, host| team.bot_deleting(host, &researcher.id).unwrap());
        fx.db(|db| delete_bot(db, &researcher.id).unwrap());
        fx.team_call(|team, host| team.bot_deleted(host, &deleting).unwrap());
        assert_eq!(status_of(&fx, &waiting.id), "failed");
        assert_eq!(
            exchange_of(&fx, waiting.exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
        set_current(&fx, &writer.id, started.clone());
        finish(&fx, &started, done("Done anyway.", None)).unwrap();
        // Nothing goes back to the deleted bot.
        assert_eq!(delivered_items(&fx).len(), 2);
        assert_eq!(
            exchange_of(&fx, started.exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
    }

    #[test]
    fn tells_the_asking_bot_when_the_bot_it_asked_is_deleted() {
        let (fx, writer, researcher) = setup();
        chat_turn(&fx, &researcher.id);
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Summarize." }),
        )
        .unwrap();
        let deleting = fx.team_call(|team, host| team.bot_deleting(host, &writer.id).unwrap());
        fx.db(|db| delete_bot(db, &writer.id).unwrap());
        fx.team_call(|team, host| team.bot_deleted(host, &deleting).unwrap());
        let items = delivered_items(&fx);
        assert_eq!(items[1].bot_id, researcher.id);
        assert_eq!(items[1].source, WorkSource::BotReply);
        assert!(items[1].text.contains("Writer was deleted"));
        assert_eq!(
            exchange_of(&fx, items[0].exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
    }

    #[cfg(windows)]
    #[test]
    fn shares_only_existing_files_inside_the_bots_own_folders_share_outside_folder() {
        let (fx, writer, _researcher) = setup();
        let home = PathBuf::from(&writer.home_folder);
        let outside = fx.root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "no").unwrap();
        fs::write(home.join("ok.txt"), "yes").unwrap();
        assert!(
            make_junction(&home.join("link"), &outside),
            "mklink /J refused"
        );
        assert_eq!(
            fx.run_tool(
                "share_file",
                &writer.id,
                json!({ "path": "ok.txt", "caption": "The summary" }),
            )
            .unwrap(),
            "Shared ok.txt."
        );
        let card = newest(&fx, &writer.id);
        assert_eq!(card.kind, MessageKind::File);
        assert_eq!(card.body, "The summary");
        assert_eq!(data_of(&card)["name"], "ok.txt");
        assert_eq!(data_of(&card)["size"], 3);
        let escapes = [
            path_arg(
                &Path::new("..")
                    .join("..")
                    .join("outside")
                    .join("secret.txt"),
            ),
            path_arg(&outside.join("secret.txt")),
            path_arg(&Path::new("link").join("secret.txt")),
        ];
        for path in &escapes {
            let message = refused(fx.run_tool("share_file", &writer.id, json!({ "path": path })));
            assert!(
                message.contains("only share files from your own folders"),
                "{path}: {message}"
            );
        }
        let message =
            refused(fx.run_tool("share_file", &writer.id, json!({ "path": "missing.txt" })));
        assert!(message.contains("no file"));
    }

    #[test]
    fn proposes_a_routine_as_a_card_and_creates_it_only_when_the_person_approves() {
        let (fx, writer, _researcher) = setup();
        let reply = fx
            .run_tool(
                "add_routine",
                &writer.id,
                json!({
                    "name": "Morning check",
                    "days": ["mon", "tue", "wed", "thu", "fri"],
                    "times": ["08:48"],
                    "prompt": "Check pnet.co.za",
                    "time_limit_minutes": 60,
                }),
            )
            .unwrap();
        assert!(reply.starts_with("Proposed"));
        let first = fx.events_named("approval.added")[0].clone();
        assert_eq!(first["kind"], "routine");
        assert_eq!(
            first["summary"],
            "Add a routine: Morning check, weekdays at 08:48, stop after 1 hour."
        );
        let first_id = first["id"].as_str().unwrap().to_owned();
        assert!(
            fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap())
                .is_empty()
        );
        fx.team_call(|team, host| {
            team.answer_routine_proposal(host, &first_id, ApprovalDecision::Once)
                .unwrap()
        });
        let routines = fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap());
        assert_eq!(routines.len(), 1);
        assert_eq!(routines[0].name, "Morning check");
        assert_eq!(routines[0].days, vec![1u8, 2, 3, 4, 5]);
        assert_eq!(routines[0].times, vec!["08:48".to_owned()]);
        assert_eq!(routines[0].time_limit_minutes, Some(60));

        fx.run_tool(
            "add_routine",
            &writer.id,
            json!({ "name": "Other", "days": ["sat"], "times": ["10:00"], "prompt": "Tidy the inbox" }),
        )
        .unwrap();
        let second_id = fx.events_named("approval.added")[1]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        fx.team_call(|team, host| {
            team.answer_routine_proposal(host, &second_id, ApprovalDecision::Deny)
                .unwrap()
        });
        assert_eq!(
            fx.db(|db| list_routines(db, Some(writer.id.as_str())).unwrap())
                .len(),
            1
        );
        assert_eq!(
            fx.db(|db| get_approval(db, &second_id).unwrap()).status,
            ApprovalStatus::Denied
        );
    }
}

mod team_room {
    use super::*;

    #[test]
    fn mentions_only_custom_room_members() {
        let (fx, writer, researcher) = setup();
        let outsider = fx.bot("Outsider", "gray", "pill", "", "claude", "haiku");
        let room = make_room(
            &fx,
            "Planning",
            &[writer.id.as_str(), researcher.id.as_str()],
        );
        post(&fx, &room.id, "@everyone plan together");
        let items = delivered_items(&fx);
        let mut bot_ids: Vec<String> = items.iter().map(|item| item.bot_id.clone()).collect();
        bot_ids.sort();
        let mut expected = vec![writer.id.clone(), researcher.id.clone()];
        expected.sort();
        assert_eq!(bot_ids, expected);
        let conversations: Vec<String> = items
            .iter()
            .map(|item| item.conversation_id.clone())
            .collect();
        assert_eq!(conversations, vec![room.id.clone(), room.id.clone()]);
        assert!(items[0].text.contains("Team room \"Planning\""));
        recorded(&fx.delivered).clear();
        post(&fx, &room.id, &format!("@{} please help", outsider.name));
        assert!(delivered_items(&fx).is_empty());
    }

    #[test]
    fn reads_mentions_against_every_bots_name_so_a_members_name_inside_a_non_members_doesnt_start_the_member()
     {
        let (fx, _writer, _researcher) = setup();
        let bot = fx.bot("Bot", "gray", "pill", "", "claude", "haiku");
        fx.bot("Bot 2", "gray", "pill", "", "claude", "haiku");
        let room = make_room(&fx, "Solo", &[bot.id.as_str()]);
        post(&fx, &room.id, "@Bot 2 please help");
        assert!(delivered_items(&fx).is_empty());
        post(&fx, &room.id, "@Bot please help");
        let bot_ids: Vec<String> = delivered_items(&fx)
            .iter()
            .map(|item| item.bot_id.clone())
            .collect();
        assert_eq!(bot_ids, vec![bot.id.clone()]);
    }

    #[test]
    fn keeps_custom_room_replies_there_and_records_their_room_id_in_the_handoff() {
        let (fx, writer, _researcher) = setup();
        let room = make_room(&fx, "Planning", &[writer.id.as_str()]);
        post(&fx, &room.id, "@Writer hi");
        let reply = add_text(&fx, &room.id, &writer.id, "Hello!");
        let item = delivered_items(&fx).remove(0);
        finish(&fx, &item, done("Hello!", Some(reply.id.as_str()))).unwrap();
        let last = newest(&fx, &room.id);
        assert_eq!(last.id, reply.id);
        assert_eq!(last.body, "Hello!");
        let handoff = newest(&fx, &writer.id);
        assert_eq!(handoff.kind, MessageKind::Handoff);
        assert_eq!(handoff.body, "Answered in Planning");
        assert_eq!(data_of(&handoff)["roomId"], room.id.as_str());
        assert_eq!(data_of(&handoff)["roomMessageId"], reply.id.as_str());
    }

    #[test]
    fn mirrors_exchanges_only_into_everyone_and_rooms_containing_both_bots() {
        let (fx, writer, researcher) = setup();
        let shared = make_room(&fx, "Shared", &[researcher.id.as_str(), writer.id.as_str()]);
        let solo = make_room(&fx, "Solo", &[researcher.id.as_str()]);
        chat_turn(&fx, &researcher.id);
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Write a report" }),
        )
        .unwrap();
        let item = delivered_items(&fx).remove(0);
        finish(&fx, &item, done("The report", None)).unwrap();
        for room_id in [TEAM_ROOM_ID, shared.id.as_str()] {
            assert_eq!(
                bodies(&fx, room_id, 5),
                vec!["Write a report", "The report"]
            );
        }
        assert!(chat(&fx, &solo.id, 5).is_empty());
    }

    #[test]
    fn mirrors_failure_notices_into_the_same_rooms_as_exchanges() {
        let (fx, writer, researcher) = setup();
        let shared = make_room(&fx, "Shared", &[researcher.id.as_str(), writer.id.as_str()]);
        let solo = make_room(&fx, "Solo", &[researcher.id.as_str()]);
        chat_turn(&fx, &researcher.id);
        fx.run_tool(
            "message_bot",
            &researcher.id,
            json!({ "bot": "Writer", "message": "Write a report" }),
        )
        .unwrap();
        let item = delivered_items(&fx).remove(0);
        finish(&fx, &item, failed("CLI exited")).unwrap();
        for room_id in [TEAM_ROOM_ID, shared.id.as_str()] {
            let messages = chat(&fx, room_id, 5);
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].body, "Write a report");
            assert_eq!(messages[1].kind, MessageKind::Notice);
            assert!(messages[1].body.contains("CLI exited"));
        }
        assert!(chat(&fx, &solo.id, 5).is_empty());
    }

    #[test]
    fn asks_each_bot_a_post_mentions_with_the_recent_room_messages_on_one_chain() {
        let (fx, writer, researcher) = setup();
        post(&fx, TEAM_ROOM_ID, "Morning all.");
        assert!(delivered_items(&fx).is_empty());
        let posted = post(&fx, TEAM_ROOM_ID, "@Writer and @researcher, what's new?");
        let items = delivered_items(&fx);
        let mut bot_ids: Vec<String> = items.iter().map(|item| item.bot_id.clone()).collect();
        bot_ids.sort();
        let mut expected = vec![writer.id.clone(), researcher.id.clone()];
        expected.sort();
        assert_eq!(bot_ids, expected);
        assert_eq!(items[0].source, WorkSource::Room);
        assert_eq!(items[0].conversation_id, TEAM_ROOM_ID);
        let message_ids: Vec<Option<String>> =
            items.iter().map(|item| item.message_id.clone()).collect();
        assert_eq!(
            message_ids,
            vec![Some(posted.id.clone()), Some(posted.id.clone())]
        );
        assert_eq!(items[0].chain_id, items[1].chain_id);
        assert!(items[0].text.contains("Person: Morning all."));
        assert!(items[0].text.contains("what's new?"));
    }

    #[test]
    fn notes_a_room_answer_in_the_bots_own_chat() {
        let (fx, writer, _researcher) = setup();
        post(&fx, TEAM_ROOM_ID, "@Writer hi");
        let item = delivered_items(&fx).remove(0);
        finish(&fx, &item, done("Hello!", Some("m1"))).unwrap();
        let handoff = newest(&fx, &writer.id);
        assert_eq!(handoff.kind, MessageKind::Handoff);
        assert_eq!(handoff.body, "Answered in Everyone");
        assert_eq!(data_of(&handoff)["roomId"], TEAM_ROOM_ID);
        assert_eq!(data_of(&handoff)["roomMessageId"], "m1");
    }
}

mod shared_file_snapshots {
    use super::*;

    fn inbox_file(bot: &StoredBot, name: &str) -> PathBuf {
        Path::new(&bot.home_folder).join("inbox").join(name)
    }

    #[cfg(windows)]
    #[test]
    fn delivers_the_snapshot_after_a_shared_source_is_swapped_for_an_outside_junction() {
        let (fx, writer, researcher) = setup();
        let item = request(&fx, &researcher.id, &writer.id);
        let folder = Path::new(&writer.home_folder).join("output");
        let outside = fx.root.path().join("private");
        fs::create_dir(&folder).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(folder.join("answer.txt"), "safe original").unwrap();
        fs::write(outside.join("answer.txt"), "private replacement").unwrap();
        fx.run_tool(
            "share_file",
            &writer.id,
            json!({ "path": "output/answer.txt" }),
        )
        .unwrap();
        fs::remove_dir_all(&folder).unwrap();
        assert!(make_junction(&folder, &outside), "mklink /J refused");
        finish(&fx, &item, done("Here it is.", None)).unwrap();
        assert_eq!(
            fs::read_to_string(inbox_file(&researcher, "answer.txt")).unwrap(),
            "safe original"
        );
    }

    #[test]
    fn delivers_the_snapshot_after_the_source_is_deleted() {
        let (fx, writer, researcher) = setup();
        let item = request(&fx, &researcher.id, &writer.id);
        let source = Path::new(&writer.home_folder).join("answer.txt");
        fs::write(&source, "safe original").unwrap();
        fx.run_tool(
            "share_file",
            &writer.id,
            json!({ "path": path_arg(&source) }),
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        finish(&fx, &item, done("Here it is.", None)).unwrap();
        assert_eq!(
            fs::read_to_string(inbox_file(&researcher, "answer.txt")).unwrap(),
            "safe original"
        );
    }

    #[test]
    fn delivers_the_bounded_snapshot_after_the_source_grows_beyond_the_share_limit() {
        let (fx, writer, researcher) = setup();
        let item = request(&fx, &researcher.id, &writer.id);
        let source = Path::new(&writer.home_folder).join("answer.txt");
        fs::write(&source, "safe original").unwrap();
        fx.run_tool(
            "share_file",
            &writer.id,
            json!({ "path": path_arg(&source) }),
        )
        .unwrap();
        grow_to(&source, OVER_LIMIT);
        finish(&fx, &item, done("Here it is.", None)).unwrap();
        let copy = inbox_file(&researcher, "answer.txt");
        assert_eq!(fs::metadata(&copy).unwrap().len(), 13);
        assert_eq!(fs::read_to_string(&copy).unwrap(), "safe original");
    }

    #[test]
    fn fails_visibly_without_publishing_a_reply_when_an_inbox_copy_fails() {
        let (fx, writer, researcher) = setup();
        let item = request(&fx, &researcher.id, &writer.id);
        fs::write(Path::new(&writer.home_folder).join("answer.txt"), "safe").unwrap();
        fx.run_tool("share_file", &writer.id, json!({ "path": "answer.txt" }))
            .unwrap();
        let inbox = Path::new(&researcher.home_folder).join("inbox");
        fs::remove_dir_all(&inbox).unwrap();
        fs::write(&inbox, "blocks copying").unwrap();
        fx.clear_events();
        finish(&fx, &item, done("The answer.", None)).unwrap();
        assert_eq!(
            exchange_of(&fx, item.exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
        assert!(
            !bodies(&fx, TEAM_ROOM_ID, 10)
                .iter()
                .any(|body| body == "The answer.")
        );
        assert!(
            delivered_items(&fx)[1]
                .text
                .contains("couldn't finish your request")
        );
        assert!(
            !fx.event_json()
                .iter()
                .any(|event| event["data"]["body"] == "The answer.")
        );
    }

    #[test]
    fn rejects_folders_and_oversized_sources() {
        let (fx, writer, _researcher) = setup();
        let home = PathBuf::from(&writer.home_folder);
        let message = file_inside(std::slice::from_ref(&home), &home.join("inbox")).unwrap_err();
        assert!(message.contains("folder"));
        let source = home.join("large.bin");
        fs::write(&source, "").unwrap();
        grow_to(&source, OVER_LIMIT);
        let message = refused(fx.run_tool(
            "share_file",
            &writer.id,
            json!({ "path": path_arg(&source) }),
        ));
        assert!(message.contains("200 MB"));
    }

    #[test]
    fn copies_every_snapshot_before_replying_and_removes_partial_deliveries_on_a_later_copy_failure()
     {
        let (fx, writer, researcher) = setup();
        let item = request(&fx, &researcher.id, &writer.id);
        let home = PathBuf::from(&writer.home_folder);
        for name in ["first.txt", "second.txt"] {
            fs::write(home.join(name), name).unwrap();
            fx.run_tool("share_file", &writer.id, json!({ "path": name }))
                .unwrap();
        }
        let card = newest(&fx, &item.conversation_id);
        let card_path = data_of(&card)["path"].as_str().unwrap().to_owned();
        assert_ne!(card_path, path_arg(&home.join("second.txt")));
        // Force the second copy to fail after the first succeeds.
        fs::remove_file(&card_path).unwrap();
        finish(&fx, &item, done("Both files.", None)).unwrap();
        assert_eq!(
            exchange_of(&fx, item.exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
        let inbox = Path::new(&researcher.home_folder).join("inbox");
        assert_eq!(fs::read_dir(&inbox).unwrap().count(), 0);
        assert!(
            !bodies(&fx, TEAM_ROOM_ID, 10)
                .iter()
                .any(|body| body == "Both files.")
        );
    }

    #[test]
    fn retains_a_file_card_snapshot_after_a_chat_source_is_deleted() {
        let (fx, writer, _researcher) = setup();
        let source = Path::new(&writer.home_folder).join("answer.txt");
        fs::write(&source, "card contents").unwrap();
        fx.run_tool(
            "share_file",
            &writer.id,
            json!({ "path": path_arg(&source) }),
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        let card = newest(&fx, &writer.id);
        let card_path = data_of(&card)["path"].as_str().unwrap().to_owned();
        assert_eq!(fs::read_to_string(card_path).unwrap(), "card contents");
    }

    #[test]
    fn removes_the_snapshot_when_the_file_card_cannot_be_saved() {
        let (fx, writer, _researcher) = setup();
        fs::write(
            Path::new(&writer.home_folder).join("answer.txt"),
            "card contents",
        )
        .unwrap();
        fx.db(|db| {
            db.execute_batch(
                "CREATE TRIGGER reject_file BEFORE INSERT ON messages WHEN NEW.kind = 'file' BEGIN SELECT RAISE(ABORT, 'card failed'); END",
            )
            .unwrap();
        });
        let message =
            refused(fx.run_tool("share_file", &writer.id, json!({ "path": "answer.txt" })));
        assert!(message.contains("card failed"));
        assert!(chat(&fx, &writer.id, 10).is_empty());
        assert_eq!(fs::read_dir(fx.shared_files()).unwrap().count(), 0);
    }

    #[test]
    fn allows_an_ordinary_project_but_rejects_another_bot_reached_through_an_ancestor_project() {
        let (fx, writer, researcher) = setup();
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("notes.txt"), "project notes").unwrap();
        patch_bot(
            &fx,
            &writer.id,
            BotPatch {
                project_folder: Some(Some(path_arg(project.path()))),
                ..Default::default()
            },
        );
        assert_eq!(
            fx.run_tool("share_file", &writer.id, json!({ "path": "notes.txt" }))
                .unwrap(),
            "Shared notes.txt."
        );
        // Simulate a legacy project selection that predates the project-folder guard.
        let root = path_arg(fx.root.path());
        fx.db(|db| {
            db.execute(
                "UPDATE bots SET project_folder = ?1 WHERE id = ?2",
                [root.as_str(), writer.id.as_str()],
            )
            .unwrap();
        });
        let other = Path::new(&researcher.home_folder).join("notes.md");
        let message = refused(fx.run_tool(
            "share_file",
            &writer.id,
            json!({ "path": path_arg(&other) }),
        ));
        assert!(message.contains("only share files from your own folders"));
    }

    #[test]
    fn rolls_back_a_reply_and_copied_files_and_fails_visibly_when_queuing_the_reply_fails() {
        let (fx, writer, researcher) = setup();
        let item = request(&fx, &researcher.id, &writer.id);
        fs::write(Path::new(&writer.home_folder).join("answer.txt"), "safe").unwrap();
        fx.run_tool("share_file", &writer.id, json!({ "path": "answer.txt" }))
            .unwrap();
        fx.db(|db| {
            db.execute_batch(
                "CREATE TRIGGER reject_reply BEFORE INSERT ON work_items WHEN NEW.source = 'bot_reply' BEGIN SELECT RAISE(ABORT, 'reply failed'); END",
            )
            .unwrap();
        });
        fx.clear_events();
        let error = finish(&fx, &item, done("Reply text.", None)).unwrap_err();
        assert!(error.message.contains("reply failed"));
        assert_eq!(
            exchange_of(&fx, item.exchange_id.as_deref().unwrap()).status,
            ExchangeStatus::Failed
        );
        assert!(!inbox_file(&researcher, "answer.txt").exists());
        assert!(
            !bodies(&fx, TEAM_ROOM_ID, 10)
                .iter()
                .any(|body| body == "Reply text.")
        );
        let notice = newest(&fx, TEAM_ROOM_ID);
        assert_eq!(notice.kind, MessageKind::Notice);
        assert!(notice.body.contains("the reply could not be saved"));
        assert!(
            !fx.event_json()
                .iter()
                .any(|event| event["data"]["body"] == "Reply text.")
        );
    }
}
