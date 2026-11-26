//! The WebView2 COM boundary: each asynchronous WebView2 call becomes a future that completes when its callback runs.
//!
//! Everything here lives on the UI thread, in the apartment that owns the parent window. A callback never touches
//! engine state: it hands its result to a one-shot channel (or pushes a typed message), and the awaiting task
//! carries on from there (the T3.2 bridge rule).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::channel::oneshot;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CallDevToolsProtocolMethodCompletedHandler,
    CapturePreviewCompletedHandler, ClearBrowsingDataCompletedHandler, ContentLoadingEventHandler,
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    GetCookiesCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        COREWEBVIEW2_MOVE_FOCUS_REASON_NEXT, COREWEBVIEW2_MOVE_FOCUS_REASON_PREVIOUS,
        COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, COREWEBVIEW2_WEB_ERROR_STATUS,
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED, CreateCoreWebView2EnvironmentWithOptions,
        ICoreWebView2, ICoreWebView2_2, ICoreWebView2_3, ICoreWebView2_13, ICoreWebView2_19,
        ICoreWebView2Controller, ICoreWebView2Controller3, ICoreWebView2Environment,
        ICoreWebView2Environment10, ICoreWebView2EnvironmentOptions, ICoreWebView2Profile2,
    },
    MoveFocusRequestedEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, TrySuspendCompletedHandler, take_pwstr,
};
use windows::{
    Win32::Foundation::{HGLOBAL, HWND},
    Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal,
    Win32::System::Com::{IStream, STREAM_SEEK_END, STREAM_SEEK_SET},
    Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT},
    core::{BOOL, HSTRING, Interface as _, PCWSTR, PWSTR},
};

use super::lifecycle::MemoryLevel;

/// A WebView2 failure, in words. The HRESULT's own message is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComError(pub String);

impl std::fmt::Display for ComError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ComError {}

impl From<windows::core::Error> for ComError {
    fn from(error: windows::core::Error) -> Self {
        Self(error.message())
    }
}

fn lost(_: oneshot::Canceled) -> ComError {
    ComError("The browser engine stopped before it answered.".into())
}

/// The one WebView2 environment: one browser process tree, with `arguments` for it. Its user data folder holds a
/// folder per profile.
pub async fn create_environment(
    user_data_folder: &std::path::Path,
    arguments: &str,
) -> Result<ICoreWebView2Environment, ComError> {
    let (tx, rx) = oneshot::channel();
    let options = CoreWebView2EnvironmentOptions::default();
    // SAFETY: the options object is ours and unshared until it is handed to WebView2 below.
    unsafe { options.set_additional_browser_arguments(arguments.to_string()) };
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            let _ = tx.send(
                result
                    .map_err(ComError::from)
                    .and_then(|()| environment.ok_or_else(|| ComError("No environment.".into()))),
            );
            Ok(())
        },
    ));
    // SAFETY: plain FFI call; the strings outlive it and the handler is reference counted by COM.
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(user_data_folder),
            &options,
            &handler,
        )?;
    }
    rx.await.map_err(lost)?
}

/// A controller for a new page in `profile`, as a child of `parent`.
pub async fn create_controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
    profile: &str,
) -> Result<ICoreWebView2Controller, ComError> {
    let environment: ICoreWebView2Environment10 = environment.cast()?;
    // SAFETY: plain COM calls on interfaces we hold.
    let options = unsafe { environment.CreateCoreWebView2ControllerOptions()? };
    unsafe { options.SetProfileName(&HSTRING::from(profile))? };
    let (tx, rx) = oneshot::channel();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result, controller| {
            let _ = tx.send(
                result
                    .map_err(ComError::from)
                    .and_then(|()| controller.ok_or_else(|| ComError("No controller.".into()))),
            );
            Ok(())
        },
    ));
    // SAFETY: `parent` is the app's window; the handler is reference counted by COM.
    unsafe { environment.CreateCoreWebView2ControllerWithOptions(parent, &options, &handler)? };
    rx.await.map_err(lost)?
}

/// An address as a comparison key: parsed and written back, so `HTTPS://Example.com` and `https://example.com/` are one.
fn address_key(address: &str) -> String {
    url::Url::parse(address).map_or_else(|_| address.to_string(), String::from)
}

/// Navigates and waits for *this* navigation to finish. `Ok(false)` is a navigation that failed (an error page).
///
/// WebView2 raises `NavigationCompleted` for every navigation of a page, and a page opened a moment ago is usually still loading its
/// first one. `Navigate` supersedes that one, and its completion (a failure: "connection aborted") is delivered before this
/// navigation's own, even before this navigation's start. Taking the first completion of any kind reported a good navigation as
/// failed, or returned before the page was there. So a navigation is followed by its id: the one whose start carries this address
/// (compared parsed, as WebView2 spells it). A completion for any other id is some other navigation's, whether it succeeded or not.
///
/// Until that start has been seen, a completion is taken for this navigation's only if it ended on this address and was not an abort.
/// That is the case of a page whose start page is the address being navigated to: the navigation joins the one already loading, so no
/// start of its own is ever raised, and the one that was loading is the answer. A superseded start page is an abort, or ends on another
/// address, so it is not.
///
/// This rests on an address matching how WebView2 spells it, which the addresses the browser allows (http, https and about:blank)
/// do; where one did not, this would wait as long as the page takes to settle, so a caller that needs a limit gives it one.
pub async fn navigate(webview: &ICoreWebView2, url: &str) -> Result<bool, ComError> {
    let (tx, rx) = oneshot::channel();
    let tx = RefCell::new(Some(tx));
    let wanted = address_key(url);
    let wanted_at_completion = wanted.clone();
    let view = webview.clone();
    // The id of this navigation, once its start has been seen.
    let own: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));

    let starting_own = own.clone();
    let starting = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = &args else { return Ok(()) };
        // A redirect starts again under the same id, and any later start is some other navigation's.
        if starting_own.get().is_some() {
            return Ok(());
        }
        let mut uri = PWSTR::null();
        // SAFETY: WebView2 fills `uri` with a CoTaskMem string, which `take_pwstr` copies and frees.
        let address = match unsafe { args.Uri(&mut uri) } {
            Ok(()) => take_pwstr(uri),
            Err(_) => String::new(),
        };
        if address_key(&address) == wanted {
            let mut id = 0u64;
            // SAFETY: `args` is the live event argument COM passed in.
            if unsafe { args.NavigationId(&mut id) }.is_ok() {
                starting_own.set(Some(id));
            }
        }
        Ok(())
    }));
    let completed = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = &args else { return Ok(()) };
        let (mut id, mut ok) = (0u64, BOOL::default());
        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
        // SAFETY: `args` is the live event argument COM passed in.
        unsafe {
            let _ = args.NavigationId(&mut id);
            let _ = args.IsSuccess(&mut ok);
            let _ = args.WebErrorStatus(&mut status);
        }
        let ours = match own.get() {
            Some(own_id) => own_id == id,
            None => {
                let aborted =
                    !ok.as_bool() && status == COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED;
                !aborted && source(&view).is_ok_and(|now| address_key(&now) == wanted_at_completion)
            }
        };
        if ours && let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(ok.as_bool());
        }
        Ok(())
    }));
    let (mut starting_token, mut completed_token) = (0i64, 0i64);
    // SAFETY: plain COM calls; both handlers are removed again below.
    unsafe {
        webview.add_NavigationStarting(&starting, &mut starting_token)?;
        if let Err(error) = webview.add_NavigationCompleted(&completed, &mut completed_token) {
            let _ = webview.remove_NavigationStarting(starting_token);
            return Err(error.into());
        }
        if let Err(error) = webview.Navigate(&HSTRING::from(url)) {
            let _ = webview.remove_NavigationStarting(starting_token);
            let _ = webview.remove_NavigationCompleted(completed_token);
            return Err(error.into());
        }
    }
    let result = rx.await.map_err(lost);
    // SAFETY: the tokens came from `add_NavigationStarting` and `add_NavigationCompleted` on this webview.
    unsafe {
        let _ = webview.remove_NavigationStarting(starting_token);
        let _ = webview.remove_NavigationCompleted(completed_token);
    }
    result
}

/// `text` as NUL-terminated UTF-16 in memory that is wiped when it drops, in one allocation (a buffer that grew would
/// leave copies behind that nothing wipes).
fn wide(text: &str) -> zeroize::Zeroizing<Vec<u16>> {
    let mut units = zeroize::Zeroizing::new(Vec::with_capacity(text.encode_utf16().count() + 1));
    units.extend(text.encode_utf16());
    units.push(0);
    units
}

/// One Chrome DevTools Protocol command. `params` and the answer are JSON text: typed encoding and decoding sit
/// above this, in `browser::cdp`.
pub async fn call_devtools(
    webview: &ICoreWebView2,
    method: &str,
    params: &str,
) -> Result<String, ComError> {
    let (tx, rx) = oneshot::channel();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            let _ = tx.send(result.map(|()| json).map_err(ComError::from));
            Ok(())
        }));
    // The parameters may hold a secret, so they go over as a wiped buffer of ours rather than an `HSTRING`, which cannot
    // be wiped.
    let params = wide(params);
    // SAFETY: plain COM call; `params` is NUL-terminated and outlives it (WebView2 copies the text before it returns).
    unsafe {
        webview.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            PCWSTR(params.as_ptr()),
            &handler,
        )?;
    }
    drop(params);
    rx.await.map_err(lost)?
}

/// The page's title.
pub fn title(webview: &ICoreWebView2) -> Result<String, ComError> {
    let mut title = PWSTR::null();
    // SAFETY: WebView2 fills `title` with a CoTaskMem string, which `take_pwstr` copies and frees.
    unsafe { webview.DocumentTitle(&mut title)? };
    Ok(take_pwstr(title))
}

/// The display scale the page renders at (1.0 at 100%).
pub fn rasterization_scale(controller: &ICoreWebView2Controller) -> f64 {
    let Ok(controller) = controller.cast::<ICoreWebView2Controller3>() else {
        return 1.0;
    };
    let mut scale = 1.0f64;
    // SAFETY: plain COM call writing one f64.
    match unsafe { controller.RasterizationScale(&mut scale) } {
        Ok(()) => scale,
        Err(_) => 1.0,
    }
}

/// The page's current address.
pub fn source(webview: &ICoreWebView2) -> Result<String, ComError> {
    let mut uri = PWSTR::null();
    // SAFETY: WebView2 fills `uri` with a CoTaskMem string, which `take_pwstr` copies and frees.
    unsafe { webview.Source(&mut uri)? };
    Ok(take_pwstr(uri))
}

pub fn is_suspended(webview: &ICoreWebView2) -> Result<bool, ComError> {
    let webview: ICoreWebView2_3 = webview.cast()?;
    let mut suspended = BOOL::default();
    // SAFETY: plain COM call writing one BOOL.
    unsafe { webview.IsSuspended(&mut suspended)? };
    Ok(suspended.as_bool())
}

/// Resumes a suspended page. A page that is not suspended is left alone.
pub fn resume(webview: &ICoreWebView2) -> Result<(), ComError> {
    let webview: ICoreWebView2_3 = webview.cast()?;
    // SAFETY: plain COM call.
    unsafe { webview.Resume()? };
    Ok(())
}

/// Asks the page to sleep (it must already be hidden). Returns at once; `is_suspended` is the truth. `done` is called when
/// the request has been answered, in WebView2's callback (it may only hand the news on): a page that was wanted awake in
/// the meantime has fallen asleep behind the caller's back and must be resumed.
pub fn try_suspend(webview: &ICoreWebView2, done: impl FnOnce() + 'static) -> Result<(), ComError> {
    let webview: ICoreWebView2_3 = webview.cast()?;
    let mut done = Some(done);
    let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
        match result {
            Ok(()) if suspended => log::debug!("browser page suspended"),
            Ok(()) => log::debug!("browser page did not suspend"),
            Err(error) => log::debug!("browser page suspend failed: {}", error.message()),
        }
        if let Some(done) = done.take() {
            done();
        }
        Ok(())
    }));
    // SAFETY: plain COM call; the handler is reference counted by COM.
    unsafe { webview.TrySuspend(&handler)? };
    Ok(())
}

/// Low asks Chromium to trim a page that stays awake out of sight; Normal puts it back. Older runtimes without
/// the API are not an error.
pub fn set_memory_level(webview: &ICoreWebView2, level: MemoryLevel) {
    let Ok(webview) = webview.cast::<ICoreWebView2_19>() else {
        return;
    };
    let target = match level {
        MemoryLevel::Low => COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
        MemoryLevel::Normal => COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    // SAFETY: plain COM call.
    if let Err(error) = unsafe { webview.SetMemoryUsageTargetLevel(target) } {
        log::debug!("browser memory target not set: {}", error.message());
    }
}

/// Clears everything the profile of `webview` keeps (cookies, storage, cache, saved sign-in) and checks that no
/// cookie is left. Fails closed: an error here means the profile must be treated as still holding secrets.
pub async fn clear_profile(webview: &ICoreWebView2) -> Result<(), ComError> {
    let with_profile: ICoreWebView2_13 = webview.cast()?;
    // SAFETY: plain COM calls on interfaces we hold.
    let profile: ICoreWebView2Profile2 = unsafe { with_profile.Profile()? }.cast()?;
    let (tx, rx) = oneshot::channel();
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
        let _ = tx.send(result.map_err(ComError::from));
        Ok(())
    }));
    // SAFETY: the handler is reference counted by COM.
    unsafe { profile.ClearBrowsingDataAll(&handler)? };
    rx.await.map_err(lost)??;

    let with_cookies: ICoreWebView2_2 = webview.cast()?;
    // SAFETY: plain COM call.
    let manager = unsafe { with_cookies.CookieManager()? };
    let (tx, rx) = oneshot::channel();
    let handler = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
        let count = match (result, list) {
            (Ok(()), Some(list)) => {
                let mut count = 0u32;
                // SAFETY: the list COM passed in; one u32 is written.
                unsafe { list.Count(&mut count) }
                    .map(|()| count)
                    .map_err(ComError::from)
            }
            (Ok(()), None) => Ok(0),
            (Err(error), _) => Err(ComError::from(error)),
        };
        let _ = tx.send(count);
        Ok(())
    }));
    // An empty address asks for every cookie of the profile.
    // SAFETY: plain COM call; the handler is reference counted by COM.
    unsafe { manager.GetCookies(&HSTRING::new(), &handler)? };
    match rx.await.map_err(lost)?? {
        0 => Ok(()),
        _ => Err(ComError(
            "Browser profile still contains cookies after clearing.".into(),
        )),
    }
}

/// Calls `on_event(parameters_json)` for each DevTools protocol event called `event`, from now on. The callback runs
/// inside a WebView2 callback: it may only hand the text on.
pub fn subscribe_devtools(
    webview: &ICoreWebView2,
    event: &str,
    mut on_event: impl FnMut(String) + 'static,
) -> Result<(), ComError> {
    // SAFETY: plain COM calls on a page this engine owns; the handler is reference counted by COM.
    unsafe {
        let receiver = webview.GetDevToolsProtocolEventReceiver(&HSTRING::from(event))?;
        let handler =
            DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                let mut json = PWSTR::null();
                if let Some(args) = &args {
                    let _ = args.ParameterObjectAsJson(&mut json);
                }
                on_event(take_pwstr(json));
                Ok(())
            }));
        let mut token = 0i64;
        receiver.add_DevToolsProtocolEventReceived(&handler, &mut token)?;
    }
    Ok(())
}

/// Calls `on_content` when a new document starts loading in the page's main frame, from now on (same rule).
pub fn subscribe_content_loading(
    webview: &ICoreWebView2,
    mut on_content: impl FnMut() + 'static,
) -> Result<(), ComError> {
    let handler = ContentLoadingEventHandler::create(Box::new(move |_, _| {
        on_content();
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: plain COM call on a page this engine owns; the handler is reference counted by COM.
    unsafe { webview.add_ContentLoading(&handler, &mut token)? };
    Ok(())
}

/// The most a page preview may weigh: a bigger one is refused, not held in memory.
const PREVIEW_LIMIT: usize = 64 * 1024 * 1024;

/// A PNG of what the page shows now (`CapturePreview`).
pub async fn capture_preview(webview: &ICoreWebView2) -> Result<Vec<u8>, ComError> {
    // SAFETY: a memory stream the capture writes into; it is released when this function ends.
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true)? };
    let (tx, rx) = oneshot::channel();
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let _ = tx.send(result.map_err(ComError::from));
        Ok(())
    }));
    // SAFETY: plain COM call; the stream and the handler outlive it (the stream is held until the answer comes).
    unsafe {
        webview.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )?;
    }
    rx.await.map_err(lost)??;
    read_stream(&stream)
}

fn read_stream(stream: &IStream) -> Result<Vec<u8>, ComError> {
    // SAFETY: plain COM calls on the memory stream this module made; the buffer is as large as the stream says.
    unsafe {
        let mut end = 0u64;
        stream.Seek(0, STREAM_SEEK_END, Some(&mut end))?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let size =
            usize::try_from(end).map_err(|_| ComError("The preview is too large.".into()))?;
        if size > PREVIEW_LIMIT {
            return Err(ComError("The preview is too large.".into()));
        }
        let mut bytes = vec![0u8; size];
        let mut read = 0u32;
        let wanted =
            u32::try_from(size).map_err(|_| ComError("The preview is too large.".into()))?;
        stream
            .Read(bytes.as_mut_ptr().cast(), wanted, Some(&mut read))
            .ok()?;
        bytes.truncate(usize::try_from(read).unwrap_or(0));
        Ok(bytes)
    }
}

/// Calls `on_move(next)` when focus is about to leave the page by Tab (`next`) or Shift+Tab, and marks it handled, so the
/// page does not wrap around inside itself. The callback runs inside WebView2's: it may only hand the answer on.
pub fn subscribe_move_focus(
    controller: &ICoreWebView2Controller,
    mut on_move: impl FnMut(bool) + 'static,
) -> Result<(), ComError> {
    let handler = MoveFocusRequestedEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = &args {
            let mut reason = COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
            // SAFETY: plain COM calls on the event's own arguments.
            unsafe {
                if args.Reason(&mut reason).is_ok()
                    && (reason == COREWEBVIEW2_MOVE_FOCUS_REASON_NEXT
                        || reason == COREWEBVIEW2_MOVE_FOCUS_REASON_PREVIOUS)
                {
                    let _ = args.SetHandled(true);
                    on_move(reason == COREWEBVIEW2_MOVE_FOCUS_REASON_NEXT);
                }
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: plain COM call on a controller this engine owns; the handler is reference counted by COM.
    unsafe { controller.add_MoveFocusRequested(&handler, &mut token)? };
    Ok(())
}

/// Calls `chosen(key, ctrl, shift, alt)` for each key press WebView2 offers as an accelerator; it answers `true` to take
/// the key for the app (the page never sees it). The callback runs inside WebView2's: it must not panic or wait.
pub fn subscribe_accelerators(
    controller: &ICoreWebView2Controller,
    mut chosen: impl FnMut(u32, bool, bool, bool) -> bool + 'static,
) -> Result<(), ComError> {
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = &args {
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP;
            let mut key = 0u32;
            // SAFETY: plain COM and key-state calls on the event's own arguments, inside its callback.
            unsafe {
                if args.KeyEventKind(&mut kind).is_ok()
                    && (kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN)
                    && args.VirtualKey(&mut key).is_ok()
                {
                    let held = |vk: u16| GetKeyState(i32::from(vk)) < 0;
                    if chosen(key, held(VK_CONTROL.0), held(VK_SHIFT.0), held(VK_MENU.0)) {
                        let _ = args.SetHandled(true);
                    }
                }
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: plain COM call on a controller this engine owns; the handler is reference counted by COM.
    unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_compare_as_webview2_writes_them() {
        // WebView2 reports the address it navigates to in its own spelling; the same address must compare equal.
        assert_eq!(
            address_key("HTTPS://Example.COM"),
            address_key("https://example.com/")
        );
        assert_eq!(
            address_key("http://127.0.0.1:80/a.html"),
            address_key("http://127.0.0.1/a.html")
        );
        assert_ne!(
            address_key("https://a.test/x"),
            address_key("https://a.test/y")
        );
        assert_ne!(
            address_key("https://a.test/"),
            address_key("https://b.test/")
        );
        // Something that is not an address is compared as it is, never refused.
        assert_eq!(address_key("not an address"), "not an address");
    }

    #[test]
    fn the_wide_copy_of_a_command_is_one_exact_allocation_ending_in_nul() {
        // A key emoji is a surrogate pair: two UTF-16 units for four UTF-8 bytes.
        let text = "pass\u{1f511}word".repeat(300);
        let wide = wide(&text);
        let (nul, units) = wide.split_last().unwrap();
        assert_eq!(*nul, 0);
        assert_eq!(String::from_utf16(units).unwrap(), text);
        assert_eq!(wide.capacity(), wide.len(), "growth leaves unwiped copies");
    }
}
