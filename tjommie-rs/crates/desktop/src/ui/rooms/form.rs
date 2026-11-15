//! New room and Edit room (`room/room-form.tsx`), in a dialog: the name and the agents in the room. Save stays off
//! until there are both (and, for an edit, a change); the dialog stays open until it works, saying why when it
//! doesn't. A new room opens once it's made.

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Entity, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use protocol::model::{Bot, Room, RoomInput, RoomPatch};
use protocol::wire::{RoomsUpdate, method};

use crate::guard::{guarded, guarded_async};
use crate::model::bot_form::{can_message_summary, same_set};
use crate::model::labels::error_text;
use crate::ui::bots::rows;
use crate::ui::fields::{TextField, label, problem};
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{r, theme};

/// What saving an edit sends (`roomPatch`): only what was changed in the form, measured from the room as the form
/// opened, so a rename or a member change made elsewhere meanwhile is never put back. `bot_ids` are the form's members
/// that still exist; a member deleted since the form opened left with its agent, so it isn't a change.
pub fn room_patch(opened: &Room, name: &str, bot_ids: &[String], bots: &[Bot]) -> RoomPatch {
    let mut patch = RoomPatch::default();
    if name.trim() != opened.name {
        patch.name = Some(name.trim().to_owned());
    }
    let before: Vec<String> = opened
        .bot_ids
        .iter()
        .filter(|id| bots.iter().any(|bot| bot.id == **id))
        .cloned()
        .collect();
    if !same_set(bot_ids, &before) {
        patch.bot_ids = Some(bot_ids.to_vec());
    }
    patch
}

struct RoomForm {
    /// The room as the form opened; the edit is measured from it. `None`: a new room.
    opened: Option<Room>,
    name: Entity<InputState>,
    bot_ids: Vec<String>,
    busy: bool,
    problem: Option<SharedString>,
    /// The window the dialog is in, for what a save does once it ends.
    window: AnyWindowHandle,
    _enter: Option<Subscription>,
}

/// Opens the room form: a new room, or `room` to edit.
pub fn open_room_form(room: Option<Room>, window: &mut Window, cx: &mut App) {
    let name = cx.new(|cx| {
        let state = InputState::new(window, cx);
        match &room {
            Some(room) => state.default_value(room.name.clone()),
            None => state,
        }
    });
    let bot_ids = room
        .as_ref()
        .map(|room| room.bot_ids.clone())
        .unwrap_or_default();
    let form = cx.new(|_| RoomForm {
        opened: room,
        name: name.clone(),
        bot_ids,
        busy: false,
        problem: None,
        window: window.window_handle(),
        _enter: None,
    });
    // Enter in the name saves, as the form's submit did.
    let saving = form.clone();
    let enter = cx.subscribe(&name, move |_, event: &InputEvent, cx| {
        guarded("save a room from Enter", || {
            if matches!(event, InputEvent::PressEnter { .. }) {
                save(&saving, cx);
            }
        });
    });
    form.update(cx, |form, _| form._enter = Some(enter));
    open_dialog(window, cx, move |dialog, window, cx| {
        let body = body(&form, window, cx);
        let heading = if form.read(cx).opened.is_some() {
            "Edit room"
        } else {
            "New room"
        };
        // room-form.tsx's 420 px, as rems.
        dialog
            .w(window.rem_size() * 26.25)
            .title(title(heading, cx))
            // Enter in the name saves (the form's submit): it never closes the form through the dialog's Confirm.
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(body)
    });
    // The name has the focus (`initialFocus`), after the dialog takes it.
    window.defer(cx, move |window, cx| {
        guarded("focus the room's name", || {
            name.update(cx, |name, cx| name.focus(window, cx))
        });
    });
}

fn chosen(form: &RoomForm, bots: &[Bot]) -> Vec<String> {
    form.bot_ids
        .iter()
        .filter(|id| bots.iter().any(|bot| bot.id == **id))
        .cloned()
        .collect()
}

fn can_save(form: &RoomForm, cx: &App) -> bool {
    let bots = &store::state(cx).bots;
    let name = form.name.read(cx).value();
    let chosen = chosen(form, bots);
    let changed = match &form.opened {
        Some(opened) => room_patch(opened, &name, &chosen, bots) != RoomPatch::default(),
        None => true,
    };
    !form.busy && !name.trim().is_empty() && !chosen.is_empty() && changed
}

fn body(form: &Entity<RoomForm>, window: &mut Window, cx: &mut App) -> AnyElement {
    let bots = store::state(cx).bots.clone();
    let state = form.read(cx);
    let ids = chosen(state, &bots);
    let editing = state.opened.is_some();
    let ready = can_save(state, cx);
    let name = state.name.clone();
    let failed = state.problem.clone();
    let members = {
        let form = form.clone();
        rows::can_message(
            "room_form.members",
            "In this room",
            can_message_summary(&ids, &bots),
            None,
            &ids,
            &bots,
            true,
            move |next, _, cx| {
                form.update(cx, |form, cx| {
                    form.bot_ids = next;
                    cx.notify();
                });
            },
            "The agents you mention answer here, and @everyone asks them all. Being in a room doesn't change who \
             an agent can message.",
            window,
            cx,
        )
    };
    let theme = theme(cx);
    let saving = form.clone();
    let mut save_pill = Pill::new(
        "room_form.save",
        if editing { "Save" } else { "Create room" },
    )
    .variant(PillVariant::Primary);
    if ready {
        save_pill = save_pill.on_press(move |_, cx| save(&saving, cx));
    }
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .mt(r(theme.space.s16))
                .flex()
                .flex_col()
                .child(label("Name", cx))
                .child(TextField::new("room_form.name", "Name", &name)),
        )
        .child(div().mt(r(theme.space.s16)).child(members))
        .children(failed.map(|text| problem("room_form.problem", text, cx)))
        .child(
            div()
                .mt(r(theme.space.s20))
                .flex()
                .justify_end()
                .gap(r(theme.space.s8))
                .child(
                    Pill::new("room_form.cancel", "Cancel")
                        .on_press(|window, cx| window.close_dialog(cx)),
                )
                // Off until it can save (`.off`, 38%).
                .child(
                    div()
                        .opacity(if ready { 1.0 } else { 0.38 })
                        .child(save_pill),
                ),
        )
        .into_any_element()
}

fn save(form: &Entity<RoomForm>, cx: &mut App) {
    if !can_save(form.read(cx), cx) {
        return;
    }
    let bots = store::state(cx).bots.clone();
    let (opened, name, ids) = {
        let state = form.read(cx);
        (
            state.opened.clone(),
            state.name.read(cx).value().trim().to_owned(),
            chosen(state, &bots),
        )
    };
    form.update(cx, |form, cx| {
        form.busy = true;
        form.problem = None;
        cx.notify();
    });
    let form = form.clone();
    match opened {
        Some(opened) => {
            let patch = room_patch(&opened, &name, &ids, &bots);
            let call = store::request::<method::RoomsUpdate>(
                cx,
                RoomsUpdate {
                    id: opened.id.clone(),
                    patch,
                },
            );
            cx.spawn(async move |cx| {
                guarded_async("save a room", async move {
                    let saved = call.await;
                    cx.update(|cx| finish(&form, saved.map(|_| None), cx));
                })
                .await;
            })
            .detach();
        }
        None => {
            let call = store::request::<method::RoomsCreate>(cx, RoomInput { name, bot_ids: ids });
            cx.spawn(async move |cx| {
                guarded_async("create a room", async move {
                    let made = call.await;
                    cx.update(|cx| finish(&form, made.map(|room| Some(room.id)), cx));
                })
                .await;
            })
            .detach();
        }
    }
}

/// After the save: close the dialog (and open a new room), or say why it failed. A save that ends after the dialog
/// was closed does neither.
fn finish(
    form: &Entity<RoomForm>,
    saved: Result<Option<String>, crate::model::connection::RequestError>,
    cx: &mut App,
) {
    let window = form.read(cx).window;
    let _ = window.update(cx, |_, window, cx| {
        if !window.has_active_dialog(cx) {
            return;
        }
        match saved {
            Ok(made) => {
                window.close_dialog(cx);
                if let Some(id) = made {
                    cx.navigate(Route::Room(id));
                }
            }
            Err(error) => form.update(cx, |form, cx| {
                form.busy = false;
                form.problem = Some(error_text(&error).into());
                cx.notify();
            }),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::FakeRpc;
    use crate::ui::testing::{app, bot, click, draw, node, state};
    use protocol::model::BotState;

    /// Growth as the form opened: Sales (b1) and Marketing (b2). Chief (b3) is the other agent.
    fn growth() -> Room {
        Room {
            id: "r1".into(),
            name: "Growth".into(),
            bot_ids: vec!["b1".into(), "b2".into()],
            everyone: false,
            created_at: Some("2026-10-25T09:00:00.000Z".into()),
        }
    }

    fn bots(ids: &[&str]) -> Vec<Bot> {
        ids.iter().map(|id| bot(id, id, BotState::Idle)).collect()
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    /// `room-form.test.tsx`: "sends only what was changed in the form, so a change made elsewhere meanwhile isn't put
    /// back".
    #[test]
    fn saving_an_edit_sends_only_what_changed_in_the_form() {
        let all = bots(&["b1", "b2", "b3"]);
        assert_eq!(
            room_patch(&growth(), "Growth", &ids(&["b1"]), &all),
            RoomPatch {
                name: None,
                bot_ids: Some(ids(&["b1"]))
            }
        );
        assert_eq!(
            room_patch(&growth(), " Growth team ", &ids(&["b1", "b2"]), &all),
            RoomPatch {
                name: Some("Growth team".into()),
                bot_ids: None
            }
        );
        assert_eq!(
            room_patch(&growth(), "Growth", &ids(&["b2", "b1"]), &all),
            RoomPatch::default()
        );
    }

    /// `room-form.test.tsx`: "doesn't count a member deleted since the form opened as a change".
    #[test]
    fn a_member_deleted_since_the_form_opened_is_not_a_change() {
        let left = bots(&["b1", "b3"]);
        assert_eq!(
            room_patch(&growth(), "Growth", &ids(&["b1"]), &left),
            RoomPatch::default()
        );
        assert_eq!(
            room_patch(&growth(), "Growth", &ids(&["b1", "b3"]), &left),
            RoomPatch {
                name: None,
                bot_ids: Some(ids(&["b1", "b3"]))
            }
        );
    }

    /// The form's submit: Enter in the name saves (and isn't the dialog's Confirm, which would close it unsaved).
    #[gpui::test]
    fn enter_in_the_name_saves_the_room(cx: &mut gpui::TestAppContext) {
        let rpc = FakeRpc::new().on::<method::RoomsCreate>(|_| {
            Ok(Room {
                id: "r9".into(),
                name: "Launch crew".into(),
                bot_ids: vec!["b1".into()],
                everyone: false,
                created_at: Some("2026-11-15T09:00:00.000Z".into()),
            })
        });
        let (cx, calls) = app(
            cx,
            Route::Agents,
            state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]),
            rpc,
        );
        click(cx, "agents.new_room");
        click(cx, "room_form.members.b1");
        click(cx, "room_form.name");
        cx.simulate_input("Launch crew");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            calls
                .calls()
                .iter()
                .any(|(name, _)| *name == "rooms.create"),
            "{:?}",
            calls.calls()
        );
        assert_eq!(
            cx.update(|_, cx| crate::ui::shell::route::current(cx)),
            Route::Room("r9".into())
        );
    }

    #[gpui::test]
    fn new_room_makes_the_room_and_opens_it(cx: &mut gpui::TestAppContext) {
        let rpc = FakeRpc::new().on::<method::RoomsCreate>(|params| {
            Ok(Room {
                id: "r9".into(),
                name: params["name"].as_str().unwrap_or_default().to_owned(),
                bot_ids: vec!["b1".into()],
                everyone: false,
                created_at: Some("2026-11-15T09:00:00.000Z".into()),
            })
        });
        let (cx, calls) = app(
            cx,
            Route::Agents,
            state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]),
            rpc,
        );
        click(cx, "agents.new_room");
        assert!(node("room_form.name").is_some(), "the form");
        assert!(
            node("room_form.name").unwrap().focused,
            "the name has the focus"
        );
        cx.simulate_input("Launch crew");
        draw(cx);
        click(cx, "room_form.members.b1");
        click(cx, "room_form.save");
        let made: Vec<_> = calls
            .calls()
            .into_iter()
            .filter(|(name, _)| *name == "rooms.create")
            .collect();
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].1["name"], "Launch crew");
        assert_eq!(made[0].1["botIds"][0], "b1");
        assert_eq!(
            cx.update(|_, cx| crate::ui::shell::route::current(cx)),
            Route::Room("r9".into())
        );
        assert!(
            !cx.update(|window, cx| window.has_active_dialog(cx)),
            "closed"
        );
    }
}
