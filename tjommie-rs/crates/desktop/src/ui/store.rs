//! The app state as the screens read it (Phase 3 plan M4, M5): `state` is store.ts's `useApp`, `update` its
//! `update`, and `request` connection.ts's `request`. The conversation helpers are connection.ts's
//! `loadConversation`, `retryConversation`, `loadOlder` and `closeConversation`.
//!
//! The live source is T1.3's `AppConnection` (`crate::live`): calls and conversations go straight to it, and its state
//! is mirrored here after each change. Before it is wired (and in a test that installs nothing) every request fails as
//! "not connected". View tests install a fake: the state they give, and calls answered from a table
//! (`model::testing::FakeRpc`), which records them.
//!
//! Screens don't keep the state: they read it as they render, so a change shows on the next frame.

use std::sync::{Arc, LazyLock};

use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui::{App, AppContext as _, Global, Task};
use protocol::ClientMethod;

use crate::live::Live;
use crate::model::connection::{FetchPage, RequestError};
use crate::model::state::AppState;

#[derive(Default)]
pub struct Store {
    state: AppState,
    source: Source,
}

impl Global for Store {}

#[derive(Default)]
enum Source {
    /// Nothing to call: every request fails as not connected.
    #[default]
    None,
    Live(Arc<Live>),
    #[cfg(test)]
    Fake(Arc<crate::model::testing::FakeRpc>),
}

/// The state before anything is wired: connecting, with nothing listed.
static EMPTY: LazyLock<AppState> = LazyLock::new(AppState::default);

/// `useApp`: the state as it is now.
pub fn state(cx: &App) -> &AppState {
    cx.try_global::<Store>()
        .map_or(&EMPTY, |store| &store.state)
}

/// `update(change)`: `change` edits the state and says whether it changed it; the windows redraw when it did. With
/// the live connection the change is the connection's, and the copy here follows at once.
pub fn update(cx: &mut App, change: impl FnOnce(&mut AppState) -> bool) {
    let store = cx.default_global::<Store>();
    let changed = match &store.source {
        Source::Live(live) => {
            let mut changed = false;
            live.update(|state| {
                changed = change(state);
                changed
            });
            if changed {
                store.state = live.state();
            }
            changed
        }
        _ => change(&mut store.state),
    };
    if changed {
        cx.refresh_windows();
    }
}

/// connection.ts's `request`: one daemon call, or `NotConnected` while there is no connection. The future is the
/// call's own: it needs nothing of the app, so it can run on any executor.
pub fn request<M: ClientMethod>(
    cx: &App,
    params: M::Params,
) -> BoxFuture<'static, Result<M::Result, RequestError>> {
    match cx.try_global::<Store>().map(|store| &store.source) {
        None | Some(Source::None) => {
            let _ = params;
            futures::future::ready(Err(RequestError::NotConnected)).boxed()
        }
        Some(Source::Live(live)) => {
            use client::Rpc as _;
            match live.daemon() {
                Ok(daemon) => {
                    async move { daemon.call::<M>(params).await.map_err(RequestError::Rpc) }.boxed()
                }
                Err(error) => futures::future::ready(Err(error)).boxed(),
            }
        }
        #[cfg(test)]
        Some(Source::Fake(rpc)) => {
            use client::Rpc as _;
            let rpc = Arc::clone(rpc);
            async move { rpc.call::<M>(params).await.map_err(RequestError::Rpc) }.boxed()
        }
    }
}

/// Which pages a conversation reads (connection.ts's `botHistory`, `roomHistory` and `exchangePage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum History {
    Bot(String),
    Room(String),
    Exchange(String),
}

/// The daemon as an `Rpc`, for the view models that take one (`model::routines::save_routine` and the like): its calls
/// go where `request`'s go, and fail as not connected while there is no connection.
#[derive(Clone)]
pub struct StoreRpc(Handle);

/// `StoreRpc` for the store as it is now.
pub fn rpc(cx: &App) -> StoreRpc {
    StoreRpc(handle(cx))
}

impl client::Rpc for StoreRpc {
    fn call<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> impl Future<Output = Result<M::Result, client::RpcError>> + Send {
        let handle = self.0.clone();
        let not_connected = || {
            client::RpcError::Closed(client::CloseReason {
                code: 1006,
                reason: "Tjommie isn't connected right now.".to_owned(),
                opened: false,
            })
        };
        async move {
            match handle {
                Handle::None => Err(not_connected()),
                Handle::Live(live) => match live.daemon() {
                    Ok(daemon) => daemon.call::<M>(params).await,
                    Err(_) => Err(not_connected()),
                },
                #[cfg(test)]
                Handle::Fake(rpc) => rpc.call::<M>(params).await,
            }
        }
    }
}

/// The selected PC and the connection as `model::logins` reads them (`LiveState`): as they are at each moment, from
/// any thread. Without a live connection (a view test, or nothing yet) they're what the state said when it was taken.
#[derive(Clone)]
pub struct StoreLive(LiveSource);

#[derive(Clone)]
enum LiveSource {
    Fixed { host: Option<String>, ready: bool },
    Live(Arc<Live>),
}

/// `StoreLive` for the store as it is now.
pub fn live_state(cx: &App) -> StoreLive {
    match handle(cx) {
        Handle::Live(live) => StoreLive(LiveSource::Live(live)),
        _ => {
            let state = state(cx);
            let ready = state.connection == crate::model::state::Connection::Ready;
            StoreLive(LiveSource::Fixed {
                host: state.host.clone(),
                ready,
            })
        }
    }
}

impl crate::model::logins::LiveState for StoreLive {
    fn selected_host(&self) -> Option<String> {
        match &self.0 {
            LiveSource::Fixed { host, .. } => host.clone(),
            LiveSource::Live(live) => live.read(|state| state.host.clone()),
        }
    }

    fn is_ready(&self) -> bool {
        match &self.0 {
            LiveSource::Fixed { ready, .. } => *ready,
            LiveSource::Live(live) => {
                live.read(|state| state.connection == crate::model::state::Connection::Ready)
            }
        }
    }

    fn daemon_generation(&self) -> Option<u64> {
        match &self.0 {
            LiveSource::Fixed { ready, .. } => ready.then_some(0),
            LiveSource::Live(live) => live.link_id(),
        }
    }
}

/// What a helper works through: the live connection, a test's fake, or nothing yet.
#[derive(Clone)]
enum Handle {
    None,
    Live(Arc<Live>),
    #[cfg(test)]
    Fake(Arc<crate::model::testing::FakeRpc>),
}

fn handle(cx: &App) -> Handle {
    match cx.try_global::<Store>().map(|store| &store.source) {
        None | Some(Source::None) => Handle::None,
        Some(Source::Live(live)) => Handle::Live(Arc::clone(live)),
        #[cfg(test)]
        Some(Source::Fake(rpc)) => Handle::Fake(Arc::clone(rpc)),
    }
}

/// A screen's feed of the daemon events it reads again on (`wanted`), for what the state doesn't keep (projects,
/// devices, usage): each one arriving sends `()` to `arrivals`. Dropping the feed stops it. Without a live connection
/// it never sends; a view test sends with `fake_event`.
pub struct EventFeed {
    pub arrivals: futures::channel::mpsc::UnboundedReceiver<()>,
    _stop: Option<StopFeed>,
}

struct StopFeed {
    live: std::sync::Weak<Live>,
    id: crate::model::store::SubscriptionId,
}

impl Drop for StopFeed {
    fn drop(&mut self) {
        if let Some(live) = self.live.upgrade() {
            live.unsubscribe(self.id);
        }
    }
}

/// `daemon().subscribe` for a screen: a feed of the events `wanted` accepts (an account's card wants its own).
pub fn events(
    cx: &mut App,
    wanted: impl Fn(&protocol::Event) -> bool + Send + 'static,
) -> EventFeed {
    let (sender, arrivals) = futures::channel::mpsc::unbounded();
    match handle(cx) {
        Handle::Live(live) => {
            // unguarded: the connection's own listener, not a GPUI handler; it only sends a signal (inside the connection's lock)
            let id = live.subscribe_events(move |event| {
                if wanted(event) {
                    let _ = sender.unbounded_send(());
                }
            });
            EventFeed {
                arrivals,
                _stop: Some(StopFeed {
                    live: Arc::downgrade(&live),
                    id,
                }),
            }
        }
        #[cfg(test)]
        Handle::Fake(_) => {
            cx.default_global::<FakeEvents>()
                .0
                .push((Box::new(wanted), sender));
            EventFeed {
                arrivals,
                _stop: None,
            }
        }
        Handle::None => EventFeed {
            arrivals,
            _stop: None,
        },
    }
}

/// The view tests' event feeds.
#[cfg(test)]
#[derive(Default)]
struct FakeEvents(Vec<FakeFeed>);

/// One fake feed: what it wants, and where it sends.
#[cfg(test)]
type FakeFeed = (
    Box<dyn Fn(&protocol::Event) -> bool>,
    futures::channel::mpsc::UnboundedSender<()>,
);

#[cfg(test)]
impl Global for FakeEvents {}

/// A view test's daemon event: applied to the fake state, and sent to the feeds that want it.
#[cfg(test)]
pub fn fake_event(cx: &mut App, event: &protocol::Event) {
    update(cx, |state| crate::model::state::apply_event(state, event));
    if let Some(feeds) = cx.try_global::<FakeEvents>() {
        for (wanted, sender) in &feeds.0 {
            if wanted(event) {
                let _ = sender.unbounded_send(());
            }
        }
    }
}

/// `loadConversation`: loads a conversation's newest page once; events keep it current from then on, and a reconnect
/// catches it up. The load finishes even if the task is dropped.
pub fn load_conversation(cx: &mut App, conversation_id: &str, history: History) -> Task<()> {
    let id = conversation_id.to_owned();
    match handle(cx) {
        Handle::None => Task::ready(()),
        Handle::Live(live) => {
            let pages = pages(&live, &history);
            cx.background_spawn(async move { live.load_conversation(&id, pages).await })
        }
        #[cfg(test)]
        Handle::Fake(rpc) => {
            // unguarded: the view tests' fake, never in the app
            cx.spawn(async move |cx| fake::load(&rpc, id, history, cx).await)
        }
    }
}

/// `retryConversation`: after "Couldn't load this chat.", forgets the failed load and loads it again.
pub fn retry_conversation(cx: &mut App, conversation_id: &str, history: History) -> Task<()> {
    let id = conversation_id.to_owned();
    match handle(cx) {
        Handle::None => Task::ready(()),
        Handle::Live(live) => {
            let pages = pages(&live, &history);
            cx.background_spawn(async move { live.retry_conversation(&id, pages).await })
        }
        #[cfg(test)]
        Handle::Fake(rpc) => {
            update(cx, |state| {
                crate::model::state::forget_conversation(state, &id)
            });
            // unguarded: the view tests' fake, never in the app
            cx.spawn(async move |cx| fake::load(&rpc, id, history, cx).await)
        }
    }
}

/// `loadOlder`: the page before the oldest loaded message, above what's shown.
pub fn load_older(
    cx: &mut App,
    conversation_id: &str,
    history: History,
) -> Task<Result<(), RequestError>> {
    let id = conversation_id.to_owned();
    match handle(cx) {
        Handle::None => Task::ready(Ok(())),
        Handle::Live(live) => {
            let pages = pages(&live, &history);
            cx.background_spawn(async move { live.load_older(&id, pages).await })
        }
        #[cfg(test)]
        Handle::Fake(rpc) => {
            // unguarded: the view tests' fake, never in the app
            cx.spawn(async move |cx| fake::older(&rpc, id, history, cx).await)
        }
    }
}

/// `closeConversation`: a closed view's conversation (an exchange) is forgotten, so it stops following events.
pub fn close_conversation(cx: &mut App, conversation_id: &str) {
    match handle(cx) {
        Handle::None => {}
        Handle::Live(live) => {
            live.close_conversation(conversation_id);
            mirror(cx, live.state());
        }
        #[cfg(test)]
        Handle::Fake(_) => update(cx, |state| {
            crate::model::state::forget_conversation(state, conversation_id)
        }),
    }
}

fn pages(live: &Arc<Live>, history: &History) -> FetchPage {
    match history {
        History::Bot(id) => live.bot_history(id),
        History::Room(id) => live.room_history(id),
        History::Exchange(id) => live.exchange_page(id),
    }
}

/// The fake's conversations: what `AppConnection` does with its pages, on the store's own state.
#[cfg(test)]
mod fake {
    use client::Rpc as _;
    use gpui::AsyncApp;
    use protocol::wire::{ChatHistory, Id, RoomHistory, method};

    use super::{History, update};
    use crate::model::connection::{PAGE, RequestError};
    use crate::model::state::{ConversationStatus, Page, add_older, finish_loading, start_loading};
    use crate::model::testing::FakeRpc;

    async fn page(
        rpc: &FakeRpc,
        history: &History,
        before: Option<String>,
    ) -> Result<Page, RequestError> {
        let page = match history {
            History::Bot(bot_id) => rpc
                .call::<method::ChatHistory>(ChatHistory {
                    bot_id: bot_id.clone(),
                    before,
                    limit: PAGE,
                })
                .await
                .map(|page| (page.messages, page.has_more)),
            History::Room(room_id) => rpc
                .call::<method::RoomHistory>(RoomHistory {
                    before,
                    limit: PAGE,
                    room_id: room_id.clone(),
                })
                .await
                .map(|page| (page.messages, page.has_more)),
            History::Exchange(id) => rpc
                .call::<method::ExchangesGet>(Id { id: id.clone() })
                .await
                .map(|view| (view.messages, false)),
        };
        page.map(|(messages, has_more)| Page { messages, has_more })
            .map_err(RequestError::Rpc)
    }

    pub(super) async fn load(rpc: &FakeRpc, id: String, history: History, cx: &mut AsyncApp) {
        let started = cx.update(|cx| {
            let known = super::state(cx).conversations.contains_key(&id);
            if !known {
                update(cx, |state| start_loading(state, &id));
            }
            !known
        });
        if !started {
            return;
        }
        let loaded = page(rpc, &history, None).await;
        cx.update(|cx| {
            update(cx, |state| match loaded {
                Ok(page) => finish_loading(state, &id, page),
                Err(_) => match state.conversations.get_mut(&id) {
                    Some(chat) if chat.status == ConversationStatus::Loading => {
                        chat.status = ConversationStatus::Failed;
                        chat.pending.clear();
                        true
                    }
                    _ => false,
                },
            })
        });
    }

    pub(super) async fn older(
        rpc: &FakeRpc,
        id: String,
        history: History,
        cx: &mut AsyncApp,
    ) -> Result<(), RequestError> {
        let before = cx.update(|cx| {
            super::state(cx).conversations.get(&id).and_then(|chat| {
                (chat.status == ConversationStatus::Ready && chat.has_more)
                    .then(|| chat.messages.first().map(|first| first.id.clone()))
                    .flatten()
            })
        });
        let Some(before) = before else {
            return Ok(());
        };
        let older = page(rpc, &history, Some(before)).await?;
        cx.update(|cx| update(cx, |state| add_older(state, &id, older)));
        Ok(())
    }
}

/// The live connection becomes the store's source (`crate::live::start`).
pub(crate) fn connect(cx: &mut App, live: Arc<Live>) {
    cx.set_global(Store {
        state: live.state(),
        source: Source::Live(live),
    });
    cx.refresh_windows();
}

/// The live connection's state after a change.
pub(crate) fn mirror(cx: &mut App, state: AppState) {
    let store = cx.default_global::<Store>();
    if store.state != state {
        store.state = state;
        cx.refresh_windows();
    }
}

/// A view test's store: `state` as given, and calls answered by `rpc`. The returned handle reads the calls made.
#[cfg(test)]
pub fn fake(
    cx: &mut App,
    state: AppState,
    rpc: crate::model::testing::FakeRpc,
) -> Arc<crate::model::testing::FakeRpc> {
    let rpc = Arc::new(rpc);
    cx.set_global(Store {
        state,
        source: Source::Fake(Arc::clone(&rpc)),
    });
    rpc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::state::{Connection, ConversationStatus};
    use crate::model::testing::FakeRpc;
    use protocol::model::{
        Autonomy, Bot, BotColor, BotShape, BotState, Message, MessageKind, MessageStatus, Provider,
    };
    use protocol::wire::{BotsPause, MessagePage, method};

    fn forge(paused: bool) -> Bot {
        Bot {
            id: "bot-1".into(),
            name: "Forge".into(),
            color: BotColor::Blue,
            shape: BotShape::Hexagon,
            instructions: String::new(),
            provider: Provider::Claude,
            model: "sonnet".into(),
            account_id: None,
            effective_account_id: None,
            using_fallback: None,
            autonomy: Autonomy::Ask,
            home_folder: "C:/Agents/Forge".into(),
            project_folder: None,
            browser: false,
            paused,
            can_message: Vec::new(),
            state: if paused {
                BotState::Paused
            } else {
                BotState::Idle
            },
            setup_reason: None,
            limited_until: None,
            created_at: "2026-11-01T09:00:00.000Z".into(),
        }
    }

    fn said(id: &str, conversation: &str, body: &str) -> Message {
        Message {
            id: id.into(),
            conversation_id: conversation.into(),
            author: "you".into(),
            kind: MessageKind::Text,
            body: body.into(),
            data: None,
            status: MessageStatus::Done,
            created_at: "2026-11-03T09:00:00.000Z".into(),
        }
    }

    fn page(messages: Vec<Message>, has_more: bool) -> MessagePage {
        MessagePage { messages, has_more }
    }

    #[gpui::test]
    fn before_anything_is_wired_it_is_connecting_and_calls_fail(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(state(cx).connection, Connection::Connecting);
            assert!(state(cx).bots.is_empty());
        });
        let call = cx.update(|cx| {
            request::<method::BotsPause>(
                cx,
                BotsPause {
                    id: "bot-1".into(),
                    paused: true,
                },
            )
        });
        assert_eq!(
            futures::executor::block_on(call),
            Err(RequestError::NotConnected)
        );
    }

    #[gpui::test]
    fn a_fake_gives_its_state_and_answers_and_records_calls(cx: &mut gpui::TestAppContext) {
        let given = AppState {
            connection: Connection::Ready,
            bots: vec![forge(false)],
            ..AppState::default()
        };
        let rpc = cx.update(|cx| {
            fake(
                cx,
                given.clone(),
                FakeRpc::new().on::<method::BotsPause>(|_| Ok(forge(true))),
            )
        });
        cx.update(|cx| assert_eq!(*state(cx), given));
        let call = cx.update(|cx| {
            request::<method::BotsPause>(
                cx,
                BotsPause {
                    id: "bot-1".into(),
                    paused: true,
                },
            )
        });
        assert_eq!(
            futures::executor::block_on(call).map(|bot| bot.paused),
            Ok(true)
        );
        let calls = rpc.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bots.pause");
        assert_eq!(calls[0].1["paused"], true);
    }

    #[gpui::test]
    fn update_changes_what_state_reads(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            fake(cx, AppState::default(), FakeRpc::new());
            update(cx, |state| {
                state.bots.push(forge(false));
                true
            });
            assert_eq!(state(cx).bots.len(), 1);
            // A change that changed nothing leaves the state as it was.
            update(cx, |_| false);
            assert_eq!(state(cx).bots.len(), 1);
        });
    }

    #[gpui::test]
    fn the_rpc_handle_calls_where_request_does(cx: &mut gpui::TestAppContext) {
        use client::Rpc as _;
        let none = cx.update(|cx| rpc(cx));
        let refused = futures::executor::block_on(none.call::<method::BotsPause>(BotsPause {
            id: "bot-1".into(),
            paused: true,
        }));
        assert!(
            matches!(&refused, Err(client::RpcError::Closed(reason)) if reason.code == 1006),
            "{refused:?}"
        );
        let calls = cx.update(|cx| {
            fake(
                cx,
                AppState::default(),
                FakeRpc::new().on::<method::BotsPause>(|_| Ok(forge(true))),
            )
        });
        let handle = cx.update(|cx| rpc(cx));
        let paused = futures::executor::block_on(handle.call::<method::BotsPause>(BotsPause {
            id: "bot-1".into(),
            paused: true,
        }));
        assert_eq!(paused.map(|bot| bot.paused), Ok(true));
        assert_eq!(calls.calls()[0].0, "bots.pause");
    }

    #[gpui::test]
    fn a_conversation_loads_its_newest_page_once(cx: &mut gpui::TestAppContext) {
        let rpc = cx.update(|cx| {
            fake(
                cx,
                AppState::default(),
                FakeRpc::new().on::<method::ChatHistory>(|_| {
                    Ok(page(
                        vec![said("m1", "bot-1", "Hi"), said("m2", "bot-1", "Again")],
                        true,
                    ))
                }),
            )
        });
        cx.update(|cx| load_conversation(cx, "bot-1", History::Bot("bot-1".into())))
            .detach();
        cx.run_until_parked();
        cx.update(|cx| {
            let chat = &state(cx).conversations["bot-1"];
            assert_eq!(chat.status, ConversationStatus::Ready);
            assert_eq!(chat.messages.len(), 2);
            assert!(chat.has_more);
        });
        // Loaded: opening it again reads nothing.
        cx.update(|cx| load_conversation(cx, "bot-1", History::Bot("bot-1".into())))
            .detach();
        cx.run_until_parked();
        let calls = rpc.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "chat.history");
        assert_eq!(calls[0].1["botId"], "bot-1");
    }

    #[gpui::test]
    fn a_failed_load_says_so_and_retry_loads_it_again(cx: &mut gpui::TestAppContext) {
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&tries);
        cx.update(|cx| {
            fake(
                cx,
                AppState::default(),
                FakeRpc::new().on::<method::RoomHistory>(move |_| {
                    if counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        Err(client::RpcError::BadResult("down".into()))
                    } else {
                        Ok(page(vec![said("m1", "room-1", "Hello all")], false))
                    }
                }),
            );
        });
        cx.update(|cx| load_conversation(cx, "room-1", History::Room("room-1".into())))
            .detach();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                state(cx).conversations["room-1"].status,
                ConversationStatus::Failed
            )
        });
        cx.update(|cx| retry_conversation(cx, "room-1", History::Room("room-1".into())))
            .detach();
        cx.run_until_parked();
        cx.update(|cx| {
            let room = &state(cx).conversations["room-1"];
            assert_eq!(room.status, ConversationStatus::Ready);
            assert_eq!(room.messages.len(), 1);
        });
    }

    #[gpui::test]
    fn older_pages_go_above_and_a_closed_conversation_is_forgotten(cx: &mut gpui::TestAppContext) {
        let rpc = cx.update(|cx| {
            fake(
                cx,
                AppState::default(),
                FakeRpc::new().on::<method::ChatHistory>(|params| {
                    Ok(if params.get("before").is_some() {
                        page(vec![said("m0", "bot-1", "First")], false)
                    } else {
                        page(vec![said("m1", "bot-1", "Second")], true)
                    })
                }),
            )
        });
        cx.update(|cx| load_conversation(cx, "bot-1", History::Bot("bot-1".into())))
            .detach();
        cx.run_until_parked();
        let older = cx.update(|cx| load_older(cx, "bot-1", History::Bot("bot-1".into())));
        cx.run_until_parked();
        assert_eq!(futures::executor::block_on(older), Ok(()));
        cx.update(|cx| {
            let chat = &state(cx).conversations["bot-1"];
            let ids: Vec<&str> = chat.messages.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids, ["m0", "m1"]);
            assert!(!chat.has_more);
        });
        assert_eq!(rpc.calls()[1].1["before"], "m1");
        cx.update(|cx| {
            close_conversation(cx, "bot-1");
            assert!(!state(cx).conversations.contains_key("bot-1"));
        });
    }
}
