//! Tjommie's desktop app: the GPUI shell in the Aegis design (Phase 3 plan).
// No console window, ever (spec §1).
#![windows_subsystem = "windows"]

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

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Tjommie RS".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        if gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Blank)).is_err() {
            cx.quit();
        }
    });
}
