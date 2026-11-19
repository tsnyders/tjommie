//! Starting the app (Phase 3 plan T0.5): the bundled assets and fonts, the Aegis theme, notices, the zoom keys, the
//! window and, in an e2e build, the test driver.

use std::path::PathBuf;

use desktop_host::identity::Identity;
use desktop_host::shell::single_instance::Instance;
use futures::StreamExt as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
    px, size,
};

use crate::assets::{self, Assets, MONO_FAMILY, TEXT_FAMILY};
use crate::guard::{guarded, guarded_async};
use crate::notices::{self, Notice};
use crate::theme::Theme;
use crate::ui::browser::local::LocalBrowser;
use crate::ui::browser::panel::BrowserPanel;
use crate::ui::shell::data::ShellData;
use crate::ui::shell::{self, Shell};
use crate::zoom::Zoom;

/// Runs the app until its window closes. `root` is the data root `main` checked with `owner_safety`; `after_crash`:
/// Windows restarted it after a crash.
pub fn run(identity: Identity, root: PathBuf, after_crash: bool, instance: Option<Instance>) {
    gpui_kit::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            notices::init(cx);
            crate::motion::init(cx);
            crate::ui::browser::init(cx);
            // What the shell shows of the daemon: connecting, until the store (T1.4) is wired to it.
            cx.set_global(ShellData::default());
            match Theme::aegis() {
                Ok(theme) => cx.set_global(theme),
                Err(error) => {
                    log::error!("The Aegis theme does not load: {error}");
                    cx.quit();
                    return;
                }
            }
            if let Err(error) = cx.text_system().add_fonts(assets::fonts()) {
                log::error!("Could not load the fonts: {error:#}");
            }
            let kit = gpui_kit::component::Theme::global_mut(cx);
            kit.font_family = TEXT_FAMILY.into();
            kit.mono_font_family = MONO_FAMILY.into();
            shell::set_rem(Zoom::default(), cx);
            cx.bind_keys(crate::keymap::field_tab(cx));
            cx.bind_keys(crate::keymap::bindings());
            // The daemon and the connection (M5): until it's ready the shell says "Starting Tjommie". A test-driver run on
            // the panel fixture keeps the fixture's store (`ui::browser::e2e_fixture`) until the host-wiring batch
            // gives the panel its live path.
            if !crate::ui::browser::panel_fixture() {
                crate::live::start(identity, root.clone(), cx);
            }
            if after_crash {
                notices::raise(Notice::ClosedUnexpectedly);
            }
            match gpui_kit::open_window(window_options(&identity, cx), cx, |window, cx| {
                // This PC's bots' browsers live under the window; nothing of WebView2 starts until a page is shown.
                LocalBrowser::start(window, identity, cx);
                let panel = BrowserPanel::create(window, cx);
                cx.new(|cx| {
                    let mut shell = Shell::new(window, cx);
                    shell.set_panel(Some(panel.into()), cx);
                    shell
                })
            }) {
                Ok((window, _)) => {
                    if let Some(instance) = instance {
                        show_on_second_launch(instance, window, cx);
                    }
                    #[cfg(feature = "test-driver")]
                    {
                        crate::test_driver::start(window, cx);
                        crate::ui::browser::e2e_fixture(cx);
                    }
                }
                Err(error) => {
                    log::error!("Could not open the window: {error:#}");
                    cx.quit();
                }
            }
        });
}

fn window_options(identity: &Identity, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1440.), px(900.)),
            cx,
        ))),
        window_min_size: Some(size(px(360.), px(600.))),
        // The caption buttons are the app's own (ui::shell), over the window's control areas.
        titlebar: Some(TitlebarOptions {
            title: Some(identity.product.into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        // An e2e run never takes the owner's keyboard; the root holds the app's own focus either way.
        focus: identity.name != "e2e",
        // Gate condition 5: no GPUI cap for an unfocused window. The motion clock (T2.3) throttles instead.
        inactive_frame_interval: None,
        app_id: Some(identity.app_id.to_owned()),
        ..Default::default()
    }
}

/// A later launch asks this one for its window (Electron's `second-instance` → `showWindow`).
fn show_on_second_launch(instance: Instance, window: AnyWindowHandle, cx: &mut App) {
    let (asked, mut asks) = futures::channel::mpsc::unbounded();
    // unguarded: desktop-host calls it on its waiter thread, under crash::catching
    instance.on_show(move || {
        let _ = asked.unbounded_send(());
    });
    cx.spawn(async move |cx| {
        guarded_async("listen for a second launch", async move {
            while asks.next().await.is_some() {
                cx.update(|cx| {
                    guarded("show the window", || {
                        window.update(cx, |_, window, _| window.activate_window())
                    });
                });
            }
        })
        .await
    })
    .detach();
}
