//! The app state as the screens read it (Phase 3 plan M4, M5): `state` is store.ts's `useApp`, `update` its
//! `update`, and `request` connection.ts's `request`. The live source is T1.3's `AppConnection`, wired after T5.1;
//! until then (and in a test that installs nothing) every request fails as "not connected". View tests install a
//! fake: the state they give, and calls answered from a table (`model::testing::FakeRpc`), which records them.
//!
//! Screens don't keep the state: they read it as they render, so a change shows on the next frame.

use std::sync::LazyLock;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui::{App, Global};
use protocol::ClientMethod;

use crate::model::connection::RequestError;
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
    #[cfg(test)]
    Fake(std::sync::Arc<crate::model::testing::FakeRpc>),
}

/// The state before anything is wired: connecting, with nothing listed.
static EMPTY: LazyLock<AppState> = LazyLock::new(AppState::default);

/// `useApp`: the state as it is now.
pub fn state(cx: &App) -> &AppState {
    cx.try_global::<Store>()
        .map_or(&EMPTY, |store| &store.state)
}

/// `update(change)`: `change` edits the state and says whether it changed it; the windows redraw when it did.
pub fn update(cx: &mut App, change: impl FnOnce(&mut AppState) -> bool) {
    if change(&mut cx.default_global::<Store>().state) {
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
        #[cfg(test)]
        Some(Source::Fake(rpc)) => {
            use client::Rpc as _;
            let rpc = std::sync::Arc::clone(rpc);
            async move { rpc.call::<M>(params).await.map_err(RequestError::Rpc) }.boxed()
        }
    }
}

/// A view test's store: `state` as given, and calls answered by `rpc`. The returned handle reads the calls made.
#[cfg(test)]
pub fn fake(
    cx: &mut App,
    state: AppState,
    rpc: crate::model::testing::FakeRpc,
) -> std::sync::Arc<crate::model::testing::FakeRpc> {
    let rpc = std::sync::Arc::new(rpc);
    cx.set_global(Store {
        state,
        source: Source::Fake(std::sync::Arc::clone(&rpc)),
    });
    rpc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::state::Connection;
    use crate::model::testing::FakeRpc;
    use protocol::model::{Autonomy, Bot, BotColor, BotShape, BotState, Provider};
    use protocol::wire::{BotsPause, method};

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
}
