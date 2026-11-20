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
}

impl Spring {
    /// At rest at `value`, on one of the theme's springs (`motion.spring.calm`, `snappy` or `playful`; mass 1).
    pub fn new(value: f32, tokens: &theme::Spring) -> Self {
        Self {
            value,
            velocity: 0.,
            target: value,
            stiffness: tokens.stiffness,
            damping_ratio: tokens.damping_ratio,
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
        if x.abs() < 0.25 && self.velocity.abs() < 8. {
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
    fn snap_completes_in_one_step() {
        let mut spring = Spring::new(0., &springs().calm);
        spring.snap(544.);
        assert!(spring.at_rest());
        assert!(!spring.step(0.01));
        assert_eq!(spring.value, 544.);
    }
}
