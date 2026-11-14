//! Crash handling (gate condition 4). A panic nothing catches is a crash: the hook appends it to
//! `<root>/logs/desktop-crash.log` with the thread, a backtrace (line tables are kept in release), a timestamp and the
//! version, then aborts. The abort is a fail-fast, so Windows Error Reporting sees a crash and restarts the app with
//! `--hidden --after-crash` once it has run 60 s; the daemon, the app's child, exits with it and restarts with it.
//! A panic inside `catching` (the UI guard) is logged as caught and unwinds to its caller instead.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::Path;

use windows::Win32::System::Recovery::{
    REGISTER_APPLICATION_RESTART_FLAGS, RegisterApplicationRestart,
};
use windows::core::w;

thread_local! {
    /// How many `catching` calls are running on this thread. A panic under one is caught, not a crash.
    static CATCHING: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` and catches a panic in it, which the hook logs as caught instead of crashing the app.
pub fn catching<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    CATCHING.with(|depth| depth.set(depth.get() + 1));
    let result = std::panic::catch_unwind(AssertUnwindSafe(f));
    CATCHING.with(|depth| depth.set(depth.get().saturating_sub(1)));
    result
}

/// Installs the panic hook and registers the restart. Call once at start, right after `log::init`.
pub fn install(root: &Path) {
    let file = root.join("logs").join("desktop-crash.log");
    std::panic::set_hook(Box::new(move |info| {
        let caught = CATCHING.try_with(|depth| depth.get() > 0).unwrap_or(false);
        crate::log::append(&file, &entry(info, caught));
        if !caught {
            std::process::abort();
        }
    }));
    // SAFETY: the command line is a static NUL-terminated UTF-16 string.
    if let Err(error) = unsafe {
        RegisterApplicationRestart(
            w!("--hidden --after-crash"),
            REGISTER_APPLICATION_RESTART_FLAGS(0),
        )
    } {
        log::warn!("Could not register the restart after a crash: {error}");
    }
}

fn entry(info: &PanicHookInfo<'_>, caught: bool) -> String {
    let thread = std::thread::current();
    let kind = if caught { "caught panic" } else { "crash" };
    format!(
        "{} Tjommie {} {kind} in thread '{}': {info}\n{}\n",
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

    /// Set in the child half of `hook_writes_log`: this test binary run again, which crashes on purpose.
    const CHILD_ROOT: &str = "TJOMMIE_CRASH_TEST_ROOT";

    #[test]
    fn hook_writes_log() {
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            install(Path::new(&root));
            // Windows holds the restart registration; read its command line back.
            let mut line = [0u16; 64];
            let mut len = line.len() as u32;
            // SAFETY: `line` holds `len` UTF-16 units, the most the call writes.
            unsafe {
                GetApplicationRestartSettings(
                    GetCurrentProcess(),
                    Some(PWSTR(line.as_mut_ptr())),
                    &mut len,
                    None,
                )
            }
            .unwrap();
            let registered = String::from_utf16_lossy(&line[..len as usize - 1]);
            assert_eq!(registered, "--hidden --after-crash");
            assert!(catching(|| panic!("planted caught panic")).is_err());
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
        assert!(
            log.contains(" crash in thread '") && log.contains("planted crash"),
            "{log}"
        );
        // The backtrace: this test's own frame, by symbol.
        assert!(
            log.contains("desktop_host::crash::tests::hook_writes_log"),
            "{log}"
        );
        // The caught panic comes first and the process carried on to the crash.
        assert!(
            log.find("planted caught panic") < log.find("planted crash"),
            "{log}"
        );
    }
}
