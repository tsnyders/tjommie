//! The desktop theme (spec Phase 0): `design/themes/aegis.json`, generated from the Aegis tokens by
//! `scripts/aegis-theme.ts` and read once at start. Lengths are px (callers apply the rem), times are ms. Keys are
//! theme-neutral, so the Workbench and Mission Control files will fill the same struct.

use gpui::Rgba;
use serde::Deserialize;

pub const AEGIS: &str = include_str!("../../../design/themes/aegis.json");

impl Theme {
    pub fn aegis() -> Result<Theme, serde_json::Error> {
        serde_json::from_str(AEGIS)
    }
}

/// The app's theme, set once at start.
impl gpui::Global for Theme {}

/// Every struct here refuses unknown keys, so a theme file can't carry a key the app never reads.
macro_rules! tokens {
    ($($(#[$doc:meta])* $name:ident { $($(#[$field_meta:meta])* $field:ident: $ty:ty),* $(,)? })*) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name { $($(#[$field_meta])* pub $field: $ty),* }
    )*};
}

tokens! {
    Theme {
        color: Colors,
        font: Fonts,
        #[serde(rename = "type")]
        typography: Typography,
        space: Space,
        radius: Radius,
        size: Size,
        shadow: Shadows,
        motion: Motion,
    }

    Colors { light: Palette, dark: Palette, bot: BotColors }
    /// One mode's colours, by role.
    Palette {
        surface: Surface,
        text: TextColors,
        ink: Ink,
        icon: IconColors,
        line: Lines,
        accent: Accent,
        action: Action,
        on: On,
        state: State,
        danger: Danger,
        overlay: Overlay,
        avatar: AvatarColors,
        brand: Brand,
    }
    Surface {
        background: Rgba,
        paper: Rgba,
        card: Rgba,
        raised_clear: Rgba,
        frame: Rgba,
        panel: Rgba,
        raised: Rgba,
        sunk: Rgba,
    }
    TextColors { primary: Rgba, secondary: Rgba, tertiary: Rgba }
    Ink { base: Rgba, mid: Rgba, muted: Rgba, hover: Rgba }
    IconColors { base: Rgba, muted: Rgba }
    Lines { base: Rgba, soft: Rgba, field: Rgba }
    /// The ochre: something is waiting for you.
    Accent { base: Rgba, text: Rgba, tint: Rgba, marker: Rgba }
    Action { primary: Rgba }
    /// Text and icons drawn on a fill.
    On { primary: Rgba, ink: Rgba, accent: Rgba }
    State { hover: Rgba, sunk_hover: Rgba, panel_hover: Rgba, pressed: Rgba, focus: Rgba }
    /// Red for what destroys or failed: the agents' red, darker in light and lighter in dark, so its text clears 4.5:1
    /// on the surfaces it sits on (`surface.sunk`, `state.sunkHover`, `surface.raised`; M5 review M7).
    Danger { text: Rgba }
    Overlay { scrim: Rgba, highlight: Rgba }
    AvatarColors { eye: Rgba, sweep: Rgba }
    /// The Tjommie mark: a body gradient from start to end, with eyes.
    Brand { mark_start: Rgba, mark_end: Rgba, mark_eye: Rgba }
    /// The agent colours, named as `protocol::BotColor`.
    BotColors {
        brown: Rgba,
        red: Rgba,
        orange: Rgba,
        amber: Rgba,
        green: Rgba,
        teal: Rgba,
        blue: Rgba,
        purple: Rgba,
        pink: Rgba,
        gray: Rgba,
    }

    /// Font family lists, the bundled family first.
    Fonts { text: Vec<String>, display: Vec<String>, mono: Vec<String> }
    Typography { desktop: TypeScale, phone: TypeScale }
    TypeScale {
        display_l: TextStyle,
        display_m: TextStyle,
        display_s: TextStyle,
        wordmark: TextStyle,
        body: TextStyle,
        body_strong: TextStyle,
        ui: TextStyle,
        ui_strong: TextStyle,
        nav: TextStyle,
        button: TextStyle,
        small: TextStyle,
        label: TextStyle,
        meta: TextStyle,
        code: TextStyle,
        figure: TextStyle,
    }
    TextStyle {
        font: FontRole,
        weight: f32,
        size: f32,
        line_height: f32,
        letter_spacing_em: f32,
        width_percent: f32,
    }

    Space {
        s0: f32,
        s2: f32,
        s4: f32,
        s6: f32,
        s8: f32,
        s10: f32,
        s12: f32,
        s14: f32,
        s16: f32,
        s20: f32,
        s24: f32,
        s28: f32,
        s32: f32,
        s40: f32,
        s48: f32,
        s56: f32,
        s64: f32,
    }
    Radius { xs: f32, sm: f32, field: f32, md: f32, slot: f32, card: f32, panel: f32, hero: f32, pill: f32 }
    Size {
        titlebar_height: f32,
        caption_button_width: f32,
        sidebar: f32,
        icon_button: f32,
        icon_button_min: f32,
        touch_min: f32,
        control_desktop: f32,
        control_phone: f32,
        avatar20: f32,
        avatar28: f32,
        avatar40: f32,
        avatar64: f32,
        badge28: f32,
        badge40: f32,
        badge64: f32,
        icon: f32,
        icon_sm: f32,
        focus_ring: f32,
        focus_offset: f32,
    }

    Shadows { light: ShadowSet, dark: ShadowSet }
    ShadowSet {
        glass: Vec<Shadow>,
        lift: Vec<Shadow>,
        raised: Vec<Shadow>,
        card: Vec<Shadow>,
        card_lift: Vec<Shadow>,
        raised_focus: Vec<Shadow>,
    }
    Shadow { x: f32, y: f32, blur: f32, spread: f32, color: Rgba }

    Motion {
        duration: Durations,
        easing: Easings,
        travel: Travel,
        stagger: Stagger,
        spring: Springs,
        distance: Distances,
        /// Fling speed in px per ms.
        flick: f32,
        eyes: Eyes,
        sweep: Sweep,
    }
    Durations {
        instant: f32,
        press: f32,
        release: f32,
        hover: f32,
        fade: f32,
        quick: f32,
        base: f32,
        settle: f32,
        slow: f32,
        entrance: f32,
        spin: f32,
        pulse: f32,
    }
    /// Cubic Béziers as `[x1, y1, x2, y2]`.
    Easings { out: [f32; 4], r#move: [f32; 4], pulse: [f32; 4], linear: [f32; 4] }
    /// duration = clamp(min_ms + distance_px × ms_per_px, min_ms, max_ms).
    Travel { min_ms: f32, max_ms: f32, ms_per_px: f32 }
    Stagger { tight: f32, step: f32, loose: f32 }
    Springs { calm: Spring, snappy: Spring, playful: Spring }
    /// Mass 1; damping follows from the ratio.
    Spring { stiffness: f32, damping_ratio: f32 }
    /// Scales are factors; the rest are px.
    Distances { press_scale: f32, press_fill: f32, lift: f32, rise: f32, enter: f32, recede_scale: f32, recede_x: f32 }
    Eyes { blink_min: f32, blink_max: f32, blink_close: f32, blink_open: f32, scan: f32, glance_hold: f32 }
    Sweep { duration: f32, every: f32 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontRole {
    Text,
    Display,
    Mono,
}
