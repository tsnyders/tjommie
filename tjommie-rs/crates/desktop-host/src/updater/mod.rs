//! The desktop updater (spec §3.10; port of `updater.ts`, with electron-updater's job done here).
//!
//! The chain of trust, link by link, because the app runs whatever this downloads:
//! 1. The feed's address is compiled into production builds only, or (update-test builds) a local address from the
//!    environment. Dev and e2e builds have none, and the address isn't even in their binary (`identity`).
//! 2. The feed and the installer are fetched over HTTPS from github.com or the two hosts its release downloads redirect to
//!    (objects. and release-assets.githubusercontent.com, by name), and every redirect hop is
//!    checked against that list (`http::Policy`).
//! 3. `latest.yml` is parsed strictly (`feed`): a plain `.exe` name, a SHA-512 of exactly 64 bytes, agreeing fields.
//! 4. The download's SHA-512 must equal the feed's, and its size must match. It is checked again just before the
//!    installer starts, so a file changed on disk in between is not run.
//! 5. The installer's embedded `AppId` must be this app's own (`version_info`), so a feed can't make a production
//!    app install a dev or other build.
//! 6. The version must be newer than the running one.
//!
//! There is no publisher signature: Windows builds are unsigned (owner decision, 2026-10-25), so links 1 and 2 (the
//! GitHub account and TLS) are what the checksum in link 3 rests on, exactly as with electron-updater.

mod feed;
mod http;
mod quit;
mod state;
pub mod version_info;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::BoxFuture;
use semver::Version;
use sha2::{Digest as _, Sha512};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use url::Url;

use crate::identity::{Identity, Kind};
use crate::spawn;

pub use feed::{MAX_FEED_BYTES, MAX_INSTALLER_BYTES, Release};
pub use http::{Http, Policy, Progress, ReqwestHttp, is_model_host};
pub use quit::{DaemonStop, QuitHost, QuitOutcome, run_quit};
pub use state::{SHUTDOWN_FAILURE, UpdateState};

/// How long Restart to update waits for `before_restart`. A page that never answers must not hold an update up.
pub const BEFORE_RESTART: Duration = Duration::from_secs(2);
/// The first check, after start-up has settled; then every six hours.
pub const FIRST_CHECK: Duration = Duration::from_secs(60);
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// What the settings screen shows for an installer that didn't pass a check. Never the server's own words.
const BAD_CHECKSUM: &str = "The update didn't match its checksum and was not installed.";
const FOREIGN_APP: &str = "The update is for a different app and was not installed.";
const CHANGED_ON_DISK: &str = "The downloaded update changed on disk and was not installed.";

/// The environment variables the updater honours (`TJOMMIE_UPDATES`, `TJOMMIE_UPDATE_FEED`, `TJOMMIE_USER_DATA`).
#[derive(Debug, Default, Clone, Copy)]
pub struct UpdateEnv<'a> {
    pub updates: Option<&'a str>,
    pub update_feed: Option<&'a str>,
    pub user_data: Option<&'a str>,
}

/// The feed to check, if any (`main.ts:223-229`):
/// - `TJOMMIE_UPDATES=off` turns it off;
/// - a production build checks its compiled-in feed, unless the run has its own `TJOMMIE_USER_DATA`, which is a smoke
///   run beside a real install and must never update it;
/// - an update-test build checks only the local feed in `TJOMMIE_UPDATE_FEED` (a folder address: `latest.yml` is
///   looked for in it), and nothing without one;
/// - dev and e2e builds never check anything, whatever the environment says.
pub fn resolve_feed(identity: &Identity, env: &UpdateEnv<'_>) -> Result<Option<Url>, String> {
    if env.updates == Some("off") {
        return Ok(None);
    }
    match identity.kind {
        Kind::Production => {
            if env.user_data.is_some_and(|dir| !dir.is_empty()) {
                return Ok(None);
            }
            identity
                .update_feed
                .map(|feed| {
                    Url::parse(feed)
                        .map_err(|_| "The built-in update feed is not an address.".to_string())
                })
                .transpose()
        }
        Kind::UpdateTest => {
            let Some(base) = env.update_feed.filter(|base| !base.is_empty()) else {
                return Ok(None);
            };
            let mut base = Url::parse(base)
                .map_err(|_| "TJOMMIE_UPDATE_FEED is not an address.".to_string())?;
            if Policy::for_feed(&base)? != Policy::Loopback {
                return Err("TJOMMIE_UPDATE_FEED must be an address on this PC.".into());
            }
            if !base.path().ends_with('/') {
                let path = format!("{}/", base.path());
                base.set_path(&path);
            }
            base.join("latest.yml")
                .map(Some)
                .map_err(|_| "TJOMMIE_UPDATE_FEED is not an address.".to_string())
        }
        Kind::Dev | Kind::E2e => Ok(None),
    }
}

/// Reads the `AppId` an installer carries.
pub trait Inspector: Send + Sync {
    fn app_id(&self, installer: &Path) -> Result<String, String>;
}

/// The real one: the installer's version resource.
pub struct VersionResource;

impl Inspector for VersionResource {
    fn app_id(&self, installer: &Path) -> Result<String, String> {
        version_info::read_string(installer, "AppId")
    }
}

/// Starts the installer. It must outlive this process.
pub trait InstallerLauncher: Send + Sync {
    fn launch(&self, installer: &Path, args: &[&str]) -> Result<(), String>;
}

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// The real one: through `spawn::command` (no console window), detached so closing this app doesn't close the installer.
pub struct DetachedLauncher;

impl InstallerLauncher for DetachedLauncher {
    fn launch(&self, installer: &Path, args: &[&str]) -> Result<(), String> {
        let mut command = spawn::command(installer);
        command.args(args);
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        command
            .spawn()
            .map(drop)
            .map_err(|_| "The installer could not be started.".to_string())
    }
}

pub type BeforeRestart = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

pub struct UpdaterConfig {
    /// The app's own id: an installer must carry the same one.
    pub app_id: String,
    pub current: Version,
    /// `latest.yml`'s address. `None`: there is nothing to check.
    pub feed: Option<Url>,
    /// Where updates are downloaded: `%TEMP%`.
    pub download_root: PathBuf,
}

pub struct UpdaterDeps {
    pub http: Arc<dyn Http>,
    pub inspector: Arc<dyn Inspector>,
    pub launcher: Arc<dyn InstallerLauncher>,
    /// Quits the app. The installer starts from the quit path (`install_on_quit`), after the daemon has stopped.
    pub quit: Arc<dyn Fn() + Send + Sync>,
    /// Runs, bounded by `BEFORE_RESTART`, before `quit` when the user restarts to update: the page is gone once the
    /// window closes, so anything that needs it happens here.
    pub before_restart: Option<BeforeRestart>,
}

type Listener = Arc<dyn Fn(&UpdateState) + Send + Sync>;

/// A downloaded installer that passed every check.
#[derive(Debug, Clone)]
struct Ready {
    path: PathBuf,
    sha512: [u8; 64],
    version: Version,
}

struct Core {
    state: UpdateState,
    restart_requested: bool,
    /// Whether a ready update installs when the app quits (electron-updater's `autoInstallOnAppQuit`).
    auto_install: bool,
    ready: Option<Ready>,
    checking: bool,
    launched: bool,
    listeners: Vec<(u64, Listener)>,
    next_listener: u64,
}

struct Inner {
    config: UpdaterConfig,
    deps: UpdaterDeps,
    handle: Handle,
    active: bool,
    core: Mutex<Core>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    stopped: AtomicBool,
}

#[derive(Clone)]
pub struct Updater {
    inner: Arc<Inner>,
}

fn sha512_of(path: &Path) -> std::io::Result<[u8; 64]> {
    sha512_of_reader(&mut std::fs::File::open(path)?)
}

fn sha512_of_reader(file: &mut impl std::io::Read) -> std::io::Result<[u8; 64]> {
    let mut hash = Sha512::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(buffer.get(..read).unwrap_or_default());
    }
    Ok(hash.finalize().into())
}

/// Opens the installer so nobody can change it while it is checked and started: shared for reading only, so any other
/// open for writing, deleting or renaming (a swap) is refused by Windows for as long as the returned handle lives. Checks
/// its SHA-512 through that handle. Hold it until the installer has been started.
fn open_for_launch(path: &Path, expected: &[u8; 64]) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    /// FILE_SHARE_READ: others may read it (the loader must, to start it), nothing else.
    const FILE_SHARE_READ: u32 = 1;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)?;
    if &sha512_of_reader(&mut file)? != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the installer does not match its checksum",
        ));
    }
    Ok(file)
}

impl Updater {
    /// `handle` runs the timers, the checks and the bounded wait of Restart to update.
    pub fn new(config: UpdaterConfig, deps: UpdaterDeps, handle: Handle) -> Updater {
        let active = config.feed.is_some();
        let core = Core {
            state: if active {
                UpdateState::Idle
            } else {
                UpdateState::Unsupported
            },
            restart_requested: false,
            auto_install: active,
            ready: None,
            checking: false,
            launched: false,
            listeners: Vec::new(),
            next_listener: 0,
        };
        Updater {
            inner: Arc::new(Inner {
                config,
                deps,
                handle,
                active,
                core: Mutex::new(core),
                tasks: Mutex::new(Vec::new()),
                stopped: AtomicBool::new(false),
            }),
        }
    }

    fn core(&self) -> std::sync::MutexGuard<'_, Core> {
        self.inner
            .core
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The first check after 60 s, then every 6 h. Nothing for a build with no feed.
    pub fn start(&self) {
        if !self.inner.active || self.inner.stopped.load(Ordering::SeqCst) {
            return;
        }
        let first = {
            let updater = self.clone();
            self.inner.handle.spawn(async move {
                tokio::time::sleep(FIRST_CHECK).await;
                updater.check().await;
            })
        };
        let repeat = {
            let updater = self.clone();
            self.inner.handle.spawn(async move {
                let mut every = tokio::time::interval_at(
                    tokio::time::Instant::now() + CHECK_EVERY,
                    CHECK_EVERY,
                );
                loop {
                    every.tick().await;
                    updater.check().await;
                }
            })
        };
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tasks.push(first);
        tasks.push(repeat);
    }

    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        for task in self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }

    pub fn state(&self) -> UpdateState {
        self.core().state.clone()
    }

    /// Whether quitting now would install an update: it was asked for, or one is ready and installs on quit.
    pub fn installation_pending(&self) -> bool {
        let core = self.core();
        core.restart_requested
            || (matches!(core.state, UpdateState::Ready { .. }) && core.auto_install)
    }

    /// Calls `listener` with every new state. The returned function stops it.
    pub fn on_state(
        &self,
        listener: impl Fn(&UpdateState) + Send + Sync + 'static,
    ) -> impl FnOnce() {
        let id = {
            let mut core = self.core();
            core.next_listener += 1;
            let id = core.next_listener;
            core.listeners.push((id, Arc::new(listener)));
            id
        };
        let updater = self.clone();
        move || updater.core().listeners.retain(|(other, _)| *other != id)
    }

    /// A ready update is never replaced by a later "checking", "none" or error; only `forced` changes can.
    fn change(&self, state: UpdateState, forced: bool) {
        let listeners: Vec<Listener> = {
            let mut core = self.core();
            if !forced
                && matches!(core.state, UpdateState::Ready { .. })
                && !matches!(state, UpdateState::Ready { .. })
            {
                return;
            }
            core.state = state.clone();
            core.listeners
                .iter()
                .map(|(_, listener)| Arc::clone(listener))
                .collect()
        };
        for listener in listeners {
            listener(&state);
        }
    }

    fn fail(&self, message: &str) {
        self.change(
            UpdateState::Error {
                message: message.into(),
            },
            false,
        );
    }

    /// Asks the feed whether there is a newer version, and downloads and checks it if so. Never panics and never
    /// returns an error: what went wrong becomes the state.
    pub async fn check(&self) {
        let Some(feed) = self.inner.config.feed.clone() else {
            return;
        };
        {
            let mut core = self.core();
            if core.checking || core.launched {
                return;
            }
            core.checking = true;
        }
        let outcome = self.check_feed(&feed).await;
        self.core().checking = false;
        if let Err(message) = outcome {
            self.fail(&message);
        }
    }

    async fn check_feed(&self, feed: &Url) -> Result<(), String> {
        self.change(UpdateState::Checking, false);
        let policy = Policy::for_feed(feed)?;
        policy.check(feed)?;
        let text = self.inner.deps.http.get_text(feed, MAX_FEED_BYTES).await?;
        let release = feed::parse(&text)?;
        let newest_known = self.core().ready.as_ref().map_or_else(
            || self.inner.config.current.clone(),
            |ready| ready.version.clone(),
        );
        if release.version <= newest_known {
            self.change(UpdateState::None, false);
            return Ok(());
        }
        let installer_url = feed
            .join(&release.file_name)
            .map_err(|_| "The release feed names an installer that has no address.".to_string())?;
        policy.check(&installer_url)?;

        let folder = self
            .inner
            .config
            .download_root
            .join(format!("tjommie-update-{}", release.version));
        let file = folder.join(&release.file_name);
        let partial = folder.join(format!("{}.partial", release.file_name));
        std::fs::create_dir_all(&folder)
            .map_err(|_| "The update folder could not be made.".to_string())?;
        let _ = std::fs::remove_file(&partial);

        // A copy from an earlier run that is still whole needs no second download.
        let already = sha512_of(&file).is_ok_and(|sha| sha == release.sha512);
        if !already {
            self.download(&installer_url, &partial, &release).await?;
            let _ = std::fs::remove_file(&file);
            let verified = Self::checksum_matches(&partial, &release);
            if !verified {
                let _ = std::fs::remove_file(&partial);
                return Err(BAD_CHECKSUM.into());
            }
            std::fs::rename(&partial, &file)
                .map_err(|_| "The update could not be saved.".to_string())?;
        }
        match self.inner.deps.inspector.app_id(&file) {
            Ok(id) if id == self.inner.config.app_id => {}
            _ => {
                let _ = std::fs::remove_file(&file);
                return Err(FOREIGN_APP.into());
            }
        }
        self.core().ready = Some(Ready {
            path: file,
            sha512: release.sha512,
            version: release.version.clone(),
        });
        self.change(
            UpdateState::Ready {
                version: release.version.to_string(),
            },
            false,
        );
        Ok(())
    }

    fn checksum_matches(path: &Path, release: &Release) -> bool {
        let size_ok = release
            .size
            .is_none_or(|size| std::fs::metadata(path).is_ok_and(|meta| meta.len() == size));
        size_ok && sha512_of(path).is_ok_and(|sha| sha == release.sha512)
    }

    async fn download(&self, url: &Url, partial: &Path, release: &Release) -> Result<(), String> {
        self.change(UpdateState::Downloading { percent: 0.0 }, false);
        let last = std::sync::atomic::AtomicU64::new(0);
        let updater = self.clone();
        let report = move |received: u64, total: Option<u64>| {
            let Some(total) = total.filter(|total| *total > 0) else {
                return;
            };
            let percent = received.saturating_mul(100) / total;
            // One notice per whole percent, not one per network chunk.
            if last.swap(percent, Ordering::Relaxed) != percent {
                updater.change(
                    UpdateState::Downloading {
                        percent: percent as f64,
                    },
                    false,
                );
            }
        };
        let limit = release.size.unwrap_or(MAX_INSTALLER_BYTES);
        let result = self
            .inner
            .deps
            .http
            .download(url, partial, limit, &report)
            .await;
        if result.is_err() {
            let _ = std::fs::remove_file(partial);
        }
        result
    }

    /// Restart to update: the user chose it from the tray or the settings. Waits for `before_restart` (at most
    /// `BEFORE_RESTART`), then quits once; the installer starts from the quit path.
    pub fn restart(&self) {
        {
            let mut core = self.core();
            if !matches!(core.state, UpdateState::Ready { .. }) || core.restart_requested {
                return;
            }
            core.restart_requested = true;
            core.auto_install = true;
        }
        let quit = Arc::clone(&self.inner.deps.quit);
        let Some(before) = self.inner.deps.before_restart.clone() else {
            quit();
            return;
        };
        // Whichever comes first, the hook finishing (or failing) or the time running out, quits; only this one task
        // quits, so a hook that answers too late can't quit a second time.
        self.inner.handle.spawn(async move {
            let _ = tokio::time::timeout(BEFORE_RESTART, async {
                let _ = before().await;
            })
            .await;
            quit();
        });
    }

    /// The app is quitting and the daemon has stopped: starts the installer if one is pending. `Ok(true)` when it was
    /// started; `Ok(false)` when nothing is pending or it already was. A file that no longer matches its checksum is
    /// not run.
    pub fn install_on_quit(&self) -> Result<bool, String> {
        let (ready, force_run) = {
            let mut core = self.core();
            let pending = core.restart_requested
                || (matches!(core.state, UpdateState::Ready { .. }) && core.auto_install);
            let Some(ready) = core.ready.clone().filter(|_| pending && !core.launched) else {
                return Ok(false);
            };
            let force_run = core.restart_requested;
            core.restart_requested = false;
            (ready, force_run)
        };
        let Ok(held) = open_for_launch(&ready.path, &ready.sha512) else {
            let _ = std::fs::remove_file(&ready.path);
            self.core().auto_install = false;
            self.change(
                UpdateState::Error {
                    message: CHANGED_ON_DISK.into(),
                },
                true,
            );
            return Err(CHANGED_ON_DISK.into());
        };
        // electron-updater's arguments: `--updated` is how the new app knows to start in the tray, and `--force-run`
        // makes the silent installer start it again when the user chose Restart to update.
        let mut args = vec!["--updated", "/S"];
        if force_run {
            args.push("--force-run");
        }
        let launched = self.inner.deps.launcher.launch(&ready.path, &args);
        drop(held); // only now: the installer has started, and may replace its own files
        launched?;
        self.core().launched = true;
        Ok(true)
    }

    /// The daemon wouldn't stop, so the update must not install: the app stays, and says so.
    pub fn shutdown_failed(&self) {
        {
            let mut core = self.core();
            core.restart_requested = false;
            core.auto_install = false;
        }
        self.change(
            UpdateState::Error {
                message: SHUTDOWN_FAILURE.into(),
            },
            true,
        );
    }
}

#[cfg(test)]
mod tests;
