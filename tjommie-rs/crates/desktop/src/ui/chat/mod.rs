//! The chat (Phase 3 plan M4; desktop-chat.html, `chat-screen.tsx`): an agent's header, its thread, and the composer
//! pinned under it. The thread loads when the chat opens, and again when a new connection clears it; older pages load
//! as you reach the top. An unknown or deleted agent has no chat: the app goes back to Agents.

pub mod approval;
pub mod bubbles;
pub mod composer;
pub mod exchange;
pub mod header;
pub mod message_view;
pub mod notices;
pub mod rows;
pub mod setup_banner;
pub mod sign_in_toast;
#[cfg(test)]
mod tests;
pub mod thread;
pub mod voice;

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div,
};
use jiff::Zoned;

use self::composer::Composer;
use self::header::HeaderActions;
use self::message_view::{MessageContext, shown};
use self::thread::Thread;
use crate::guard::{guarded, guarded_async};
use crate::model::labels::autonomy_hint;
use crate::model::state::{Connection, ConversationStatus};
use crate::motion::keyboard::Trigger;
use crate::semantic::SemanticExt as _;
use crate::ui::browser::panel::BrowserPanel;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::shell::connection::failed_load;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store::{self, History};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// The chat with `bot_id`, kept while the route shows it.
pub fn screen(bot_id: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let id = SharedString::from(bot_id.to_owned());
    window
        .use_keyed_state(SharedString::from(format!("chat.{bot_id}")), cx, |_, cx| {
            // Leaving the chat closes the exchange beside it, forgetting its conversation.
            cx.on_release(|_, cx| {
                guarded("close the exchange", || exchange::close(cx));
            })
            .detach();
            ChatScreen::new(id)
        })
        .into_any_element()
}

pub struct ChatScreen {
    bot_id: SharedString,
    thread: Thread,
    /// The newest page was asked for, and the conversation hasn't appeared since.
    asked: bool,
    /// The older page on its way, if any.
    older: Option<Task<()>>,
    /// The oldest message when an older page was last asked for: a page that didn't come isn't asked for again until
    /// the conversation changes.
    older_from: Option<String>,
    /// Made once the agent is known: its placeholder and model come from it.
    composer: Option<Entity<Composer>>,
}

impl ChatScreen {
    pub fn new(bot_id: SharedString) -> Self {
        ChatScreen {
            thread: Thread::new(bot_id.clone()),
            bot_id,
            asked: false,
            older: None,
            older_from: None,
            composer: None,
        }
    }

    fn history(&self) -> History {
        History::Bot(self.bot_id.to_string())
    }

    /// Loads the newest page when the chat opens, and again when a new connection clears it.
    fn load(&mut self, cx: &mut Context<Self>) {
        let state = store::state(cx);
        let ready = state.connection == Connection::Ready;
        let missing = !state.conversations.contains_key(self.bot_id.as_ref());
        if !missing {
            self.asked = false;
        } else if ready && !self.asked {
            self.asked = true;
            let history = self.history();
            store::load_conversation(cx, &self.bot_id, history).detach();
        }
    }

    /// Loads the page before the oldest message once the top of the thread is in reach.
    fn load_older(&mut self, cx: &mut Context<Self>) {
        let state = store::state(cx);
        let Some(conversation) = state.conversations.get(self.bot_id.as_ref()) else {
            return;
        };
        let first = conversation
            .messages
            .first()
            .map(|message| message.id.clone());
        if conversation.status != ConversationStatus::Ready
            || !conversation.has_more
            || self.older.is_some()
            || first == self.older_from
            || !self.thread.top_in_reach()
        {
            return;
        }
        self.older_from = first;
        let page = store::load_older(cx, &self.bot_id, self.history());
        self.older = Some(cx.spawn(async move |this, cx| {
            guarded_async("load an older page", async move {
                // A page that didn't come is left: the top asks again once the conversation changes.
                let _ = page.await;
                let _ = this.update(cx, |this, cx| {
                    this.older = None;
                    cx.notify();
                });
            })
            .await;
        }));
    }
}

impl Render for ChatScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load(cx);
        exchange::reconcile(cx);
        let open_exchange = exchange::open_id(cx);
        let panel_open = open_exchange.is_some() || BrowserPanel::open_now(cx);
        let state = store::state(cx);
        let ready = state.connection == Connection::Ready;
        let bots = Rc::new(state.bots.clone());
        let Some(bot) = bots.iter().find(|bot| *bot.id == *self.bot_id).cloned() else {
            if ready {
                cx.defer(|cx| {
                    guarded("leave a chat with no agent", || cx.navigate(Route::Agents));
                });
            }
            return div().semantic("screen.chat", Role::Group).flex_1();
        };
        let conversation = state.conversations.get(self.bot_id.as_ref());
        let status = conversation.map(|conversation| conversation.status);
        let empty =
            conversation.is_some_and(|conversation| !conversation.messages.iter().any(shown));
        if let Some(conversation) = conversation
            && conversation.status == ConversationStatus::Ready
        {
            self.thread.sync(&conversation.messages, &Zoned::now());
        }
        self.load_older(cx);

        let from = self.bot_id.clone();
        let context = MessageContext {
            bots: bots.clone(),
            conversation_id: self.bot_id.clone(),
            open_exchange,
            on_open_exchange: Rc::new(move |id, window, cx| {
                exchange::open(id, from.clone(), window, cx)
            }),
        };
        let actions = HeaderActions {
            panel_open,
            on_toggle_panel: Rc::new(|window, cx| exchange::toggle_panel(Trigger::of(window), cx)),
            on_open_browser: Rc::new(|window, cx| {
                exchange::close(cx);
                BrowserPanel::open_browser(Trigger::of(window), cx);
            }),
        };
        let body = match status {
            Some(ConversationStatus::Failed) => {
                let id = self.bot_id.clone();
                failed_load(
                    "chat.failed",
                    "Couldn't load this chat.",
                    move |_, cx| {
                        store::retry_conversation(cx, &id, History::Bot(id.to_string())).detach();
                    },
                    cx,
                )
            }
            Some(ConversationStatus::Ready) if empty => empty_chat(&bot, cx),
            Some(ConversationStatus::Ready) => self.thread.render(context, cx),
            _ => div().flex_1().into_any_element(),
        };
        let header = header::header(&bot, &actions, window, cx);
        sign_in_toast::forget_resolved(cx);
        // An account notice it raises is the shell's to draw: it stays through a navigation (M4 review A9).
        let banner = setup_banner::banner(&bot, window, cx);
        let composer = self
            .composer
            .get_or_insert_with(|| cx.new(|cx| Composer::new(&bot, window, cx)))
            .clone();
        div()
            .semantic("screen.chat", Role::Group)
            .aria_label(SharedString::from(format!("Chat with {}", bot.name)))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .relative()
            .child(header)
            .children(banner)
            .child(body)
            .child(composer)
    }
}

/// A chat with nothing in it yet: the agent, ready, and how it works.
fn empty_chat(bot: &protocol::model::Bot, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let title = SharedString::from(format!("{} is ready", bot.name));
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(r(theme.space.s12))
        .px(r(theme.space.s32))
        .child(
            BotAvatar::new(bot.color, bot.shape, theme.size.avatar64)
                .pose(Pose::of(bot.state))
                .living(bot.id.clone().into()),
        )
        .child(
            div()
                .semantic("chat.empty", Role::Heading)
                .aria_label(title.clone())
                .mt(r(theme.space.s6))
                .type_style(&theme.typography.desktop.display_m)
                .text_color(hsla(colors.text.primary))
                .child(title),
        )
        .child(
            div()
                .max_w(r(340.))
                .text_center()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.secondary))
                .child(autonomy_hint(bot.autonomy)),
        )
        .into_any_element()
}
