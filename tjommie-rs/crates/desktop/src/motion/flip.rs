//! FLIP (Phase 3 plan T2.3; limit 14: GPUI has no layout animation). When an element's place in a list changes, it
//! starts from where it was painted and springs to its new place, keeping any velocity it had: a Recents row that
//! moves to the top, a card that makes room. `Flip` keeps each row's place from the last frame, relative to its list
//! (so scrolling moves nothing), and its offset springs. `list` and `item` wrap the list and its rows, so their
//! prepaint reads and moves them; the owner steps the springs each frame (`Flip::step`) and keeps the clock going
//! while they move.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, SharedString, Window, point, px,
};

use super::spring::Spring;
use crate::theme;

pub struct Flip {
    rest: Spring,
    /// The list's origin this frame.
    origin: Point<Pixels>,
    rows: HashMap<SharedString, Row>,
}

struct Row {
    /// Where the row was laid out, relative to the list.
    place: Point<f32>,
    x: Spring,
    y: Spring,
    /// Placed since the last step; a row that wasn't has left the list.
    seen: bool,
}

impl Flip {
    /// Rows move on `tokens` (the theme's calm spring for a reorder).
    pub fn new(tokens: &theme::Spring) -> Self {
        Flip {
            rest: Spring::new(0., tokens),
            origin: Point::default(),
            rows: HashMap::new(),
        }
    }

    /// `key` is laid out at `place` (relative to the list) this frame: returns the offset to paint it at. A row that
    /// moved starts from where it was painted last frame (its old place plus its offset then) and springs to 0, its
    /// velocity kept; `snap` (a key, reduced motion) puts it in place at once. A new row starts in place.
    pub fn place(&mut self, key: &SharedString, place: Point<f32>, snap: bool) -> Point<f32> {
        let rest = self.rest;
        let row = self.rows.entry(key.clone()).or_insert_with(|| Row {
            place,
            x: rest,
            y: rest,
            seen: true,
        });
        row.seen = true;
        if row.place != place {
            row.x.value += row.place.x - place.x;
            row.y.value += row.place.y - place.y;
            row.place = place;
            if snap {
                row.x.snap(0.);
                row.y.snap(0.);
            }
        }
        point(row.x.value, row.y.value)
    }

    /// Where `key` was painted last frame, relative to the list: its place plus its offset then. A row that leaves
    /// fades out from there.
    pub fn painted(&self, key: &str) -> Option<Point<f32>> {
        self.rows
            .get(key)
            .map(|row| point(row.place.x + row.x.value, row.place.y + row.y.value))
    }

    /// Steps every row by `dt` and forgets the rows that left. Returns whether any still moves.
    pub fn step(&mut self, dt: f32) -> bool {
        self.rows
            .retain(|_, row| std::mem::replace(&mut row.seen, false));
        let mut moving = false;
        for row in self.rows.values_mut() {
            moving |= row.x.step(dt) | row.y.step(dt);
        }
        moving
    }
}

/// The list whose rows FLIP: their places are taken relative to it.
pub fn list(flip: Rc<RefCell<Flip>>, child: impl IntoElement) -> FlipList {
    FlipList {
        flip,
        child: child.into_any_element(),
    }
}

/// A row of a `list`, by `key`.
pub fn item(
    flip: Rc<RefCell<Flip>>,
    key: impl Into<SharedString>,
    snap: bool,
    child: impl IntoElement,
) -> FlipItem {
    FlipItem {
        flip,
        key: key.into(),
        snap,
        child: child.into_any_element(),
    }
}

pub struct FlipList {
    flip: Rc<RefCell<Flip>>,
    child: AnyElement,
}

pub struct FlipItem {
    flip: Rc<RefCell<Flip>>,
    key: SharedString,
    snap: bool,
    child: AnyElement,
}

impl IntoElement for FlipList {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl IntoElement for FlipItem {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FlipList {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.flip.borrow_mut().origin = bounds.origin;
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl Element for FlipItem {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let offset = {
            let mut flip = self.flip.borrow_mut();
            let relative = bounds.origin - flip.origin;
            let place = point(f32::from(relative.x), f32::from(relative.y));
            flip.place(&self.key, place, self.snap)
        };
        // A move is only found here, after the frame's render has stepped and asked for its frames: this one asks for
        // the next itself, so the slide starts without waiting for something else to draw.
        if offset != Point::default() {
            super::clock::keep_moving(window, cx);
        }
        window.with_element_offset(point(px(offset.x), px(offset.y)), |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn flip() -> Flip {
        Flip::new(&Theme::aegis().unwrap().motion.spring.calm)
    }

    #[test]
    fn a_moved_row_starts_where_it_was_painted() {
        let mut flip = flip();
        let row: SharedString = "forge".into();
        assert_eq!(
            flip.place(&row, point(0., 72.), false),
            point(0., 0.),
            "a new row starts in place"
        );
        flip.step(0.016);
        // To the top: laid out at 0, painted where it was (72) and springing up.
        assert_eq!(flip.place(&row, point(0., 0.), false), point(0., 72.));
        assert!(flip.step(0.016), "it moves");
        let partway = flip.place(&row, point(0., 0.), false).y;
        assert!((0. ..72.).contains(&partway), "on its way: {partway}");
        while flip.step(0.016) {
            flip.place(&row, point(0., 0.), false);
        }
        assert_eq!(
            flip.place(&row, point(0., 0.), false),
            point(0., 0.),
            "at rest in its place"
        );
    }

    #[test]
    fn a_move_mid_flight_keeps_where_it_is_painted() {
        let mut flip = flip();
        let row: SharedString = "scout".into();
        flip.place(&row, point(0., 0.), false);
        flip.step(0.016);
        flip.place(&row, point(0., 108.), false);
        for _ in 0..5 {
            flip.step(0.016);
            flip.place(&row, point(0., 108.), false);
        }
        let painted = 108. + flip.place(&row, point(0., 108.), false).y;
        // It moves again before landing: painted at the same spot, now relative to the new place.
        let offset = flip.place(&row, point(0., 36.), false).y;
        assert!(
            (36. + offset - painted).abs() < 1e-3,
            "no jump: {painted} -> {}",
            36. + offset
        );
    }

    #[test]
    fn a_keyboard_move_snaps_and_rows_that_left_are_forgotten() {
        let mut flip = flip();
        let row: SharedString = "quill".into();
        flip.place(&row, point(0., 0.), true);
        flip.step(0.016);
        assert_eq!(
            flip.place(&row, point(0., 36.), true),
            point(0., 0.),
            "snapped"
        );
        flip.step(0.016);
        flip.step(0.016);
        assert!(flip.rows.is_empty(), "not placed for a frame: gone");
    }
}
