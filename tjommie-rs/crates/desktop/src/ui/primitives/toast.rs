//! Toasts (navigation-shell `.toasts`, overlays `.toast`): the `Notices` global's notices, as near-black pills in a
//! column at the top centre, under the header. Each closes from its own button.
//!
//! Their motion (motion.html "Toast and sheet"): a toast drops 16 px in from the top edge on the playful spring as it
//! fades in, and leaves the way it came, faster: 8 px up and out in the quick 200 ms. A key or reduced motion shows and
//! closes it at once.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Global, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use super::button::{IconButton, IconVariant};
use crate::motion::eyes::bezier;
use crate::motion::keyboard::Trigger;
use crate::motion::spring::Spring;
use crate::motion::{clock, reduced};
use crate::notices::{self, Notice, Notices};
use crate::semantic::SemanticExt as _;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// How far a toast drops in, and rises out, in design px.
const DROP: f32 = 16.;
const RISE: f32 = 8.;

#[derive(Default)]
struct ToastMotion {
    /// Each shown notice's drop: 1 a whole drop above its place, 0 in it.
    arriving: Vec<(Notice, Spring)>,
    /// Closed notices on their way out, and how far (0 to 1 of the quick duration).
    leaving: Vec<(Notice, f32)>,
}

impl Global for ToastMotion {}

/// Steps the toasts' motion for this frame; whether any still moves.
fn step(shown: &[Notice], cx: &mut App) -> bool {
    let playful = theme(cx).motion.spring.playful.clone();
    let quick = theme(cx).motion.duration.quick / 1000.;
    let snap = reduced::is_on(cx);
    let dt = clock::dt(cx);
    let motion = cx.default_global::<ToastMotion>();
    motion.arriving.retain(|(notice, _)| shown.contains(notice));
    for notice in shown {
        if !motion.arriving.iter().any(|(each, _)| each == notice) {
            let mut drop = Spring::scaled(1., &playful, DROP);
            if snap {
                drop.snap(0.);
            } else {
                drop.set_target(0.);
            }
            motion.arriving.push((*notice, drop));
        }
    }
    let mut moving = false;
    for (_, drop) in &mut motion.arriving {
        moving |= drop.step(dt);
    }
    for (_, out) in &mut motion.leaving {
        *out += dt / quick.max(0.001);
    }
    motion.leaving.retain(|(_, out)| *out < 1.);
    moving || !motion.leaving.is_empty()
}

/// Whether a toast is on its way in or out (the motion checks).
pub fn moving(cx: &App) -> bool {
    cx.try_global::<ToastMotion>().is_some_and(|motion| {
        !motion.leaving.is_empty() || motion.arriving.iter().any(|(_, drop)| !drop.at_rest())
    })
}

/// Closes the toast at `index`: after the pointer it rises out, after a key it is gone at once.
fn close(index: usize, window: &Window, cx: &mut App) {
    let notice = cx
        .try_global::<Notices>()
        .and_then(|notices| notices.0.get(index).copied());
    if let Some(notice) = notice
        && !Trigger::of(window).snaps(cx)
    {
        cx.default_global::<ToastMotion>()
            .leaving
            .push((notice, 0.));
    }
    notices::dismiss(index, cx);
}

/// The toast layer, laid over the window; empty when nothing is raised.
pub fn toasts(window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let notices = cx
        .try_global::<Notices>()
        .map(|notices| notices.0.clone())
        .unwrap_or_default();
    if step(&notices, cx) {
        clock::mark(clock::TOAST, cx);
        clock::keep_moving(window, cx);
    }
    let (drops, leaving) = cx
        .try_global::<ToastMotion>()
        .map(|motion| {
            let drops: Vec<f32> = notices
                .iter()
                .map(|notice| {
                    motion
                        .arriving
                        .iter()
                        .find(|(each, _)| each == notice)
                        .map_or(0., |(_, drop)| drop.value)
                })
                .collect();
            (drops, motion.leaving.clone())
        })
        .unwrap_or_default();
    let theme = theme(cx);
    let colors = palette(cx);
    let out_curve = theme.motion.easing.out;
    let pill = || {
        div()
            .relative()
            .flex()
            .items_center()
            .gap(r(theme.space.s12))
            .pl(r(theme.space.s16))
            .pr(r(theme.space.s4))
            .py(r(theme.space.s4))
            .rounded_full()
            .bg(hsla(colors.action.primary))
            .text_color(hsla(colors.on.primary))
            .shadow(shadows(&theme.shadow.light.raised))
            .type_style(&theme.typography.desktop.ui)
    };
    let gone: Vec<_> = leaving
        .iter()
        .map(|(notice, out)| {
            let out = bezier(out_curve, *out);
            // What is left of a closed toast: no button, nothing to reach, rising out as it fades.
            pill()
                .debug_selector(|| "toast.leaving".into())
                .top(r(-RISE * out))
                .opacity(1. - out)
                .pr(r(theme.space.s16))
                .h(r(theme.size.icon_button + 2. * theme.space.s4))
                .child(notice.text())
        })
        .collect();
    div()
        .absolute()
        .top(r(theme.size.titlebar_height + theme.space.s12))
        .left_0()
        .right_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(r(theme.space.s8))
        .children(notices.into_iter().enumerate().map(|(index, notice)| {
            let drop = drops.get(index).copied().unwrap_or(0.);
            // A status, as the design's toast is, so Windows reads it out (M2 review F8).
            pill()
                .semantic(format!("toast.{index}"), Role::Status)
                .aria_label(notice.text())
                .when(drop != 0., |pill| {
                    pill.top(r(-DROP * drop)).opacity((1. - drop).clamp(0., 1.))
                })
                .child(notice.text())
                .child(
                    IconButton::new(format!("toast.close.{index}"), "x", "Close")
                        .variant(IconVariant::OnDark)
                        .on_press(move |window, cx| close(index, window, cx)),
                )
        }))
        .children(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notices::Notice;
    use crate::ui::primitives::test_window;

    #[gpui::test]
    fn a_toast_drops_in_and_rises_out_faster(cx: &mut gpui::TestAppContext) {
        use crate::ui::testing::{click, draw, next_frame};
        let cx = test_window(cx, |window, cx| toasts(window, cx).into_any_element());
        cx.update(|window, _| window.activate_window());
        draw(cx);
        cx.update(|_, cx| cx.set_global(Notices(vec![Notice::SomethingWentWrong])));
        draw(cx);
        let first = cx.debug_bounds("toast.0").expect("the toast");
        let mut frames = 0;
        while next_frame(cx) {
            frames += 1;
            assert!(frames < 120, "it lands");
        }
        let rest = cx.debug_bounds("toast.0").expect("the toast");
        assert!(
            first.origin.y < rest.origin.y - gpui::px(8.),
            "it drops in from above its place: {first:?} -> {rest:?}"
        );
        click(cx, "toast.close.0");
        assert!(cx.debug_bounds("toast.0").is_none(), "closed");
        let mut out = 0;
        while cx.debug_bounds("toast.leaving").is_some() {
            assert!(next_frame(cx), "it rises out frame by frame");
            out += 1;
            assert!(out < 60, "and is gone");
        }
        assert!(
            out > 2 && out < frames,
            "out is faster than in: {out} < {frames}"
        );
        assert!(
            toast_frames(cx) > out,
            "its frames are marked for the motion checks"
        );
    }

    /// The frames a toast moved in, by the frame log's marks.
    fn toast_frames(cx: &mut gpui::VisualTestContext) -> usize {
        cx.update(|_, cx| {
            crate::motion::clock::frames(cx)
                .iter()
                .filter(|frame| frame.marks & crate::motion::clock::TOAST != 0)
                .count()
        })
    }

    #[gpui::test]
    fn under_reduced_motion_a_toast_shows_at_once(cx: &mut gpui::TestAppContext) {
        use crate::ui::testing::{draw, next_frame};
        let cx = test_window(cx, |window, cx| toasts(window, cx).into_any_element());
        cx.update(|window, cx| {
            window.activate_window();
            crate::motion::reduced::set_for_test(true, cx);
        });
        draw(cx);
        cx.update(|_, cx| cx.set_global(Notices(vec![Notice::SomethingWentWrong])));
        draw(cx);
        assert!(cx.debug_bounds("toast.0").is_some());
        assert!(!next_frame(cx), "no frames in between");
        assert_eq!(toast_frames(cx), 0, "none marked");
    }

    #[gpui::test]
    fn a_notice_is_a_status_toast(cx: &mut gpui::TestAppContext) {
        let cx = test_window(cx, |window, cx| toasts(window, cx).into_any_element());
        cx.update(|_, cx| cx.set_global(Notices(vec![Notice::SomethingWentWrong])));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("toast.0").is_some(),
            "found by its id, as UI Automation finds it"
        );
    }
}
