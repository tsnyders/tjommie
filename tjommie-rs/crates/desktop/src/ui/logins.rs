//! The app's logins client (T1.4's `LoginsClient`, wired by the Screens lane in M5): one for the selected PC and the
//! connection as they are, over the daemon's `vault.*` calls, made again when either changes. Settings, Logins keeps its
//! own; the browser panel's logins menu shares one here (`BrowserData::on_logins`), and what it answers is kept where the
//! panel reads it (`BrowserData::logins`).
//!
//! This PC's own vault is reached directly, through the gate's local controller (`hosts::Bridge`, the TypeScript's
//! `desktop().vault`); another PC's goes through that PC's daemon.

use std::sync::Arc;

use gpui::{App, Global, Window};
use protocol::vault::{VaultHostAction, VaultStatus};

use crate::guard::guarded_async;
use crate::hosts::Bridge;
use crate::model::logins::{
    LiveState as _, LoginsClient, LoginsError, VaultReply, create_logins_client,
};
use crate::ui::browser::BrowserData;
use crate::ui::store::{self, StoreLive, StoreRpc};

pub type AppLogins = LoginsClient<StoreLive, Bridge, StoreRpc>;

/// `createLoginsClient()`: for the selected PC and the connection now.
pub fn create(cx: &App) -> Arc<AppLogins> {
    let live = store::live_state(cx);
    let daemon = live
        .daemon_generation()
        .map(|generation| (generation, store::rpc(cx)));
    Arc::new(create_logins_client(live, Bridge::of(cx), daemon))
}

/// The browser panel's client, kept while its PC and connection hold, and the vault's latest status: a bot's view made
/// after the status came still gets it.
#[derive(Default)]
struct Panel {
    client: Option<Arc<AppLogins>>,
    status: Option<VaultStatus>,
}

impl Global for Panel {}

fn panel_client(cx: &mut App) -> Arc<AppLogins> {
    let kept = cx
        .try_global::<Panel>()
        .and_then(|panel| panel.client.clone())
        .filter(|client| client.current());
    if let Some(client) = kept {
        return client;
    }
    let client = create(cx);
    cx.default_global::<Panel>().client = Some(Arc::clone(&client));
    client
}

/// `BrowserData::on_logins`: the panel's action, sent through the shared client.
pub fn browser_logins(action: VaultHostAction, _: &mut Window, cx: &mut App) {
    send(action, cx);
}

fn send(action: VaultHostAction, cx: &mut App) {
    let client = panel_client(cx);
    let bot = match &action {
        VaultHostAction::List(list) => list.bot_id.clone(),
        _ => None,
    };
    // An unlock or a fill changes the vault: its status is read again.
    let changes = matches!(action, VaultHostAction::Unlock | VaultHostAction::Fill(_));
    cx.spawn(async move |cx| {
        guarded_async("send the browser's logins action", async move {
            let reply = client.send(action).await;
            cx.update(|cx| {
                let done = reply.is_ok();
                keep(reply, bot.as_deref(), cx);
                if done && changes {
                    send(VaultHostAction::Status, cx);
                }
            });
        })
        .await;
    })
    .detach();
}

/// Keeps an answer where the panel reads it: the vault's status in every bot's view, a list in its bot's. A failure
/// marks the view it was for (all of them for the status); a cancel changes nothing.
fn keep(reply: Result<VaultReply, LoginsError>, bot: Option<&str>, cx: &mut App) {
    let status = cx.default_global::<Panel>().status.clone();
    let data = cx.default_global::<BrowserData>();
    match (reply, bot) {
        (Ok(VaultReply::Status(next)), _) => {
            for view in data.logins.values_mut() {
                view.status = Some(next.clone());
                view.failed = false;
            }
            cx.default_global::<Panel>().status = Some(next);
        }
        (Ok(VaultReply::List(entries)), Some(bot)) => {
            let view = data.logins.entry(bot.to_owned()).or_default();
            view.entries = entries;
            view.status = view.status.clone().or(status);
            view.failed = false;
        }
        (Err(LoginsError::Problem), Some(bot)) => {
            data.logins.entry(bot.to_owned()).or_default().failed = true
        }
        (Err(LoginsError::Problem), None) => {
            data.logins.values_mut().for_each(|view| view.failed = true)
        }
        _ => {}
    }
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use protocol::vault::{VaultHostAction, VaultList, VaultState};
    use protocol::wire::method;
    use serde_json::json;

    use crate::model::testing::FakeRpc;
    use crate::ui::browser::BrowserData;
    use crate::ui::shell::route::Route;
    use crate::ui::testing::{app, draw, state};

    /// The panel's logins menu reaches the vault through the app's client, and its answers are kept for its bot.
    #[gpui::test]
    fn the_panels_answers_are_kept_for_its_bot(cx: &mut TestAppContext) {
        let saved = json!({ "id": "5f0c2f8e-9a43-4f7e-9b0a-3c1d2e4f5a6b", "name": "Sample login", "botId": "one", "origin": "https://example.com", "revision": 1,
                            "createdAt": "2026-11-03T00:00:00Z", "updatedAt": "2026-11-03T00:00:00Z" });
        let rpc = FakeRpc::new()
            .on::<method::VaultStatus>(|_| {
                Ok(serde_json::from_value(
                    json!({ "state": "locked", "reason": null, "pendingDeletes": 0 }),
                )
                .unwrap())
            })
            .on::<method::VaultList>(move |_| {
                Ok(vec![serde_json::from_value(saved.clone()).unwrap()])
            });
        let (cx, calls_made) = app(cx, Route::Agents, state(vec![], vec![], vec![]), rpc);
        cx.update(|window, cx| {
            super::browser_logins(VaultHostAction::Status, window, cx);
            super::browser_logins(
                VaultHostAction::List(VaultList {
                    bot_id: Some("one".into()),
                }),
                window,
                cx,
            );
        });
        draw(cx);
        let view = cx.update(|_, cx| {
            cx.try_global::<BrowserData>()
                .and_then(|data| data.logins.get("one").cloned())
        });
        let view = view.unwrap();
        let asked = calls_made.calls();
        assert_eq!(
            view.status.clone().map(|status| status.state),
            Some(VaultState::Locked),
            "{view:?} after {asked:?}"
        );
        assert_eq!(view.entries.len(), 1);
        assert!(!view.failed);
        assert_eq!(
            calls_made
                .calls()
                .iter()
                .filter(|(name, _)| name.starts_with("vault."))
                .count(),
            2
        );
    }
}
