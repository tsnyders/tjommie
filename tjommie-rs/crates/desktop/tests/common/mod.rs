//! What the host-wiring e2e tests share (Phase 3 host wiring): the app on a temp root with the provider-fixtures daemon, an
//! agent made through the UI, a mention sent from the Everyone room, and a loopback page server. The agents' turns are
//! recorded fixtures, invented in each test; no provider runs and no account is touched. Needs the daemon built with the
//! switch, beside the app:
//! `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, then
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test <name> --locked`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use desktop::test_driver::{Client, Predicate};

/// The debug daemon beside the app, built with `--features provider-fixtures`.
pub fn daemon_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tjommie-desktop")).with_file_name("tjommie-daemon.exe")
}

/// A fixture for an agent that is never messaged: its folder needs a file the daemon can read, or it refuses to start.
pub const QUIET: &str = r#"{ "version": 1, "turns": [ { "events": [
  { "type": "text", "delta": "Nothing to do." }, { "type": "text_end" } ] } ] }"#;

/// The app on `root` with the fixtures daemon, up to the Agents screen. `fixtures` are `(file name, contents)`: each file
/// is an agent's recorded turns (`<agent name>.json`, or `default.json`).
pub fn launch(root: &Path, fixtures: &[(&str, &str)]) -> Client {
    let mut app = launch_args(root, fixtures, &[]);
    // A daemon without the switch refuses the variable and never starts: this wait is where that shows.
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    app
}

/// `launch`, with command-line arguments for the app (`--hidden`), and without waiting for a screen: a window that starts in
/// the tray draws nothing to wait for.
pub fn launch_args(root: &Path, fixtures: &[(&str, &str)], args: &[&str]) -> Client {
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked"
    );
    let folder = root.join("provider-fixtures");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, contents) in fixtures {
        std::fs::write(folder.join(name), contents).unwrap();
    }
    Client::launch_args(
        env!("CARGO_BIN_EXE_tjommie-desktop"),
        root,
        &[
            ("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap()),
            ("TJOMMIE_PROVIDER_FIXTURES", folder.to_str().unwrap()),
        ],
        args,
    )
    .unwrap()
}

/// The process ids of the app's daemon: `tjommie-daemon.exe` children of the app.
pub fn daemons(app: u32) -> Vec<u32> {
    let filter = format!("ParentProcessId={app} and Name='tjommie-daemon.exe'");
    let command = format!("(Get-CimInstance Win32_Process -Filter \"{filter}\").ProcessId");
    let output = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// Whether process `pid` is running.
pub fn process_alive(pid: u32) -> bool {
    let command = format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue) -ne $null");
    let output = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim() == "True"
}

/// Asks the app's window to close as the person does (WM_CLOSE, which `taskkill` without /F sends).
pub fn close_window(app: u32) {
    let closed = desktop_host::spawn::command("taskkill")
        .args(["/PID", &app.to_string()])
        .output()
        .unwrap();
    assert!(closed.status.success(), "the window was not asked to close");
}

/// Polls `condition` until it holds, or fails after `timeout` saying `what`.
pub fn eventually(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < timeout, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The id of the control that has the keyboard focus.
fn focused(app: &mut Client) -> Option<String> {
    app.snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.focused)
        .map(|node| node.id)
}

/// Tabs until `id` has the keyboard focus. The screen scrolls the focused control into view, so this reaches a control
/// however short the window is, which a click does not: the driver's click is a mouse event at the control's middle, and
/// one below the fold lands outside the window and does nothing, and says nothing.
pub fn tab_to(app: &mut Client, id: &str) {
    let mut passed = Vec::new();
    for _ in 0..80 {
        let before = focused(app);
        app.key("tab").unwrap();
        let started = Instant::now();
        let now = loop {
            let now = focused(app);
            if now != before || started.elapsed() > Duration::from_millis(600) {
                break now;
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        if now.as_deref() == Some(id) {
            // The scroll that brings it into view comes a frame after the focus does.
            std::thread::sleep(Duration::from_millis(150));
            return;
        }
        passed.extend(now);
    }
    panic!("no Tab reaches {id}; it passed {passed:?}");
}

/// Makes an agent through the New agent screen, with its browser on or off, and lands on its chat.
pub fn make_agent(app: &mut Client, name: &str, browser: bool) {
    // The size every e2e runs at, the same on every machine: it fits a hosted runner's 1024 by 768 display. A taller window
    // does not exist there (Windows holds a window to the display), so nothing here may depend on one.
    app.resize(1000., 700.).unwrap();
    app.click("sidebar.nav.agents").unwrap();
    app.wait("agents.new", Predicate::Exists, 10_000).unwrap();
    app.click("agents.new").unwrap();
    app.wait("screen.new", Predicate::Exists, 10_000).unwrap();
    app.click("bot_form.name").unwrap();
    app.text(name).unwrap();
    if browser {
        // By keyboard: the switch is below the fold in a window that fits the display, where a click on it misses silently
        // and the agent is made with its browser off (the fixtures daemon then says "this agent's browser is off").
        tab_to(app, "bot_form.browser");
        app.key("enter").unwrap();
        let started = Instant::now();
        while !app
            .snapshot()
            .unwrap()
            .node("bot_form.browser")
            .is_some_and(|node| node.toggled)
        {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the browser switch did not turn on"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    app.click("new_bot.create").unwrap();
    app.wait("screen.chat", Predicate::Exists, 30_000).unwrap();
}

/// Keeps a picture of the app's window as a PNG: the window of the process started from this test's app, drawn by itself
/// (PrintWindow with PW_RENDERFULLCONTENT, 2), so a window over it never shows in the picture. It is cut to the window's
/// visible frame (DWMWA_EXTENDED_FRAME_BOUNDS), so the invisible resize border Windows adds to a window's rectangle is not in
/// it: a 1000 by 700 window is a 1000 by 700 picture at 100% scale, a hosted runner's. A blank picture fails: a 40 by 40
/// grid of its pixels must hold at least four colours, as any drawn screen does (the sidebar, the page, a card and text).
/// Nothing but the app's own window is ever drawn, so nothing else on the desktop can get into a picture.
pub fn picture(out: &Path) {
    let exe = env!("CARGO_BIN_EXE_tjommie-desktop")
        .replace('/', "\\")
        .replace('\'', "''");
    let out = out.display().to_string().replace('\'', "''");
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Win {{
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr window, int attribute, out Rect rect, int size);
  public struct Rect {{ public int Left, Top, Right, Bottom; }}
}}
'@
[Win]::SetProcessDPIAware() | Out-Null
$app = Get-CimInstance Win32_Process -Filter "Name='tjommie-desktop.exe'" |
  Where-Object {{ $_.ExecutablePath -eq '{exe}' }} | Sort-Object CreationDate -Descending | Select-Object -First 1
$window = (Get-Process -Id $app.ProcessId).MainWindowHandle
$rect = New-Object Win+Rect
[Win]::GetWindowRect($window, [ref]$rect) | Out-Null
$frame = New-Object Win+Rect
if ([Win]::DwmGetWindowAttribute($window, 9, [ref]$frame, 16) -ne 0) {{ $frame = $rect }}
$bitmap = New-Object System.Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
$drawn = [Win]::PrintWindow($window, $dc, 2)
$graphics.ReleaseHdc($dc)
if (-not $drawn) {{ throw 'PrintWindow drew nothing' }}
$cut = New-Object System.Drawing.Rectangle ($frame.Left - $rect.Left), ($frame.Top - $rect.Top), ($frame.Right - $frame.Left), ($frame.Bottom - $frame.Top)
$shown = $bitmap.Clone($cut, $bitmap.PixelFormat)
$seen = @{{}}
$stepY = [Math]::Max(1, [int]($shown.Height / 40))
$stepX = [Math]::Max(1, [int]($shown.Width / 40))
for ($y = 0; $y -lt $shown.Height; $y += $stepY) {{
  for ($x = 0; $x -lt $shown.Width; $x += $stepX) {{ $seen[$shown.GetPixel($x, $y).ToArgb()] = $true }}
}}
if ($seen.Count -lt 4) {{ throw "The picture is blank: $($seen.Count) colours" }}
$shown.Save('{out}', [System.Drawing.Imaging.ImageFormat]::Png)
"$($shown.Width)x$($shown.Height)"
"#
    );
    let output = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "no picture of the window, or a blank one, for {out}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "picture {out}: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
}

/// The id of the Recents entry named `name`.
pub fn recent(app: &mut Client, name: &str) -> String {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        let found = snapshot
            .nodes
            .iter()
            .find(|node| node.id.starts_with("sidebar.recent.") && node.text.contains(name));
        if let Some(node) = found {
            return node.id.clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no Recents entry for {name}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits until some node's text contains `needle`; returns that text.
pub fn wait_for_text(app: &mut Client, needle: &str, timeout: Duration) -> String {
    let started = Instant::now();
    loop {
        let snapshot = app.snapshot().unwrap();
        if let Some(node) = snapshot
            .nodes
            .iter()
            .find(|node| node.text.contains(needle))
        {
            return node.text.clone();
        }
        assert!(
            started.elapsed() < timeout,
            "no node says {needle:?}: {:?}",
            snapshot
                .nodes
                .iter()
                .filter(|node| !node.text.is_empty())
                .map(|node| (node.id.as_str(), node.text.as_str()))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Everyone, from the Rooms list, with a message that mentions `agent`: the daemon hands it to the agent, which plays its
/// next recorded turn.
pub fn mention(app: &mut Client, agent: &str, message: &str) {
    app.click("sidebar.nav.rooms").unwrap();
    app.wait("rooms.room.team.open", Predicate::Exists, 10_000)
        .unwrap();
    app.click("rooms.room.team.open").unwrap();
    app.wait("screen.room", Predicate::Text("Everyone".into()), 10_000)
        .unwrap();
    app.click("room.composer.field").unwrap();
    let start: String = agent.chars().take(3).collect();
    app.text(&format!("@{start}")).unwrap();
    app.wait("room.mentions.0", Predicate::Text(agent.into()), 10_000)
        .unwrap();
    app.key("enter").unwrap();
    app.text(message).unwrap();
    app.key("enter").unwrap();
}

/// A page server on loopback, for an agent's browser to open: `/` is "Fixture page", `/two` is "Second page".
pub struct Pages {
    pub port: u16,
}

impl Pages {
    pub fn start() -> Pages {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = [0u8; 2048];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let text = String::from_utf8_lossy(request.get(..read).unwrap_or(&[]));
                    let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let (title, heading) = if path == "/two" {
                        ("Second page", "The second page")
                    } else {
                        ("Fixture page", "The fixture page")
                    };
                    let body = format!("<!doctype html><title>{title}</title><h1>{heading}</h1>");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes());
                });
            }
        });
        Pages { port }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

/// An agent's recorded turns: one per message, each opening the page at `urls[n]` through the browser tool, then saying
/// `said[n]`. The messages must contain `expect[n]`.
pub fn browsing(turns: &[(&str, &str, &str)]) -> String {
    let turns: Vec<String> = turns
        .iter()
        .enumerate()
        .map(|(at, (expect, url, said))| {
            format!(
                r#"{{ "expect": "{expect}", "events": [
                    {{ "type": "browser", "id": "b{at}", "tool": "browser_navigate", "arguments": {{ "url": "{url}" }} }},
                    {{ "type": "text", "delta": "{said}" }}, {{ "type": "text_end" }} ] }}"#
            )
        })
        .collect();
    format!(r#"{{ "version": 1, "turns": [ {} ] }}"#, turns.join(","))
}
