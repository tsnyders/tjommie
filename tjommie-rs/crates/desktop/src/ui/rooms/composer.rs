//! A room's composer (`room-composer.tsx`; WF5, WF11), drawn as Aegis's composer card (chat-composer.html). Typing "@"
//! opens the mention list above it, filtered by what follows; it offers the room's own agents, as only they answer
//! there, then @everyone. A post that mentions none of them gets no answer, and the composer says so. Room posts carry
//! no files. The card's shared parts (the lift, the send pop, dictation) come with the Chat lane's composer (T4.3).
//!
//! The keys are the keymap's: in the field, Enter sends (`Composer > Input`), Shift+Enter adds a line; while the list
//! is open, the arrows move through it, Enter and Tab pick, and Escape closes it (`Mentions > Input`, and `Mentions`
//! from the composer's buttons).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    KeyContext, MouseButton, ParentElement as _, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, relative, svg,
};
use gpui_kit::component::input::{InputEvent, Position, Textarea, TextareaState};
use protocol::mentions::find_mentions;
use protocol::model::Bot;
use protocol::wire::{RoomPost, method};

use crate::guard::{guarded, guarded_async};
use crate::keymap::{COMPOSER, MENTIONS, MentionClose, MentionDown, MentionPick, MentionUp, Send};
use crate::model::connection::RequestError;
use crate::model::labels::{error_text, short_state};
use crate::model::mentions::{insert_mention, mention_options, typed_mention};
use crate::model::state::room_bots;
use crate::semantic::{SemanticExt as _, SemanticFocus as _};
use crate::ui::fields::problem;
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{IconButton, IconVariant};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// Said after a post that mentions none of the room's agents, until something is typed.
pub const NOBODY: &str = "No agent was mentioned, so nobody will reply. Type @ to ask an agent.";

/// What @everyone stands for in the mention list.
pub fn everyone_label(count: usize) -> String {
    let agents = if count == 1 { "agent" } else { "agents" };
    format!("All {count} {agents} in this room")
}

/// The text after the @ button at the caret: a space first unless the caret already follows one, so the mention
/// starts a word. The text, and where the caret goes (byte offsets).
pub fn with_at(text: &str, caret: usize) -> (String, usize) {
    let (before, after) = match (text.get(..caret), text.get(caret..)) {
        (Some(before), Some(after)) => (before, after),
        _ => (text, ""),
    };
    let at = if before.is_empty() || before.ends_with(char::is_whitespace) {
        "@"
    } else {
        " @"
    };
    (format!("{before}{at}{after}"), before.len() + at.len())
}

/// The field's position for a byte offset: its line, and its column in characters.
fn position(text: &str, caret: usize) -> Position {
    let before = text.get(..caret).unwrap_or(text);
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let column = before.get(line_start..).unwrap_or("").chars().count();
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Position::new(count(before.matches('\n').count()), count(column))
}

pub(crate) fn placeholder(room: &str) -> String {
    format!("Message {room}. Type @ to mention an agent.")
}

/// Agents as `find_mentions` takes them: (id, name).
fn pairs(bots: &[Bot]) -> Vec<(&str, &str)> {
    bots.iter()
        .map(|bot| (bot.id.as_str(), bot.name.as_str()))
        .collect()
}

pub struct RoomComposer {
    room_id: String,
    /// The room's name in the placeholder: a rename changes it.
    named: String,
    field: Entity<TextareaState>,
    list: ScrollHandle,
    /// Every edit moves it on: a post that lands clears the box only if nothing was typed since it was sent.
    revision: u64,
    highlight: usize,
    /// Where the mention starts that Escape closed: its list stays closed until a new "@".
    dismissed: Option<usize>,
    busy: bool,
    hint: bool,
    problem: Option<SharedString>,
    _field: Subscription,
}

impl RoomComposer {
    pub fn new(room_id: String, name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .placeholder(placeholder(&name))
        });
        let subscription = cx.subscribe_in(&field, window, |this, _, event, _, cx| {
            guarded("type in a room", || {
                if matches!(event, InputEvent::Change) {
                    this.edited(cx);
                }
            });
        });
        RoomComposer {
            room_id,
            named: name,
            field,
            list: ScrollHandle::new(),
            revision: 0,
            highlight: 0,
            dismissed: None,
            busy: false,
            hint: false,
            problem: None,
            _field: subscription,
        }
    }

    /// The field's text and caret.
    fn typed(&self, cx: &App) -> (String, usize) {
        let field = self.field.read(cx);
        (field.value().to_string(), field.cursor())
    }

    /// The room's agents.
    fn members(&self, cx: &App) -> (Vec<Bot>, Vec<Bot>) {
        let state = store::state(cx);
        let members = state
            .rooms
            .iter()
            .find(|room| room.id == self.room_id)
            .map(|room| room_bots(room, &state.bots).into_iter().cloned().collect())
            .unwrap_or_default();
        (state.bots.clone(), members)
    }

    /// The open list's options, each a name and its agent (`None` for @everyone); empty while the list is closed.
    fn options(&self, cx: &App) -> Vec<(String, Option<Bot>)> {
        let (_, members) = self.members(cx);
        let (text, caret) = self.typed(cx);
        match typed_mention(&text, caret) {
            Some(typed) if Some(typed.start) != self.dismissed => {
                mention_options(&typed.query, &members)
                    .into_iter()
                    .map(|option| (option.name, option.bot.cloned()))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// After any change to the text (`edit`).
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.highlight = 0;
        let (text, caret) = self.typed(cx);
        // Escape closes the list for the mention being typed; a new "@" opens it again, even in the same place.
        if typed_mention(&text, caret).map(|typed| typed.start) != self.dismissed {
            self.dismissed = None;
        }
        cx.notify();
    }

    /// Puts `text` in the field; with a caret, puts it there and gives the field the keyboard.
    fn replace(
        &mut self,
        text: String,
        caret: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = caret.map(|caret| position(&text, caret));
        self.field.update(cx, |field, cx| {
            field.set_value(text, window, cx);
            if let Some(at) = at {
                field.set_cursor_position(at, window, cx);
            }
        });
        // `set_value` sends no Change.
        self.edited(cx);
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.options(cx).len();
        if count == 0 {
            return;
        }
        let active = self.highlight.min(count - 1);
        self.highlight = (active + if down { 1 } else { count - 1 }) % count;
        // The highlighted agent stays in view in a long list.
        self.list.scroll_to_item(self.highlight);
        cx.notify();
    }

    /// Puts the option `at` (the highlighted one when `None`) in place of the "@..." being typed.
    fn pick(&mut self, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let options = self.options(cx);
        let index = at
            .unwrap_or(self.highlight)
            .min(options.len().saturating_sub(1));
        let Some((name, _)) = options.get(index) else {
            return;
        };
        let (text, caret) = self.typed(cx);
        let next = typed_mention(&text, caret)
            .and_then(|typed| insert_mention(&text, typed.start, caret, name));
        if let Some(next) = next {
            self.replace(next.text, Some(next.caret), window, cx);
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let (text, caret) = self.typed(cx);
        self.dismissed = typed_mention(&text, caret).map(|typed| typed.start);
        cx.notify();
    }

    /// The @ button: starts a mention at the caret and opens the list.
    fn at(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, caret) = self.typed(cx);
        let (next, caret) = with_at(&text, caret);
        self.replace(next, Some(caret), window, cx);
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, _) = self.typed(cx);
        if self.busy || text.trim().is_empty() {
            return;
        }
        let revision = self.revision;
        self.busy = true;
        self.problem = None;
        cx.notify();
        let call = store::request::<method::RoomPost>(
            cx,
            RoomPost {
                room_id: self.room_id.clone(),
                text: text.clone(),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            guarded_async("post in a room", async move {
                let result = call.await.map(|_| ());
                let _ = this.update_in(cx, |this, window, cx| {
                    this.sent(&text, revision, result, window, cx)
                });
            })
            .await;
        })
        .detach();
    }

    fn sent(
        &mut self,
        text: &str,
        revision: u64,
        result: Result<(), RequestError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        match result {
            Ok(()) => {
                let (bots, members) = self.members(cx);
                // "Nobody will reply" counts the room's agents only.
                self.hint = find_mentions(text, &pairs(&bots), &pairs(&members)).is_empty();
                // What was typed meanwhile, with its caret and mention, stays.
                if self.revision == revision {
                    self.replace(String::new(), None, window, cx);
                }
            }
            Err(error) => self.problem = Some(error_text(&error).into()),
        }
        cx.notify();
    }
}

impl Render for RoomComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (_, members) = self.members(cx);
        let name = store::state(cx)
            .rooms
            .iter()
            .find(|room| room.id == self.room_id)
            .map_or_else(|| self.named.clone(), |room| room.name.clone());
        if name != self.named {
            self.named = name.clone();
            self.field.update(cx, |field, cx| {
                field.set_placeholder(placeholder(&name), window, cx)
            });
        }
        let options = self.options(cx);
        let open = !options.is_empty();
        let active = self.highlight.min(options.len().saturating_sub(1));
        let (text, _) = self.typed(cx);
        let empty = text.trim().is_empty();
        let focus = self.field.focus_handle(cx);
        let focused = focus.is_focused(window);
        let entity = cx.entity();
        let mut context = KeyContext::default();
        context.add(COMPOSER);
        if open {
            context.add(MENTIONS);
        }
        let theme = theme(cx);
        let colors = palette(cx);
        let list = open.then(|| {
            let rows: Vec<gpui::AnyElement> = options
                .iter()
                .enumerate()
                .map(|(index, (option, bot))| {
                    let selected = index == active;
                    let (label, detail) = match bot {
                        Some(bot) => (option.clone(), short_state(bot.state).to_owned()),
                        None => ("@everyone".to_owned(), everyone_label(members.len())),
                    };
                    let mark = match bot {
                        Some(bot) => BotAvatar::new(bot.color, bot.shape, 18.).into_any_element(),
                        None => svg()
                            .path("icons/i/175/messages-square.svg")
                            .size(r(18.))
                            .flex_none()
                            .text_color(hsla(colors.icon.base))
                            .into_any_element(),
                    };
                    div()
                        .semantic(format!("room.mentions.{index}"), Role::ListBoxOption)
                        .aria_label(label.clone())
                        .aria_selected(selected)
                        .h(r(36.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s8))
                        .px(r(theme.space.s10))
                        .rounded(r(theme.radius.sm))
                        .when(selected, |row| row.bg(hsla(colors.state.hover)))
                        .child(mark)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .type_style(&theme.typography.desktop.ui)
                                .text_color(hsla(colors.text.primary))
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_none()
                                .type_style(&theme.typography.desktop.meta)
                                .text_color(hsla(colors.text.tertiary))
                                .child(detail),
                        )
                        // A press, not a click: the field keeps the keyboard.
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                guarded("pick a mention", || this.pick(Some(index), window, cx));
                            }),
                        )
                        .into_any_element()
                })
                .collect();
            div()
                .semantic("room.mentions", Role::ListBox)
                .aria_label("Mention an agent")
                .track_scroll(&self.list)
                .overflow_y_scroll()
                .absolute()
                .bottom(relative(1.))
                .left(r(theme.space.s24))
                .mb(r(theme.space.s8))
                .w(r(280.))
                .max_h(r(240.))
                .flex()
                .flex_col()
                .gap(r(theme.space.s4))
                .p(r(theme.space.s6))
                .rounded(r(theme.radius.md))
                .bg(hsla(colors.surface.raised))
                .border_t_1()
                .border_color(hsla(colors.overlay.highlight))
                .shadow(shadows(&theme.shadow.light.raised))
                .children(rows)
        });
        let hint = (self.hint && text.is_empty()).then(|| {
            div()
                .semantic("room.composer.hint", Role::Status)
                .aria_label(NOBODY)
                .mb(r(theme.space.s8))
                .text_center()
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.tertiary))
                .child(NOBODY)
        });
        let field = div()
            .semantic("room.composer.field", Role::ComboBox)
            .aria_label(format!("Message {name}"))
            .aria_expanded(open)
            .semantic_focus(&focus)
            .min_h(r(36.))
            .px(r(theme.space.s6))
            .pt(r(theme.space.s6))
            .pb(r(theme.space.s10))
            .type_style(&theme.typography.desktop.body)
            .text_color(hsla(colors.text.primary))
            .child(
                Textarea::new(&self.field)
                    .appearance(false)
                    .aria_label(format!("Message {name}")),
            );
        let (mention, sending) = (entity.clone(), entity);
        let controls = div()
            .flex()
            .items_center()
            .gap(r(theme.space.s8))
            .child(
                IconButton::new("room.composer.mention", "at-sign", "Mention an agent")
                    .variant(IconVariant::Grey)
                    .on_press(move |window, cx| {
                        mention.update(cx, |this, cx| this.at(window, cx))
                    }),
            )
            .child(div().flex_1())
            .child(
                IconButton::new("room.composer.send", "arrow-up", "Send")
                    .variant(IconVariant::Dark)
                    .disabled(empty || self.busy)
                    .on_press(move |window, cx| {
                        sending.update(cx, |this, cx| this.send(window, cx))
                    }),
            );
        let failed = self
            .problem
            .clone()
            .map(|text| problem("room.composer.problem", text, cx));
        div()
            .id("room.composer")
            .key_context(context)
            .on_action(cx.listener(|this, _: &Send, window, cx| {
                guarded("send a room post", || this.send(window, cx));
            }))
            .on_action(cx.listener(|this, _: &MentionUp, _, cx| {
                guarded("move up the mention list", || this.step(false, cx));
            }))
            .on_action(cx.listener(|this, _: &MentionDown, _, cx| {
                guarded("move down the mention list", || this.step(true, cx));
            }))
            .on_action(cx.listener(|this, _: &MentionPick, window, cx| {
                guarded("pick a mention", || this.pick(None, window, cx));
            }))
            .on_action(cx.listener(|this, _: &MentionClose, _, cx| {
                guarded("close the mention list", || this.close(cx));
            }))
            .relative()
            .flex_none()
            .px(r(theme.space.s24))
            .pb(r(theme.space.s20))
            .children(list)
            .children(hint)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .bg(hsla(colors.surface.raised))
                    .rounded(r(theme.radius.card))
                    .border_t_1()
                    .border_color(hsla(colors.overlay.highlight))
                    .shadow(shadows(if focused {
                        &theme.shadow.light.raised_focus
                    } else {
                        &theme.shadow.light.raised
                    }))
                    .px(r(theme.space.s12))
                    .pt(r(theme.space.s12))
                    .pb(r(theme.space.s10))
                    .child(field)
                    .child(controls),
            )
            .children(failed)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext};
    use protocol::model::BotState;

    use super::*;
    use crate::model::testing::FakeRpc;
    use crate::theme::Theme;
    use crate::ui::testing::{bot, room, state};

    /// `room-composer.test.tsx`: "a post that finishes after more was typed clears the box only if nothing was typed
    /// since it was sent".
    #[gpui::test]
    fn a_post_that_lands_after_more_was_typed_keeps_it(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Theme::aegis().unwrap());
            let rooms = vec![room("r1", "Growth", &["b1"])];
            store::fake(cx, state(vec![bot("b1", "Sales", BotState::Idle)], rooms, vec![]), FakeRpc::new());
        });
        let slot = Rc::new(RefCell::new(None));
        let keep = slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let composer = cx.new(|cx| RoomComposer::new("r1".into(), "Growth".into(), window, cx));
            *keep.borrow_mut() = Some(composer.clone());
            gpui_kit::component::Root::new(composer, window, cx)
        });
        let composer = slot.borrow().clone().unwrap();
        cx.update(|window, cx| {
            composer.update(cx, |this, cx| {
                this.replace("@Sales hi".into(), Some(9), window, cx);
                let sent = this.revision;
                this.replace("@Sales hi and more".into(), Some(18), window, cx);
                this.sent("@Sales hi", sent, Ok(()), window, cx);
                assert_eq!(this.typed(cx).0, "@Sales hi and more", "typed since: kept");
                let sent = this.revision;
                this.sent("@Sales hi and more", sent, Ok(()), window, cx);
                assert_eq!(this.typed(cx).0, "", "nothing typed since: cleared");
            })
        });
    }
}
