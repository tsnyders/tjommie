//! Which variant the browser panel's motion uses (Phase 3 plan T3.7; spec §5 Phase 2 decision, condition 3). The live
//! variant moves and clips the page's window with the panel every frame, and is the default: the owner prefers it ("I love
//! the Live Variant"). At each motion's start the panel falls back to the snapshot variant, which slides a still picture
//! of the page, when the last live motion missed the frame budget or the PC is busy.
//!
//! What the fallback cannot do:
//! - it decides per motion, so the first live motion after the load rises can still miss;
//! - it cannot rescue a saturated CPU: at 100% CPU both variants missed in the spike (live 46–53 ms, snapshot 27–66 ms);
//! - snapshot frames are lighter but still do layout, so the panel needs a mostly quiet PC to meet 16.7 ms;
//! - the 16.7 ms budget assumes a display of 60 Hz or faster: on a slower one (a 30 Hz television, a 48 Hz battery-saving
//!   laptop panel) every live motion's p95 is at least one refresh (20.8 ms at 48 Hz), so after the first motion the
//!   panel stays on the snapshot variant, trying live again once a minute and missing each time (review F10).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The frame budget: a live motion whose p95 frame interval went over it hands the next motions to the snapshot variant.
pub const BUDGET_MS: f32 = 16.7;
/// How long a live miss counts. After this long with no miss, live is tried again.
pub const MISS_MEMORY: Duration = Duration::from_secs(60);
/// System CPU over the last second above this picks the snapshot variant: the load at which the spike measured the snapshot
/// variant still passing (15.85 and 16.49 ms at 53% and 62%).
pub const CPU_LIMIT: f32 = 60.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelVariant {
    /// The page's window moves with the panel, clipped to it, every frame.
    Live,
    /// A picture of the page, taken at rest, slides with the panel; the live page comes back at rest.
    Snapshot,
}

impl PanelVariant {
    /// The variant for a motion starting `now`: the last live motion's p95 frame interval and when it ended, and the
    /// system CPU over the last second in percent.
    pub fn choose(
        last_live_p95: Option<f32>,
        last_live_at: Option<Instant>,
        now: Instant,
        system_cpu_1s: f32,
    ) -> Self {
        let recent_miss = match (last_live_p95, last_live_at) {
            (Some(p95), Some(at)) => {
                p95 > BUDGET_MS && now.saturating_duration_since(at) < MISS_MEMORY
            }
            _ => false,
        };
        if recent_miss || system_cpu_1s > CPU_LIMIT {
            PanelVariant::Snapshot
        } else {
            PanelVariant::Live
        }
    }
}

/// The p95 frame interval in ms of a motion: the nearest rank over the intervals between consecutive frame starts
/// (`tools/perf/frame-stats.py`'s app intervals), from QPC ticks at `frequency` per second. `None` with fewer than two.
pub fn p95_interval_ms(starts: &[i64], frequency: i64) -> Option<f32> {
    if frequency <= 0 {
        return None;
    }
    let mut intervals: Vec<f64> = starts
        .windows(2)
        .filter_map(|pair| match pair {
            [a, b] => Some((b - a) as f64 * 1000. / frequency as f64),
            _ => None,
        })
        .collect();
    intervals.sort_by(f64::total_cmp);
    let rank = (0.95 * intervals.len() as f64).ceil() as usize;
    intervals.get(rank.checked_sub(1)?).map(|ms| *ms as f32)
}

/// The system's idle and busy time so far, in 100 ns units (`GetSystemTimes`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    /// Kernel plus user time; the kernel time includes the idle time.
    pub total: u64,
}

/// The CPU use between two readings, in percent; `None` when no time passed between them.
pub fn cpu_percent(earlier: CpuTimes, later: CpuTimes) -> Option<f32> {
    let total = later
        .total
        .checked_sub(earlier.total)
        .filter(|total| *total > 0)?;
    let idle = later.idle.checked_sub(earlier.idle)?.min(total);
    Some(((total - idle) as f64 * 100. / total as f64) as f32)
}

impl CpuTimes {
    /// The system's times now; `None` if Windows won't say.
    pub fn now() -> Option<CpuTimes> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::GetSystemTimes;
        let (mut idle, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: three FILETIMEs Windows writes into; nothing else is touched.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Some(CpuTimes {
            idle: ticks(idle),
            total: ticks(kernel).saturating_add(ticks(user)),
        })
    }
}

/// The system CPU over about the last second: a thread of its own reads the system's times every 250 ms and keeps the last
/// 1.5 s, so the panel can ask at a motion's start without waiting.
pub struct CpuMeter(Arc<Mutex<VecDeque<(Instant, CpuTimes)>>>);

const CPU_SAMPLE: Duration = Duration::from_millis(250);
const CPU_KEPT: usize = 6;

impl CpuMeter {
    /// Starts the sampling thread; it ends when the meter drops.
    pub fn start() -> CpuMeter {
        let samples = Arc::new(Mutex::new(VecDeque::with_capacity(CPU_KEPT + 1)));
        let weak = Arc::downgrade(&samples);
        let thread = std::thread::Builder::new().name("cpu-meter".into());
        // unguarded: a thread of its own, off the UI thread, where a panic ends only it
        let spawned = thread.spawn(move || {
            while let Some(samples) = weak.upgrade() {
                if let (Some(times), Ok(mut samples)) = (CpuTimes::now(), samples.lock()) {
                    samples.push_back((Instant::now(), times));
                    if samples.len() > CPU_KEPT {
                        samples.pop_front();
                    }
                }
                drop(samples);
                std::thread::sleep(CPU_SAMPLE);
            }
        });
        if let Err(error) = spawned {
            log::warn!("No CPU meter for the panel's fallback: {error}");
        }
        CpuMeter(samples)
    }

    /// The CPU use since the reading nearest to one second ago, in percent; 0 until a reading is old enough (the meter
    /// has just started), so an unknown load never forces the snapshot.
    pub fn last_second(&self) -> f32 {
        let (now, later) = (Instant::now(), CpuTimes::now());
        let Ok(samples) = self.0.lock() else {
            return 0.;
        };
        let age = |at: &Instant| now.saturating_duration_since(*at);
        let earlier = samples
            .iter()
            .filter(|(at, _)| age(at) >= Duration::from_millis(500))
            .min_by_key(|(at, _)| age(at).abs_diff(Duration::from_secs(1)));
        match (earlier, later) {
            (Some((_, earlier)), Some(later)) => cpu_percent(*earlier, later).unwrap_or(0.),
            _ => 0.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_is_the_busy_share_between_two_readings() {
        let earlier = CpuTimes {
            idle: 1_000,
            total: 4_000,
        };
        let later = CpuTimes {
            idle: 1_350,
            total: 5_000,
        };
        // 1000 units passed, 350 of them idle: 65% busy.
        assert_eq!(cpu_percent(earlier, later), Some(65.));
        assert_eq!(cpu_percent(later, later), None, "no time passed");
        assert_eq!(cpu_percent(later, earlier), None, "readings out of order");
    }

    #[test]
    fn the_meter_reads_the_system_once_a_reading_is_old_enough() {
        let meter = CpuMeter::start();
        assert_eq!(
            meter.last_second(),
            0.,
            "just started: unknown reads as quiet"
        );
        std::thread::sleep(Duration::from_millis(1100));
        let cpu = meter.last_second();
        assert!((0. ..=100.).contains(&cpu), "{cpu}");
        let times = CpuTimes::now().expect("GetSystemTimes");
        assert!(times.total >= times.idle);
    }

    #[test]
    fn choose() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let quiet = 20.;
        assert_eq!(
            PanelVariant::choose(None, None, at(0), quiet),
            PanelVariant::Live,
            "no history: live"
        );
        assert_eq!(
            PanelVariant::choose(Some(17.2), Some(at(0)), at(10), quiet),
            PanelVariant::Snapshot,
            "a 17.2 ms p95 within 60 s: the snapshot"
        );
        assert_eq!(
            PanelVariant::choose(Some(17.2), Some(at(0)), at(61), quiet),
            PanelVariant::Live,
            "61 s later: live again"
        );
        assert_eq!(
            PanelVariant::choose(None, None, at(0), 65.),
            PanelVariant::Snapshot,
            "CPU at 65%: the snapshot"
        );
        assert_eq!(
            PanelVariant::choose(Some(15.0), Some(at(0)), at(10), quiet),
            PanelVariant::Live,
            "a live motion within budget keeps live"
        );
        assert_eq!(
            PanelVariant::choose(Some(BUDGET_MS), Some(at(0)), at(1), CPU_LIMIT),
            PanelVariant::Live,
            "exactly at the budget and the CPU limit is not over them"
        );
    }

    #[test]
    fn p95_is_the_nearest_rank_of_the_start_intervals() {
        // 1 ms ticks; 20 intervals: 19 of 10 ms and one of 40 ms, as frame-stats.py's self-test.
        let mut starts = vec![0i64];
        for gap in std::iter::repeat_n(10, 19).chain([40]) {
            starts.push(starts.last().copied().unwrap() + gap);
        }
        assert_eq!(p95_interval_ms(&starts, 1000), Some(10.));
        // Two slow frames out of 20 reach the p95.
        let mut starts = vec![0i64];
        for gap in std::iter::repeat_n(10, 18).chain([40, 40]) {
            starts.push(starts.last().copied().unwrap() + gap);
        }
        assert_eq!(p95_interval_ms(&starts, 1000), Some(40.));
        assert_eq!(
            p95_interval_ms(&[5], 1000),
            None,
            "one frame has no interval"
        );
        assert_eq!(p95_interval_ms(&[], 1000), None);
    }
}
