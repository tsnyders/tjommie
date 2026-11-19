//! The Aegis design tokens as GPUI styles: theme access, design px as rems (so they follow the app zoom), shadows,
//! type styles and the focus ring. Shadows stay in px and don't zoom (accepted, as in the spike).

use desktop_host::device_settings::{Density, TextSize, Theme as Mode};
use gpui::{
    App, BoxShadow, Div, FocusHandle, FontWeight, Global, Hsla, ParentElement as _, Rems, Rgba, Styled,
    Window, WindowAppearance, div, point, px, rems,
};

use crate::assets::{DISPLAY_FAMILY, MONO_FAMILY, TEXT_FAMILY};
use crate::theme::{FontRole, Palette, Shadow, TextStyle, Theme};

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// The current mode's colours. `color.light` holds the mode shown: `show_appearance` puts the dark colours there in
/// dark mode, so every view follows the mode through it (and `shadow.light` the same way).
pub fn palette(cx: &App) -> &Palette {
    &theme(cx).color.light
}

/// How this device chose to look (Settings, Appearance; saved in `desktop-ui.json`): the mode, the text size and the
/// spacing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub mode: Mode,
    pub text: TextSize,
    pub density: Density,
}

impl Global for Appearance {}

impl Appearance {
    /// The rem's scale: Large is 112.5 % and Larger 125 % (`globals.css`).
    pub fn text_scale(self) -> f32 {
        match self.text {
            TextSize::System => 1.0,
            TextSize::Large => 1.125,
            TextSize::Larger => 1.25,
        }
    }

    pub fn compact(self) -> bool {
        self.density == Density::Compact
    }
}

/// The appearance shown (the default, comfortable and system-sized, before one is chosen).
pub fn appearance(cx: &App) -> Appearance {
    cx.try_global::<Appearance>().copied().unwrap_or_default()
}

/// The gap between cards in a grid: 16 px, or 8 px in compact spacing (`[data-slot="bot-list"]` in `globals.css`).
pub fn card_gap(cx: &App) -> Rems {
    if appearance(cx).compact() {
        r(8.)
    } else {
        r(theme(cx).space.s16)
    }
}

/// The Aegis theme as it loaded, with both modes: the theme shown is made from it.
struct Aegis(Theme);

impl Global for Aegis {}

/// Shows `chosen`: in dark mode the theme's current colours and shadows become the dark ones, the rem follows the
/// text size (on top of the zoom), and every window draws again. System follows Windows' app mode when chosen and at
/// start.
/// ponytail: a change of Windows' mode while the app runs shows at the next start or choice; `observe_window_appearance`
/// on the main window would follow it live.
pub fn show_appearance(chosen: Appearance, cx: &mut App) {
    let aegis = match cx.try_global::<Aegis>() {
        Some(aegis) => aegis.0.clone(),
        None => {
            let loaded = theme(cx).clone();
            cx.set_global(Aegis(loaded.clone()));
            loaded
        }
    };
    let dark = match chosen.mode {
        Mode::Dark => true,
        Mode::Light => false,
        Mode::System => matches!(
            cx.window_appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let mut shown = aegis;
    if dark {
        shown.color.light = shown.color.dark.clone();
        shown.shadow.light = shown.shadow.dark.clone();
    }
    let before = appearance(cx).text_scale();
    cx.set_global(shown);
    cx.set_global(chosen);
    let kit = gpui_kit::component::Theme::global_mut(cx);
    kit.font_size *= chosen.text_scale() / before;
    cx.refresh_windows();
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
/// radius in design px. Add it to a `relative()` element while `focus_visible`. It also scrolls its element into view
/// in the nearest scroll area (`primitives::scroll`, T5.6): keyboard focus never sits below the fold.
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
        .child(crate::ui::primitives::scroll::reveal())
}
