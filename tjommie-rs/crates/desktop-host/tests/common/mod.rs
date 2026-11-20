//! What the browser tests share: a UI thread (an STA with a Win32 message pump and a local executor, as the app's
//! main thread is), a plain parent window, a loopback fixture server, and process counts scoped to one test's folder.

#![allow(dead_code)] // each test binary uses a different part
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::future::Future;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt as _;
use serde::Deserialize;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::w;

use desktop_host::browser::BotIdentity;
use desktop_host::browser::engine::{
    Engine, EngineConfig, EngineError, Notification, Panel, Reply, TabHandle, ToolGuard,
};
use protocol::browser::{BrowserFrame, BrowserState};
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserStreamMessage, StreamEnd};
use std::cell::Cell;

unsafe extern "system" fn parent_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the default window procedure, for a window class that does nothing of its own.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The UI thread of a test: COM apartment, parent window, message pump, local executor.
pub struct Ui {
    pool: LocalPool,
    pub parent: HWND,
}

impl Ui {
    pub fn new() -> Ui {
        // SAFETY: each test runs on its own fresh thread, which becomes this test's single-threaded apartment.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .expect("COM init")
        };
        // SAFETY: plain Win32 window creation on this thread.
        let parent = unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(parent_proc),
                lpszClassName: w!("TjommieTestParent"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let parent = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("TjommieTestParent"),
                w!("Tjommie browser test"),
                WS_OVERLAPPEDWINDOW,
                60,
                60,
                1300,
                900,
                None,
                None,
                None,
                None,
            )
            .expect("parent window");
            let _ = ShowWindow(parent, SW_SHOWNOACTIVATE);
            parent
        };
        Ui {
            pool: LocalPool::new(),
            parent,
        }
    }

    pub fn client_rect(&self) -> RECT {
        let mut rect = RECT::default();
        // SAFETY: a live window of this thread.
        unsafe { GetClientRect(self.parent, &mut rect).expect("client rect") };
        rect
    }

    /// The spawner of this thread's local executor, for code that runs tasks of its own.
    pub fn spawner(&self) -> futures::executor::LocalSpawner {
        self.pool.spawner()
    }

    pub fn spawn(&self, future: impl Future<Output = ()> + 'static) {
        self.pool.spawner().spawn_local(future).expect("spawn");
    }

    /// Runs `future` to completion while the window's messages are pumped, so WebView2's callbacks arrive.
    pub fn run<T: 'static>(&mut self, future: impl Future<Output = T> + 'static) -> T {
        let slot: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
        let out = slot.clone();
        self.spawn(async move {
            let value = future.await;
            *out.borrow_mut() = Some(value);
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            self.pool.run_until_stalled();
            if let Some(value) = slot.borrow_mut().take() {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "the test future did not finish in 120 s"
            );
            pump(Duration::from_millis(10));
        }
    }

    /// Like [`run`](Self::run), but gives up after `within` and returns `None` (the future is dropped).
    pub fn run_within<T: 'static>(
        &mut self,
        within: Duration,
        future: impl Future<Output = T> + 'static,
    ) -> Option<T> {
        let slot: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
        let out = slot.clone();
        self.spawn(async move {
            let value = future.await;
            *out.borrow_mut() = Some(value);
        });
        let deadline = Instant::now() + within;
        loop {
            self.pool.run_until_stalled();
            if let Some(value) = slot.borrow_mut().take() {
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            pump(Duration::from_millis(10));
        }
    }

    /// Pumps messages and tasks until `condition` holds, or fails after `timeout`.
    pub fn wait_until(&mut self, what: &str, timeout: Duration, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + timeout;
        loop {
            self.pool.run_until_stalled();
            if condition() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            pump(Duration::from_millis(20));
        }
    }

    /// Lets the page and the browser run for `time` while messages are pumped.
    pub fn settle(&mut self, time: Duration) {
        let until = Instant::now() + time;
        while Instant::now() < until {
            self.pool.run_until_stalled();
            pump(Duration::from_millis(20));
        }
    }
}

fn pump(wait: Duration) {
    // SAFETY: the standard message loop of this thread.
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        MsgWaitForMultipleObjectsEx(
            None,
            wait.as_millis() as u32,
            QS_ALLINPUT,
            MWMO_INPUTAVAILABLE,
        );
    }
}

/// A loopback HTTP server for `tests/fixtures`, on a thread of its own. Real pages from a real origin, with no
/// hook in the engine to serve them.
pub struct FixtureServer {
    pub port: u16,
}

impl FixtureServer {
    pub fn start() -> FixtureServer {
        let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let mut request = [0u8; 4096];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let text = String::from_utf8_lossy(request.get(..read).unwrap_or_default())
                        .into_owned();
                    let path = text
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .split('?')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    let file = dir.join(if path.is_empty() { "blank.html" } else { &path });
                    let (status, body) = match std::fs::read(&file) {
                        Ok(body) if !path.contains("..") => ("200 OK", body),
                        _ => ("404 Not Found", b"not found".to_vec()),
                    };
                    // One page is an attachment, to be downloaded rather than shown; one asks for a password.
                    let (kind, extra) = match path.as_str() {
                        "download.txt" => (
                            "application/octet-stream",
                            "Content-Disposition: attachment; filename=\"report.txt\"\r\n",
                        ),
                        "basic-auth" => (
                            "text/html; charset=utf-8",
                            "WWW-Authenticate: Basic realm=\"fixture\"\r\n",
                        ),
                        _ => ("text/html; charset=utf-8", ""),
                    };
                    let status = if path == "basic-auth" {
                        "401 Unauthorized"
                    } else {
                        status
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        FixtureServer { port }
    }

    pub fn url(&self, page: &str) -> String {
        format!("http://127.0.0.1:{}/{page}", self.port)
    }
}

#[derive(Debug, Deserialize)]
pub struct ProcessInfo {
    #[serde(rename = "ProcessId")]
    pub pid: u32,
    #[serde(rename = "CommandLine")]
    pub command_line: Option<String>,
}

/// The `msedgewebview2.exe` processes whose command line names `folder`: this test's own browser tree, whatever
/// else on the PC runs WebView2 and however many tests run at once.
pub fn webview_processes(folder: &Path) -> Vec<ProcessInfo> {
    let needle = folder.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$p = @(Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains('{needle}') }} | Select-Object ProcessId,CommandLine); ConvertTo-Json -InputObject $p -Compress"
    );
    let output = desktop_host::spawn::command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("run powershell");
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() || text == "[]" {
        return Vec::new();
    }
    serde_json::from_str(text).unwrap_or_else(|error| panic!("process list {text:?}: {error}"))
}

/// Steps of a test, printed when `TJOMMIE_TEST_TRACE` is set (a hang shows where it stopped).
pub fn trace(step: &str) {
    if std::env::var_os("TJOMMIE_TEST_TRACE").is_some() {
        eprintln!(
            "[trace {:?}] {step}",
            std::thread::current().name().unwrap_or("?")
        );
    }
}

/// Where the engine's live frames and stream ends go in a test, standing in for the bridge to the daemon.
pub struct Sink {
    /// Each frame, with when the sink received it.
    pub frames: Vec<(Instant, BrowserFrame)>,
    pub ends: Vec<StreamEnd>,
    /// Answers `false` ("nobody is watching") to every message while set.
    pub refuse: bool,
    /// How long each delivery takes.
    pub delay: Duration,
}

/// A future that is ready after `time`, without blocking the UI thread.
pub fn after(time: Duration) -> futures::future::LocalBoxFuture<'static, ()> {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(time);
        let _ = tx.send(());
    });
    Box::pin(async move {
        let _ = rx.await;
    })
}

/// The identity of the TLS fixture servers: a self-signed certificate for `127.0.0.1`, made for this test run. Nothing is
/// committed (not even a test key), so a fresh checkout has everything the tests need.
pub fn tls_identity() -> (
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
) {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()])
        .expect("certificate parameters");
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "127.0.0.1");
    let cert = params.self_signed(&key).expect("a self-signed certificate");
    (
        vec![cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
}

/// A bot directory that lists every bot with its browser on, as `Rig::bot` names it: for tests that serve jobs and don't
/// mean to test who the daemon's bots are.
pub struct Everyone(PathBuf);

impl desktop_host::hosts::BotDirectory for Everyone {
    fn find(
        &self,
        id: &str,
    ) -> futures::future::BoxFuture<
        '_,
        Result<Option<desktop_host::hosts::BotInfo>, desktop_host::hosts::DirectoryError>,
    > {
        let identity = BotIdentity {
            id: id.into(),
            name: id.into(),
            home_folder: self.0.join("bots").join(id).to_string_lossy().into_owned(),
            project_folder: None,
        };
        Box::pin(async move {
            Ok(Some(desktop_host::hosts::BotInfo {
                identity,
                browser: true,
            }))
        })
    }
}

pub struct Rig {
    pub ui: Ui,
    pub sink: Rc<RefCell<Sink>>,
    pub engine: Rc<Engine>,
    pub root: tempfile::TempDir,
    pub server: FixtureServer,
    pub clock: Rc<Cell<Instant>>,
    pub notices: async_channel::Receiver<Notification>,
}

impl Rig {
    pub fn new() -> Rig {
        let ui = Ui::new();
        let root = tempfile::tempdir().expect("temp root");
        let server = FixtureServer::start();
        let clock = Rc::new(Cell::new(Instant::now()));
        let mut config = EngineConfig::new(
            ui.parent,
            root.path().join("data").join("webview2"),
            desktop_host::identity::E2E,
        );
        config.start_url = server.url("blank.html");
        let shared = clock.clone();
        config.clock = Rc::new(move || shared.get());
        let spawner = ui.spawner();
        config.spawn = Some(Rc::new(move |task| {
            let _ = spawner.spawn_local(task);
        }));
        let sink = Rc::new(RefCell::new(Sink {
            frames: Vec::new(),
            ends: Vec::new(),
            refuse: false,
            delay: Duration::ZERO,
        }));
        let recorder = sink.clone();
        config.on_stream = Some(Rc::new(move |message| {
            let (accept, delay) = {
                let mut sink = recorder.borrow_mut();
                match message {
                    NativeBrowserStreamMessage::StreamFrame { frame, .. } => {
                        sink.frames.push((Instant::now(), frame));
                    }
                    NativeBrowserStreamMessage::StreamEnd { reason, .. } => sink.ends.push(reason),
                }
                (!sink.refuse, sink.delay)
            };
            Box::pin(async move {
                if !delay.is_zero() {
                    after(delay).await;
                }
                accept
            })
        }));
        let (events, inbox) = async_channel::unbounded();
        let (notify, notices) = async_channel::unbounded();
        let engine = Engine::new(config, events, notify).expect("a root the e2e identity may use");
        let applier = engine.clone();
        ui.spawn(async move {
            while let Ok(event) = inbox.recv().await {
                applier.apply_event(event).await;
            }
        });
        Rig {
            ui,
            sink,
            engine,
            root,
            server,
            clock,
            notices,
        }
    }

    /// Every bot, with its browser on, for `serve`.
    pub fn everyone(&self) -> std::sync::Arc<dyn desktop_host::hosts::BotDirectory> {
        std::sync::Arc::new(Everyone(self.root.path().to_owned()))
    }

    pub fn bot(&self, id: &str) -> BotIdentity {
        BotIdentity {
            id: id.into(),
            name: id.into(),
            home_folder: self
                .root
                .path()
                .join("bots")
                .join(id)
                .to_string_lossy()
                .into_owned(),
            project_folder: None,
        }
    }

    /// A panel over the right half of the parent's client area, in physical pixels.
    pub fn panel(&self) -> Panel {
        let client = self.ui.client_rect();
        Panel {
            rect: RECT {
                left: client.right / 2,
                top: 80,
                right: client.right - 20,
                bottom: client.bottom - 20,
            },
            occluded: false,
            clip: None,
        }
    }

    pub fn open(&mut self, id: &str) {
        trace(&format!("open {id}"));
        let (engine, bot) = (self.engine.clone(), self.bot(id));
        self.ui
            .run(async move { engine.open(&bot).await.expect("open") });
    }

    /// A tool starts on a bot, as one does in production: its page is awake, parked or in the panel, until the guard
    /// ends. A page can only be reached through a guard.
    pub fn begin(&mut self, id: &str) -> ToolGuard {
        let (engine, bot) = (self.engine.clone(), self.bot(id));
        self.ui
            .run(async move { engine.begin_tool(&bot).await.expect("begin tool") })
    }

    /// Loads `page` from the fixture server and waits until the page says it is done.
    pub fn load(&mut self, id: &str, page: &str) {
        trace(&format!("load {id} {page}"));
        let guard = self.begin(id);
        let (tab, url) = (guard.tab().expect("active tab"), self.server.url(page));
        self.ui.run(async move {
            tab.navigate(&url).await.expect("navigate");
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut last = String::new();
            while Instant::now() < deadline {
                last = eval(&tab, "document.readyState + '|' + location.href").await;
                if last == format!("complete|{url}") {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            panic!("page did not load: {url} (the page says {last:?})");
        });
        guard.end();
    }

    /// `eval` until the page accepts the command and the answer is `expected`, or the time is up.
    pub fn eval_until(&mut self, id: &str, expression: &str, expected: &str, within: Duration) {
        let label = format!("{expression} never became {expected:?}");
        let guard = self.begin(id);
        let (tab, expression, expected) = (
            guard.tab().expect("active tab"),
            expression.to_string(),
            expected.to_string(),
        );
        let outcome = self.ui.run(async move {
            let deadline = Instant::now() + within;
            let mut last = String::new();
            let mut refused = 0u32;
            while Instant::now() < deadline {
                match try_eval(&tab, &expression).await {
                    Ok(answer) if answer == expected => {
                        if refused > 0 {
                            eprintln!("note: the page refused {refused} command(s) with the one transient error before it answered");
                        }
                        return Ok(());
                    }
                    Ok(answer) => last = answer,
                    // The one transient error a page may give for a moment while it unwinds a dialog. Any other
                    // error is not that, and fails the test at once instead of being retried away.
                    Err(error) if error.contains(TRANSIENT) => {
                        refused += 1;
                        last = format!("refused: {error}");
                    }
                    Err(error) => panic!("{expression}: a refusal that is not the transient one: {error}"),
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(last)
        });
        guard.end();
        assert!(outcome.is_ok(), "{label}: {outcome:?}");
    }

    pub fn eval(&mut self, id: &str, expression: &str) -> String {
        trace(&format!("eval {expression}"));
        let guard = self.begin(id);
        let (tab, expression) = (guard.tab().expect("active tab"), expression.to_string());
        let answer = self.ui.run(async move { eval(&tab, &expression).await });
        guard.end();
        answer
    }

    pub fn show(&mut self, id: &str) {
        self.engine
            .set_panel(id, Some(self.panel()))
            .expect("set panel");
    }

    pub fn hide(&mut self, id: &str) {
        self.engine.set_panel(id, None).expect("clear panel");
    }

    pub fn wait_suspended(&mut self, id: &str) {
        let engine = self.engine.clone();
        let id = id.to_string();
        self.ui
            .wait_until("the page to suspend", Duration::from_secs(20), move || {
                engine.snapshot(&id).is_some_and(|s| s.suspended)
            });
    }
}

#[derive(Deserialize)]
struct Evaluated {
    result: RemoteObject,
}
#[derive(Deserialize)]
struct RemoteObject {
    value: Option<serde_json::Value>,
}

/// `Runtime.evaluate` of a JavaScript expression, as text.
/// What WebView2 says to a command sent to a page that is still unwinding a dialog it has just been told to close.
pub const TRANSIENT: &str = "The parameter is incorrect";

/// `eval`, but a refusal is an `Err`: a page that has only just been unblocked may refuse a command for a moment.
pub async fn try_eval(tab: &TabHandle, expression: &str) -> Result<String, String> {
    let params = serde_json::json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }).to_string();
    let json = tab
        .call("Runtime.evaluate", &params)
        .await
        .map_err(|error| error.to_string())?;
    let evaluated: Evaluated = serde_json::from_str(&json).map_err(|error| error.to_string())?;
    Ok(match evaluated.result.value {
        Some(serde_json::Value::String(text)) => text,
        Some(other) => other.to_string(),
        None => "undefined".to_string(),
    })
}

pub async fn eval(tab: &TabHandle, expression: &str) -> String {
    let params = serde_json::json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }).to_string();
    let json = tab
        .call("Runtime.evaluate", &params)
        .await
        .expect("Runtime.evaluate");
    match serde_json::from_str::<Evaluated>(&json)
        .expect("evaluate result")
        .result
        .value
    {
        Some(serde_json::Value::String(text)) => text,
        Some(other) => other.to_string(),
        None => "undefined".to_string(),
    }
}

impl Rig {
    /// Runs a wire command for a bot, as the daemon would send it.
    pub fn exec(&mut self, id: &str, command: serde_json::Value) -> Result<Reply, EngineError> {
        trace(&format!("exec {command}"));
        let command: NativeBrowserCommand =
            serde_json::from_value(command).expect("a command the protocol accepts");
        let (engine, bot) = (self.engine.clone(), self.bot(id));
        self.ui
            .run(async move { engine.execute(&bot, command).await })
    }

    pub fn exec_state(&mut self, id: &str, command: serde_json::Value) -> BrowserState {
        match self.exec(id, command).expect("the command to succeed") {
            Reply::State(state) => *state,
            other => panic!("expected a browser state, got {other:?}"),
        }
    }

    pub fn exec_error(&mut self, id: &str, command: serde_json::Value) -> String {
        match self.exec(id, command) {
            Err(error) => error.to_string(),
            Ok(reply) => panic!("expected a refusal, got {reply:?}"),
        }
    }

    /// A remote controller's command for a bot, as the daemon sends it.
    pub fn remote(
        &mut self,
        id: &str,
        controller: &str,
        action: serde_json::Value,
    ) -> Result<Reply, EngineError> {
        self.exec(
            id,
            serde_json::json!({ "kind": "remote", "controllerId": controller, "action": action }),
        )
    }

    /// Frames the sink has received so far.
    pub fn frame_count(&self) -> usize {
        self.sink.borrow().frames.len()
    }

    /// Pumps until the sink holds `count` frames (more than it held when this was called, if `count` is 0 relative).
    pub fn wait_frames(&mut self, count: usize, within: Duration) {
        let sink = self.sink.clone();
        self.ui.wait_until("frames to arrive", within, move || {
            sink.borrow().frames.len() >= count
        });
    }

    pub fn state(&self, id: &str) -> BrowserState {
        self.engine.state(id, "local").expect("state")
    }

    /// The notices and states the engine has sent so far.
    pub fn notifications(&self) -> Vec<Notification> {
        let mut seen = Vec::new();
        while let Ok(item) = self.notices.try_recv() {
            seen.push(item);
        }
        seen
    }

    /// Pumps until a notice containing `text` arrives (other notices are kept out of the way).
    pub fn wait_notice(&mut self, text: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            for item in self.notifications() {
                if let Notification::Notice { message, .. } = item
                    && message.contains(text)
                {
                    return message;
                }
            }
            assert!(Instant::now() < deadline, "no notice containing {text:?}");
            self.ui.settle(Duration::from_millis(100));
        }
    }
}
