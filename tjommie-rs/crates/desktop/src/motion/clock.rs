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
use gpui::{App, Global, IntoElement, Styled as _, Window, canvas};

use crate::guard::guarded_async;

/// The step of a motion's first frame.
const FIRST: f32 = 1. / 100.;
/// The frames the log keeps: about 34 s at 60 fps.
const LOG: usize = 2048;
/// The frame interval of an inactive window with nothing live: 30 fps.
const INACTIVE: Duration = Duration::from_nanos(33_333_333);

/// A frame: when it began and when its paint ended (QPC), and whether something moved in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub begin_qpc: i64,
    pub painted_qpc: i64,
    pub moving: bool,
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
    /// A throttled frame is on its way.
    throttled: bool,
    log: VecDeque<Frame>,
    trace: Option<BufWriter<File>>,
}

impl Global for Clock {}

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
            throttled: false,
            log: VecDeque::with_capacity(LOG),
            trace: None,
        }
    }
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
    let clock = cx.default_global::<Clock>();
    let now = Instant::now();
    clock.dt = clock
        .last_moving
        .map_or(FIRST, |last| (now - last).as_secs_f32());
    clock.frame = now;
    clock.frame_qpc = qpc();
    clock.moving = false;
    clock.throttled = false;
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
    let clock = cx.default_global::<Clock>();
    clock.moving = true;
    match next_frame(window.is_window_active(), live) {
        None => window.request_animation_frame(),
        Some(delay) => {
            if !std::mem::replace(&mut clock.throttled, true) {
                refresh_after(delay, window, cx);
            }
        }
    }
}

/// A frame after `delay`, for something that next changes then (a blink, a beat).
pub fn wake_in(delay: Duration, window: &mut Window, cx: &mut App) {
    refresh_after(delay, window, cx);
}

/// When a motion's next frame comes: `None` at the display's rate, or after a delay (gate condition 5).
pub fn next_frame(active: bool, live: bool) -> Option<Duration> {
    (!active && !live).then_some(INACTIVE)
}

fn refresh_after(delay: Duration, window: &mut Window, cx: &mut App) {
    let timer = cx.background_executor().timer(delay);
    window
        .spawn(cx, async move |cx| {
            guarded_async("draw a timed frame", async move {
                timer.await;
                let _ = cx.update(|window, _| window.refresh());
            })
            .await
        })
        .detach();
}

/// The element that ends each frame: the root view's last child, so its paint comes after the rest.
pub fn painted() -> impl IntoElement {
    canvas(|_, _, _| {}, |_, _, _, cx| end_frame(cx))
        .absolute()
        .size_0()
}

fn end_frame(cx: &mut App) {
    let clock = cx.default_global::<Clock>();
    let frame = Frame {
        begin_qpc: clock.frame_qpc,
        painted_qpc: qpc(),
        moving: clock.moving,
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
