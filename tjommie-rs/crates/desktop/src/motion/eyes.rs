//! The eyes (aegis/README.md, "Agents feel alive"; the spike's eyes; limit 15). Each agent blinks on its own clock,
//! now and then twice; a working agent's eyes narrow and scan side to side; while you type to an agent its eyes glance
//! at the composer. Poses are still looks, so a still frame reads; under reduced motion the eyes keep their pose and
//! nothing blinks, scans or glances. The eyes layer is cropped to its own box, so the lids scale about the eyes' centre
//! (GPUI scales an svg about the centre of its bounds).

use crate::theme;
use crate::ui::primitives::avatar::{Gaze, Pose};

/// A blink's closing curve (motion.html's blinkOnce).
const BLINK_IN: [f32; 4] = [0.5, 0., 1., 1.];
/// How far the eyes scan and glance, in units of the 40-unit avatar.
const REACH: f32 = 1.4;
/// A double blink's second blink, after the first starts.
const DOUBLE: f32 = 0.24;

/// CSS `cubic-bezier(x1, y1, x2, y2)` at time fraction `x` (0 to 1).
pub fn bezier([x1, y1, x2, y2]: [f32; 4], x: f32) -> f32 {
    let x = x.clamp(0., 1.);
    let at = |a: f32, b: f32, t: f32| {
        3. * a * t * (1. - t) * (1. - t) + 3. * b * t * t * (1. - t) + t * t * t
    };
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if at(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(y1, y2, (lo + hi) / 2.)
}

/// The lids' scale `t` seconds into a blink, or `None` once it is over: shut to 0.08 in `blinkClose` (easing in), open
/// in `blinkOpen` on the brand's ease out.
pub fn blink(t: f32, motion: &theme::Motion) -> Option<f32> {
    let close = motion.eyes.blink_close / 1000.;
    let open = motion.eyes.blink_open / 1000.;
    match t {
        t if t < 0. => Some(1.),
        t if t < close => Some(1. - 0.92 * bezier(BLINK_IN, t / close)),
        t if t < close + open => Some(0.08 + 0.92 * bezier(motion.easing.out, (t - close) / open)),
        _ => None,
    }
}

/// A working agent's scan at `t` seconds: the look's x from -1.4 to 1.4 and back, `scan` each way on the pulse curve.
pub fn scan(t: f32, motion: &theme::Motion) -> f32 {
    let leg = (t / (motion.eyes.scan / 1000.)).rem_euclid(2.);
    let p = if leg < 1. { leg } else { 2. - leg };
    -REACH + 2. * REACH * bezier(motion.easing.pulse, p)
}

/// xorshift: each avatar its own blink clock, without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// One living avatar's eyes.
pub struct Eyes {
    rng: Rng,
    next_blink: f32,
    blink_started: Option<f32>,
    double_at: Option<f32>,
    /// Until when the eyes glance, and where.
    glance: Option<(f32, f32)>,
}

impl Eyes {
    /// The eyes of one avatar from `now` (seconds on the motion clock). `seed` gives it its own blink clock: any number,
    /// such as a hash of the agent's id.
    pub fn new(seed: u64, now: f32, motion: &theme::Motion) -> Self {
        let mut rng = Rng(seed | 1);
        let next_blink = now + gap(&mut rng, motion);
        Eyes {
            rng,
            next_blink,
            blink_started: None,
            double_at: None,
            glance: None,
        }
    }

    /// Looks toward `x` (-1 left to 1 right) and holds it for `glanceHold`: at the composer while you type.
    pub fn glance(&mut self, now: f32, x: f32, motion: &theme::Motion) {
        self.glance = Some((
            now + motion.eyes.glance_hold / 1000.,
            x.clamp(-1., 1.) * REACH,
        ));
    }

    /// Where the eyes are at `now` in `pose`. Schedules the next blink, `blinkMin` to `blinkMax` after this one, now and
    /// then with a second one 240 ms after it.
    pub fn gaze(&mut self, now: f32, pose: Pose, reduced: bool, motion: &theme::Motion) -> Gaze {
        let still = Gaze::of(pose);
        if reduced {
            return still;
        }
        if self.blink_started.is_none() && now >= self.next_blink {
            self.blink_started = Some(now);
            self.double_at = (self.rng.next() < 0.18).then_some(now + DOUBLE);
            self.next_blink = now + gap(&mut self.rng, motion);
        }
        if self.blink_started.is_none() && self.double_at.is_some_and(|at| now >= at) {
            self.blink_started = self.double_at.take();
        }
        let lid = match self.blink_started.map(|start| blink(now - start, motion)) {
            Some(Some(scale)) => scale,
            Some(None) => {
                self.blink_started = None;
                1.
            }
            None => 1.,
        };
        self.glance = self.glance.filter(|(until, _)| now < *until);
        let x = match (self.glance, pose) {
            (Some((_, x)), _) => x,
            (None, Pose::Working) => scan(now, motion),
            (None, _) => still.x,
        };
        Gaze {
            lid: still.lid * lid,
            x,
            y: still.y,
        }
    }

    /// Seconds until the eyes next change by themselves: 0 while they move (a blink, the scan, a glance).
    pub fn next_change(&self, now: f32, pose: Pose) -> f32 {
        if self.blink_started.is_some() || self.glance.is_some() || pose == Pose::Working {
            return 0.;
        }
        let next = self
            .double_at
            .map_or(self.next_blink, |double| double.min(self.next_blink));
        (next - now).max(0.)
    }
}

fn gap(rng: &mut Rng, motion: &theme::Motion) -> f32 {
    let eyes = &motion.eyes;
    (eyes.blink_min + rng.next() * (eyes.blink_max - eyes.blink_min)) / 1000.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn motion() -> theme::Motion {
        Theme::aegis().unwrap().motion
    }

    #[test]
    fn eyes_follow_the_tokens() {
        let motion = motion();
        assert!(
            (bezier(motion.easing.out, 0.5) - 0.92).abs() < 0.05,
            "the brand's ease out is fast early"
        );
        assert_eq!(blink(0., &motion), Some(1.));
        assert!(
            (blink(0.070, &motion).unwrap() - 0.08).abs() < 0.01,
            "shut at 70 ms"
        );
        assert!(blink(0.180, &motion).is_none(), "open again after 180 ms");
        assert!((scan(0., &motion) + 1.4).abs() < 0.01);
        assert!((scan(1.8, &motion) - 1.4).abs() < 0.01);
        assert!((scan(3.6, &motion) + 1.4).abs() < 0.01);

        let mut eyes = Eyes::new(7, 0., &motion);
        let first = eyes.next_blink;
        assert!((2.6..=6.4).contains(&first), "first blink at {first} s");
        assert!((eyes.next_change(0., Pose::Idle) - first).abs() < 1e-6);
        assert_eq!(
            eyes.gaze(first + 0.07, Pose::Idle, false, &motion).lid,
            1.,
            "the blink starts on the first frame at or after its time"
        );
        assert!(
            eyes.gaze(first + 0.14, Pose::Idle, false, &motion).lid < 0.2,
            "70 ms later the eyes are shut"
        );
        assert_eq!(eyes.next_change(first + 0.14, Pose::Idle), 0., "blinking");
    }

    #[test]
    fn a_working_agent_scans_and_glances_at_the_composer() {
        let motion = motion();
        let mut eyes = Eyes::new(7, 0., &motion);
        let start = eyes.gaze(0., Pose::Working, false, &motion);
        assert!((start.x + 1.4).abs() < 0.01, "the scan starts at the left");
        assert_eq!(start.lid, 0.7, "the lids narrow");
        eyes.glance(1., 1., &motion);
        assert!(
            (eyes.gaze(1.1, Pose::Working, false, &motion).x - 1.4).abs() < 1e-6,
            "it glances at the composer"
        );
        let after = 1. + motion.eyes.glance_hold / 1000. + 0.01;
        assert!(
            (eyes.gaze(after, Pose::Working, false, &motion).x - scan(after, &motion)).abs() < 1e-6,
            "and goes back to scanning"
        );
    }

    #[test]
    fn reduced_motion_keeps_the_pose_still() {
        let motion = motion();
        let mut eyes = Eyes::new(7, 0., &motion);
        let first = eyes.next_blink;
        for t in [0., first + 0.03, first + 0.07, 2. * first] {
            assert_eq!(
                eyes.gaze(t, Pose::Waiting, true, &motion),
                Gaze::of(Pose::Waiting),
                "at {t} s"
            );
        }
    }
}
