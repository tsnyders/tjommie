//! The shared-element flight (the spike's flight; aegis/README.md "Shared elements across screens"): an element, such
//! as an agent's avatar, travels in a window-level overlay between two captured bounds, from a Recents row to the chat
//! header. x, y and size each ride the calm spring, so a retarget keeps its velocity. While it flies, both ends hide
//! their own copy; under reduced motion the ends cross-fade instead and nothing flies.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, Deferred, IntoElement, ParentElement as _, Pixels, Styled as _, canvas, deferred, div,
    point, px, size,
};

use super::spring::Spring;
use crate::theme;

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
