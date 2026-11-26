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
            // A motion check (T7.2, test-driver builds) shows invented data in place of the daemon.
            #[cfg(feature = "test-driver")]
            let scenario = crate::scenario::options();
            #[cfg(feature = "test-driver")]
            let live = scenario.is_none();
            #[cfg(not(feature = "test-driver"))]
            let live = true;
            // The daemon and the connection (M5): until it's ready the shell says "Starting Tjommie". The daemon's browser,
            // Logins and worker clients start with it (`hosts`), on the engine the window makes below.
            if live {
                crate::live::start(identity, root.clone(), cx);
            }
            #[cfg(feature = "test-driver")]
            if let Some(options) = &scenario {
                crate::scenario::install(options, cx);
            }
            if after_crash {
                notices::raise(Notice::ClosedUnexpectedly);
            }
            match gpui_kit::open_window(window_options(&identity, cx), cx, |window, cx| {
                // This PC's bots' browsers live under the window; nothing of WebView2 starts until a page is shown.
                LocalBrowser::start(window, identity, cx);
                // What the vault's gate trusts a local request by, and a close that stops the daemon's clients first.
                if let Some(hosts) = cx.try_global::<crate::hosts::AppHosts>().cloned() {
                    crate::hosts::watch_window(&hosts, window, cx);
                }
                // The tray, the toasts, start at sign-in and the updater (M6), for this window; none for a motion check.
                if live {
                    crate::services::start(&identity, window, cx);
                    if after_crash && starts_in_the_tray() {
                        crate::services::tell_of_a_restart_in_the_tray(cx);
                    }
                }
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
                        if let Some(options) = scenario {
                            crate::scenario::start(options, window, cx);
                        }
                    }
                }
                Err(error) => {
                    log::error!("Could not open the window: {error:#}");
                    cx.quit();
                }
            }
        });
}

/// Windows starts the app at sign-in with `--hidden`, and the installer relaunches it after an update with `--updated`: both
/// start in the tray, with no window (`startsHidden`).
fn starts_in_the_tray() -> bool {
    let args: Vec<String> = std::env::args().collect();
    desktop_host::shell::login_item::starts_hidden(&args)
}

fn window_options(identity: &Identity, cx: &App) -> WindowOptions {
    WindowOptions {
        // A start in the tray shows no window (and takes no keyboard); the tray's Show brings it.
        show: !starts_in_the_tray(),
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
        focus: identity.name != "e2e" && !starts_in_the_tray(),
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
                        crate::services::show_window(window, cx)
                    });
                });
            }
        })
        .await
    })
    .detach();
}
