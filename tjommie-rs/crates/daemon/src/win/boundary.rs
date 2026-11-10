//! The owner-side worker boundary (`WindowsWorkerBoundary`, `windows-worker.ts:72-412`). It decides when a provider
//! or review CLI may be launched as its worker account and keeps both identities safe:
//! - one preparation at a time across the paired identities (`WorkerPreparation`)
//! - a tree whose stop can't be confirmed poisons both identities for good
//! - a review runs inside one exclusive scope, observed before and after
//!
//! The runner uses it as a `CliHost`: `launch` goes through the isolated identity and returns a `CliChild` whose exit
//! fires only after the worker's job is confirmed stopped. Native calls go through `WorkerNative`, so tests can
//! stand in for Windows the way the TypeScript tests stand in for the helper process.

use crate::cli::{BoxFuture, ChildExit, CliChild, CliCommand, CliExit, CliHost, LaunchOptions};
use crate::win::acl::{below, same_path};
use crate::win::cred;
use crate::win::logon::{self, WorkerProcess};
use crate::win::policy_observer::WORKER_POLICY_OBSERVER;
use crate::win::worker::{
    BOOTSTRAP, FAILURE, Purpose, WorkerConfiguration, command_line, grant_target,
    launch_environment, launch_frame, ready, required, validate_pair,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::watch;

pub const UNAVAILABLE: &str = "Logins needs the verified Windows worker account. Set up and verify provider isolation on the host before saving or filling logins.";
const TERMINATION: &str = "The Windows worker launch could not confirm process-tree termination. Keep its account and working folders reserved.";
const BUSY: &str = "Other worker jobs are running. Review can start after they finish.";

/// The TypeScript errors, by class and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryError {
    /// `failure()`: the identity can't be used now.
    Unavailable,
    /// `WorkerLaunchTerminationError`: a tree's stop could not be confirmed; both identities stay unusable.
    Termination,
    /// `WorkerPolicyBusyError`
    Busy,
    /// `review_isolated_identity_unavailable`
    ReviewIdentityUnavailable,
    /// `review_worker_policy_changed`
    PolicyChanged,
    /// Invalid launch input (`Invalid Windows worker launch input.` or the launch limit).
    Input(String),
    /// A review action's own failure, passed through unchanged.
    Other(String),
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => FAILURE,
            Self::Termination => TERMINATION,
            Self::Busy => BUSY,
            Self::ReviewIdentityUnavailable => "review_isolated_identity_unavailable",
            Self::PolicyChanged => "review_worker_policy_changed",
            Self::Input(message) | Self::Other(message) => message,
        })
    }
}

impl std::error::Error for BoundaryError {}

/// `WorkerPreparation`: one preparation at a time, shared by the paired identities. Poisoning rejects every waiter at
/// once, without waiting for a blocked preparation, and every later one.
pub struct Preparation {
    queue: tokio::sync::Mutex<()>,
    poisoned: watch::Sender<bool>,
    retirements: AtomicU64,
}

impl Preparation {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: tokio::sync::Mutex::new(()),
            poisoned: watch::channel(false).0,
            retirements: AtomicU64::new(0),
        })
    }

    pub fn failed(&self) -> bool {
        *self.poisoned.borrow()
    }

    pub fn poison(&self) {
        self.poisoned.send_replace(true);
    }

    /// How many worker trees were confirmed stopped so far.
    pub fn retirements(&self) -> u64 {
        self.retirements.load(Ordering::SeqCst)
    }

    pub async fn run<T>(
        &self,
        action: impl Future<Output = Result<T, BoundaryError>>,
    ) -> Result<T, BoundaryError> {
        if self.failed() {
            return Err(BoundaryError::Termination);
        }
        let mut poisoned = self.poisoned.subscribe();
        let _turn = tokio::select! {
            turn = self.queue.lock() => turn,
            _ = poisoned.wait_for(|poisoned| *poisoned) => return Err(BoundaryError::Termination),
        };
        if self.failed() {
            return Err(BoundaryError::Termination);
        }
        action.await
    }
}

/// A worker job this daemon holds (the TypeScript receipt). Known jobs are exempt from the untracked-provider scan.
#[derive(Clone)]
pub struct Receipt {
    pub purpose: Purpose,
    pub worker_sid: String,
    pub worker_pid: u32,
    /// The native process and job, when Windows made them.
    pub process: Option<Arc<WorkerProcess>>,
}

/// Why a native launch failed, and whether anything it started was confirmed gone.
#[derive(Debug)]
pub struct LaunchFailure {
    pub message: String,
    pub stopped: bool,
}

/// A worker running after `resume`.
pub struct Running {
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Box<dyn AsyncRead + Send + Unpin>,
    /// Resolves with the exit code when the worker process exits.
    pub exited: BoxFuture<'static, Option<i32>>,
    /// Stops the job and confirms nothing is left; safe to call more than once.
    pub retire: Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>,
}

/// A worker created suspended inside its job; nothing of it runs until `resume`.
pub trait NativeLaunch: Send {
    fn receipt(&self) -> Receipt;
    /// Writes the private launch frame to stdin, lets the worker run and hands over its pipes.
    fn resume(
        self: Box<Self>,
        frame: Vec<u8>,
    ) -> BoxFuture<'static, Result<Running, LaunchFailure>>;
    /// Kills a launch that never resumed; `true` when its tree is confirmed gone.
    fn abandon(self: Box<Self>) -> BoxFuture<'static, bool>;
}

/// The Windows calls the boundary makes. `WindowsNative` is the real one.
pub trait WorkerNative: Send + Sync + 'static {
    fn verify<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        known: &'a [Receipt],
    ) -> BoxFuture<'a, Result<(), String>>;
    fn read_credential<'a>(
        &'a self,
        target: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>>;
    fn grant_directory<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        path: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn launch<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        password: String,
        cwd: &'a Path,
        environment: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Box<dyn NativeLaunch>, LaunchFailure>>;
}

/// What a review saw of the machine's provider policy (`WorkerPolicyReceipt`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyReceipt {
    pub worker_sid: String,
    pub provider_sid: String,
    pub configuration_sha256: String,
    pub observer_sha256: String,
    pub policy_sha256: String,
}

pub type Observer =
    Arc<dyn Fn() -> BoxFuture<'static, Result<PolicyReceipt, BoundaryError>> + Send + Sync>;

pub struct BoundaryOptions {
    pub configuration_path: PathBuf,
    pub purpose: Purpose,
    /// The daemon's own folder, which a worker is never granted (the TypeScript helper's folder).
    pub daemon_dir: PathBuf,
    /// `workerCliExclusive`: no legacy launch has happened and this boundary is the installed one.
    pub exclusive: Arc<dyn Fn() -> bool + Send + Sync>,
    pub native: Arc<dyn WorkerNative>,
    /// Replaces the policy observation, as the TypeScript tests spy on `observeReviewPolicy`. `None` in the daemon.
    pub observer: Option<Observer>,
}

#[derive(Default)]
struct State {
    generation: u64,
    leases: usize,
    closed: bool,
    children: BTreeMap<u32, Receipt>,
    launches: usize,
    termination_unknown: bool,
    review_lease: Option<String>,
}

struct Inner {
    options: BoundaryOptions,
    state: Mutex<State>,
    preparation: Mutex<Arc<Preparation>>,
    peer: Mutex<Option<Weak<Inner>>>,
}

tokio::task_local! {
    /// The review lease a task runs under (the TypeScript `AsyncLocalStorage` review scope).
    static REVIEW_SCOPE: String;
}

/// One worker identity's boundary; clones share it.
#[derive(Clone)]
pub struct WorkerBoundary(Arc<Inner>);

/// `acquire()`'s lease: valid until released, the boundary changes generation, or either identity is poisoned.
pub struct Lease {
    boundary: WorkerBoundary,
    generation: u64,
    released: bool,
}

impl Lease {
    pub fn assert(&self) -> Result<(), BoundaryError> {
        let b = &self.boundary;
        let state = b.state();
        let valid = !self.released
            && !state.closed
            && !state.termination_unknown
            && !b.preparation().failed()
            && self.generation == state.generation
            && (b.0.options.exclusive)();
        if valid {
            Ok(())
        } else {
            Err(BoundaryError::Unavailable)
        }
    }

    pub fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.boundary.state().leases -= 1;
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.release();
    }
}

fn inside(root: &Path, path: &Path) -> bool {
    match (std::fs::canonicalize(root), std::fs::canonicalize(path)) {
        (Ok(root), Ok(path)) => same_path(&root, &path) || below(&root, &path).is_some(),
        _ => false,
    }
}

impl WorkerBoundary {
    pub fn new(options: BoundaryOptions) -> Self {
        Self(Arc::new(Inner {
            options,
            state: Mutex::default(),
            preparation: Mutex::new(Preparation::new()),
            peer: Mutex::new(None),
        }))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn preparation(&self) -> Arc<Preparation> {
        self.0
            .preparation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn purpose(&self) -> Purpose {
        self.0.options.purpose
    }

    pub fn configured(&self) -> bool {
        self.0.options.configuration_path.exists()
    }

    /// Configured, or still marked as needing protection.
    pub fn required(&self) -> bool {
        required(&self.0.options.configuration_path)
    }

    pub fn configuration(&self) -> Result<WorkerConfiguration, BoundaryError> {
        if self.state().closed {
            return Err(BoundaryError::Unavailable);
        }
        WorkerConfiguration::load(&self.0.options.configuration_path, self.purpose())
            .map_err(|_| BoundaryError::Unavailable)
    }

    /// `pairWith`: binds the provider and review identities and makes them share one preparation queue.
    pub fn pair_with(&self, peer: &WorkerBoundary) -> Result<(), BoundaryError> {
        let busy = |b: &WorkerBoundary| {
            let state = b.state();
            !state.children.is_empty() || state.launches > 0
        };
        let current = self
            .0
            .peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        if current.is_some_and(|current| !Arc::ptr_eq(&current, &peer.0))
            || busy(self)
            || busy(peer)
        {
            return Err(BoundaryError::Unavailable);
        }
        validate_pair(&self.configuration()?, &peer.configuration()?)
            .map_err(|_| BoundaryError::Unavailable)?;
        *self
            .0
            .preparation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = peer.preparation();
        *self
            .0
            .peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::downgrade(&peer.0));
        Ok(())
    }

    /// The jobs this identity holds.
    pub fn receipts(&self) -> Vec<Receipt> {
        self.state().children.values().cloned().collect()
    }

    fn known_jobs(&self) -> Vec<Receipt> {
        let mut jobs = self.receipts();
        let peer = self
            .0
            .peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(peer) = peer {
            jobs.extend(WorkerBoundary(peer).receipts());
        }
        jobs
    }

    /// Call before any legacy launch or boundary change: live leases fail closed.
    pub fn invalidate(&self) {
        self.state().generation += 1;
    }

    pub fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        state.generation += 1;
    }

    fn unknown_termination(&self) {
        {
            let mut state = self.state();
            state.termination_unknown = true;
            state.generation += 1;
        }
        self.preparation().poison();
    }

    fn assert_preparation(&self) -> Result<(), BoundaryError> {
        if self.state().termination_unknown || self.preparation().failed() {
            Err(BoundaryError::Termination)
        } else {
            Ok(())
        }
    }

    /// `unavailableReason`: `None` when this identity is ready to launch.
    pub async fn unavailable_reason(&self) -> Option<&'static str> {
        let checked = self
            .preparation()
            .run(async {
                if self.state().termination_unknown {
                    return Err(BoundaryError::Unavailable);
                }
                let config = ready(&self.0.options.configuration_path, self.purpose())
                    .map_err(|_| BoundaryError::Unavailable)?;
                if self.state().closed || !(self.0.options.exclusive)() {
                    return Err(BoundaryError::Unavailable);
                }
                self.verify_held(&config).await
            })
            .await;
        checked.err().map(|_| UNAVAILABLE)
    }

    pub async fn acquire(&self) -> Result<Lease, BoundaryError> {
        if self.unavailable_reason().await.is_some() {
            return Err(BoundaryError::Unavailable);
        }
        self.assert_preparation()?;
        let mut state = self.state();
        state.leases += 1;
        Ok(Lease {
            boundary: self.clone(),
            generation: state.generation,
            released: false,
        })
    }

    /// `grantDirectory`: an inherited Modify allow for the worker, only on a folder it may be granted.
    pub async fn grant_directory(&self, path: &Path) -> Result<(), BoundaryError> {
        let config = self.configuration()?;
        let target = grant_target(&config, &self.0.options.daemon_dir, path)
            .map_err(|_| BoundaryError::Unavailable)?;
        self.0
            .options
            .native
            .grant_directory(&config, &target)
            .await
            .map_err(|_| BoundaryError::Unavailable)
    }

    /// `#verifyHeld`: retried once, only when a known job was confirmed stopped while verifying.
    async fn verify_held(&self, config: &WorkerConfiguration) -> Result<(), BoundaryError> {
        let preparation = self.preparation();
        for attempt in 0..2 {
            self.assert_preparation()?;
            let (retirement, jobs) = (preparation.retirements(), self.known_jobs());
            match self.0.options.native.verify(config, &jobs).await {
                Ok(()) => {
                    self.assert_preparation()?;
                    return Ok(());
                }
                Err(_)
                    if attempt == 0
                        && !preparation.failed()
                        && retirement != preparation.retirements() => {}
                Err(_) => return Err(BoundaryError::Unavailable),
            }
        }
        Err(BoundaryError::Unavailable)
    }

    /// `spawn`: launches `file` with `args` as this identity's worker.
    pub async fn spawn(
        &self,
        file: &str,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
    ) -> Result<CliChild, BoundaryError> {
        let scope = REVIEW_SCOPE.try_with(Clone::clone).ok();
        {
            let state = self.state();
            if self.purpose() == Purpose::Review && state.review_lease.is_none() {
                return Err(BoundaryError::ReviewIdentityUnavailable);
            }
            if let Some(lease) = &state.review_lease
                && (scope.as_ref() != Some(lease)
                    || !state.children.is_empty()
                    || state.launches > 0)
            {
                return Err(BoundaryError::Busy);
            }
            if state.termination_unknown {
                return Err(BoundaryError::Termination);
            }
        }
        let frame = launch_frame(file, args).map_err(BoundaryError::Input)?;
        let environment = launch_environment(env).map_err(BoundaryError::Input)?;
        self.state().launches += 1;
        let preparation = self.preparation();
        let result = preparation
            .run(async {
                for attempt in 0..2 {
                    match self
                        .spawn_held(file, cwd, env, frame.clone(), environment.clone())
                        .await?
                    {
                        Some(child) => return Ok(child),
                        None if attempt == 0 && !preparation.failed() => {}
                        None => break,
                    }
                }
                Err(BoundaryError::Unavailable)
            })
            .await;
        self.state().launches -= 1;
        result
    }

    /// `#spawnHeld`. `Ok(None)` means the native launch failed while a known job retired, so it may be retried once.
    async fn spawn_held(
        &self,
        file: &str,
        cwd: &Path,
        env: &[(String, String)],
        frame: Vec<u8>,
        environment: Vec<(String, String)>,
    ) -> Result<Option<CliChild>, BoundaryError> {
        let unavailable = || BoundaryError::Unavailable;
        let config = self.configuration()?;
        let file_path = Path::new(file);
        if !(self.0.options.exclusive)()
            || !inside(&config.runtime_root, file_path)
            || !file.to_lowercase().ends_with(".exe")
        {
            return Err(unavailable());
        }
        let bootstrap = config.runtime_root.join("node.exe");
        let line = command_line(
            &bootstrap.to_string_lossy(),
            &["-e".into(), BOOTSTRAP.into()],
        )
        .map_err(|_| unavailable())?;
        if line.encode_utf16().count() + 1 > 1024 {
            return Err(unavailable());
        }
        let homes: Vec<&str> = env
            .iter()
            .filter(|(key, _)| key == "CODEX_HOME" || key == "CLAUDE_CONFIG_DIR")
            .map(|(_, value)| value.as_str())
            .filter(|v| !v.is_empty())
            .collect();
        if homes
            .iter()
            .any(|home| !inside(&config.worker_root, Path::new(home)))
        {
            return Err(unavailable());
        }
        self.verify_held(&config).await?;
        self.assert_preparation()?;
        self.grant_directory(cwd).await?;
        self.assert_preparation()?;
        for home in &homes {
            self.grant_directory(Path::new(home)).await?;
            self.assert_preparation()?;
        }
        if !inside(&config.worker_root, cwd) && same_path(cwd, &config.owner_data_root) {
            return Err(unavailable());
        }
        let saved = self
            .0
            .options
            .native
            .read_credential(&config.credential_target)
            .await
            .map_err(|_| unavailable())?;
        self.assert_preparation()?;
        let password = saved
            .and_then(|saved| serde_json::from_str::<serde_json::Value>(&saved).ok())
            .filter(|value| value["username"] == config.worker_name.as_str())
            .and_then(|value| {
                value["password"]
                    .as_str()
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
            })
            .ok_or_else(unavailable)?;
        let preparation = self.preparation();
        let launch_generation = preparation.retirements();
        self.assert_preparation()?;
        let launched = match self
            .0
            .options
            .native
            .launch(&config, password, cwd, environment)
            .await
        {
            Ok(launched) => launched,
            Err(failure) if !failure.stopped => {
                self.unknown_termination();
                return Err(BoundaryError::Termination);
            }
            // Poisoned while the launch was pending: TypeScript's receipt wait notices that first.
            Err(_) if self.assert_preparation().is_err() => return Err(BoundaryError::Termination),
            Err(_) if launch_generation != preparation.retirements() => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        let receipt = launched.receipt();
        if receipt.purpose != config.purpose
            || receipt.worker_sid != config.worker_sid
            || self.assert_preparation().is_err()
        {
            // Created but never resumed: settle it without letting it run.
            let poisoned = self.assert_preparation().is_err();
            if !launched.abandon().await {
                self.unknown_termination();
                return Err(BoundaryError::Termination);
            }
            return Err(if poisoned {
                BoundaryError::Termination
            } else {
                unavailable()
            });
        }
        let pid = receipt.worker_pid;
        self.state().children.insert(pid, receipt);
        let running = match launched.resume(frame).await {
            Ok(running) => running,
            Err(failure) => {
                self.state().children.remove(&pid);
                if !failure.stopped {
                    self.unknown_termination();
                    return Err(BoundaryError::Termination);
                }
                return Err(unavailable());
            }
        };
        let exit = ChildExit::new();
        let (retire, exited) = (running.retire.clone(), running.exited);
        let (watcher, fired) = (self.clone(), exit.clone());
        tokio::spawn(async move {
            let code = exited.await;
            if retire().await {
                watcher.state().children.remove(&pid);
                watcher
                    .preparation()
                    .retirements
                    .fetch_add(1, Ordering::SeqCst);
                fired.fire(CliExit::Exited { code, signal: None });
            } else {
                watcher.unknown_termination();
                fired.fire(CliExit::Failed(TERMINATION.into()));
            }
        });
        let (stop, waiting) = (running.retire.clone(), exit.clone());
        Ok(Some(CliChild {
            pid: Some(pid),
            stdin: running.stdin,
            stdout: running.stdout,
            stderr: running.stderr,
            exit,
            kill_tree: Arc::new(move || {
                let (stop, waiting) = (stop.clone(), waiting.clone());
                Box::pin(async move {
                    stop().await;
                    match waiting.wait().await {
                        CliExit::Failed(message) => Err(message),
                        CliExit::Exited { .. } => Ok(()),
                    }
                })
            }),
        }))
    }

    /// `observeReviewPolicy`: runs the fixed observer as the review worker and hashes what it saw.
    pub async fn observe_review_policy(&self) -> Result<PolicyReceipt, BoundaryError> {
        if let Some(observer) = &self.0.options.observer {
            return observer().await;
        }
        {
            let state = self.state();
            let scope = REVIEW_SCOPE.try_with(Clone::clone).ok();
            if state.review_lease.is_none()
                || scope != state.review_lease
                || !state.children.is_empty()
                || state.launches > 0
            {
                return Err(BoundaryError::Busy);
            }
        }
        let config = self.configuration()?;
        let scratch = config.worker_root.join("policy-observer");
        std::fs::create_dir_all(&scratch).map_err(|_| BoundaryError::Unavailable)?;
        let node = config.runtime_root.join("node.exe");
        let mut child = self
            .spawn(
                &node.to_string_lossy(),
                &["-e".into(), WORKER_POLICY_OBSERVER.into()],
                &scratch,
                &[],
            )
            .await?;
        drop(child.stdin);
        let mut output = Vec::new();
        let read = async {
            let mut chunk = [0u8; 1024];
            loop {
                let count = child
                    .stdout
                    .read(&mut chunk)
                    .await
                    .map_err(|_| BoundaryError::Unavailable)?;
                if count == 0 {
                    return Ok(());
                }
                if output.len() + count > 4096 {
                    return Err(BoundaryError::Unavailable);
                }
                output.extend_from_slice(&chunk[..count]);
            }
        };
        let drain = async {
            let _ = tokio::io::copy(&mut child.stderr, &mut tokio::io::sink()).await;
        };
        let finished = tokio::time::timeout(Duration::from_secs(20), async {
            let (read, ()) = tokio::join!(read, drain);
            read?;
            Ok::<_, BoundaryError>(child.exit.wait().await)
        })
        .await;
        let exit = match finished {
            Ok(Ok(exit)) => exit,
            Ok(Err(error)) => {
                let _ = (child.kill_tree)().await;
                return Err(error);
            }
            Err(_) => {
                let _ = (child.kill_tree)().await;
                return Err(BoundaryError::Termination);
            }
        };
        let code = match exit {
            CliExit::Failed(_) => return Err(BoundaryError::Termination),
            CliExit::Exited { code, .. } => code,
        };
        let value: ObservedPolicy =
            serde_json::from_slice(&output).map_err(|_| BoundaryError::Unavailable)?;
        let names = ["managed-settings.json", "managed-mcp.json"];
        let valid = value.version == 1
            && value.registry_absent
            && value.files.iter().all(|(name, hash)| {
                names.contains(&name.as_str())
                    && hash.as_ref().is_none_or(|h| {
                        h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                    })
            });
        if !valid || code != Some(0) {
            return Err(BoundaryError::Unavailable);
        }
        let hex = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        Ok(PolicyReceipt {
            worker_sid: config.worker_sid.clone(),
            provider_sid: config.peer_sid.clone(),
            configuration_sha256: config.hash(),
            observer_sha256: hex(WORKER_POLICY_OBSERVER.as_bytes()),
            policy_sha256: hex(serde_json::to_string(&value)
                .expect("plain data")
                .as_bytes()),
        })
    }

    /// `withReviewPolicy`: runs `action` as the only review, between two matching policy observations. If a tree's
    /// stop can't be confirmed, the lease is kept, so nothing else runs as either identity.
    pub async fn with_review_policy<T, F>(
        &self,
        action: impl FnOnce(PolicyReceipt) -> F,
    ) -> Result<T, BoundaryError>
    where
        F: Future<Output = Result<T, BoundaryError>>,
    {
        if self.purpose() != Purpose::Review {
            return Err(BoundaryError::ReviewIdentityUnavailable);
        }
        let lease = {
            let mut state = self.state();
            if state.review_lease.is_some() || !state.children.is_empty() || state.launches > 0 {
                return Err(BoundaryError::Busy);
            }
            let lease = logon::random_uuid().map_err(|_| BoundaryError::Unavailable)?;
            state.review_lease = Some(lease.clone());
            lease
        };
        let result = REVIEW_SCOPE
            .scope(lease.clone(), async {
                let before = self.observe_review_policy().await?;
                let result = action(before.clone()).await?;
                let after = self.observe_review_policy().await?;
                if before != after {
                    return Err(BoundaryError::PolicyChanged);
                }
                Ok(result)
            })
            .await;
        let terminated = matches!(result, Err(BoundaryError::Termination));
        if terminated {
            self.unknown_termination();
        }
        let busy = {
            let state = self.state();
            !state.children.is_empty() || state.launches > 0
        };
        if busy {
            self.unknown_termination();
        }
        let mut state = self.state();
        if state.termination_unknown {
            if !terminated {
                return Err(BoundaryError::Termination);
            }
        } else if state.review_lease.as_ref() == Some(&lease) {
            state.review_lease = None;
        }
        result
    }
}

/// The observer's report (`windows-worker.ts:382`), in that field order so its hash matches.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservedPolicy {
    version: u8,
    registry_absent: bool,
    files: [(String, Option<String>); 2],
}

impl CliHost for WorkerBoundary {
    /// `launchCli` with a worker installed: the private Claude key travels in the environment, as in TypeScript.
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, std::io::Result<CliChild>> {
        Box::pin(async move {
            let mut all = cmd.args.clone();
            all.extend(args);
            let mut env = options.env;
            if let Some(key) = options.claude_api_key {
                env.insert("ANTHROPIC_API_KEY".into(), key.0);
                env.insert("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB".into(), "1".into());
                env.insert("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST".into(), "1".into());
            }
            let env: Vec<(String, String)> = env.into_iter().collect();
            self.spawn(&cmd.file, &all, Path::new(&options.cwd), &env)
                .await
                .map_err(std::io::Error::other)
        })
    }
}

/// The real Windows calls, each on a blocking thread.
pub struct WindowsNative;

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}

impl WorkerNative for WindowsNative {
    fn verify<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        known: &'a [Receipt],
    ) -> BoxFuture<'a, Result<(), String>> {
        let (config, known) = (config.clone(), known.to_vec());
        Box::pin(async move {
            blocking(move || {
                let jobs: Vec<&logon::Job> = known
                    .iter()
                    .filter_map(|receipt| receipt.process.as_ref().map(|p| &p.job))
                    .collect();
                logon::verify(&config, &jobs)
            })
            .await
            .unwrap_or_else(|| Err(FAILURE.into()))
        })
    }

    fn read_credential<'a>(
        &'a self,
        target: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        let target = target.to_owned();
        Box::pin(async move {
            blocking(move || cred::read(&target).map_err(|e| e.to_string()))
                .await
                .unwrap_or_else(|| Err(FAILURE.into()))
        })
    }

    fn grant_directory<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        path: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        let (path, sid, peer) = (
            path.to_path_buf(),
            config.worker_sid.clone(),
            config.peer_sid.clone(),
        );
        Box::pin(async move {
            blocking(move || logon::grant_directory(&path, &sid, &peer))
                .await
                .unwrap_or_else(|| Err(FAILURE.into()))
        })
    }

    fn launch<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        password: String,
        cwd: &'a Path,
        environment: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Box<dyn NativeLaunch>, LaunchFailure>> {
        let (config, cwd) = (config.clone(), cwd.to_path_buf());
        Box::pin(async move {
            let made = blocking(move || {
                let mut password = password;
                let made = logon::launch(&config, &password, &cwd, &environment);
                // SAFETY: zeros keep the string valid UTF-8.
                unsafe { password.as_bytes_mut().fill(0) };
                made.map(|mut process| {
                    let pipes = (
                        process.worker.stdin(),
                        process.worker.stdout(),
                        process.worker.stderr(),
                    );
                    (
                        config.purpose,
                        config.worker_sid.clone(),
                        Arc::new(process),
                        pipes,
                    )
                })
            })
            .await
            .unwrap_or_else(|| Err((FAILURE.into(), false)));
            match made {
                Ok((purpose, worker_sid, process, (stdin, stdout, stderr))) => {
                    Ok(Box::new(WindowsLaunch {
                        purpose,
                        worker_sid,
                        process,
                        stdin,
                        stdout,
                        stderr,
                    }) as Box<dyn NativeLaunch>)
                }
                Err((message, stopped)) => Err(LaunchFailure { message, stopped }),
            }
        })
    }
}

struct WindowsLaunch {
    purpose: Purpose,
    worker_sid: String,
    process: Arc<WorkerProcess>,
    stdin: Option<std::fs::File>,
    stdout: Option<std::fs::File>,
    stderr: Option<std::fs::File>,
}

impl NativeLaunch for WindowsLaunch {
    fn receipt(&self) -> Receipt {
        Receipt {
            purpose: self.purpose,
            worker_sid: self.worker_sid.clone(),
            worker_pid: self.process.worker.pid(),
            process: Some(self.process.clone()),
        }
    }

    fn resume(
        self: Box<Self>,
        frame: Vec<u8>,
    ) -> BoxFuture<'static, Result<Running, LaunchFailure>> {
        Box::pin(async move {
            let WindowsLaunch {
                process,
                stdin,
                stdout,
                stderr,
                ..
            } = *self;
            let (Some(mut stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
                return Err(LaunchFailure {
                    message: FAILURE.into(),
                    stopped: process.retire(),
                });
            };
            let started = process.clone();
            let resumed = blocking(move || {
                let written = stdin.write_all(&frame).map_err(|e| e.to_string());
                written
                    .and_then(|()| started.worker.resume())
                    .map(|()| stdin)
            })
            .await
            .unwrap_or_else(|| Err(FAILURE.into()));
            let stdin = match resumed {
                Ok(stdin) => stdin,
                Err(message) => {
                    let stopper = process.clone();
                    let stopped = blocking(move || stopper.retire()).await.unwrap_or(false);
                    return Err(LaunchFailure { message, stopped });
                }
            };
            let (waiter, stopper) = (process.clone(), process);
            Ok(Running {
                stdin: Box::new(tokio::fs::File::from_std(stdin)),
                stdout: Box::new(tokio::fs::File::from_std(stdout)),
                stderr: Box::new(tokio::fs::File::from_std(stderr)),
                exited: Box::pin(async move {
                    blocking(move || waiter.worker.wait().ok())
                        .await
                        .flatten()
                        .map(|code| code as i32)
                }),
                retire: Arc::new(move || {
                    let stopper = stopper.clone();
                    Box::pin(
                        async move { blocking(move || stopper.retire()).await.unwrap_or(false) },
                    )
                }),
            })
        })
    }

    fn abandon(self: Box<Self>) -> BoxFuture<'static, bool> {
        let process = self.process;
        Box::pin(async move {
            blocking(move || {
                let killed = process.worker.kill().is_ok() && process.worker.wait().is_ok();
                process.retire() && killed
            })
            .await
            .unwrap_or(false)
        })
    }
}
