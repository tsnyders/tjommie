//! The performance counter (QPC), the clock PresentMon stamps presents with, so the app's frame log lines up with
//! PresentMon's (`tools/perf/frame-stats.py`). The two kernel32 calls are declared here: the windows crate's
//! `Win32_System_Performance` feature would change the feature set GPUI is built with, and rebuild it.

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    fn GetCurrentThread() -> isize;
    fn SetThreadPriority(thread: isize, priority: i32) -> i32;
}

#[link(name = "avrt")]
unsafe extern "system" {
    fn AvSetMmThreadCharacteristicsW(task: *const u16, index: *mut u32) -> isize;
}

/// The heavy-load experiment (Phase 3 plan T7.3): registers this thread with MMCSS as a "Games" task, which the
/// scheduler favours while the system is busy. The registration lasts for the thread's life.
pub fn mmcss_games() -> Result<(), String> {
    let task: Vec<u16> = "Games".encode_utf16().chain(std::iter::once(0)).collect();
    let mut index = 0u32;
    // SAFETY: a NUL-terminated wide string that outlives the call, and a u32 to write the task index into.
    let handle = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) };
    if handle == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// T7.3's second option: this thread at above-normal priority.
pub fn above_normal() -> Result<(), String> {
    const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
    // SAFETY: the pseudo handle of the calling thread, which needs no closing.
    let done = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) };
    if done == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// The motion-idle check (M7 review m8): puts the taskbar in front, so the app's window stays visible but is no longer
/// the active one. A process in the foreground may hand the foreground on.
pub fn put_taskbar_in_front() -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};
    // SAFETY: a lookup by class name, and a foreground request for the window it found.
    unsafe {
        let taskbar = FindWindowW(
            windows::core::w!("Shell_TrayWnd"),
            windows::core::PCWSTR::null(),
        )
        .map_err(|error| format!("no taskbar: {error}"))?;
        if !SetForegroundWindow(taskbar).as_bool() {
            return Err("Windows kept the app in front".to_owned());
        }
    }
    Ok(())
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
