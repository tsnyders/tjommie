//! The trusted Logins windows (Phase 3 plan T3.6; `vault-entry.ts`, `vault-entry.html`, `vault-entry.js`): a login is typed
//! into a window of its own, never into the panel or a page, and each Logins step the vault asks the person to confirm is
//! answered there. One window at a time, modal over the app's window. It closes after two minutes, when the vault
//! cancels or locks, or when the person closes it or presses Escape, and each of those is a cancel. The username and
//! password fields are emptied the moment a form is handed over, and `EntryForm` wipes its copies when it drops; the
//! fields' own copies (gpui-kit's text rope and its undo history) are freed, not wiped, as gpui-kit has no zeroing.
//! Screen capture can't see the window (`WDA_EXCLUDEFROMCAPTURE`, as Electron's content protection); when Windows won't
//! hide it, the failure is logged, an entry window takes no login (no fields, only Cancel), and a confirmation, which
//! shows no secret, says capture can see it. It has no test-driver hooks outside test-driver builds.
//!
//! The vault asks from its own thread: `EntryWindows` sends each request to the UI thread and awaits its answer.

use std::sync::Arc;

use desktop_host::vault::entry::{
    ENTRY_TIMEOUT, EntryForm, EntryInput, FieldsRefused, REFUSED, WindowAnswer, accept_entry,
};
use desktop_host::vault::login::{Cancel, Confirmation, EntryWindow, LoginError};
use futures::channel::oneshot;
use futures::future::BoxFuture;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowKind, WindowOptions, div, px, size,
};
use gpui_kit::base::input::InputState;
use protocol::vault::{FinishMode, LoginEntry};

use crate::guard::{guarded, guarded_async};
use crate::keymap::{CancelLogins, LOGINS};
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// What a window is asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// A login to save: blank, or the saved name and origin when editing.
    Entry {
        bot_id: String,
        previous: Option<LoginEntry>,
    },
    Confirm(Confirmation),
}

/// What a window says: its title, what it asks, its confirm button, and the destination it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words {
    pub title: &'static str,
    pub description: String,
    pub button: &'static str,
    pub destination: String,
}

/// `vault-entry.js`'s words for each request.
pub fn words(ask: &Ask) -> Words {
    match ask {
        Ask::Entry { previous, .. } => Words {
            title: "Save login",
            description: "Enter this website login on the host. Secrets never enter the ordinary app view.".into(),
            button: "Save login",
            destination: previous.as_ref().map_or(String::new(), |entry| entry.origin.as_str().to_owned()),
        },
        Ask::Confirm(Confirmation::Fill { entry, destination }) => Words {
            title: "Fill login",
            description: format!(
                "Fill {} in this agent's browser? Verify the exact destination below.",
                entry.name
            ),
            button: "Fill login",
            destination: destination.as_str().to_owned(),
        },
        Ask::Confirm(Confirmation::Delete { entry }) => Words {
            title: "Delete login",
            description: format!(
                "Delete {} from this host's protected store? Browser sign-ins and profiles stay in place.",
                entry.name
            ),
            button: "Delete login",
            destination: entry.origin.as_str().to_owned(),
        },
        Ask::Confirm(Confirmation::Finish { mode }) => Words {
            title: "Finish private interaction",
            description: match mode {
                FinishMode::Close => "Close this agent's browser to remove the private page from memory. Its saved browser profile remains.",
                FinishMode::Clear => "Clear the login fields before leaving private mode. Browser automation remains paused until you explicitly return control.",
            }
            .into(),
            button: match mode {
                FinishMode::Close => "Close private browser",
                FinishMode::Clear => "Clear fields",
            },
            destination: String::new(),
        },
        Ask::Confirm(Confirmation::Unlock) => Words {
            title: "Unlock Logins",
            description: "Allow user-confirmed login entry and fills on this host for this session. Restarting or locking the app locks Logins again.".into(),
            button: "Unlock Logins",
            destination: String::new(),
        },
    }
}

/// Where a window's answer goes.
enum Answer {
    Entry(oneshot::Sender<Result<Option<EntryInput>, LoginError>>),
    Confirm(oneshot::Sender<Result<bool, LoginError>>),
}

impl Answer {
    fn cancel(self) {
        match self {
            Answer::Entry(answer) => {
                let _ = answer.send(Ok(None));
            }
            Answer::Confirm(answer) => {
                let _ = answer.send(Ok(false));
            }
        }
    }
}

enum Request {
    Open {
        // Boxed: a request with a login in it is far larger than Close.
        ask: Box<Ask>,
        cancel: Cancel,
        answer: Answer,
    },
    Close,
}

/// The vault's side: `Send`, so the vault can ask from its own thread.
pub struct EntryWindows {
    requests: async_channel::Sender<Request>,
}

/// The one Logins window open, if any.
#[derive(Default)]
struct Open(Option<(AnyWindowHandle, Entity<EntryView>)>);

impl Global for Open {}

/// How a window is kept out of screen capture: `hide_from_capture`, or a test's stand-in (the test platform's windows
/// have no HWND). Whether it worked.
#[derive(Clone, Copy)]
struct HideFromCapture(fn(&Window) -> bool);

impl Global for HideFromCapture {}

impl Default for HideFromCapture {
    fn default() -> Self {
        HideFromCapture(hide_from_capture)
    }
}

impl EntryWindows {
    /// Starts taking the vault's requests on the UI thread.
    pub fn start(cx: &mut App) -> Arc<EntryWindows> {
        let (requests, inbox) = async_channel::unbounded::<Request>();
        cx.default_global::<Open>();
        cx.default_global::<HideFromCapture>();
        cx.spawn(async move |cx| {
            guarded_async("open the Logins windows", async move {
                while let Ok(request) = inbox.recv().await {
                    cx.update(|cx| {
                        guarded("answer the vault", || match request {
                            Request::Open {
                                ask,
                                cancel,
                                answer,
                            } => open(*ask, cancel, answer, cx),
                            Request::Close => close(cx),
                        });
                    });
                }
            })
            .await
        })
        .detach();
        Arc::new(EntryWindows { requests })
    }

    fn ask(&self, request: Request) -> bool {
        self.requests.try_send(request).is_ok()
    }
}

impl EntryWindow for EntryWindows {
    fn entry(
        &self,
        bot_id: String,
        previous: Option<LoginEntry>,
        cancel: Cancel,
    ) -> BoxFuture<'_, Result<Option<EntryInput>, LoginError>> {
        let (answer, answered) = oneshot::channel();
        let sent = self.ask(Request::Open {
            ask: Box::new(Ask::Entry { bot_id, previous }),
            cancel,
            answer: Answer::Entry(answer),
        });
        Box::pin(async move {
            if !sent {
                return Err(LoginError::Failed);
            }
            answered.await.unwrap_or(Ok(None))
        })
    }

    fn confirm(
        &self,
        request: Confirmation,
        cancel: Cancel,
    ) -> BoxFuture<'_, Result<bool, LoginError>> {
        let (answer, answered) = oneshot::channel();
        let sent = self.ask(Request::Open {
            ask: Box::new(Ask::Confirm(request)),
            cancel,
            answer: Answer::Confirm(answer),
        });
        Box::pin(async move {
            if !sent {
                return Err(LoginError::Failed);
            }
            answered.await.unwrap_or(Ok(false))
        })
    }

    fn close(&self) {
        self.ask(Request::Close);
    }
}

/// Opens a window for `ask`, or refuses it while one is open (`vault-entry.ts`: one at a time).
fn open(ask: Ask, cancel: Cancel, answer: Answer, cx: &mut App) {
    if cx.global::<Open>().0.is_some() {
        match answer {
            Answer::Entry(answer) => {
                let _ = answer.send(Err(LoginError::Failed));
            }
            Answer::Confirm(answer) => {
                let _ = answer.send(Err(LoginError::Failed));
            }
        }
        return;
    }
    if cancel.is_cancelled() {
        answer.cancel();
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(480.), px(620.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Tjommie Logins".into()),
            ..TitlebarOptions::default()
        }),
        is_resizable: false,
        // Modal over the app's window while it asks (GPUI disables the active window, and enables it again when this
        // one is destroyed); minimized, it would leave that window disabled with nothing to answer.
        kind: WindowKind::Dialog,
        is_minimizable: false,
        ..WindowOptions::default()
    };
    let hide = cx.global::<HideFromCapture>().0;
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        let hidden = hide(window);
        let view = cx.new(|cx| EntryView::new(ask, answer, hidden, window, cx));
        // The keyboard starts in the window, so its own keys (Escape) are its.
        let focus = view.read(cx).focus.clone();
        focus.focus(window, cx);
        // Weak, here and in the timer below: only the window keeps the typed fields.
        let closing = view.downgrade();
        // unguarded: it must answer Windows with a bool; its work runs in guarded inside
        window.on_window_should_close(cx, move |_, cx| {
            guarded("close the Logins window", || {
                if let Some(view) = closing.upgrade() {
                    view.update(cx, |view, _| view.cancel());
                    // Off the record now: the next request needn't wait for the timer (review F4).
                    forget(&view, cx);
                }
            });
            true
        });
        view
    });
    match opened {
        Ok((handle, view)) => {
            cx.global_mut::<Open>().0 = Some((handle, view.clone()));
            // The vault cancelling (or locking), and two minutes with no answer, close it as a cancel.
            let timer = cx.background_executor().timer(ENTRY_TIMEOUT);
            let view = view.downgrade();
            cx.spawn(async move |cx| {
                guarded_async("close the Logins window when it is over", async move {
                    futures::future::select(Box::pin(cancel.cancelled()), Box::pin(timer)).await;
                    cx.update(|cx| {
                        if let Some(view) = view.upgrade() {
                            finish(&view, cx);
                        }
                    });
                })
                .await
            })
            .detach();
        }
        Err(error) => log::error!("Could not open the Logins window: {error:#}"),
    }
}

/// Closes the open window as a cancel (a lock).
fn close(cx: &mut App) {
    if let Some((_, view)) = cx.global::<Open>().0.clone() {
        finish(&view, cx);
    }
}

/// Answers `view`'s request with a cancel if it is still open, and closes its window.
fn finish(view: &Entity<EntryView>, cx: &mut App) {
    view.update(cx, |view, _| view.cancel());
    if let Some(handle) = forget(view, cx) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Takes `view` off the record if it is the open window, and gives back its window.
fn forget(view: &Entity<EntryView>, cx: &mut App) -> Option<AnyWindowHandle> {
    let open = cx.global_mut::<Open>();
    if open.0.as_ref().is_some_and(|(_, shown)| shown == view) {
        open.0.take().map(|(handle, _)| handle)
    } else {
        None
    }
}

/// Keeps the window out of screenshots and screen recordings. Whether Windows did; a failure is logged loudly.
fn hide_from_capture(window: &Window) -> bool {
    let hidden = match raw_window_handle::HasWindowHandle::window_handle(window)
        .map(|handle| handle.as_raw())
    {
        Ok(raw_window_handle::RawWindowHandle::Win32(handle)) => {
            desktop_host::vault::entry::exclude_from_capture(handle.hwnd.get())
                .map_err(|error| error.to_string())
        }
        Ok(other) => Err(format!("not a Win32 window: {other:?}")),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = &hidden {
        log::error!(
            "Screen capture can see the Logins window ({error}): it takes no login, and its confirmations warn"
        );
    }
    hidden.is_ok()
}

/// What an entry window says when Windows won't keep it out of screen capture: it takes no login (review F3).
const CAPTURE_REFUSED: &str =
    "This PC can't keep this window out of screen capture, so it won't take a login.";
/// What a confirmation says then: it shows no secret, so it still asks.
const CAPTURE_WARNING: &str = "This PC can't keep this window out of screen capture.";

/// The typed fields of an entry window.
struct Fields {
    name: Entity<InputState>,
    origin: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

pub struct EntryView {
    ask: Ask,
    fields: Option<Fields>,
    problem: Option<&'static str>,
    answer: Option<Answer>,
    /// Screen capture can't see the window.
    hidden: bool,
    focus: FocusHandle,
}

impl EntryView {
    fn new(
        ask: Ask,
        answer: Answer,
        hidden: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields = match &ask {
            // Screen capture could see what is typed: no fields (review F3).
            Ask::Entry { previous, .. } if hidden => {
                let input =
                    |value: String, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
                        cx.new(|cx| {
                            let mut input = InputState::new(window, cx).masked(masked);
                            input.set_value(value, window, cx);
                            input
                        })
                    };
                Some(Fields {
                    name: input(
                        previous
                            .as_ref()
                            .map_or(String::new(), |entry| entry.name.clone()),
                        false,
                        window,
                        cx,
                    ),
                    origin: input(
                        previous
                            .as_ref()
                            .map_or(String::new(), |entry| entry.origin.as_str().to_owned()),
                        false,
                        window,
                        cx,
                    ),
                    username: input(String::new(), false, window, cx),
                    password: input(String::new(), true, window, cx),
                })
            }
            Ask::Entry { .. } | Ask::Confirm(_) => None,
        };
        EntryView {
            ask,
            fields,
            problem: None,
            answer: Some(answer),
            hidden,
            focus: cx.focus_handle(),
        }
    }

    fn cancel(&mut self) {
        if let Some(answer) = self.answer.take() {
            answer.cancel();
        }
    }

    /// Continue: an entry hands over its checked fields (or says to check them, and stays open); a confirmation says yes.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(answer) = self.answer.take() else {
            return;
        };
        match (answer, &self.fields) {
            (Answer::Entry(answer), Some(fields)) => {
                // Straight from the field's rope into the form's own string, which it wipes: `value()` would leave
                // one more copy, in a shared string nothing wipes (review F11).
                let read = |input: &Entity<InputState>, cx: &App| input.read(cx).text().to_string();
                let form = EntryForm::new(
                    read(&fields.name, cx),
                    read(&fields.origin, cx),
                    read(&fields.username, cx),
                    read(&fields.password, cx),
                );
                for secret in [&fields.username, &fields.password] {
                    secret.update(cx, |input, cx| input.set_value("", window, cx));
                }
                match accept_entry(WindowAnswer::Form(form)) {
                    Ok(input) => {
                        let _ = answer.send(Ok(input));
                        self.done(window, cx);
                    }
                    Err(FieldsRefused) => {
                        self.problem = Some(REFUSED);
                        self.answer = Some(Answer::Entry(answer));
                        cx.notify();
                    }
                }
            }
            (Answer::Confirm(answer), _) => {
                let _ = answer.send(Ok(true));
                self.done(window, cx);
            }
            (answer, None) => answer.cancel(),
        }
    }

    fn done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        forget(&cx.entity(), cx);
        window.remove_window();
    }
}

impl Render for EntryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx).clone();
        let colors = palette(cx).clone();
        let words = words(&self.ask);
        let entity = cx.entity();
        let field = |id: &'static str, label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(r(theme.space.s4))
                .child(
                    div()
                        .type_style(&theme.typography.desktop.label)
                        .text_color(hsla(colors.text.secondary))
                        .child(label),
                )
                .child(
                    div()
                        .semantic(id, Role::TextInput)
                        .aria_label(label)
                        .h(r(theme.size.control_desktop))
                        .px(r(theme.space.s12))
                        .flex()
                        .items_center()
                        .rounded(r(theme.radius.field))
                        .border_1()
                        .border_color(hsla(colors.line.field))
                        .bg(hsla(colors.surface.raised))
                        .type_style(&theme.typography.desktop.ui)
                        .text_color(hsla(colors.text.primary))
                        .child(input.clone()),
                )
        };
        let cancel = {
            let entity = entity.clone();
            move |window: &mut Window, cx: &mut App| {
                entity.update(cx, |view, cx| {
                    view.cancel();
                    view.done(window, cx);
                })
            }
        };
        let confirm = move |window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.submit(window, cx))
        };
        let asks = self.fields.is_some() || matches!(self.ask, Ask::Confirm(_));
        div()
            .semantic("logins.window", Role::Dialog)
            .aria_label(words.title)
            .key_context(LOGINS)
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &CancelLogins, window, cx| {
                guarded("cancel the Logins window by key", || {
                    view.cancel();
                    view.done(window, cx);
                });
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap(r(theme.space.s16))
            .p(r(theme.space.s24))
            .bg(hsla(colors.surface.paper))
            .type_style(&theme.typography.desktop.body)
            .text_color(hsla(colors.text.primary))
            .child(
                div()
                    .semantic("logins.title", Role::Heading)
                    .aria_label(words.title)
                    .type_style(&theme.typography.desktop.display_s)
                    .child(words.title),
            )
            .child(div().text_color(hsla(colors.text.secondary)).child(words.description))
            .when(!self.hidden, |body| {
                let said = if matches!(self.ask, Ask::Entry { .. }) {
                    CAPTURE_REFUSED
                } else {
                    CAPTURE_WARNING
                };
                body.child(
                    div()
                        .semantic("logins.capture", Role::Alert)
                        .aria_label(said)
                        .text_color(hsla(theme.color.bot.red))
                        .child(said),
                )
            })
            .when(!words.destination.is_empty(), |body| {
                body.child(
                    div()
                        .semantic("logins.destination", Role::Paragraph)
                        .aria_label(SharedString::from(words.destination.clone()))
                        .type_style(&theme.typography.desktop.code)
                        .child(words.destination.clone()),
                )
            })
            .when_some(self.fields.as_ref(), |body, fields| {
                body.child(field("logins.name", "Login name", &fields.name))
                    .child(field("logins.origin", "Exact HTTPS origin", &fields.origin))
                    .child(field("logins.username", "Username", &fields.username))
                    .child(field("logins.password", "Password", &fields.password))
            })
            .when_some(self.problem, |body, problem| {
                body.child(
                    div()
                        .semantic("logins.problem", Role::Alert)
                        .aria_label(problem)
                        .text_color(hsla(theme.color.bot.red))
                        .child(problem),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(r(theme.space.s8))
                    .child(Pill::new("logins.cancel", "Cancel").on_press(cancel))
                    // An entry window that takes no login has nothing to save.
                    .when(asks, |buttons| {
                        buttons.child(
                            Pill::new("logins.confirm", words.button)
                                .variant(PillVariant::Primary)
                                .on_press(confirm),
                        )
                    }),
            )
            .child(
                div()
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(colors.text.tertiary))
                    .child("Secrets stay on this host. The website receiving a login can read or expose it. Tjommie keeps browser automation and captures paused until the private interaction is finished."),
            )
    }
}

#[cfg(test)]
#[path = "vault_entry_tests.rs"]
mod tests;
