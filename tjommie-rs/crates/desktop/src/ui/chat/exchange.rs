//! The exchange view (Phase 3 plan T4.1; `exchange/exchange-view.tsx`, WF6): who asked whom, the whole exchange, and a box
//! that replies in Everyone with both agents mentioned. It opens beside the chat or room it was opened from, in the right
//! panel's place: opening it closes the browser panel, and the panel opening again closes it (the newer one wins, as the
//! TypeScript showed one or the other). Closing it, opening another, or leaving forgets its conversation; its x and
//! Escape give the keyboard back to the link that opened it.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, svg,
};
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::base::input::{InputEditorStyle, InputEvent, Textarea, TextareaState};
use jiff::Zoned;
use protocol::model::{Bot, Exchange, Message, TEAM_ROOM_ID};
use protocol::wire::{Id, RoomPost, method};

use super::message_view::{self, MessageContext, shown};
use crate::guard::{guarded, guarded_async};
use crate::keymap::{self, Send};
use crate::model::exchange::{exchange_caption, exchange_id, exchange_meta, handoff_status};
use crate::model::labels::{DELETED_BOT, error_text};
use crate::model::state::ConversationStatus;
use crate::motion::keyboard::Trigger;
use crate::semantic::{SemanticExt as _, SemanticFocus as _};
use crate::ui::browser::panel::BrowserPanel;
use crate::ui::fields::Link;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::primitives::button::{IconButton, IconVariant};
use crate::ui::primitives::focus_by_id;
use crate::ui::shell::connection::failed_load;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store::{self, History};
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// The exchange open beside the chat or room, if any.
#[derive(Default)]
struct Open(Option<Entity<ExchangeView>>);

impl Global for Open {}

/// Opens `exchange_id` beside the conversation `from` (the chat or room it was opened from, whose hand-off row tells
/// when it ends). Opening it again does nothing.
pub fn open(exchange_id: SharedString, from: SharedString, window: &mut Window, cx: &mut App) {
    if open_id(cx).as_ref() == Some(&exchange_id) {
        return;
    }
    close(cx);
    BrowserPanel::dismiss(cx);
    let view = cx.new(|cx| ExchangeView::new(exchange_id, from, window, cx));
    // Over the chat as a sheet, it takes the keyboard, so nobody types into the chat it hides; Escape and its x give the
    // keyboard back to View thread (`dismiss`).
    if crate::ui::shell::overlaid(window, cx) {
        let trap = view.read(cx).trap.clone();
        trap.focus(window, cx);
    }
    cx.set_global(Open(Some(view)));
    cx.refresh_windows();
}

/// Closes the exchange view, forgetting its conversation.
pub fn close(cx: &mut App) {
    let Some(view) = cx.try_global::<Open>().and_then(|open| open.0.clone()) else {
        return;
    };
    let id = view.read(cx).id.clone();
    cx.set_global(Open(None));
    store::close_conversation(cx, &id);
    cx.refresh_windows();
}

/// Closes the exchange view from its own x or by Escape (M4 review A4, A5): the keyboard goes back to the "View
/// thread" link that opened it or, when that row isn't drawn, to the panel button: the chat's, or a room's Members.
/// Whether one was open.
pub fn dismiss(window: &mut Window, cx: &mut App) -> bool {
    let Some(view) = cx.try_global::<Open>().and_then(|open| open.0.clone()) else {
        return false;
    };
    let (id, from) = {
        let view = view.read(cx);
        (view.id.clone(), view.from.clone())
    };
    close(cx);
    let _ = focus_by_id(&format!("exchange.{id}.open"), window, cx)
        || focus_by_id(&format!("chat.{from}.panel"), window, cx)
        || focus_by_id("room.panel", window, cx);
    true
}

/// The open exchange's id.
pub fn open_id(cx: &App) -> Option<SharedString> {
    cx.try_global::<Open>()
        .and_then(|open| open.0.as_ref())
        .map(|view| view.read(cx).id.clone())
}

/// The view for the shell's side slot, while an exchange is open.
pub fn side(cx: &App) -> Option<AnyView> {
    cx.try_global::<Open>()
        .and_then(|open| open.0.clone())
        .map(AnyView::from)
}

pub struct ExchangeView {
    id: SharedString,
    /// The conversation it was opened from.
    from: SharedString,
    exchange: Option<Exchange>,
    problem: Option<SharedString>,
    /// What the header was last read for: the exchange's progress and how the hand-off row stands.
    read_for: Option<String>,
    /// The store version `read_for` was worked out at.
    read_at: Option<u64>,
    asked: bool,
    scroll: ScrollHandle,
    reply: Entity<TextareaState>,
    /// The reply's placeholder as last set: it follows who can be asked.
    placeholder: String,
    text: String,
    revision: u64,
    sending: bool,
    posted: bool,
    reply_problem: Option<SharedString>,
    _subscription: Subscription,
    /// Its focus as a sheet over the chat: Tab stays inside it (gpui-kit's focus trap).
    trap: FocusHandle,
}

impl ExchangeView {
    fn new(
        id: SharedString,
        from: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let colors = palette(cx);
        let editor = InputEditorStyle {
            foreground: hsla(colors.text.primary),
            muted_foreground: hsla(colors.text.tertiary),
            selection: hsla(colors.state.pressed),
            caret: hsla(colors.text.primary),
            ..InputEditorStyle::default()
        };
        let reply = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .submit_on_enter(true)
                .auto_grow(1, 4);
            field.set_editor_style(editor);
            field
        });
        let subscription =
            cx.subscribe_in(&reply, window, |this, _, event: &InputEvent, window, cx| {
                guarded("follow the reply", || this.follow_reply(event, window, cx));
            });
        ExchangeView {
            id,
            from,
            exchange: None,
            problem: None,
            read_for: None,
            read_at: None,
            asked: false,
            scroll: ScrollHandle::new(),
            reply,
            placeholder: String::new(),
            text: String::new(),
            revision: 0,
            sending: false,
            posted: false,
            reply_problem: None,
            _subscription: subscription,
            trap: cx.focus_handle(),
        }
    }

    fn follow_reply(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                self.revision += 1;
                self.text = self.reply.read(cx).value().to_string();
                self.posted = false;
                cx.notify();
            }
            InputEvent::PressEnter {
                secondary: false,
                shift: false,
            } => self.post(window, cx),
            _ => {}
        }
    }

    /// The two agents, as they are now (a deleted one is left out: there's no one to mention).
    fn agents(&self, cx: &App) -> Vec<Bot> {
        let Some(exchange) = &self.exchange else {
            return Vec::new();
        };
        let bots = &store::state(cx).bots;
        [&exchange.from_bot_id, &exchange.to_bot_id]
            .into_iter()
            .filter_map(|id| bots.iter().find(|bot| bot.id == **id).cloned())
            .collect()
    }

    /// Replies in Everyone with both agents mentioned, so both answer there. Its success clears the box only if nothing
    /// was typed since.
    fn post(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agents = self.agents(cx);
        if self.sending || agents.is_empty() || self.text.trim().is_empty() {
            return;
        }
        self.sending = true;
        self.reply_problem = None;
        let mentions: Vec<String> = agents.iter().map(|bot| format!("@{}", bot.name)).collect();
        let call = store::request::<method::RoomPost>(
            cx,
            RoomPost {
                room_id: TEAM_ROOM_ID.into(),
                text: format!("{} {}", mentions.join(" "), self.text.trim()),
            },
        );
        let sent = self.revision;
        cx.spawn_in(window, async move |this, cx| {
            guarded_async("reply in Everyone", async move {
                let outcome = call.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.sending = false;
                    match outcome {
                        Ok(_) if this.revision == sent => {
                            this.reply
                                .update(cx, |field, cx| field.set_value("", window, cx));
                            this.text.clear();
                            this.posted = true;
                        }
                        Ok(_) => {}
                        Err(error) => this.reply_problem = Some(error_text(&error).into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
        cx.notify();
    }

    /// Reads the exchange again when its messages move on, or the hand-off row in the conversation it was opened from
    /// changes: the exchange is answered or fails at the end of the asked agent's turn, after its last message.
    fn read(&mut self, cx: &mut Context<Self>) {
        // The progress decodes the opener's page: worked out again only when the state moved (M4 review A8).
        let version = store::version(cx);
        if self.read_at == Some(version) {
            return;
        }
        self.read_at = Some(version);
        let state = store::state(cx);
        let messages = state
            .conversations
            .get(self.id.as_ref())
            .map(|conversation| conversation.messages.as_slice())
            .unwrap_or_default();
        let opener = state
            .conversations
            .get(self.from.as_ref())
            .map(|conversation| conversation.messages.as_slice())
            .unwrap_or_default();
        // In a room, the exchange's newest post or notice there: its reply and its failure are saved with its new status.
        let posted = opener
            .iter()
            .rev()
            .find(|message| exchange_id(message).as_deref() == Some(self.id.as_ref()));
        let progress = format!(
            "{}:{:?}:{:?}:{:?}",
            messages.len(),
            messages.last().map(|message| message.status),
            handoff_status(opener, &self.id),
            posted.map(|message| (&message.id, message.status))
        );
        if self.read_for.as_ref() == Some(&progress) {
            return;
        }
        self.read_for = Some(progress);
        let call = store::request::<method::ExchangesGet>(
            cx,
            Id {
                id: self.id.to_string(),
            },
        );
        cx.spawn(async move |this, cx| {
            guarded_async("read an exchange", async move {
                let outcome = call.await;
                let _ = this.update(cx, |this, cx| {
                    match outcome {
                        Ok(view) => this.exchange = Some(view.exchange),
                        Err(error) => this.problem = Some(error_text(&error).into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }
}

impl Render for ExchangeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Loads once while connected, and again when a new connection clears it.
        let (ready, missing) = {
            let state = store::state(cx);
            (
                state.connection == crate::model::state::Connection::Ready,
                !state.conversations.contains_key(self.id.as_ref()),
            )
        };
        if !missing {
            self.asked = false;
        } else if ready && !self.asked {
            self.asked = true;
            store::load_conversation(cx, &self.id, History::Exchange(self.id.to_string())).detach();
        }
        self.read(cx);

        // The state's pointer: the rows borrow their messages (M4 review A8).
        let state = store::snapshot(cx);
        let bots = &state.bots;
        let conversation = state.conversations.get(self.id.as_ref());
        let failed = conversation
            .is_some_and(|conversation| conversation.status == ConversationStatus::Failed);
        let all = conversation
            .filter(|conversation| conversation.status == ConversationStatus::Ready)
            .map(|conversation| conversation.messages.as_slice())
            .unwrap_or_default();
        let messages: Vec<&Message> = all.iter().filter(|message| shown(message)).collect();
        // It counts the files, which are always shown.
        let caption = self
            .exchange
            .as_ref()
            .and_then(|exchange| exchange_caption(exchange, all, bots));
        let from = self.exchange.as_ref().and_then(|exchange| {
            bots.iter()
                .find(|bot| bot.id == exchange.from_bot_id)
                .cloned()
        });
        let to = self.exchange.as_ref().and_then(|exchange| {
            bots.iter()
                .find(|bot| bot.id == exchange.to_bot_id)
                .cloned()
        });
        let title = match &self.exchange {
            Some(_) => format!(
                "{} asked {}",
                from.as_ref().map_or(DELETED_BOT, |bot| bot.name.as_str()),
                to.as_ref().map_or(DELETED_BOT, |bot| bot.name.as_str())
            ),
            None => "Exchange".into(),
        };
        let meta = match (&self.exchange, &self.problem) {
            (Some(exchange), _) => exchange_meta(exchange, &Zoned::now()),
            (None, Some(problem)) => problem.to_string(),
            (None, None) => "Loading".into(),
        };
        let agents = self.agents(cx);

        let context = MessageContext {
            state: state.clone(),
            conversation_id: self.id.clone(),
            open_exchange: None,
            on_open_exchange: std::rc::Rc::new(|_, _, _| {}),
        };
        let mut previous: Option<String> = None;
        let rows: Vec<AnyElement> = messages
            .iter()
            .map(|message| {
                let starts = previous.as_deref() != Some(message.author.as_str());
                previous = Some(message.author.clone());
                message_view::row(message, starts, &context, window, cx)
            })
            .collect();

        let placeholder = match agents.as_slice() {
            [_, _] => "Ask both agents in Everyone".to_owned(),
            [one] => format!("Ask {} in Everyone", one.name),
            _ => "Reply in Everyone".to_owned(),
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            let text = placeholder.clone();
            self.reply
                .update(cx, |field, cx| field.set_placeholder(text, window, cx));
        }
        let theme = theme(cx);
        let colors = palette(cx);
        let id = self.id.clone();
        let avatar = |bot: Option<&Bot>| -> AnyElement {
            match bot {
                Some(bot) => BotAvatar::new(bot.color, bot.shape, 24.)
                    .pose(Pose::of(bot.state))
                    .into_any_element(),
                None => div()
                    .size(r(24.))
                    .flex_none()
                    .rounded_full()
                    .bg(hsla(colors.surface.sunk))
                    .into_any_element(),
            }
        };
        let can_send = !agents.is_empty() && !self.text.trim().is_empty();
        let reply_focus = self.reply.focus_handle(cx);
        let entity = cx.entity().downgrade();
        let send_entity = entity.clone();

        let body: AnyElement = if failed {
            let retry_id = id.clone();
            failed_load(
                "exchange.failed",
                "Couldn't load this exchange.",
                move |_, cx| {
                    store::retry_conversation(
                        cx,
                        &retry_id,
                        History::Exchange(retry_id.to_string()),
                    )
                    .detach();
                },
                cx,
            )
        } else {
            div()
                .id(SharedString::from(format!("exchange.{id}.messages")))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .px(r(theme.space.s16))
                .pt(r(theme.space.s16))
                .pb(r(theme.space.s12))
                .flex()
                .flex_col()
                .gap(r(theme.space.s12))
                .children(rows)
                .when_some(caption, |list, caption| {
                    list.child(
                        div()
                            .type_style(&theme.typography.desktop.small)
                            .text_color(hsla(colors.text.secondary))
                            .child(caption),
                    )
                })
                .into_any_element()
        };

        let chips = agents.iter().map(|bot| {
            div()
                .min_w_0()
                .max_w_full()
                .h(r(28.))
                .flex()
                .items_center()
                .gap(r(theme.space.s6))
                .px(r(theme.space.s10))
                .rounded_full()
                .bg(hsla(colors.surface.panel))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.primary))
                .child(BotAvatar::new(bot.color, bot.shape, 18.).pose(Pose::of(bot.state)))
                .child(div().min_w_0().truncate().child(format!("@{}", bot.name)))
        });

        // Over the chat or room it opened from it says so: a keyboard user knows what is behind it, its Send too.
        let over = crate::ui::shell::overlaid(window, cx).then(|| {
            if matches!(crate::ui::shell::route::current(cx), Route::Room(_)) {
                "Exchange, covers the room"
            } else {
                "Exchange, covers the chat"
            }
        });
        let view = div()
            .semantic(format!("exchange.{id}.view"), Role::Complementary)
            .aria_label(over.unwrap_or("Exchange"))
            .map(|view| {
                if crate::ui::shell::narrow(window) {
                    view.w_full()
                } else {
                    // Its own width, but never past the content area as a sheet over the chat.
                    view.w(r(460.)).max_w_full()
                }
            })
            // Over the chat, nothing behind it is clicked through it.
            .occlude()
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(colors.surface.panel))
            .rounded(r(theme.radius.panel))
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(shadows(&theme.shadow.light.glass))
            .child(
                div()
                    .mx(r(theme.space.s16))
                    .pt(r(theme.space.s12))
                    .pb(r(theme.space.s10))
                    .flex()
                    .items_start()
                    .gap(r(theme.space.s12))
                    .border_b_1()
                    .border_color(hsla(colors.line.base))
                    .child(
                        div()
                            .mt(r(theme.space.s4))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(r(theme.space.s6))
                            .child(avatar(from.as_ref()))
                            .child(
                                svg()
                                    .path("icons/i/175/arrow-right.svg")
                                    .size(r(14.))
                                    .text_color(hsla(colors.icon.muted)),
                            )
                            .child(avatar(to.as_ref())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .semantic(format!("exchange.{id}.title"), Role::Heading)
                                    .aria_label(SharedString::from(title.clone()))
                                    .truncate()
                                    .type_style(&theme.typography.desktop.ui_strong)
                                    .text_color(hsla(colors.text.primary))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .semantic(format!("exchange.{id}.meta"), Role::Label)
                                    .aria_label(SharedString::from(meta.clone()))
                                    .type_style(&theme.typography.desktop.meta)
                                    .text_color(hsla(colors.text.tertiary))
                                    .child(meta),
                            ),
                    )
                    .child(
                        IconButton::new(format!("exchange.{id}.close"), "x", "Close the exchange")
                            .on_press(|window, cx| {
                                dismiss(window, cx);
                            }),
                    ),
            )
            .child(body)
            .child(
                div()
                    .flex_none()
                    .px(r(theme.space.s16))
                    .pb(r(theme.space.s16))
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s6))
                    .when(self.posted, |reply| {
                        reply.child(
                            div()
                                .semantic(format!("exchange.{id}.posted"), Role::Status)
                                .aria_label("Posted in Everyone.")
                                .flex()
                                .items_center()
                                .gap(r(theme.space.s8))
                                .type_style(&theme.typography.desktop.small)
                                .text_color(hsla(colors.text.secondary))
                                .child("Posted in Everyone.")
                                .child(
                                    Link::new(format!("exchange.{id}.room"), "Open room").on_press(
                                        |_, cx| {
                                            cx.navigate(Route::Room(TEAM_ROOM_ID.into()));
                                        },
                                    ),
                                ),
                        )
                    })
                    .child(
                        div()
                            .key_context(keymap::COMPOSER)
                            .on_action(move |_: &Send, window, cx| {
                                guarded("reply by Enter", || {
                                    let _ =
                                        send_entity.update(cx, |this, cx| this.post(window, cx));
                                });
                            })
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(r(theme.space.s6))
                            .p(r(theme.space.s10))
                            .bg(hsla(colors.surface.raised))
                            .rounded(r(theme.radius.card))
                            .border_t_1()
                            .border_color(hsla(colors.overlay.highlight))
                            .shadow(shadows(&theme.shadow.light.raised))
                            .child(
                                div()
                                    .semantic(format!("exchange.{id}.recipients"), Role::List)
                                    .aria_label("Reply recipients")
                                    .flex()
                                    .flex_wrap()
                                    .min_w_0()
                                    .gap(r(theme.space.s8))
                                    .children(chips),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_end()
                                    .gap(r(theme.space.s8))
                                    .child(
                                        div()
                                            .semantic(format!("exchange.{id}.reply"), Role::Group)
                                            .aria_label(SharedString::from(placeholder))
                                            // The field inside is gpui-kit's: the driver reads its focus as this
                                            // group's (M4 review C4).
                                            .semantic_focus(&reply_focus)
                                            .flex_1()
                                            .min_w_0()
                                            .px(r(theme.space.s6))
                                            .py(r(theme.space.s6))
                                            .type_style(&theme.typography.desktop.body)
                                            .text_color(hsla(colors.text.primary))
                                            .child(Textarea::new(&self.reply)),
                                    )
                                    .child(
                                        IconButton::new(
                                            format!("exchange.{id}.send"),
                                            "arrow-up",
                                            "Send",
                                        )
                                        .variant(if can_send {
                                            IconVariant::Dark
                                        } else {
                                            IconVariant::Grey
                                        })
                                        .on_press(
                                            move |window, cx| {
                                                let _ = entity
                                                    .update(cx, |this, cx| this.post(window, cx));
                                            },
                                        ),
                                    ),
                            ),
                    )
                    .when_some(self.reply_problem.clone(), |reply, problem| {
                        reply.child(
                            div()
                                .semantic(format!("exchange.{id}.problem"), Role::Alert)
                                .aria_label(problem.clone())
                                .type_style(&theme.typography.desktop.small)
                                .text_color(hsla(colors.danger.text))
                                .child(problem),
                        )
                    }),
            );
        if over.is_some() {
            view.focus_trap("exchange.trap", &self.trap)
                .into_any_element()
        } else {
            view.into_any_element()
        }
    }
}

/// The browser panel and the exchange view never show together; the newer one wins: once the panel opens while an
/// exchange shows, the exchange closes.
pub fn reconcile(cx: &mut App) {
    if open_id(cx).is_some() && BrowserPanel::open_now(cx) {
        close(cx);
    }
}

/// The header's panel button and Ctrl+B (M4 review A6): closes the exchange if one shows, otherwise opens or closes the
/// browser panel.
pub fn toggle_panel(trigger: Trigger, cx: &mut App) {
    if open_id(cx).is_some() {
        close(cx);
    } else {
        BrowserPanel::toggle(trigger, cx);
    }
}
