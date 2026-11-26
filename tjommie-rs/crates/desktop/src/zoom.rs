//! App zoom (GPUI limit 11). The rem is 16 px × zoom × the text-size factor; Ctrl+= and Ctrl+- step through `STEPS`,
//! clamped at the ends, and Ctrl+0 resets. Shadows stay in px and don't scale (accepted, as in the spike). Storing it
//! per device in desktop-ui.json is T1.5's.

pub const STEPS: [f32; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5];
const RESET: usize = 2;

/// An index into `STEPS`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom(usize);

impl Default for Zoom {
    fn default() -> Self {
        Zoom(RESET)
    }
}

impl Zoom {
    pub fn value(self) -> f32 {
        STEPS.get(self.0).copied().unwrap_or(1.0)
    }

    pub fn zoom_in(self) -> Zoom {
        Zoom((self.0 + 1).min(STEPS.len() - 1))
    }

    pub fn zoom_out(self) -> Zoom {
        Zoom(self.0.saturating_sub(1))
    }

    /// The rem in px for this zoom and a text-size factor.
    pub fn rem_px(self, text_size: f32) -> f32 {
        16.0 * self.value() * text_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_clamp() {
        let mut zoom = Zoom::default();
        assert_eq!(zoom.value(), 1.0);
        let up: Vec<f32> = (0..4)
            .map(|_| {
                zoom = zoom.zoom_in();
                zoom.value()
            })
            .collect();
        assert_eq!(up, [1.1, 1.25, 1.5, 1.5]);
        let down: Vec<f32> = (0..7)
            .map(|_| {
                zoom = zoom.zoom_out();
                zoom.value()
            })
            .collect();
        assert_eq!(down, [1.25, 1.1, 1.0, 0.9, 0.8, 0.8, 0.8]);
        assert_eq!(Zoom::default(), Zoom::default().zoom_in().zoom_out());
        // The rem: 16 px × zoom × the text-size factor.
        assert_eq!(Zoom::default().zoom_in().rem_px(1.25), 16.0 * 1.1 * 1.25);
    }
}
