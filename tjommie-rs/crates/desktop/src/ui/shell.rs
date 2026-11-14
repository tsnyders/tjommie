//! The app skeleton (Phase 3 plan T0.5): the stone frame, the sidebar with the wordmark, and caption buttons drawn in
//! GPUI over the window's control areas, so Windows still handles them (snap layouts included). T2.1 builds the shell
//! on it.

use gpui::{
    App, Context, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Rems, Render, Role, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, WindowControlArea, actions, div, px, rems, svg,
};

use crate::assets::DISPLAY_FAMILY;
use crate::guard::guarded;
use crate::semantic::SemanticExt as _;
use crate::theme::Theme;
use crate::zoom::Zoom;

actions!(desktop, [ZoomIn, ZoomOut, ZoomReset]);

/// Ctrl+= (or Ctrl++) and Ctrl+- step the zoom, Ctrl+0 resets it. T2.4 moves them into the keymap.
pub fn zoom_bindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ZoomReset, None),
    ]
}

/// The rem follows the zoom: gpui-kit's root sets the window's rem from its theme's font size every frame.
pub fn set_rem(zoom: Zoom, cx: &mut App) {
    gpui_kit::component::Theme::global_mut(cx).font_size = px(zoom.rem_px(1.0));
}

/// A design length in px, as rems, so it scales with the zoom.
fn r(px: f32) -> Rems {
    rems(px / 16.0)
}

pub struct Shell {
    focus: FocusHandle,
    zoom: Zoom,
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
            _hold_focus: hold_focus,
        }
    }

    fn zoom_to(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        self.zoom = zoom;
        set_rem(zoom, cx);
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "test-driver")]
        crate::test_driver::begin_frame();
        let theme = cx.global::<Theme>();
        let colors = &theme.color.light;
        let wordmark = &theme.typography.desktop.wordmark;
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
            .bg(Hsla::from(colors.surface.frame))
            .child(
                div()
                    .w(r(theme.size.sidebar))
                    .h_full()
                    .flex()
                    .flex_col()
                    .pt(r(theme.size.titlebar_height))
                    .px(r(theme.space.s20))
                    .child(
                        div()
                            .semantic("sidebar.wordmark", Role::Heading)
                            .aria_label("Tjommie")
                            .font_family(DISPLAY_FAMILY)
                            .font_weight(FontWeight(wordmark.weight))
                            .text_size(r(wordmark.size))
                            .line_height(r(wordmark.line_height))
                            .text_color(colors.text.primary)
                            .child("Tjommie"),
                    ),
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
    }
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
            .hover(|style| style.bg(Hsla::from(colors.state.hover)))
            .window_control_area(area)
            .child(
                svg()
                    .path(format!("icons/i/125/{icon}.svg"))
                    .size(px(16.0))
                    .text_color(colors.icon.base),
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
