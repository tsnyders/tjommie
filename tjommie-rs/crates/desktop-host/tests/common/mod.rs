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
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
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
