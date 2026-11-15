//! Crash handling (gate condition 4). A panic on the UI thread that nothing catches is a crash: the hook appends it to
//! `<root>/logs/desktop-crash.log` with the thread, a backtrace (line tables are kept in release), a timestamp and the
//! version, then aborts. The abort is a fail-fast, so Windows Error Reporting sees a crash and restarts the app with
//! `--hidden --after-crash` once it has run 60 s; the daemon, the app's child, exits with it and restarts with it.
//! A panic inside `catching` (the UI guard) is logged as caught and unwinds to its caller instead, and a panic on any
//! other thread is logged and unwinds that thread.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::System::Recovery::{
    RESTART_NO_PATCH, RESTART_NO_REBOOT, RegisterApplicationRestart,
};
use windows::core::w;

thread_local! {
    /// How many `catching` calls are running on this thread. A panic under one is caught, not a crash.
    static CATCHING: Cell<u32> = const { Cell::new(0) };
    /// Whether this is the thread that called `install`, GPUI's UI thread. Only its panics are crashes: elsewhere a
    /// panic unwinds its own thread, which tokio turns into a failed task (review F1).
    static UI_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// How often a panic from one place is written in full, so a handler that panics every frame can't flood the log;
/// the ones between are counted (review F7). A crash is always written.
const EVERY: Duration = Duration::from_secs(60);

/// Where recent panics came from, when one from there was last written, and how many from there were skipped since.
type Recent = Vec<(String, Instant, u32)>;

static RECENT: Mutex<Recent> = Mutex::new(Vec::new());

/// Runs `f` and catches a panic in it, which the hook logs as caught instead of crashing the app.
pub fn catching<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    CATCHING.with(|depth| depth.set(depth.get() + 1));
    let result = std::panic::catch_unwind(AssertUnwindSafe(f));
    CATCHING.with(|depth| depth.set(depth.get().saturating_sub(1)));
    result
}

/// Installs the panic hook and registers the restart. Call once at start, on the UI thread, right after `log::init`.
pub fn install(root: &Path) {
    let file = root.join("logs").join("desktop-crash.log");
    UI_THREAD.with(|ui| ui.set(true));
    std::panic::set_hook(Box::new(move |info| {
        let kind = if CATCHING.try_with(|depth| depth.get() > 0).unwrap_or(false) {
            "caught panic"
        } else if UI_THREAD.try_with(Cell::get).unwrap_or(false) {
            "crash"
        } else {
            "background panic"
        };
        let skipped = if kind == "crash" {
            Some(0)
        } else {
            let location = info.location().map(ToString::to_string).unwrap_or_default();
            let mut recent = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
            admit(&mut recent, &location, Instant::now())
        };
        if let Some(skipped) = skipped {
            crate::log::append(&file, &entry(info, kind, skipped));
        }
        if kind == "crash" {
            std::process::abort();
        }
    }));
    // After a crash or a hang, not after a patch or a Windows Update reboot: neither is a crash (review F6).
    // SAFETY: the command line is a static NUL-terminated UTF-16 string.
    if let Err(error) = unsafe {
        RegisterApplicationRestart(
            w!("--hidden --after-crash"),
            RESTART_NO_PATCH | RESTART_NO_REBOOT,
        )
    } {
        log::warn!("Could not register the restart after a crash: {error}");
    }
}

/// Whether to write a panic from `location` at `now`: `Some(how many from there were skipped since the last one
/// written)`, or `None` to skip it.
fn admit(recent: &mut Recent, location: &str, now: Instant) -> Option<u32> {
    match recent.iter_mut().find(|(at, ..)| at == location) {
        None => {
            recent.push((location.to_owned(), now, 0));
            Some(0)
        }
        Some((_, written, skipped)) if now.duration_since(*written) < EVERY => {
            *skipped += 1;
            None
        }
        Some((_, written, skipped)) => {
            *written = now;
            Some(std::mem::take(skipped))
        }
    }
}

fn entry(info: &PanicHookInfo<'_>, kind: &str, skipped: u32) -> String {
    let thread = std::thread::current();
    let skipped = match skipped {
        0 => String::new(),
        n => format!("({n} more from there since the last entry)\n"),
    };
    format!(
        "{} Tjommie {} {kind} in thread '{}': {info}\n{skipped}{}\n",
        jiff::Timestamp::now(),
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("unnamed"),
        Backtrace::force_capture(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Recovery::GetApplicationRestartSettings;
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::core::PWSTR;

    #[test]
    fn a_storm_is_written_once_a_minute() {
        let mut recent = Vec::new();
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        assert_eq!(admit(&mut recent, "a.rs:1:1", at(0)), Some(0));
        assert_eq!(admit(&mut recent, "a.rs:1:1", at(1)), None);
        assert_eq!(admit(&mut recent, "b.rs:2:2", at(2)), Some(0));
        assert_eq!(admit(&mut recent, "a.rs:1:1", at(30)), None);
        assert_eq!(admit(&mut recent, "a.rs:1:1", at(61)), Some(2));
        assert_eq!(admit(&mut recent, "a.rs:1:1", at(62)), None);
    }

    /// Set in the child half of `hook_writes_log`: this test binary run again, which crashes on purpose.
    const CHILD_ROOT: &str = "TJOMMIE_CRASH_TEST_ROOT";

    #[test]
    fn hook_writes_log() {
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            install(Path::new(&root));
            // Windows holds the restart registration; read its command line and flags back.
            let mut line = [0u16; 64];
            let mut len = line.len() as u32;
            let mut flags = 0;
            // SAFETY: `line` holds `len` UTF-16 units, the most the call writes.
            unsafe {
                GetApplicationRestartSettings(
                    GetCurrentProcess(),
                    Some(PWSTR(line.as_mut_ptr())),
                    &mut len,
                    Some(&mut flags),
                )
            }
            .unwrap();
            let registered = String::from_utf16_lossy(&line[..len as usize - 1]);
            assert_eq!(registered, "--hidden --after-crash");
            assert_eq!(flags, (RESTART_NO_PATCH | RESTART_NO_REBOOT).0);
            // A storm from one place is written once a minute (review F7).
            for _ in 0..3 {
                assert!(catching(|| panic!("planted caught panic")).is_err());
            }
            // Another thread's panic unwinds that thread only (review F1); tokio contains its tasks' panics this way.
            assert!(
                std::thread::spawn(|| panic!("planted background panic"))
                    .join()
                    .is_err()
            );
            panic!("planted crash");
        }
        let root = tempfile::tempdir().unwrap();
        let output = crate::spawn::command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash::tests::hook_writes_log",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(CHILD_ROOT, root.path())
            .output()
            .unwrap();
        let child = String::from_utf8_lossy(&output.stderr);
        // An uncaught panic aborts: a fail-fast, STATUS_STACK_BUFFER_OVERRUN, which Windows treats as a crash.
        assert_eq!(
            output.status.code(),
            Some(0xC000_0409_u32 as i32),
            "child stderr: {child}"
        );
        let log =
            std::fs::read_to_string(root.path().join("logs").join("desktop-crash.log")).unwrap();
        let version = concat!(" Tjommie ", env!("CARGO_PKG_VERSION"), " ");
        assert!(log.contains(version), "{log}");
        assert!(
            log.contains("caught panic in thread '") && log.contains("planted caught panic"),
            "{log}"
        );
        assert_eq!(log.matches("planted caught panic").count(), 1, "{log}");
        assert!(
            log.contains(" crash in thread '") && log.contains("planted crash"),
            "{log}"
        );
        // The backtrace: this test's own frame, by symbol.
        assert!(
            log.contains("desktop_host::crash::tests::hook_writes_log"),
            "{log}"
        );
        assert!(
            log.contains("background panic in thread '")
                && log.contains("planted background panic"),
            "{log}"
        );
        // In order, and the process carried on past the caught and the background panic to the crash.
        let at = |text| log.find(text).unwrap_or(usize::MAX);
        assert!(
            at("planted caught panic") < at("planted background panic")
                && at("planted background panic") < at("planted crash"),
            "{log}"
        );
    }
}
