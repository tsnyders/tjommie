//! Keyboard focus that moves out of a scroll area's view brings its control back into view (Phase 3 plan T5.6, part of
//! the keyboard-only exit criterion). A scroll area (`follow_focus`) puts its scroll handle on a stack while its contents
//! prepaint, and the focus ring, which shows only for keyboard focus, scrolls the nearest area just enough to show its
//! control with a little room, as a browser's `scrollIntoView({ block: "nearest" })` does. A mouse focus draws no ring,
//! so it never scrolls anything.

use std::cell::RefCell;
use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels,
    ScrollHandle, SharedString, Styled as _, Window, canvas, point, px,
};

thread_local! {
    /// The scroll areas around what is prepainting now, the nearest last.
    static AREAS: RefCell<Vec<ScrollHandle>> = const { RefCell::new(Vec::new()) };
}

/// The room left between a revealed control and its area's edge, in px.
const ROOM: f32 = 16.;

/// A scroll handle kept across renders under `id`, for a scroll area.
pub fn scroll_handle(id: &str, window: &mut Window, cx: &mut App) -> ScrollHandle {
    window
        .use_keyed_state(SharedString::from(format!("{id}.scroll")), cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone()
}

/// A scroll area that keeps keyboard focus in view. `content` is the scrolling element, tracking `handle`
/// (`.track_scroll(&handle)`).
pub fn follow_focus(handle: &ScrollHandle, content: impl IntoElement) -> Scroller {
    Scroller {
        handle: handle.clone(),
        content: content.into_any_element(),
    }
}

/// See [`follow_focus`]. It lays out as its content, and adds nothing to what Windows reads.
pub struct Scroller {
    handle: ScrollHandle,
    content: AnyElement,
}

impl IntoElement for Scroller {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Scroller {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.content.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        AREAS.with(|areas| areas.borrow_mut().push(self.handle.clone()));
        self.content.prepaint(window, cx);
        AREAS.with(|areas| areas.borrow_mut().pop());
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
        self.content.paint(window, cx);
    }
}

/// Laid over a focus ring: scrolls the nearest scroll area so the ring shows.
pub(crate) fn reveal() -> impl IntoElement {
    canvas(|bounds, window, _| keep_in_view(bounds, window), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// Scrolls the nearest scroll area so `bounds` show with a little room. One taller than the view shows its top.
fn keep_in_view(bounds: Bounds<Pixels>, window: &mut Window) {
    let Some(area) = AREAS.with(|areas| areas.borrow().last().cloned()) else {
        return;
    };
    let view = area.bounds();
    let room = px(ROOM);
    // How far the control's top is above the view, and its bottom below it (each with the room).
    let above = view.top() - (bounds.top() - room);
    let below = (bounds.bottom() + room) - view.bottom();
    let shift = if above > px(0.) {
        above
    } else if below > px(0.) {
        // Up as far as the bottom needs, but never so far that the top leaves the view.
        let slack = px(0.) - above;
        px(0.) - if below < slack { below } else { slack }
    } else {
        return;
    };
    if shift.abs() < px(0.5) {
        return;
    }
    let offset = area.offset();
    area.set_offset(point(offset.x, offset.y + shift));
    window.refresh();
}
