//! One app per Windows session (Electron's single-instance lock and its `second-instance` → `showWindow`). The first
//! launch holds the named mutex `Local\<appId>`. A later launch sets the named event `Local\<appId>.show`, which the
//! first launch waits on to show its window, and exits. An event rather than a registered window message: GPUI owns
//! the window procedure and has no hook for one.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

pub enum Launch {
    /// The first launch: it holds the lock as long as its `Instance` lives.
    First(Instance),
    /// Another launch holds the lock and has been asked to show its window. Exit.
    Second,
}

pub struct Instance {
    _lock: Handle,
    show: Handle,
}

/// A kernel handle, closed on drop. Kernel handles may be used from any thread.
struct Handle(HANDLE);

// SAFETY: a kernel object handle is not tied to the thread that opened it.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Create* call and is closed only here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn acquire(app_id: &str) -> windows::core::Result<Launch> {
    // SAFETY: plain Win32 calls with owned names; every handle is closed by `Handle`.
    unsafe {
        let lock = Handle(CreateMutexW(
            None,
            false,
            &HSTRING::from(format!("Local\\{app_id}")),
        )?);
        let held_elsewhere = GetLastError() == ERROR_ALREADY_EXISTS;
        let show = Handle(CreateEventW(
            None,
            false,
            false,
            &HSTRING::from(format!("Local\\{app_id}.show")),
        )?);
        if held_elsewhere {
            SetEvent(show.0)?;
            return Ok(Launch::Second);
        }
        Ok(Launch::First(Instance { _lock: lock, show }))
    }
}

impl Instance {
    /// Calls `on_show`, on a thread of its own, each time a later launch asks for the window. The lock is held as long
    /// as that thread runs: the app's lifetime.
    pub fn on_show(self, on_show: impl Fn() + Send + 'static) {
        std::thread::spawn(move || {
            // The whole instance moves in (a closure would capture only the fields it reads, and drop the lock).
            let instance = self;
            // SAFETY: `instance` owns the event handle and lives as long as this thread.
            while unsafe { WaitForSingleObject(instance.show.0, INFINITE) } == WAIT_OBJECT_0 {
                on_show();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// The two halves of `second_launch_signals_first` run as this test binary again: `first` or `second`.
    const ROLE: &str = "TJOMMIE_SINGLE_INSTANCE_ROLE";
    const APP_ID: &str = "TJOMMIE_SINGLE_INSTANCE_APP_ID";
    const READY: &str = "TJOMMIE_SINGLE_INSTANCE_READY";

    fn child(role: &str, app_id: &str, ready: &Path) -> std::process::Child {
        crate::spawn::command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "shell::single_instance::tests::second_launch_signals_first",
                "--test-threads=1",
            ])
            .env(ROLE, role)
            .env(APP_ID, app_id)
            .env(READY, ready)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn second_launch_signals_first() {
        if let (Ok(role), Ok(app_id), Some(ready)) = (
            std::env::var(ROLE),
            std::env::var(APP_ID),
            std::env::var_os(READY),
        ) {
            match (role.as_str(), acquire(&app_id).unwrap()) {
                ("first", Launch::First(instance)) => {
                    let (shown, show) = std::sync::mpsc::channel();
                    instance.on_show(move || {
                        let _ = shown.send(());
                    });
                    std::fs::write(&ready, "holding").unwrap();
                    show.recv_timeout(Duration::from_secs(20)).unwrap();
                }
                ("second", Launch::Second) => {}
                (role, _) => panic!("the {role} launch got the wrong side of the lock"),
            }
            return;
        }
        // A name of its own, so a running Tjommie is never involved.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let app_id = format!("com.tsnyders.tjommie.test.{}.{stamp}", std::process::id());
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let mut first = child("first", &app_id, &ready);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "the first launch never took the lock"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        let second = child("second", &app_id, &ready).wait().unwrap();
        assert!(second.success(), "the second launch: {second}");
        let first = first.wait().unwrap();
        assert!(
            first.success(),
            "the first launch was never signalled: {first}"
        );
    }
}
