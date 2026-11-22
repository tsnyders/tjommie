//! Rooms (T5.2): `room-composer.test.tsx`, `room-screen.test.tsx` and `rooms-list.test.ts` one for one where they test
//! behaviour, and the screens' flows through the store's fake. Sample data is invented.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::TestAppContext;
use protocol::mentions::find_mentions;
use protocol::model::{Bot, BotState, Message, MessageKind, MessageStatus, Room, Settings};
use protocol::wire::{Empty, Event, MessagePage, method};

use super::composer::{NOBODY, everyone_label, placeholder, with_at};
use super::list::rooms_summary;
use super::members::chain_line;
use super::post::marked;
use super::screen::progress_of;
use crate::model::mentions::mention_options;
use crate::model::overview::room_line;
use crate::model::state::{AppState, apply_event, room_bots};
use crate::model::testing::FakeRpc;
use crate::ui::shell::route::{self, Navigate as _, Route};
use crate::ui::testing::{app, bot, click, draw, ids, node, room, state, text};

fn bots() -> Vec<Bot> {
    vec![
        bot("b1", "Sales", BotState::Idle),
        bot("b2", "Marketing", BotState::Working),
        bot("b3", "Chief", BotState::Idle),
    ]
}

fn growth() -> Room {
    room("r1", "Growth", &["b1", "b2"])
}

fn everyone() -> Room {
    Room {
        everyone: true,
        created_at: None,
        ..room("team", "Everyone", &["b1", "b2", "b3"])
    }
}

fn post(id: &str, author: &str, body: &str, status: MessageStatus, minute: u32) -> Message {
    Message {
        id: id.into(),
        conversation_id: "r1".into(),
        author: author.into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status,
        created_at: format!("2026-11-02T09:{minute:02}:00.000Z"),
    }
}

fn rooms_state() -> AppState {
    AppState {
        settings: Some(Settings {
            chain_limit: 3,
            relay_url: None,
        }),
        ..state(bots(), vec![everyone(), growth()], vec![])
    }
}

/// A fake whose rooms read `posts`, and which answers a post with the post.
fn rpc(posts: Vec<Message>) -> FakeRpc {
    FakeRpc::new()
        .on::<method::RoomHistory>(move |_| {
            Ok(MessagePage {
                messages: posts.clone(),
                has_more: false,
            })
        })
        .on::<method::RoomPost>(|params| {
            Ok(post(
                "m9",
                "you",
                params["text"].as_str().unwrap_or_default(),
                MessageStatus::Done,
                30,
            ))
        })
}

fn calls_of(calls: &[(&'static str, serde_json::Value)], method: &str) -> Vec<serde_json::Value> {
    calls
        .iter()
        .filter(|(name, _)| *name == method)
        .map(|(_, params)| params.clone())
        .collect()
}

/// The room's composer, with the keyboard.
fn type_in(cx: &mut gpui::VisualTestContext, typed: &str) {
    click(cx, "room.composer.field");
    cx.simulate_input(typed);
    draw(cx);
}

fn mention_rows() -> Vec<String> {
    ids("room.mentions.")
        .into_iter()
        .map(|id| text(&id))
        .collect()
}

/// `room-composer.test.tsx`: "the phone's @ button starts a mention at the caret, as its own word".
#[test]
fn the_at_button_starts_a_mention_at_the_caret_as_its_own_word() {
    assert_eq!(with_at("", 0), ("@".to_owned(), 1));
    assert_eq!(with_at("hi ", 3), ("hi @".to_owned(), 4));
    assert_eq!(with_at("hi there", 2), ("hi @ there".to_owned(), 4));
}

/// `room-composer.test.tsx`: "offers only the room's own bots, and @everyone means all of them".
#[test]
fn mentions_offer_only_the_rooms_agents_and_everyone_means_all_of_them() {
    let bots = bots();
    let members: Vec<Bot> = room_bots(&growth(), &bots).into_iter().cloned().collect();
    let names: Vec<String> = mention_options("", &members)
        .into_iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(names, ["Sales", "Marketing", "everyone"]);
    assert!(
        mention_options("Ch", &members).is_empty(),
        "Chief isn't in Growth"
    );
    assert_eq!(everyone_label(2), "All 2 agents in this room");
    assert_eq!(everyone_label(1), "All 1 agent in this room");
    assert_eq!(
        placeholder("Growth"),
        "Message Growth. Type @ to mention an agent."
    );
}

/// `room-composer.test.tsx`: "says nobody will reply to a bot outside the room, even one whose name starts with a
/// member's".
#[test]
fn nobody_replies_to_an_agent_outside_the_room() {
    let bots = [("b", "Bot"), ("b2", "Bot 2")];
    let solo = [("b", "Bot")];
    assert!(find_mentions("@Bot 2 please help", &bots, &solo).is_empty());
    assert_eq!(find_mentions("@Bot please help", &bots, &solo), ["b"]);
}

/// `room-screen.test.tsx`: "your post in a room marks the mentions the room's bots answer, read against every bot's
/// name".
#[test]
fn your_post_marks_the_mentions_the_rooms_agents_answer() {
    let bots = vec![
        bot("b", "Bot", BotState::Idle),
        bot("b2", "Bot 2", BotState::Idle),
    ];
    let members = vec![&bots[0]];
    let spans = |text: &str| -> Vec<(usize, usize)> {
        marked(text, &bots, &members)
            .into_iter()
            .map(|range| (range.start, range.end))
            .collect()
    };
    assert!(spans("@Bot 2 please help").is_empty());
    assert_eq!(spans("@Bot please help"), [(0, 4)]);
    assert_eq!(spans("@everyone hi"), [(0, 9)]);
    // A character whose lowercase is longer (İ: two bytes, three lowercased) doesn't move the mark off the mention.
    assert_eq!(spans("İ @Bot hi"), [(3, 7)]);
}

/// `room-screen.test.tsx`: "a refused Delete room says why until the room moves on: a message added, or one
/// finishing, not a reply's words streaming in".
#[test]
fn a_refusal_lasts_until_the_room_moves_on() {
    let yours = post("m1", "you", "@Sales hi", MessageStatus::Done, 0);
    let reply = |status, body: &str| post("m2", "b1", body, status, 1);
    let refused = progress_of(&[yours.clone(), reply(MessageStatus::Streaming, "")]);
    assert_eq!(
        progress_of(&[
            yours.clone(),
            reply(MessageStatus::Streaming, "Half an ans")
        ]),
        refused
    );
    assert_ne!(
        progress_of(&[yours.clone(), reply(MessageStatus::Done, "An answer.")]),
        refused
    );
    let added = post("m3", "you", "@Sales hi", MessageStatus::Done, 2);
    assert_ne!(
        progress_of(&[
            yours.clone(),
            reply(MessageStatus::Streaming, ""),
            added.clone()
        ]),
        refused
    );
    // "stops saying why when an earlier reply finishes behind a newer post".
    let later = progress_of(&[
        yours.clone(),
        reply(MessageStatus::Streaming, ""),
        added.clone(),
    ]);
    assert_eq!(
        progress_of(&[
            yours.clone(),
            reply(MessageStatus::Streaming, "Half"),
            added.clone()
        ]),
        later
    );
    assert_ne!(
        progress_of(&[yours, reply(MessageStatus::Done, "An answer."), added]),
        later
    );
}

#[test]
fn the_chain_line_and_the_summary_count_in_words() {
    assert_eq!(
        chain_line(3),
        "Agents can ask each other up to 3 times per chain."
    );
    assert_eq!(
        chain_line(1),
        "Agents can ask each other up to 1 time per chain."
    );
    assert_eq!(rooms_summary(1), "1 room");
    assert_eq!(rooms_summary(2), "2 rooms");
}

/// `rooms-list.test.ts`: the previews stay current from events, and the rooms are read again when they change, not
/// whenever an agent does.
#[gpui::test]
fn the_rooms_list_reads_each_room_once_and_follows_new_posts(cx: &mut TestAppContext) {
    let posts = vec![post("m1", "b1", "Numbers are in.", MessageStatus::Done, 0)];
    let (cx, calls) = app(cx, Route::Rooms, rooms_state(), rpc(posts));
    assert_eq!(text("screen.rooms"), "Rooms");
    assert_eq!(text("rooms.summary"), "2 rooms");
    assert!(node("rooms.room.team").is_some() && node("rooms.room.r1").is_some());
    assert_eq!(calls_of(&calls.calls(), "room.history").len(), 2);
    let line = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            let state = crate::ui::store::state(cx);
            let messages = state
                .conversations
                .get("r1")
                .map(|chat| chat.messages.clone());
            room_line(messages.as_deref(), &state.bots, &jiff::Zoned::now())
        })
    };
    assert_eq!(text("rooms.room.r1.line"), line(cx));
    // A new post arrives as an event: the card follows it, and nothing is read again.
    cx.update(|_, cx| {
        crate::ui::store::update(cx, |state| {
            apply_event(
                state,
                &Event::MessageAdded(post(
                    "m2",
                    "b2",
                    "Campaign drafted.",
                    MessageStatus::Done,
                    5,
                )),
            )
        })
    });
    draw(cx);
    assert_eq!(text("rooms.room.r1.line"), line(cx));
    // An agent changing is not a reason to read the rooms again; a new room is read once.
    cx.update(|_, cx| {
        crate::ui::store::update(cx, |state| {
            apply_event(
                state,
                &Event::BotUpdated(bot("b3", "Chief", BotState::Working)),
            )
        })
    });
    draw(cx);
    assert_eq!(calls_of(&calls.calls(), "room.history").len(), 2);
    cx.update(|_, cx| {
        crate::ui::store::update(cx, |state| {
            apply_event(state, &Event::RoomUpdated(room("r2", "Ops", &["b3"])))
        })
    });
    draw(cx);
    assert_eq!(calls_of(&calls.calls(), "room.history").len(), 3);
    assert_eq!(text("rooms.summary"), "3 rooms");
}

#[gpui::test]
fn new_room_opens_the_room_form(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Rooms, rooms_state(), rpc(vec![]));
    click(cx, "rooms.new");
    assert!(node("room_form.name").is_some());
}

/// `room-screen.test.tsx`: "a room's members panel lists the room's own bots, and offers Edit members in a room other
/// than Everyone"; and "Everyone's header has no Edit room or Delete room".
#[gpui::test]
fn the_members_panel_lists_the_rooms_agents_and_everyone_has_no_more(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    assert_eq!(text("screen.room"), "Growth");
    assert_eq!(text("room.members"), "Growth members");
    assert_eq!(
        ids("room.members.b"),
        ["room.members.b1", "room.members.b2"]
    );
    assert_eq!(text("room.members.b1"), "Sales");
    assert!(node("room.members.edit").is_some());
    assert!(node("room.more").is_some());
    assert_eq!(
        text("room.members.chain"),
        "Agents can ask each other up to 3 times per chain."
    );
    cx.update(|_, cx| cx.navigate(Route::Room("team".into())));
    draw(cx);
    assert_eq!(text("screen.room"), "Everyone");
    assert_eq!(ids("room.members.b").len(), 3, "Everyone holds every agent");
    assert!(
        node("room.members.edit").is_none(),
        "Everyone always holds every agent"
    );
    assert!(
        node("room.more").is_none(),
        "Everyone can't be edited or deleted"
    );
}

#[gpui::test]
fn the_members_panel_hides_and_comes_back(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    click(cx, "room.members.hide");
    assert!(node("room.members").is_none());
    click(cx, "room.panel");
    assert!(node("room.members").is_some());
    click(cx, "room.panel");
    assert!(node("room.members").is_none());
}

#[gpui::test]
fn a_room_shows_its_posts(cx: &mut TestAppContext) {
    let posts = vec![
        post("m1", "you", "@Sales numbers?", MessageStatus::Done, 0),
        post("m2", "b1", "Up 4 percent.", MessageStatus::Done, 1),
    ];
    let _ = app(cx, Route::Room("r1".into()), rooms_state(), rpc(posts));
    assert_eq!(text("room.post.m1"), "You: @Sales numbers?");
    assert_eq!(text("room.post.m2"), "Sales: Up 4 percent.");
    assert!(node("room.empty").is_none());
}

#[gpui::test]
fn an_empty_room_says_how_to_start(cx: &mut TestAppContext) {
    let (_, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    assert_eq!(
        text("room.empty"),
        "No posts yet. @mention an agent to ask it something."
    );
}

#[gpui::test]
fn a_room_that_failed_to_load_offers_retry(cx: &mut TestAppContext) {
    let tries = Arc::new(AtomicUsize::new(0));
    let counted = tries.clone();
    let rpc = FakeRpc::new().on::<method::RoomHistory>(move |_| {
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "The disk is busy.".into(),
            })
        } else {
            Ok(MessagePage {
                messages: vec![post("m1", "b1", "Back again.", MessageStatus::Done, 0)],
                has_more: false,
            })
        }
    });
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc);
    assert_eq!(text("room.failed.text"), "Couldn't load this room.");
    click(cx, "room.failed.retry");
    assert_eq!(text("room.post.m1"), "Sales: Back again.");
    assert_eq!(tries.load(Ordering::SeqCst), 2);
}

/// `room-composer.test.tsx`: "names the room it posts in"; "points at the mention list only while it's open"; and the
/// list offers the room's own agents, then @everyone.
#[gpui::test]
fn typing_at_opens_the_list_of_the_rooms_agents(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    assert_eq!(text("room.composer.field"), "Message Growth");
    assert!(node("room.mentions").is_none());
    type_in(cx, "@");
    assert_eq!(mention_rows(), ["Sales", "Marketing", "@everyone"]);
    assert!(node("room.mentions.0").unwrap().selected);
    cx.simulate_input("Ma");
    draw(cx);
    assert_eq!(mention_rows(), ["Marketing"]);
}

/// The list's keys (`Mentions > Input`): the arrows move, Enter picks rather than sends; then Enter sends.
#[gpui::test]
fn the_arrows_move_enter_picks_and_then_sends(cx: &mut TestAppContext) {
    let (cx, calls) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    type_in(cx, "@");
    cx.simulate_keystrokes("down");
    draw(cx);
    assert!(node("room.mentions.1").unwrap().selected);
    cx.simulate_keystrokes("up up");
    draw(cx);
    assert!(
        node("room.mentions.2").unwrap().selected,
        "up from the top wraps"
    );
    cx.simulate_keystrokes("down down enter");
    draw(cx);
    assert!(node("room.mentions").is_none(), "picked: the list closes");
    assert!(
        calls_of(&calls.calls(), "room.post").is_empty(),
        "Enter picked, it didn't send"
    );
    cx.simulate_input("hi");
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(
        calls_of(&calls.calls(), "room.post"),
        [serde_json::json!({ "roomId": "r1", "text": "@Marketing hi" })]
    );
    assert!(
        node("room.composer.hint").is_none(),
        "Marketing is in the room"
    );
    // The box was cleared: Enter again sends nothing.
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(calls_of(&calls.calls(), "room.post").len(), 1);
}

/// `room-composer.test.tsx`: "Escape and the mention list".
#[gpui::test]
fn escape_closes_the_list_for_that_mention(cx: &mut TestAppContext) {
    let (cx, calls) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    type_in(cx, "@S");
    assert_eq!(mention_rows(), ["Sales"]);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(node("room.mentions").is_none());
    cx.simulate_input("a");
    draw(cx);
    assert!(
        node("room.mentions").is_none(),
        "the same mention stays closed"
    );
    cx.simulate_input(" @");
    draw(cx);
    assert_eq!(
        mention_rows(),
        ["Sales", "Marketing", "@everyone"],
        "a new @ opens it"
    );
    // Escape closed the list, not the composer's text: Tab picks now.
    cx.simulate_keystrokes("tab enter");
    draw(cx);
    assert_eq!(
        calls_of(&calls.calls(), "room.post"),
        [serde_json::json!({ "roomId": "r1", "text": "@Sa @Sales " })]
    );
}

/// WF11: a post that mentions none of the room's agents gets no answer, and the composer says so.
#[gpui::test]
fn a_post_that_mentions_nobody_says_nobody_will_reply(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    type_in(cx, "@Chief are you there");
    assert!(node("room.mentions").is_none());
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(text("room.composer.hint"), NOBODY);
    cx.simulate_input("x");
    draw(cx);
    assert!(
        node("room.composer.hint").is_none(),
        "gone once something is typed"
    );
}

#[gpui::test]
fn a_post_that_fails_says_why(cx: &mut TestAppContext) {
    let rpc = FakeRpc::new()
        .on::<method::RoomHistory>(|_| {
            Ok(MessagePage {
                messages: vec![],
                has_more: false,
            })
        })
        .on::<method::RoomPost>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "Growth is busy.".into(),
            })
        });
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc);
    type_in(cx, "@Sales hi");
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(text("room.composer.problem").contains("Growth is busy."));
}

#[gpui::test]
fn delete_room_asks_then_goes_to_the_rooms_list(cx: &mut TestAppContext) {
    let rpc = rpc(vec![]).on::<method::RoomsDelete>(|_| Ok(Empty {}));
    let (cx, calls) = app(cx, Route::Room("r1".into()), rooms_state(), rpc);
    click(cx, "room.more");
    click(cx, "room.menu.delete");
    assert_eq!(text("dialog.title"), "Delete Growth?");
    click(cx, "dialog.confirm");
    assert_eq!(
        calls_of(&calls.calls(), "rooms.delete"),
        [serde_json::json!({ "id": "r1" })]
    );
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::Rooms);
}

/// `room-screen.test.tsx`: a refused Delete room says why under the header, until the room moves on.
#[gpui::test]
fn a_refused_delete_says_why_until_the_room_moves_on(cx: &mut TestAppContext) {
    let rpc = rpc(vec![post("m1", "you", "@Sales hi", MessageStatus::Done, 0)])
        .on::<method::RoomsDelete>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "Sales is still answering here.".into(),
            })
        });
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc);
    click(cx, "room.more");
    click(cx, "room.menu.delete");
    click(cx, "dialog.confirm");
    assert!(text("room.problem").contains("Sales is still answering here."));
    assert_eq!(
        cx.update(|_, cx| route::current(cx)),
        Route::Room("r1".into())
    );
    cx.update(|_, cx| {
        crate::ui::store::update(cx, |state| {
            apply_event(
                state,
                &Event::MessageAdded(post("m2", "b1", "Done answering.", MessageStatus::Done, 1)),
            )
        })
    });
    draw(cx);
    assert!(node("room.problem").is_none(), "the room moved on");
}

#[gpui::test]
fn edit_room_opens_the_form_on_this_room(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    click(cx, "room.more");
    click(cx, "room.menu.edit");
    assert!(node("room_form.name").is_some());
    assert!(node("room_form.members.b1").unwrap().toggled);
}

#[gpui::test]
fn a_gone_room_shows_the_rooms_list(cx: &mut TestAppContext) {
    let (cx, _) = app(cx, Route::Room("gone".into()), rooms_state(), rpc(vec![]));
    draw(cx);
    assert_eq!(cx.update(|_, cx| route::current(cx)), Route::Rooms);
    assert!(node("screen.rooms").is_some());
}

/// A narrow window (360 × 600): the members panel is a sheet over the room, not a column beside it (`members-panel.tsx`
/// under `sm`); Hide still closes it.
#[gpui::test]
fn a_narrow_window_shows_the_members_as_a_sheet(cx: &mut TestAppContext) {
    use gpui::{px, size};
    let (cx, _) = app(cx, Route::Room("r1".into()), rooms_state(), rpc(vec![]));
    let wide = node("room.members").expect("the members panel");
    assert!(wide.bounds.width < 400., "wide: a column beside the posts");
    cx.simulate_resize(size(px(360.), px(600.)));
    draw(cx);
    let sheet = node("room.members").expect("the members panel");
    let room = node("room").expect("the room");
    assert!(
        (sheet.bounds.x - room.bounds.x).abs() < 1.
            && (sheet.bounds.width - room.bounds.width).abs() < 1.,
        "narrow: a sheet over the room: {:?} over {:?}",
        sheet.bounds,
        room.bounds
    );
}
