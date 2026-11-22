//! The shared-element flight (the spike's flight; aegis/README.md "Shared elements across screens"): an element, such
//! as an agent's avatar, travels in a window-level overlay between two captured bounds, from a Recents row to the chat
//! header. x, y and size each ride the calm spring, so a retarget keeps its velocity. While it flies, both ends hide
//! their own copy; under reduced motion the ends cross-fade instead and nothing flies.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Deferred, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, SharedString, Styled as _, Window, canvas, deferred, div, point,
    px, size,
};
use protocol::model::{BotColor, BotShape};

use super::clock;
use super::keyboard::Trigger;
use super::spring::Spring;
use crate::theme::{self, Theme};
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::shell::route::Route;

/// How many frames a flight waits for the chat header to be painted before it gives up and lets the header show.
const WAIT_FRAMES: u32 = 30;

/// The avatar flying into a chat (aegis/README.md "Shared elements across screens"): opened from a Recents row or an
/// Agents card by the pointer, the agent's avatar flies from there into the chat header. Each end records where it was
/// painted, and while the avatar flies both ends hide their own copy. A key or reduced motion opens the chat with no
/// flight: the header simply shows.
#[derive(Default)]
pub struct SharedAvatar {
    /// Where each end's avatar was painted in the last frame, by `end` key.
    ends: HashMap<SharedString, Bounds<Pixels>>,
    /// The ends painted in this frame so far.
    painted: HashMap<SharedString, Bounds<Pixels>>,
    flying: Option<Flying>,
}

impl Global for SharedAvatar {}

struct Flying {
    id: SharedString,
    /// The end it left, which hides its own copy while it flies.
    from: SharedString,
    /// Where it took off, captured once: an Agents card's screen is gone from the chat's first frame on (M7 review M1).
    start: Bounds<Pixels>,
    look: (BotColor, BotShape, Pose),
    flight: Option<Flight>,
    frames: u32,
}

/// The key of agent `id`'s avatar at `place`: `recents`, `card` or `header`.
pub fn end(place: &str, id: &str) -> SharedString {
    format!("{place}:{id}").into()
}

/// Records where the avatar at end `key` is painted: a child of the avatar's (relative) box.
pub fn end_capture(key: SharedString) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            cx.default_global::<SharedAvatar>()
                .painted
                .insert(key, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// Whether the avatar at end `key` hides while the flight carries agent `id`: the end it left and the header.
pub fn hides(id: &str, key: &str, cx: &App) -> bool {
    cx.try_global::<SharedAvatar>()
        .and_then(|shared| shared.flying.as_ref())
        .is_some_and(|flying| {
            *flying.id == *id && (*flying.from == *key || *key == *end("header", id))
        })
}

/// Whether an avatar is flying (the motion checks).
pub fn is_flying(cx: &App) -> bool {
    cx.try_global::<SharedAvatar>()
        .is_some_and(|shared| shared.flying.is_some())
}

/// Agent `id`'s chat is being opened from its avatar at end `from`: after the pointer, the avatar takes off for the
/// chat header. A key or reduced motion opens it with no flight.
pub fn take_off(
    id: &str,
    from: SharedString,
    look: (BotColor, BotShape, Pose),
    window: &Window,
    cx: &mut App,
) {
    if Trigger::of(window).snaps(cx) {
        return;
    }
    let shared = cx.default_global::<SharedAvatar>();
    if let Some(start) = shared.ends.get(&from).copied() {
        shared.flying = Some(Flying {
            id: id.to_owned().into(),
            from,
            start,
            look,
            flight: None,
            frames: 0,
        });
    }
}

/// The frame's flight, called by the shell as each frame starts: it takes off from where its end was painted when it
/// left (that end need not be painted again), heads for the header once the header has been painted, and lands there.
/// Returns the overlay to draw while it flies.
pub fn frame(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let calm = cx.global::<Theme>().motion.spring.calm.clone();
    let dt = clock::dt(cx);
    let zoom = f32::from(window.rem_size()) / 16.;
    let shared = cx.default_global::<SharedAvatar>();
    shared.ends = std::mem::take(&mut shared.painted);
    let flying = shared.flying.as_mut()?;
    flying.frames += 1;
    let header = shared.ends.get(&end("header", &flying.id)).copied();
    let on_chat = matches!(route, Route::Chat(id) if **id == *flying.id);
    if !on_chat || (header.is_none() && flying.frames > WAIT_FRAMES) {
        shared.flying = None;
        return None;
    }
    let start = flying.start;
    let flight = flying
        .flight
        .get_or_insert_with(|| Flight::launch(start, header.unwrap_or(start), &calm));
    if let Some(header) = header {
        flight.retarget(header);
    }
    if !flight.step(dt) && header.is_some() {
        // Landed: the header shows its own copy from this frame on.
        shared.flying = None;
        return None;
    }
    let (color, shape, pose) = flying.look;
    // The avatar's size is in design px, which the zoom scales.
    let side = f32::from(flight.bounds().size.width) / zoom;
    let overlay = flight
        .overlay(
            div()
                .debug_selector(|| "flight.avatar".into())
                .size_full()
                .child(BotAvatar::new(color, shape, side).pose(pose)),
        )
        .into_any_element();
    clock::keep_moving(window, cx);
    Some(overlay)
}

/// Where an element was painted last frame, for a flight to start or land: `capture` keeps it up to date.
pub type Captured = Rc<Cell<Option<Bounds<Pixels>>>>;

/// An element that records the bounds of its parent into `into` each frame: add it as a child.
pub fn capture(into: Captured) -> impl IntoElement {
    canvas(move |bounds, _, _| into.set(Some(bounds)), |_, _, _, _| {})
        .absolute()
        .inset_0()
}

pub struct Flight {
    x: Spring,
    y: Spring,
    size: Spring,
}

impl Flight {
    /// Takes off from `from` for `to`, on `tokens` (the theme's calm spring).
    pub fn launch(from: Bounds<Pixels>, to: Bounds<Pixels>, tokens: &theme::Spring) -> Self {
        let mut flight = Flight {
            x: Spring::new(f32::from(from.origin.x), tokens),
            y: Spring::new(f32::from(from.origin.y), tokens),
            size: Spring::new(f32::from(from.size.width), tokens),
        };
        flight.retarget(to);
        flight
    }

    /// Heads for `to` from wherever it is, keeping its velocity.
    pub fn retarget(&mut self, to: Bounds<Pixels>) {
        self.x.set_target(f32::from(to.origin.x));
        self.y.set_target(f32::from(to.origin.y));
        self.size.set_target(f32::from(to.size.width));
    }

    /// Steps by `dt`; whether it still flies.
    pub fn step(&mut self, dt: f32) -> bool {
        self.x.step(dt) | self.y.step(dt) | self.size.step(dt)
    }

    /// Where it is now: a square of its current size.
    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(self.x.value), px(self.y.value)),
            size: size(px(self.size.value), px(self.size.value)),
        }
    }

    /// The overlay: `content`, sized to fill, at the flight's place over everything else. Add it to the root.
    pub fn overlay(&self, content: impl IntoElement) -> Deferred {
        let bounds = self.bounds();
        deferred(
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .size(bounds.size.width)
                .child(content),
        )
        .with_priority(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn square(x: f32, y: f32, side: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(side), px(side)),
        }
    }

    #[test]
    fn flies_from_the_row_to_the_header() {
        let calm = Theme::aegis().unwrap().motion.spring.calm;
        let row = square(28., 360., 20.);
        let header = square(300., 40., 40.);
        let mut flight = Flight::launch(row, header, &calm);
        assert_eq!(flight.bounds(), row, "it takes off from the row");
        flight.step(0.016);
        let partway = flight.bounds();
        assert!(partway.origin.x > row.origin.x && partway.origin.x < header.origin.x);
        while flight.step(0.016) {}
        assert_eq!(flight.bounds(), header, "and lands on the header");
    }

    #[test]
    fn a_recall_mid_flight_keeps_its_velocity() {
        let calm = Theme::aegis().unwrap().motion.spring.calm;
        let row = square(28., 360., 20.);
        let mut flight = Flight::launch(row, square(300., 40., 40.), &calm);
        for _ in 0..8 {
            flight.step(0.016);
        }
        let (at, heading) = (flight.bounds().origin.x, flight.x.velocity);
        flight.retarget(row);
        assert_eq!(
            (flight.bounds().origin.x, flight.x.velocity),
            (at, heading),
            "it turns, it doesn't jump"
        );
        while flight.step(0.016) {}
        assert_eq!(flight.bounds(), row, "back home");
    }
}
