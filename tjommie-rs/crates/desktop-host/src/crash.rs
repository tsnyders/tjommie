//! Crash handling (gate condition 4). A panic on the UI thread that nothing catches is a crash: the hook appends it to
//! `<root>/logs/desktop-crash.log` with the thread, a backtrace (line tables are kept in release), a timestamp and the
//! version, then aborts. The abort is a fail-fast, so Windows Error Reporting sees a crash and restarts the app with
//! `--hidden --after-crash` once it has run 60 s; the daemon, the app's child, exits with it and restarts with it.
//! A panic inside `catching` (the UI guard) is logged as caught and unwinds to its caller instead, and a panic on any
//! other thread is logged and unwinds that thread.
//!
//! Each entry also says which build wrote it, so the log can be read again later, on another PC, against the release's PDBs
//! (`Tjommie-<version>-symbols.zip`): a `modules:` block with each exe's file name, where the running one loaded (`base`),
//! its size, its PE timestamp and its PDB's name, GUID and age (the build id, from `build_id`); and a `stack:` line with the
//! return addresses of the panic's thread, written `<exe>+0x<offset from base>` for the frames inside the running exe and
//! as plain addresses for the rest. The offsets and the build id are all a symboliser needs (`docs/DEVELOPMENT.md`).

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Recovery::{
    RESTART_NO_PATCH, RESTART_NO_REBOOT, RegisterApplicationRestart,
};
use windows::core::w;

use crate::build_id::{self, BuildId};

/// How many return addresses an entry keeps; the deepest frames of a UI thread are the OS's.
const STACK_FRAMES: usize = 48;

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

/// One exe an entry names.
struct Module {
    /// The file name, never the folder.
    name: String,
    /// Where the running exe loaded; `None` for an exe the app runs as another process, whose base only it knows.
    base: Option<usize>,
    /// `None` when the file is no PE or can't be read.
    id: Option<BuildId>,
}

static MODULES: Mutex<Vec<Module>> = Mutex::new(Vec::new());

/// Names an exe the app runs as another process (the daemon) in every entry, by its build id, so a failure of that process
/// can be matched to its PDB too. Its load base is unknown here and left out. Reads the file's headers, so call it off a hot
/// path, once per exe.
pub fn note_exe(path: &Path) {
    record(path, None);
}

fn record(path: &Path, base: Option<usize>) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let module = Module {
        id: build_id::read(path),
        name,
        base,
    };
    let mut modules = MODULES.lock().unwrap_or_else(PoisonError::into_inner);
    modules.retain(|known| known.name != module.name);
    modules.push(module);
}

/// The running exe, with the address Windows loaded it at.
fn record_running_exe() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // SAFETY: `None` asks for the module of the file that started this process, which stays loaded for its whole life.
    let base = unsafe { GetModuleHandleW(None) }
        .ok()
        .map(|module| module.0.addr());
    record(&exe, base);
}

/// The `modules:` block, one line per exe.
fn modules_block(modules: &[Module]) -> String {
    let mut block = String::from("modules:\n");
    for module in modules {
        block.push_str(&format!("  {}", module.name));
        match module.base {
            Some(base) => block.push_str(&format!(" base=0x{base:016x}")),
            None => block.push_str(" (another process)"),
        }
        match &module.id {
            Some(id) => block.push_str(&format!(" {}\n", id.describe())),
            None => block.push_str(" (no build id)\n"),
        }
    }
    block
}

/// The `stack:` line: the return addresses of this thread, the ones inside a module with a known base as `name+0xoffset`.
fn stack_line(modules: &[Module]) -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_FRAMES];
    // SAFETY: `frames` has the `STACK_FRAMES` slots the call may fill, and it returns how many it filled.
    let count = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) };
    let named: Vec<String> = frames
        .iter()
        .take(usize::from(count))
        .map(|frame| {
            let address = frame.addr();
            let inside = modules.iter().find_map(|module| {
                let base = module.base?;
                let size = usize::try_from(module.id.as_ref()?.size_of_image).ok()?;
                (address >= base && address - base < size).then(|| (&module.name, address - base))
            });
            match inside {
                Some((name, offset)) => format!("{name}+0x{offset:x}"),
                None => format!("0x{address:x}"),
            }
        })
        .collect();
    format!("stack: {}\n", named.join(" "))
}

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
    record_running_exe();
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
    let modules = MODULES.lock().unwrap_or_else(PoisonError::into_inner);
    format!(
        "{} Tjommie {} {kind} in thread '{}': {info}\n{skipped}{}{}{}\n",
        jiff::Timestamp::now(),
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("unnamed"),
        modules_block(&modules),
        stack_line(&modules),
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
        names_the_build(&log);
    }

    /// The crash entry's `modules:` block and `stack:` line carry what a later symbolisation needs, and they are right:
    /// DbgHelp, given only the logged base, the exe and the PDB beside it, names this test's own frame from the logged offsets.
    fn names_the_build(log: &str) {
        let exe = std::env::current_exe().unwrap();
        let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
        // Everything from the crash entry on (the crash is the last entry).
        let crash = log
            .split_once(" crash in thread '")
            .expect("a crash entry")
            .1;
        let id = build_id::read(&exe).expect("this test binary has a build id");
        // The line of the running exe: its base, then the build id exactly as the file's headers give it.
        let line = crash
            .lines()
            .find(|line| line.starts_with(&format!("  {exe_name} base=0x")))
            .unwrap_or_else(|| panic!("no module line for {exe_name}: {crash}"));
        let base = usize::from_str_radix(
            line.split(" base=0x")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap(),
            16,
        )
        .unwrap();
        assert!(
            line.ends_with(&id.describe()),
            "{line} vs {}",
            id.describe()
        );
        // Only file names: no folder of this machine, the PDB's included.
        let pdb = id.pdb.as_ref().expect("a PDB");
        assert!(
            !line.contains(['\\', '/']) && line.contains(&pdb.guid),
            "{line}"
        );
        // Every entry of the log has them, the caught ones too.
        assert_eq!(log.matches("modules:\n").count(), 3, "{log}");
        assert_eq!(log.matches("\nstack: ").count(), 3, "{log}");

        let stack = crash
            .lines()
            .find_map(|line| line.strip_prefix("stack: "))
            .expect("a stack line");
        let offsets: Vec<usize> = stack
            .split(' ')
            .filter_map(|frame| frame.strip_prefix(&format!("{exe_name}+0x")))
            .filter_map(|offset| usize::from_str_radix(offset, 16).ok())
            .collect();
        assert!(offsets.len() >= 3, "frames inside the exe: {stack}");
        assert!(
            offsets
                .iter()
                .all(|offset| *offset < id.size_of_image as usize),
            "{stack}"
        );
        // A return address points after its call, so the call itself is one byte before.
        let names = symbolise(
            &exe,
            base,
            id.size_of_image,
            offsets.iter().map(|o| base + o - 1),
        );
        assert!(
            names.iter().any(|name| name.contains("hook_writes_log")),
            "no frame of this test in {names:?} (from {stack})"
        );
    }

    /// The names DbgHelp gives `addresses` in `exe` loaded at `base`, found through the PDB in the exe's folder. A handle
    /// of its own keeps this away from the one the standard library's backtraces use.
    fn symbolise(
        exe: &Path,
        base: usize,
        size: u32,
        addresses: impl Iterator<Item = usize>,
    ) -> Vec<String> {
        use std::os::windows::ffi::OsStrExt as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Diagnostics::Debug::{
            SLMFLAG_NONE, SYMBOL_INFOW, SymCleanup, SymFromAddrW, SymInitializeW, SymLoadModuleExW,
        };
        use windows::core::PCWSTR;

        const NAME_CHARS: usize = 512;
        let wide =
            |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
        let process = HANDLE(0x5eed_usize as *mut c_void);
        let (exe_wide, search) = (wide(exe), wide(exe.parent().unwrap()));
        let mut names = Vec::new();
        // SAFETY: the wide strings end in NUL and outlive the calls; the symbol buffer holds a `SYMBOL_INFOW` and
        // `NAME_CHARS` name characters, 8-byte aligned, as `SizeOfStruct` and `MaxNameLen` say; the session is cleaned up.
        unsafe {
            SymInitializeW(process, PCWSTR(search.as_ptr()), false).unwrap();
            let loaded = SymLoadModuleExW(
                process,
                None,
                PCWSTR(exe_wide.as_ptr()),
                PCWSTR::null(),
                base as u64,
                size,
                None,
                Some(SLMFLAG_NONE),
            );
            assert_ne!(loaded, 0, "DbgHelp could not load {}", exe.display());
            for address in addresses {
                let mut buffer =
                    vec![0u64; (size_of::<SYMBOL_INFOW>() + NAME_CHARS * 2).div_ceil(8)];
                let info = buffer.as_mut_ptr().cast::<SYMBOL_INFOW>();
                (*info).SizeOfStruct = size_of::<SYMBOL_INFOW>() as u32;
                (*info).MaxNameLen = NAME_CHARS as u32;
                if SymFromAddrW(process, address as u64, None, info).is_ok() {
                    let name =
                        std::slice::from_raw_parts((*info).Name.as_ptr(), (*info).NameLen as usize);
                    names.push(String::from_utf16_lossy(name));
                }
            }
            SymCleanup(process).unwrap();
        }
        names
    }
}
