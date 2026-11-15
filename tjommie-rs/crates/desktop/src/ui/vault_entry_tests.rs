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
        EntryWindows::start(cx)
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
