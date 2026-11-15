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
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, WindowControlArea, div, px, svg,
};

use self::data::ShellData;
use self::route::Route;
use crate::guard::guarded;
use crate::keymap::{ZoomIn, ZoomOut, ZoomReset};
use crate::semantic::SemanticExt as _;
use crate::theme::Theme;
use crate::ui::style::{TypeExt as _, hsla, r, shadows};
use crate::zoom::Zoom;

/// The rem follows the zoom: gpui-kit's root sets the window's rem from its theme's font size every frame.
pub fn set_rem(zoom: Zoom, cx: &mut App) {
    gpui_kit::component::Theme::global_mut(cx).font_size = px(zoom.rem_px(1.0));
}

pub struct Shell {
    focus: FocusHandle,
    zoom: Zoom,
    /// The browser panel (T3.7), beside the main column while a chat shows it.
    panel: Option<AnyView>,
    _hold_focus: Subscription,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // Key bindings fire only while something in the window has focus (GPUI limit 17): the root takes it back.
        let hold_focus = cx.on_focus_lost(window, |this, window, cx| {
            guarded("hold focus", || this.focus.focus(window, cx));
        });
        Self {
            focus,
            zoom: Zoom::default(),
            panel: None,
            _hold_focus: hold_focus,
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
        let sidebar = sidebar::sidebar(&route, &data, window, cx);
        let shown = match connection::words(&data.connection, data.host.as_deref(), &data.saved) {
            Some(words) => connection::screen(&data, words, window, cx),
            None => screen(&route, &data, window, cx),
        };
        let status = connection::status_pill(&data.connection, window, cx);
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
            .child(crate::ui::primitives::toast::toasts(cx))
            .child(crate::motion::clock::painted())
    }
}

/// The route's screen. The screens come with their own tasks (M4, M5); until then each shows its name.
fn screen(route: &Route, data: &ShellData, window: &mut gpui::Window, cx: &mut App) -> AnyElement {
    let (key, title): (&str, SharedString) = match route {
        Route::Agents => return crate::ui::agents::screen(window, cx),
        Route::Projects => return crate::ui::projects::screen(window, cx),
        Route::Routines => return crate::ui::routines::screen(window, cx),
        Route::Settings(section) => return crate::ui::settings::screen(*section, window, cx),
        Route::Chat(id) => ("chat", data.name_of(id)),
        Route::Rooms => ("rooms", "Rooms".into()),
        Route::Room(id) => ("room", data.name_of(id)),
        Route::NewBot => ("new", "New agent".into()),
        Route::BotSettings(id) => (
            "bot_settings",
            format!("{} settings", data.name_of(id)).into(),
        ),
    };
    let theme = cx.global::<Theme>();
    div()
        .flex_1()
        .px(r(theme.space.s32))
        .pt(r(theme.space.s32))
        .child(
            div()
                .semantic(format!("screen.{key}"), Role::Heading)
                .aria_label(title.clone())
                .type_style(&theme.typography.desktop.display_m)
                .text_color(hsla(theme.color.light.text.primary))
                .child(title),
        )
        .into_any_element()
}

/// Minimize, maximize and close: Lucide at stroke 1.25, as the Aegis shell draws them.
fn caption_buttons(theme: &Theme) -> impl IntoElement {
    let colors = &theme.color.light;
    let button = |id: &'static str, label: &'static str, icon: &str, area: WindowControlArea| {
        div()
            .semantic(id, Role::Button)
            .aria_label(label)
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
