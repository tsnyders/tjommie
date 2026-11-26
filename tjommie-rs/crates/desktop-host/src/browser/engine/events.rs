//! What WebView2's callbacks say, as typed messages (the bridge rule, Phase 3 plan T3.2), and what the engine says
//! back to its host.
//!
//! A callback decides only what COM needs decided on the spot (cancel a navigation, deny a permission) from pure
//! policy, then pushes a [`HostEvent`] and returns. It never touches engine state and never panics. The host drains
//! the channel on the UI thread and calls `Engine::apply_event`, which is where state changes.

use std::cell::RefCell;
use std::rc::Rc;

use async_channel::Sender;
use protocol::browser::BrowserState;
use webview2_com::{
    BasicAuthenticationRequestedEventHandler, BytesReceivedChangedEventHandler,
    ClientCertificateRequestedEventHandler, DocumentTitleChangedEventHandler,
    DownloadStartingEventHandler, HistoryChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_DOWNLOAD_STATE, COREWEBVIEW2_PERMISSION_KIND,
        COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_SCRIPT_DIALOG_KIND, ICoreWebView2,
        ICoreWebView2_4, ICoreWebView2_5, ICoreWebView2_10, ICoreWebView2_27,
        ICoreWebView2Controller, ICoreWebView2Deferral, ICoreWebView2NewWindowRequestedEventArgs,
        ICoreWebView2ScriptDialogOpeningEventArgs,
    },
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler, ProcessFailedEventHandler,
    ScreenCaptureStartingEventHandler, ScriptDialogOpeningEventHandler, SourceChangedEventHandler,
    StateChangedEventHandler, WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::core::{BOOL, HSTRING, Interface as _, PWSTR};

use super::super::com::ComError;
use super::super::downloads;
use super::super::keyboard::{Chord, FocusDirection};
use super::super::permissions::{self, Permission};
use super::super::policy;

/// Typed messages from WebView2 callbacks and dropped guards, applied on the UI thread by `Engine::apply_event`.
#[derive(Debug)]
pub enum HostEvent {
    /// A bot's need changed where no `await` could carry the change (a dropped tool guard): look at it again.
    Reapply { bot_id: String },
    /// A page's address, title, loading state or history changed. `navigation` is true when a new main-frame
    /// navigation began, which makes whatever a controller saw of the old page stale.
    PageChanged {
        bot_id: String,
        tab_id: String,
        navigation: bool,
        /// Set when the page started (true) or finished (false) loading: a loading page is not put to sleep.
        loading: Option<bool>,
    },
    /// The page closed itself (`window.close()`).
    TabClosed { bot_id: String, tab_id: String },
    /// The page opened a window. Its contents are supplied to a new tab of the same bot, or refused.
    NewWindow {
        bot_id: String,
        args: ICoreWebView2NewWindowRequestedEventArgs,
        deferral: ICoreWebView2Deferral,
    },
    /// The page opened a JavaScript dialog. It stays open until answered through `args` and `deferral`.
    Dialog {
        bot_id: String,
        tab_id: String,
        kind: String,
        message: String,
        default_value: String,
        args: ICoreWebView2ScriptDialogOpeningEventArgs,
        deferral: ICoreWebView2Deferral,
    },
    /// Something for the person to read: a denied permission, a refused address, a failed page.
    Notice { bot_id: String, message: String },
    /// A panel is showing a page that had been given a fixed size: give it its window's size back.
    RestoreViewport { bot_id: String, tab_id: String },
    /// A page's screencast made a frame (`Page.screencastFrame`, as JSON text): decoded and acknowledged by the engine.
    ScreencastFrame {
        bot_id: String,
        tab_id: String,
        json: String,
    },
    /// A request to suspend a page was answered: if the bot wants the page awake by now, it fell asleep behind the
    /// engine's back and must be resumed.
    SuspendDone { bot_id: String },
    /// The page's isolated world called the hand-off binding (Shift+Tab was pressed in the page), as JSON text.
    BindingCalled { bot_id: String, json: String },
    /// Focus is leaving the page by Tab (`next`) or Shift+Tab past its first or last control.
    MoveFocus { bot_id: String, next: bool },
    /// A chord the app keeps for itself was pressed with the page in focus; the page never saw it.
    Shortcut { bot_id: String, chord: Chord },
}

/// What the engine tells its host: the state to draw, and things to say.
#[derive(Debug, Clone, PartialEq)]
pub enum Notification {
    State(Box<BrowserState>),
    Notice {
        bot_id: String,
        message: String,
    },
    /// Focus left the page by the keyboard and belongs to the app: it goes to the next or previous control.
    FocusReturn {
        bot_id: String,
        direction: FocusDirection,
    },
    /// An app chord (`keymap::page_forwarded()`) was pressed with a page in focus: the app runs its action.
    Shortcut {
        bot_id: String,
        chord: Chord,
    },
}

/// The page is blocked on a dialog: its arguments and the deferral that lets it go on.
#[derive(Debug)]
pub(super) struct PendingDialog {
    pub args: ICoreWebView2ScriptDialogOpeningEventArgs,
    pub deferral: ICoreWebView2Deferral,
}

fn push(events: &Sender<HostEvent>, event: HostEvent) {
    // An unbounded channel only refuses once the host is gone; then nobody is listening.
    let _ = events.try_send(event);
}

fn uri_of(args: &impl UriArgs) -> String {
    args.uri()
}

/// The event arguments that carry an address.
trait UriArgs {
    fn uri(&self) -> String;
}

macro_rules! uri_args {
    ($($ty:ty),+) => {$(
        impl UriArgs for $ty {
            fn uri(&self) -> String {
                let mut uri = PWSTR::null();
                // SAFETY: WebView2 fills `uri` with a CoTaskMem string, which `take_pwstr` copies and frees.
                match unsafe { self.Uri(&mut uri) } {
                    Ok(()) => take_pwstr(uri),
                    Err(_) => String::new(),
                }
            }
        }
    )+};
}

uri_args!(
    webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationStartingEventArgs,
    webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NewWindowRequestedEventArgs
);

/// Has a page's screencast frames pushed to the engine as they come. The handler only hands the text on.
pub(super) fn watch_screencast(
    events: &Sender<HostEvent>,
    bot_id: &str,
    tab_id: &str,
    webview: &ICoreWebView2,
) -> Result<(), ComError> {
    let (events, bot_id, tab_id) = (events.clone(), bot_id.to_string(), tab_id.to_string());
    super::super::com::subscribe_devtools(webview, "Page.screencastFrame", move |json| {
        push(
            &events,
            HostEvent::ScreencastFrame {
                bot_id: bot_id.clone(),
                tab_id: tab_id.clone(),
                json,
            },
        );
    })
}

/// A protection of the browser that needs a newer WebView2 runtime than this PC may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protection {
    ScreenCapture,
    Password,
    ClientCertificate,
    Downloads,
}

/// The interface a protection needs, if the runtime has it. When it does not, that protection is off: this is said in the
/// log and to the person, never skipped silently (a prompt nobody can see would otherwise hang the page).
fn present<T>(
    found: windows::core::Result<T>,
    protection: Protection,
    events: &Sender<HostEvent>,
    bot_id: &str,
) -> Option<T> {
    match found {
        Ok(interface) => Some(interface),
        Err(error) => {
            let cannot = match protection {
                Protection::ScreenCapture => "refuse a page's screen sharing",
                Protection::Password => "refuse a page's password prompt",
                Protection::ClientCertificate => "refuse a page's client certificate prompt",
                Protection::Downloads => "keep downloads in the agent's folder",
            };
            log::error!("the WebView2 runtime is too old for {protection:?}: {error}");
            push(
                events,
                HostEvent::Notice {
                    bot_id: bot_id.to_string(),
                    message: format!(
                        "This PC's web component is out of date, so Tjommie's browser cannot {cannot}. Update Microsoft Edge WebView2."
                    ),
                },
            );
            None
        }
    }
}

/// The app chord a key press is, if any. It runs inside a COM callback, which must never panic: a list that cannot be
/// borrowed (it is being replaced) leaves the key to the page.
fn forwarded_hit(
    forwarded: &RefCell<Vec<Chord>>,
    key: u32,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<Chord> {
    forwarded
        .try_borrow()
        .ok()?
        .iter()
        .find(|chord| chord.matches(key, ctrl, shift, alt))
        .copied()
}

/// Registers the keyboard hand-off of a bot's page: Shift+Tab (through the isolated world's binding), Tab past either end
/// of the page, and the app's own chords. The handlers only hand a typed message on.
pub(super) fn watch_keyboard(
    events: &Sender<HostEvent>,
    bot_id: &str,
    controller: &ICoreWebView2Controller,
    webview: &ICoreWebView2,
    forwarded: Rc<RefCell<Vec<Chord>>>,
) -> Result<(), ComError> {
    let (sender, id) = (events.clone(), bot_id.to_string());
    super::super::com::subscribe_devtools(webview, "Runtime.bindingCalled", move |json| {
        push(
            &sender,
            HostEvent::BindingCalled {
                bot_id: id.clone(),
                json,
            },
        );
    })?;
    let (sender, id) = (events.clone(), bot_id.to_string());
    super::super::com::subscribe_move_focus(controller, move |next| {
        push(
            &sender,
            HostEvent::MoveFocus {
                bot_id: id.clone(),
                next,
            },
        );
    })?;
    let (sender, id) = (events.clone(), bot_id.to_string());
    super::super::com::subscribe_accelerators(controller, move |key, ctrl, shift, alt| {
        match forwarded_hit(&forwarded, key, ctrl, shift, alt) {
            Some(chord) => {
                push(
                    &sender,
                    HostEvent::Shortcut {
                        bot_id: id.clone(),
                        chord,
                    },
                );
                true
            }
            None => false,
        }
    })
}

/// Registers every handler a bot's page needs. The handlers live as long as the page.
pub(super) fn watch(
    events: &Sender<HostEvent>,
    bot_id: &str,
    tab_id: &str,
    home_folder: &str,
    webview: &ICoreWebView2,
) -> Result<(), ComError> {
    let changed = |navigation: bool, loading: Option<bool>| {
        let (events, bot_id, tab_id) = (events.clone(), bot_id.to_string(), tab_id.to_string());
        move || {
            push(
                &events,
                HostEvent::PageChanged {
                    bot_id: bot_id.clone(),
                    tab_id: tab_id.clone(),
                    navigation,
                    loading,
                },
            );
        }
    };
    let mut token = 0i64;
    // SAFETY: plain COM registrations on a page we own. Each closure only reads its arguments and pushes a message.
    unsafe {
        // A change of address inside the same document (a fragment, history.pushState) makes what a controller
        // saw stale, like a navigation does.
        let (same_document, other) = (changed(true, None), changed(false, None));
        webview.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |_, args| {
                let mut new_document = BOOL::default();
                if let Some(args) = &args {
                    let _ = args.IsNewDocument(&mut new_document);
                }
                if new_document.as_bool() {
                    other();
                } else {
                    same_document();
                }
                Ok(())
            })),
            &mut token,
        )?;
        let notify = changed(false, None);
        webview.add_DocumentTitleChanged(
            &DocumentTitleChangedEventHandler::create(Box::new(move |_, _| {
                notify();
                Ok(())
            })),
            &mut token,
        )?;
        let notify = changed(false, None);
        webview.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |_, _| {
                notify();
                Ok(())
            })),
            &mut token,
        )?;
        let notify = changed(false, Some(false));
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                notify();
                Ok(())
            })),
            &mut token,
        )?;

        // A main-frame navigation: refused addresses are cancelled here, on the spot.
        let (notify, events_for_guard, bot) = (
            changed(true, Some(true)),
            events.clone(),
            bot_id.to_string(),
        );
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                if policy::native_browser_frame_url(&uri_of(&args), true).is_err() {
                    let _ = args.SetCancel(true);
                    push(
                        &events_for_guard,
                        HostEvent::Notice {
                            bot_id: bot.clone(),
                            message: "That page tried to open an unsupported browser address."
                                .into(),
                        },
                    );
                } else {
                    notify();
                }
                Ok(())
            })),
            &mut token,
        )?;
        // A frame inside the page: web-created documents are fine, privileged addresses are not.
        let (events_for_guard, bot) = (events.clone(), bot_id.to_string());
        webview.add_FrameNavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                if policy::native_browser_frame_url(&uri_of(&args), false).is_err() {
                    let _ = args.SetCancel(true);
                    push(
                        &events_for_guard,
                        HostEvent::Notice {
                            bot_id: bot.clone(),
                            message: "That page tried to open an unsupported browser address."
                                .into(),
                        },
                    );
                }
                Ok(())
            })),
            &mut token,
        )?;

        let (events_for_close, bot, tab) = (events.clone(), bot_id.to_string(), tab_id.to_string());
        webview.add_WindowCloseRequested(
            &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                push(
                    &events_for_close,
                    HostEvent::TabClosed {
                        bot_id: bot.clone(),
                        tab_id: tab.clone(),
                    },
                );
                Ok(())
            })),
            &mut token,
        )?;

        // A popup: refused addresses are handled here; the rest become a tab of this bot once the host has made it.
        let (events_for_window, bot) = (events.clone(), bot_id.to_string());
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                if policy::native_browser_url(&uri_of(&args)).is_err() {
                    let _ = args.SetHandled(true);
                    return Ok(());
                }
                let Ok(deferral) = args.GetDeferral() else {
                    let _ = args.SetHandled(true);
                    return Ok(());
                };
                let event = HostEvent::NewWindow {
                    bot_id: bot.clone(),
                    args: args.clone(),
                    deferral: deferral.clone(),
                };
                if events_for_window.try_send(event).is_err() {
                    let _ = args.SetHandled(true);
                    let _ = deferral.Complete();
                }
                Ok(())
            })),
            &mut token,
        )?;

        // Nothing is granted: every request is denied, and the person is told.
        let (events_for_permission, bot) = (events.clone(), bot_id.to_string());
        webview.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                let _ = args.PermissionKind(&mut kind);
                let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                push(
                    &events_for_permission,
                    HostEvent::Notice {
                        bot_id: bot.clone(),
                        message: permissions::notice(Permission::from_kind(kind.0)),
                    },
                );
                Ok(())
            })),
            &mut token,
        )?;

        // Prompts nobody could see, because a bot's page is parked or hidden, are cancelled: the screen picker
        // (`getDisplayMedia`), a password dialog and a client-certificate picker. Electron denied them all.
        if let Some(webview) = present(
            webview.cast::<ICoreWebView2_27>(),
            Protection::ScreenCapture,
            events,
            bot_id,
        ) {
            let (events_for_capture, bot) = (events.clone(), bot_id.to_string());
            webview.add_ScreenCaptureStarting(
                &ScreenCaptureStartingEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let _ = args.SetCancel(true);
                    }
                    push(
                        &events_for_capture,
                        HostEvent::Notice {
                            bot_id: bot.clone(),
                            message: permissions::notice(Permission::ScreenCapture),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }
        if let Some(webview) = present(
            webview.cast::<ICoreWebView2_10>(),
            Protection::Password,
            events,
            bot_id,
        ) {
            let (events_for_password, bot) = (events.clone(), bot_id.to_string());
            webview.add_BasicAuthenticationRequested(
                &BasicAuthenticationRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let _ = args.SetCancel(true);
                    }
                    push(
                        &events_for_password,
                        HostEvent::Notice {
                            bot_id: bot.clone(),
                            message:
                                "A page asked for a password, which Tjommie's browser never gives."
                                    .into(),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }
        if let Some(webview) = present(
            webview.cast::<ICoreWebView2_5>(),
            Protection::ClientCertificate,
            events,
            bot_id,
        ) {
            let (events_for_certificate, bot) = (events.clone(), bot_id.to_string());
            webview.add_ClientCertificateRequested(
                &ClientCertificateRequestedEventHandler::create(Box::new(move |_, args| {
                    // Handled, with no certificate chosen: the request goes on without one, and no picker is shown.
                    // (Cancelling would abort the connection instead.)
                    if let Some(args) = args {
                        let _ = args.SetHandled(true);
                    }
                    push(
                        &events_for_certificate,
                        HostEvent::Notice {
                            bot_id: bot.clone(),
                            message: "A page asked for a client certificate, which Tjommie's browser never gives."
                                .into(),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }

        let (events_for_failure, bot) = (events.clone(), bot_id.to_string());
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                let mut kind = Default::default();
                if let Some(args) = args {
                    let _ = args.ProcessFailedKind(&mut kind);
                }
                log::warn!("browser process failed: kind {}", kind.0);
                push(
                    &events_for_failure,
                    HostEvent::Notice {
                        bot_id: bot.clone(),
                        message: "The browser page stopped working. Reload it to continue.".into(),
                    },
                );
                Ok(())
            })),
            &mut token,
        )?;

        // Downloads go to the agent's own browser-files folder, up to 100 MB, and the person is told how they ended.
        if let Some(webview) = present(
            webview.cast::<ICoreWebView2_4>(),
            Protection::Downloads,
            events,
            bot_id,
        ) {
            let (events_for_download, bot, home) = (
                events.clone(),
                bot_id.to_string(),
                std::path::PathBuf::from(home_folder),
            );
            webview.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let notice = |message: String| {
                        push(
                            &events_for_download,
                            HostEvent::Notice {
                                bot_id: bot.clone(),
                                message,
                            },
                        );
                    };
                    let Ok(download) = args.DownloadOperation() else {
                        let _ = args.SetCancel(true);
                        return Ok(());
                    };
                    let mut total = 0i64;
                    let _ = download.TotalBytesToReceive(&mut total);
                    if total > downloads::LIMIT_BYTES {
                        let _ = args.SetCancel(true);
                        notice("Downloads are limited to 100 MB.".to_string());
                        return Ok(());
                    }
                    let mut suggested = PWSTR::null();
                    let _ = args.ResultFilePath(&mut suggested);
                    let name = downloads::safe_name(&take_pwstr(suggested));
                    match downloads::target(&home, &name) {
                        Ok(path) => {
                            let _ = args.SetResultFilePath(&HSTRING::from(path.as_os_str()));
                            let _ = args.SetHandled(true);
                            let mut token = 0i64;
                            let _ = download.add_BytesReceivedChanged(
                                &BytesReceivedChangedEventHandler::create(Box::new(|sender, _| {
                                    let mut received = 0i64;
                                    if let Some(download) = sender
                                        && download.BytesReceived(&mut received).is_ok()
                                        && received > downloads::LIMIT_BYTES
                                    {
                                        let _ = download.Cancel();
                                    }
                                    Ok(())
                                })),
                                &mut token,
                            );
                            let (events_for_state, bot_for_state) =
                                (events_for_download.clone(), bot.clone());
                            let _ = download.add_StateChanged(
                                &StateChangedEventHandler::create(Box::new(move |sender, _| {
                                    let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
                                    if let Some(download) = sender {
                                        let _ = download.State(&mut state);
                                    }
                                    let message = match state.0 {
                                        2 => Some(format!(
                                            "Downloaded {name} to the agent's browser-files folder."
                                        )),
                                        1 => Some(format!("Download interrupted: {name}")),
                                        _ => None,
                                    };
                                    if let Some(message) = message {
                                        push(
                                            &events_for_state,
                                            HostEvent::Notice {
                                                bot_id: bot_for_state.clone(),
                                                message,
                                            },
                                        );
                                    }
                                    Ok(())
                                })),
                                &mut token,
                            );
                        }
                        Err(error) => {
                            let _ = args.SetCancel(true);
                            notice(format!("Download failed: {error}"));
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // JavaScript dialogs are held (a deferral) until the person or the bot answers them.
        let (events_for_dialog, bot, tab) =
            (events.clone(), bot_id.to_string(), tab_id.to_string());
        webview.add_ScriptDialogOpening(
            &ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let Ok(deferral) = args.GetDeferral() else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_SCRIPT_DIALOG_KIND::default();
                let _ = args.Kind(&mut kind);
                let (mut message, mut default_text) = (PWSTR::null(), PWSTR::null());
                let _ = args.Message(&mut message);
                let _ = args.DefaultText(&mut default_text);
                let event = HostEvent::Dialog {
                    bot_id: bot.clone(),
                    tab_id: tab.clone(),
                    kind: match kind.0 {
                        1 => "confirm",
                        2 => "prompt",
                        3 => "beforeunload",
                        _ => "alert",
                    }
                    .to_string(),
                    message: take_pwstr(message),
                    default_value: take_pwstr(default_text),
                    args: args.clone(),
                    deferral: deferral.clone(),
                };
                if events_for_dialog.try_send(event).is_err() {
                    // Nobody is listening: the dialog is declined.
                    let _ = deferral.Complete();
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chords(names: &[&str]) -> RefCell<Vec<Chord>> {
        RefCell::new(names.iter().filter_map(|name| Chord::parse(name)).collect())
    }

    #[test]
    fn a_key_press_is_an_app_chord_only_when_its_chord_is_listed() {
        let list = chords(&["Ctrl+T", "F11"]);
        assert_eq!(
            forwarded_hit(&list, 0x54, true, false, false),
            Chord::parse("Ctrl+T")
        );
        assert_eq!(
            forwarded_hit(&list, 0x7A, false, false, false),
            Chord::parse("F11")
        );
        assert_eq!(
            forwarded_hit(&list, 0x54, false, false, false),
            None,
            "a bare T is the page's"
        );
        assert_eq!(
            forwarded_hit(&list, 0x57, true, false, false),
            None,
            "Ctrl+W is not listed"
        );
    }

    #[test]
    fn a_protection_the_runtime_lacks_is_said_to_the_person_not_skipped() {
        let (events, inbox) = async_channel::unbounded();
        let found: windows::core::Result<()> =
            Err(windows::Win32::Foundation::E_NOINTERFACE.into());
        assert!(present(found, Protection::ClientCertificate, &events, "alpha").is_none());
        match inbox.try_recv() {
            Ok(HostEvent::Notice { bot_id, message }) => {
                assert_eq!(bot_id, "alpha");
                assert!(
                    message.contains("out of date") && message.contains("client certificate"),
                    "{message}"
                );
            }
            other => panic!("expected a notice, got {other:?}"),
        }
        // One that is there says nothing.
        assert_eq!(
            present(Ok(7), Protection::Downloads, &events, "alpha"),
            Some(7)
        );
        assert!(inbox.try_recv().is_err());
    }

    #[test]
    fn a_list_being_replaced_does_not_panic_the_callback() {
        // COM calls this inside a callback: a borrow that cannot be had is the page's key, never a panic.
        let list = chords(&["Ctrl+T"]);
        let replacing = list.borrow_mut();
        assert_eq!(forwarded_hit(&list, 0x54, true, false, false), None);
        drop(replacing);
        assert!(forwarded_hit(&list, 0x54, true, false, false).is_some());
    }
}
