//! The visibility state machine of one bot's browser (research #2, #3, #4 and #5; Phase 3 plan T3.1).
//!
//! What a bot's browser needs decides where it sits, whether it renders and whether it sleeps:
//!
//! | Need | Placement | Visibility | Suspend |
//! |---|---|---|---|
//! | `shown` | in the panel | visible | `Resume()` first |
//! | `streaming` or `tool`, not shown | parked off-screen at the bot's viewport | visible | `Resume()` first |
//! | none | hidden | not visible, then `TrySuspend` | suspended |
//!
//! This module is plain logic with an injected clock. The engine reads the plan and applies it to WebView2.

use std::time::{Duration, Instant};

/// Why a bot's page must be awake. Computed per bot from the three inputs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Need {
    /// It is the active bot in the open local panel.
    pub shown: bool,
    /// A `browser.stream` or `browser.frame` capture is on.
    pub streaming: bool,
    /// An MCP tool, a vault fill or a UI command is executing.
    pub tool: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Over the panel's page frame.
    Panel,
    /// Outside the window's client area at the bot's own viewport size. The page keeps rendering.
    Parked,
    /// Not placed at all.
    Hidden,
}

/// What WebView2 should be doing for a [`Need`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub placement: Placement,
    /// `SetIsVisible`. A page that is not visible does not render.
    pub visible: bool,
    /// `TrySuspend` once hidden; otherwise `Resume` first.
    pub suspend: bool,
}

impl Need {
    pub fn plan(self) -> Plan {
        if self.shown {
            Plan {
                placement: Placement::Panel,
                visible: true,
                suspend: false,
            }
        } else if self.awake() {
            Plan {
                placement: Placement::Parked,
                visible: true,
                suspend: false,
            }
        } else {
            Plan {
                placement: Placement::Hidden,
                visible: false,
                suspend: true,
            }
        }
    }

    /// Whether anything needs the page awake.
    pub fn awake(self) -> bool {
        self.shown || self.streaming || self.tool
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLevel {
    Normal,
    Low,
}

/// The two timers. A test seam changes them.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// A hidden bot with no need for this long has its controller closed.
    pub idle_close: Duration,
    /// A hidden bot kept awake by a tool (not a stream) for this long gets the Low memory target.
    pub low_memory_after: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            idle_close: Duration::from_secs(10 * 60),
            low_memory_after: Duration::from_secs(30),
        }
    }
}

/// One bot's inputs and timers.
#[derive(Debug)]
pub struct Lifecycle {
    timeouts: Timeouts,
    shown: bool,
    streaming: bool,
    /// Tools can overlap; the bot needs the page awake until the last one ends.
    tools: u32,
    /// Since when nothing has needed the page.
    idle_since: Option<Instant>,
    /// Since when the page has been awake only for a tool (not a stream), hidden from the person.
    awake_hidden_since: Option<Instant>,
}

impl Lifecycle {
    /// A bot that has just been opened and shown to nobody starts its idle clock at `now`.
    pub fn new(timeouts: Timeouts, now: Instant) -> Self {
        Self {
            timeouts,
            shown: false,
            streaming: false,
            tools: 0,
            idle_since: Some(now),
            awake_hidden_since: None,
        }
    }

    pub fn need(&self) -> Need {
        Need {
            shown: self.shown,
            streaming: self.streaming,
            tool: self.tools > 0,
        }
    }

    pub fn set_shown(&mut self, shown: bool, now: Instant) {
        self.shown = shown;
        self.refresh(now);
    }

    pub fn set_streaming(&mut self, on: bool, now: Instant) {
        self.streaming = on;
        self.refresh(now);
    }

    pub fn begin_tool(&mut self, now: Instant) {
        self.tools = self.tools.saturating_add(1);
        self.refresh(now);
    }

    pub fn end_tool(&mut self, now: Instant) {
        self.tools = self.tools.saturating_sub(1);
        self.refresh(now);
    }

    fn refresh(&mut self, now: Instant) {
        let need = self.need();
        if need.shown {
            self.idle_since = None;
            self.awake_hidden_since = None;
        } else if need.awake() {
            self.idle_since = None;
            // Only a tool keeps a page awake out of sight long enough to trim it: a stream needs its frames.
            if need.tool && !need.streaming {
                self.awake_hidden_since.get_or_insert(now);
            } else {
                self.awake_hidden_since = None;
            }
        } else {
            self.awake_hidden_since = None;
            self.idle_since.get_or_insert(now);
        }
    }

    /// Low for a page that a tool has kept awake out of sight for a while (never a streaming one); Normal otherwise, and always
    /// Normal once shown or suspended. Never set on a page `TrySuspend` handles (Microsoft's rule).
    pub fn memory_level(&self, now: Instant) -> MemoryLevel {
        match self.awake_hidden_since {
            Some(since) if now.duration_since(since) >= self.timeouts.low_memory_after => {
                MemoryLevel::Low
            }
            _ => MemoryLevel::Normal,
        }
    }

    pub fn idle_close_due(&self, now: Instant) -> bool {
        self.idle_since
            .is_some_and(|since| now.duration_since(since) >= self.timeouts.idle_close)
    }

    /// When the engine should look at this bot again: the next timer that has not fired yet, if any.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let idle = self
            .idle_since
            .map(|since| since + self.timeouts.idle_close)
            .filter(|deadline| *deadline > now);
        let low = self
            .awake_hidden_since
            .map(|since| since + self.timeouts.low_memory_after)
            .filter(|deadline| *deadline > now);
        match (idle, low) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(shown: bool, streaming: bool, tool: bool) -> Need {
        Need {
            shown,
            streaming,
            tool,
        }
    }

    #[test]
    fn shown_sits_in_the_panel_visible_and_awake() {
        for streaming in [false, true] {
            for tool in [false, true] {
                assert_eq!(
                    need(true, streaming, tool).plan(),
                    Plan {
                        placement: Placement::Panel,
                        visible: true,
                        suspend: false
                    }
                );
            }
        }
    }

    #[test]
    fn streaming_or_a_tool_parks_a_bot_that_is_not_shown() {
        for (streaming, tool) in [(true, false), (false, true), (true, true)] {
            assert_eq!(
                need(false, streaming, tool).plan(),
                Plan {
                    placement: Placement::Parked,
                    visible: true,
                    suspend: false
                }
            );
        }
    }

    #[test]
    fn no_need_hides_and_suspends_and_never_parks() {
        let plan = need(false, false, false).plan();
        assert_eq!(
            plan,
            Plan {
                placement: Placement::Hidden,
                visible: false,
                suspend: true
            }
        );
        assert!(!need(false, false, false).awake());
        assert!(need(false, true, false).awake());
        assert!(need(false, false, true).awake());
        assert!(need(true, false, false).awake());
    }

    #[test]
    fn inputs_combine_into_the_need_and_tools_count() {
        let start = Instant::now();
        let mut life = Lifecycle::new(Timeouts::default(), start);
        assert_eq!(life.need(), need(false, false, false));
        life.set_shown(true, start);
        life.set_streaming(true, start);
        life.begin_tool(start);
        life.begin_tool(start);
        assert_eq!(life.need(), need(true, true, true));
        life.end_tool(start);
        assert!(life.need().tool, "one tool is still running");
        life.end_tool(start);
        life.end_tool(start); // an extra end never underflows
        assert!(!life.need().tool);
        life.set_shown(false, start);
        life.set_streaming(false, start);
        assert_eq!(life.need(), need(false, false, false));
    }

    #[test]
    fn a_hidden_bot_with_no_need_is_due_for_idle_close_after_the_timeout() {
        let start = Instant::now();
        let timeouts = Timeouts {
            idle_close: Duration::from_secs(600),
            low_memory_after: Duration::from_secs(30),
        };
        let mut life = Lifecycle::new(timeouts, start);
        life.set_shown(true, start);
        assert!(!life.idle_close_due(start + Duration::from_secs(9999)));
        life.set_shown(false, start);
        assert_eq!(
            life.next_deadline(start),
            Some(start + Duration::from_secs(600))
        );
        assert!(!life.idle_close_due(start + Duration::from_secs(599)));
        assert!(life.idle_close_due(start + Duration::from_secs(600)));
        // Any need restarts the clock.
        life.begin_tool(start + Duration::from_secs(300));
        assert!(!life.idle_close_due(start + Duration::from_secs(9999)));
        life.end_tool(start + Duration::from_secs(400));
        assert!(!life.idle_close_due(start + Duration::from_secs(999)));
        assert!(life.idle_close_due(start + Duration::from_secs(1000)));
    }

    #[test]
    fn a_hidden_bot_kept_awake_over_the_timeout_gets_the_low_memory_target_until_shown() {
        let start = Instant::now();
        let timeouts = Timeouts {
            idle_close: Duration::from_secs(600),
            low_memory_after: Duration::from_secs(30),
        };
        let mut life = Lifecycle::new(timeouts, start);
        life.begin_tool(start);
        assert_eq!(
            life.memory_level(start + Duration::from_secs(29)),
            MemoryLevel::Normal
        );
        assert_eq!(
            life.next_deadline(start),
            Some(start + Duration::from_secs(30))
        );
        // A timer that has fired is not a deadline any more, so the engine's timer never spins on it.
        assert_eq!(life.next_deadline(start + Duration::from_secs(30)), None);
        assert_eq!(
            life.memory_level(start + Duration::from_secs(30)),
            MemoryLevel::Low
        );
        // A bot that is awake is never idle-closed.
        assert!(!life.idle_close_due(start + Duration::from_secs(9999)));
        // It is back to Normal the moment it is shown.
        life.set_shown(true, start + Duration::from_secs(40));
        assert_eq!(
            life.memory_level(start + Duration::from_secs(41)),
            MemoryLevel::Normal
        );
        // And it never gets Low while it is suspended: no need, no Low.
        life.set_shown(false, start + Duration::from_secs(50));
        life.end_tool(start + Duration::from_secs(50));
        assert_eq!(
            life.memory_level(start + Duration::from_secs(999)),
            MemoryLevel::Normal
        );
    }

    #[test]
    fn a_streaming_page_never_gets_the_low_memory_target() {
        // Low trims caches and adds page-fault latency: it costs frames, so a page that streams to a phone, with or
        // without a tool running too, keeps the Normal target however long it stays awake out of sight.
        let start = Instant::now();
        let timeouts = Timeouts {
            idle_close: Duration::from_secs(600),
            low_memory_after: Duration::from_secs(30),
        };
        let later = start + Duration::from_secs(300);
        let mut streaming = Lifecycle::new(timeouts, start);
        streaming.set_streaming(true, start);
        assert_eq!(streaming.memory_level(later), MemoryLevel::Normal);
        assert_eq!(
            streaming.next_deadline(start),
            None,
            "no timer runs for a streaming page"
        );
        let mut both = Lifecycle::new(timeouts, start);
        both.begin_tool(start);
        both.set_streaming(true, start + Duration::from_secs(5));
        assert_eq!(both.memory_level(later), MemoryLevel::Normal);
        // When the stream ends and the tool is still at work, the clock starts then, not from the tool's start.
        both.set_streaming(false, start + Duration::from_secs(100));
        assert_eq!(
            both.memory_level(start + Duration::from_secs(120)),
            MemoryLevel::Normal
        );
        assert_eq!(
            both.memory_level(start + Duration::from_secs(130)),
            MemoryLevel::Low
        );
    }
}
