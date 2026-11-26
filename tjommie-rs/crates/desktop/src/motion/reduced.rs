//! Reduced motion (Phase 3 plan T2.3; spike limit 18; review focus 5): on while Windows' Animation effects is off, and
//! it follows the setting while the app runs, or with `--force-reduced` (tests). While it is on, every distance is 0,
//! shared elements cross-fade, the eyes, scans and the sweep stop, and numbers snap: each motion asks `is_on`.
//! GPUI's own flag (`App::reduce_motion`), which GPUI's `with_animation` and gpui-kit's transitions read, follows the
//! same value: gpui-kit reads the setting once at start and doesn't follow it on Windows, and once the app sets the
//! flag, the app owns it (M2 review F3). `init` runs after `gpui_kit::init`.

use desktop_host::shell::animation::{self, Watcher};
use futures::StreamExt as _;
use gpui::{App, Global};

use crate::guard::guarded_async;

pub struct Reduced {
    on: bool,
    forced: bool,
    /// Hears the setting change; kept for the app's life.
    _watcher: Option<Watcher>,
}

impl Global for Reduced {}

/// Whether motion is reduced now.
pub fn is_on(cx: &App) -> bool {
    cx.try_global::<Reduced>().is_some_and(|reduced| reduced.on)
}

/// Reduced motion on or off for a view test, without a watcher on the owner's setting.
#[cfg(test)]
pub fn set_for_test(on: bool, cx: &mut App) {
    cx.set_global(Reduced {
        on,
        forced: on,
        _watcher: None,
    });
    cx.set_reduce_motion(on);
}

/// Reads Animation effects and follows it from now on; `forced` (`--force-reduced`) keeps motion reduced.
pub fn init(forced: bool, cx: &mut App) {
    start(animation::animations_on, forced, cx);
}

/// `init` with `read` standing in for the setting, which tests never change.
fn start(read: fn() -> bool, forced: bool, cx: &mut App) {
    let (changed, mut changes) = futures::channel::mpsc::unbounded();
    let watcher = animation::watch(read, move |animates| {
        let _ = changed.unbounded_send(animates);
    })
    .inspect_err(|error| {
        log::warn!("Reduced motion won't follow Animation effects while the app runs: {error}");
    })
    .ok();
    let on = forced || !read();
    cx.set_global(Reduced {
        on,
        forced,
        _watcher: watcher,
    });
    cx.set_reduce_motion(on);
    cx.spawn(async move |cx| {
        guarded_async("follow Animation effects", async move {
            while let Some(animates) = changes.next().await {
                cx.update(|cx| {
                    let reduced = cx.global_mut::<Reduced>();
                    reduced.on = reduced.forced || !animates;
                    let on = reduced.on;
                    cx.set_reduce_motion(on);
                    cx.refresh_windows();
                });
            }
        })
        .await
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::keyboard::{Trigger, animate};
    use crate::motion::spring::Spring;
    use crate::theme::Theme;
    use crate::ui::primitives::sheet::open_sheet;
    use crate::ui::primitives::test_window;
    use gpui::{
        Animation, AnimationExt as _, InteractiveElement as _, IntoElement as _,
        ParentElement as _, Styled as _, div, px,
    };
    use gpui_kit::component::WindowExt as _;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// The setting, as this test sees it: the owner's real setting is never changed.
    static ANIMATIONS: AtomicBool = AtomicBool::new(true);

    fn stand_in() -> bool {
        ANIMATIONS.load(Ordering::SeqCst)
    }

    /// The setting as `gpui_animations_and_the_sheet_follow_a_live_change` sees it (tests run in parallel).
    static SHEET_ANIMATIONS: AtomicBool = AtomicBool::new(true);

    fn sheet_stand_in() -> bool {
        SHEET_ANIMATIONS.load(Ordering::SeqCst)
    }

    fn always_animates() -> bool {
        true
    }

    /// Whether the next motion, started by the pointer, moves rather than completing at once.
    fn next_motion_moves(cx: &mut gpui::TestAppContext) -> bool {
        cx.update(|cx| {
            let mut spring = Spring::new(0., &cx.global::<Theme>().motion.spring.calm);
            animate(&mut spring, 100., Trigger::Pointer, cx);
            !spring.at_rest()
        })
    }

    /// Windows broadcasts WM_SETTINGCHANGE; the app takes it in.
    fn announce(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.global::<Reduced>()
                ._watcher
                .as_ref()
                .expect("the watcher")
                .announce();
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn follows_setting_change(cx: &mut gpui::TestAppContext) {
        // The watcher is a real thread, so its wake-ups aren't the test scheduler's own.
        cx.executor().allow_parking();
        ANIMATIONS.store(true, Ordering::SeqCst);
        cx.update(|cx| {
            cx.set_global(Theme::aegis().unwrap());
            start(stand_in, false, cx);
        });
        assert!(!cx.update(|cx| is_on(cx)));
        assert!(
            next_motion_moves(cx),
            "Animation effects on: the next motion moves"
        );
        assert!(!cx.update(|cx| cx.reduce_motion()), "GPUI's flag agrees");

        ANIMATIONS.store(false, Ordering::SeqCst);
        announce(cx);
        assert!(cx.update(|cx| is_on(cx)), "turned off while the app runs");
        assert!(!next_motion_moves(cx), "the next motion completes at once");
        assert!(
            cx.update(|cx| cx.reduce_motion()),
            "and GPUI's flag follows (review F3)"
        );

        ANIMATIONS.store(true, Ordering::SeqCst);
        announce(cx);
        assert!(next_motion_moves(cx), "turned on again: it moves again");
        assert!(!cx.update(|cx| cx.reduce_motion()));
    }

    #[gpui::test]
    fn force_reduced_holds_whatever_the_setting(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            cx.set_global(Theme::aegis().unwrap());
            start(always_animates, true, cx);
        });
        assert!(cx.update(|cx| is_on(cx)));
        assert!(!next_motion_moves(cx));
        assert!(
            cx.update(|cx| cx.reduce_motion()),
            "GPUI's own animations too"
        );
    }

    #[gpui::test]
    fn gpui_animations_and_the_sheet_follow_a_live_change(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        SHEET_ANIMATIONS.store(true, Ordering::SeqCst);
        let shown = Rc::new(Cell::new(-1f32));
        let probe = shown.clone();
        let cx = test_window(cx, move |_, _| {
            let probe = probe.clone();
            div()
                .size_full()
                .child(div().id("probe").size(px(10.)).with_animation(
                    "probe",
                    Animation::new(Duration::from_secs(30)),
                    move |el, delta| {
                        probe.set(delta);
                        el
                    },
                ))
                .into_any_element()
        });
        cx.update(|_, cx| start(sheet_stand_in, false, cx));
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        // Where the sheet's content is on its first frame, and where it rests.
        let sheet_first_and_rest = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                open_sheet(window, cx, |sheet, _, _| {
                    sheet.child(div().debug_selector(|| "sheet.probe".into()).size(px(20.)))
                })
            });
            cx.run_until_parked();
            draw(cx);
            let first = cx.debug_bounds("sheet.probe").expect("the sheet's content");
            // GPUI's animations run on the scheduler's clock.
            cx.executor().advance_clock(Duration::from_millis(400));
            draw(cx);
            let rest = cx.debug_bounds("sheet.probe").expect("the sheet's content");
            cx.update(|window, cx| window.close_sheet(cx));
            cx.run_until_parked();
            draw(cx);
            (first, rest)
        };

        draw(cx);
        assert!(
            shown.get() < 1.,
            "animations on: the probe animates ({})",
            shown.get()
        );
        let (first, rest) = sheet_first_and_rest(cx);
        assert_ne!(first, rest, "animations on: the sheet slides in");

        SHEET_ANIMATIONS.store(false, Ordering::SeqCst);
        cx.update(|_, cx| {
            cx.global::<Reduced>()
                ._watcher
                .as_ref()
                .expect("the watcher")
                .announce();
        });
        cx.run_until_parked();
        draw(cx);
        assert_eq!(
            shown.get(),
            1.,
            "turned off while the app runs: GPUI's animation shows its last frame"
        );
        let (first, rest) = sheet_first_and_rest(cx);
        assert_eq!(first, rest, "and the sheet appears in place");
    }
}
