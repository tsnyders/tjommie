//! Rolling numbers (motion.html "numbers", `roll`): when a count or an amount changes, each digit's column of 0 to 9
//! slides on the spring it is given (the design's is playful) to its new digit. Other characters change at once, and
//! so does the whole number when its length changes, after a key, or under reduced motion.

use gpui::{IntoElement, ParentElement as _, SharedString, Styled as _, div};

use super::spring::Spring;
use crate::theme;
use crate::ui::style::r;

/// About how tall a rolling digit is, in pixels, for its column's rest rule (one unit is one digit).
const DIGIT_PX: f32 = 20.;

pub struct Rolling {
    text: SharedString,
    rest: Spring,
    /// One per character, from the right: the digit column's position (0 to 9, fractional while it rolls).
    columns: Vec<Spring>,
}

impl Rolling {
    /// Shows `text`, on `tokens` (the theme's playful spring, as the design rolls).
    pub fn new(text: impl Into<SharedString>, tokens: &theme::Spring) -> Self {
        let mut rolling = Rolling {
            text: SharedString::default(),
            rest: Spring::scaled(0., tokens, DIGIT_PX),
            columns: Vec::new(),
        };
        rolling.set(text, true);
        rolling
    }

    /// Changes to `text`: a digit that was a digit rolls to its new value; the rest changes at once, as everything does
    /// with `snap` or when the length changes (motion.html's `roll`).
    pub fn set(&mut self, text: impl Into<SharedString>, snap: bool) {
        let text = text.into();
        let snap = snap || text.chars().count() != self.text.chars().count();
        let old: Vec<char> = self.text.chars().rev().collect();
        let mut columns = Vec::with_capacity(text.len());
        for (at, char) in text.chars().rev().enumerate() {
            let mut column = self.columns.get(at).copied().unwrap_or(self.rest);
            match (
                char.to_digit(10),
                old.get(at).and_then(|old| old.to_digit(10)),
            ) {
                (Some(new), Some(_)) if !snap => column.set_target(new as f32),
                (Some(new), _) => column.snap(new as f32),
                (None, _) => column.snap(0.),
            }
            columns.push(column);
        }
        self.text = text;
        self.columns = columns;
    }

    /// Steps by `dt`; whether a column still rolls.
    pub fn step(&mut self, dt: f32) -> bool {
        self.columns
            .iter_mut()
            .fold(false, |moving, column| column.step(dt) | moving)
    }

    /// Each character, left to right, with its column's position when it is a digit.
    pub fn positions(&self) -> Vec<(char, Option<f32>)> {
        // The columns run from the right, as the characters do reversed.
        let mut positions: Vec<_> = self
            .text
            .chars()
            .rev()
            .zip(&self.columns)
            .map(|(char, column)| (char, char.is_ascii_digit().then_some(column.value)))
            .collect();
        positions.reverse();
        positions
    }

    /// The number, `line_height` design px tall: each digit a one-line window onto its column. Style it (a mono face)
    /// from the parent.
    pub fn render(&self, line_height: f32) -> impl IntoElement {
        div()
            .flex()
            .children(self.positions().into_iter().map(|(char, position)| {
                match position {
                    Some(position) => div().h(r(line_height)).overflow_hidden().child(
                        div()
                            .relative()
                            .top(r(-position * line_height))
                            .flex()
                            .flex_col()
                            .children(
                                (0..=9)
                                    .map(|digit| div().h(r(line_height)).child(digit.to_string())),
                            ),
                    ),
                    None => div().h(r(line_height)).child(char.to_string()),
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn rolling(text: &str) -> Rolling {
        Rolling::new(text, &Theme::aegis().unwrap().motion.spring.playful)
    }

    fn settled(rolling: &mut Rolling) -> Vec<(char, Option<f32>)> {
        while rolling.step(0.016) {}
        rolling.positions()
    }

    #[test]
    fn a_changed_digit_rolls_and_the_rest_stays() {
        let mut amount = rolling("4.12");
        amount.set("4.18", false);
        assert!(amount.step(0.016), "the units roll");
        let rolling_units = amount.positions()[3].1.unwrap();
        assert!(
            (2. ..8.).contains(&rolling_units),
            "on the way: {rolling_units}"
        );
        assert_eq!(amount.positions()[0], ('4', Some(4.)), "the rest stays");
        assert_eq!(amount.positions()[1], ('.', None));
        assert_eq!(settled(&mut amount)[3], ('8', Some(8.)));
    }

    #[test]
    fn a_roll_lands_on_its_digit_without_a_jump() {
        let mut count = rolling("2");
        count.set("8", false);
        let mut last = 2.;
        while count.step(1. / 60.) {
            last = count.positions()[0].1.unwrap();
        }
        assert!(
            (8. - last).abs() * DIGIT_PX <= 0.25,
            "the frame before rest was {last}, a jump of {} px",
            (8. - last).abs() * DIGIT_PX
        );
    }

    #[test]
    fn a_length_change_snaps_the_whole_number() {
        let mut count = rolling("9");
        count.set("10", false);
        assert!(!count.step(0.016), "nothing rolls");
        assert_eq!(count.positions(), [('1', Some(1.)), ('0', Some(0.))]);
    }

    #[test]
    fn snap_changes_everything_at_once() {
        let mut count = rolling("12");
        count.set("47", true);
        assert!(!count.step(0.016));
        assert_eq!(count.positions(), [('4', Some(4.)), ('7', Some(7.))]);
    }
}
