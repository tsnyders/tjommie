//! The Logins windows (`vault-entry.test.ts` where the GPUI windows have the behaviour): an entry is typed in a window of
//! its own and handed over, refused fields keep it open, a confirmation answers yes or no, one window at a time, and the
//! vault cancelling closes it.

use desktop_host::vault::login::{Cancel, Confirmation, EntryWindow};
use gpui::TestAppContext;
use protocol::vault::{FinishMode, LoginEntry, LoginOrigin};

use super::*;
use crate::theme::Theme;

fn start(cx: &mut TestAppContext) -> Arc<EntryWindows> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(crate::keymap::bindings());
        cx.set_global(Theme::aegis().unwrap());
        crate::notices::init(cx);
        let windows = EntryWindows::start(cx);
        // The test platform's windows have no HWND for Windows to hide; screen capture is out of the picture.
        cx.set_global(HideFromCapture(|_| true));
        windows
    })
}

/// The vault's side of a request, run as the vault would run it: off the window, awaiting its answer.
fn ask<T: 'static>(
    cx: &mut TestAppContext,
    request: impl Future<Output = T> + 'static,
) -> gpui::Task<T> {
    let executor = cx.foreground_executor().clone();
    // unguarded: a test's task, not a UI handler
    executor.spawn(request)
}

/// The window open now, and its view.
fn shown(cx: &mut TestAppContext) -> Option<(AnyWindowHandle, Entity<EntryView>)> {
    cx.update(|cx| cx.global::<Open>().0.clone())
}

/// Types `values` into the open entry window's fields.
fn type_fields(values: [&str; 4], cx: &mut TestAppContext) {
    let (handle, view) = shown(cx).expect("a Logins window");
    cx.update(|cx| {
        handle
            .update(cx, |_, window, cx| {
                let fields = view.read(cx).fields.as_ref().map(|fields| {
                    [
                        fields.name.clone(),
                        fields.origin.clone(),
                        fields.username.clone(),
                        fields.password.clone(),
                    ]
                });
                for (input, value) in fields.expect("an entry window").into_iter().zip(values) {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
            })
            .unwrap();
    });
}

/// Presses the window's confirm button (Save login, Fill login, …).
fn confirm(cx: &mut TestAppContext) {
    let (handle, view) = shown(cx).expect("a Logins window");
    cx.update(|cx| {
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.submit(window, cx))
            })
            .unwrap();
    });
    cx.run_until_parked();
}

fn entry() -> LoginEntry {
    LoginEntry {
        id: "e1".into(),
        name: "Docs".into(),
        origin: LoginOrigin::new("https://docs.rs").unwrap(),
        bot_id: "b1".into(),
        revision: 1,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

#[gpui::test]
async fn an_entry_is_typed_in_its_own_window_and_handed_over(cx: &mut TestAppContext) {
    let windows = start(cx);
    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(
        cx,
        async move { asking.entry("b1".into(), None, cancel).await },
    );
    cx.run_until_parked();
    assert!(shown(cx).is_some(), "the entry window opened");
    assert!(
        on_screen("logins.password", cx) && !on_screen("logins.capture", cx),
        "hidden from capture, it takes a login and says nothing of capture"
    );
    type_fields(["  Docs  ", "https://docs.rs", "forge", "secret-1"], cx);
    let (_, view) = shown(cx).unwrap();
    confirm(cx);
    let input = answer.await.expect("answered").expect("a login");
    assert_eq!(input.name, "Docs", "the name is trimmed");
    assert_eq!(input.origin.as_str(), "https://docs.rs");
    assert_eq!(
        (input.secret.username(), input.secret.password()),
        ("forge", "secret-1")
    );
    let emptied = cx.update(|cx| {
        let fields = view.read(cx).fields.as_ref().unwrap();
        (
            fields.username.read(cx).value().is_empty(),
            fields.password.read(cx).value().is_empty(),
        )
    });
    assert_eq!(
        emptied,
        (true, true),
        "the typed secret leaves the fields at once"
    );
    assert!(shown(cx).is_none(), "and the window closed");
}

#[gpui::test]
async fn refused_fields_keep_the_window_open_and_say_so(cx: &mut TestAppContext) {
    let windows = start(cx);
    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(
        cx,
        async move { asking.entry("b1".into(), None, cancel).await },
    );
    cx.run_until_parked();
    type_fields(["Docs", "http://docs.rs", "forge", "secret-1"], cx);
    confirm(cx);
    let (_, view) = shown(cx).expect("still open");
    assert_eq!(cx.update(|cx| view.read(cx).problem), Some(REFUSED));
    // The vault locking closes it as a cancel.
    windows.close();
    cx.run_until_parked();
    assert_eq!(answer.await.map(|input| input.is_none()), Ok(true));
    assert!(shown(cx).is_none());
}

#[gpui::test]
async fn a_confirmation_answers_yes_and_one_window_at_a_time(cx: &mut TestAppContext) {
    let windows = start(cx);
    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let first = ask(cx, async move {
        asking.confirm(Confirmation::Unlock, cancel).await
    });
    cx.run_until_parked();
    let (_fire_second, cancel) = Cancel::pair();
    let asking = windows.clone();
    let second = ask(cx, async move {
        asking
            .confirm(
                Confirmation::Finish {
                    mode: FinishMode::Clear,
                },
                cancel,
            )
            .await
    });
    cx.run_until_parked();
    assert_eq!(
        second.await,
        Err(LoginError::Failed),
        "one window at a time"
    );
    confirm(cx);
    assert_eq!(first.await, Ok(true));
}

#[gpui::test]
async fn the_vault_cancelling_closes_the_window(cx: &mut TestAppContext) {
    let windows = start(cx);
    let (fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(cx, async move {
        asking
            .confirm(Confirmation::Delete { entry: entry() }, cancel)
            .await
    });
    cx.run_until_parked();
    assert!(shown(cx).is_some());
    let _ = fire.send(true);
    cx.run_until_parked();
    assert_eq!(answer.await, Ok(false));
    assert!(shown(cx).is_none(), "closed");
}

/// Draws the open window, as Windows does once it shows: keys reach what its last frame holds.
fn draw(cx: &mut TestAppContext) -> AnyWindowHandle {
    let (handle, _) = shown(cx).expect("a Logins window");
    cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    handle
}

/// Whether the open window draws `id`.
fn on_screen(id: &'static str, cx: &mut TestAppContext) -> bool {
    let window = draw(cx);
    gpui::VisualTestContext::from_window(window, cx)
        .debug_bounds(id)
        .is_some()
}

/// Closing the window with its X is a cancel, and frees it at once (review F4): the next request opens straight away,
/// not when the two-minute timer ends, and nothing keeps the fields typed into it.
#[gpui::test]
async fn closing_the_window_frees_it_at_once(cx: &mut TestAppContext) {
    let windows = start(cx);
    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(
        cx,
        async move { asking.entry("b1".into(), None, cancel).await },
    );
    cx.run_until_parked();
    type_fields(["Docs", "https://docs.rs", "forge", "secret-1"], cx);
    let (window, view) = shown(cx)
        .map(|(window, view)| (window, view.downgrade()))
        .unwrap();
    // Windows asks the window first (its X), then closes it.
    let mut closing = gpui::VisualTestContext::from_window(window, cx);
    assert!(closing.simulate_close(), "it may close");
    assert!(shown(cx).is_none(), "and is off the record at once");
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        answer.await.map(|input| input.is_none()),
        Ok(true),
        "a cancel"
    );
    assert!(view.upgrade().is_none(), "nothing keeps its fields");

    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let next = ask(cx, async move {
        asking.confirm(Confirmation::Unlock, cancel).await
    });
    cx.run_until_parked();
    assert!(shown(cx).is_some(), "the next request opens at once");
    confirm(cx);
    assert_eq!(next.await, Ok(true));
}

/// When Windows won't keep the window out of screen capture (review F3), an entry window takes no login: no fields, no
/// Save, only Cancel, and it says why. A confirmation shows no secret: it still asks, and says capture can see it.
#[gpui::test]
async fn a_window_screen_capture_can_see_takes_no_login(cx: &mut TestAppContext) {
    let windows = start(cx);
    cx.update(|cx| cx.set_global(HideFromCapture(|_| false)));
    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(
        cx,
        async move { asking.entry("b1".into(), None, cancel).await },
    );
    cx.run_until_parked();
    assert!(on_screen("logins.capture", cx), "it says why");
    for id in [
        "logins.name",
        "logins.origin",
        "logins.username",
        "logins.password",
        "logins.confirm",
    ] {
        assert!(!on_screen(id, cx), "{id} is not drawn");
    }
    assert!(on_screen("logins.cancel", cx), "Cancel is");
    let window = draw(cx);
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(answer.await.map(|input| input.is_none()), Ok(true));

    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(cx, async move {
        asking.confirm(Confirmation::Unlock, cancel).await
    });
    cx.run_until_parked();
    assert!(on_screen("logins.capture", cx), "a confirmation warns");
    assert!(on_screen("logins.confirm", cx), "and still asks");
    confirm(cx);
    assert_eq!(answer.await, Ok(true));
}

/// Escape and Ctrl+B typed in a Logins window are its own (review F1): GPUI's bindings and `App::on_action` are app-wide,
/// so without the `Logins` context they reached the browser panel's handlers and could hand the browser back to the
/// agent mid-confirmation. Escape cancels the window, from its body and from a field.
#[gpui::test]
async fn escape_and_ctrl_b_in_a_logins_window_stay_there(cx: &mut TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    use crate::keymap::{Dismiss, TogglePanel};

    let windows = start(cx);
    // The browser panel's handlers, as `ui::browser::init` registers them: app-wide.
    let heard = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let on_dismiss = heard.clone();
        cx.on_action(move |_: &Dismiss, _: &mut App| {
            guarded("test dismiss", || on_dismiss.set(on_dismiss.get() + 1));
        });
        let on_toggle = heard.clone();
        cx.on_action(move |_: &TogglePanel, _: &mut App| {
            guarded("test toggle", || on_toggle.set(on_toggle.get() + 1));
        });
    });

    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(cx, async move {
        asking.confirm(Confirmation::Unlock, cancel).await
    });
    cx.run_until_parked();
    let window = draw(cx);
    cx.simulate_keystrokes(window, "ctrl-b");
    assert_eq!(heard.get(), 0, "Ctrl+B leaves the browser panel alone");
    assert!(shown(cx).is_some(), "and the Logins window open");
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(heard.get(), 0, "Escape leaves the browser panel alone");
    assert_eq!(answer.await, Ok(false), "and cancels the Logins window");
    assert!(shown(cx).is_none(), "which closed");

    // Typing in a field: the field passes Escape on, and it cancels the entry.
    let (_fire, cancel) = Cancel::pair();
    let asking = windows.clone();
    let answer = ask(
        cx,
        async move { asking.entry("b1".into(), None, cancel).await },
    );
    cx.run_until_parked();
    let window = draw(cx);
    let (_, view) = shown(cx).unwrap();
    cx.update_window(window, |_, window, cx| {
        let password = view.read(cx).fields.as_ref().unwrap().password.clone();
        password.update(cx, |input, cx| input.focus(window, cx));
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window, "ctrl-b escape");
    assert_eq!(heard.get(), 0, "the panel heard nothing from the field");
    assert_eq!(
        answer.await.map(|input| input.is_none()),
        Ok(true),
        "Escape cancelled the entry"
    );
    assert!(shown(cx).is_none());
}

#[test]
fn says_what_each_request_is() {
    let saved = words(&Ask::Entry {
        bot_id: "b1".into(),
        previous: Some(entry()),
    });
    assert_eq!((saved.title, saved.button), ("Save login", "Save login"));
    assert_eq!(saved.destination, "https://docs.rs");
    let fill = words(&Ask::Confirm(Confirmation::Fill {
        entry: entry(),
        destination: LoginOrigin::new("https://docs.rs").unwrap(),
    }));
    assert_eq!(
        fill.description,
        "Fill Docs in this agent's browser? Verify the exact destination below."
    );
    let delete = words(&Ask::Confirm(Confirmation::Delete { entry: entry() }));
    assert_eq!(delete.button, "Delete login");
    let close = words(&Ask::Confirm(Confirmation::Finish {
        mode: FinishMode::Close,
    }));
    assert_eq!(close.button, "Close private browser");
    let unlock = words(&Ask::Confirm(Confirmation::Unlock));
    assert_eq!(unlock.title, "Unlock Logins");
}
