//! The performance counter (QPC), the clock PresentMon stamps presents with, so the app's frame log lines up with
//! PresentMon's (`tools/perf/frame-stats.py`). The two kernel32 calls are declared here: the windows crate's
//! `Win32_System_Performance` feature would change the feature set GPUI is built with, and rebuild it.

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

/// The counter now.
pub fn qpc() -> i64 {
    let mut count = 0;
    // SAFETY: writes one i64 through the pointer; it can't fail since Windows XP.
    unsafe { QueryPerformanceCounter(&mut count) };
    count
}

/// The counter's ticks per second.
pub fn qpc_frequency() -> i64 {
    let mut frequency = 0;
    // SAFETY: as above.
    unsafe { QueryPerformanceFrequency(&mut frequency) };
    frequency
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_up_at_its_frequency() {
        let first = qpc();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let elapsed = (qpc() - first) as f64 / qpc_frequency() as f64;
        assert!((0.015..1.0).contains(&elapsed), "{elapsed} s");
    }
}
