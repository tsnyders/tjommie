//! The updater's tests. None reaches a real server: the state machine runs against a fake `Http`, and the one test of
//! the real client talks to a throw-away server on 127.0.0.1 (the test build of `Policy::check` refuses every other
//! address).

use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

use futures_util::FutureExt as _;

use super::*;

const APP_ID: &str = "com.example.app";
const INSTALLER: &str = "App-Setup-0.7.0.exe";

fn sha512(bytes: &[u8]) -> [u8; 64] {
    Sha512::digest(bytes).into()
}

fn feed_for(version: &str, name: &str, bytes: &[u8]) -> String {
    feed::feed_text(version, name, &sha512(bytes), bytes.len() as u64)
}

#[derive(Default)]
struct FakeHttp {
    feed: Mutex<Option<Result<String, String>>>,
    installer: Mutex<Vec<u8>>,
    download_error: Mutex<Option<String>>,
    texts: AtomicUsize,
    downloads: AtomicUsize,
    asked: Mutex<Vec<String>>,
}

impl FakeHttp {
    fn serving(feed: String, installer: &[u8]) -> Arc<FakeHttp> {
        let http = FakeHttp::default();
        *http.feed.lock().unwrap() = Some(Ok(feed));
        *http.installer.lock().unwrap() = installer.to_vec();
        Arc::new(http)
    }
}

impl Http for FakeHttp {
    fn get_text<'a>(&'a self, url: &'a Url, _max: usize) -> BoxFuture<'a, Result<String, String>> {
        self.texts.fetch_add(1, Ordering::SeqCst);
        self.asked.lock().unwrap().push(url.to_string());
        let answer = self.feed.lock().unwrap().clone();
        async move {
            // A real request takes time; this lets a second check start while the first is still going.
            tokio::task::yield_now().await;
            answer.unwrap_or_else(|| Err("no feed".into()))
        }
        .boxed()
    }

    fn download<'a>(
        &'a self,
        url: &'a Url,
        destination: &'a Path,
        _max: u64,
        progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        self.asked.lock().unwrap().push(url.to_string());
        let bytes = self.installer.lock().unwrap().clone();
        let error = self.download_error.lock().unwrap().clone();
        async move {
            if let Some(error) = error {
                std::fs::write(destination, b"partial").unwrap();
                return Err(error);
            }
            std::fs::write(destination, &bytes).unwrap();
            let total = bytes.len() as u64;
            progress(total / 2, Some(total));
            progress(total, Some(total));
            Ok(())
        }
        .boxed()
    }
}

struct FakeInspector(Result<String, String>);

impl Inspector for FakeInspector {
    fn app_id(&self, _installer: &Path) -> Result<String, String> {
        self.0.clone()
    }
}

#[derive(Default)]
struct FakeLauncher {
    launches: Mutex<Vec<(PathBuf, Vec<String>)>>,
}

impl InstallerLauncher for FakeLauncher {
    fn launch(&self, installer: &Path, args: &[&str]) -> Result<(), String> {
        self.launches.lock().unwrap().push((
            installer.to_path_buf(),
            args.iter().map(|arg| (*arg).to_string()).collect(),
        ));
        Ok(())
    }
}

struct Fixture {
    updater: Updater,
    launcher: Arc<FakeLauncher>,
    quits: Arc<AtomicUsize>,
    order: Arc<Mutex<Vec<&'static str>>>,
    root: tempfile::TempDir,
}

fn feed_url() -> Url {
    Url::parse("http://127.0.0.1:9/updates/latest.yml").unwrap()
}

struct Options {
    feed: Option<Url>,
    app_id: Result<String, String>,
    before_restart: Option<BeforeRestart>,
    current: &'static str,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            feed: Some(feed_url()),
            app_id: Ok(APP_ID.into()),
            before_restart: None,
            current: "0.5.0",
        }
    }
}

fn fixture(http: Arc<FakeHttp>, options: Options) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let launcher = Arc::new(FakeLauncher::default());
    let quits = Arc::new(AtomicUsize::new(0));
    let order = Arc::new(Mutex::new(Vec::new()));
    let quit = {
        let quits = Arc::clone(&quits);
        let order = Arc::clone(&order);
        Arc::new(move || {
            quits.fetch_add(1, Ordering::SeqCst);
            order.lock().unwrap().push("quit");
        })
    };
    let updater = Updater::new(
        UpdaterConfig {
            app_id: APP_ID.into(),
            current: Version::parse(options.current).unwrap(),
            feed: options.feed,
            download_root: root.path().to_path_buf(),
        },
        UpdaterDeps {
            http: Arc::clone(&http) as Arc<dyn Http>,
            inspector: Arc::new(FakeInspector(options.app_id)),
            launcher: Arc::clone(&launcher) as Arc<dyn InstallerLauncher>,
            quit,
            before_restart: options.before_restart,
        },
        Handle::current(),
    );
    Fixture {
        updater,
        launcher,
        quits,
        order,
        root,
    }
}

fn installer_bytes() -> Vec<u8> {
    b"MZ a pretend installer".to_vec()
}

fn serving_a_new_version() -> Arc<FakeHttp> {
    let bytes = installer_bytes();
    FakeHttp::serving(feed_for("0.7.0", INSTALLER, &bytes), &bytes)
}

fn ready(version: &str) -> UpdateState {
    UpdateState::Ready {
        version: version.into(),
    }
}

// `updater.test.ts` "reports unsupported without configuring or checking an unpackaged app"
#[tokio::test]
async fn reports_unsupported_without_a_feed_and_checks_nothing() {
    let http = serving_a_new_version();
    let f = fixture(
        Arc::clone(&http),
        Options {
            feed: None,
            ..Options::default()
        },
    );
    assert_eq!(f.updater.state(), UpdateState::Unsupported);
    f.updater.check().await;
    f.updater.start();
    f.updater.restart();
    assert_eq!(http.texts.load(Ordering::SeqCst), 0);
    assert_eq!(f.quits.load(Ordering::SeqCst), 0);
    assert!(!f.updater.installation_pending());
    f.updater.stop();
}

// "tracks download and ready states, and restarts silently"
#[tokio::test(start_paused = true)]
async fn tracks_download_and_ready_states_and_restarts_silently() {
    let http = serving_a_new_version();
    let f = fixture(Arc::clone(&http), Options::default());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let _stop = f
        .updater
        .on_state(move |state| sink.lock().unwrap().push(state.clone()));
    assert_eq!(f.updater.state(), UpdateState::Idle);
    f.updater.start();
    tokio::task::yield_now().await; // the timers register their sleeps now, not after the clock moves
    // The first check comes after 60 s, not before.
    tokio::time::advance(Duration::from_secs(59)).await;
    assert_eq!(http.texts.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(http.texts.load(Ordering::SeqCst), 1);
    assert_eq!(f.updater.state(), ready("0.7.0"));
    assert_eq!(
        *seen.lock().unwrap(),
        [
            UpdateState::Checking,
            UpdateState::Downloading { percent: 0.0 },
            UpdateState::Downloading { percent: 50.0 },
            UpdateState::Downloading { percent: 100.0 },
            ready("0.7.0"),
        ]
    );
    assert!(f.updater.installation_pending());

    // A later check that fails, or finds nothing, doesn't take Ready away.
    *http.feed.lock().unwrap() = Some(Err("offline".into()));
    f.updater.check().await;
    assert_eq!(f.updater.state(), ready("0.7.0"));
    // The next check is six hours after the start, not six after the first.
    tokio::time::advance(CHECK_EVERY - FIRST_CHECK).await;
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        http.texts.load(Ordering::SeqCst),
        3,
        "the first, the manual one and the six-hourly one"
    );

    f.updater.restart();
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
    assert!(
        f.launcher.launches.lock().unwrap().is_empty(),
        "the installer starts from the quit path, not here"
    );
    assert_eq!(f.updater.install_on_quit(), Ok(true));
    {
        let launches = f.launcher.launches.lock().unwrap();
        assert_eq!(launches.len(), 1);
        let (path, args) = launches.first().unwrap();
        assert_eq!(
            path,
            &f.root.path().join("tjommie-update-0.7.0").join(INSTALLER)
        );
        // electron-updater's arguments for a silent restart: `--updated` so the new app starts in the tray, `/S`,
        // and `--force-run` so the silent installer starts it again.
        assert_eq!(args, &["--updated", "/S", "--force-run"]);
    }
    assert_eq!(f.updater.install_on_quit(), Ok(false), "once");
    assert_eq!(f.launcher.launches.lock().unwrap().len(), 1);
    // Once the installer has started, nothing checks again.
    f.updater.check().await;
    assert_eq!(http.texts.load(Ordering::SeqCst), 3);
    f.updater.stop();
}

// A plain Quit with a downloaded update installs it, silently and without relaunching (autoInstallOnAppQuit).
#[tokio::test]
async fn a_plain_quit_installs_a_ready_update_without_relaunching() {
    let f = fixture(serving_a_new_version(), Options::default());
    f.updater.check().await;
    assert_eq!(f.updater.state(), ready("0.7.0"));
    assert!(f.updater.installation_pending());
    assert_eq!(f.updater.install_on_quit(), Ok(true));
    let launches = f.launcher.launches.lock().unwrap();
    assert_eq!(launches.first().unwrap().1, ["--updated", "/S"]);
}

// "exports the paired PCs and appearance first, then quits, when restarting for an update"
#[tokio::test]
async fn runs_before_restart_first_then_quits() {
    let order_for_hook = Arc::new(Mutex::new(Vec::new()));
    let hook_order = Arc::clone(&order_for_hook);
    let hook: BeforeRestart = Arc::new(move || {
        let order = Arc::clone(&hook_order);
        async move {
            order.lock().unwrap().push("export");
            Ok(())
        }
        .boxed()
    });
    let f = fixture(
        serving_a_new_version(),
        Options {
            before_restart: Some(hook),
            ..Options::default()
        },
    );
    f.updater.check().await;
    let shared_order = Arc::clone(&f.order);
    f.updater.restart();
    assert_eq!(
        f.quits.load(Ordering::SeqCst),
        0,
        "not before the hook has run"
    );
    for _ in 0..50 {
        if f.quits.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
    assert_eq!(*order_for_hook.lock().unwrap(), ["export"]);
    assert_eq!(*shared_order.lock().unwrap(), ["quit"]);
    assert_eq!(f.updater.install_on_quit(), Ok(true));
}

// "a hung export can't hold the update install past 2 seconds"
#[tokio::test(start_paused = true)]
async fn a_hung_before_restart_cant_hold_the_update_past_two_seconds() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let hook_gate = Arc::clone(&gate);
    let hook: BeforeRestart = Arc::new(move || {
        let gate = Arc::clone(&hook_gate);
        async move {
            gate.notified().await;
            Ok(())
        }
        .boxed()
    });
    let f = fixture(
        serving_a_new_version(),
        Options {
            before_restart: Some(hook),
            ..Options::default()
        },
    );
    f.updater.check().await;
    f.updater.restart();
    tokio::task::yield_now().await; // the bounded wait starts its 2 s now
    tokio::time::advance(Duration::from_millis(1_999)).await;
    tokio::task::yield_now().await;
    assert_eq!(f.quits.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_millis(1)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
    // The page answers at last, too late: that must not quit a second time.
    gate.notify_waiters();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
    assert_eq!(f.updater.install_on_quit(), Ok(true));
}

// "an export that fails doesn't stop the update install"
#[tokio::test]
async fn a_failing_before_restart_doesnt_stop_the_install() {
    let hook: BeforeRestart = Arc::new(|| async { Err("The page is gone.".to_string()) }.boxed());
    let f = fixture(
        serving_a_new_version(),
        Options {
            before_restart: Some(hook),
            ..Options::default()
        },
    );
    f.updater.check().await;
    f.updater.restart();
    for _ in 0..50 {
        if f.quits.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
    assert_eq!(f.updater.install_on_quit(), Ok(true));
}

// "restarting again while the export runs quits only once"
#[tokio::test]
async fn restarting_again_while_the_hook_runs_quits_only_once() {
    let started = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Notify::new());
    let (hook_started, hook_gate) = (Arc::clone(&started), Arc::clone(&gate));
    let hook: BeforeRestart = Arc::new(move || {
        hook_started.fetch_add(1, Ordering::SeqCst);
        let gate = Arc::clone(&hook_gate);
        async move {
            gate.notified().await;
            Ok(())
        }
        .boxed()
    });
    let f = fixture(
        serving_a_new_version(),
        Options {
            before_restart: Some(hook),
            ..Options::default()
        },
    );
    f.updater.check().await;
    f.updater.restart();
    f.updater.restart();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(started.load(Ordering::SeqCst), 1);
    gate.notify_waiters();
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
}

// "cancels installation when daemon shutdown fails"
#[tokio::test]
async fn cancels_installation_when_daemon_shutdown_fails() {
    let f = fixture(serving_a_new_version(), Options::default());
    f.updater.check().await;
    assert!(f.updater.installation_pending());
    f.updater.restart();
    f.updater.shutdown_failed();
    assert_eq!(f.updater.install_on_quit(), Ok(false));
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: SHUTDOWN_FAILURE.into()
        }
    );
    assert!(!f.updater.installation_pending());
    assert!(f.launcher.launches.lock().unwrap().is_empty());
    assert_eq!(f.quits.load(Ordering::SeqCst), 1);
}

// "does not treat a plain Quit or unsupported build as an update install"
#[tokio::test]
async fn a_plain_quit_or_an_unsupported_build_is_not_an_install() {
    let f = fixture(serving_a_new_version(), Options::default());
    assert!(!f.updater.installation_pending());
    f.updater.check().await;
    assert!(f.updater.installation_pending());
    let off = fixture(
        serving_a_new_version(),
        Options {
            feed: None,
            ..Options::default()
        },
    );
    assert!(!off.updater.installation_pending());
}

// "maps no-update and errors to state without throwing"
#[tokio::test]
async fn maps_no_update_and_errors_to_state_without_panicking() {
    let bytes = installer_bytes();
    let http = FakeHttp::serving(feed_for("0.5.0", INSTALLER, &bytes), &bytes);
    let f = fixture(Arc::clone(&http), Options::default());
    f.updater.check().await;
    assert_eq!(f.updater.state(), UpdateState::None);
    *http.feed.lock().unwrap() = Some(Err("offline".into()));
    f.updater.check().await;
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: "offline".into()
        }
    );
    // A rejected download is the same.
    *http.feed.lock().unwrap() = Some(Ok(feed_for("0.7.0", INSTALLER, &bytes)));
    *http.download_error.lock().unwrap() = Some("download failed".into());
    f.updater.check().await;
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: "download failed".into()
        }
    );
    let leftovers: Vec<_> = std::fs::read_dir(f.root.path().join("tjommie-update-0.7.0"))
        .unwrap()
        .collect();
    assert!(
        leftovers.is_empty(),
        "a failed download leaves nothing behind"
    );
    // And the next check can still succeed.
    *http.download_error.lock().unwrap() = None;
    f.updater.check().await;
    assert_eq!(f.updater.state(), ready("0.7.0"));
}

#[tokio::test]
async fn only_a_newer_version_is_an_update() {
    let bytes = installer_bytes();
    for (current, feed_version) in [
        ("0.7.0", "0.7.0"),
        ("0.8.0", "0.7.0"),
        ("0.7.0-beta.1", "0.7.0-alpha.1"),
        ("1.0.0", "0.9.9"),
    ] {
        let http = FakeHttp::serving(feed_for(feed_version, INSTALLER, &bytes), &bytes);
        let f = fixture(
            Arc::clone(&http),
            Options {
                current,
                ..Options::default()
            },
        );
        f.updater.check().await;
        assert_eq!(
            f.updater.state(),
            UpdateState::None,
            "{current} -> {feed_version}"
        );
        assert_eq!(http.downloads.load(Ordering::SeqCst), 0);
    }
    // A pre-release of a later version is newer than the release before it.
    let http = FakeHttp::serving(feed_for("0.7.0-test", INSTALLER, &bytes), &bytes);
    let f = fixture(
        http,
        Options {
            current: "0.6.9",
            ..Options::default()
        },
    );
    f.updater.check().await;
    assert_eq!(f.updater.state(), ready("0.7.0-test"));
}

// Review focus: the checksum. A byte changed anywhere in the download is a refusal.
#[tokio::test]
async fn rejects_bad_sha512() {
    let good = installer_bytes();
    let mut tampered = good.clone();
    tampered[3] ^= 1;
    let http = FakeHttp::serving(feed_for("0.7.0", INSTALLER, &good), &tampered);
    let f = fixture(Arc::clone(&http), Options::default());
    f.updater.check().await;
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: BAD_CHECKSUM.into()
        }
    );
    assert!(!f.updater.installation_pending());
    assert_eq!(f.updater.install_on_quit(), Ok(false));
    assert!(f.launcher.launches.lock().unwrap().is_empty());
    let folder = f.root.path().join("tjommie-update-0.7.0");
    assert!(
        std::fs::read_dir(folder).unwrap().next().is_none(),
        "the refused file is deleted"
    );

    // The right bytes with the wrong length for the feed's size are refused too.
    let wrong_size = feed::feed_text("0.7.0", INSTALLER, &sha512(&good), good.len() as u64 + 1);
    let http = FakeHttp::serving(wrong_size, &good);
    let f = fixture(http, Options::default());
    f.updater.check().await;
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: BAD_CHECKSUM.into()
        }
    );
}

// Review focus: an installer built for another app (a dev build's, or one a bad feed points at) never installs.
#[tokio::test]
async fn rejects_foreign_app_id() {
    for id in [
        Ok("com.example.other".to_string()),
        Ok(String::new()),
        Err("no version information".to_string()),
    ] {
        let f = fixture(
            serving_a_new_version(),
            Options {
                app_id: id,
                ..Options::default()
            },
        );
        f.updater.check().await;
        assert_eq!(
            f.updater.state(),
            UpdateState::Error {
                message: FOREIGN_APP.into()
            }
        );
        assert!(!f.updater.installation_pending());
        assert_eq!(f.updater.install_on_quit(), Ok(false));
        assert!(f.launcher.launches.lock().unwrap().is_empty());
        let folder = f.root.path().join("tjommie-update-0.7.0");
        assert!(
            std::fs::read_dir(folder).unwrap().next().is_none(),
            "the refused file is deleted"
        );
    }
}

// The file is checked again just before it runs: whatever happened to it on disk since the download is not run.
#[tokio::test]
async fn a_file_changed_after_the_download_is_not_run() {
    let f = fixture(serving_a_new_version(), Options::default());
    f.updater.check().await;
    let file = f.root.path().join("tjommie-update-0.7.0").join(INSTALLER);
    std::fs::write(&file, b"MZ something else").unwrap();
    assert_eq!(
        f.updater.install_on_quit(),
        Err(CHANGED_ON_DISK.to_string())
    );
    assert!(f.launcher.launches.lock().unwrap().is_empty());
    assert!(!file.exists());
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: CHANGED_ON_DISK.into()
        }
    );
    assert!(!f.updater.installation_pending());
}

#[tokio::test]
async fn an_intact_download_from_an_earlier_run_is_reused() {
    let http = serving_a_new_version();
    let f = fixture(Arc::clone(&http), Options::default());
    f.updater.check().await;
    assert_eq!(http.downloads.load(Ordering::SeqCst), 1);
    // A new run of the app: the same feed, the same folder.
    let again = Updater::new(
        UpdaterConfig {
            app_id: APP_ID.into(),
            current: Version::new(0, 5, 0),
            feed: Some(feed_url()),
            download_root: f.root.path().to_path_buf(),
        },
        UpdaterDeps {
            http: Arc::clone(&http) as Arc<dyn Http>,
            inspector: Arc::new(FakeInspector(Ok(APP_ID.into()))),
            launcher: Arc::new(FakeLauncher::default()),
            quit: Arc::new(|| {}),
            before_restart: None,
        },
        Handle::current(),
    );
    again.check().await;
    assert_eq!(again.state(), ready("0.7.0"));
    assert_eq!(
        http.downloads.load(Ordering::SeqCst),
        1,
        "no second download"
    );
}

#[tokio::test]
async fn a_feed_that_names_a_path_or_another_host_downloads_nothing() {
    let bytes = installer_bytes();
    for name in [
        "..\\evil.exe",
        "sub/evil.exe",
        "https://evil.example/a.exe",
        "C:evil.exe",
    ] {
        let http = FakeHttp::serving(feed_for("0.7.0", name, &bytes), &bytes);
        let f = fixture(Arc::clone(&http), Options::default());
        f.updater.check().await;
        assert!(
            matches!(f.updater.state(), UpdateState::Error { .. }),
            "{name}"
        );
        assert_eq!(http.downloads.load(Ordering::SeqCst), 0, "{name}");
    }
}

#[tokio::test]
async fn the_installer_is_asked_for_next_to_the_feed() {
    let http = serving_a_new_version();
    let f = fixture(Arc::clone(&http), Options::default());
    f.updater.check().await;
    let asked = http.asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        [
            "http://127.0.0.1:9/updates/latest.yml",
            "http://127.0.0.1:9/updates/App-Setup-0.7.0.exe"
        ]
    );
}

// A second check while one runs (the timer and the settings screen's button) is one check.
#[tokio::test]
async fn overlapping_checks_are_one_check() {
    let http = serving_a_new_version();
    let f = fixture(Arc::clone(&http), Options::default());
    tokio::join!(f.updater.check(), f.updater.check());
    assert_eq!(http.texts.load(Ordering::SeqCst), 1);
    assert_eq!(f.updater.state(), ready("0.7.0"));
}

// "disables checks by environment and ignores restart before an update is ready"
#[test]
fn the_environment_and_the_identity_decide_whether_there_is_a_feed() {
    fn identity(kind: Kind, update_feed: Option<&'static str>) -> Identity {
        Identity {
            kind,
            app_id: APP_ID,
            product: "App",
            update_feed,
            ..crate::identity::DEV
        }
    }
    let production = identity(
        Kind::Production,
        Some("https://github.com/example/app/releases/latest/download/latest.yml"),
    );
    let none = UpdateEnv::default();
    assert_eq!(
        resolve_feed(&production, &none).unwrap().unwrap().as_str(),
        "https://github.com/example/app/releases/latest/download/latest.yml"
    );
    // Off by TJOMMIE_UPDATES, and off for a smoke run with its own user-data folder.
    assert_eq!(
        resolve_feed(
            &production,
            &UpdateEnv {
                updates: Some("off"),
                ..none
            }
        )
        .unwrap(),
        None
    );
    assert_eq!(
        resolve_feed(
            &production,
            &UpdateEnv {
                user_data: Some(r"C:\tmp\smoke"),
                ..none
            }
        )
        .unwrap(),
        None
    );
    // A production build never takes its feed from the environment.
    let redirected = UpdateEnv {
        update_feed: Some("http://127.0.0.1:9/"),
        ..none
    };
    assert_eq!(
        resolve_feed(&production, &redirected).unwrap(),
        production.update_feed.map(|f| Url::parse(f).unwrap())
    );

    // The update test takes only a local feed, as a folder, and nothing without one.
    let test = identity(Kind::UpdateTest, None);
    assert_eq!(resolve_feed(&test, &none).unwrap(), None);
    let local = |text| {
        resolve_feed(
            &test,
            &UpdateEnv {
                update_feed: Some(text),
                ..none
            },
        )
    };
    assert_eq!(
        local("http://127.0.0.1:8765").unwrap().unwrap().as_str(),
        "http://127.0.0.1:8765/latest.yml"
    );
    assert_eq!(
        local("http://127.0.0.1:8765/feed/")
            .unwrap()
            .unwrap()
            .as_str(),
        "http://127.0.0.1:8765/feed/latest.yml"
    );
    assert_eq!(
        local("http://localhost:1/feed").unwrap().unwrap().as_str(),
        "http://localhost:1/feed/latest.yml"
    );
    for refused in [
        "https://evil.example/",
        "http://192.168.1.2/",
        "https://github.com/a/b/",
        "not an address",
    ] {
        assert!(local(refused).is_err(), "{refused}");
    }
}

// `identity::dev_has_no_feed`, from the updater's side: no environment gives a dev or e2e build a feed.
#[test]
fn dev_identity_has_no_feed() {
    let env = UpdateEnv {
        updates: None,
        update_feed: Some("http://127.0.0.1:8765"),
        user_data: None,
    };
    for kind in [Kind::Dev, Kind::E2e] {
        let identity = Identity {
            kind,
            app_id: APP_ID,
            product: "App",
            ..crate::identity::DEV
        };
        assert_eq!(resolve_feed(&identity, &env).unwrap(), None);
    }
    assert_eq!(
        crate::identity::CURRENT.update_feed.is_some(),
        crate::identity::CURRENT.kind == Kind::Production
    );
}

// ---- the quit path ----------------------------------------------------------------------------------------------

struct FakeHost {
    stop: Mutex<VecDeque<Result<DaemonStop, String>>>,
    calls: Mutex<Vec<String>>,
}

impl FakeHost {
    fn stopping(result: Result<DaemonStop, String>) -> FakeHost {
        FakeHost {
            stop: Mutex::new(VecDeque::from([result])),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl QuitHost for FakeHost {
    async fn close_native(&self) {
        self.calls.lock().unwrap().push("close_native".into());
    }
    async fn stop_daemon(&self) -> Result<DaemonStop, String> {
        self.calls.lock().unwrap().push("stop_daemon".into());
        self.stop
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(DaemonStop::Graceful))
    }
    async fn close_native_host(&self) {
        self.calls.lock().unwrap().push("close_native_host".into());
    }
    fn update_not_installed(&self, daemon_stopped: bool) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("update_not_installed({daemon_stopped})"));
    }
}

async fn ready_for_quit() -> Fixture {
    let f = fixture(serving_a_new_version(), Options::default());
    f.updater.check().await;
    f.updater.restart();
    f
}

#[tokio::test]
async fn quit_stops_the_daemon_before_the_installer_starts() {
    let f = ready_for_quit().await;
    let host = FakeHost::stopping(Ok(DaemonStop::Graceful));
    assert_eq!(
        run_quit(&host, &f.updater).await,
        QuitOutcome::Quit {
            installer_started: true
        }
    );
    assert_eq!(
        host.calls(),
        ["close_native", "stop_daemon", "close_native_host"]
    );
    assert_eq!(f.launcher.launches.lock().unwrap().len(), 1);
}

// Review focus and the plan's test: a daemon that had to be forced means the update is not installed.
#[tokio::test]
async fn quit_with_forced_daemon_stop_skips_install() {
    let f = ready_for_quit().await;
    let host = FakeHost::stopping(Ok(DaemonStop::Forced));
    assert_eq!(run_quit(&host, &f.updater).await, QuitOutcome::Aborted);
    assert!(f.launcher.launches.lock().unwrap().is_empty());
    assert_eq!(
        host.calls().last().map(String::as_str),
        Some("update_not_installed(true)")
    );
    assert_eq!(
        f.updater.state(),
        UpdateState::Error {
            message: SHUTDOWN_FAILURE.into()
        }
    );
    assert!(!f.updater.installation_pending());
    // Quitting again, later, doesn't install it either.
    assert_eq!(f.updater.install_on_quit(), Ok(false));
}

#[tokio::test]
async fn quit_when_the_daemons_stop_cant_be_confirmed_skips_install() {
    let f = ready_for_quit().await;
    let host = FakeHost::stopping(Err("the process list failed".into()));
    assert_eq!(run_quit(&host, &f.updater).await, QuitOutcome::Aborted);
    assert!(f.launcher.launches.lock().unwrap().is_empty());
    assert_eq!(
        host.calls().last().map(String::as_str),
        Some("update_not_installed(false)")
    );
}

// With no update to install, a daemon that needed forcing doesn't hold the quit.
#[tokio::test]
async fn a_plain_quit_goes_ahead_whatever_the_daemon_did() {
    for stop in [Ok(DaemonStop::Forced), Err("unknown".to_string())] {
        let f = fixture(serving_a_new_version(), Options::default());
        let host = FakeHost::stopping(stop);
        assert_eq!(
            run_quit(&host, &f.updater).await,
            QuitOutcome::Quit {
                installer_started: false
            }
        );
        assert!(
            !host
                .calls()
                .iter()
                .any(|call| call.starts_with("update_not_installed"))
        );
        assert!(f.launcher.launches.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_downloaded_file_that_changed_is_not_run_at_quit() {
    let f = ready_for_quit().await;
    std::fs::write(
        f.root.path().join("tjommie-update-0.7.0").join(INSTALLER),
        b"MZ changed",
    )
    .unwrap();
    let host = FakeHost::stopping(Ok(DaemonStop::Graceful));
    assert_eq!(
        run_quit(&host, &f.updater).await,
        QuitOutcome::Quit {
            installer_started: false
        }
    );
    assert!(f.launcher.launches.lock().unwrap().is_empty());
}

// ---- the real HTTP client, against a server on 127.0.0.1 ----------------------------------------------------------

/// A path, the status to answer it with, extra headers, and the body.
type Route = (&'static str, u16, Vec<(&'static str, String)>, Vec<u8>);

/// Serves `routes` on a loopback port and returns it with a log of the paths asked for. One request per connection.
async fn serve(routes: Vec<Route>) -> (u16, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0u8; 4096];
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            seen.lock().unwrap().push(path.clone());
            let (status, headers, body) = routes.iter().find(|(route, ..)| *route == path).map_or(
                (404, Vec::new(), b"not found".to_vec()),
                |(_, status, headers, body)| (*status, headers.clone(), body.clone()),
            );
            let mut response = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                body.len()
            );
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        }
    });
    (port, log)
}

#[tokio::test]
async fn the_real_client_follows_allowed_redirects_and_refuses_the_rest() {
    let bytes = installer_bytes();
    let feed = feed_for("0.7.0", INSTALLER, &bytes);
    // Another local server, which a redirect tries to send the updater to. The policy allows one port, so the updater
    // must refuse it without asking, even though the address is on this PC.
    let (other_port, other_log) = serve(vec![("/steal", 200, vec![], b"gotcha".to_vec())]).await;
    let (port, log) = serve(vec![
        ("/feed/latest.yml", 200, vec![], feed.into_bytes()),
        // The installer sits behind a redirect, as GitHub's release downloads do.
        (
            "/feed/App-Setup-0.7.0.exe",
            302,
            vec![("location", "/assets/1".into())],
            vec![],
        ),
        ("/assets/1", 200, vec![], bytes.clone()),
        // A redirect off this PC is never followed.
        (
            "/escape",
            302,
            vec![("location", format!("http://127.0.0.1:{other_port}/steal"))],
            vec![],
        ),
        ("/loop", 302, vec![("location", "/loop".into())], vec![]),
        ("/big", 200, vec![], vec![b'x'; 100]),
    ])
    .await;
    let base = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
    let client = ReqwestHttp::new(Policy::LoopbackPort(port)).unwrap();

    let text = client
        .get_text(&base.join("/feed/latest.yml").unwrap(), MAX_FEED_BYTES)
        .await
        .unwrap();
    assert!(text.starts_with("version: 0.7.0"));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.exe");
    let received = Mutex::new(Vec::new());
    client
        .download(
            &base.join("/feed/App-Setup-0.7.0.exe").unwrap(),
            &target,
            1024,
            &|done, total| received.lock().unwrap().push((done, total)),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    assert_eq!(
        received.lock().unwrap().last().copied(),
        Some((bytes.len() as u64, Some(bytes.len() as u64)))
    );

    assert!(
        client
            .get_text(&base.join("/escape").unwrap(), 1024)
            .await
            .is_err()
    );
    assert!(
        log.lock().unwrap().iter().any(|path| path == "/escape"),
        "the redirect was served"
    );
    assert!(
        other_log.lock().unwrap().is_empty(),
        "the address the redirect pointed at was never asked for"
    );
    assert!(
        client
            .get_text(&base.join("/loop").unwrap(), 1024)
            .await
            .unwrap_err()
            .contains("too many")
    );
    assert!(
        client
            .get_text(&base.join("/missing").unwrap(), 1024)
            .await
            .unwrap_err()
            .contains("404")
    );
    // The size limits hold on text and on downloads.
    assert!(
        client
            .get_text(&base.join("/big").unwrap(), 10)
            .await
            .is_err()
    );
    let capped = dir.path().join("capped.exe");
    assert!(
        client
            .download(&base.join("/big").unwrap(), &capped, 10, &|_, _| {})
            .await
            .is_err()
    );
}

// The whole flow through the real client, with only the installer inspection faked.
#[tokio::test]
async fn an_update_flows_through_the_real_client_to_ready() {
    let bytes = installer_bytes();
    let (port, _log) = serve(vec![
        (
            "/latest.yml",
            200,
            vec![],
            feed_for("0.7.0", INSTALLER, &bytes).into_bytes(),
        ),
        ("/App-Setup-0.7.0.exe", 200, vec![], bytes),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let updater = Updater::new(
        UpdaterConfig {
            app_id: APP_ID.into(),
            current: Version::new(0, 5, 0),
            feed: Some(Url::parse(&format!("http://127.0.0.1:{port}/latest.yml")).unwrap()),
            download_root: root.path().to_path_buf(),
        },
        UpdaterDeps {
            http: Arc::new(ReqwestHttp::new(Policy::Loopback).unwrap()),
            inspector: Arc::new(FakeInspector(Ok(APP_ID.into()))),
            launcher: Arc::new(FakeLauncher::default()),
            quit: Arc::new(|| {}),
            before_restart: None,
        },
        Handle::current(),
    );
    updater.check().await;
    assert_eq!(updater.state(), ready("0.7.0"));
}
