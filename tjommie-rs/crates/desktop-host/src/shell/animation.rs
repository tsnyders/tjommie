//! Windows' Animation effects (Settings, Accessibility, Visual effects), which the app's reduced motion follows: GPUI
//! doesn't expose the setting (spike limit 18). It is read at start and again on every WM_SETTINGCHANGE, which Windows
//! broadcasts to every top-level window when a setting changes; message-only windows don't get broadcasts, so the
//! watcher is a hidden top-level window on a thread of its own. Read only: nothing here changes the setting.

use std::cell::RefCell;
use std::io;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, SPI_GETCLIENTAREAANIMATION,
    SPI_SETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW,
    SystemParametersInfoW, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_SETTINGCHANGE, WNDCLASSW,
    WS_OVERLAPPED,
};
use windows::core::{BOOL, w};

/// Whether Windows animates: Animation effects is on. A failed read counts as on.
pub fn animations_on() -> bool {
    let mut on = BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the pointer.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || on.as_bool()
}

thread_local! {
    /// The watcher thread's check, run on each WM_SETTINGCHANGE.
    static ON_SETTING_CHANGE: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// The hidden window that hears setting changes; dropping it closes the window and ends its thread.
pub struct Watcher {
    /// The window, as an integer: a window handle may cross threads, the type may not.
    window: isize,
    thread: Option<JoinHandle<()>>,
}

/// Calls `changed` with `read()`'s new value whenever it differs after a WM_SETTINGCHANGE. `read` is `animations_on`,
/// or a test's stand-in. `changed` runs on the watcher's thread.
pub fn watch(read: fn() -> bool, changed: impl Fn(bool) + Send + 'static) -> io::Result<Watcher> {
    let (ready, opened) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("animation-setting".into())
        .spawn(move || {
            let mut last = read();
            ON_SETTING_CHANGE.set(Some(Box::new(move || {
                let now = read();
                if now != last {
                    last = now;
                    changed(now);
                }
            })));
            let window = open_window();
            let opened = window.is_ok();
            let _ = ready.send(window.map(|window| window.0 as isize));
            if opened {
                let mut message = MSG::default();
                // SAFETY: a standard message loop on the thread that owns the window.
                while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
                    unsafe { DispatchMessageW(&message) };
                }
            }
        })?;
    let window = opened.recv().map_err(io::Error::other)??;
    Ok(Watcher {
        window,
        thread: Some(thread),
    })
}

impl Watcher {
    /// What Windows does when the setting changes: a WM_SETTINGCHANGE naming Animation effects, answered before this
    /// returns. For tests, with a stand-in `read`.
    pub fn announce(&self) {
        // SAFETY: the window lives until this watcher drops; SendMessageW waits for its thread to answer.
        unsafe {
            SendMessageW(
                self.hwnd(),
                WM_SETTINGCHANGE,
                Some(WPARAM(SPI_SETCLIENTAREAANIMATION.0 as usize)),
                Some(LPARAM(0)),
            )
        };
    }

    fn hwnd(&self) -> HWND {
        HWND(self.window as *mut _)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: as above; WM_CLOSE destroys the window, which ends the loop.
        if unsafe { PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

fn open_window() -> windows::core::Result<HWND> {
    static CLASS: OnceLock<bool> = OnceLock::new();
    // SAFETY: the module handle of this exe, and a class whose procedure is below.
    let instance = unsafe { GetModuleHandleW(None) }?;
    let class = w!("TjommieAnimationSetting");
    CLASS.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // SAFETY: the class lives as long as the process.
        unsafe { RegisterClassW(&class) != 0 }
    });
    // SAFETY: a hidden top-level window of that class: no parent, never shown.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Tjommie animation setting"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SETTINGCHANGE => {
            // A panic must not unwind out of a window procedure; it is logged as caught instead.
            let _ = crate::crash::catching(|| {
                ON_SETTING_CHANGE.with_borrow_mut(|check| {
                    if let Some(check) = check {
                        check();
                    }
                })
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: the window belongs to this thread.
            let _ = unsafe { DestroyWindow(window) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: everything else gets Windows' default handling.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// The test's stand-in for the setting: the real one is never changed.
    static ANIMATIONS: AtomicBool = AtomicBool::new(true);

    fn stand_in() -> bool {
        ANIMATIONS.load(Ordering::SeqCst)
    }

    #[test]
    fn hears_each_change_of_the_setting() {
        ANIMATIONS.store(true, Ordering::SeqCst);
        let (told, heard) = mpsc::channel();
        let watcher = watch(stand_in, move |on| {
            let _ = told.send(on);
        })
        .unwrap();
        let wait = || heard.recv_timeout(Duration::from_secs(5));

        ANIMATIONS.store(false, Ordering::SeqCst);
        watcher.announce();
        assert_eq!(wait(), Ok(false), "turned off");
        watcher.announce();
        assert!(heard.try_recv().is_err(), "unchanged: nothing to tell");
        ANIMATIONS.store(true, Ordering::SeqCst);
        watcher.announce();
        assert_eq!(wait(), Ok(true), "turned on again");
        drop(watcher);
    }

    #[test]
    fn reads_the_real_setting() {
        // The value is the owner's; reading it must simply work.
        let _ = animations_on();
    }
}
