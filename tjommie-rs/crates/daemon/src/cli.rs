//! `packages/daemon/src/cli.ts`: finding, launching and stopping the provider CLIs. Every process starts through
//! `spawn::command()`, so none opens a console window, and every CLI gets a scrubbed environment: it must use the
//! login in its Tjommie account folder, never keys or config folders the daemon happens to inherit.
//!
//! `openConsole` is not ported: nothing calls it, and the owner's rule is that sign-in never opens a console.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::Shared;
use protocol::model::Provider;
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::watch;

use crate::providers::codex_rpc::{self, CodexServer};
use crate::providers::{ApiKey, ProviderError};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A process environment. Keys keep their case, as on `process.env`.
pub type Env = BTreeMap<String, String>;

/// An npm-installed Codex whose native binary runs directly (`codexPackage` in TypeScript).
#[derive(Debug, Clone, PartialEq)]
pub struct CodexPackage {
    pub root: String,
    pub path_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CliCommand {
    pub file: String,
    pub args: Vec<String>,
    pub codex_package: Option<CodexPackage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchOptions {
    pub cwd: String,
    pub env: Env,
    /// Only this explicit private capability restores a key after inherited-environment scrubbing.
    pub claude_api_key: Option<ApiKey>,
}

/// How a child ended: Node's `exit` event, or its `error` event (it failed to start or was lost).
#[derive(Debug, Clone, PartialEq)]
pub enum CliExit {
    Exited {
        code: Option<i32>,
        signal: Option<String>,
    },
    Failed(String),
}

/// Stops the child's whole process tree and waits for it (`killTreeAndWait(child.pid)`). Claude's fire-and-forget
/// `killTree` is the same call, unawaited.
pub type KillTree = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// A worker's proof that a child's whole process tree has gone (`workerProcessRetired`). An error means the tree may
/// still run.
pub type Retirement = Shared<BoxFuture<'static, Result<(), String>>>;

type ExitListener = Box<dyn FnOnce(&CliExit) + Send>;

struct ExitState {
    status: watch::Sender<Option<CliExit>>,
    listeners: Mutex<Vec<ExitListener>>,
}

/// A child's exit, like Node's `exit` event plus `exitCode`: listeners run synchronously when it fires.
#[derive(Clone)]
pub struct ChildExit(Arc<ExitState>);

impl Default for ChildExit {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildExit {
    pub fn new() -> Self {
        ChildExit(Arc::new(ExitState {
            status: watch::channel(None).0,
            listeners: Mutex::new(Vec::new()),
        }))
    }

    /// Records the exit and runs the listeners. Only the first call counts.
    pub fn fire(&self, exit: CliExit) {
        let first = self
            .0
            .status
            .send_if_modified(|status| status.is_none() && status.replace(exit.clone()).is_none());
        if first {
            let listeners =
                std::mem::take(&mut *self.0.listeners.lock().expect("exit listeners lock"));
            listeners.into_iter().for_each(|listener| listener(&exit));
        }
    }

    /// Runs `listener` when the child exits, or now if it already has.
    pub fn on_exit(&self, listener: impl FnOnce(&CliExit) + Send + 'static) {
        let mut listeners = self.0.listeners.lock().expect("exit listeners lock");
        match self.status() {
            Some(exit) => {
                drop(listeners);
                listener(&exit);
            }
            None => listeners.push(Box::new(listener)),
        }
    }

    pub fn status(&self) -> Option<CliExit> {
        self.0.status.borrow().clone()
    }

    pub async fn wait(&self) -> CliExit {
        let mut status = self.0.status.subscribe();
        let exit = status
            .wait_for(Option::is_some)
            .await
            .expect("the sender lives in self");
        exit.clone().expect("waited for an exit")
    }
}

/// A launched CLI with piped stdio.
pub struct CliChild {
    pub pid: Option<u32>,
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Box<dyn AsyncRead + Send + Unpin>,
    pub exit: ChildExit,
    pub kill_tree: KillTree,
    /// Set for a child inside a worker boundary, whose exit alone doesn't prove its tree has gone.
    pub retirement: Option<Retirement>,
}

pub trait CliHost: Send + Sync {
    /// `launchCli`: starts `cmd` with `args` appended, in `options.cwd` with exactly `options.env`.
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>>;

    /// `startCodexServer`. Tests replace this to drive a Codex session's RPC directly, as the TypeScript tests mock it.
    fn start_codex_server<'a>(
        &'a self,
        cmd: &'a CliCommand,
        env: &'a Env,
        cwd: &'a str,
        ephemeral_auth: bool,
    ) -> BoxFuture<'a, Result<CodexServer, ProviderError>> {
        Box::pin(codex_rpc::start_codex_server(
            self,
            cmd,
            env,
            cwd,
            ephemeral_auth,
        ))
    }

    /// `prepareProviderDirectories`: lets the worker boundary reach these folders. Nothing to do without one.
    fn prepare_provider_directories(
        &self,
        _folders: Vec<PathBuf>,
    ) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    // The rest of `cli.ts` that setup uses. The defaults run nothing: a fake that doesn't override one sees "not
    // installed", never a real process.

    /// `resolveCli(name)` on this host.
    fn find_cli(&self, _name: Provider) -> Option<CliCommand> {
        None
    }

    /// `runCli`.
    fn run_cli<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        _args: &'a [String],
        _env: &'a Env,
        _timeout: Duration,
    ) -> BoxFuture<'a, io::Result<CliOutput>> {
        Box::pin(async {
            Ok(CliOutput {
                code: None,
                stdout: String::new(),
                stderr: "This host runs no CLI.".into(),
            })
        })
    }

    /// `openUrl`.
    fn open_url(&self, _url: &str) -> Result<(), String> {
        Err("This host opens no browser.".into())
    }

    /// `providerAccountFolder`.
    fn provider_account_folder(&self, folder: &str) -> String {
        folder.to_string()
    }

    /// `workerAccountsRoot`.
    fn worker_accounts_root(&self) -> Option<PathBuf> {
        None
    }

    /// `cliEnv(extra, base)`, mapping account folders into the worker.
    fn cli_env_from(&self, extra: &Env, base: &Env) -> Env {
        cli_env(extra, base)
    }
}

/// What a short command run printed (`runCli`).
#[derive(Debug, Clone, PartialEq)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// `WorkerLaunchTerminationError`: a worker launch whose process tree may still run. It is never an ordinary failure:
/// the account and its folders stay reserved.
#[derive(Debug)]
pub struct WorkerTermination;

impl fmt::Display for WorkerTermination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "The Windows worker launch could not confirm process-tree termination. Keep its account and working folders reserved.",
        )
    }
}

impl std::error::Error for WorkerTermination {}

pub fn is_worker_termination(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<WorkerTermination>())
}

/// The Windows worker that runs provider CLIs under their own account (spec §3.8). T2.3 implements it; `SystemCli`
/// routes launches, folder grants and CLI lookup through it when it is required.
pub trait WorkerBoundary: Send + Sync {
    fn required(&self) -> bool;
    fn purpose(&self) -> WorkerPurpose;
    /// `configuration()`'s `(workerSid, peerSid)`, or `None` when the configuration can't be read.
    fn sids(&self) -> Option<(String, String)>;
    /// `configuration().workerRoot`: account folders live under its `accounts` folder.
    fn worker_root(&self) -> PathBuf;
    /// `configuration().runtimeRoot`: the only place a worker's CLIs are found.
    fn runtime_root(&self) -> PathBuf;
    fn grant_directory<'a>(&'a self, folder: &'a Path) -> BoxFuture<'a, io::Result<()>>;
    /// Launches inside the worker. A `WorkerTermination` error means the launch may have left a running tree.
    fn spawn<'a>(
        &'a self,
        file: &'a str,
        args: Vec<String>,
        cwd: &'a str,
        env: Env,
    ) -> BoxFuture<'a, io::Result<CliChild>>;
}

/// A worker's half of the provider/review pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPurpose {
    Provider,
    Review,
}

type WorkerPair = (Arc<dyn WorkerBoundary>, Arc<dyn WorkerBoundary>);

/// `installWindowsWorker` and `workerCliExclusive` (`cli.ts:12-27`): the daemon's installed provider and review workers,
/// and whether any CLI has run outside them. One per daemon, shared with its `SystemCli`s (`reporting_to`) and its
/// boundaries' `exclusive` checks, instead of TypeScript's module globals.
#[derive(Default)]
pub struct WorkerInstall {
    installed: Mutex<Option<WorkerPair>>,
    legacy_used: AtomicBool,
}

/// An installed pair. Dropping it uninstalls the pair.
pub struct InstalledWorkers {
    install: Arc<WorkerInstall>,
    pair: WorkerPair,
}

impl WorkerInstall {
    fn installed(&self) -> std::sync::MutexGuard<'_, Option<WorkerPair>> {
        self.installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes this pair the daemon's CLI launch path: a provider and a review worker under two different accounts, each
    /// the other's peer. Pair the boundaries both ways first: pairing runs the rest of `validateWorkerPair` (owner,
    /// roots, names, credential targets, provisioning) and lets each see the other's receipts.
    pub fn install(
        self: &Arc<Self>,
        worker: Arc<dyn WorkerBoundary>,
        review: Arc<dyn WorkerBoundary>,
    ) -> Result<InstalledWorkers, String> {
        let mut installed = self.installed();
        if installed.is_some() {
            return Err("A different Windows worker boundary is already installed.".into());
        }
        let peers = match (worker.sids(), review.sids()) {
            (Some((worker_sid, worker_peer)), Some((review_sid, review_peer))) => {
                worker_sid != review_sid && worker_peer == review_sid && review_peer == worker_sid
            }
            _ => false,
        };
        if worker.purpose() != WorkerPurpose::Provider
            || review.purpose() != WorkerPurpose::Review
            || !peers
        {
            return Err("review_isolated_identity_unavailable".into());
        }
        *installed = Some((Arc::clone(&worker), Arc::clone(&review)));
        Ok(InstalledWorkers {
            install: Arc::clone(self),
            pair: (worker, review),
        })
    }

    /// `workerCliExclusive`: a worker of this purpose is installed and no CLI has run outside the pair. A boundary's
    /// `exclusive` option is `move || install.exclusive(purpose)`. ponytail: identity by purpose, since one daemon
    /// builds one pair; compare boundaries if a daemon ever holds two.
    pub fn exclusive(&self, purpose: WorkerPurpose) -> bool {
        let installed = self.installed();
        let present = installed.as_ref().is_some_and(|(worker, review)| {
            worker.purpose() == purpose || review.purpose() == purpose
        });
        present && !self.legacy_used.load(Ordering::SeqCst)
    }

    fn legacy_launch(&self) {
        self.legacy_used.store(true, Ordering::SeqCst);
    }
}

impl Drop for InstalledWorkers {
    fn drop(&mut self) {
        let mut installed = self.install.installed();
        let ours = installed.as_ref().is_some_and(|(worker, review)| {
            Arc::ptr_eq(worker, &self.pair.0) && Arc::ptr_eq(review, &self.pair.1)
        });
        if ours {
            *installed = None;
        }
    }
}

const STRIPPED_ENV: [&str; 20] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "CODEX_HOME",
    "CODEX_MANAGED_PACKAGE_ROOT",
    "CODEX_MANAGED_BY_NPM",
    "CODEX_MANAGED_BY_PNPM",
    "CODEX_MANAGED_BY_BUN",
    "CODEX_MANAGED_BY_VITE_PLUS",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
];
// `(?i-u)`: JavaScript's `/i` without `/u` folds ASCII only.
static SECRET_ENV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?i-u)(TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|CREDENTIALS|PRIVATE_KEY|API_KEY|ACCESS_KEY|_KEY)$").unwrap()
});
static CLOUD_ENV: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i-u)^(AWS_|AZURE_|GOOGLE_|GCP_)").unwrap());

fn is_account_folder_key(key: &str) -> bool {
    matches!(
        key.to_uppercase().as_str(),
        "CODEX_HOME" | "CLAUDE_CONFIG_DIR"
    )
}

/// The daemon's own environment.
pub fn process_env() -> Env {
    std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// `cliEnv(extra, base)`: `base` without inherited keys, config folders, endpoint overrides, secrets or cloud settings,
/// then `extra` (secrets and cloud settings still dropped).
pub fn cli_env(extra: &Env, base: &Env) -> Env {
    cli_env_mapped(extra, base, &|folder| folder.to_string())
}

fn cli_env_mapped(extra: &Env, base: &Env, map_folder: &dyn Fn(&str) -> String) -> Env {
    let allowed = |key: &str| !SECRET_ENV.is_match(key) && !CLOUD_ENV.is_match(key);
    let mut env: Env = base
        .iter()
        .filter(|(key, _)| !STRIPPED_ENV.contains(&key.to_uppercase().as_str()) && allowed(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, value) in extra.iter().filter(|(key, _)| allowed(key)) {
        env.insert(
            key.clone(),
            if is_account_folder_key(key) {
                map_folder(value)
            } else {
                value.clone()
            },
        );
    }
    env
}

fn safe_child_env(env: &Env, map_folder: &dyn Fn(&str) -> String) -> Env {
    let folders: Env = env
        .iter()
        .filter(|(key, _)| is_account_folder_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    cli_env_mapped(&folders, env, map_folder)
}

/// `nodeEnv`: the environment a CLI is started with.
pub fn node_env(cmd: &CliCommand, env: &Env) -> Env {
    node_env_mapped(cmd, env, &|folder| folder.to_string())
}

fn node_env_mapped(cmd: &CliCommand, env: &Env, map_folder: &dyn Fn(&str) -> String) -> Env {
    let mut safe = safe_child_env(env, map_folder);
    if let Some(package) = &cmd.codex_package {
        safe.insert("CODEX_MANAGED_PACKAGE_ROOT".into(), package.root.clone());
        safe.insert("CODEX_MANAGED_BY_NPM".into(), "1".into());
        if let Some(path_dir) = &package.path_dir {
            let path = safe
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            safe.retain(|key, _| !key.eq_ignore_ascii_case("PATH"));
            let joined: Vec<&str> = [path_dir.as_str(), path.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            safe.insert("PATH".into(), joined.join(";"));
        }
    }
    // The Rust daemon never runs as Electron, so TypeScript's ELECTRON_RUN_AS_NODE has no counterpart.
    safe
}

fn with_claude_key(mut env: Env, key: Option<&ApiKey>) -> Env {
    if let Some(key) = key {
        env.insert("ANTHROPIC_API_KEY".into(), key.0.clone());
        env.insert("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB".into(), "1".into());
        env.insert("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST".into(), "1".into());
    }
    env
}

/// The platform and architecture CLI lookup resolves for (`process.platform`, `process.arch`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Runtime {
    pub platform: &'static str,
    pub arch: &'static str,
}

impl Runtime {
    pub fn current() -> Self {
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            other => other,
        };
        Runtime {
            platform: if cfg!(windows) {
                "win32"
            } else {
                std::env::consts::OS
            },
            arch,
        }
    }
}

fn npm_entry(name: Provider) -> PathBuf {
    let parts: &[&str] = match name {
        Provider::Claude => &["@anthropic-ai", "claude-code", "cli.js"],
        Provider::Codex => &["@openai", "codex", "bin", "codex.js"],
    };
    parts
        .iter()
        .fold(PathBuf::from("node_modules"), |path, part| path.join(part))
}

fn cli_name(name: Provider) -> &'static str {
    match name {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn plain(path: PathBuf) -> PathBuf {
    let shown = text(&path);
    match shown.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => shown.strip_prefix(r"\\?\").map_or(path, PathBuf::from),
    }
}

/// Prefer native executables: Codex's npm wrapper starts its binary without windowsHide, opening a terminal even when
/// the wrapper is hidden. Unknown layouts keep their JS entry.
fn npm_codex(entry: &Path, runtime: Runtime) -> Option<CliCommand> {
    if runtime.platform != "win32" || !["x64", "arm64"].contains(&runtime.arch) {
        return None;
    }
    let triple = if runtime.arch == "x64" {
        "x86_64-pc-windows-msvc"
    } else {
        "aarch64-pc-windows-msvc"
    };
    let canonical = plain(std::fs::canonicalize(entry).ok()?);
    let root = canonical.parent()?.parent()?.to_path_buf();
    // Node's module lookup for @openai/codex-win32-<arch>; older npm releases bundle vendor in the main package.
    let platform_package = Path::new("node_modules")
        .join("@openai")
        .join(format!("codex-win32-{}", runtime.arch))
        .join("package.json");
    let vendor = canonical
        .parent()?
        .ancestors()
        .filter(|dir| dir.file_name().is_none_or(|name| name != "node_modules"))
        .map(|dir| dir.join(&platform_package))
        .find(|candidate| candidate.is_file())
        .and_then(|package| Some(package.parent()?.join("vendor")))
        .unwrap_or_else(|| root.join("vendor"));
    let target = vendor.join(triple);
    let file = [
        target.join("bin").join("codex.exe"),
        target.join("codex").join("codex.exe"),
    ]
    .into_iter()
    .find(|file| file.exists())?;
    let path_dir = target.join("path");
    Some(CliCommand {
        file: text(&file),
        args: vec![],
        codex_package: Some(CodexPackage {
            root: text(&root),
            path_dir: path_dir.exists().then(|| text(&path_dir)),
        }),
    })
}

/// Runs and stops the provider CLIs on this PC, inside the worker boundary when one is required.
pub struct SystemCli {
    worker: Option<Arc<dyn WorkerBoundary>>,
    /// Told about every launch outside a required worker (`legacyUsed`).
    install: Option<Arc<WorkerInstall>>,
    /// Owner account folders already mapped into the worker, by lower-cased resolved path.
    account_folders: Mutex<HashMap<String, String>>,
}

impl Default for SystemCli {
    fn default() -> Self {
        Self::new()
    }
}

fn resolved_lower(path: &str) -> String {
    crate::win::cred::resolve(Path::new(path))
        .unwrap_or_else(|_| path.to_string())
        .to_lowercase()
}

impl SystemCli {
    /// Launches directly, with no worker boundary.
    pub fn new() -> Self {
        SystemCli {
            worker: None,
            install: None,
            account_folders: Mutex::default(),
        }
    }

    pub fn with_worker(worker: Arc<dyn WorkerBoundary>) -> Self {
        SystemCli {
            worker: Some(worker),
            install: None,
            account_folders: Mutex::default(),
        }
    }

    /// Reports launches outside a required worker to the daemon's worker install, which then stops being exclusive.
    pub fn reporting_to(mut self, install: Arc<WorkerInstall>) -> Self {
        self.install = Some(install);
        self
    }

    fn legacy_launch(&self) {
        if let Some(install) = &self.install {
            install.legacy_launch();
        }
    }

    fn boundary(&self) -> Option<&Arc<dyn WorkerBoundary>> {
        self.worker.as_ref().filter(|worker| worker.required())
    }

    /// `workerAccountsRoot`.
    pub fn worker_accounts_root(&self) -> Option<PathBuf> {
        self.boundary()
            .map(|worker| worker.worker_root().join("accounts"))
    }

    /// `providerAccountFolder`: an owner account folder's private copy inside the worker root, named by a hash of its
    /// path. Folders already inside the worker root, and every folder without a worker, stay as they are.
    pub fn provider_account_folder(&self, folder: &str) -> String {
        let Some(worker) = self.boundary() else {
            return folder.to_string();
        };
        let root = text(&worker.worker_root());
        let (root_key, folder_key) = (resolved_lower(&root), resolved_lower(folder));
        let inside = folder_key == root_key
            || folder_key.starts_with(&format!("{}\\", root_key.trim_end_matches('\\')));
        if inside {
            return folder.to_string();
        }
        let hash = format!("{:x}", Sha256::digest(folder_key.as_bytes()));
        let mapped = text(&Path::new(&root).join("accounts").join(&hash[..24]));
        self.account_folders
            .lock()
            .expect("account folders lock")
            .insert(folder_key, mapped.clone());
        mapped
    }

    /// `cliEnv(extra)` over the daemon's own environment.
    pub fn cli_env(&self, extra: &Env) -> Env {
        self.cli_env_from(extra, &process_env())
    }

    pub fn cli_env_from(&self, extra: &Env, base: &Env) -> Env {
        cli_env_mapped(extra, base, &|folder| self.provider_account_folder(folder))
    }

    /// `resolveCli`: the native executable on PATH, else an npm install. Inside a worker, only its protected runtime.
    pub fn resolve_cli(&self, name: Provider, env: &Env, runtime: Runtime) -> Option<CliCommand> {
        let cli = cli_name(name);
        let command = |file: &Path, args: Vec<String>| CliCommand {
            file: text(file),
            args,
            codex_package: None,
        };
        if let Some(worker) = self.boundary() {
            let root = worker.runtime_root();
            for file in [
                root.join(format!("{cli}.exe")),
                root.join(cli).join(format!("{cli}.exe")),
            ] {
                if file.exists() {
                    return Some(command(&file, vec![]));
                }
            }
            let entry = root.join(npm_entry(name));
            if !entry.exists() {
                return None;
            }
            if name == Provider::Codex
                && let Some(native) = npm_codex(&entry, runtime)
            {
                return Some(native);
            }
            let node = root.join("node.exe");
            return node.exists().then(|| command(&node, vec![text(&entry)]));
        }
        let path = env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        // Relative entries (`.`) would make the lookup depend on the daemon's working directory.
        let dirs: Vec<&Path> = path
            .split(';')
            .map(Path::new)
            .filter(|dir| dir.is_absolute())
            .collect();
        let exe = if runtime.platform == "win32" {
            format!("{cli}.exe")
        } else {
            cli.to_string()
        };
        if let Some(file) = dirs
            .iter()
            .map(|dir| dir.join(&exe))
            .find(|file| file.exists())
        {
            return Some(command(&file, vec![]));
        }
        let entry = dirs
            .iter()
            .map(|dir| dir.join(npm_entry(name)))
            .find(|entry| entry.exists())?;
        if name == Provider::Codex
            && let Some(native) = npm_codex(&entry, runtime)
        {
            return Some(native);
        }
        // TypeScript runs a JS entry on its own Node. The Rust daemon has none, so it uses the Node on PATH.
        let node = dirs
            .iter()
            .map(|dir| dir.join("node.exe"))
            .find(|file| file.exists())?;
        Some(command(&node, vec![text(&entry)]))
    }

    /// `runCli`: runs a short command (version and status checks) and collects its output, stopping its tree after
    /// `timeout`. The only error is a `WorkerTermination`; a failed start is an output with no code.
    pub async fn run_cli(
        &self,
        cmd: &CliCommand,
        args: &[String],
        env: &Env,
        timeout: Duration,
    ) -> io::Result<CliOutput> {
        if self.boundary().is_none() {
            self.legacy_launch();
            let mut command = crate::spawn::command(&cmd.file);
            command
                .args(&cmd.args)
                .args(args)
                .env_clear()
                .envs(node_env(cmd, env))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            return Ok(
                match tokio::process::Command::from(std::process::Command::from(command)).spawn() {
                    Ok(child) => collect(wrap(child), timeout).await?,
                    Err(error) => CliOutput {
                        code: None,
                        stdout: String::new(),
                        stderr: error.to_string(),
                    },
                },
            );
        }
        let cwd = ["CODEX_HOME", "CLAUDE_CONFIG_DIR"]
            .iter()
            .find_map(|key| env.get(*key).cloned())
            .unwrap_or_else(|| text(&std::env::current_dir().unwrap_or_default()));
        match self
            .launch(
                cmd,
                args.to_vec(),
                LaunchOptions {
                    cwd,
                    env: env.clone(),
                    claude_api_key: None,
                },
            )
            .await
        {
            Ok(child) => collect(child, timeout).await,
            Err(error) if is_worker_termination(&error) => Err(error),
            Err(_) => Ok(CliOutput {
                code: None,
                stdout: String::new(),
                stderr: "The provider could not start in its required execution boundary.".into(),
            }),
        }
    }
}

impl CliHost for SystemCli {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        Box::pin(async move {
            let map = |folder: &str| self.provider_account_folder(folder);
            let env = with_claude_key(
                node_env_mapped(cmd, &options.env, &map),
                options.claude_api_key.as_ref(),
            );
            let all: Vec<String> = cmd.args.iter().cloned().chain(args).collect();
            if let Some(worker) = self.boundary() {
                let mapped = self
                    .account_folders
                    .lock()
                    .expect("account folders lock")
                    .get(&resolved_lower(&options.cwd))
                    .cloned();
                let cwd = mapped.unwrap_or(options.cwd);
                return worker.spawn(&cmd.file, all, &cwd, env).await;
            }
            self.legacy_launch();
            // This is a new pipe for the CLI protocol. It never shares the daemon's token input pipe.
            let mut command = crate::spawn::command(&cmd.file);
            command
                .args(&all)
                .env_clear()
                .envs(&env)
                .current_dir(&options.cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            Ok(wrap(
                tokio::process::Command::from(std::process::Command::from(command)).spawn()?,
            ))
        })
    }

    fn find_cli(&self, name: Provider) -> Option<CliCommand> {
        self.resolve_cli(name, &process_env(), Runtime::current())
    }

    fn run_cli<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: &'a [String],
        env: &'a Env,
        timeout: Duration,
    ) -> BoxFuture<'a, io::Result<CliOutput>> {
        Box::pin(SystemCli::run_cli(self, cmd, args, env, timeout))
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        open_url(url)
    }

    fn provider_account_folder(&self, folder: &str) -> String {
        SystemCli::provider_account_folder(self, folder)
    }

    fn worker_accounts_root(&self) -> Option<PathBuf> {
        SystemCli::worker_accounts_root(self)
    }

    fn cli_env_from(&self, extra: &Env, base: &Env) -> Env {
        SystemCli::cli_env_from(self, extra, base)
    }

    fn prepare_provider_directories(&self, folders: Vec<PathBuf>) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            let Some(worker) = self.boundary() else {
                return Ok(());
            };
            let mut seen = HashSet::new();
            for folder in folders
                .into_iter()
                .filter(|folder| seen.insert(folder.clone()))
            {
                worker.grant_directory(&folder).await?;
            }
            Ok(())
        })
    }
}

fn wrap(mut child: tokio::process::Child) -> CliChild {
    let pid = child.id();
    let stdin: Box<dyn AsyncWrite + Send + Unpin> = match child.stdin.take() {
        Some(stdin) => Box::new(stdin),
        None => Box::new(tokio::io::sink()),
    };
    let stdout: Box<dyn AsyncRead + Send + Unpin> = match child.stdout.take() {
        Some(stdout) => Box::new(stdout),
        None => Box::new(tokio::io::empty()),
    };
    let stderr: Box<dyn AsyncRead + Send + Unpin> = match child.stderr.take() {
        Some(stderr) => Box::new(stderr),
        None => Box::new(tokio::io::empty()),
    };
    let exit = ChildExit::new();
    tokio::spawn({
        let exit = exit.clone();
        async move {
            exit.fire(match child.wait().await {
                Ok(status) => CliExit::Exited {
                    code: status.code(),
                    signal: None,
                },
                Err(error) => CliExit::Failed(error.to_string()),
            });
        }
    });
    let kill_tree: KillTree = Arc::new(move || {
        Box::pin(async move {
            kill_tree_and_wait(pid)
                .await
                .map_err(|error| error.to_string())
        })
    });
    CliChild {
        pid,
        stdin,
        stdout,
        stderr,
        exit,
        kill_tree,
        retirement: None,
    }
}

async fn collect(child: CliChild, timeout: Duration) -> io::Result<CliOutput> {
    let CliChild {
        stdin,
        mut stdout,
        mut stderr,
        exit,
        kill_tree,
        retirement,
        ..
    } = child;
    drop(stdin);
    let timer = tokio::spawn(async move {
        tokio::time::sleep(timeout).await;
        let _ = kill_tree().await;
    });
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
    let status = exit.wait().await;
    timer.abort();
    if let Some(proof) = retirement {
        proof
            .await
            .map_err(|_| io::Error::other(WorkerTermination))?;
    }
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    );
    Ok(match status {
        CliExit::Exited { code, .. } => CliOutput {
            code,
            stdout,
            stderr,
        },
        CliExit::Failed(error) => CliOutput {
            code: None,
            stdout,
            stderr: error,
        },
    })
}

fn system32(program: &str) -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(program)
}

/// `killTree`: stops a CLI and everything it started, without waiting. The CLIs start their own children (shells,
/// MCP servers), so stopping one means stopping its whole tree.
pub fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let _ = crate::spawn::command(system32("taskkill.exe"))
        .args(["/pid", &pid.to_string(), "/T", "/F"])
        .env_clear()
        .envs(cli_env(&Env::new(), &process_env()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// taskkill's output, keeping at most 65,536 characters of each stream.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskkillReport {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

const REPORT_LIMIT: usize = 65_536;

impl TaskkillReport {
    pub fn new(code: Option<i32>, stdout: &str, stderr: &str) -> Self {
        let bounded = |text: &str| {
            (
                text.chars().take(REPORT_LIMIT).collect::<String>(),
                text.chars().nth(REPORT_LIMIT).is_some(),
            )
        };
        let ((stdout, stdout_truncated), (stderr, stderr_truncated)) =
            (bounded(stdout), bounded(stderr));
        TaskkillReport {
            code,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        }
    }
}

/// A process tree that could not be shown to have stopped. Carries no key material: taskkill output names processes.
#[derive(Debug)]
pub struct KillTreeError {
    pub pid: Option<u32>,
    pub report: Option<TaskkillReport>,
    message: String,
}

impl fmt::Display for KillTreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for KillTreeError {}

/// `killTreeAndWait`: Stop must wait for taskkill /T to finish, not just for the parent CLI's exit. The caller supplies
/// only a PID from its own live child.
pub async fn kill_tree_and_wait(pid: Option<u32>) -> Result<(), KillTreeError> {
    let Some(pid) = pid else {
        return Err(KillTreeError {
            pid: None,
            report: None,
            message: "Cannot stop a CLI without its process ID".into(),
        });
    };
    let mut command = crate::spawn::command(system32("taskkill.exe"));
    command
        .args(["/pid", &pid.to_string(), "/T", "/F"])
        .env_clear()
        .envs(cli_env(&Env::new(), &process_env()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::process::Command::from(std::process::Command::from(command))
        .output()
        .await
        .map_err(|error| KillTreeError {
            pid: Some(pid),
            report: None,
            message: error.to_string(),
        })?;
    let report = TaskkillReport::new(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    );
    confirm_tree_stopped(pid, std::process::id(), report, process_gone).await
}

/// Accepts a nonzero taskkill exit only with a complete reported tree and proof that every PID in it has gone:
/// sandboxed descendants can refuse taskkill yet disappear when their owner is terminated. `gone(pid)` probes one PID.
pub async fn confirm_tree_stopped(
    root: u32,
    own_pid: u32,
    report: TaskkillReport,
    mut gone: impl FnMut(u32) -> bool,
) -> Result<(), KillTreeError> {
    if report.code == Some(0) {
        return Ok(());
    }
    let targets = if report.code.is_some_and(|code| code > 0)
        && !report.stdout_truncated
        && !report.stderr_truncated
    {
        taskkill_targets(root, own_pid, &report.stdout, &report.stderr)
    } else {
        None
    };
    if let Some(targets) = targets
        && targets_gone(root, &targets, &mut gone).await
    {
        return Ok(());
    }
    let code = report
        .code
        .map_or_else(|| "null".to_string(), |code| code.to_string());
    Err(KillTreeError {
        pid: Some(root),
        report: Some(report),
        message: format!(
            "Could not stop the CLI process tree (taskkill exit {code}). Try Stop again."
        ),
    })
}

static TERMINATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^SUCCESS: The process with PID (\d+) \(child process of PID (\d+)\) has been terminated\.$").unwrap()
});
static REFUSED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ERROR: The process with PID (\d+) \(child process of PID (\d+)\) could not be terminated\.$").unwrap()
});
static REASON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Reason: .+$").unwrap());

/// Every PID of a complete reported tree under `root`, which this process owns. Unrecognized or localized output
/// keeps the failure: it cannot prove a complete tree.
fn taskkill_targets(root: u32, own_pid: u32, stdout: &str, stderr: &str) -> Option<Vec<u32>> {
    let lines = |text: &str| -> Vec<String> {
        text.split('\n')
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect()
    };
    let mut parents: Vec<(u32, u32)> = Vec::new();
    let mut add = |captures: Option<regex::Captures>| -> Option<u32> {
        let captures = captures?;
        let number = |index: usize| {
            captures[index]
                .parse::<u64>()
                .ok()
                .filter(|value| (1..=0xffff_ffff).contains(value))
                .map(|value| value as u32)
        };
        let (pid, parent) = (number(1)?, number(2)?);
        if parents.iter().any(|(known, _)| *known == pid) {
            return None;
        }
        parents.push((pid, parent));
        Some(pid)
    };
    let mut root_terminated = false;
    for line in lines(stdout) {
        root_terminated |= add(TERMINATED.captures(&line))? == root;
    }
    let errors = lines(stderr);
    for pair in errors.chunks(2) {
        add(REFUSED.captures(&pair[0]))?;
        if !pair.get(1).is_some_and(|reason| REASON.is_match(reason)) {
            return None;
        }
    }
    let parent_of = |pid: u32| {
        parents
            .iter()
            .find(|(known, _)| *known == pid)
            .map(|(_, parent)| *parent)
    };
    // An absent or failed root alone gives no evidence that taskkill enumerated its descendants.
    if !root_terminated || parent_of(root) != Some(own_pid) || parent_of(own_pid).is_some() {
        return None;
    }
    for (pid, _) in &parents {
        let mut seen = HashSet::new();
        let mut ancestor = *pid;
        while ancestor != root {
            if !seen.insert(ancestor) {
                return None;
            }
            ancestor = parent_of(ancestor)?;
        }
    }
    Some(parents.into_iter().map(|(pid, _)| pid).collect())
}

async fn targets_gone(root: u32, targets: &[u32], gone: &mut impl FnMut(u32) -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !gone(root) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    loop {
        let states: Vec<bool> = targets.iter().map(|pid| gone(*pid)).collect();
        if states.into_iter().all(|state| state) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `process.kill(pid, 0)` failing with ESRCH: no such process, or it has exited. A process this user may not open
/// counts as still there.
pub fn process_gone(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is checked and closed; the exit-code pointer outlives the call.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Err(error) => error.code() == ERROR_INVALID_PARAMETER.to_hresult(),
            Ok(handle) => {
                let mut code = 0u32;
                let read = GetExitCodeProcess(handle, &mut code).is_ok();
                let _ = CloseHandle(handle);
                read && code != STILL_ACTIVE.0 as u32
            }
        }
    }
}

/// `openUrl`: opens a sign-in URL in the default browser. No shell: URLs contain `&`, which cmd.exe would split on.
pub fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("Refusing to open a non-https URL: {url}"));
    }
    let _ = crate::spawn::command(system32("rundll32.exe"))
        .args(["url.dll,FileProtocolHandler", url])
        .env_clear()
        .envs(cli_env(&Env::new(), &process_env()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    Ok(())
}

/// `waitForCliExit`: a worker child's retirement proof, else its exit.
pub async fn wait_for_cli_exit(
    exit: &ChildExit,
    retirement: Option<&Retirement>,
) -> Result<(), ProviderError> {
    match retirement {
        Some(proof) => proof
            .clone()
            .await
            .map_err(|_| ProviderError::Failed(WorkerTermination.to_string())),
        None => {
            exit.wait().await;
            Ok(())
        }
    }
}
