//! Agents in Settings (`app/settings/page.tsx`'s `BotsSection`): how many agent-to-agent requests one chain may make.

use gpui::{AnyElement, App, IntoElement, ParentElement as _, SharedString, Window};
use protocol::model::SettingsPatch;
use protocol::wire::method;

use crate::guard::guarded_async;
use crate::model::labels::error_text;
use crate::ui::fields::{Stepper, problem};
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::store;

const LIMIT: &str = "Requests between agents per chain";

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    // Why the last change failed, kept while the section shows.
    let failed = window.use_keyed_state("settings.bots.problem", cx, |_, _| None::<SharedString>);
    let limit = store::state(cx)
        .settings
        .as_ref()
        .map(|settings| settings.chain_limit);
    let said = failed.read(cx).clone();
    let control = limit.map(|limit| {
        let failed = failed.clone();
        Stepper::new("settings.bots.chain_limit", LIMIT, limit, 1, 20).on_choose(
            move |next, _, cx| {
                let call = store::request::<method::SettingsSet>(
                    cx,
                    SettingsPatch {
                        chain_limit: Some(next),
                        relay_url: None,
                    },
                );
                let failed = failed.clone();
                cx.spawn(async move |cx| {
                    guarded_async("change the chain limit", async move {
                        let outcome = call.await;
                        cx.update(|cx| {
                            failed.update(cx, |failed, cx| {
                                *failed = outcome.err().map(|error| error_text(&error).into());
                                cx.notify();
                            })
                        });
                    })
                    .await;
                })
                .detach();
            },
        )
    });
    let row = SettingsRow::new(LIMIT).description(
        "A chain starts with each message you send and each routine run. When a chain reaches this many \
         agent-to-agent requests, the next request stops and the agent asks you instead.",
    );
    let row = match control {
        Some(control) => row.control(control),
        None => row,
    };
    super::frame("bots", "Agents", "", cx)
        .child(super::heading("Agents asking each other", cx))
        .child(super::card(cx).child(row))
        .children(said.map(|text| problem("settings.bots.problem", text, cx)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, state, text};
    use protocol::model::Settings;
    use protocol::wire::method;

    fn with_limit(limit: u32) -> AppState {
        AppState {
            settings: Some(Settings {
                chain_limit: limit,
                relay_url: None,
            }),
            ..state(vec![], vec![], vec![])
        }
    }

    #[gpui::test]
    fn the_stepper_sets_the_chain_limit_and_says_why_when_it_cant(cx: &mut gpui::TestAppContext) {
        let rpc = FakeRpc::new().on::<method::SettingsSet>(|_| {
            Err(client::RpcError::Daemon {
                code: protocol::ErrorCode::Internal,
                message: "The limit can't change now.".into(),
            })
        });
        let (cx, calls) = app(cx, Route::Settings(Section::Bots), with_limit(5), rpc);
        assert_eq!(text("settings.bots.chain_limit.value"), "5");
        click(cx, "settings.bots.chain_limit.more");
        draw(cx);
        let set: Vec<_> = calls
            .calls()
            .into_iter()
            .filter(|(name, _)| *name == "settings.set")
            .collect();
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].1["chainLimit"], 6);
        assert!(text("settings.bots.problem").contains("The limit can't change now."));
    }
}
