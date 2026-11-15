//! A room (`room-screen.tsx`; WF5): Everyone, which every agent is in, or a room of chosen agents. The header has the
//! room's chip (its mark, its name and how many agents), the members panel's button and, for a room other than
//! Everyone, More with Edit room and Delete room; then the posts, the composer, and the members panel beside them.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, svg,
};
use protocol::model::{Message, MessageStatus, Room};
use protocol::wire::{Id, method};

use super::composer::RoomComposer;
use super::{open_room_form, post};
use crate::guard::{guarded, guarded_async};
use crate::model::labels::error_text;
use crate::model::state::{Connection, ConversationStatus, room_bots};
use crate::model::timeline::{TimelineItem, timeline};
use crate::semantic::SemanticExt as _;
use crate::ui::agents::cards::more_trigger;
use crate::ui::fields::{Link, problem};
use crate::ui::primitives::button::{IconButton, IconVariant};
use crate::ui::primitives::dialog::open_confirm;
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::shell::connection::failed_load;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store::{self, History};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// Where a room's posts are at: a post added, or any of them finishing, moves them on; a reply's words streaming in
/// don't. A refused Delete room is about the room as it was then, so its notice goes once the room moves on.
pub fn progress_of(messages: &[Message]) -> Vec<MessageStatus> {
    messages.iter().map(|message| message.status).collect()
}

/// The members panel is hidden: a choice for every room, kept while the app runs.
#[derive(Default)]
struct MembersHidden(bool);

impl Global for MembersHidden {}

/// The room `room_id`, kept while the route shows it. A room that isn't in the lists is gone (deleted, here or
/// elsewhere, or an old link's): the rooms list shows instead.
pub fn screen(room_id: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(room) = store::state(cx)
        .rooms
        .iter()
        .find(|room| room.id == room_id)
        .cloned()
    else {
        window.defer(cx, |_, cx| {
            guarded("leave a gone room", || cx.navigate(Route::Rooms));
        });
        return div().into_any_element();
    };
    window
        .use_keyed_state(
            SharedString::from(format!("screen.room.{}.view", room.id)),
            cx,
            move |window, cx| RoomScreen::new(&room, window, cx),
        )
        .into_any_element()
}

pub struct RoomScreen {
    room_id: String,
    composer: Entity<RoomComposer>,
    posts: ScrollHandle,
    /// Why Delete room failed.
    problem: Option<SharedString>,
    seen: Vec<MessageStatus>,
}

impl RoomScreen {
    fn new(room: &Room, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (id, name) = (room.id.clone(), room.name.clone());
        RoomScreen {
            room_id: room.id.clone(),
            composer: cx.new(|cx| RoomComposer::new(id, name, window, cx)),
            posts: ScrollHandle::new(),
            problem: None,
            seen: Vec::new(),
        }
    }
}

/// Shows or hides the members panel, for every room.
fn hide_members(hidden: bool, cx: &mut App) {
    cx.set_global(MembersHidden(hidden));
    cx.refresh_windows();
}

/// Delete room, once confirmed: the room's screen goes to the rooms list; a refusal (an agent still answering) shows
/// under the header.
fn remove(screen: &Entity<RoomScreen>, cx: &mut App) {
    let id = screen.read(cx).room_id.clone();
    screen.update(cx, |screen, cx| {
        screen.problem = None;
        cx.notify();
    });
    let call = store::request::<method::RoomsDelete>(cx, Id { id });
    let screen = screen.clone();
    cx.spawn(async move |cx| {
        guarded_async("delete a room", async move {
            let result = call.await;
            cx.update(|cx| match result {
                Ok(_) => cx.navigate(Route::Rooms),
                Err(error) => screen.update(cx, |screen, cx| {
                    screen.problem = Some(error_text(&error).into());
                    cx.notify();
                }),
            });
        })
        .await;
    })
    .detach();
}

impl Render for RoomScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = store::state(cx).clone();
        let Some(room) = state.rooms.iter().find(|room| room.id == self.room_id) else {
            return div().into_any_element();
        };
        let members = room_bots(room, &state.bots);
        let chat = state.conversations.get(&self.room_id);
        if state.connection == Connection::Ready && chat.is_none() {
            store::load_conversation(cx, &self.room_id, History::Room(self.room_id.clone()))
                .detach();
        }
        let progress = progress_of(chat.map_or(&[][..], |chat| chat.messages.as_slice()));
        if progress != self.seen {
            self.seen = progress;
            self.problem = None;
        }
        let shown = !cx.try_global::<MembersHidden>().is_some_and(|hidden| hidden.0);
        let screen = cx.entity();
        let header = self.header(room, members.len(), shown, &screen, window, cx);
        let body = self.body(room, chat, &state.bots, &members, cx);
        let panel = shown.then(|| {
            let editing = room.clone();
            super::members::panel(
                room,
                &members,
                state.settings.as_ref(),
                move |window, cx| open_room_form(Some(editing.clone()), window, cx),
                |_, cx| hide_members(true, cx),
                window,
                cx,
            )
        });
        let failed = self
            .problem
            .clone()
            .map(|text| div().flex().justify_center().child(problem("room.problem", text, cx)));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .semantic("room", Role::Group)
                    .aria_label(room.name.clone())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .children(failed)
                    .child(body)
                    .child(self.composer.clone()),
            )
            .children(panel)
            .into_any_element()
    }
}

impl RoomScreen {
    /// The header: the chip in the middle; the members panel's button and More at the right.
    fn header(
        &self,
        room: &Room,
        count: usize,
        shown: bool,
        screen: &Entity<RoomScreen>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let more = (!room.everyone).then(|| {
            let focus = focus_handle("room.more", window, cx);
            let (editing, deleting) = (room.clone(), screen.clone());
            let name = room.name.clone();
            Menu::new("room.menu", move |open, window, cx| {
                more_trigger("room.more".to_owned(), "More".into(), open, &focus, window, cx)
            })
            .anchor(gpui::Anchor::TopRight)
            .offset(px(6.))
            .item(
                MenuItem::new("room.menu.edit", "Edit room", move |window, cx| {
                    open_room_form(Some(editing.clone()), window, cx)
                })
                .icon("sliders-horizontal"),
            )
            .item(
                MenuItem::new("room.menu.delete", "Delete room", move |window, cx| {
                    let screen = deleting.clone();
                    open_confirm(
                        window,
                        cx,
                        format!("Delete {name}?"),
                        "Its posts are deleted. Its agents and their chats aren't touched.",
                        "Delete room",
                        true,
                        move |_, cx| remove(&screen, cx),
                    );
                })
                .icon("trash-2"),
            )
        });
        let theme = theme(cx);
        let colors = palette(cx);
        let mark = div()
            .size(r(24.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(hsla(colors.surface.raised))
            .child(
                svg()
                    .path("icons/i/175/messages-square.svg")
                    .size(r(14.))
                    .text_color(hsla(colors.icon.base)),
            )
            .into_any_element();
        let heading = div()
            .semantic("screen.room", Role::Heading)
            .aria_label(room.name.clone())
            .min_w_0()
            .truncate()
            .child(room.name.clone())
            .into_any_element();
        let agents = format!("{count} {}", if count == 1 { "agent" } else { "agents" });
        let chip = crate::ui::bots::chip(mark, heading, Some(agents), cx);
        div()
            .h(r(56.))
            .flex_none()
            .flex()
            .items_center()
            .gap(r(theme.space.s8))
            .px(r(theme.space.s16))
            .child(div().w(r(80.)).flex_none())
            .child(div().flex_1().min_w_0().flex().justify_center().child(chip))
            .child(
                div()
                    .w(r(80.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .gap(r(theme.space.s4))
                    .child(
                        IconButton::new("room.panel", "panel-right", "Members panel")
                            .variant(if shown {
                                IconVariant::Grey
                            } else {
                                IconVariant::Plain
                            })
                            .on_press(move |_, cx| hide_members(shown, cx)),
                    )
                    .children(more),
            )
            .into_any_element()
    }

    /// The posts: a failed load with Retry, nothing while they're read, a line when there are none, or the posts
    /// with Load older above them.
    fn body(
        &self,
        room: &Room,
        chat: Option<&crate::model::state::Conversation>,
        bots: &[protocol::model::Bot],
        members: &[&protocol::model::Bot],
        cx: &mut App,
    ) -> AnyElement {
        let id = room.id.clone();
        match chat {
            Some(chat) if chat.status == ConversationStatus::Failed => failed_load(
                "room.failed",
                "Couldn't load this room.",
                move |_, cx| {
                    store::retry_conversation(cx, &id, History::Room(id.clone())).detach()
                },
                cx,
            ),
            Some(chat) if chat.status == ConversationStatus::Ready && chat.messages.is_empty() => {
                let theme = theme(cx);
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(r(theme.space.s32))
                    .child(
                        div()
                            .semantic("room.empty", Role::Paragraph)
                            .aria_label("No posts yet. @mention an agent to ask it something.")
                            .text_center()
                            .type_style(&theme.typography.desktop.ui)
                            .text_color(hsla(palette(cx).text.secondary))
                            .child("No posts yet. @mention an agent to ask it something."),
                    )
                    .into_any_element()
            }
            Some(chat) if chat.status == ConversationStatus::Ready => {
                // ponytail: keeps to the newest post while the reader is at the bottom; the Chat lane's message
                // list (T4.1) adds a long room's virtual list and holds the place when older posts load.
                let (offset, max): (Pixels, Pixels) = (self.posts.offset().y, self.posts.max_offset().y);
                if -offset >= max - px(4.) {
                    self.posts.scroll_to_bottom();
                }
                let now = jiff::Zoned::now();
                let theme = theme(cx);
                let older = chat.has_more.then(|| {
                    let id = room.id.clone();
                    div().flex().justify_center().child(
                        Link::new("room.older", "Load older posts").on_press(move |_, cx| {
                            store::load_older(cx, &id, History::Room(id.clone())).detach()
                        }),
                    )
                });
                let rows: Vec<AnyElement> = timeline(&chat.messages, &now)
                    .into_iter()
                    .map(|item| match item {
                        TimelineItem::Stamp { label, .. } => div()
                            .flex()
                            .justify_center()
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(palette(cx).text.tertiary))
                            .child(label)
                            .into_any_element(),
                        TimelineItem::Message { message, .. } => {
                            post::post(message, bots, members, &now, cx)
                        }
                    })
                    .collect();
                div()
                    .id("room.posts")
                    .track_scroll(&self.posts)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .mx_auto()
                            .w_full()
                            .max_w(r(760.))
                            .flex()
                            .flex_col()
                            .gap(r(theme.space.s16))
                            .px(r(theme.space.s24))
                            .py(r(theme.space.s16))
                            .children(older)
                            .children(rows),
                    )
                    .into_any_element()
            }
            _ => div().flex_1().into_any_element(),
        }
    }
}
