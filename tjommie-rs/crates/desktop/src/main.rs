//! Tjommie's desktop app: the GPUI shell in the Aegis design (Phase 3 plan).
// No console window, ever (spec §1).
#![windows_subsystem = "windows"]

use std::process::ExitCode;

use desktop::notices::{self, Notice};
use desktop_host::{identity, owner_safety};
use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, Render, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, px, size,
};

/// The empty window T0.1 opens; T0.5 replaces it with the app skeleton.
struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn main() -> ExitCode {
    let identity = identity::CURRENT;
    // The data root (spec §4), refused when it is the owner's and this isn't the production build. There is no window
    // or log yet to say so; the exit code does.
    let root = match identity.data_root() {
        Ok(root) if owner_safety::assert_root_allowed(&root, &identity).is_ok() => root,
        _ => return ExitCode::from(2),
    };
    desktop_host::log::init(&root);
    desktop_host::crash::install(&root);
    // Windows passes --after-crash when it restarts the app after a crash. Its --hidden (start in the tray) waits for
    // the tray (T6.1): until then the window shows, so a restarted app is never invisible.
    let after_crash = std::env::args().any(|arg| arg == "--after-crash");
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        notices::init(cx);
        if after_crash {
            notices::raise(Notice::ClosedUnexpectedly);
        }
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(identity.product.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        if gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Blank)).is_err() {
            cx.quit();
        }
    });
    ExitCode::SUCCESS
}
