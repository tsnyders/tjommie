//! Port of `packages/ui/lib/browser-preview.ts` (T1.4): the visible browser card's picture, polled once a second.

use std::time::{Duration, Instant};

use protocol::browser::{BrowserFrame, BrowserFrameResult};

/// A visible card keeps one image and one request, with no queued video or persistent cache. TypeScript drives this with a
/// timer and an async pull; here the caller owns both. `begin_pull` says when a pull is due and `finish_pull` takes its answer.
#[derive(Debug, Default)]
pub struct BrowserPreview {
    visible: bool,
    epoch: u64,
    pending: bool,
    next_at: Option<Instant>,
    shown: Option<BrowserFrame>,
}

/// The wait between pulls.
const POLL: Duration = Duration::from_millis(1000);

impl BrowserPreview {
    pub fn new() -> Self {
        Self::default()
    }

    /// `setVisible`: hiding drops the picture and the timer; showing asks for a pull at once.
    pub fn set_visible(&mut self, visible: bool, now: Instant) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.epoch += 1;
        self.next_at = None;
        self.shown = None;
        if visible {
            self.next_at = Some(now);
        }
    }

    /// The epoch of a pull to start now: the card is shown, no pull is out, and the timer has run out. The caller makes the
    /// request, then calls `finish_pull` with its answer.
    pub fn begin_pull(&mut self, now: Instant) -> Option<u64> {
        let due = self.visible && !self.pending && self.next_at.is_some_and(|at| now >= at);
        if !due {
            return None;
        }
        self.pending = true;
        Some(self.epoch)
    }

    /// The answer to a pull `begin_pull` started (`None`: it failed). A reply from before a hide or a re-show is dropped.
    pub fn finish_pull(&mut self, epoch: u64, answer: Option<BrowserFrameResult>, now: Instant) {
        if self.visible && self.epoch == epoch {
            self.shown = answer
                .filter(|result| result.state.private_active != Some(true))
                .and_then(|result| result.frame);
        }
        self.pending = false;
        if self.visible {
            // A reply from this epoch waits the second out; a stale one (the card was hidden or re-shown) sets its timer at once.
            let delay = if self.epoch == epoch {
                POLL
            } else {
                Duration::ZERO
            };
            self.next_at = Some(now + delay);
        }
    }

    pub fn shown(&self) -> Option<&BrowserFrame> {
        self.shown.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn state(patch: Value) -> protocol::browser::BrowserState {
        let mut base = json!({
            "botId": "bot", "generation": "one", "control": "bot", "activePageId": "page", "tabs": [], "dialog": null
        });
        if let (Some(base), Value::Object(patch)) = (base.as_object_mut(), patch) {
            base.extend(patch);
        }
        serde_json::from_value(base).expect("a browser state")
    }

    fn frame() -> BrowserFrame {
        serde_json::from_value(json!({
            "data": "AAAA", "mime": "image/jpeg", "width": 640, "height": 480, "sequence": 1
        }))
        .expect("a frame")
    }

    fn answer(
        state: protocol::browser::BrowserState,
        frame: Option<BrowserFrame>,
    ) -> Option<BrowserFrameResult> {
        Some(BrowserFrameResult {
            state,
            frame,
            unchanged: None,
        })
    }

    #[test]
    fn polls_once_a_second_only_while_visible_and_clears_retained_pixels_when_hidden_private() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut preview = BrowserPreview::new();
        assert_eq!(preview.begin_pull(at(5000)), None); // hidden: nothing is pulled
        preview.set_visible(true, at(5000));
        let first = preview.begin_pull(at(5000)).expect("shown: a pull is due");
        preview.finish_pull(first, answer(state(json!({})), Some(frame())), at(5000));
        assert_eq!(preview.shown(), Some(&frame()));
        assert_eq!(preview.begin_pull(at(5999)), None); // one second between pulls
        let second = preview
            .begin_pull(at(6000))
            .expect("a second later: pulled again");
        preview.finish_pull(second, answer(state(json!({})), Some(frame())), at(6000));
        preview.set_visible(false, at(6000));
        assert_eq!(preview.shown(), None); // hidden: the pixels go
        assert_eq!(preview.begin_pull(at(11_000)), None); // and nothing is pulled while hidden
        // Shown again while the bot's browser is private: no picture.
        preview.set_visible(true, at(11_000));
        let third = preview
            .begin_pull(at(11_000))
            .expect("shown again: a pull is due");
        preview.finish_pull(
            third,
            answer(state(json!({ "privateActive": true })), Some(frame())),
            at(11_000),
        );
        assert_eq!(preview.shown(), None);
        preview.set_visible(false, at(11_000));
    }

    #[test]
    fn drops_late_hidden_replies_and_never_has_a_second_request_queued() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut preview = BrowserPreview::new();
        preview.set_visible(true, at(0));
        let epoch = preview.begin_pull(at(0)).expect("the first pull");
        assert_eq!(preview.begin_pull(at(10_000)), None); // ten seconds on, the first is still out: no second request
        preview.set_visible(false, at(10_000));
        preview.finish_pull(epoch, answer(state(json!({})), Some(frame())), at(10_000)); // the late reply
        assert_eq!(preview.shown(), None); // hidden, so the frame is never shown
        assert_eq!(preview.begin_pull(at(20_000)), None);
    }
}
