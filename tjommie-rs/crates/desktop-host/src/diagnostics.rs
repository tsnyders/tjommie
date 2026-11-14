//! Diagnostics (T6.6; port of `diagnostics.ts`): what the app knows about itself, and saving a report to a file the
//! person chooses. `diagnostics.get` (the daemon's side) and the connection state are put together into a
//! `DiagnosticReport` by the screens; this module adds the desktop's own facts, checks the report against the allowlist,
//! and writes it so that a failed save never leaves a half-written file or loses the file that was there.
//!
//! The report is a fixed set of fields with no logs, paths or names (`protocol::diagnostics`). The crash log is a
//! separate, plain-text report the person chooses to save from the "closed unexpectedly" notice.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use protocol::diagnostics::{ReportDevice, parse_diagnostic_report};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;

use crate::identity::{Identity, Kind};
use crate::shell::registry::{Hive, Registry};

/// The file name the save dialog starts with.
pub const REPORT_FILE_NAME: &str = "tjommie-diagnostics.json";
pub const CRASH_REPORT_FILE_NAME: &str = "tjommie-crash-report.txt";
/// How much of the end of `desktop-crash.log` a crash report carries.
pub const CRASH_LOG_TAIL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInfo {
    pub working_set_bytes: u64,
    pub private_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub app_version: String,
    pub app_id: String,
    pub product: String,
    pub identity: &'static str,
    pub platform: &'static str,
    pub gpu_adapter: Option<String>,
    pub webview2_runtime: Option<String>,
    pub memory: Option<MemoryInfo>,
}

/// `DiagnosticVersion`: `major.minor.patch`, each 1 to 4 digits, optionally `-alpha`, `-beta` or `-rc` and a number.
pub fn diagnostic_version_ok(version: &str) -> bool {
    let (numbers, tag) = match version.split_once('-') {
        Some((numbers, tag)) => (numbers, Some(tag)),
        None => (version, None),
    };
    let digits =
        |part: &str| (1..=4).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit());
    let core = numbers.split('.').collect::<Vec<_>>();
    let tag_ok = tag.is_none_or(|tag| {
        let (name, number) = match tag.split_once('.') {
            Some((name, number)) => (name, Some(number)),
            None => (tag, None),
        };
        matches!(name, "alpha" | "beta" | "rc") && number.is_none_or(digits)
    });
    core.len() == 3 && core.iter().all(|part| digits(part)) && tag_ok
}

const WEBVIEW2_CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// The Evergreen WebView2 runtime's version, from where its installer records it (machine-wide first, then per user).
pub fn webview2_runtime(registry: &dyn Registry) -> Option<String> {
    let machine = format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT}");
    let user = format!(r"Software\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT}");
    registry
        .read_string(Hive::LocalMachine, &machine, "pv")
        .or_else(|| registry.read_string(Hive::CurrentUser, &user, "pv"))
        .filter(|version| {
            !version.is_empty()
                && version.len() <= 32
                && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        })
}

fn gpu_adapter() -> Option<String> {
    // SAFETY: plain DXGI calls; the factory and adapter are released when they go out of scope.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let adapter = factory.EnumAdapters1(0).ok()?;
        let description = adapter.GetDesc1().ok()?;
        let units = &description.Description;
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        let name = String::from_utf16_lossy(units.get(..end)?);
        (!name.is_empty()).then_some(name)
    }
}

fn memory() -> Option<MemoryInfo> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the structure is sized and owned here; EX is a superset of the plain counters the API is typed for.
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::from_mut(&mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
        .ok()?;
    }
    Some(MemoryInfo {
        working_set_bytes: counters.WorkingSetSize as u64,
        private_bytes: counters.PrivateUsage as u64,
    })
}

fn identity_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Production => "production",
        Kind::UpdateTest => "update-test",
        Kind::Dev => "dev",
        Kind::E2e => "e2e",
    }
}

/// App version, identity, GPU adapter, WebView2 runtime version and a memory snapshot.
pub fn app_info(identity: &Identity, app_version: &str, registry: &dyn Registry) -> AppInfo {
    AppInfo {
        app_version: app_version.into(),
        app_id: identity.app_id.into(),
        product: identity.product.into(),
        identity: identity_name(identity.kind),
        platform: "windows",
        gpu_adapter: gpu_adapter(),
        webview2_runtime: webview2_runtime(registry),
        memory: memory(),
    }
}

impl AppInfo {
    /// The report's `device` section (`info()` in `diagnostics.ts`). The version must be a diagnostic version.
    pub fn device(&self) -> Result<ReportDevice, String> {
        if !diagnostic_version_ok(&self.app_version) {
            return Err("App version is unavailable.".into());
        }
        Ok(ReportDevice {
            platform: self.platform.into(),
            app_version: Some(self.app_version.clone()),
            file_bridge: "present".into(),
        })
    }
}

/// The plain-text crash report: the app's facts, then the end of `desktop-crash.log`.
pub fn crash_report_text(info: &AppInfo, root: &Path) -> String {
    let mut text = format!(
        "Tjommie crash report\nVersion: {}\nBuild: {} ({})\nPlatform: {}\nGPU: {}\nWebView2 runtime: {}\n",
        info.app_version,
        info.product,
        info.identity,
        info.platform,
        info.gpu_adapter.as_deref().unwrap_or("unknown"),
        info.webview2_runtime.as_deref().unwrap_or("not installed"),
    );
    if let Some(memory) = &info.memory {
        text.push_str(&format!(
            "Memory: {} MB working set, {} MB private\n",
            memory.working_set_bytes / 1_048_576,
            memory.private_bytes / 1_048_576
        ));
    }
    text.push_str("\n--- desktop-crash.log ---\n");
    match std::fs::read(root.join("logs").join("desktop-crash.log")) {
        Ok(bytes) if !bytes.is_empty() => {
            let start = bytes.len().saturating_sub(CRASH_LOG_TAIL_BYTES);
            let tail = String::from_utf8_lossy(bytes.get(start..).unwrap_or_default()).into_owned();
            // A tail that starts mid-line starts at the next line instead.
            let tail = if start > 0 {
                tail.split_once('\n')
                    .map_or(tail.as_str(), |(_, rest)| rest)
                    .to_string()
            } else {
                tail
            };
            text.push_str(&tail);
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
        _ => text.push_str("(no crash log)\n"),
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    Cancelled,
}

/// One report save at a time: the second would open a second dialog over the first.
#[derive(Default)]
pub struct ReportSaver {
    saving: AtomicBool,
}

struct Saving<'a>(&'a AtomicBool);

impl Drop for Saving<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

const NOT_SAVED: &str = "The diagnostic report could not be saved.";

impl ReportSaver {
    /// Checks `report_json` against the allowlist **before** any dialog opens, asks where to save it with `choose`, and
    /// writes the checked, normalized report there. `cancelled` says the screen that asked has gone, which cancels a save
    /// not yet finished. A picker answering `None` is a cancel too.
    pub async fn save_report<C, F>(
        &self,
        report_json: &str,
        choose: C,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<SaveOutcome, String>
    where
        C: FnOnce() -> F,
        F: Future<Output = Option<PathBuf>>,
    {
        let text = parse_diagnostic_report(report_json).map_err(|invalid| invalid.0)?;
        self.save(text.into_bytes(), choose, cancelled).await
    }

    /// Saves any text the same way: a crash report, say.
    pub async fn save_text<C, F>(
        &self,
        text: String,
        choose: C,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<SaveOutcome, String>
    where
        C: FnOnce() -> F,
        F: Future<Output = Option<PathBuf>>,
    {
        self.save(text.into_bytes(), choose, cancelled).await
    }

    async fn save<C, F>(
        &self,
        bytes: Vec<u8>,
        choose: C,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<SaveOutcome, String>
    where
        C: FnOnce() -> F,
        F: Future<Output = Option<PathBuf>>,
    {
        if self
            .saving
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("A diagnostic save is already open.".into());
        }
        let _saving = Saving(&self.saving);
        let Some(destination) = choose().await else {
            return Ok(SaveOutcome::Cancelled);
        };
        if cancelled() {
            return Ok(SaveOutcome::Cancelled);
        }
        let staging = tokio::task::spawn_blocking({
            let destination = destination.clone();
            move || stage(&destination, &bytes)
        })
        .await
        .map_err(|_| NOT_SAVED.to_string())?
        .map_err(|_| NOT_SAVED.to_string())?;
        if cancelled() {
            return remove(&staging).map(|()| SaveOutcome::Cancelled);
        }
        match std::fs::rename(&staging, &destination) {
            Ok(()) => Ok(SaveOutcome::Saved),
            Err(_) => {
                remove(&staging)?;
                Err(NOT_SAVED.into())
            }
        }
    }
}

/// Writes `bytes` to a new file next to `destination`, and makes sure they are on the disk. Nothing else is touched.
fn stage(destination: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    let folder = destination.parent().unwrap_or(Path::new("."));
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staging = folder.join(format!(".{name}.{}.part", uuid::Uuid::new_v4()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(error);
    }
    Ok(staging)
}

fn remove(staging: &Path) -> Result<(), String> {
    match std::fs::remove_file(staging) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("The incomplete diagnostic save could not be removed.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::registry::Fake;

    fn report() -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "reportVersion": 1,
            "device": { "platform": "windows", "appVersion": "0.3.2", "fileBridge": "present" },
            "host": { "selection": "this-device", "appVersion": null, "daemonVersion": null,
                "providers": [{ "provider": "claude", "version": null }, { "provider": "codex", "version": null }],
                "relay": "unknown", "storage": "unknown" },
            "connection": { "status": "ready", "issue": null, "route": "unknown" },
            "speech": { "engine": "unknown", "availability": "unknown", "permission": "unknown", "model": "unknown" },
        }))
        .unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    async fn picks(path: PathBuf) -> Option<PathBuf> {
        Some(path)
    }

    // `diagnostics.test.ts` "returns real app metadata"
    #[test]
    fn app_info_has_real_metadata() {
        let registry = Fake::default();
        let info = app_info(
            &crate::identity::CURRENT,
            env!("CARGO_PKG_VERSION"),
            &registry,
        );
        assert_eq!(info.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.platform, "windows");
        assert_eq!(info.app_id, crate::identity::CURRENT.app_id);
        let memory = info.memory.clone().expect("a running process has memory");
        assert!(memory.working_set_bytes > 1_000_000 && memory.private_bytes > 0);
        assert!(
            info.gpu_adapter
                .as_ref()
                .is_none_or(|name| !name.is_empty())
        );
        assert_eq!(
            info.webview2_runtime, None,
            "an empty registry has no runtime"
        );
        let device = info.device().unwrap();
        assert_eq!(
            (device.platform.as_str(), device.file_bridge.as_str()),
            ("windows", "present")
        );
        assert_eq!(
            device.app_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        // An app version that isn't a diagnostic version is not reported.
        let odd = AppInfo {
            app_version: "1.2.3+build".into(),
            ..info
        };
        assert!(odd.device().is_err());
    }

    #[test]
    fn version_numbers_follow_the_diagnostic_pattern() {
        for good in [
            "0.3.2",
            "10.0.1234",
            "0.6.0-rc",
            "0.6.0-beta.2",
            "1.2.3-alpha.10",
        ] {
            assert!(diagnostic_version_ok(good), "{good}");
        }
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "12345.0.0",
            "1.2.3-test",
            "1.2.3-rc.",
            "1.2.3-rc.12345",
            "v1.2.3",
            "1.2.3+4",
        ] {
            assert!(!diagnostic_version_ok(bad), "{bad}");
        }
    }

    #[test]
    fn finds_the_webview2_runtime_version_where_its_installer_records_it() {
        let machine =
            format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT}");
        let user = format!(r"Software\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT}");
        let registry = Fake::default();
        registry.set_string(&user, "pv", "130.0.2849.80");
        assert_eq!(
            webview2_runtime(&registry).as_deref(),
            Some("130.0.2849.80")
        );
        registry.set_string(&machine, "pv", "131.0.1.2");
        assert_eq!(webview2_runtime(&registry).as_deref(), Some("131.0.1.2"));
        registry.set_string(&machine, "pv", "not a version\nwith lines");
        // Fake ignores hives, so the bad machine value shadows the user's; both bad shapes are refused.
        assert_eq!(webview2_runtime(&registry), None);
    }

    // "atomically saves exactly the preview while preserving no partial file"
    #[tokio::test]
    async fn atomically_saves_exactly_the_checked_report_and_leaves_no_partial_file() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("report.json");
        std::fs::write(&destination, "previous complete report").unwrap();
        let saver = ReportSaver::default();
        let outcome = saver
            .save_report(&report(), || picks(destination.clone()), || false)
            .await;
        assert_eq!(outcome, Ok(SaveOutcome::Saved));
        // What is saved is the checked report in the schema's own order, the same data as was handed in.
        let saved = std::fs::read_to_string(&destination).unwrap();
        assert_eq!(saved, parse_diagnostic_report(&report()).unwrap());
        let (saved, given): (serde_json::Value, serde_json::Value) = (
            serde_json::from_str(&saved).unwrap(),
            serde_json::from_str(&report()).unwrap(),
        );
        assert_eq!(saved, given);
        assert_eq!(names(root.path()), ["report.json"]);
    }

    // "does not write after picker cancellation or app navigation"
    #[tokio::test]
    async fn does_not_write_after_picker_cancellation_or_the_screen_going_away() {
        let saver = ReportSaver::default();
        assert_eq!(
            saver
                .save_report(&report(), || async { None }, || false)
                .await,
            Ok(SaveOutcome::Cancelled)
        );
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("report.json");
        let outcome = saver
            .save_report(&report(), || picks(destination.clone()), || true)
            .await;
        assert_eq!(outcome, Ok(SaveOutcome::Cancelled));
        assert!(!destination.exists());
        assert!(names(root.path()).is_empty());
        // Going away after the file is staged but before it is put in place also writes nothing.
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = std::sync::Arc::clone(&calls);
        let late = move || seen.fetch_add(1, Ordering::SeqCst) >= 1;
        let outcome = saver
            .save_report(&report(), || picks(destination.clone()), late)
            .await;
        assert_eq!(outcome, Ok(SaveOutcome::Cancelled));
        assert!(names(root.path()).is_empty(), "the staged file is removed");
    }

    // "validates the allowlist before showing a destination picker"
    #[tokio::test]
    async fn validates_the_allowlist_before_showing_a_destination_picker() {
        let saver = ReportSaver::default();
        let mut with_logs: serde_json::Value = serde_json::from_str(&report()).unwrap();
        with_logs["logs"] = serde_json::json!(["SYNTHETIC_SECRET"]);
        let asked = std::sync::atomic::AtomicBool::new(false);
        let choose = || {
            asked.store(true, Ordering::SeqCst);
            async { None }
        };
        let error = saver
            .save_report(&with_logs.to_string(), choose, || false)
            .await
            .unwrap_err();
        assert_eq!(error, "Invalid diagnostic report.");
        assert!(
            saver
                .save_report("not json", || async { None }, || false)
                .await
                .is_err()
        );
        assert!(
            saver
                .save_report(&"x".repeat(20_000), || async { None }, || false)
                .await
                .is_err()
        );
        assert!(!asked.load(Ordering::SeqCst));
    }

    // "keeps the existing destination and removes staging on a failed final save"
    #[tokio::test]
    async fn keeps_the_existing_destination_and_removes_staging_on_a_failed_final_save() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("existing-folder");
        std::fs::create_dir(&destination).unwrap();
        let saver = ReportSaver::default();
        let error = saver
            .save_report(&report(), || picks(destination.clone()), || false)
            .await
            .unwrap_err();
        assert_eq!(error, "The diagnostic report could not be saved.");
        assert_eq!(names(root.path()), ["existing-folder"]);
        // A destination in a folder that doesn't exist fails too, and leaves nothing.
        let missing = root.path().join("no-such-folder").join("report.json");
        assert!(
            saver
                .save_report(&report(), || picks(missing.clone()), || false)
                .await
                .is_err()
        );
        assert_eq!(names(root.path()), ["existing-folder"]);
        // And the saver is free again afterwards.
        assert!(
            saver
                .save_report(&report(), || picks(root.path().join("ok.json")), || false)
                .await
                .is_ok()
        );
    }

    // "does not open overlapping save dialogs"
    #[tokio::test]
    async fn does_not_open_overlapping_save_dialogs() {
        let saver = std::sync::Arc::new(ReportSaver::default());
        let (release, gate) = tokio::sync::oneshot::channel::<Option<PathBuf>>();
        let first = tokio::spawn({
            let saver = std::sync::Arc::clone(&saver);
            async move {
                saver
                    .save_report(
                        &report(),
                        || async move { gate.await.unwrap_or(None) },
                        || false,
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let second = saver
            .save_report(
                &report(),
                || {
                    asked.fetch_add(1, Ordering::SeqCst);
                    async { None }
                },
                || false,
            )
            .await;
        assert!(second.unwrap_err().contains("already open"));
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        release.send(None).unwrap();
        assert_eq!(first.await.unwrap(), Ok(SaveOutcome::Cancelled));
    }

    #[tokio::test]
    async fn the_crash_report_is_saved_the_same_way_with_the_end_of_the_log() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let info = app_info(&crate::identity::CURRENT, "0.6.0", &Fake::default());
        // No log: the report says so.
        assert!(crash_report_text(&info, root.path()).contains("(no crash log)"));
        // A short log is all there. A long one is cut to its end, starting at a whole line.
        std::fs::write(
            logs.join("desktop-crash.log"),
            "2026-11-14 panic at a.rs:1\n",
        )
        .unwrap();
        let text = crash_report_text(&info, root.path());
        assert!(text.starts_with("Tjommie crash report\nVersion: 0.6.0\n"));
        assert!(text.ends_with("2026-11-14 panic at a.rs:1\n"));
        let long = format!("{}\nTHE-LAST-LINE\n", "ü line\n".repeat(30_000));
        std::fs::write(logs.join("desktop-crash.log"), &long).unwrap();
        let text = crash_report_text(&info, root.path());
        let log = text.split("--- desktop-crash.log ---\n").nth(1).unwrap();
        assert!(log.len() <= CRASH_LOG_TAIL_BYTES + 4 && log.ends_with("THE-LAST-LINE\n"));
        assert!(log.starts_with("ü line\n") && !log.contains('\u{FFFD}'));
        let saver = ReportSaver::default();
        let destination = root.path().join("crash.txt");
        assert_eq!(
            saver
                .save_text(text.clone(), || picks(destination.clone()), || false)
                .await,
            Ok(SaveOutcome::Saved)
        );
        assert_eq!(std::fs::read_to_string(destination).unwrap(), text);
    }
}
