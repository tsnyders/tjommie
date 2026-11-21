//! The chat's view tests (T4.1): the component tests' behaviours, over a fake store, read back through the semantic
//! tree. Sample data is invented.

use gpui::{
    Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase, point, px,
};
use protocol::model::{BotState, Message, MessageKind, MessageStatus};
use protocol::wire::{MessagePage, method};
use serde_json::{Value, json};

use crate::model::state::{AppState, Conversation, ConversationStatus};
use crate::model::testing::FakeRpc;
use crate::ui::shell::route::{self, Route};
use crate::ui::store;
use crate::ui::testing::{app, bot, click, draw, node, room, state};

pub(super) fn message(id: &str, author: &str, body: &str, minute: usize) -> Message {
    Message {
        id: id.into(),
        conversation_id: "b1".into(),
        author: author.into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status: MessageStatus::Done,
        created_at: format!(
            "2026-11-15T{:02}:{:02}:00.000Z",
            8 + minute / 60,
            minute % 60
        ),
    }
}

pub(super) fn with(kind: MessageKind, data: Value, mut message: Message) -> Message {
    message.kind = kind;
    message.data = data.as_object().cloned();
    message
}

/// A turn of `count` messages, yours and Forge's in turn.
pub(super) fn turns(count: usize) -> Vec<Message> {
    (0..count)
        .map(|n| {
            let author = if n % 2 == 0 { "you" } else { "b1" };
            message(&format!("m{n}"), author, &format!("Message {n}"), n)
        })
        .collect()
}

/// Forge's chat, loaded with `messages`.
pub(super) fn chat(messages: Vec<Message>, has_more: bool) -> AppState {
    let mut state = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    state.conversations.insert(
        "b1".into(),
        Conversation {
            status: ConversationStatus::Ready,
            messages,
            has_more,
            pending: Vec::new(),
        },
    );
    state
}

pub(super) fn calls_to(rpc: &FakeRpc, name: &str) -> Vec<Value> {
    rpc.calls()
        .into_iter()
        .filter(|(method, _)| *method == name)
        .map(|(_, params)| params)
        .collect()
}

#[gpui::test]
fn opens_on_the_newest_page_and_loads_it_once(cx: &mut TestAppContext) {
    let page = turns(3);
    let rpc = FakeRpc::new().on::<method::ChatHistory>(move |_| {
        Ok(MessagePage {
            messages: page.clone(),
            has_more: false,
        })
    });
    let ready = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    let (cx, calls) = app(cx, Route::Chat("b1".into()), ready, rpc);
    draw(cx);
    draw(cx);
    assert!(node("bubble.m0").is_some(), "your first message shows");
    assert!(node("prose.m1").is_some(), "Forge's reply shows");
    let history = calls_to(&calls, "chat.history");
    assert_eq!(history.len(), 1, "one load: {history:?}");
    assert_eq!(history[0]["botId"], "b1");
    assert!(history[0].get("before").is_none(), "the newest page");
}

#[gpui::test]
fn a_long_thread_sits_at_the_bottom_and_draws_only_what_shows(cx: &mut TestAppContext) {
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(turns(300), false),
        FakeRpc::new(),
    );
    draw(cx);
    assert!(node("bubble.m298").is_some(), "the newest of yours shows");
    assert!(node("prose.m299").is_some(), "and the reply under it");
    assert!(node("bubble.m0").is_none(), "the oldest isn't drawn");
    let thread = cx.debug_bounds("chat.thread").unwrap();
    let last = cx.debug_bounds("prose.m299").unwrap();
    assert!(
        thread.bottom() - last.bottom() < px(48.),
        "bottom-anchored: {last:?} in {thread:?}"
    );
}

#[gpui::test]
fn your_own_message_always_comes_into_view(cx: &mut TestAppContext) {
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(turns(300), false),
        FakeRpc::new(),
    );
    draw(cx);
    let thread = cx.debug_bounds("screen.chat").unwrap().center();
    cx.simulate_event(ScrollEvent {
        position: thread,
        delta: ScrollDelta::Pixels(point(px(0.), px(6000.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    draw(cx);
    assert!(
        node("prose.m299").is_none(),
        "scrolled up, away from the newest"
    );
    cx.update(|_, cx| {
        store::update(cx, |state| {
            let messages = &mut state.conversations.get_mut("b1").unwrap().messages;
            messages.push(message("m300", "b1", "A reply you scrolled away from", 300));
            true
        });
    });
    draw(cx);
    assert!(
        node("prose.m300").is_none(),
        "an agent's message doesn't pull you down"
    );
    cx.update(|_, cx| {
        store::update(cx, |state| {
            let messages = &mut state.conversations.get_mut("b1").unwrap().messages;
            messages.push(message("m301", "you", "Also run clippy.", 301));
            true
        });
    });
    draw(cx);
    draw(cx);
    assert!(
        node("bubble.m301").is_some(),
        "your own message brings the thread to the bottom"
    );
}

#[gpui::test]
fn the_older_page_loads_when_the_top_is_in_reach(cx: &mut TestAppContext) {
    let older = vec![message("m-1", "b1", "An older reply", 0)];
    let rpc = FakeRpc::new().on::<method::ChatHistory>(move |_| {
        Ok(MessagePage {
            messages: older.clone(),
            has_more: false,
        })
    });
    let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(4), true), rpc);
    draw(cx);
    draw(cx);
    draw(cx);
    let history = calls_to(&calls, "chat.history");
    assert_eq!(history.len(), 1, "one older page: {history:?}");
    assert_eq!(
        history[0]["before"], "m0",
        "the page before the oldest message"
    );
    assert!(node("prose.m-1").is_some(), "the older reply shows above");
}

#[gpui::test]
fn an_empty_chat_says_the_agent_is_ready(cx: &mut TestAppContext) {
    let _ = app(
        cx,
        Route::Chat("b1".into()),
        chat(Vec::new(), false),
        FakeRpc::new(),
    );
    assert_eq!(node("chat.empty").unwrap().text, "Forge is ready");
}

#[gpui::test]
fn a_chat_that_didnt_load_offers_retry(cx: &mut TestAppContext) {
    let ready = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    let (cx, calls) = app(cx, Route::Chat("b1".into()), ready, FakeRpc::new());
    draw(cx);
    assert_eq!(
        node("chat.failed.text").unwrap().text,
        "Couldn't load this chat."
    );
    click(cx, "chat.failed.retry");
    draw(cx);
    assert_eq!(calls_to(&calls, "chat.history").len(), 2, "loaded again");
}

#[gpui::test]
fn a_deleted_agent_has_no_chat(cx: &mut TestAppContext) {
    let ready = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
    let (cx, _) = app(cx, Route::Chat("gone".into()), ready, FakeRpc::new());
    draw(cx);
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::Agents);
}

#[gpui::test]
fn a_reply_says_thinking_until_its_first_word_and_a_blank_one_shows_nothing(
    cx: &mut TestAppContext,
) {
    let mut typing = message("m1", "b1", "", 1);
    typing.status = MessageStatus::Streaming;
    let mut blank = message("m2", "b1", "\n\n", 2);
    blank.status = MessageStatus::Done;
    let (cx, _) = app(
        cx,
        Route::Chat("b1".into()),
        chat(vec![message("m0", "you", "Hi", 0), typing, blank], false),
        FakeRpc::new(),
    );
    assert!(
        node("thinking.m1").is_some(),
        "a reply with no words yet says Thinking"
    );
    assert!(
        node("prose.m2").is_none() && node("thinking.m2").is_none(),
        "a blank reply shows nothing"
    );
    cx.update(|_, cx| {
        store::update(cx, |state| {
            let messages = &mut state.conversations.get_mut("b1").unwrap().messages;
            messages[1].body = "**Done.**".into();
            true
        });
    });
    draw(cx);
    assert!(
        node("prose.m1").is_some() && node("thinking.m1").is_none(),
        "its words replace Thinking"
    );
}

#[gpui::test]
fn an_error_with_retry_runs_the_work_again(cx: &mut TestAppContext) {
    let error = with(
        MessageKind::Error,
        json!({ "retryMessageId": "m0" }),
        message("m1", "system", "Codex stopped unexpectedly.", 1),
    );
    let rpc = FakeRpc::new().on::<method::ChatRetry>(|_| Ok(message("m0", "you", "Hi", 0)));
    let (cx, calls) = app(
        cx,
        Route::Chat("b1".into()),
        chat(vec![message("m0", "you", "Hi", 0), error], false),
        rpc,
    );
    click(cx, "error.m1.retry");
    draw(cx);
    let retries = calls_to(&calls, "chat.retry");
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0]["messageId"], "m0");
}

#[gpui::test]
fn a_room_answer_opens_its_room(cx: &mut TestAppContext) {
    let answer = with(
        MessageKind::Handoff,
        json!({ "roomMessageId": "p1", "roomId": "r 1" }),
        message("m1", "b1", "Answered in Growth", 1),
    );
    // The room exists: the room screen sends an unknown one back to Rooms.
    let mut state = chat(vec![answer], false);
    state.rooms.push(room("r 1", "Growth", &["b1"]));
    let (cx, _) = app(cx, Route::Chat("b1".into()), state, FakeRpc::new());
    assert_eq!(node("handoff.m1").unwrap().text, "Answered in Growth");
    click(cx, "handoff.m1");
    assert_eq!(
        cx.update(|_, cx| route::current(cx)),
        Route::Room("r 1".into())
    );
}

#[gpui::test]
fn pause_asks_the_daemon(cx: &mut TestAppContext) {
    let paused = bot("b1", "Forge", BotState::Paused);
    let rpc = FakeRpc::new().on::<method::BotsPause>(move |_| Ok(paused.clone()));
    let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), rpc);
    assert_eq!(node("chat.b1.name").unwrap().text, "Forge");
    click(cx, "chat.b1.pause");
    draw(cx);
    let pauses = calls_to(&calls, "bots.pause");
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0], json!({ "id": "b1", "paused": true }));
}

mod exchange_view {
    use super::*;
    use protocol::wire::ExchangeView;

    fn handoff() -> Message {
        with(
            MessageKind::Handoff,
            json!({ "exchangeId": "x1", "fromBotId": "b1", "toBotId": "b2", "messages": 2, "status": "replied" }),
            message("m1", "b1", "Does the bundled SQLite match?", 1),
        )
    }

    fn team() -> AppState {
        let mut state = chat(vec![handoff()], false);
        state.bots.push(bot("b2", "Scout", BotState::Idle));
        // Everyone, which Open room goes to.
        let mut everyone = room("team", "Everyone", &["b1", "b2"]);
        everyone.everyone = true;
        state.rooms.push(everyone);
        state
    }

    fn exchanges() -> FakeRpc {
        FakeRpc::new()
            .on::<method::ExchangesGet>(|_| {
                let mut asked = message("e1", "b1", "Does the bundled SQLite match?", 1);
                asked.conversation_id = "x1".into();
                let mut answer = message("e2", "b2", "Yes, both are 3.50.", 2);
                answer.conversation_id = "x1".into();
                Ok(serde_json::from_value::<ExchangeView>(json!({
                    "messages": [asked, answer],
                    "hasMore": false,
                    "exchange": {
                        "id": "x1", "fromBotId": "b1", "toBotId": "b2", "request": "Does the bundled SQLite match?",
                        "reply": "Yes, both are 3.50.", "status": "replied",
                        "createdAt": "2026-11-15T08:01:00.000Z", "repliedAt": "2026-11-15T08:02:00.000Z"
                    }
                }))
                .unwrap())
            })
            .on::<method::RoomPost>(|_| Ok(message("p1", "you", "posted", 3)))
    }

    #[gpui::test]
    fn view_thread_opens_the_exchange_beside_the_chat(cx: &mut TestAppContext) {
        let (cx, calls) = app(cx, Route::Chat("b1".into()), team(), exchanges());
        click(cx, "exchange.x1.open");
        draw(cx);
        draw(cx);
        assert_eq!(node("exchange.x1.title").unwrap().text, "Forge asked Scout");
        assert!(
            node("bubble.e1").is_none() && node("prose.e2").is_some(),
            "the exchange's messages"
        );
        assert!(!calls_to(&calls, "exchanges.get").is_empty());
        assert_eq!(
            node("chat.b1.panel").unwrap().text,
            "Hide the panel",
            "the panel button closes it"
        );
    }

    #[gpui::test]
    fn replies_in_everyone_with_both_agents_mentioned(cx: &mut TestAppContext) {
        let (cx, calls) = app(cx, Route::Chat("b1".into()), team(), exchanges());
        click(cx, "exchange.x1.open");
        draw(cx);
        draw(cx);
        assert_eq!(
            node("exchange.x1.reply").unwrap().text,
            "Ask both agents in Everyone"
        );
        assert_eq!(
            node("exchange.x1.recipients").unwrap().text,
            "Reply recipients"
        );
        click(cx, "exchange.x1.reply");
        cx.simulate_input("Compare sqlite_master too.");
        cx.simulate_keystrokes("enter");
        draw(cx);
        let posts = calls_to(&calls, "room.post");
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(posts[0]["roomId"], "team");
        assert_eq!(posts[0]["text"], "@Forge @Scout Compare sqlite_master too.");
        assert_eq!(
            node("exchange.x1.posted").unwrap().text,
            "Posted in Everyone."
        );
        click(cx, "exchange.x1.room");
        assert_eq!(
            cx.update(|_, cx| route::current(cx)),
            Route::Room("team".into())
        );
    }

    #[gpui::test]
    fn closing_the_exchange_forgets_it(cx: &mut TestAppContext) {
        let (cx, _) = app(cx, Route::Chat("b1".into()), team(), exchanges());
        click(cx, "exchange.x1.open");
        draw(cx);
        draw(cx);
        assert!(cx.update(|_, cx| store::state(cx).conversations.contains_key("x1")));
        click(cx, "exchange.x1.close");
        draw(cx);
        assert!(node("exchange.x1.view").is_none());
        assert!(
            !cx.update(|_, cx| store::state(cx).conversations.contains_key("x1")),
            "forgotten"
        );
        // The header's panel button closes an open exchange too.
        click(cx, "exchange.x1.open");
        draw(cx);
        click(cx, "chat.b1.panel");
        draw(cx);
        assert!(node("exchange.x1.view").is_none());
    }
}

/// T4.4: a shared file's card, the viewer reading a Markdown file through the transfer, and the card offline.
mod file_card {
    use super::*;
    use base64::Engine as _;
    use protocol::attachments::{AttachmentChunk, AttachmentInfo, AttachmentOpen};
    use protocol::wire::Empty;

    use crate::model::state::Connection;

    /// The protocol wants a UUID.
    const FILE_ID: &str = "7d3f5b1e-2c4a-4e6b-9f80-1a2b3c4d5e6f";

    fn shared(name: &str, size: u64, content_type: &str) -> AppState {
        let file = with(
            MessageKind::File,
            json!({
                "path": format!(r"D:\Shared\{name}"), "name": name, "size": size,
                "attachmentId": FILE_ID, "contentType": content_type
            }),
            message("m1", "b1", "", 1),
        );
        chat(vec![file], false)
    }

    /// The daemon's side of one Markdown file: open, read in chunks as asked, close.
    fn serving(text: &'static str) -> FakeRpc {
        let size = text.len() as u64;
        FakeRpc::new()
            .on::<method::AttachmentsOpen>(move |_| {
                Ok(AttachmentOpen {
                    transfer_id: "t1".into(),
                    attachment: AttachmentInfo {
                        id: FILE_ID.into(),
                        host_id: "h1".into(),
                        name: "notes.md".into(),
                        size,
                        content_type: "text/markdown".into(),
                        sha256: "rev".into(),
                    },
                    chunk_bytes: 262_144,
                    expires_at: "2026-11-15T09:00:00.000Z".into(),
                })
            })
            .on::<method::AttachmentsRead>(move |params| {
                let offset = params["offset"].as_u64().unwrap();
                let end = (offset + params["length"].as_u64().unwrap()).min(size);
                let bytes = &text.as_bytes()[offset as usize..end as usize];
                Ok(AttachmentChunk {
                    transfer_id: "t1".into(),
                    revision: "rev".into(),
                    offset,
                    bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
                    eof: end == size,
                })
            })
            .on::<method::AttachmentsClose>(|_| Ok(Empty {}))
    }

    #[gpui::test]
    fn a_shared_file_is_a_card_with_its_actions(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Chat("b1".into()),
            shared("report.pdf", 2_516_582, "application/pdf"),
            FakeRpc::new(),
        );
        draw(cx);
        assert_eq!(node("file.m1.0").unwrap().text, "report.pdf");
        assert_eq!(
            node("file.m1.0.preview").unwrap().text,
            "Preview report.pdf"
        );
        assert_eq!(
            node("file.m1.0.download").unwrap().text,
            "Download report.pdf"
        );
        assert_eq!(
            node("file.m1.0.folder").unwrap().text,
            "Show report.pdf in its folder",
            "on this PC, the file's folder"
        );
    }

    #[gpui::test]
    fn the_viewer_reads_a_markdown_file_and_closes_its_transfer_with_it(cx: &mut TestAppContext) {
        let (cx, calls) = app(
            cx,
            Route::Chat("b1".into()),
            shared("notes.md", 26, "text/markdown"),
            serving("# Plan\n\nShip **T4.4** now."),
        );
        draw(cx);
        click(cx, "file.m1.0.preview");
        draw(cx);
        draw(cx);
        assert!(node("file.m1.0.viewer").is_some(), "the viewer is open");
        assert!(
            node("file.m1.0.viewer.status").is_none() && node("file.m1.0.viewer.problem").is_none(),
            "and its section is read"
        );
        let opens = calls_to(&calls, "attachments.open");
        assert_eq!(
            opens,
            [json!({ "messageId": "m1", "attachmentIndex": 0, "attachmentId": FILE_ID })]
        );
        assert_eq!(calls_to(&calls, "attachments.read")[0]["offset"], 0);
        assert!(calls_to(&calls, "attachments.close").is_empty());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(node("file.m1.0.viewer").is_none(), "Escape closes it");
        assert_eq!(
            calls_to(&calls, "attachments.close"),
            [json!({ "transferId": "t1" })],
            "and its transfer with it"
        );
    }

    #[gpui::test]
    fn offline_the_card_says_why_it_cannot_download(cx: &mut TestAppContext) {
        let mut state = shared("report.pdf", 2_516_582, "application/pdf");
        state.connection = Connection::Reconnecting;
        let (cx, calls) = app(cx, Route::Chat("b1".into()), state, FakeRpc::new());
        draw(cx);
        assert!(
            !node("file.m1.0.preview").unwrap().enabled,
            "no preview offline"
        );
        click(cx, "file.m1.0.download");
        draw(cx);
        assert_eq!(
            node("file.m1.0.status").unwrap().text,
            "The file host is offline. Reconnect to download it."
        );
        let asked: Vec<&str> = calls.calls().into_iter().map(|(name, _)| name).collect();
        assert!(
            !asked.iter().any(|name| name.starts_with("attachments.")),
            "nothing asked of the file host: {asked:?}"
        );
        assert_eq!(
            node("file.m1.0.preview").unwrap().text,
            "Preview report.pdf, unavailable",
            "M4 review C3: an unavailable Preview says so"
        );
    }

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use client::RpcError;
    use desktop_host::sha2::{Digest as _, Sha256};
    use gpui::{Image, ImageFormat, VisualTestContext};
    use protocol::wire::ErrorCode;

    /// A 1 x 1 PNG and a still 1 x 1 GIF, real enough for GPUI's decoder.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    const STILL_GIF: &str = "R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==";

    fn decoded(text: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .unwrap()
    }

    /// A 64 x 64 GIF of three frames, which no decoder is given.
    fn animated_gif() -> Vec<u8> {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[64, 0, 64, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
        for _ in 0..3 {
            gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 64, 0, 64, 0, 0, 2, 2, 0x4C, 0x01, 0]);
        }
        gif.push(0x3B);
        gif
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// The live runtime the app's file work runs on, for the test's life.
    fn runtime(cx: &mut VisualTestContext) -> tokio::runtime::Runtime {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        cx.update(|_, cx| cx.set_global(crate::live::Tokio(handle)));
        // The runtime's threads (a save's file writes) wake the app's tasks from outside the test's executor.
        cx.executor().allow_parking();
        runtime
    }

    /// Draws until `done`, giving the runtime's threads time: their wakes come from outside the test's executor.
    fn until(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..300 {
            draw(cx);
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out: {what}");
    }

    /// Shared files in Forge's chat: one file message each, `(message id, name, content type, bytes)`.
    fn files_chat(files: &[(&str, &str, &str, &[u8])]) -> AppState {
        let messages = files
            .iter()
            .enumerate()
            .map(|(at, (id, name, content_type, bytes))| {
                with(
                    MessageKind::File,
                    json!({
                        "path": format!(r"D:\Shared\{name}"), "name": name, "size": bytes.len(),
                        "attachmentId": FILE_ID, "contentType": content_type
                    }),
                    message(id, "b1", "", at),
                )
            })
            .collect();
        chat(messages, false)
    }

    /// The files a fake host serves: (message id, name, content type, bytes).
    type Served = Arc<Vec<(String, String, String, Vec<u8>)>>;

    /// The daemon's side of shared files, by message id: metadata, open, read in chunks, close. At most two transfers
    /// are open at once, as the daemon allows; a third is refused as busy. The pair is (open now, most open at once).
    fn host(files: &[(&str, &str, &str, &[u8])]) -> (FakeRpc, Arc<Mutex<(usize, usize)>>) {
        let served: Served = Arc::new(
            files
                .iter()
                .map(|(id, name, kind, bytes)| {
                    (
                        id.to_string(),
                        name.to_string(),
                        kind.to_string(),
                        bytes.to_vec(),
                    )
                })
                .collect(),
        );
        let leases = Arc::new(Mutex::new((0usize, 0usize)));
        let info = |served: &Served, id: &str| {
            let (_, name, kind, bytes) = served.iter().find(|(known, ..)| known == id).unwrap();
            AttachmentInfo {
                id: FILE_ID.into(),
                host_id: "h1".into(),
                name: name.clone(),
                size: bytes.len() as u64,
                content_type: kind.clone(),
                sha256: sha256(bytes),
            }
        };
        let (opening, reading, metadata) = (served.clone(), served.clone(), served);
        let (opened, closed) = (leases.clone(), leases.clone());
        let rpc = FakeRpc::new()
            .on::<method::AttachmentsMetadata>(move |params| {
                Ok(info(&metadata, params["messageId"].as_str().unwrap()))
            })
            .on::<method::AttachmentsOpen>(move |params| {
                let mut leases = opened.lock().unwrap();
                if leases.0 >= 2 {
                    return Err(RpcError::Daemon {
                        code: ErrorCode::Busy,
                        message: "Two file transfers are already active".into(),
                    });
                }
                leases.0 += 1;
                leases.1 = leases.1.max(leases.0);
                let id = params["messageId"].as_str().unwrap();
                Ok(AttachmentOpen {
                    transfer_id: id.into(),
                    attachment: info(&opening, id),
                    chunk_bytes: 262_144,
                    expires_at: "2026-11-15T09:00:00.000Z".into(),
                })
            })
            .on::<method::AttachmentsRead>(move |params| {
                let id = params["transferId"].as_str().unwrap();
                let (.., bytes) = reading.iter().find(|(known, ..)| known == id).unwrap();
                let offset = params["offset"].as_u64().unwrap();
                let end = (offset + params["length"].as_u64().unwrap()).min(bytes.len() as u64);
                Ok(AttachmentChunk {
                    transfer_id: id.into(),
                    revision: sha256(bytes),
                    offset,
                    bytes: base64::engine::general_purpose::STANDARD
                        .encode(&bytes[offset as usize..end as usize]),
                    eof: end == bytes.len() as u64,
                })
            })
            .on::<method::AttachmentsClose>(move |_| {
                let mut leases = closed.lock().unwrap();
                leases.0 = leases.0.saturating_sub(1);
                Ok(Empty {})
            });
        (rpc, leases)
    }

    /// M4 review C1: a reply quoted twenty thousand deep draws as plain text; parsed as written it was a stack overflow,
    /// which no guard catches, and the chat aborted the app on every open.
    #[gpui::test]
    fn a_reply_nested_thousands_deep_draws_as_text(cx: &mut TestAppContext) {
        let deep = format!("{}hello", ">".repeat(20_000));
        let state = chat(vec![message("m1", "b1", &deep, 1)], false);
        let (cx, _) = app(cx, Route::Chat("b1".into()), state, FakeRpc::new());
        draw(cx);
        assert!(node("prose.m1").is_some(), "the reply is drawn");
    }

    /// M4 review M1 (`file-viewer-download.test.tsx`: "retires a background thumbnail before opening the explicit
    /// viewer"): three image cards read their thumbnails one at a time, and a preview opened meanwhile isn't refused
    /// by the daemon's two-transfer limit.
    #[gpui::test]
    fn three_thumbnails_and_a_preview_share_the_two_transfers(cx: &mut TestAppContext) {
        let png = decoded(PNG);
        let files: [(&str, &str, &str, &[u8]); 3] = [
            ("m1", "one.png", "image/png", &png),
            ("m2", "two.png", "image/png", &png),
            ("m3", "three.png", "image/png", &png),
        ];
        let (rpc, leases) = host(&files);
        let (cx, _) = app(cx, Route::Chat("b1".into()), files_chat(&files), rpc);
        let _runtime = runtime(cx);
        draw(cx);
        click(cx, "file.m2.0.preview");
        until(cx, "the preview", || {
            node("file.m2.0.viewer.image").is_some() || node("file.m2.0.viewer.problem").is_some()
        });
        assert!(
            node("file.m2.0.viewer.problem").is_none(),
            "the preview wasn't refused"
        );
        // The thumbnails held back while it was open; they all come once it closes.
        cx.simulate_keystrokes("escape");
        until(cx, "all three thumbnails", || {
            ["m1", "m2", "m3"]
                .iter()
                .all(|id| node(&format!("file.{id}.0.thumbnail")).is_some())
        });
        assert!(
            leases.lock().unwrap().1 <= 2,
            "never more than two transfers"
        );
    }

    /// M4 review M3: a card's thumbnail outlives the card: scrolled away and back (here, its message gone and back),
    /// it isn't read again.
    #[gpui::test]
    fn a_card_scrolled_away_and_back_reads_its_thumbnail_once(cx: &mut TestAppContext) {
        let png = decoded(PNG);
        let files: [(&str, &str, &str, &[u8]); 1] = [("m1", "one.png", "image/png", &png)];
        let (rpc, _) = host(&files);
        let state = files_chat(&files);
        let (cx, calls) = app(cx, Route::Chat("b1".into()), state.clone(), rpc);
        let _runtime = runtime(cx);
        until(cx, "the thumbnail", || {
            node("file.m1.0.thumbnail").is_some()
        });
        let kept = state.conversations["b1"].messages.clone();
        cx.update(|_, cx| {
            store::update(cx, |state| {
                state.conversations.get_mut("b1").unwrap().messages =
                    vec![message("m9", "b1", "Moved on.", 9)];
                true
            })
        });
        draw(cx);
        assert!(node("file.m1.0").is_none(), "the card is gone");
        cx.update(|_, cx| {
            store::update(cx, |state| {
                state.conversations.get_mut("b1").unwrap().messages = kept;
                true
            })
        });
        until(cx, "the card back", || {
            node("file.m1.0.thumbnail").is_some()
        });
        assert_eq!(calls_to(&calls, "attachments.open").len(), 1, "read once");
    }

    /// M4 review C2: an animated GIF is never decoded, neither for its thumbnail nor in the viewer, which says it's too
    /// large to preview; a still GIF still shows.
    #[gpui::test]
    fn an_animated_gif_is_refused_and_a_still_one_shows(cx: &mut TestAppContext) {
        let (animated, still) = (animated_gif(), decoded(STILL_GIF));
        let files: [(&str, &str, &str, &[u8]); 2] = [
            ("m1", "moving.gif", "image/gif", &animated),
            ("m2", "still.gif", "image/gif", &still),
        ];
        let (rpc, _) = host(&files);
        let (cx, _) = app(cx, Route::Chat("b1".into()), files_chat(&files), rpc);
        let _runtime = runtime(cx);
        until(cx, "the still GIF's thumbnail", || {
            node("file.m2.0.thumbnail").is_some()
        });
        assert!(
            node("file.m1.0.thumbnail").is_none(),
            "no thumbnail for the animation"
        );
        click(cx, "file.m1.0.preview");
        until(cx, "the viewer's answer", || {
            node("file.m1.0.viewer.problem").is_some()
        });
        assert_eq!(
            node("file.m1.0.viewer.problem").unwrap().text,
            "This image is too large to preview. Download is still available."
        );
    }

    /// M4 review M2: an image the viewer showed leaves GPUI's image cache when the viewer closes.
    #[gpui::test]
    fn closing_the_viewer_frees_its_image(cx: &mut TestAppContext) {
        let png = decoded(PNG);
        let files: [(&str, &str, &str, &[u8]); 1] = [("m1", "one.png", "image/png", &png)];
        let (rpc, _) = host(&files);
        let (cx, _) = app(cx, Route::Chat("b1".into()), files_chat(&files), rpc);
        let _runtime = runtime(cx);
        draw(cx);
        click(cx, "file.m1.0.preview");
        let shown = Arc::new(Image::from_bytes(ImageFormat::Png, png.clone()));
        until(cx, "the image drawn", || {
            node("file.m1.0.viewer.image").is_some()
        });
        for _ in 0..300 {
            if cx.update(|_, cx| shown.is_asset_cached(cx)) {
                break;
            }
            draw(cx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            cx.update(|_, cx| shown.is_asset_cached(cx)),
            "drawn, so cached"
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(node("file.m1.0.viewer").is_none());
        assert!(
            !cx.update(|_, cx| shown.is_asset_cached(cx)),
            "out of the cache"
        );
    }

    /// `file-card.test.tsx` ("offers local file reveal only while showing this PC") and `file-viewer-download.test.tsx`
    /// ("closes ... the document before starting a new download"): another PC's file has no Show in folder; the
    /// viewer's Download closes the preview, asks where to save and saves the file whole, verified.
    #[gpui::test]
    fn the_viewers_download_closes_it_and_saves_the_file(cx: &mut TestAppContext) {
        let png = decoded(PNG);
        let files: [(&str, &str, &str, &[u8]); 1] = [("m1", "one.png", "image/png", &png)];
        let (rpc, leases) = host(&files);
        let mut state = files_chat(&files);
        state.host = Some("pc-2".into());
        let (cx, calls) = app(cx, Route::Chat("b1".into()), state, rpc);
        let _runtime = runtime(cx);
        draw(cx);
        assert!(
            node("file.m1.0.folder").is_none(),
            "another PC's file has no folder here"
        );
        click(cx, "file.m1.0.preview");
        until(cx, "the preview", || {
            node("file.m1.0.viewer.image").is_some()
        });
        let folder = desktop_host::owner_safety::test_root().unwrap();
        let target = folder.path().join("one.png");
        click(cx, "file.m1.0.viewer.download");
        draw(cx);
        assert!(cx.did_prompt_for_new_path(), "it asks where to save");
        let answer = target.clone();
        cx.simulate_new_path_selection(move |_| Some(answer));
        until(cx, "the save", || {
            node("file.m1.0.status").is_some_and(|status| status.text == "Saved on this device.")
        });
        assert!(
            node("file.m1.0.viewer").is_none(),
            "the preview closed first"
        );
        assert_eq!(std::fs::read(&target).unwrap(), png, "the file, whole");
        assert!(leases.lock().unwrap().1 <= 2);
        let closes = calls_to(&calls, "attachments.close");
        assert!(
            closes.len() >= 2,
            "the preview's transfer and the download's both closed: {closes:?}"
        );
    }
}
