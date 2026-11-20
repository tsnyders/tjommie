//! The rooms list (`rooms-list.tsx`; `/room/`, where the sidebar's Rooms goes): Everyone, then the other rooms, oldest
//! first (the daemon's order), each a card as on Agents with its agents beside its name, and New room. A card's posts are
//! its room's history in the store, read once a room when connected; new posts come as events, so a card stays current
//! without being read again (the TypeScript's `withMessage` and `withPage` are the store's here).

use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Role, StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::model::state::{Connection, ConversationStatus, room_bots};
use crate::semantic::SemanticExt as _;
use crate::ui::agents::{cards, columns};
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::scroll::{follow_focus, scroll_handle};
use crate::ui::store::{self, History};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// "1 room", "3 rooms".
pub fn rooms_summary(count: usize) -> String {
    format!("{count} {}", if count == 1 { "room" } else { "rooms" })
}

/// The rooms list, kept while the route shows it.
pub fn screen(window: &mut Window, cx: &mut App) -> AnyElement {
    window
        .use_keyed_state("screen.rooms.view", cx, |_, _| RoomsList::default())
        .into_any_element()
}

#[derive(Default)]
pub struct RoomsList {
    /// The rooms whose posts have been asked for.
    asked: HashSet<String>,
}

impl Render for RoomsList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = store::state(cx).clone();
        if state.connection == Connection::Ready {
            for room in &state.rooms {
                if self.asked.insert(room.id.clone()) {
                    store::load_conversation(cx, &room.id, History::Room(room.id.clone())).detach();
                }
            }
        }
        let now = jiff::Zoned::now();
        let cards: Vec<AnyElement> = state
            .rooms
            .iter()
            .map(|room| {
                let messages = state
                    .conversations
                    .get(&room.id)
                    .filter(|chat| chat.status == ConversationStatus::Ready)
                    .map(|chat| chat.messages.as_slice());
                let members = room_bots(room, &state.bots);
                cards::room_card(
                    "rooms.room",
                    room,
                    messages,
                    &state.bots,
                    Some(&members),
                    &now,
                    cx,
                )
            })
            .collect();
        let columns = columns(window, cx);
        let scroll = scroll_handle("rooms.scroll", window, cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let summary = rooms_summary(state.rooms.len());
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(r(theme.space.s24))
            .pt(r(theme.space.s32))
            .px(r(theme.space.s32))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(r(theme.space.s16))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .semantic("screen.rooms", Role::Heading)
                                    .aria_label("Rooms")
                                    .type_style(&theme.typography.desktop.display_l)
                                    .text_color(hsla(colors.text.primary))
                                    .child("Rooms"),
                            )
                            .child(
                                div()
                                    .semantic("rooms.summary", Role::Paragraph)
                                    .aria_label(summary.clone())
                                    .mt(r(theme.space.s10))
                                    .type_style(&theme.typography.desktop.ui)
                                    .text_color(hsla(colors.text.secondary))
                                    .child(summary),
                            ),
                    )
                    .child(
                        Pill::new("rooms.new", "New room")
                            .variant(PillVariant::Primary)
                            .small()
                            .icon("plus")
                            .on_press(|window, cx| super::open_room_form(None, window, cx)),
                    ),
            )
            .child(follow_focus(
                &scroll,
                div()
                    .id("rooms.scroll")
                    .track_scroll(&scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(r(theme.space.s28))
                    .child(
                        div()
                            .grid()
                            .grid_cols(columns)
                            .gap(crate::ui::style::card_gap(cx))
                            .children(cards),
                    ),
            ))
    }
}
