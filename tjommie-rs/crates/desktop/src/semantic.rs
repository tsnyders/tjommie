//! The app's semantic tree (Phase 3 plan T0.5). One call gives an element its GPUI id, its UI Automation identity
//! (AutomationId) and its accessible role, so Windows UI Automation and the test driver read the same semantics. With
//! the `test-driver` feature the element is also observed for the driver's snapshot; without it, the call returns the
//! plain element.
//!
//! Call it first on a `div()`, then build on as usual: `div().semantic("sidebar.wordmark", Role::Heading).aria_label(…)`.
//! Give a focusable element its focus handle after it (`.track_focus(&handle)`) so the snapshot sees its focus.

use gpui::{
    Element, InteractiveElement, Role, SharedString, Stateful, StatefulInteractiveElement as _,
};

#[cfg(feature = "test-driver")]
pub type Semantic<E> = crate::test_driver::Observed<E>;
#[cfg(not(feature = "test-driver"))]
pub type Semantic<E> = E;

pub trait SemanticExt: InteractiveElement + Element {
    fn semantic(self, id: impl Into<SharedString>, role: Role) -> Semantic<Stateful<Self>>;
}

impl<E: InteractiveElement + Element> SemanticExt for E {
    fn semantic(self, id: impl Into<SharedString>, role: Role) -> Semantic<Stateful<E>> {
        let id: SharedString = id.into();
        // View tests find the element by its id (`VisualTestContext::debug_bounds`); outside test builds, a no-op.
        let element = self
            .debug_selector(|| id.to_string())
            .id(id.clone())
            .accessibility_id(id.clone())
            .role(role);
        #[cfg(feature = "test-driver")]
        let element = crate::test_driver::Observed::new(element, id);
        element
    }
}
