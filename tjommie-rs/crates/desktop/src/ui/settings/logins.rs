//! Logins in Settings (Phase 3 plan T5.5; `settings/logins-section.tsx`; row 15). The selected host's saved-login
//! metadata, through the app's logins client (`ui::logins`): the vault's state with Refresh, Unlock on host and Lock
//! Logins; a new login for an agent, entered in the host's own native Logins window; and each saved login with Edit and
//! Delete on the host. Usernames and passwords never reach this screen. Another PC or connection starts again with a
//! new client, and cancels what the last one had pending.
//!
//! Left out: the cancel when the window is hidden (`visibilitychange`; a desktop window gets no such event here), and
//! the agents' pictures inside the agent menu (its rows have no picture; the button shows the chosen agent's).

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px, svg,
};
use protocol::model::Bot;
use protocol::vault::{
    DeleteStatus, EntryStatus, EntryVersion, FillStatus, FinishStatus, LoginEntry, VaultEntry,
    VaultHostAction, VaultList, VaultState, VaultStatus,
};

use super::accounts::pill;
use crate::guard::{guarded, guarded_async};
use crate::model::logins::{LOGINS_PROBLEM, LoginsError, VaultReply};
use crate::semantic::{SemanticDisabled as _, SemanticExt as _};
use crate::ui::fields::problem;
use crate::ui::logins::{self, AppLogins};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::projects::line;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

const NOTE: &str = "Saved login metadata belongs to the selected host. Enter usernames and passwords only in its \
                    dedicated native Logins window; this screen never receives them.";
/// The most saved logins a list may hold (`LoginEntry.array().max(1000)`).
const MOST: usize = 1000;

/// One host's Logins (`HostLogins`): its client, what it last read, and what's under way.
struct HostLogins {
    client: Arc<AppLogins>,
    status: Option<VaultStatus>,
    entries: Vec<LoginEntry>,
    /// The agent chosen for a new login.
    bot: Option<String>,
    busy: bool,
    note: Option<String>,
    failed: bool,
    /// Goes up with each action and cancel: an answer for an older one changes nothing.
    revision: u64,
    /// Goes up with each read: only the newest read's answer shows.
    reads: u64,
}

impl HostLogins {
    fn new(cx: &mut Context<Self>) -> Self {
        let client = logins::create(cx);
        // Leaving this host or connection cancels what was pending with it.
        cx.on_release(|logins, cx| {
            guarded("cancel the host's pending logins", || {
                let client = Arc::clone(&logins.client);
                cx.spawn(async move |_| {
                    guarded_async("cancel the host's pending logins", async move {
                        client.cancel().await
                    })
                    .await;
                })
                .detach();
            });
        })
        .detach();
        let mut logins = HostLogins {
            client,
            status: None,
            entries: Vec::new(),
            bot: None,
            busy: false,
            note: None,
            failed: false,
            revision: 0,
            reads: 0,
        };
        logins.refresh(cx);
        logins
    }

    /// `refresh`: the status and the list together, shown only if nothing newer started meanwhile.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reads += 1;
        let (expected, read, client) = (self.revision, self.reads, Arc::clone(&self.client));
        cx.spawn(async move |this, cx| {
            guarded_async("read the host's logins", async move {
                let both = futures::join!(
                    client.send(VaultHostAction::Status),
                    client.send(VaultHostAction::List(VaultList { bot_id: None }))
                );
                let current = client.current();
                let _ = this.update(cx, |logins, cx| {
                    if logins.revision != expected || logins.reads != read {
                        return;
                    }
                    match both {
                        (Ok(VaultReply::Status(status)), Ok(VaultReply::List(entries)))
                            if entries.len() <= MOST =>
                        {
                            if current {
                                logins.status = Some(status);
                                logins.entries = entries;
                                logins.failed = false;
                            }
                        }
                        _ => logins.failed = true,
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }

    /// `run`: an action on the host, which may open its trusted Logins window; the list is read again after.
    fn run(&mut self, action: VaultHostAction, cx: &mut Context<Self>) {
        self.revision += 1;
        let (expected, client) = (self.revision, Arc::clone(&self.client));
        self.busy = true;
        self.failed = false;
        let waiting = if matches!(action, VaultHostAction::Lock) {
            "Locking Logins…"
        } else {
            "Continue in Tjommie's trusted Logins window on the host."
        };
        self.note = Some(waiting.to_owned());
        let done = match action {
            VaultHostAction::Entry(_) => "Login saved on the host.",
            VaultHostAction::Delete(_) => {
                "Deletion confirmed. Any pending protected-storage cleanup is shown below."
            }
            _ => "Logins updated.",
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            guarded_async("run a logins action", async move {
                let reply = client.send(action).await;
                let current = client.current();
                let _ = this.update(cx, |logins, cx| {
                    if !current || logins.revision != expected {
                        return;
                    }
                    logins.busy = false;
                    match reply {
                        Ok(reply) => {
                            logins.note = Some(
                                if cancelled(&reply) {
                                    "Cancelled."
                                } else {
                                    done
                                }
                                .to_owned(),
                            );
                            logins.refresh(cx);
                        }
                        Err(LoginsError::Cancelled) => logins.note = Some("Cancelled.".to_owned()),
                        Err(LoginsError::Problem) => {
                            logins.failed = true;
                            logins.note = None;
                        }
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }

    /// `cancel`: stops waiting at once, and asks the host to cancel what it has open.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.busy = false;
        self.note = Some("Cancelled.".to_owned());
        cx.notify();
        let client = Arc::clone(&self.client);
        cx.spawn(async move |_, _| {
            guarded_async(
                "cancel a logins action",
                async move { client.cancel().await },
            )
            .await;
        })
        .detach();
    }
}

/// Whether the host answered that the person cancelled (`result.status === "cancelled"`).
fn cancelled(reply: &VaultReply) -> bool {
    match reply {
        VaultReply::Entry(result) => result.status == EntryStatus::Cancelled,
        VaultReply::Delete(result) => result.status == DeleteStatus::Cancelled,
        VaultReply::Fill(result) => result.status == FillStatus::Cancelled,
        VaultReply::Finish(result) => result.status == FinishStatus::Cancelled,
        VaultReply::Status(_) | VaultReply::List(_) | VaultReply::Done => false,
    }
}

fn state_word(state: VaultState) -> &'static str {
    match state {
        VaultState::Locked => "locked",
        VaultState::Unlocked => "unlocked",
        VaultState::Unavailable => "unavailable",
    }
}

/// A settings row the tests and Windows read by `id`: its title and note, and its buttons.
fn row(
    id: impl Into<SharedString>,
    title: String,
    note: String,
    control: Option<AnyElement>,
    ruled: bool,
) -> AnyElement {
    let spoken = format!("{title}. {note}");
    let row = SettingsRow::new(title).description(note);
    let row = match control {
        Some(control) => row.control(control),
        None => row,
    };
    let row = if ruled { row.ruled() } else { row };
    div()
        .semantic(id, Role::Group)
        .aria_label(spoken)
        .child(row)
        .into_any_element()
}

/// A button that runs `action` on the host.
fn act(
    logins: &Entity<HostLogins>,
    action: VaultHostAction,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let logins = logins.clone();
    move |_, cx| {
        let action = action.clone();
        logins.update(cx, |logins, cx| logins.run(action, cx));
    }
}

/// The agent for a new login (`Agent for new login`): the chosen agent's picture and name, or Choose agent.
fn agent_trigger(
    open: bool,
    chosen: Option<&Bot>,
    focus: Option<&FocusHandle>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let ring = focus.is_some_and(|focus| focus_visible(focus, window));
    let trigger = div()
        .semantic("settings.logins.agent", Role::Button)
        .aria_label(if focus.is_some() {
            "Agent for new login"
        } else {
            "Agent for new login, unavailable"
        })
        .semantic_disabled(focus.is_none())
        .aria_expanded(open)
        .relative()
        .min_w_0()
        .h(r(36.))
        .flex()
        .items_center()
        .gap(r(theme.space.s6))
        .px(r(theme.space.s14))
        .rounded_full()
        .bg(hsla(colors.surface.sunk))
        .type_style(&theme.typography.desktop.button)
        .text_color(hsla(colors.text.primary))
        .children(chosen.map(|bot| BotAvatar::new(bot.color, bot.shape, 18.)))
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(chosen.map_or_else(|| "Choose agent".to_owned(), |bot| bot.name.clone())),
        )
        .child(
            svg()
                .path("icons/i/175/chevron-down.svg")
                .size(r(14.))
                .flex_none()
                .text_color(hsla(colors.icon.base)),
        )
        .children(ring.then(|| focus_ring(theme.radius.pill, cx)));
    match focus {
        Some(focus) => trigger
            .track_focus(focus)
            .cursor_pointer()
            .into_any_element(),
        None => trigger.opacity(0.38).into_any_element(),
    }
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let now = store::state(cx);
    let key = format!(
        "settings.logins.{}.{:?}",
        now.host.clone().unwrap_or_default(),
        now.connection
    );
    let logins = window.use_keyed_state(SharedString::from(key), cx, |_, cx| HostLogins::new(cx));
    let agent = focus_handle("settings.logins.agent", window, cx);
    let cx = &*cx;
    let theme = theme(cx);
    let bots = store::state(cx).bots.clone();
    let host = logins.read(cx);
    let state = host.status.as_ref().map(|status| status.state);
    let (locked, unlocked) = (
        state == Some(VaultState::Locked),
        state == Some(VaultState::Unlocked),
    );
    let free = !host.busy;

    let title = match (&host.status, host.failed) {
        (Some(status), _) => format!("Logins {}", state_word(status.state)),
        (None, true) => "Logins unavailable".to_owned(),
        (None, false) => "Checking Logins…".to_owned(),
    };
    let note = host
        .status
        .as_ref()
        .and_then(|status| status.reason.clone())
        .unwrap_or_else(|| {
            "The host asks for confirmation before unlock, entry changes or filling a login."
                .to_owned()
        });
    let refresh = {
        let logins = logins.clone();
        move |_: &mut Window, cx: &mut App| logins.update(cx, |logins, cx| logins.refresh(cx))
    };
    let controls = div()
        .flex()
        .flex_wrap()
        .gap(r(theme.space.s8))
        .child(pill(
            "settings.logins.refresh".to_owned(),
            "Refresh",
            false,
            free,
            refresh,
        ))
        .child(pill(
            "settings.logins.unlock".to_owned(),
            "Unlock on host",
            false,
            free && locked,
            act(&logins, VaultHostAction::Unlock),
        ))
        .child(pill(
            "settings.logins.lock".to_owned(),
            "Lock Logins",
            false,
            host.client.current(),
            act(&logins, VaultHostAction::Lock),
        ));
    let pending = host
        .status
        .as_ref()
        .map_or(0, |status| status.pending_deletes);
    let state_card = super::card(cx).child(row("settings.logins.state", title, note, Some(controls.into_any_element()), false)).children(
        (pending > 0).then(|| {
            let plural = if pending == 1 { "" } else { "s" };
            row(
                "settings.logins.pending",
                format!("{pending} protected-storage deletion{plural} pending"),
                "Cleanup is retried by the host. An entry removed from this list is not proof that its OS record was deleted."
                    .to_owned(),
                None,
                true,
            )
        }),
    );

    let chosen = host
        .bot
        .as_ref()
        .and_then(|id| bots.iter().find(|bot| &bot.id == id))
        .cloned();
    let agent_menu = if free && !bots.is_empty() {
        let shown = chosen.clone();
        let menu = Menu::new("settings.logins.agent_menu", move |open, window, cx| {
            agent_trigger(open, shown.as_ref(), Some(&agent), window, cx)
        })
        .anchor(gpui::Anchor::TopRight)
        .offset(px(6.));
        bots.iter()
            .fold(menu, |menu, bot| {
                let (logins, id) = (logins.clone(), bot.id.clone());
                let checked = host.bot.as_deref() == Some(bot.id.as_str());
                menu.item(
                    MenuItem::new(
                        format!("settings.logins.agent.{}", bot.id),
                        bot.name.clone(),
                        move |_, cx| {
                            logins.update(cx, |logins, cx| {
                                logins.bot = Some(id.clone());
                                cx.notify();
                            });
                        },
                    )
                    .checked(checked),
                )
            })
            .into_any_element()
    } else {
        agent_trigger(false, chosen.as_ref(), None, window, cx)
    };
    let new_login = match host.bot.clone() {
        Some(bot_id) => {
            let action = VaultHostAction::Entry(VaultEntry {
                bot_id,
                id: None,
                revision: None,
            });
            pill(
                "settings.logins.new".to_owned(),
                "New login on host",
                true,
                free && unlocked,
                act(&logins, action),
            )
        }
        None => pill(
            "settings.logins.new".to_owned(),
            "New login on host",
            true,
            false,
            |_, _| {},
        ),
    };
    let new_card = super::card(cx).child(row(
        "settings.logins.new_row",
        "New saved login".to_owned(),
        "Choose its agent, then enter its name, exact HTTPS origin and secret in the host's native window.".to_owned(),
        Some(div().flex().flex_wrap().gap(r(theme.space.s8)).child(agent_menu).child(new_login).into_any_element()),
        false,
    ));

    let rows: Vec<AnyElement> = if host.entries.is_empty() {
        let none = "Refresh after the host has saved an entry.".to_owned();
        vec![row(
            "settings.logins.none",
            "No saved login metadata".to_owned(),
            none,
            None,
            false,
        )]
    } else {
        host.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let agent = bots
                    .iter()
                    .find(|bot| bot.id == entry.bot_id)
                    .map_or("Missing agent", |bot| bot.name.as_str());
                let edit = VaultHostAction::Entry(VaultEntry {
                    bot_id: entry.bot_id.clone(),
                    id: Some(entry.id.clone()),
                    revision: Some(entry.revision),
                });
                let delete = VaultHostAction::Delete(EntryVersion {
                    id: entry.id.clone(),
                    revision: entry.revision,
                });
                let controls = div()
                    .flex()
                    .flex_wrap()
                    .gap(r(theme.space.s8))
                    .child(pill(
                        format!("settings.logins.{}.edit", entry.id),
                        "Edit on host",
                        false,
                        free && unlocked,
                        act(&logins, edit),
                    ))
                    .child(pill(
                        format!("settings.logins.{}.delete", entry.id),
                        "Delete on host",
                        false,
                        free,
                        act(&logins, delete),
                    ));
                let note = format!("{} · {agent}", entry.origin.as_str());
                row(
                    format!("settings.logins.{}", entry.id),
                    entry.name.clone(),
                    note,
                    Some(controls.into_any_element()),
                    index > 0,
                )
            })
            .collect()
    };

    let cancel = {
        let logins = logins.clone();
        move |_: &mut Window, cx: &mut App| logins.update(cx, |logins, cx| logins.cancel(cx))
    };
    super::frame("logins", "Logins", NOTE, cx)
        .child(state_card)
        .child(new_card)
        .child(super::card(cx).children(rows))
        .children(host.busy.then(|| {
            div().mt(r(theme.space.s12)).child(pill(
                "settings.logins.cancel".to_owned(),
                "Cancel host interaction",
                false,
                true,
                cancel,
            ))
        }))
        .children(host.note.clone().map(|note| {
            div()
                .mt(r(theme.space.s12))
                .child(line("settings.logins.note", Role::Status, note, cx))
        }))
        .children(
            host.failed
                .then(|| problem("settings.logins.problem", LOGINS_PROBLEM, cx)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use protocol::model::BotState;
    use protocol::wire::method;
    use serde_json::{Value, json};

    use crate::model::logins::LOGINS_PROBLEM;
    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, bot, click, draw, node, state, text};

    const ENTRY_ID: &str = "41f8bac7-358c-48cf-9f48-a3a358804da6";

    /// logins-section.test.tsx's saved login: metadata only.
    fn entry() -> Value {
        json!({ "id": ENTRY_ID, "name": "Synthetic login", "botId": "one", "origin": "https://example.com", "revision": 3,
                "createdAt": "2026-11-03T00:00:00Z", "updatedAt": "2026-11-03T00:00:00Z" })
    }

    /// The host's vault, unlocked, with the one login; an entry is saved as asked.
    fn vault() -> FakeRpc {
        let unlocked = json!({ "state": "unlocked", "reason": null, "pendingDeletes": 0 });
        FakeRpc::new()
            .on::<method::VaultStatus>(move |_| {
                Ok(serde_json::from_value(unlocked.clone()).unwrap())
            })
            .on::<method::VaultList>(|_| Ok(vec![serde_json::from_value(entry()).unwrap()]))
            .on::<method::VaultEntry>(|_| {
                Ok(serde_json::from_value(json!({ "status": "saved", "entry": entry() })).unwrap())
            })
    }

    /// On a selected PC, with agent `one`, Writer.
    fn on_host() -> AppState {
        AppState {
            host: Some("selected-host".into()),
            ..state(vec![bot("one", "Writer", BotState::Idle)], vec![], vec![])
        }
    }

    fn calls(rpc: &FakeRpc, method: &str) -> Vec<Value> {
        rpc.calls()
            .into_iter()
            .filter(|(name, _)| *name == method)
            .map(|(_, params)| params)
            .collect()
    }

    /// logins-section.test.tsx: "shows metadata and invokes native host entry without ordinary secret inputs".
    #[gpui::test]
    fn shows_metadata_and_asks_the_host_for_its_entry_window(cx: &mut TestAppContext) {
        let (cx, calls_made) = app(cx, Route::Settings(Section::Logins), on_host(), vault());
        draw(cx);
        assert!(text("settings.logins.state").starts_with("Logins unlocked."));
        assert_eq!(
            text("settings.logins.41f8bac7-358c-48cf-9f48-a3a358804da6"),
            "Synthetic login. https://example.com · Writer"
        );
        assert!(
            crate::test_driver::nodes()
                .iter()
                .all(|node| node.role != "TextInput"),
            "nothing secret is typed here"
        );
        click(cx, "settings.logins.agent");
        click(cx, "settings.logins.agent.one");
        click(cx, "settings.logins.new");
        draw(cx);
        assert_eq!(
            calls(&calls_made, "vault.entry"),
            [json!({ "botId": "one" })]
        );
        assert_eq!(text("settings.logins.note"), "Login saved on the host.");
        click(
            cx,
            "settings.logins.41f8bac7-358c-48cf-9f48-a3a358804da6.edit",
        );
        draw(cx);
        let edits = calls(&calls_made, "vault.entry");
        assert_eq!(
            edits.get(1),
            Some(&json!({ "botId": "one", "id": ENTRY_ID, "revision": 3 }))
        );
    }

    /// When the host's vault can't be asked (no vault host answers), it says so and why, and guesses no state.
    #[gpui::test]
    fn says_logins_are_unavailable_when_the_host_cannot_be_asked(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Logins),
            on_host(),
            FakeRpc::new(),
        );
        draw(cx);
        assert!(text("settings.logins.state").starts_with("Logins unavailable."));
        assert_eq!(text("settings.logins.problem"), LOGINS_PROBLEM);
        assert!(node("settings.logins.unlock").is_some());
    }
}
