//! Port of `packages/ui/lib/panel-layout.ts` (T1.4): the right panel's width rules (bounded, remembered, and an overlay when
//! the content area is narrow), and which pane the panel shows for one bot. Widths are CSS pixels as `f64`, the
//! TypeScript's numbers; `Math.round` halves go up.

pub const CHAT_MIN: f64 = 480.0; // the chat always keeps this much beside the panel
pub const OVERLAY_BELOW: f64 = 1000.0; // a narrower content area gives the panel the chat's place, over it
const OVERLAY_EDGE: f64 = 48.0; // the strip of chat left showing beside an overlay
const KEY_STEP: f64 = 16.0;
const KEY_STEP_BIG: f64 = 64.0;
const UNMEASURED_MAX: f64 = 900.0; // the largest width before the content area is measured
pub const PHONE_BELOW: f64 = 640.0; // a viewport narrower than this is a phone (Tailwind's sm); the panel is a full-screen sheet

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Details,
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneSpec {
    pub fallback: f64,
    pub min: f64,
}

/// `PANES`: the width a pane starts at, and the least it may be.
pub fn pane_spec(pane: Pane) -> PaneSpec {
    match pane {
        Pane::Details => PaneSpec {
            fallback: 460.0,
            min: 400.0,
        },
        Pane::Browser => PaneSpec {
            fallback: 560.0,
            min: 420.0,
        },
    }
}

/// `Math.round`: halves go up.
fn round_half_up(value: f64) -> f64 {
    (value + 0.5).floor()
}

pub fn is_phone(viewport_width: f64) -> bool {
    viewport_width > 0.0 && viewport_width < PHONE_BELOW
}

pub fn is_overlay(room: f64) -> bool {
    room > 0.0 && room < OVERLAY_BELOW
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: f64,
    pub max: f64,
}

/// The smallest and largest width the panel may have, given the room (the content area's width; 0 until measured).
pub fn width_bounds(pane: Pane, room: f64) -> Bounds {
    let min = pane_spec(pane).min;
    let max = if room <= 0.0 {
        UNMEASURED_MAX
    } else if is_overlay(room) {
        room - OVERLAY_EDGE
    } else {
        room - CHAT_MIN
    };
    Bounds {
        min,
        max: max.max(min),
    }
}

pub fn clamp_width(width: f64, bounds: Bounds) -> f64 {
    bounds.max.min(bounds.min.max(round_half_up(width)))
}

/// The separator sits on the panel's left edge: Left makes the panel wider, Right narrower. `None` for other keys.
pub fn splitter_width(key: &str, shift: bool, width: f64, bounds: Bounds) -> Option<f64> {
    let step = if shift { KEY_STEP_BIG } else { KEY_STEP };
    match key {
        "ArrowLeft" => Some(clamp_width(width + step, bounds)),
        "ArrowRight" => Some(clamp_width(width - step, bounds)),
        "Home" => Some(bounds.min),
        "End" => Some(bounds.max),
        _ => None,
    }
}

/// The `getItem`/`setItem` the widths are kept in (`localStorage` in the app).
pub trait WidthStorage {
    fn get_item(&self, key: &str) -> Option<String>;
    fn set_item(&mut self, key: &str, value: &str);
}

fn width_key(pane: Pane) -> String {
    let name = match pane {
        Pane::Details => "details",
        Pane::Browser => "browser",
    };
    format!("tjommie.panel-width.{name}")
}

/// The remembered width, or the pane's default for nothing, or for a value that isn't a positive number.
pub fn read_width(storage: &impl WidthStorage, pane: Pane) -> f64 {
    let stored = storage
        .get_item(&width_key(pane))
        .and_then(|value| value.trim().parse::<f64>().ok());
    match stored {
        Some(width) if width.is_finite() && width > 0.0 => width,
        _ => pane_spec(pane).fallback,
    }
}

pub fn save_width(storage: &mut impl WidthStorage, pane: Pane, width: f64) {
    storage.set_item(&width_key(pane), &round_half_up(width).to_string());
}

/// Which pane the right panel shows for one bot, and whether the browser fills the content area. Another bot starts at
/// Details.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelView {
    pub bot: String,
    pub pane: Pane,
    pub expanded: bool,
}

pub fn panel_for(view: &PanelView, bot: &str) -> PanelView {
    if view.bot == bot {
        view.clone()
    } else {
        PanelView {
            bot: bot.to_owned(),
            pane: Pane::Details,
            expanded: false,
        }
    }
}

/// Each change starts from the latest view (pass these to a state setter), so two in a row can't undo each other. Leaving
/// the browser always docks it.
pub fn choose_pane(view: &PanelView, bot: &str, pane: Pane) -> PanelView {
    let current = panel_for(view, bot);
    let expanded = pane == Pane::Browser && current.expanded;
    PanelView {
        bot: current.bot,
        pane,
        expanded,
    }
}

pub fn set_expanded(view: &PanelView, bot: &str, expanded: bool) -> PanelView {
    PanelView {
        expanded,
        ..panel_for(view, bot)
    }
}

/// The chat is covered (and so inert) only by what is really showing: an overlay, or an expanded browser the bot still has.
pub fn covers_chat(shown: bool, overlay: bool, view: &PanelView, browser: bool) -> bool {
    shown && (overlay || (browser && view.pane == Pane::Browser && view.expanded))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    impl WidthStorage for HashMap<String, String> {
        fn get_item(&self, key: &str) -> Option<String> {
            self.get(key).cloned()
        }

        fn set_item(&mut self, key: &str, value: &str) {
            self.insert(key.to_owned(), value.to_owned());
        }
    }

    fn bounds(min: f64, max: f64) -> Bounds {
        Bounds { min, max }
    }

    fn docked() -> PanelView {
        PanelView {
            bot: "b1".to_owned(),
            pane: Pane::Browser,
            expanded: false,
        }
    }

    #[test]
    fn keeps_the_chat_minimum_beside_the_panel_and_the_panes_own_minimum() {
        assert_eq!(width_bounds(Pane::Browser, 1400.0), bounds(420.0, 920.0));
        assert_eq!(width_bounds(Pane::Details, 1400.0), bounds(400.0, 920.0));
        assert_eq!(width_bounds(Pane::Details, 0.0).max, 900.0);
    }

    #[test]
    fn becomes_an_overlay_below_1000_px_with_a_strip_of_chat_left_showing() {
        assert_eq!(
            [is_overlay(999.0), is_overlay(1000.0), is_overlay(0.0)],
            [true, false, false]
        );
        assert_eq!(width_bounds(Pane::Browser, 700.0), bounds(420.0, 652.0));
        assert_eq!(width_bounds(Pane::Browser, 450.0).max, 420.0);
    }

    #[test]
    fn clamps_a_width_into_its_bounds() {
        let limits = bounds(420.0, 920.0);
        assert_eq!(
            [
                clamp_width(100.0, limits),
                clamp_width(600.4, limits),
                clamp_width(5000.0, limits)
            ],
            [420.0, 600.0, 920.0]
        );
    }

    #[test]
    fn steps_with_the_arrow_keys_by_64_with_shift_and_jumps_with_home_and_end() {
        let limits = bounds(420.0, 920.0);
        assert_eq!(
            splitter_width("ArrowLeft", false, 560.0, limits),
            Some(576.0)
        );
        assert_eq!(
            splitter_width("ArrowRight", true, 560.0, limits),
            Some(496.0)
        );
        assert_eq!(
            splitter_width("ArrowRight", false, 425.0, limits),
            Some(420.0)
        );
        assert_eq!(splitter_width("Home", false, 560.0, limits), Some(420.0));
        assert_eq!(splitter_width("End", false, 560.0, limits), Some(920.0));
        assert_eq!(splitter_width("a", false, 560.0, limits), None);
    }

    #[test]
    fn remembers_a_width_per_pane_and_falls_back_to_the_default_for_nothing_or_junk() {
        let mut store: HashMap<String, String> = HashMap::new();
        assert_eq!(
            [
                read_width(&store, Pane::Browser),
                read_width(&store, Pane::Details)
            ],
            [560.0, 460.0]
        );
        save_width(&mut store, Pane::Browser, 640.4);
        assert_eq!(read_width(&store, Pane::Browser), 640.0);
        assert_eq!(read_width(&store, Pane::Details), 460.0);
        store.insert("tjommie.panel-width.details".to_owned(), "junk".to_owned());
        assert_eq!(read_width(&store, Pane::Details), 460.0);
    }

    #[test]
    fn is_a_viewport_narrower_than_640_px_and_unknown_0_is_not_a_phone() {
        assert_eq!(PHONE_BELOW, 640.0);
        assert_eq!([360.0, 390.0, 639.0].map(is_phone), [true, true, true]);
        assert_eq!([640.0, 999.0, 1440.0, 0.0].map(is_phone), [false; 4]);
    }

    #[test]
    fn docks_the_browser_when_leaving_it_so_expand_details_browser_comes_back_docked() {
        let mut view = set_expanded(&docked(), "b1", true);
        view = choose_pane(&view, "b1", Pane::Details);
        assert_eq!(
            view,
            PanelView {
                bot: "b1".to_owned(),
                pane: Pane::Details,
                expanded: false
            }
        );
        view = choose_pane(&view, "b1", Pane::Browser);
        assert_eq!(view, docked());
    }

    #[test]
    fn keeps_an_expanded_browser_expanded_when_the_browser_is_chosen_again() {
        let expanded = set_expanded(&docked(), "b1", true);
        assert!(choose_pane(&expanded, "b1", Pane::Browser).expanded);
    }

    #[test]
    fn starts_another_bot_at_details() {
        assert_eq!(
            panel_for(&docked(), "b2"),
            PanelView {
                bot: "b2".to_owned(),
                pane: Pane::Details,
                expanded: false
            }
        );
        assert_eq!(
            choose_pane(&docked(), "b2", Pane::Browser),
            PanelView {
                bot: "b2".to_owned(),
                pane: Pane::Browser,
                expanded: false
            }
        );
    }

    #[test]
    fn covers_the_chat_for_an_overlay_or_an_expanded_browser_only_while_the_bot_has_a_browser() {
        let expanded = PanelView {
            expanded: true,
            ..docked()
        };
        assert!(covers_chat(true, false, &expanded, true));
        assert!(!covers_chat(true, false, &expanded, false));
        assert!(!covers_chat(true, false, &docked(), true));
        assert!(covers_chat(true, true, &docked(), false));
        assert!(!covers_chat(false, true, &expanded, true));
    }
}
