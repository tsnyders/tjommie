//! The e2e test driver (feature `test-driver`, never in a release build). The app serves `\\.\pipe\tjommie-ui-<pid>`
//! with one JSON request per line and one JSON response per line: `click{id}`, `type{id,text}`, `key{chord}`,
//! `snapshot` and `wait{id,predicate,timeout_ms}`. The snapshot is the semantic tree: each element registered with
//! `semantic()`, with its AccessKit role and text as Windows UI Automation sees them. Only the current user may open
//! the pipe.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    A11ySubtreeBuilder, AnyElement, AnyWindowHandle, App, Bounds, Element, ElementId, FocusHandle,
    GlobalElementId, InspectorElementId, InteractiveElement, Interactivity, IntoElement, Keystroke,
    LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Pixels, PlatformInput, Point, StatefulInteractiveElement, StyleRefinement, Styled, Window,
    point, px,
};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PWSTR};

use crate::guard::guarded;

// Review F4: the driver lets another process click and type in the app, so it never builds into the owner's app.
const _: () = assert!(
    !matches!(
        desktop_host::identity::CURRENT.name.as_bytes(),
        b"production"
    ),
    "the test driver never builds into the production identity"
);

const PIPE_PREFIX: &str = r"\\.\pipe\tjommie-ui-";

pub fn pipe_name(pid: u32) -> String {
    format!("{PIPE_PREFIX}{pid}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Click {
        id: String,
    },
    Type {
        id: String,
        text: String,
    },
    Key {
        chord: String,
    },
    Snapshot,
    Wait {
        id: String,
        predicate: Predicate,
        timeout_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    Exists,
    Focused,
    Text(String),
}

impl Predicate {
    pub fn holds(&self, node: &UiNode) -> bool {
        match self {
            Predicate::Exists => true,
            Predicate::Focused => node.focused,
            Predicate::Text(text) => node.text == *text,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<UiSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn done() -> Response {
        Response {
            ok: true,
            ..Response::default()
        }
    }

    pub fn failed(error: impl Into<String>) -> Response {
        Response {
            ok: false,
            snapshot: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub nodes: Vec<UiNode>,
}

impl UiSnapshot {
    pub fn node(&self, id: &str) -> Option<&UiNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiNode {
    pub id: String,
    /// The AccessKit role, e.g. "Heading".
    pub role: String,
    /// The accessible label, else the value.
    pub text: String,
    pub enabled: bool,
    pub focused: bool,
    /// In window coordinates, logical px.
    pub bounds: UiBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UiBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// An e2e test's handle on a running app: launched on a root, driven through its pipe, killed on drop.
pub struct Client {
    app: std::process::Child,
    reader: BufReader<File>,
    writer: File,
}

impl Client {
    /// Starts `exe` with `root` as TJOMMIE_HOME and connects to its pipe.
    pub fn launch(exe: &str, root: &Path) -> io::Result<Client> {
        let mut app = desktop_host::spawn::command(exe)
            .env("TJOMMIE_HOME", root)
            .spawn()?;
        let name = pipe_name(app.id());
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
            {
                Ok(pipe) => {
                    let writer = pipe.try_clone()?;
                    return Ok(Client {
                        app,
                        reader: BufReader::new(pipe),
                        writer,
                    });
                }
                Err(error) => {
                    if let Some(status) = app.try_wait()? {
                        return Err(io::Error::other(format!(
                            "the app exited before its pipe opened: {status}"
                        )));
                    }
                    if Instant::now() > deadline {
                        let _ = app.kill();
                        return Err(io::Error::other(format!("{name} never opened: {error}")));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    pub fn request(&mut self, request: &Request) -> io::Result<Response> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        line.clear();
        self.reader.read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }

    fn expect_ok(&mut self, request: &Request) -> io::Result<Response> {
        let response = self.request(request)?;
        if response.ok {
            Ok(response)
        } else {
            Err(io::Error::other(response.error.unwrap_or_default()))
        }
    }

    pub fn snapshot(&mut self) -> io::Result<UiSnapshot> {
        Ok(self
            .expect_ok(&Request::Snapshot)?
            .snapshot
            .unwrap_or_default())
    }

    pub fn key(&mut self, chord: &str) -> io::Result<()> {
        self.expect_ok(&Request::Key {
            chord: chord.to_owned(),
        })
        .map(drop)
    }

    pub fn click(&mut self, id: &str) -> io::Result<()> {
        self.expect_ok(&Request::Click { id: id.to_owned() })
            .map(drop)
    }

    pub fn wait(&mut self, id: &str, predicate: Predicate, timeout_ms: u64) -> io::Result<()> {
        self.expect_ok(&Request::Wait {
            id: id.to_owned(),
            predicate,
            timeout_ms,
        })
        .map(drop)
    }

    /// The pipe's access list as Windows reports it, in SDDL.
    pub fn pipe_dacl(&self) -> io::Result<String> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let mut text = PWSTR::null();
        // SAFETY: the handle is our open pipe; both outputs are freed with LocalFree once copied.
        unsafe {
            GetSecurityInfo(
                HANDLE(self.writer.as_raw_handle()),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                Some(&mut descriptor),
            )
            .ok()?;
            let converted = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                None,
            );
            let sddl = converted.and_then(|()| text.to_string().map_err(Into::into));
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            Ok(sddl?)
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.app.kill();
        let _ = self.app.wait();
    }
}

/// The current user's SID, e.g. `S-1-5-21-…`.
pub fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: the token handle is closed below; the buffer is u64-aligned for TOKEN_USER and sized by the first call;
    // the SID string is freed with LocalFree once copied.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let read = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        );
        let _ = windows::Win32::Foundation::CloseHandle(token);
        read?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid.map_err(io::Error::other)
    }
}

// The app side: the observed elements, the pipe, and the answers on the UI thread.

type Ask = (Request, mpsc::Sender<Response>);

thread_local! {
    /// The semantic elements painted in the current frame, on the UI thread.
    static NODES: RefCell<Vec<UiNode>> = const { RefCell::new(Vec::new()) };
}

/// The root view calls this as it renders a frame; the frame's elements then register as they prepaint.
pub fn begin_frame() {
    NODES.with(|nodes| nodes.borrow_mut().clear());
}

/// An element registered with `semantic()`, observed for the snapshot. Everything else, its accessibility included,
/// it forwards to the element unchanged.
pub struct Observed<E> {
    inner: E,
    id: &'static str,
    focus: Option<FocusHandle>,
}

impl<E> Observed<E> {
    pub fn new(inner: E, id: &'static str) -> Self {
        Observed {
            inner,
            id,
            focus: None,
        }
    }
}

impl<E: Element> IntoElement for Observed<E> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<E: Element> Element for Observed<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.inner.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.inner.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.inner.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // The element's own accessibility facts: the same ones Windows UI Automation reads.
        let role = self.inner.a11y_role();
        let mut node = gpui::accesskit::Node::new(role.unwrap_or(gpui::Role::Unknown));
        self.inner.write_a11y_info(&mut node);
        let observed = UiNode {
            id: self.id.to_owned(),
            role: role.map(|role| format!("{role:?}")).unwrap_or_default(),
            text: node.label().or(node.value()).unwrap_or_default().to_owned(),
            enabled: !node.is_disabled(),
            focused: self
                .focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window)),
            bounds: UiBounds {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            },
        };
        NODES.with(|nodes| nodes.borrow_mut().push(observed));
        self.inner
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
    }

    fn a11y_role(&self) -> Option<gpui::Role> {
        self.inner.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        self.inner.write_a11y_info(node);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        self.inner.a11y_synthetic_children(prepaint, builder);
    }
}

impl<E: InteractiveElement> InteractiveElement for Observed<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }

    fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self.inner = self.inner.track_focus(focus);
        self
    }
}

impl<E: StatefulInteractiveElement> StatefulInteractiveElement for Observed<E> {}

impl<E: Styled> Styled for Observed<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}

impl<E: ParentElement> ParentElement for Observed<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements);
    }
}

/// Serves the pipe for `window`: a thread of its own talks to the pipe, the UI thread answers each request.
pub fn start(window: AnyWindowHandle, cx: &mut App) {
    let (asks, mut inbox) = unbounded::<Ask>();
    std::thread::spawn(move || {
        if let Err(error) = serve(&asks) {
            log::error!("The test driver stopped: {error}");
        }
    });
    cx.spawn(async move |cx| {
        while let Some((request, reply)) = inbox.next().await {
            let answered = cx.update(|cx| {
                guarded("answer the test driver", || {
                    window.update(cx, |_, window, cx| handle(request, window, cx))
                })
            });
            let response = match answered {
                Some(Ok(response)) => response,
                Some(Err(error)) => Response::failed(format!("{error:#}")),
                None => Response::failed("answering panicked"),
            };
            let _ = reply.send(response);
        }
    })
    .detach();
}

/// The pipe, open to the current user only, and remote clients refused.
fn create_pipe() -> io::Result<File> {
    let sddl = format!("D:P(A;;GA;;;{})", current_user_sid()?);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the descriptor lives until the pipe exists and is freed with LocalFree; the handle becomes the File's.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let pipe = CreateNamedPipeW(
            &HSTRING::from(pipe_name(std::process::id())),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            0,
            Some(&attributes),
        );
        let created = if pipe.is_invalid() {
            Err(io::Error::last_os_error())
        } else {
            Ok(File::from_raw_handle(pipe.0))
        };
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        created
    }
}

fn serve(asks: &UnboundedSender<Ask>) -> io::Result<()> {
    let pipe = create_pipe()?;
    loop {
        // SAFETY: `pipe` owns the handle for the life of this loop.
        let connected = unsafe { ConnectNamedPipe(HANDLE(pipe.as_raw_handle()), None) };
        if let Err(error) = connected
            && error.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            return Err(error.into());
        }
        let mut reader = BufReader::new(&pipe);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(request) => answer(request, asks),
                Err(error) => Response::failed(format!("not a request: {error}")),
            };
            line.clear();
            let mut out = serde_json::to_string(&response)?;
            out.push('\n');
            (&pipe).write_all(out.as_bytes())?;
        }
        // SAFETY: as above.
        let _ = unsafe { DisconnectNamedPipe(HANDLE(pipe.as_raw_handle())) };
    }
}

/// `wait` polls snapshots here, off the UI thread; everything else is answered by the UI thread.
fn answer(request: Request, asks: &UnboundedSender<Ask>) -> Response {
    let Request::Wait {
        id,
        predicate,
        timeout_ms,
    } = request
    else {
        return ask(request, asks);
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let snapshot = ask(Request::Snapshot, asks).snapshot.unwrap_or_default();
        if snapshot.node(&id).is_some_and(|node| predicate.holds(node)) {
            return Response::done();
        }
        if Instant::now() >= deadline {
            return Response::failed(format!("{id} was not {predicate:?} within {timeout_ms} ms"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn ask(request: Request, asks: &UnboundedSender<Ask>) -> Response {
    let (reply, answered) = mpsc::channel();
    if asks.unbounded_send((request, reply)).is_err() {
        return Response::failed("the app is closing");
    }
    answered
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| Response::failed("the app did not answer"))
}

fn handle(request: Request, window: &mut Window, cx: &mut App) -> Response {
    match request {
        Request::Snapshot => Response {
            ok: true,
            snapshot: Some(UiSnapshot {
                nodes: NODES.with(|nodes| nodes.borrow().clone()),
            }),
            error: None,
        },
        Request::Click { id } => match center(&id) {
            Some(position) => {
                click(position, window, cx);
                Response::done()
            }
            None => Response::failed(format!("{id} is not on screen")),
        },
        Request::Type { id, text } => match center(&id) {
            Some(position) => {
                click(position, window, cx);
                for typed in text.chars() {
                    let key_char = typed.to_string();
                    let keystroke = Keystroke {
                        modifiers: Modifiers::default(),
                        key: key_char.to_lowercase(),
                        key_char: Some(key_char),
                    };
                    window.dispatch_keystroke(keystroke, cx);
                }
                Response::done()
            }
            None => Response::failed(format!("{id} is not on screen")),
        },
        Request::Key { chord } => match Keystroke::parse_with_os(&chord, cx.platform_info().os) {
            Ok(keystroke) => {
                window.dispatch_keystroke(keystroke, cx);
                Response::done()
            }
            Err(error) => Response::failed(format!("{chord}: {error}")),
        },
        Request::Wait { .. } => Response::failed("wait is answered off the UI thread"),
    }
}

fn center(id: &str) -> Option<Point<Pixels>> {
    NODES.with(|nodes| {
        nodes
            .borrow()
            .iter()
            .find(|node| node.id == id)
            .map(|node| {
                let bounds = node.bounds;
                point(
                    px(bounds.x + bounds.width / 2.0),
                    px(bounds.y + bounds.height / 2.0),
                )
            })
    })
}

fn click(position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let modifiers = Modifiers::default();
    let button = MouseButton::Left;
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count: 1,
        }),
        cx,
    );
}
