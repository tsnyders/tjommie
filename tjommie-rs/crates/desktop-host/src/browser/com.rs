//! The WebView2 COM boundary: each asynchronous WebView2 call becomes a future that completes when its callback runs.
//!
//! Everything here lives on the UI thread, in the apartment that owns the parent window. A callback never touches
//! engine state: it hands its result to a one-shot channel (or pushes a typed message), and the awaiting task
//! carries on from there (the T3.2 bridge rule).

use futures::channel::oneshot;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, ClearBrowsingDataCompletedHandler,
    ContentLoadingEventHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler, GetCookiesCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2_2, ICoreWebView2_3,
        ICoreWebView2_13, ICoreWebView2_19, ICoreWebView2Controller, ICoreWebView2Controller3,
        ICoreWebView2Environment, ICoreWebView2Environment10, ICoreWebView2EnvironmentOptions,
        ICoreWebView2Profile2,
    },
    NavigationCompletedEventHandler, TrySuspendCompletedHandler, take_pwstr,
};
use windows::{
    Win32::Foundation::HWND,
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

/// Navigates and waits for the navigation to finish. `Ok(false)` is a navigation that failed (an error page).
pub async fn navigate(webview: &ICoreWebView2, url: &str) -> Result<bool, ComError> {
    let (tx, rx) = oneshot::channel();
    let tx = std::cell::RefCell::new(Some(tx));
    let handler = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
        let mut ok = BOOL::default();
        if let Some(args) = &args {
            // SAFETY: `args` is the live event argument COM passed in.
            let _ = unsafe { args.IsSuccess(&mut ok) };
        }
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(ok.as_bool());
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: plain COM calls; the handler is removed again below.
    unsafe {
        webview.add_NavigationCompleted(&handler, &mut token)?;
        if let Err(error) = webview.Navigate(&HSTRING::from(url)) {
            let _ = webview.remove_NavigationCompleted(token);
            return Err(error.into());
        }
    }
    let result = rx.await.map_err(lost);
    // SAFETY: the token came from `add_NavigationCompleted` on this webview.
    let _ = unsafe { webview.remove_NavigationCompleted(token) };
    result
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
    // SAFETY: plain COM call; the strings outlive it.
    unsafe {
        webview.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params),
            &handler,
        )?;
    }
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

/// Asks the page to sleep (it must already be hidden). Returns at once: the answer, `true` once the page is
/// suspended, only reaches `log`, and `is_suspended` is the truth.
pub fn try_suspend(webview: &ICoreWebView2) -> Result<(), ComError> {
    let webview: ICoreWebView2_3 = webview.cast()?;
    let handler = TrySuspendCompletedHandler::create(Box::new(|result, suspended| {
        match result {
            Ok(()) if suspended => log::debug!("browser page suspended"),
            Ok(()) => log::debug!("browser page did not suspend"),
            Err(error) => log::debug!("browser page suspend failed: {}", error.message()),
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
