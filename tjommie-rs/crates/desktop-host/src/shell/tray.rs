//! The notification-area icon (T6.1; `main.ts:698-716`, `updater.ts` `trayTemplate`): a left click shows the window, the
//! right-click menu has Open, Restart to update (once an update is ready) and Quit.
//!
//! The icon lives on a hidden top-level window with its own thread and message loop, so it works while the UI thread is
//! busy, and the UI thread never waits for it: Explorer can take seconds to take an icon. The window procedure only turns
//! messages into typed `TrayEvent`s on a channel; it never calls into the UI and never panics (a panic across a window
//! procedure aborts the process).

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW,
    GetWindowLongPtrW, HICON, IDI_APPLICATION, LR_DEFAULTCOLOR, LoadIconW, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetWindowLongPtrW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, UnregisterClassW, WM_APP, WM_CLOSE, WM_DESTROY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::updater::{SHUTDOWN_FAILURE, UpdateState};

/// One line of the right-click menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayItem {
    Open,
    Separator,
    RestartToUpdate {
        version: String,
    },
    /// A greyed-out line that says what went wrong; never the raw error text, which can carry request headers.
    Info(&'static str),
    Quit,
}

impl TrayItem {
    /// The text shown, `None` for the separator.
    pub fn label(&self) -> Option<String> {
        match self {
            TrayItem::Open => Some("Open Tjommie".into()),
            TrayItem::Separator => None,
            TrayItem::RestartToUpdate { version } => {
                Some(format!("Restart to update to v{version}"))
            }
            TrayItem::Info(text) => Some((*text).into()),
            TrayItem::Quit => Some("Quit Tjommie".into()),
        }
    }
}

/// The menu for an update state (`trayTemplate`, `updater.ts:79-93`).
pub fn menu_items(state: &UpdateState) -> Vec<TrayItem> {
    let mut items = vec![TrayItem::Open, TrayItem::Separator];
    match state {
        UpdateState::Ready { version } => items.push(TrayItem::RestartToUpdate {
            version: version.clone(),
        }),
        UpdateState::Error { message } => {
            items.push(TrayItem::Info(if message == SHUTDOWN_FAILURE {
                "Couldn't install the update"
            } else {
                "Couldn't check for updates"
            }));
        }
        _ => {}
    }
    items.push(TrayItem::Quit);
    items
}

/// What the tray asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Show and focus the window (a left click, or Open).
    Show,
    /// Restart to install the ready update.
    Restart,
    Quit,
}

const ID_OPEN: usize = 1;
const ID_RESTART: usize = 2;
const ID_QUIT: usize = 3;
const ID_INFO: usize = 4;
const ICON_ID: u32 = 1;
/// The callback message the icon posts to the window; `lParam` is the mouse message.
const WM_TRAY: u32 = WM_APP + 1;
/// Posted by `Drop` and by nothing else: it is the only thing that ends the tray's window. A `WM_CLOSE` from outside is
/// ignored, since default handling would destroy the window and take the icon with it.
const WM_STOP: u32 = WM_APP + 2;

fn event_for_command(id: usize) -> Option<TrayEvent> {
    match id {
        ID_OPEN => Some(TrayEvent::Show),
        ID_RESTART => Some(TrayEvent::Restart),
        ID_QUIT => Some(TrayEvent::Quit),
        _ => None,
    }
}

/// The marker `hintTray` leaves in Electron's userData folder (`%APPDATA%\Tjommie` for a real install): the hint
/// shows the first time the window closes, ever, and a rollback to R1 doesn't repeat it.
const HINT_MARKER: &str = "tray-hint-shown";
pub const HINT_TITLE: &str = "Tjommie is still running";
pub const HINT_BODY: &str =
    "Your agents and routines keep working. Quit from the Tjommie icon in the taskbar tray.";

/// Closing the window only hides it. The first time, says that Tjommie keeps running in the tray. `show` shows the
/// toast and says whether it could. True if the hint was shown now. Unlike the TypeScript, which would throw out of
/// its close handler, a marker that can't be written means no hint: better silent than repeated on every close.
pub fn hint_once(user_data: &Path, show: impl FnOnce(&str, &str) -> bool) -> bool {
    let marker = user_data.join(HINT_MARKER);
    if marker.exists() {
        return false;
    }
    if std::fs::create_dir_all(user_data)
        .and_then(|()| std::fs::write(&marker, b""))
        .is_err()
    {
        return false;
    }
    show(HINT_TITLE, HINT_BODY)
}

pub struct TrayConfig {
    pub tooltip: String,
    /// A 32x32 PNG; `None` uses the system's default application icon.
    pub icon_png: Option<&'static [u8]>,
}

struct Shared {
    state: Mutex<UpdateState>,
    events: Sender<TrayEvent>,
    icon: isize,
    tooltip: String,
    icon_added: AtomicBool,
    /// The registered `TaskbarCreated` message: Explorer restarted, so the icon has to be added again.
    taskbar_created: AtomicU32,
    /// The tray's window, once its thread has made it and offered Windows the icon; 0 until then.
    hwnd: AtomicIsize,
    /// Asked to go. A window made after the ask had no `WM_STOP` posted to it, so its thread takes it down.
    stopping: AtomicBool,
}

pub struct Tray {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Tray {
    /// Starts the tray's thread, which makes its window, adds the icon and sends what the user picks to `events`. It
    /// returns without waiting for them, as Explorer can take seconds to take an icon (M6 re-review Nit 3): until it has,
    /// `icon_added` is false. A window that can't be made is logged, and leaves it false.
    pub fn start(config: TrayConfig, events: Sender<TrayEvent>) -> Result<Tray, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(UpdateState::Idle),
            events,
            icon: icon_from(config.icon_png).0 as isize,
            tooltip: config.tooltip,
            icon_added: AtomicBool::new(false),
            taskbar_created: AtomicU32::new(0),
            hwnd: AtomicIsize::new(0),
            stopping: AtomicBool::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || run(&thread_shared))
            .map_err(|error| format!("The tray thread could not start: {error}"))?;
        Ok(Tray {
            shared,
            thread: Some(thread),
        })
    }

    /// The menu is built from this when the user opens it.
    pub fn set_update_state(&self, state: UpdateState) {
        if let Ok(mut current) = self.shared.state.lock() {
            *current = state;
        }
    }

    /// Whether Windows took the icon (it doesn't when there is no shell, such as in some services).
    pub fn icon_added(&self) -> bool {
        self.shared.icon_added.load(Ordering::SeqCst)
    }

    /// The tray's window, so tests can post it the icon's messages; 0 until its thread has made it.
    pub fn window(&self) -> isize {
        self.shared.hwnd.load(Ordering::SeqCst)
    }

    /// Takes the icon away now, without waiting for the drop: an app that is quitting must not leave its icon in the
    /// notification area until the pointer passes over it. The window goes with it (on its own thread, a moment later).
    pub fn remove(&self) {
        stop(&self.shared);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        stop(&self.shared);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Asks the tray's window to go: posted to it once it is made, else taken up by its thread when it makes it. One of the
/// two always sees the other's write, so the ask is never lost.
fn stop(shared: &Shared) {
    shared.stopping.store(true, Ordering::SeqCst);
    let hwnd = shared.hwnd.load(Ordering::SeqCst);
    if hwnd != 0 {
        // SAFETY: a window handle of ours; if it is already gone, the post just fails.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd as *mut c_void)),
                WM_STOP,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn icon_from(png: Option<&[u8]>) -> HICON {
    // SAFETY: plain Win32 calls with valid arguments; a failure falls back to the stock icon.
    unsafe {
        if let Some(png) = png
            && let Ok(icon) =
                CreateIconFromResourceEx(png, true, 0x0003_0000, 32, 32, LR_DEFAULTCOLOR)
        {
            return icon;
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

fn add_icon(hwnd: HWND, shared: &Shared) {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: HICON(shared.icon as *mut c_void),
        ..Default::default()
    };
    // The tip holds 127 characters and a terminator.
    for (slot, unit) in data
        .szTip
        .iter_mut()
        .zip(wide(&shared.tooltip).into_iter().take(127))
    {
        *slot = unit;
    }
    // SAFETY: `data` is a fully initialised NOTIFYICONDATAW for our own window.
    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() };
    shared.icon_added.store(added, Ordering::SeqCst);
    if !added {
        log::warn!("Windows didn't take the tray icon; Tjommie keeps running without it.");
    }
}

fn remove_icon(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    };
    // SAFETY: removes only the icon this window added.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

static CLASS_COUNTER: AtomicU32 = AtomicU32::new(0);

fn run(shared: &Arc<Shared>) {
    let class_name = wide(&format!(
        "TjommieTray-{}-{}",
        std::process::id(),
        CLASS_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    // SAFETY: Win32 window creation on this thread; the window and class are destroyed before it ends, and the
    // `Shared` the window procedure reads stays alive (this thread holds an `Arc`) until the window is gone.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            log::warn!("No tray icon: the tray window class could not be registered.");
            return;
        }
        // A hidden top-level window, never shown. Not message-only: `SetForegroundWindow` (which makes the context menu
        // close when the user clicks elsewhere) can't activate a message-only window, and message-only windows don't get
        // broadcasts like `TaskbarCreated`. A tool window stays out of the taskbar and Alt+Tab. Chromium's notify-icon host
        // and the `tray-icon` crate do the same.
        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            w!("Tjommie tray"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance));
                log::warn!("No tray icon: the tray window could not be created: {error}");
                return;
            }
        };
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::as_ptr(shared) as isize);
        shared.taskbar_created.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::SeqCst,
        );
        if !shared.stopping.load(Ordering::SeqCst) {
            add_icon(hwnd, shared);
        }
        shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
        // Asked to go before the window was known: no `WM_STOP` was posted, so it goes now.
        if shared.stopping.load(Ordering::SeqCst) {
            let _ = DestroyWindow(hwnd);
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance));
    }
}

/// A panic must not cross the window procedure, so everything runs inside `catch_unwind`.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| handle(hwnd, message, lparam)));
    match handled {
        Ok(Some(result)) => result,
        // SAFETY: the default procedure for the message we were given.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn handle(hwnd: HWND, message: u32, lparam: LPARAM) -> Option<LRESULT> {
    // SAFETY: set in `run` to an `Arc<Shared>` that outlives the window; null before that.
    let shared = unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
        pointer.as_ref()?
    };
    if message == WM_TRAY {
        match lparam.0 as u32 & 0xFFFF {
            WM_LBUTTONUP => {
                let _ = shared.events.send(TrayEvent::Show);
            }
            WM_RBUTTONUP => open_menu(hwnd, shared),
            _ => {}
        }
        return Some(LRESULT(0));
    }
    if message == WM_STOP {
        // SAFETY: destroys this thread's own window, from this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        return Some(LRESULT(0));
    }
    if message == WM_CLOSE {
        // Nobody of ours asks this way: a stray close must not take the icon away.
        return Some(LRESULT(0));
    }
    if message == WM_DESTROY {
        remove_icon(hwnd);
        // The icon is gone, so `icon_added` stops saying the window can be brought back from the tray.
        shared.icon_added.store(false, Ordering::SeqCst);
        // SAFETY: ends this thread's message loop.
        unsafe { PostQuitMessage(0) };
        return Some(LRESULT(0));
    }
    let taskbar_created = shared.taskbar_created.load(Ordering::SeqCst);
    if taskbar_created != 0 && message == taskbar_created {
        add_icon(hwnd, shared);
        return Some(LRESULT(0));
    }
    None
}

fn open_menu(hwnd: HWND, shared: &Shared) {
    let state = shared
        .state
        .lock()
        .map(|state| state.clone())
        .unwrap_or(UpdateState::Unsupported);
    // SAFETY: a popup menu made and destroyed here; the label buffers outlive each AppendMenuW call.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for item in menu_items(&state) {
            let (flags, id) = match &item {
                TrayItem::Open => (MF_STRING, ID_OPEN),
                TrayItem::Separator => (MF_SEPARATOR, 0),
                TrayItem::RestartToUpdate { .. } => (MF_STRING, ID_RESTART),
                TrayItem::Info(_) => (MF_STRING | MF_GRAYED, ID_INFO),
                TrayItem::Quit => (MF_STRING, ID_QUIT),
            };
            let label = wide(&item.label().unwrap_or_default());
            let _ = AppendMenuW(menu, flags, id, PCWSTR(label.as_ptr()));
        }
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Without this the menu doesn't close when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if let Some(event) = event_for_command(command.0 as usize) {
            let _ = shared.events.send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use super::*;

    fn labels(state: &UpdateState) -> Vec<String> {
        menu_items(state)
            .iter()
            .map(|item| item.label().unwrap_or_else(|| "separator".into()))
            .collect()
    }

    fn config() -> TrayConfig {
        TrayConfig {
            tooltip: "Tjommie RS test".into(),
            icon_png: None,
        }
    }

    /// A tray for a test, once its thread has made its window and offered the icon (`start` doesn't wait for them).
    fn started(events: Sender<TrayEvent>) -> Tray {
        let tray = Tray::start(config(), events).unwrap();
        let begun = std::time::Instant::now();
        while tray.window() == 0 {
            assert!(
                begun.elapsed() < Duration::from_secs(5),
                "the tray's window was never made"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        tray
    }

    // `updater.test.ts` "shows restart in the tray only after an update is ready".
    #[test]
    fn menu_items_follow_update_state() {
        assert_eq!(
            labels(&UpdateState::Idle),
            ["Open Tjommie", "separator", "Quit Tjommie"]
        );
        assert_eq!(
            labels(&UpdateState::Ready {
                version: "0.3.1".into()
            }),
            [
                "Open Tjommie",
                "separator",
                "Restart to update to v0.3.1",
                "Quit Tjommie"
            ]
        );
        assert_eq!(
            labels(&UpdateState::Error {
                message: SHUTDOWN_FAILURE.into()
            }),
            [
                "Open Tjommie",
                "separator",
                "Couldn't install the update",
                "Quit Tjommie"
            ]
        );
        // The raw error text, which can carry request headers, is never shown.
        let http = UpdateState::Error {
            message: "Cannot download update\nHeaders: { authorization: secret }".into(),
        };
        assert_eq!(labels(&http)[2], "Couldn't check for updates");
        assert!(matches!(menu_items(&http)[2], TrayItem::Info(_)));
        for state in [
            UpdateState::Checking,
            UpdateState::None,
            UpdateState::Downloading { percent: 4.0 },
            UpdateState::Unsupported,
        ] {
            assert_eq!(labels(&state).len(), 3);
        }
    }

    #[test]
    fn the_still_running_hint_shows_once_ever() {
        let user_data = tempfile::tempdir().unwrap();
        let mut shown = Vec::new();
        let mut show = |title: &str, body: &str| {
            shown.push((title.to_string(), body.to_string()));
            true
        };
        assert!(hint_once(user_data.path(), &mut show));
        assert!(!hint_once(user_data.path(), &mut show));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].0, "Tjommie is still running");
        assert!(user_data.path().join("tray-hint-shown").exists());
        // A folder that can't be made (a file is in the way) means no hint, not a hint on every close.
        let blocked = user_data.path().join("blocked");
        std::fs::write(&blocked, b"a file").unwrap();
        assert!(!hint_once(&blocked.join("sub"), |_, _| panic!(
            "must not show"
        )));
        // A refused toast still counts as shown once: the marker was written first, as in `hintTray`.
        let refused = tempfile::tempdir().unwrap();
        assert!(!hint_once(refused.path(), |_, _| false));
        assert!(refused.path().join("tray-hint-shown").exists());
    }

    #[test]
    fn a_picked_menu_command_becomes_an_event() {
        assert_eq!(event_for_command(ID_OPEN), Some(TrayEvent::Show));
        assert_eq!(event_for_command(ID_RESTART), Some(TrayEvent::Restart));
        assert_eq!(event_for_command(ID_QUIT), Some(TrayEvent::Quit));
        // The greyed-out line, a dismissed menu (0) and anything else do nothing.
        assert_eq!(event_for_command(ID_INFO), None);
        assert_eq!(event_for_command(0), None);
    }

    // Review finding 2. The right-click menu is owned by the tray's window, and `SetForegroundWindow` (which is what makes the
    // menu close when the user clicks elsewhere) can only activate a top-level window. A message-only window is a child of
    // the hidden Message window, never top-level, and also never receives broadcasts such as `TaskbarCreated` (which is how
    // the icon comes back after Explorer restarts). So the window is a hidden top-level tool window: never shown, absent
    // from the taskbar and Alt+Tab. That a click outside really closes the menu needs real input: the owner's check.
    #[test]
    fn the_tray_window_is_a_hidden_top_level_tool_window() {
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GA_PARENT, GWL_EXSTYLE, GetAncestor, GetDesktopWindow, GetWindowLongPtrW,
            IsWindowVisible, WS_EX_TOOLWINDOW,
        };
        use windows::core::BOOL;
        unsafe extern "system" fn collect(window: HWND, found: LPARAM) -> BOOL {
            // SAFETY: `found` is the address of the Vec the test passes below, alive for the whole EnumWindows call.
            let windows = unsafe { &mut *(found.0 as *mut Vec<isize>) };
            windows.push(window.0 as isize);
            BOOL(1)
        }
        let (events, _received) = channel();
        let tray = started(events);
        let window = HWND(tray.window() as *mut c_void);
        // SAFETY: queries about the window this test owns, and one enumeration into a Vec that outlives it.
        unsafe {
            // A child of the desktop (top-level), not of the Message window; never shown.
            assert_eq!(GetAncestor(window, GA_PARENT), GetDesktopWindow());
            assert!(!IsWindowVisible(window).as_bool());
            assert_ne!(
                GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0,
                0
            );
            // Top-level windows, visible or not, are what broadcasts reach; message-only windows are not among them.
            let mut top_level: Vec<isize> = Vec::new();
            EnumWindows(Some(collect), LPARAM(&raw mut top_level as isize)).unwrap();
            assert!(
                top_level.contains(&tray.window()),
                "the tray window must be enumerable as top-level"
            );
        }
    }

    /// Re-review Nit 1: a WM_CLOSE from outside (nothing of ours posts one) must not destroy the tray's window, which would
    /// take the icon with it while the app still hides on close for want of a way back.
    #[test]
    fn a_close_from_outside_does_not_take_the_tray_away() {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        let (events, _received) = channel();
        let tray = started(events);
        let window = HWND(tray.window() as *mut c_void);
        // SAFETY: posts to, and asks about, the window this test owns.
        unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        // The message has been handled once a later one posted behind it has.
        unsafe { PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0)) }.unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            // SAFETY: a query about the window this test owns.
            unsafe { IsWindow(Some(window)).as_bool() },
            "a foreign WM_CLOSE destroyed the tray window"
        );
        drop(tray);
        assert!(
            // SAFETY: a query about a window handle, which is safe to ask of one that is gone.
            !unsafe { IsWindow(Some(window)).as_bool() },
            "dropping the tray left its window"
        );
    }

    /// Re-review Nit 7: the icon of an app that is quitting is removed when it asks, not whenever the process is torn down.
    /// M6 re-review Nit 3: `start` doesn't wait for the window, so the ask can come before the window is made; it goes
    /// once it is.
    #[test]
    fn removing_the_tray_takes_its_icon_and_window_away_at_once() {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        let (events, _received) = channel();
        let tray = Tray::start(config(), events).unwrap();
        tray.remove();
        // The window is made either way, and taken down.
        let gone = || {
            let window = tray.window();
            // SAFETY: a query about a window handle, which is safe to ask of one that is gone.
            window != 0 && !unsafe { IsWindow(Some(HWND(window as *mut c_void))).as_bool() }
        };
        let begun = std::time::Instant::now();
        while !gone() {
            assert!(
                begun.elapsed() < Duration::from_secs(5),
                "the tray window was still there after remove()"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!tray.icon_added(), "the tray still says it has an icon");
        drop(tray);
    }

    // Posts the icon's own callback messages to the real window. A click on the actual icon is the owner's check
    // (phase3-checklist.md, M6).
    #[test]
    fn a_left_click_on_the_icon_shows_the_window() {
        let (events, received) = channel();
        let tray = started(events);
        // A machine with no desktop shell (a CI runner, a service) can't take the icon; the routing below doesn't need it.
        if std::env::var_os("CI").is_none() {
            assert!(
                tray.icon_added(),
                "Windows should take a notification-area icon"
            );
        }
        tray.set_update_state(UpdateState::Ready {
            version: "9.9.9".into(),
        });
        let post = |mouse: u32| {
            // SAFETY: posts to the window this test owns.
            unsafe {
                PostMessageW(
                    Some(HWND(tray.window() as *mut c_void)),
                    WM_TRAY,
                    WPARAM(ICON_ID as usize),
                    LPARAM(mouse as isize),
                )
            }
            .unwrap();
        };
        post(WM_LBUTTONUP);
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            TrayEvent::Show
        );
        // Other mouse messages (a move, a press without its release) do nothing.
        post(0x0200);
        post(0x0201);
        post(WM_LBUTTONUP);
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            TrayEvent::Show
        );
        assert!(received.try_recv().is_err());
        drop(tray);
        // The thread has stopped and dropped its sender: nothing more will arrive.
        assert!(received.recv_timeout(Duration::from_millis(200)).is_err());
    }
}
