//! The check that draws on (motion.html, the approved record's check): Lucide's check, stroked to a fraction of its
//! length, the fraction on a spring. Reduced motion and keyboard approvals draw it whole at once.

use gpui::{Hsla, IntoElement, PathBuilder, Styled as _, canvas, point, px};

use super::spring::Spring;
use crate::theme;
use crate::ui::style::r;

/// Lucide's check in its 24-unit box, in drawing order: the short stroke first.
const CHECK: [(f32, f32); 3] = [(4., 12.), (9., 17.), (20., 6.)];

/// How much of the check shows, on a spring from 0 to 1.
pub struct DrawOn {
    progress: Spring,
}

impl DrawOn {
    /// Not drawn yet, on `tokens` (the theme's calm spring: no overshoot past whole).
    pub fn new(tokens: &theme::Spring) -> Self {
        DrawOn {
            progress: Spring::new(0., tokens),
        }
    }

    /// Draws the check on, or at once with `snap`.
    pub fn draw(&mut self, snap: bool) {
        if snap {
            self.progress.snap(1.);
        } else {
            self.progress.set_target(1.);
        }
    }

    /// Steps by `dt`; whether it is still drawing.
    pub fn step(&mut self, dt: f32) -> bool {
        self.progress.step(dt)
    }

    pub fn progress(&self) -> f32 {
        self.progress.value.clamp(0., 1.)
    }
}

/// The polyline `points` cut at fraction `t` (0 to 1) of its length.
pub fn partial(points: &[(f32, f32)], t: f32) -> Vec<(f32, f32)> {
    let length = |(ax, ay): (f32, f32), (bx, by): (f32, f32)| (bx - ax).hypot(by - ay);
    let segments = || points.iter().copied().zip(points.iter().copied().skip(1));
    let total: f32 = segments().map(|(a, b)| length(a, b)).sum();
    let mut left = total * t.clamp(0., 1.);
    let mut out = points.iter().take(1).copied().collect::<Vec<_>>();
    for (a, b) in segments() {
        let segment = length(a, b);
        if left >= segment {
            out.push(b);
            left -= segment;
        } else {
            if left > 0. {
                let k = left / segment;
                out.push((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k));
            }
            break;
        }
    }
    out
}

/// The check drawn to `progress` (0 to 1), `side` design px square, in `color`.
pub fn check(progress: f32, side: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let unit = f32::from(bounds.size.width) / 24.;
            let points = partial(&CHECK, progress);
            let Some((&first, rest)) = points.split_first() else {
                return;
            };
            if rest.is_empty() {
                return;
            }
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.origin.x + px(x * unit),
                    bounds.origin.y + px(y * unit),
                )
            };
            let mut path = PathBuilder::stroke(px(2. * unit));
            path.move_to(at(first));
            for &next in rest {
                path.line_to(at(next));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(r(side))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_the_stroke_at_a_fraction_of_its_length() {
        assert_eq!(partial(&CHECK, 0.), [(4., 12.)], "nothing drawn");
        assert_eq!(partial(&CHECK, 1.), CHECK, "the whole check");
        let half = partial(&CHECK, 0.5);
        assert_eq!(half.len(), 3, "past the bend");
        let (x, y) = half[2];
        assert!(
            (x - 12.).abs() < 0.05 && (y - 14.).abs() < 0.05,
            "half way along: ({x}, {y})"
        );
    }

    #[test]
    fn draws_on_and_snaps() {
        let calm = crate::theme::Theme::aegis().unwrap().motion.spring.calm;
        let mut mark = DrawOn::new(&calm);
        mark.draw(false);
        assert!(mark.step(0.016));
        assert!((0. ..1.).contains(&mark.progress()));
        while mark.step(0.016) {}
        assert_eq!(mark.progress(), 1.);
        let mut keyed = DrawOn::new(&calm);
        keyed.draw(true);
        assert_eq!(keyed.progress(), 1.);
    }
}
