//! The browser panel's motion (Phase 3 plan T3.7; motion.html card 12, "Browser panel"): the panel's width springs open
//! and shut on the calm spring while the chat column takes the rest, and its content keeps its width and is revealed, not
//! squashed. A key, or reduced motion, completes the change at once. Each motion runs in the variant chosen at its start
//! (`panel_variant`); a finished live motion's p95 frame interval decides the next choice.

use std::time::Instant;

use super::panel_variant::{self, PanelVariant};
use crate::motion::spring::Spring;
use crate::theme;

pub struct PanelMotion {
    /// The panel's visible width in design px: 0 closed, `open_width` open.
    pub width: Spring,
    /// The width the panel opens to.
    open_width: f32,
    /// The motion opens (or the last one opened) the panel.
    opening: bool,
    /// The variant of the motion in progress, or of the last one.
    variant: PanelVariant,
    /// When the motion in progress began (QPC), for its p95.
    started_qpc: Option<i64>,
    /// The last live motion's p95 frame interval in ms, and when it ended.
    last_live: Option<(f32, Instant)>,
}

impl PanelMotion {
    /// Closed, opening to `open_width` on `spring` (the theme's calm spring).
    pub fn new(open_width: f32, spring: &theme::Spring) -> Self {
        PanelMotion {
            width: Spring::new(0., spring),
            open_width,
            opening: false,
            variant: PanelVariant::Live,
            started_qpc: None,
            last_live: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.width.target > 0.
    }

    /// The width the panel opens to, in design px.
    pub fn open_width(&self) -> f32 {
        self.open_width
    }

    /// The room changed, or the person resized the panel: an open panel takes the new width at once (a resize never
    /// animates).
    pub fn set_open_width(&mut self, width: f32) {
        self.open_width = width;
        if self.is_open() {
            self.width.snap(width);
        }
    }

    pub fn is_moving(&self) -> bool {
        !self.width.at_rest()
    }

    pub fn variant(&self) -> PanelVariant {
        self.variant
    }

    /// Opens or closes the panel. `snaps`: a key or reduced motion completes it at once. Otherwise it moves on the spring
    /// from where it is, keeping its velocity, in `variant`, from the frame starting at `now_qpc`.
    pub fn set_open(&mut self, open: bool, snaps: bool, variant: PanelVariant, now_qpc: i64) {
        let target = if open { self.open_width } else { 0. };
        self.opening = open;
        if snaps {
            self.width.snap(target);
            self.started_qpc = None;
            return;
        }
        if self.width.target == target && self.width.at_rest() {
            return;
        }
        self.width.set_target(target);
        // A turn mid-flight is part of the motion already measured: it keeps its start.
        if self.started_qpc.is_none() {
            self.variant = variant;
            self.started_qpc = Some(now_qpc);
        }
    }

    /// Advances by `dt` seconds. Returns whether it still moves.
    pub fn step(&mut self, dt: f32) -> bool {
        self.width.step(dt)
    }

    /// The panel's content opacity: it fades in after 30% of an opening and out in the first 40% of a closing.
    pub fn content_opacity(&self) -> f32 {
        if !self.is_moving() {
            return if self.is_open() { 1. } else { 0. };
        }
        let shown = if self.open_width > 0. {
            (self.width.value / self.open_width).clamp(0., 1.)
        } else {
            0.
        };
        let opacity = if self.opening {
            (shown - 0.3) / 0.7
        } else {
            (shown - 0.6) / 0.4
        };
        opacity.clamp(0., 1.)
    }

    /// The variant for a motion starting `now`, from the last live motion and the system CPU (`PanelVariant::choose`).
    pub fn choose(&self, now: Instant, system_cpu_1s: f32) -> PanelVariant {
        PanelVariant::choose(
            self.last_live.map(|(p95, _)| p95),
            self.last_live.map(|(_, at)| at),
            now,
            system_cpu_1s,
        )
    }

    /// The motion came to rest: a live one leaves its p95 frame interval, from the frames that began since it started (QPC
    /// starts, at `frequency` per second), for the next choice.
    pub fn finish(&mut self, frame_starts: &[i64], frequency: i64, now: Instant) {
        let Some(started) = self.started_qpc.take() else {
            return;
        };
        if self.variant != PanelVariant::Live {
            return;
        }
        let own: Vec<i64> = frame_starts
            .iter()
            .copied()
            .filter(|start| *start >= started)
            .collect();
        if let Some(p95) = panel_variant::p95_interval_ms(&own, frequency) {
            self.last_live = Some((p95, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    const W: f32 = 560.;
    const DT: f32 = 1. / 60.;

    fn calm() -> theme::Spring {
        Theme::aegis().unwrap().motion.spring.calm
    }

    /// The values a motion passes through, one per frame, from its start to rest.
    fn run(motion: &mut PanelMotion, frames: Option<usize>) -> Vec<f32> {
        let mut values = vec![motion.width.value];
        let mut stepped = 0;
        while frames.is_none_or(|frames| stepped < frames) && motion.step(DT) {
            values.push(motion.width.value);
            stepped += 1;
        }
        if !motion.is_moving() {
            values.push(motion.width.value);
        }
        values
    }

    fn largest_step(values: &[f32]) -> f32 {
        values
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0., f32::max)
    }

    #[test]
    fn interrupt_continues_from_value() {
        let mut uninterrupted = PanelMotion::new(W, &calm());
        uninterrupted.set_open(true, false, PanelVariant::Live, 0);
        let whole = run(&mut uninterrupted, None);
        assert_eq!(whole.last().copied(), Some(W), "opens fully");
        let largest = largest_step(&whole);
        assert!(largest > 0.);

        // The same opening, turned back at half its frames.
        let mut turned = PanelMotion::new(W, &calm());
        turned.set_open(true, false, PanelVariant::Live, 0);
        let mut values = run(&mut turned, Some(whole.len() / 2));
        let at_turn = turned.width.value;
        let velocity = turned.width.velocity;
        turned.set_open(false, false, PanelVariant::Live, 0);
        assert_eq!(
            (turned.width.value, turned.width.velocity),
            (at_turn, velocity),
            "the turn starts from the value and velocity it had"
        );
        values.extend(run(&mut turned, None).into_iter().skip(1));
        assert_eq!(values.last().copied(), Some(0.), "it ends closed");
        let turn = values
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .nth(whole.len() / 2)
            .unwrap();
        assert!(
            turn <= largest + 0.5,
            "the step at the turn ({turn} px) is within the largest uninterrupted step ({largest} px) + 0.5 px"
        );
        assert!(
            largest_step(&values) <= largest + 0.5,
            "no step of the turned motion is bigger than the largest uninterrupted one"
        );
    }

    #[test]
    fn a_key_or_reduced_motion_snaps_with_no_frames_between() {
        let mut motion = PanelMotion::new(W, &calm());
        motion.set_open(true, true, PanelVariant::Live, 0);
        assert!(motion.is_open() && !motion.is_moving());
        assert_eq!(motion.width.value, W);
        assert!(!motion.step(DT), "nothing left to move");
        motion.set_open(false, true, PanelVariant::Live, 0);
        assert!(!motion.is_open() && !motion.is_moving());
        assert_eq!(motion.width.value, 0.);
    }

    #[test]
    fn the_content_fades_in_late_on_opening_and_out_early_on_closing() {
        let mut motion = PanelMotion::new(W, &calm());
        motion.set_open(true, false, PanelVariant::Live, 0);
        motion.width.value = 0.25 * W;
        assert_eq!(motion.content_opacity(), 0., "hidden until 30% open");
        motion.width.value = W;
        assert_eq!(motion.content_opacity(), 1.);
        motion.set_open(false, false, PanelVariant::Live, 0);
        motion.width.value = 0.65 * W;
        assert!(motion.content_opacity() > 0., "still fading at 35% closed");
        motion.width.value = 0.55 * W;
        assert_eq!(
            motion.content_opacity(),
            0.,
            "gone after the first 40% of a closing"
        );
    }

    #[test]
    fn a_finished_live_motion_leaves_its_p95_for_the_next_choice() {
        let now = Instant::now();
        let mut motion = PanelMotion::new(W, &calm());
        motion.set_open(true, false, PanelVariant::Live, 1_000);
        // Frames before the motion don't count; inside it, 1 ms ticks, 30 ms apart: a miss.
        let starts: Vec<i64> = std::iter::once(0)
            .chain((0..20).map(|n| 1_000 + n * 30))
            .collect();
        motion.finish(&starts, 1000, now);
        assert_eq!(motion.choose(now, 0.), PanelVariant::Snapshot);
        // A snapshot motion leaves the live record as it was.
        motion.set_open(false, false, PanelVariant::Snapshot, 5_000);
        motion.finish(&[5_000, 5_010, 5_020], 1000, now);
        assert_eq!(motion.choose(now, 0.), PanelVariant::Snapshot);
    }
}
