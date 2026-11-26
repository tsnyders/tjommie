//! The spring (the Aegis motion tokens; the spike's `motion.rs`): position, velocity and target, integrated each frame
//! by semi-implicit Euler in 8 substeps, with dt capped at 64 ms, and at rest within 0.25 px and 8 px/s. GPUI's
//! `with_animation` starts over from its start value (limit 13); retargeting this spring keeps its velocity, so an
//! interruption turns and never jumps.

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub value: f32,
    /// Units per second.
    pub velocity: f32,
    pub target: f32,
    stiffness: f32,
    damping_ratio: f32,
    /// Pixels per unit, for the rest rule: a spring in px has 1; a fraction (a check drawn on, a row's share) has the
    /// pixels its whole spans, so it comes to rest within a quarter pixel and not a quarter of its whole.
    scale: f32,
}

impl Spring {
    /// At rest at `value`, on one of the theme's springs (`motion.spring.calm`, `snappy` or `playful`; mass 1).
    pub fn new(value: f32, tokens: &theme::Spring) -> Self {
        Self::scaled(value, tokens, 1.)
    }

    /// A spring whose unit spans `scale` pixels on screen: a 0-to-1 fraction of a 36 px row has 36.
    pub fn scaled(value: f32, tokens: &theme::Spring, scale: f32) -> Self {
        // A degenerate token (0, negative or NaN) would never come to rest (M7 review n12): the calm spring's values.
        let usable = |token: f32| token.is_finite() && token > 0.;
        Self {
            value,
            velocity: 0.,
            target: value,
            stiffness: if usable(tokens.stiffness) {
                tokens.stiffness
            } else {
                260.
            },
            damping_ratio: if usable(tokens.damping_ratio) {
                tokens.damping_ratio
            } else {
                1.
            },
            scale: scale.max(1.),
        }
    }

    /// Heads for `target` from wherever the value is, keeping its velocity.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Completes the change at once: reduced motion and keyboard actions.
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.;
    }

    pub fn at_rest(&self) -> bool {
        self.value == self.target && self.velocity == 0.
    }

    /// Advances by `dt` seconds. Returns whether it still moves.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.at_rest() {
            return false;
        }
        let k = self.stiffness;
        let c = 2. * self.damping_ratio * k.sqrt();
        let h = dt.clamp(0., 0.064) / 8.;
        let mut x = self.value - self.target;
        for _ in 0..8 {
            let acceleration = -k * x - c * self.velocity;
            self.velocity += acceleration * h;
            x += self.velocity * h;
        }
        if (x * self.scale).abs() < 0.25 && (self.velocity * self.scale).abs() < 8. {
            self.snap(self.target);
            return false;
        }
        self.value = self.target + x;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn springs() -> theme::Springs {
        Theme::aegis().unwrap().motion.spring
    }

    /// M7 review n12: a degenerate token (stiffness 0, damping NaN) still comes to rest, on the calm spring's values.
    #[test]
    fn a_degenerate_token_still_settles() {
        let mut tokens = springs().calm;
        tokens.stiffness = 0.;
        tokens.damping_ratio = f32::NAN;
        let mut spring = Spring::new(0., &tokens);
        spring.set_target(40.);
        let mut steps = 0;
        while spring.step(0.016) {
            steps += 1;
            assert!(steps < 200, "it settles");
        }
        assert_eq!(spring.value, 40.);
    }

    /// Runs at 100 Hz until rest: (seconds to rest, peak overshoot as a fraction of the distance).
    fn settle(tokens: &theme::Spring, distance: f32) -> (f32, f32) {
        let mut spring = Spring::new(0., tokens);
        spring.set_target(distance);
        let (mut t, mut peak) = (0., 0f32);
        while spring.step(0.01) {
            t += 0.01;
            peak = peak.max(spring.value - distance);
        }
        (t, peak / distance)
    }

    #[test]
    fn calm_settles_in_560ms_within_tolerance() {
        // tokens.json: calm settles in 560 ms with no overshoot. The rest rule is absolute (0.25 px), so over 544 px
        // "at rest" lands near the 0.1% time.
        let (t, overshoot) = settle(&springs().calm, 544.);
        assert!((0.45..=0.70).contains(&t), "calm settled in {t} s");
        assert!(overshoot <= 0.0005, "calm overshot {overshoot}");
    }

    #[test]
    fn snappy_and_playful_overshoot_as_the_tokens_say() {
        // tokens.json: snappy 360 ms with 1.5% overshoot, playful 510 ms with 4.6%.
        let (_, snappy) = settle(&springs().snappy, 544.);
        assert!(
            (0.010..=0.020).contains(&snappy),
            "snappy overshot {snappy}"
        );
        let (_, playful) = settle(&springs().playful, 544.);
        assert!(
            (0.040..=0.052).contains(&playful),
            "playful overshot {playful}"
        );
    }

    #[test]
    fn retarget_keeps_velocity() {
        let mut spring = Spring::new(0., &springs().calm);
        spring.set_target(544.);
        for _ in 0..15 {
            spring.step(0.01);
        }
        let (value, velocity) = (spring.value, spring.velocity);
        assert!(velocity > 500., "moving fast mid-flight: {velocity}");
        spring.set_target(0.);
        assert_eq!((spring.value, spring.velocity), (value, velocity));
        // The next step turns smoothly: no jump, and the velocity changes by at most a frame's acceleration.
        spring.step(0.01);
        assert!((spring.value - value).abs() < velocity * 0.011 + 1.);
        assert!(spring.velocity < velocity);
        while spring.step(0.01) {}
        assert_eq!(spring.value, 0.);
    }

    #[test]
    fn a_fraction_lands_without_a_jump() {
        // A check drawn on, 24 px of stroke: it may not jump the last quarter of its length when it comes to rest.
        let mut drawn = Spring::scaled(0., &springs().calm, 24.);
        drawn.set_target(1.);
        let mut last = 0f32;
        let mut largest_late_step = 0f32;
        while drawn.step(1. / 60.) {
            if drawn.value > 0.9 {
                largest_late_step = largest_late_step.max(drawn.value - last);
            }
            last = drawn.value;
        }
        let landing = drawn.value - last;
        assert_eq!(drawn.value, 1.);
        // At rest within a quarter pixel, after a last frame at under the rest speed (8 px/s).
        assert!(
            landing * 24. <= 0.25 + 8. / 60.,
            "the last step was {} px",
            landing * 24.
        );
        assert!(
            largest_late_step < 0.05,
            "no jump near the end: {largest_late_step}"
        );
    }

    #[test]
    fn snap_completes_in_one_step() {
        let mut spring = Spring::new(0., &springs().calm);
        spring.snap(544.);
        assert!(spring.at_rest());
        assert!(!spring.step(0.01));
        assert_eq!(spring.value, 544.);
    }
}
