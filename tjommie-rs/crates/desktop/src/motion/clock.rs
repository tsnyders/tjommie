//! The motion clock (Phase 3 plan T2.3; GPUI limit 13, gate condition 5). One clock per app: the root view starts each
//! frame on it, every motion steps by its `dt`, and anything still moving asks for the next frame through it, so the
//! window draws at the display's rate only while something moves. It keeps the app's own frame log, when each frame
//! began and when its paint ended (QPC), the second measure next to PresentMon; `--trace <csv>` writes the log in
//! `tools/perf/frame-stats.py`'s format.
//!
//! The throttle (gate condition 5): while the window is inactive the next frame comes at 30 fps, unless something
//! live shows (`Live`): a remote live stream, the browser panel's live variant moving, or a motion check. GPUI's own
//! `inactive_frame_interval` is lifted (T0.5), so this is the only throttle.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use desktop_host::perf::qpc;
use gpui::{App, Global, IntoElement, Styled as _, Window, canvas, deferred};

use crate::guard::guarded_async;

/// The step of a motion's first frame.
const FIRST: f32 = 1. / 100.;
/// The frames the log keeps: about 34 s at 60 fps.
const LOG: usize = 2048;
/// The frame interval of an inactive window with nothing live: 30 fps.
const INACTIVE: Duration = Duration::from_nanos(33_333_333);

/// A frame mark: the avatar's flight drew in it (`mark`).
pub const FLIGHT: u8 = 1;
/// A frame mark: a toast moved in it.
pub const TOAST: u8 = 2;

/// A frame: when it began and when its paint ended (QPC), whether something moved in it, and which marked motions drew
/// in it (`FLIGHT`, `TOAST`): the motion checks count those frames after a key (M7 review m3), as the eyes' blinks make
/// `moving` no measure of one motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub begin_qpc: i64,
    pub painted_qpc: i64,
    pub moving: bool,
    pub marks: u8,
}

pub struct Clock {
    started: Instant,
    /// When the current frame began.
    frame: Instant,
    frame_qpc: i64,
    /// The seconds this frame's motions step by.
    dt: f32,
    /// When the last frame that moved began; `None` after a frame where nothing moved.
    last_moving: Option<Instant>,
    /// Something asked for another frame during this frame.
    moving: bool,
    /// The marked motions that drew in this frame.
    marks: u8,
    /// A throttled frame is on its way: only its own timer clears this, so a redraw in between starts no second
    /// chain (review F2).
    throttled: bool,
    /// When the pending `wake_in` frame comes.
    wake_at: Option<Instant>,
    log: VecDeque<Frame>,
    trace: Option<BufWriter<File>>,
    /// A view test's frame step, so its motions advance the same on any machine.
    #[cfg(test)]
    fixed: Option<f32>,
}

impl Global for Clock {}

/// How a motion's next frame comes.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// At the display's rate.
    Display,
    /// A throttled frame after the delay: start its timer.
    Timer(Duration),
    /// A throttled frame is already on its way.
    Pending,
}

impl Clock {
    fn begin(&mut self) {
        let now = Instant::now();
        self.dt = self
            .last_moving
            .map_or(FIRST, |last| (now - last).as_secs_f32());
        #[cfg(test)]
        if let Some(fixed) = self.fixed {
            self.dt = fixed;
        }
        self.frame = now;
        self.frame_qpc = qpc();
        self.moving = false;
        self.marks = 0;
    }

    /// Something moves and asks for the next frame.
    fn request(&mut self, active: bool, live: bool) -> Next {
        self.moving = true;
        match next_frame(active, live) {
            None => Next::Display,
            Some(_) if self.throttled => Next::Pending,
            Some(delay) => {
                self.throttled = true;
                Next::Timer(delay)
            }
        }
    }

    /// The throttled frame's timer fired.
    fn fired(&mut self) {
        self.throttled = false;
    }

    /// Whether a wake at `at` needs a timer: not while an earlier or equal one is pending.
    fn wake(&mut self, at: Instant) -> bool {
        if self.wake_at.is_some_and(|pending| pending <= at) {
            return false;
        }
        self.wake_at = Some(at);
        true
    }

    /// The wake at `at` fired.
    fn woke(&mut self, at: Instant) {
        if self.wake_at == Some(at) {
            self.wake_at = None;
        }
    }
}

impl Default for Clock {
    fn default() -> Self {
        let now = Instant::now();
        Clock {
            started: now,
            frame: now,
            frame_qpc: qpc(),
            dt: FIRST,
            last_moving: None,
            moving: false,
            marks: 0,
            throttled: false,
            wake_at: None,
            log: VecDeque::with_capacity(LOG),
            trace: None,
            #[cfg(test)]
            fixed: None,
        }
    }
}

/// Every frame steps its motions by `dt` seconds, as at a steady display rate: a view test's frames come microseconds
/// apart.
#[cfg(test)]
pub fn fixed_step(dt: f32, cx: &mut App) {
    cx.default_global::<Clock>().fixed = Some(dt);
}

/// Starts the clock, writing each frame to `trace` (`--trace <csv>`) when given.
pub fn init(trace: Option<PathBuf>, cx: &mut App) {
    let trace = trace.and_then(|path| match File::create(&path) {
        Ok(file) => {
            let mut trace = BufWriter::new(file);
            let _ = trace.write_all(b"begin_qpc,painted_qpc\n");
            Some(trace)
        }
        Err(error) => {
            log::warn!("No frame trace at {}: {error}", path.display());
            None
        }
    });
    cx.set_global(Clock {
        trace,
        ..Clock::default()
    });
}

/// Starts a frame: the root view calls this first in its render.
pub fn begin_frame(cx: &mut App) {
    cx.default_global::<Clock>().begin();
}

/// Marks this frame as one `motion` drew in (`FLIGHT`, `TOAST`), for the motion checks.
pub fn mark(motion: u8, cx: &mut App) {
    cx.default_global::<Clock>().marks |= motion;
}

/// The seconds every motion steps by this frame: the time since the last frame that moved (1/100 s to start one).
pub fn dt(cx: &App) -> f32 {
    cx.try_global::<Clock>().map_or(FIRST, |clock| clock.dt)
}

/// The seconds since the clock started: the eyes' time.
pub fn now(cx: &App) -> f32 {
    cx.try_global::<Clock>()
        .map_or(0., |clock| (clock.frame - clock.started).as_secs_f32())
}

/// Something still moves: the next frame comes at the display's rate, or at 30 fps while the window is inactive and
/// nothing live shows.
pub fn keep_moving(window: &mut Window, cx: &mut App) {
    let live = Live::any(cx);
    let active = window.is_window_active();
    match cx.default_global::<Clock>().request(active, live) {
        Next::Display => window.request_animation_frame(),
        Next::Timer(delay) => timed_frame(delay, window, cx, Clock::fired),
        Next::Pending => {}
    }
}

/// A frame after `delay`, for something that next changes then (a blink, a beat). One wake is pending at a time, the
/// earliest; a later one is asked for again at the frames before it.
pub fn wake_in(delay: Duration, window: &mut Window, cx: &mut App) {
    let at = Instant::now() + delay;
    if cx.default_global::<Clock>().wake(at) {
        timed_frame(delay, window, cx, move |clock| clock.woke(at));
    }
}

/// When a motion's next frame comes: `None` at the display's rate, or after a delay (gate condition 5).
pub fn next_frame(active: bool, live: bool) -> Option<Duration> {
    (!active && !live).then_some(INACTIVE)
}

/// Draws a frame after `delay`, settling the clock's state for it first.
fn timed_frame(
    delay: Duration,
    window: &mut Window,
    cx: &mut App,
    settle: impl FnOnce(&mut Clock) + 'static,
) {
    let timer = cx.background_executor().timer(delay);
    window
        .spawn(cx, async move |cx| {
            guarded_async("draw a timed frame", async move {
                timer.await;
                let _ = cx.update(|window, cx| {
                    settle(cx.default_global::<Clock>());
                    window.refresh();
                });
            })
            .await
        })
        .detach();
}

/// The element that ends each frame: the root view's last child, deferred at the highest priority, so its paint comes
/// after everything else's, the deferred overlays included (M2 review F10).
pub fn painted() -> impl IntoElement {
    deferred(
        canvas(|_, _, _| {}, |_, _, _, cx| end_frame(cx))
            .absolute()
            .size_0(),
    )
    .with_priority(usize::MAX)
}

fn end_frame(cx: &mut App) {
    let clock = cx.default_global::<Clock>();
    let frame = Frame {
        begin_qpc: clock.frame_qpc,
        painted_qpc: qpc(),
        moving: clock.moving,
        marks: clock.marks,
    };
    if clock.log.len() == LOG {
        clock.log.pop_front();
    }
    clock.log.push_back(frame);
    if let Some(trace) = &mut clock.trace {
        let _ = writeln!(trace, "{},{}", frame.begin_qpc, frame.painted_qpc);
        if !frame.moving {
            let _ = trace.flush();
        }
    }
    clock.last_moving = clock.moving.then_some(clock.frame);
}

/// The frame log, oldest first.
pub fn frames(cx: &App) -> Vec<Frame> {
    cx.try_global::<Clock>()
        .map(|clock| clock.log.iter().copied().collect())
        .unwrap_or_default()
}

/// Something live shows: a remote live stream view, the browser panel's live variant moving, or a motion check. While
/// any holds it, the clock keeps the display's rate in an inactive window too.
#[derive(Default)]
pub struct Live(Rc<Cell<usize>>);

impl Global for Live {}

/// Holds `Live` until dropped.
pub struct LiveGuard(Rc<Cell<usize>>);

impl Live {
    pub fn hold(cx: &mut App) -> LiveGuard {
        let count = cx.default_global::<Live>().0.clone();
        count.set(count.get() + 1);
        LiveGuard(count)
    }

    pub fn any(cx: &App) -> bool {
        cx.try_global::<Live>().is_some_and(|live| live.0.get() > 0)
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_throttles_unless_live() {
        assert_eq!(next_frame(true, false), None, "active: the display's rate");
        assert_eq!(next_frame(false, false), Some(INACTIVE), "inactive: 30 fps");
        assert!((INACTIVE.as_secs_f64() - 1. / 30.).abs() < 1e-6);
        assert_eq!(
            next_frame(false, true),
            None,
            "inactive, but live: the display's rate"
        );
        assert_eq!(next_frame(true, true), None);
    }

    #[test]
    fn a_redraw_mid_interval_starts_no_second_chain() {
        let mut clock = Clock::default();
        assert_eq!(
            clock.request(false, false),
            Next::Timer(INACTIVE),
            "inactive: a 30 fps chain starts"
        );
        // A hover, a notice or a store update redraws the window before the timer fires.
        clock.begin();
        assert_eq!(
            clock.request(false, false),
            Next::Pending,
            "no second chain"
        );
        clock.begin();
        assert_eq!(clock.request(false, false), Next::Pending, "nor a third");
        // The chain's own frame schedules its next one.
        clock.fired();
        clock.begin();
        assert_eq!(clock.request(false, false), Next::Timer(INACTIVE));
        assert_eq!(
            clock.request(true, false),
            Next::Display,
            "active again: the display's rate"
        );
    }

    #[test]
    fn one_wake_pends_and_the_earliest_wins() {
        let mut clock = Clock::default();
        let at = |secs| clock.started + Duration::from_secs(secs);
        let (one, two, three, five) = (at(1), at(2), at(3), at(5));
        assert!(clock.wake(three), "the first wake");
        assert!(
            !clock.wake(five),
            "a later one waits: its owner asks again at the next frame"
        );
        assert!(clock.wake(one), "an earlier one goes first");
        clock.woke(one);
        assert!(clock.wake(two), "after it, the next");
        clock.woke(three);
        assert!(
            !clock.wake(five),
            "the stale 3 s timer left the 2 s wake pending"
        );
    }

    #[gpui::test]
    fn the_frame_ends_after_the_overlays_paint(cx: &mut gpui::TestAppContext) {
        use gpui::{IntoElement as _, ParentElement as _, deferred, div, px};
        let overlay = Rc::new(Cell::new(0i64));
        let painted_at = overlay.clone();
        // An overlay, deferred as menus, dialogs and the flight are.
        let cx = crate::ui::primitives::test_window(cx, move |_, _| {
            let painted_at = painted_at.clone();
            div()
                .size_full()
                .child(
                    deferred(
                        canvas(|_, _, _| {}, move |_, _, _, _| painted_at.set(qpc())).size(px(10.)),
                    )
                    .with_priority(2),
                )
                .into_any_element()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let frame = cx.update(|_, cx| *frames(cx).last().expect("a frame"));
        assert!(overlay.get() > 0, "the overlay painted");
        assert!(
            frame.painted_qpc >= overlay.get(),
            "the frame's end is marked after the overlay's paint (M2 review F10)"
        );
    }

    #[gpui::test]
    fn live_holds_until_every_guard_drops(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(!Live::any(cx));
            let stream = Live::hold(cx);
            let check = Live::hold(cx);
            drop(stream);
            assert!(Live::any(cx), "the motion check still holds it");
            drop(check);
            assert!(!Live::any(cx));
        });
    }

    #[gpui::test]
    fn steps_by_the_time_since_the_last_moving_frame(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(None, cx);
            begin_frame(cx);
            assert_eq!(dt(cx), FIRST, "a motion's first frame");
            cx.global_mut::<Clock>().moving = true;
            end_frame(cx);
        });
        std::thread::sleep(Duration::from_millis(20));
        cx.update(|cx| {
            begin_frame(cx);
            assert!(
                (0.015..0.5).contains(&dt(cx)),
                "since the last moving frame: {}",
                dt(cx)
            );
            end_frame(cx);
            begin_frame(cx);
            assert_eq!(
                dt(cx),
                FIRST,
                "after a frame where nothing moved, a new motion starts"
            );
        });
    }

    /// M7 review m3: a frame keeps the motions marked in it, for the motion checks to count; the next starts with none.
    #[gpui::test]
    fn a_frame_keeps_the_motions_marked_in_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(None, cx);
            begin_frame(cx);
            mark(FLIGHT, cx);
            mark(TOAST, cx);
            end_frame(cx);
            begin_frame(cx);
            end_frame(cx);
            let frames = frames(cx);
            assert_eq!(frames[frames.len() - 2].marks, FLIGHT | TOAST);
            assert_eq!(frames[frames.len() - 1].marks, 0);
        });
    }

    #[gpui::test]
    fn logs_the_last_frames_and_traces_them(cx: &mut gpui::TestAppContext) {
        let folder = desktop_host::owner_safety::test_root().unwrap();
        let path = folder.path().join("trace.csv");
        cx.update(|cx| {
            init(Some(path.clone()), cx);
            for _ in 0..LOG + 5 {
                begin_frame(cx);
                end_frame(cx);
            }
            let frames = frames(cx);
            assert_eq!(frames.len(), LOG, "the ring keeps the last {LOG}");
            assert!(
                frames
                    .iter()
                    .all(|frame| frame.painted_qpc >= frame.begin_qpc)
            );
        });
        let trace = std::fs::read_to_string(&path).unwrap();
        let mut lines = trace.lines();
        assert_eq!(lines.next(), Some("begin_qpc,painted_qpc"));
        assert_eq!(lines.count(), LOG + 5, "the trace keeps every frame");
    }
}
