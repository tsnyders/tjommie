//! M5's screens in dark and light (Phase 3 plan M5, the dark-mode pass). A fresh install opens in dark, so this opens
//! each screen and Settings section the Screens lane built and keeps a picture of the window for each, then switches
//! Appearance to Light and does it again. The pictures go to `target/m5-screens/<mode>-<screen>.png` (build output),
//! for an owner's eye; the token pairs' contrast is checked by numbers in the M5 report, not here by pixels. A picture is
//! the app's own window (PrintWindow), never the screen, so nothing else on the desktop can get into one. Clicks are
//! fine here: keyboard-only is `e2e_keyboard_navigation`'s. Needs the daemon beside the app, as that test says:
//! `TJOMMIE_IDENTITY=e2e cargo test -p desktop --features test-driver --test e2e_screens_dark_light --locked`.
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use desktop::test_driver::{Client, Predicate};
use desktop_host::{identity, owner_safety};

const SCREENS: [(&str, &str, &str); 5] = [
    ("sidebar.nav.agents", "screen.agents", "agents"),
    ("sidebar.nav.rooms", "screen.rooms", "rooms"),
    ("sidebar.nav.projects", "screen.projects", "projects"),
    ("sidebar.nav.routines", "screen.routines", "routines"),
    ("sidebar.new", "screen.new", "new-agent"),
];

const SECTIONS: [&str; 12] = [
    "general",
    "appearance",
    "notifications",
    "files",
    "voice",
    "logins",
    "bots",
    "accounts",
    "usage",
    "devices",
    "diagnostics",
    "about",
];

fn daemon_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tjommie-desktop")).with_file_name("tjommie-daemon.exe")
}

/// Keeps a picture of the app's window as a PNG: the window of the process started from this test's app, drawn by
/// itself (PrintWindow with PW_RENDERFULLCONTENT, 2), so a window over it never shows in the picture. A blank picture
/// fails (M5 review Minor 29): a 40 by 40 grid of its pixels must hold at least four colours, as any drawn screen does
/// (the sidebar, the page, a card and text).
fn picture(out: &Path) {
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
  public struct Rect {{ public int Left, Top, Right, Bottom; }}
}}
'@
[Win]::SetProcessDPIAware() | Out-Null
$app = Get-CimInstance Win32_Process -Filter "Name='tjommie-desktop.exe'" |
  Where-Object {{ $_.ExecutablePath -eq '{exe}' }} | Sort-Object CreationDate -Descending | Select-Object -First 1
$window = (Get-Process -Id $app.ProcessId).MainWindowHandle
$rect = New-Object Win+Rect
[Win]::GetWindowRect($window, [ref]$rect) | Out-Null
$bitmap = New-Object System.Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
$drawn = [Win]::PrintWindow($window, $dc, 2)
$graphics.ReleaseHdc($dc)
if (-not $drawn) {{ throw 'PrintWindow drew nothing' }}
$seen = @{{}}
$stepY = [Math]::Max(1, [int]($bitmap.Height / 40))
$stepX = [Math]::Max(1, [int]($bitmap.Width / 40))
for ($y = 0; $y -lt $bitmap.Height; $y += $stepY) {{
  for ($x = 0; $x -lt $bitmap.Width; $x += $stepX) {{ $seen[$bitmap.GetPixel($x, $y).ToArgb()] = $true }}
}}
if ($seen.Count -lt 4) {{ throw "The picture is blank: $($seen.Count) colours" }}
$bitmap.Save('{out}', [System.Drawing.Imaging.ImageFormat]::Png)
"#
    );
    let status = desktop_host::spawn::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "no picture of the window, or a blank one, for {out}"
    );
}

/// Clicks `nav` until `title` shows: a click that lands while the app is busy can go unheard.
fn open(app: &mut Client, nav: &str, title: &str) {
    for _ in 0..3 {
        app.click(nav).unwrap();
        if app.wait(title, Predicate::Exists, 5_000).is_ok() {
            return;
        }
    }
    panic!("{title} never showed after clicking {nav}");
}

/// Every M5 screen and Settings section, a picture of each.
fn tour(app: &mut Client, mode: &str, dir: &Path) {
    let keep = |name: &str| {
        // Past the screens' entrances and the focus ring's settling.
        sleep(Duration::from_millis(500));
        picture(&dir.join(format!("{mode}-{name}.png")));
    };
    for (nav, title, name) in SCREENS {
        open(app, nav, title);
        keep(name);
    }
    open(app, "sidebar.nav.settings", "settings.nav.general");
    for section in SECTIONS {
        open(
            app,
            &format!("settings.nav.{section}"),
            &format!("settings.{section}.title"),
        );
        keep(section);
    }
}

#[test]
fn e2e_screens_dark_light() {
    assert_eq!(
        identity::CURRENT.name,
        "e2e",
        "build the e2e app: TJOMMIE_IDENTITY=e2e"
    );
    assert!(
        daemon_exe().exists(),
        "build the daemon first: cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked"
    );
    let root = owner_safety::test_root().unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m5-screens");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = Client::launch_with(
        env!("CARGO_BIN_EXE_tjommie-desktop"),
        root.path(),
        &[("TJOMMIE_DAEMON_EXE", daemon_exe().to_str().unwrap())],
    )
    .unwrap();
    app.wait("screen.agents", Predicate::Exists, 60_000)
        .unwrap();
    tour(&mut app, "dark", &dir);
    app.click("settings.nav.appearance").unwrap();
    app.wait("settings.appearance.theme.light", Predicate::Exists, 10_000)
        .unwrap();
    app.click("settings.appearance.theme.light").unwrap();
    // A radio choice reads as toggled when it's the one chosen.
    let started = std::time::Instant::now();
    while !app
        .snapshot()
        .unwrap()
        .node("settings.appearance.theme.light")
        .is_some_and(|node| node.toggled)
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Light wasn't chosen"
        );
        sleep(Duration::from_millis(100));
    }
    tour(&mut app, "light", &dir);
    for mode in ["dark", "light"] {
        for name in SCREENS.map(|(_, _, name)| name).into_iter().chain(SECTIONS) {
            let file = dir.join(format!("{mode}-{name}.png"));
            assert!(
                std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 0),
                "{} is empty",
                file.display()
            );
        }
    }
}
