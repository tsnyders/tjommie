//! The media viewer (Phase 3 plan T4.4; owner decision 5): audio and video played by WebView2's own player. Each open
//! viewer is one controller in the profile `media`, made when the viewer opens and closed with it. Its page lives at an
//! address that is never on the network: `WebResourceRequested` serves the page (one `<video>` or `<audio>` element)
//! and the file, in ranges of at most 256 KiB read from a [`MediaSource`] (the daemon's `attachments.read`) as the
//! player asks for them. Every other request is refused, every other navigation is cancelled, and no window opens.
//!
//! Everything here runs on the UI thread, like the browser engine. A WebView2 callback answers on the spot only from
//! what it was given. A read waits for its bytes on a thread of its own, and its answer goes to WebView2 from a task on
//! the UI thread, because WebView2's objects may only be used there.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_COLOR, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, ICoreWebView2, ICoreWebView2Controller,
        ICoreWebView2Controller2, ICoreWebView2Environment, ICoreWebView2Settings3,
        ICoreWebView2Settings4, ICoreWebView2Settings6, ICoreWebView2Settings8,
        ICoreWebView2WebResourceRequestedEventArgs,
    },
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_OUTOFMEMORY, HWND, RECT};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{HSTRING, Interface as _, PWSTR, w};

use crate::browser::com::{self, ComError};
use crate::browser::engine::{Shield, png_size};
use crate::browser::flags;
use crate::browser::keyboard::FocusDirection;

/// The most one read asks for, and one answer carries: 256 KiB, the daemon's `attachments.read` chunk.
pub const CHUNK: usize = 256 * 1024;

/// The player's profile, whose storage is its own (a bot's profile is named by a 64-hex hash).
const PROFILE: &str = "media";
/// The player's address. A `.invalid` name never resolves (RFC 6761): a request for it is answered here or not at all.
const HOST: &str = "media.tjommie.invalid";
const PAGE_URL: &str = "https://media.tjommie.invalid/";
/// Nothing the player is given stays in its profile's cache.
const NO_STORE: &str = "Cache-Control: no-store";

/// Where a viewer's file comes from. The app implements it over the daemon's `attachments.read`.
pub trait MediaSource: Send + Sync {
    /// The file's length in bytes. Known when the viewer opens: it must not block.
    fn len(&self) -> u64;

    /// Whether the file has no bytes (nothing to play).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The file's content type, such as `video/mp4`. Known when the viewer opens: it must not block.
    fn content_type(&self) -> &str;

    /// Up to `len` bytes of the file from `offset`; `len` is never more than [`CHUNK`]. Fewer bytes than asked are
    /// served as they are, and the player asks again for the rest; an error is said in words. It may block: it runs on
    /// a thread of its own, never on the UI thread.
    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, String>;
}

/// Runs a task on the UI thread, to its end: the app's executor, as the browser engine takes it
/// (`EngineConfig::spawn`). A read's answer reaches WebView2 from such a task.
pub type Spawn = Rc<dyn Fn(LocalBoxFuture<'static, ()>)>;

/// Why a viewer could not open. The text is shown to the person.
#[derive(Debug)]
pub enum MediaError {
    /// The player's data folder could not be made.
    Folder(std::io::Error),
    /// WebView2 failed, or refused a call.
    WebView(ComError),
    /// The player page did not load.
    Page,
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Folder(error) => {
                write!(
                    f,
                    "The media player's data folder could not be made: {error}"
                )
            }
            Self::WebView(error) => write!(f, "The media player could not start: {error}"),
            Self::Page => f.write_str("The media player did not load."),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<ComError> for MediaError {
    fn from(error: ComError) -> Self {
        Self::WebView(error)
    }
}

impl From<windows::core::Error> for MediaError {
    fn from(error: windows::core::Error) -> Self {
        Self::WebView(error.into())
    }
}

/// The media controllers open now, in this process.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// How many media players are open in this process. A closed player is not counted (the plan's
/// `media_viewer::closed_after_close`).
pub fn live_controllers() -> usize {
    LIVE.load(Ordering::SeqCst)
}

/// One open controller in [`live_controllers`], from when it is made until it drops.
struct Counted;

impl Counted {
    fn new() -> Self {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An open media player: a WebView2 controller in the app's window, counted in [`live_controllers`] until it closes.
/// Use it only on the UI thread. Dropping it closes it, as [`close`](Self::close) does.
pub struct MediaViewer {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    tokens: Tokens,
    _counted: Counted,
}

/// The page's handler registrations, removed when the player closes.
#[derive(Default)]
struct Tokens {
    resources: i64,
    navigations: i64,
    windows: i64,
}

impl MediaViewer {
    /// Opens a player for `source` at `bounds` (physical pixels in `parent`'s client area): a `<video>` player, or an
    /// `<audio>` one. Its controller is made in the profile `media` of the WebView2 environment for `user_data_folder`.
    /// Given the browser engine's folder (`<root>/data/webview2`), it shares the bots' browser process, with storage of
    /// its own; another folder starts a browser process for the player alone. `spawn` runs the tasks that answer reads.
    ///
    /// It returns once the page has loaded. Dropped before then (a caller that stops waiting), or failing, the player
    /// is closed again.
    pub async fn open(
        parent: HWND,
        bounds: RECT,
        source: Arc<dyn MediaSource>,
        video: bool,
        user_data_folder: &Path,
        spawn: Spawn,
    ) -> Result<Self, MediaError> {
        std::fs::create_dir_all(user_data_folder).map_err(MediaError::Folder)?;
        let environment =
            com::create_environment(user_data_folder, &flags::browser_arguments()).await?;
        let controller = com::create_controller(&environment, parent, PROFILE).await?;
        let server = Server {
            total: source.len(),
            content_type: media_type(source.content_type()).to_string(),
            source,
            video,
            environment,
            spawn,
        };
        let (webview, tokens) = match prepare(&controller, bounds, server) {
            Ok(prepared) => prepared,
            Err(error) => {
                // SAFETY: closing the controller made above; its page and handlers go with it.
                let _ = unsafe { controller.Close() };
                return Err(error);
            }
        };
        let viewer = Self {
            controller,
            webview,
            tokens,
            _counted: Counted::new(),
        };
        if com::navigate(&viewer.webview, PAGE_URL).await? {
            Ok(viewer)
        } else {
            Err(MediaError::Page)
        }
    }

    /// Moves the player to `bounds`, in physical pixels in the parent's client area (as the browser panel's
    /// `Panel::rect`). The app calls it when the viewer's frame moves or the display scale changes, after the draw
    /// (`window.defer`), as it places a bot's page.
    pub fn set_bounds(&self, bounds: RECT) {
        // SAFETY: plain COM call on the player's controller, on the UI thread.
        let _ = unsafe { self.controller.SetBounds(bounds) };
    }

    /// Shows or hides the player. Hidden, it goes on playing: its sound is still heard.
    pub fn set_visible(&self, visible: bool) {
        // SAFETY: plain COM call on the player's controller, on the UI thread.
        let _ = unsafe { self.controller.SetIsVisible(visible) };
    }

    /// Gives the player the keyboard: when the viewer opens, or when Tab comes to it from the app's controls.
    pub fn focus(&self) {
        let reason = COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
        // SAFETY: plain COM call on the player's controller, on the UI thread.
        let _ = unsafe { self.controller.MoveFocus(reason) };
    }

    /// Hands the keyboard back to the app, as a bot's page does. `leave` is called when Tab or Shift+Tab moves past
    /// the player's first or last control; the app then takes the focus back (`SetFocus` on its window, as
    /// `Engine::focus_app` does). `keys(key, ctrl, shift, alt)` is called for each accelerator key pressed in the
    /// player (Escape, Ctrl+...), and answers `true` to take the key for the app. Both run inside WebView2's callbacks:
    /// they may only hand the news on. Call it once.
    pub fn on_keyboard(
        &self,
        mut leave: impl FnMut(FocusDirection) + 'static,
        keys: impl FnMut(u32, bool, bool, bool) -> bool + 'static,
    ) -> Result<(), MediaError> {
        com::subscribe_move_focus(&self.controller, move |next| {
            let direction = if next {
                FocusDirection::Next
            } else {
                FocusDirection::Previous
            };
            leave(direction);
        })?;
        com::subscribe_accelerators(&self.controller, keys)?;
        Ok(())
    }

    /// Raises the shield, as the browser's (`Engine::raise_shield`): a picture of the player taken now, while it shows,
    /// then the player hidden, so the app can draw the picture in its place and a menu or dialog over it. The picture
    /// is empty if WebView2 could not take one. It waits for WebView2 without a limit: a caller that needs one races
    /// it with a timer (the browser's is 3 s).
    pub fn raise_shield(&self) -> impl Future<Output = Shield> + use<> {
        let (webview, controller) = (self.webview.clone(), self.controller.clone());
        async move {
            let png = match com::capture_preview(&webview).await {
                Ok(png) => png,
                Err(error) => {
                    log::warn!("media player shield without a picture: {error}");
                    Vec::new()
                }
            };
            // SAFETY: plain COM call on the player's controller, on the UI thread.
            let _ = unsafe { controller.SetIsVisible(false) };
            let (width, height) = png_size(&png).unwrap_or((0, 0));
            Shield { png, width, height }
        }
    }

    /// Lowers the shield: the live player comes back.
    pub fn lower_shield(&self) {
        self.set_visible(true);
    }

    /// Closes the player: its handlers are removed, its controller is closed, and it is no longer counted. A read
    /// still on its way finishes, and its answer goes nowhere.
    pub fn close(self) {
        drop(self);
    }
}

impl Drop for MediaViewer {
    fn drop(&mut self) {
        let (webview, tokens) = (&self.webview, &self.tokens);
        // SAFETY: the registrations were made on this page, and the controller is ours: it is closed once, here, on the
        // UI thread.
        unsafe {
            let _ = webview.remove_WebResourceRequested(tokens.resources);
            let _ = webview.remove_NavigationStarting(tokens.navigations);
            let _ = webview.remove_NewWindowRequested(tokens.windows);
            if let Err(error) = self.controller.Close() {
                log::warn!("media player close failed: {}", error.message());
            }
        }
    }
}

/// Readies a new player page: locked down, placed, its requests answered by `server`, its navigations and new windows
/// refused. Returns the page and its handlers' registrations; on an error the caller closes the controller.
fn prepare(
    controller: &ICoreWebView2Controller,
    bounds: RECT,
    server: Server,
) -> Result<(ICoreWebView2, Tokens), MediaError> {
    let black = COREWEBVIEW2_COLOR {
        A: 255,
        R: 0,
        G: 0,
        B: 0,
    };
    let mut tokens = Tokens::default();
    // SAFETY: plain COM calls on the controller just made, on the UI thread. Each handler reads only its own arguments
    // and answers them, or hands the answer to a task on the UI thread.
    unsafe {
        let webview = controller.CoreWebView2()?;
        lock_down(&webview);
        // Black before the page draws, as the page is: no white flash.
        if let Ok(controller) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = controller.SetDefaultBackgroundColor(black);
        }
        controller.SetBounds(bounds)?;
        webview.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        let serve = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                server.answer(&args);
            }
            Ok(())
        }));
        webview.add_WebResourceRequested(&serve, &mut tokens.resources)?;
        let navigate = NavigationStartingEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut uri = PWSTR::null();
                let address = match args.Uri(&mut uri) {
                    Ok(()) => take_pwstr(uri),
                    Err(_) => String::new(),
                };
                if route(&address) != Route::Page {
                    let _ = args.SetCancel(true);
                }
            }
            Ok(())
        }));
        webview.add_NavigationStarting(&navigate, &mut tokens.navigations)?;
        // Handled, with no window given: none opens.
        let window = NewWindowRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let _ = args.SetHandled(true);
            }
            Ok(())
        }));
        webview.add_NewWindowRequested(&window, &mut tokens.windows)?;
        Ok((webview, tokens))
    }
}

/// The player page's settings: no script, web messages, host objects, menus, dev tools, status bar, zoom, error page,
/// autofill, password saving, swipe navigation, browser shortcuts or reputation checks (the address is never on the
/// network, and isn't sent to SmartScreen). The player's controls are the browser's own and need none of them (a bot's
/// page gets the same lockdown, `engine::configure`, but keeps its script) (M4 review B3).
fn lock_down(webview: &ICoreWebView2) {
    // SAFETY: plain COM calls on the page we hold; a setting an older runtime lacks is skipped.
    unsafe {
        let Ok(settings) = webview.Settings() else {
            return;
        };
        let _ = settings.SetIsScriptEnabled(false);
        let _ = settings.SetIsWebMessageEnabled(false);
        let _ = settings.SetAreHostObjectsAllowed(false);
        let _ = settings.SetAreDefaultContextMenusEnabled(false);
        let _ = settings.SetAreDevToolsEnabled(false);
        let _ = settings.SetIsStatusBarEnabled(false);
        let _ = settings.SetIsZoomControlEnabled(false);
        let _ = settings.SetIsBuiltInErrorPageEnabled(false);
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings3>() {
            let _ = settings.SetAreBrowserAcceleratorKeysEnabled(false);
        }
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings4>() {
            let _ = settings.SetIsGeneralAutofillEnabled(false);
            let _ = settings.SetIsPasswordAutosaveEnabled(false);
        }
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings6>() {
            let _ = settings.SetIsSwipeNavigationEnabled(false);
        }
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings8>() {
            let _ = settings.SetIsReputationCheckingRequired(false);
        }
    }
}

/// What the page's requests are answered from, fixed when the player opens.
struct Server {
    total: u64,
    content_type: String,
    source: Arc<dyn MediaSource>,
    video: bool,
    environment: ICoreWebView2Environment,
    spawn: Spawn,
}

impl Server {
    /// Answers one request of the page, at once or once its bytes are read. It runs inside WebView2's callback: it
    /// reads only its arguments, and never waits.
    fn answer(&self, args: &ICoreWebView2WebResourceRequestedEventArgs) {
        let (address, range) = request_of(args);
        let reply = match route(&address) {
            Route::Page => Reply {
                status: 200,
                reason: "OK",
                headers: format!("Content-Type: text/html; charset=utf-8\r\n{NO_STORE}"),
                body: page(self.video).into_bytes(),
            },
            Route::File => match span(range.as_deref(), self.total) {
                Span::Bytes { start, end } => return self.read_later(args, start, end),
                Span::Unsatisfiable => unsatisfiable(self.total),
            },
            Route::Refused => Reply::bare(403, "Forbidden"),
        };
        if let Err(error) = respond(&self.environment, args, &reply) {
            log::warn!("media player request not answered: {}", error.message());
        }
    }

    /// Reads bytes `start..=end` on a thread of their own, and answers the request from a task on the UI thread, where
    /// WebView2's objects live. The request waits for it meanwhile (its deferral).
    fn read_later(&self, args: &ICoreWebView2WebResourceRequestedEventArgs, start: u64, end: u64) {
        // SAFETY: the event's own arguments, inside its callback.
        let deferral = match unsafe { args.GetDeferral() } {
            Ok(deferral) => deferral,
            Err(error) => {
                log::warn!("media read not deferred: {}", error.message());
                let busy = Reply::bare(503, "Service Unavailable");
                let _ = respond(&self.environment, args, &busy);
                return;
            }
        };
        let count = usize::try_from(end.saturating_sub(start).saturating_add(1)).unwrap_or(0);
        let (done, read) = oneshot::channel();
        let source = self.source.clone();
        // ponytail: a thread per read, as the player asks for one range at a time; one reading thread with a queue if
        // reads ever overlap enough to show.
        let started = std::thread::Builder::new()
            .name("media-read".into())
            .spawn(move || {
                let _ = done.send(source.read(start, count));
            });
        if let Err(error) = started {
            // The thread took the read's channel with it: the task below answers 503.
            log::warn!("media read not started: {error}");
        }
        let environment = self.environment.clone();
        let (content_type, total) = (self.content_type.clone(), self.total);
        let args = args.clone();
        (self.spawn)(Box::pin(async move {
            let reply = match read.await {
                Ok(Ok(mut bytes)) => {
                    bytes.truncate(count);
                    partial(&content_type, start, bytes, total)
                }
                Ok(Err(message)) => {
                    log::warn!("media read failed: {message}");
                    Reply::bare(500, "Internal Server Error")
                }
                Err(_) => Reply::bare(503, "Service Unavailable"),
            };
            if let Err(error) = respond(&environment, &args, &reply) {
                // Most likely the player closed meanwhile.
                log::debug!("media read not answered: {}", error.message());
            }
            // SAFETY: the deferral taken for this request, completed once, on the UI thread.
            let _ = unsafe { deferral.Complete() };
        }));
    }
}

/// The request's address and its `Range` header. A request that cannot be read has no address, so it is refused.
fn request_of(args: &ICoreWebView2WebResourceRequestedEventArgs) -> (String, Option<String>) {
    // SAFETY: plain COM calls on the event's own arguments, inside its callback. WebView2 fills each PWSTR with a
    // CoTaskMem string, which `take_pwstr` copies and frees.
    unsafe {
        let Ok(request) = args.Request() else {
            return (String::new(), None);
        };
        let mut uri = PWSTR::null();
        let address = match request.Uri(&mut uri) {
            Ok(()) => take_pwstr(uri),
            Err(_) => String::new(),
        };
        let mut range = None;
        if let Ok(headers) = request.Headers() {
            let mut value = PWSTR::null();
            if headers.GetHeader(w!("Range"), &mut value).is_ok() {
                range = Some(take_pwstr(value));
            }
        }
        (address, range)
    }
}

/// Hands `reply` to WebView2 as the request's response.
fn respond(
    environment: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    reply: &Reply,
) -> windows::core::Result<()> {
    let reason = HSTRING::from(reply.reason);
    let headers = HSTRING::from(reply.headers.as_str());
    // SAFETY: a memory stream holding a copy of the body, and plain COM calls on the UI thread. WebView2 holds the
    // stream for as long as it reads it; the strings outlive the calls.
    unsafe {
        let content = if reply.body.is_empty() {
            None
        } else {
            let stream = SHCreateMemStream(Some(reply.body.as_slice()));
            Some(stream.ok_or(E_OUTOFMEMORY)?)
        };
        let response = environment.CreateWebResourceResponse(
            content.as_ref(),
            reply.status,
            &reason,
            &headers,
        )?;
        args.SetResponse(&response)
    }
}

/// A response to one of the page's requests, before WebView2 has it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reply {
    status: i32,
    reason: &'static str,
    /// `Name: value` lines, apart by CRLF, as `CreateWebResourceResponse` takes them.
    headers: String,
    body: Vec<u8>,
}

impl Reply {
    /// A refusal or a failure: a status and nothing else.
    fn bare(status: i32, reason: &'static str) -> Self {
        Self {
            status,
            reason,
            headers: NO_STORE.to_string(),
            body: Vec::new(),
        }
    }
}

/// A 206 with `bytes`, read at `start` of a file of `total` bytes. A source with nothing there is a failure (500).
fn partial(content_type: &str, start: u64, bytes: Vec<u8>, total: u64) -> Reply {
    if bytes.is_empty() {
        return Reply::bare(500, "Internal Server Error");
    }
    let length = bytes.len();
    let end = start.saturating_add(length as u64 - 1);
    Reply {
        status: 206,
        reason: "Partial Content",
        headers: format!(
            "Content-Type: {content_type}\r\nContent-Length: {length}\r\n\
             Content-Range: bytes {start}-{end}/{total}\r\nAccept-Ranges: bytes\r\n{NO_STORE}"
        ),
        body: bytes,
    }
}

/// A 416 for a file of `total` bytes: none of the range asked for is in it.
fn unsatisfiable(total: u64) -> Reply {
    Reply {
        status: 416,
        reason: "Range Not Satisfiable",
        headers: format!("Content-Range: bytes */{total}\r\nAccept-Ranges: bytes\r\n{NO_STORE}"),
        body: Vec::new(),
    }
}

/// The player page: the browser's own player for `/file`, without its Download, picture-in-picture, casting or full
/// screen (nothing here would show one). No script, and nothing from anywhere else; nothing of the file or the person
/// goes into it.
const PAGE_HEAD: &str = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>\
    html,body{margin:0;height:100%;background:#000;color-scheme:dark}\
    body{display:flex;align-items:center;justify-content:center}\
    video,audio{width:100%}video{height:100%}</style></head><body>";
const VIDEO: &str = "<video src=\"/file\" controls disablepictureinpicture \
    controlslist=\"nodownload nofullscreen noremoteplayback\"></video>";
const AUDIO: &str = "<audio src=\"/file\" controls \
    controlslist=\"nodownload noremoteplayback\"></audio>";
const PAGE_TAIL: &str = "</body></html>";

/// The player page, with a `<video>` player or an `<audio>` one.
fn page(video: bool) -> String {
    let player = if video { VIDEO } else { AUDIO };
    [PAGE_HEAD, player, PAGE_TAIL].concat()
}

/// The file's content type as a header can carry it: printable ASCII only, so that a type from the daemon can never
/// add a header of its own. Anything else is served as `application/octet-stream` (the player looks at the bytes).
fn media_type(content_type: &str) -> &str {
    let content_type = content_type.trim();
    let printable = content_type
        .bytes()
        .all(|byte| (b' '..=b'~').contains(&byte));
    if printable && !content_type.is_empty() {
        content_type
    } else {
        "application/octet-stream"
    }
}

/// What a request of the page is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The player page: the only address the page may navigate to.
    Page,
    /// The file, in ranges.
    File,
    /// Anything else: refused.
    Refused,
}

/// What an address is for: only the player's own, over HTTPS on the default port, is anything.
fn route(address: &str) -> Route {
    let Ok(parsed) = url::Url::parse(address) else {
        return Route::Refused;
    };
    if parsed.scheme() != "https" || parsed.host_str() != Some(HOST) || parsed.port().is_some() {
        return Route::Refused;
    }
    match parsed.path() {
        "/" => Route::Page,
        "/file" => Route::File,
        _ => Route::Refused,
    }
}

/// What of the file a request is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    /// Bytes `start..=end`, with 206 Partial Content: never more than [`CHUNK`].
    Bytes { start: u64, end: u64 },
    /// None of it, with 416 Range Not Satisfiable.
    Unsatisfiable,
}

/// A byte range as a `Range` header asks for it.
enum Wanted {
    /// `bytes=first-` or `bytes=first-last`.
    Start(u64, Option<u64>),
    /// `bytes=-count`: the last `count` bytes.
    Suffix(u64),
}

/// What a request for the file is served, from its `Range` header and the file's length: `bytes=a-b`, `bytes=a-` or
/// the suffix `bytes=-n`, cut to the file and capped at [`CHUNK`] bytes from its start (the player asks again for the
/// rest). A request without a usable header (none, another unit, or an invalid range, which HTTP says to ignore) is
/// served from the start, as `bytes=0-` is. Of several ranges only the first is served. An empty file has nothing.
fn span(range: Option<&str>, total: u64) -> Span {
    let Some(last) = total.checked_sub(1) else {
        return Span::Unsatisfiable;
    };
    let (start, end) = match range.and_then(wanted) {
        None => (0, last),
        Some(Wanted::Start(first, _)) if first > last => return Span::Unsatisfiable,
        Some(Wanted::Start(first, end)) => (first, end.map_or(last, |asked| asked.min(last))),
        Some(Wanted::Suffix(0)) => return Span::Unsatisfiable,
        Some(Wanted::Suffix(count)) => (total.saturating_sub(count), last),
    };
    let end = end.min(start.saturating_add(CHUNK as u64 - 1));
    Span::Bytes { start, end }
}

/// The first range of a `Range` header in bytes, or `None` when it has none to honour.
fn wanted(header: &str) -> Option<Wanted> {
    let (unit, ranges) = header.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (from, to) = ranges.split(',').next()?.split_once('-')?;
    match (number(from), number(to)) {
        (Some(first), None) if to.trim().is_empty() => Some(Wanted::Start(first, None)),
        (Some(first), Some(last)) if first <= last => Some(Wanted::Start(first, Some(last))),
        (None, Some(count)) if from.trim().is_empty() => Some(Wanted::Suffix(count)),
        _ => None,
    }
}

/// A decimal number as HTTP writes one: digits only, no sign.
fn number(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: u64 = CHUNK as u64;

    fn bytes(start: u64, end: u64) -> Span {
        Span::Bytes { start, end }
    }

    #[test]
    fn a_range_is_served_as_asked_cut_to_the_file_and_capped() {
        // (header, file length, bytes served)
        let served = [
            ("bytes=100-199", 1000, (100, 199)),
            ("bytes=100-", 1000, (100, 999)),
            // Cut at the file's end.
            ("bytes=100-5000", 1000, (100, 999)),
            // The last 100 bytes; a suffix longer than the file is all of it.
            ("bytes=-100", 1000, (900, 999)),
            ("bytes=-5000", 1000, (0, 999)),
            // The player asks for everything from a point: it gets 256 KiB, and asks again from there.
            ("bytes=0-", 10_000_000, (0, CAP - 1)),
            ("bytes=10-", 10_000_000, (10, 10 + CAP - 1)),
            ("bytes=-300000", 1_000_000, (700_000, 700_000 + CAP - 1)),
            // The unit in any case, and only the first of several ranges.
            ("Bytes = 1-2", 10, (1, 2)),
            ("bytes=0-9, 20-29", 1000, (0, 9)),
            // A header HTTP says to ignore (not a valid byte range) is no header: served from the start.
            ("items=0-1", 1000, (0, 999)),
            ("bytes=5-1", 1000, (0, 999)),
            ("bytes=+5-", 1000, (0, 999)),
            ("bytes=-", 1000, (0, 999)),
            ("bytes", 1000, (0, 999)),
            ("bytes=99999999999999999999-", 1000, (0, 999)),
        ];
        for (header, total, (start, end)) in served {
            assert_eq!(span(Some(header), total), bytes(start, end), "{header}");
        }
        assert_eq!(span(None, 1000), bytes(0, 999));
    }

    #[test]
    fn a_range_with_nothing_to_serve_is_refused() {
        // Starting past the end, asking for no bytes, or any range of an empty file.
        let refused = [
            (Some("bytes=1000-"), 1000),
            (Some("bytes=-0"), 1000),
            (Some("bytes=0-"), 0),
            (None, 0),
        ];
        for (header, total) in refused {
            assert_eq!(span(header, total), Span::Unsatisfiable, "{header:?}");
        }
    }

    #[test]
    fn an_answer_says_exactly_which_bytes_it_carries() {
        let reply = partial("video/mp4", 100, vec![7; 50], 1000);
        assert_eq!((reply.status, reply.reason), (206, "Partial Content"));
        assert_eq!(
            reply.headers,
            "Content-Type: video/mp4\r\nContent-Length: 50\r\nContent-Range: bytes 100-149/1000\r\n\
             Accept-Ranges: bytes\r\nCache-Control: no-store"
        );
        assert_eq!(reply.body, vec![7; 50]);
        // A source with nothing where the file has bytes is a failure, not an empty answer.
        assert_eq!(partial("video/mp4", 100, Vec::new(), 1000).status, 500);
        let refused = unsatisfiable(1000);
        assert_eq!(refused.status, 416);
        assert_eq!(
            refused.headers,
            "Content-Range: bytes */1000\r\nAccept-Ranges: bytes\r\nCache-Control: no-store"
        );
        assert!(refused.body.is_empty());
    }

    #[test]
    fn a_content_type_can_never_add_a_header() {
        for kept in ["video/mp4", "audio/ogg; codecs=opus"] {
            assert_eq!(media_type(kept), kept);
        }
        assert_eq!(media_type(" video/webm "), "video/webm");
        let refused = [
            "video/mp4\r\nSet-Cookie: a=b",
            "video\tmp4",
            "vid\x7fo/mp4",
            "vidéo/mp4",
            "",
        ];
        for content_type in refused {
            assert_eq!(media_type(content_type), "application/octet-stream");
        }
    }

    #[test]
    fn the_page_is_one_player_for_the_file_and_nothing_else() {
        let (video, audio) = (page(true), page(false));
        assert!(video.contains(r#"<video src="/file" controls"#));
        assert!(!video.contains("<audio"));
        assert!(audio.contains(r#"<audio src="/file" controls"#));
        assert!(!audio.contains("<video"));
        for html in [&video, &audio] {
            assert!(!html.contains("<script") && !html.contains("://"), "{html}");
            assert!(html.contains("nodownload"));
        }
    }

    #[test]
    fn only_the_page_and_the_file_are_served_and_only_the_page_is_a_navigation() {
        assert_eq!(route(PAGE_URL), Route::Page);
        assert_eq!(route("https://media.tjommie.invalid/file"), Route::File);
        assert_eq!(route("https://MEDIA.tjommie.invalid/file?t=1"), Route::File);
        let refused = [
            "https://media.tjommie.invalid/favicon.ico",
            "http://media.tjommie.invalid/file",
            "https://media.tjommie.invalid:8443/file",
            "https://example.com/",
            "about:blank",
            "",
        ];
        for address in refused {
            assert_eq!(route(address), Route::Refused, "{address}");
        }
    }

    #[test]
    fn a_controller_is_counted_from_when_it_is_made_until_it_drops() {
        let before = live_controllers();
        let (first, second) = (Counted::new(), Counted::new());
        assert_eq!(live_controllers(), before + 2);
        drop(first);
        assert_eq!(live_controllers(), before + 1);
        drop(second);
        assert_eq!(live_controllers(), before);
    }
}
