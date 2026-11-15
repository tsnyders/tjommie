//! Reduced motion (Phase 3 plan T2.3; spike limit 18; review focus 5): on while Windows' Animation effects is off, and
//! it follows the setting while the app runs, or with `--force-reduced` (tests). While it is on, every distance is 0,
//! shared elements cross-fade, the eyes, scans and the sweep stop, and numbers snap: each motion asks `is_on`.

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
    cx.set_global(Reduced {
        on: forced || !read(),
        forced,
        _watcher: watcher,
    });
    cx.spawn(async move |cx| {
        guarded_async("follow Animation effects", async move {
            while let Some(animates) = changes.next().await {
                cx.update(|cx| {
                    let reduced = cx.global_mut::<Reduced>();
                    reduced.on = reduced.forced || !animates;
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
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The setting, as this test sees it: the owner's real setting is never changed.
    static ANIMATIONS: AtomicBool = AtomicBool::new(true);

    fn stand_in() -> bool {
        ANIMATIONS.load(Ordering::SeqCst)
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

        ANIMATIONS.store(false, Ordering::SeqCst);
        announce(cx);
        assert!(cx.update(|cx| is_on(cx)), "turned off while the app runs");
        assert!(!next_motion_moves(cx), "the next motion completes at once");

        ANIMATIONS.store(true, Ordering::SeqCst);
        announce(cx);
        assert!(next_motion_moves(cx), "turned on again: it moves again");
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
    }
}
