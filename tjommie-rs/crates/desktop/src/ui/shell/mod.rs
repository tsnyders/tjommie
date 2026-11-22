//! The app shell (Phase 3 plan T2.1; navigation-shell.html, `app-shell.tsx`): the stone frame, the sidebar, and the
//! main column, which shows the route's screen, or the connection screen while there is no connection, with the
//! browser panel's slot beside it. The caption buttons are drawn in GPUI over the window's control areas, so Windows
//! still handles them (snap layouts included).

pub mod connection;
pub mod data;
pub mod route;
mod sidebar;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, Context, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, WindowControlArea, div, px, svg,
};

use self::data::ShellData;
use self::route::Route;
use crate::guard::guarded;
use crate::keymap::{Dismiss, ZoomIn, ZoomOut, ZoomReset};
use crate::semantic::SemanticExt as _;
use crate::theme::Theme;
use crate::ui::style::{hsla, r, shadows};
use crate::zoom::Zoom;

/// Below this width, in design px, the window is narrow (`sm` in the TypeScript): the sidebar becomes the compact
/// navigation and the side panels (the browser panel, an exchange, a room's members) open as sheets over the content.
pub const NARROW: f32 = 640.;

/// Whether `window` is narrow: under 640 design px wide at its zoom.
pub fn narrow(window: &Window) -> bool {
    let zoom = f32::from(window.rem_size()) / 16.;
    f32::from(window.viewport_size().width) / zoom.max(0.1) < NARROW
}

/// The rem follows the zoom and the text size (Settings, Appearance): gpui-kit's root sets the window's rem from its
/// theme's font size every frame.
pub fn set_rem(zoom: Zoom, cx: &mut App) {
    let scale = crate::ui::style::appearance(cx).text_scale();
    gpui_kit::component::Theme::global_mut(cx).font_size = px(zoom.rem_px(1.0) * scale);
}

pub struct Shell {
    focus: FocusHandle,
    zoom: Zoom,
    /// The browser panel (T3.7), beside the main column while a chat shows it.
    panel: Option<AnyView>,
    _hold_focus: Subscription,
    _blur: Subscription,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // Key bindings fire only while something in the window has focus (GPUI limit 17): the root takes it back.
        let hold_focus = cx.on_focus_lost(window, |this, window, cx| {
            guarded("hold focus", || this.focus.focus(window, cx));
        });
        // Dictation that has heard something stops when the window loses focus, as 0.5.x's did (M4 re-review R2).
        let blur = cx.observe_window_activation(window, |_, window, cx| {
            guarded("stop dictation behind another window", || {
                if !window.is_window_active() {
                    crate::ui::chat::voice::blur(cx);
                }
            });
        });
        Self {
            focus,
            zoom: Zoom::default(),
            panel: None,
            _hold_focus: hold_focus,
            _blur: blur,
        }
    }

    /// Puts the browser panel beside the main column, or takes it away.
    pub fn set_panel(&mut self, panel: Option<AnyView>, cx: &mut Context<Self>) {
        self.panel = panel;
        cx.notify();
    }

    fn zoom_to(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        self.zoom = zoom;
        set_rem(zoom, cx);
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-driver"))]
        crate::test_driver::begin_frame();
        crate::motion::clock::begin_frame(cx);
        let route = route::current(cx);
        let data = cx.try_global::<ShellData>().cloned().unwrap_or_default();
        let narrow = narrow(window);
        let sidebar = if narrow {
            sidebar::compact(&route, window, cx)
        } else {
            sidebar::sidebar(&route, &data, window, cx)
        };
        let shown = match connection::words(&data.connection, data.host.as_deref(), &data.saved) {
            Some(words) => connection::screen(&data, words, window, cx),
            None => screen(&route, window, cx),
        };
        let status = connection::status_pill(&data.connection, window, cx);
        // The account notice a chat raised is the app's, as `sign-in-toast.tsx`'s viewport was: it stays through a
        // navigation for its 8 s (M4 review A9).
        let notice = crate::ui::chat::sign_in_toast::view(cx);
        let theme = cx.global::<Theme>();
        let colors = &theme.color.light;
        div()
            .semantic("root", Role::Group)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                guarded("zoom in", || this.zoom_to(this.zoom.zoom_in(), cx));
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                guarded("zoom out", || this.zoom_to(this.zoom.zoom_out(), cx));
            }))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| {
                guarded("reset the zoom", || this.zoom_to(Zoom::default(), cx));
            }))
            // Escape closes an open exchange first, wherever the focus is, giving the keyboard back to the link that
            // opened it; with none open, Escape goes on to the browser panel's handler (M4 review A5).
            .on_action(|_: &Dismiss, window, cx| {
                guarded("close the exchange by Escape", || {
                    if !crate::ui::chat::exchange::dismiss(window, cx) {
                        cx.propagate();
                    }
                });
            })
        let toasts = crate::ui::primitives::toast::toasts(window, cx);
            .size_full()
            .relative()
            .flex()
            .bg(hsla(colors.surface.frame))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .gap(r(theme.space.s12))
                    .pt(r(theme.size.titlebar_height))
                    .pr(r(theme.space.s12))
                    .pb(r(theme.space.s12))
                    .child(
                        div()
                            .id("main")
                            .flex_1()
                            .min_w_0()
                            .relative()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .bg(hsla(colors.surface.panel))
                            .rounded(r(theme.radius.panel))
                            .border_t_1()
                            .border_color(hsla(colors.overlay.highlight))
                            .shadow(shadows(&theme.shadow.light.glass))
                            .child(shown)
                            .children(status),
                    )
                    // An open exchange takes the right panel's place (T4.1); the panel stays drawn, closed, beside it.
                    .children(crate::ui::chat::exchange::side(cx))
                    .when_some(self.panel.clone(), |column, panel| column.child(panel)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right(px(3.0 * theme.size.caption_button_width))
                    .h(px(theme.size.titlebar_height))
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(caption_buttons(theme))
            .child(toasts)
            .children(notice.map(|notice| {
                // Under the chat header's line, at the main column's right.
                div()
                    .absolute()
                    .top(r(theme.size.titlebar_height + theme.space.s56))
                    .right(r(theme.space.s28))
                    .child(notice)
            }))
            .child(crate::motion::clock::painted())
    }
}

/// The route's screen.
fn screen(route: &Route, window: &mut gpui::Window, cx: &mut App) -> AnyElement {
    match route {
        Route::Agents => crate::ui::agents::screen(window, cx),
        Route::Projects => crate::ui::projects::screen(window, cx),
        Route::Routines => crate::ui::routines::screen(window, cx),
        Route::NewBot => crate::ui::bots::new::screen(window, cx),
        Route::BotSettings(id) => crate::ui::bots::settings::screen(id, window, cx),
        Route::Settings(section) => crate::ui::settings::screen(*section, window, cx),
        Route::Account { .. } => crate::ui::settings::screen(route::Section::Accounts, window, cx),
            .children(flight)
        Route::Rooms => crate::ui::rooms::list::screen(window, cx),
        Route::Room(id) => crate::ui::rooms::screen::screen(id, window, cx),
        Route::Chat(id) => crate::ui::chat::screen(id, window, cx),
    }
}

/// Minimize, maximize and close: Lucide at stroke 1.25, as the Aegis shell draws them.
fn caption_buttons(theme: &Theme) -> impl IntoElement {
    let colors = &theme.color.light;
    let button = |id: &'static str, label: &'static str, icon: &str, area: WindowControlArea| {
        div()
            .semantic(id, Role::Button)
            .aria_label(label)
            .children(flight)
            .w(px(theme.size.caption_button_width))
            .h(px(theme.size.titlebar_height))
            .flex()
            .items_center()
            .justify_center()
            .hover(|style| style.bg(hsla(colors.state.hover)))
            .window_control_area(area)
            .child(
                svg()
                    .path(format!("icons/i/125/{icon}.svg"))
                    .size(px(16.0))
                    .text_color(hsla(colors.icon.base)),
            )
    };
    div()
        .absolute()
        .top_0()
        .right_0()
        .flex()
        .child(button(
            "caption.minimize",
            "Minimize",
            "minus",
            WindowControlArea::Min,
        ))
        .child(button(
            "caption.maximize",
            "Maximize",
            "square",
            WindowControlArea::Max,
        ))
        .child(button(
            "caption.close",
            "Close",
            "x",
            WindowControlArea::Close,
        ))
}

#[cfg(test)]
mod tests {
    use super::data::{Connection, Recent, RecentKind};
    use super::route::Navigate as _;
    use super::*;
    use gpui::{AppContext as _, Modifiers, VisualTestContext};
    use protocol::model::{BotColor, BotShape, BotState};

    fn shell_window(cx: &mut gpui::TestAppContext, data: ShellData) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Theme::aegis().unwrap());
            cx.set_global(data);
            crate::notices::init(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let shell = cx.new(|cx| Shell::new(window, cx));
            gpui_kit::component::Root::new(shell, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    fn forge() -> Recent {
        Recent {
            id: "bot-1".into(),
            name: "Forge".into(),
            kind: RecentKind::Bot {
                color: BotColor::Blue,
                shape: BotShape::Hexagon,
                state: BotState::Waiting,
            },
        }
    }

    #[gpui::test]
    fn navigates_to_chat_from_recents(cx: &mut gpui::TestAppContext) {
        let cx = shell_window(
            cx,
            ShellData {
                connection: Connection::Ready,
                recents: vec![forge()],
                ..ShellData::default()
            },
        );
        assert!(
            cx.debug_bounds("screen.agents").is_some(),
            "the app opens on Agents"
        );

        let forge = cx
            .debug_bounds("sidebar.recent.bot-1")
            .expect("Forge's row");
        cx.simulate_click(forge.center(), Modifiers::none());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(
            cx.update(|_, cx| route::current(cx)),
            Route::Chat("bot-1".into())
        );
        assert!(
            cx.debug_bounds("screen.chat").is_some(),
            "Forge's chat shows"
        );
        assert!(cx.debug_bounds("screen.agents").is_none());
    }

    #[gpui::test]
    fn the_connection_screen_takes_the_main_columns_place(cx: &mut gpui::TestAppContext) {
        let cx = shell_window(cx, ShellData::default());
        assert!(
            cx.debug_bounds("connection.title").is_some(),
            "connecting: the connection screen"
        );
        assert!(cx.debug_bounds("screen.agents").is_none());
        // The sidebar still works while connecting.
        assert!(cx.debug_bounds("sidebar.nav.rooms").is_some());

        cx.update(|_, cx| {
            cx.global_mut::<ShellData>().connection = Connection::Offline {
                reason: data::OfflineReason::Unreachable,
            };
            cx.navigate(Route::Rooms);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("screen.rooms").is_some(),
            "offline: the cached screen stays"
        );
        assert!(
            cx.debug_bounds("shell.status").is_some(),
            "under the status pill"
        );
    }
}
    fn recents_slide_to_new_places_and_snap_under_reduced_motion(cx: &mut gpui::TestAppContext) {
        use crate::ui::testing::next_frame;
        let room = |id: &str| Recent {
            id: id.to_owned().into(),
            name: id.to_owned().into(),
            kind: RecentKind::Room { unread: false },
        };
        let cx = shell_window(
            cx,
            ShellData {
                connection: Connection::Ready,
                recents: vec![room("launch"), room("everyone"), room("design")],
                ..ShellData::default()
            },
        );
        let draw = |cx: &mut VisualTestContext| cx.update(|window, cx| window.draw(cx).clear(cx));
        // An inactive window would get its frames at 30 fps by timer instead (gate condition 5).
        cx.update(|window, cx| {
            window.activate_window();
            crate::motion::clock::fixed_step(1. / 60., cx);
        });
        cx.run_until_parked();
        draw(cx);
        let settle = |cx: &mut VisualTestContext| {
            for _ in 0..300 {
                if !next_frame(cx) {
                    return;
                }
            }
            panic!("still moving after 300 frames");
        };
        settle(cx);
        let before = cx.debug_bounds("sidebar.recent.launch").expect("Launch's row");
        cx.update(|_, cx| cx.global_mut::<ShellData>().recents.rotate_left(1));
        draw(cx);
        let moved = cx.debug_bounds("sidebar.recent.launch").expect("Launch's row");
        assert!(
            (moved.origin.y - before.origin.y).abs() < gpui::px(1.),
            "on the first frame Launch is painted where it was: {before:?} -> {moved:?}"
        );
        assert!(next_frame(cx), "then the rows slide to their new places");
        settle(cx);
        let landed = cx.debug_bounds("sidebar.recent.launch").expect("Launch's row");
        assert!(landed.origin.y > before.origin.y, "Launch ends at the bottom");

        cx.update(|_, cx| crate::motion::reduced::set_for_test(true, cx));
        cx.update(|_, cx| cx.global_mut::<ShellData>().recents.rotate_left(1));
        draw(cx);
        assert!(!next_frame(cx), "reduced: the rows are in place at once");
    }

    #[gpui::test]
