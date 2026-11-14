//! The Aegis design tokens as GPUI styles: theme access, design px as rems (so they follow the app zoom), shadows,
//! type styles and the focus ring. Shadows stay in px and don't zoom (accepted, as in the spike).

use gpui::{
    App, BoxShadow, Div, FocusHandle, FontWeight, Hsla, Rems, Rgba, Styled, Window, div, point, px,
    rems,
};

use crate::assets::{DISPLAY_FAMILY, MONO_FAMILY, TEXT_FAMILY};
use crate::theme::{FontRole, Palette, Shadow, TextStyle, Theme};

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// The current mode's colours. Light until the appearance setting (T1.5, T5.5) picks the mode.
pub fn palette(cx: &App) -> &Palette {
    &theme(cx).color.light
}

/// A design length in px, as rems.
pub fn r(px: f32) -> Rems {
    rems(px / 16.0)
}

pub fn hsla(color: Rgba) -> Hsla {
    Hsla::from(color)
}

pub fn shadows(layers: &[Shadow]) -> Vec<BoxShadow> {
    layers
        .iter()
        .map(|layer| BoxShadow {
            color: hsla(layer.color),
            offset: point(px(layer.x), px(layer.y)),
            blur_radius: px(layer.blur),
            spread_radius: px(layer.spread),
            inset: false,
        })
        .collect()
}

pub fn family(role: FontRole) -> &'static str {
    match role {
        FontRole::Text => TEXT_FAMILY,
        FontRole::Display => DISPLAY_FAMILY,
        FontRole::Mono => MONO_FAMILY,
    }
}

pub trait TypeExt: Styled + Sized {
    /// One of the theme's type styles: family, weight, size and line height.
    fn type_style(self, style: &TextStyle) -> Self {
        self.font_family(family(style.font))
            .font_weight(FontWeight(style.weight))
            .text_size(r(style.size))
            .line_height(r(style.line_height))
    }
}

impl<E: Styled> TypeExt for E {}

/// Whether `focus` should show its ring: focused, and reached by keyboard.
pub fn focus_visible(focus: &FocusHandle, window: &Window) -> bool {
    focus.is_focused(window) && window.last_input_was_keyboard()
}

/// The Aegis focus ring (2 px ink, 2 px gap): a child laid over the element's edge, for an element with this corner
/// radius in design px. Add it to a `relative()` element while `focus_visible`.
pub fn focus_ring(radius: f32, cx: &App) -> Div {
    let size = &theme(cx).size;
    let outset = size.focus_ring + size.focus_offset;
    div()
        .absolute()
        .top(px(-outset))
        .left(px(-outset))
        .right(px(-outset))
        .bottom(px(-outset))
        .rounded(r(radius + outset))
        .border(px(size.focus_ring))
        .border_color(hsla(palette(cx).state.focus))
}
