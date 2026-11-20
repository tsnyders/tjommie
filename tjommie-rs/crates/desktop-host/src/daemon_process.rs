//! Port of `packages/desktop/src/daemon-process.ts` and the daemon lifecycle in `main.ts` (`launchDaemon`, `onDaemonExit`):
//! start `tjommie-daemon.exe --token-stdin`, hand it its tokens on one JSON line, find its port, restart it when it stops on
//! its own (at most 3 times a minute), and stop it (close stdin, then kill its tree) before the app quits.
//!
//! Left out on purpose: the Electron-only pieces (`userDataFor`, `daemonExecutable`, the `runAsNode` switch for 0.4.2's
//! Node daemon, an isolated test build's data root). What the panel hosts do when the daemon restarts is T3.2's.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::spawn;

/// An environment as name and value pairs. Names compare without case, as on Windows.
pub type Env = Vec<(String, String)>;

pub const PRODUCT_FOLDER: &str = "Tjommie";
/// A development run keeps its data here and listens on the debug ports, so it never touches the installed app's folder
/// or ports 47100 and 47101 (spec §4's dev column).
pub const DEV_FOLDER: &str = "Tjommie-rs";
pub const DEV_PORT: u16 = 47200;
pub const DEV_LAN_PORT: u16 = 47201;
pub const BUILD_RUST_DAEMON: &str =
    "Build the Rust daemon first: cargo build -p daemon --bin tjommie-daemon (inside tjommie-rs)";
pub const KEEPS_STOPPING: &str = "Tjommie's background service keeps stopping.";

const STOP_GRACE: Duration = Duration::from_secs(10);
const FORCE_STOP: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// The default for [`stop_before_quit`] callers: how long a graceful stop may take.
pub const QUIT_TIMEOUT: Duration = Duration::from_secs(20);
const RESTARTS_PER_WINDOW: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const STDOUT_TAIL: usize = 8_000;
const STDERR_TAIL: usize = 4_000;
const LISTENING: &str = "Tjommie daemon listening on http://127.0.0.1:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonError {
    /// No daemon exe to start; the text says what to do.
    NotBuilt(String),
    Spawn(String),
    /// "Tjommie's background service didn't start in time."
    StartTimedOut,
    /// It stopped before it was listening: its own last stderr line, or its exit code.
    Exited(String),
    /// "Tjommie's background service did not stop in time."
    StopTimedOut,
    /// Development mode found no running daemon; the text says how to start one.
    NotRunning(String),
    Random(String),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBuilt(text) | Self::Exited(text) | Self::NotRunning(text) => {
                formatter.write_str(text)
            }
            Self::Spawn(why) => write!(formatter, "Couldn't start the background service: {why}"),
            Self::StartTimedOut => {
                formatter.write_str("Tjommie's background service didn't start in time.")
            }
            Self::StopTimedOut => {
                formatter.write_str("Tjommie's background service did not stop in time.")
            }
            Self::Random(why) => write!(formatter, "No random source: {why}"),
        }
    }
}

impl std::error::Error for DaemonError {}

/// A token that must not reach a log: its `Debug` is masked.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(..)")
    }
}

/// Where the UI connects: the daemon's loopback port and its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConnection {
    pub port: u16,
    pub token: Secret,
}

// ---- environment, paths and commands (pure) ----

fn named(key: &str, name: &str) -> bool {
    key.eq_ignore_ascii_case(name)
}

pub fn env_get<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .find(|(key, _)| named(key, name))
        .map(|(_, value)| value.as_str())
}

fn env_set(env: &mut Env, name: &str, value: String) {
    env.retain(|(key, _)| !named(key, name));
    env.push((name.to_owned(), value));
}

/// A fresh token for every daemon start: 32 random bytes, 43 base64url characters.
pub fn new_token() -> Result<String, DaemonError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| DaemonError::Random(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The port in the daemon's "Tjommie daemon listening on http://127.0.0.1:<port>" line.
pub fn listening_port(output: &str) -> Option<u16> {
    let (_, after) = output.split_once(LISTENING)?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// What the daemon inherits. It runs in production mode, so nothing development-only or test-only rides in from this PC's
/// environment: `TJOMMIE_DEV`, the account-folder hook, `ELECTRON_RUN_AS_NODE`, and every `TJOMMIE_SKIP_*` switch (a debug
/// build honours them to turn off its account check and its Codex folder protection). `test_accounts_dir` is the only way a
/// test can send the account-folder hook to its disposable daemon.
pub fn daemon_env(base: &[(String, String)], test_accounts_dir: Option<&Path>) -> Env {
    let mut env: Env = base
        .iter()
        .filter(|(key, _)| {
            let name = key.to_ascii_uppercase();
            !matches!(
                name.as_str(),
                "TJOMMIE_DEV" | "TJOMMIE_ACCOUNTS_DIR" | "ELECTRON_RUN_AS_NODE"
            ) && !name.starts_with("TJOMMIE_SKIP_")
        })
        .cloned()
        .collect();
    if let Some(dir) = test_accounts_dir {
        env.push((
            "TJOMMIE_ACCOUNTS_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// `TJOMMIE_HOME` always follows the data root. `TJOMMIE_PORT` is forced to 0 only for a disposable test root; an inherited
/// one passes through, so a second install sets both and never contends for 47100. A development run that inherits no port
/// gets the development ports.
pub fn daemon_launch_env(
    base: &[(String, String)],
    data_root: &Path,
    test_root: bool,
    development: bool,
) -> Env {
    let mut env = Env::new();
    for (name, port) in [
        ("TJOMMIE_PORT", DEV_PORT),
        ("TJOMMIE_LAN_PORT", DEV_LAN_PORT),
    ] {
        if development && env_get(base, name).is_none() {
            env.push((name.to_owned(), port.to_string()));
        }
    }
    env.extend(base.iter().cloned());
    env_set(
        &mut env,
        "TJOMMIE_HOME",
        data_root.to_string_lossy().into_owned(),
    );
    if test_root {
        env_set(&mut env, "TJOMMIE_PORT", "0".to_owned());
    }
    env
}

/// Where this run keeps its data: `TJOMMIE_HOME`, else the installed app's `Tjommie` folder under Local AppData, or
/// `Tjommie-rs` when this is a development build. A development run can never resolve to the installed app's folder (a
/// different folder is a different Credential Manager namespace too).
pub fn data_root_for(env: &[(String, String)], production: bool, home: &Path) -> PathBuf {
    if let Some(chosen) = env_get(env, "TJOMMIE_HOME") {
        return PathBuf::from(chosen);
    }
    let base = match env_get(env, "LOCALAPPDATA") {
        Some(local) => PathBuf::from(local),
        None => home.join(".local").join("share"),
    };
    base.join(if production {
        PRODUCT_FOLDER
    } else {
        DEV_FOLDER
    })
}

/// The Rust daemon ships as one exe in the installed app's resources.
pub fn packaged_daemon_path(resources: &Path) -> PathBuf {
    resources.join("daemon").join("tjommie-daemon.exe")
}

/// The daemon a development run starts: the exe `TJOMMIE_DAEMON_EXE` names, else the debug build under the repo root.
pub fn source_daemon_path(env: &[(String, String)], repo_root: &Path) -> PathBuf {
    match env_get(env, "TJOMMIE_DAEMON_EXE") {
        Some(exe) if !exe.is_empty() => std::path::absolute(exe).unwrap_or_else(|_| exe.into()),
        _ => repo_root
            .join("tjommie-rs")
            .join("target")
            .join("debug")
            .join("tjommie-daemon.exe"),
    }
}

/// What starts the daemon: always the Rust exe. An installed app (`packaged_exe`) runs its bundled one whatever the
/// environment says; a development run uses [`source_daemon_path`] and says what to do when it isn't there.
pub fn daemon_command(
    env: &[(String, String)],
    packaged_exe: Option<&Path>,
    repo_root: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf, DaemonError> {
    if let Some(exe) = packaged_exe {
        return Ok(exe.to_owned());
    }
    let exe = source_daemon_path(env, repo_root);
    if exists(&exe) {
        return Ok(exe);
    }
    Err(DaemonError::NotBuilt(
        match env_get(env, "TJOMMIE_DAEMON_EXE") {
            Some(named) if !named.is_empty() => format!(
                "TJOMMIE_DAEMON_EXE names a daemon that is not there: {}",
                exe.display()
            ),
            _ => BUILD_RUST_DAEMON.to_owned(),
        },
    ))
}

/// What the app says when development mode finds no running daemon: with no `TJOMMIE_HOME` or `TJOMMIE_PORT` of its own the
/// app and a debug daemon already agree; with either set, the daemon needs the same value.
pub fn development_daemon_message(env: &[(String, String)], data_root: &Path, port: u16) -> String {
    let same: Vec<String> = [
        env_get(env, "TJOMMIE_HOME").map(|_| format!("TJOMMIE_HOME={}", data_root.display())),
        env_get(env, "TJOMMIE_PORT").map(|_| format!("TJOMMIE_PORT={port}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let start = "Start the Rust daemon in development mode first: cargo run -p daemon --bin tjommie-daemon -- --dev (inside tjommie-rs)";
    if same.is_empty() {
        format!("{start}.")
    } else {
        format!("{start}, with the same {} as this app.", same.join(" and "))
    }
}

/// Development mode connects to a daemon someone started with `--dev`: its token is in `<root>/daemon-token`.
pub fn development_connection(
    env: &[(String, String)],
    data_root: &Path,
) -> Result<DaemonConnection, DaemonError> {
    let port = env_get(env, "TJOMMIE_PORT")
        .and_then(|port| port.trim().parse().ok())
        .unwrap_or(DEV_PORT);
    match std::fs::read_to_string(data_root.join("daemon-token")) {
        Ok(token) => Ok(DaemonConnection {
            port,
            token: Secret(token.trim().to_owned()),
        }),
        Err(_) => Err(DaemonError::NotRunning(development_daemon_message(
            env, data_root, port,
        ))),
    }
}

// ---- the process ----

/// How the daemon is started. `new` fills in the defaults; tests shorten the times.
pub struct DaemonLaunch {
    pub exe: PathBuf,
    pub env: Env,
    pub test_accounts_dir: Option<PathBuf>,
    pub start_timeout: Duration,
    /// How long `stop` waits for the daemon to exit after its stdin closes before it kills the tree.
    pub stop_grace: Duration,
    /// Counts every tree kill this launch makes, for tests.
    pub kill_probe: Option<Arc<AtomicUsize>>,
}

impl DaemonLaunch {
    pub fn new(exe: PathBuf, env: Env) -> Self {
        Self {
            exe,
            env,
            test_accounts_dir: None,
            start_timeout: START_TIMEOUT,
            stop_grace: STOP_GRACE,
            kill_probe: None,
        }
    }
}

pub type Output = Arc<dyn Fn(&str) + Send + Sync>;

/// `None` while it runs; then its exit code (`None` inside when Windows gave none).
type Exit = watch::Receiver<Option<Option<i32>>>;

async fn finished(mut exit: Exit) -> Option<i32> {
    match exit.wait_for(Option::is_some).await {
        Ok(state) => (*state).flatten(),
        Err(_) => None,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Credentials<'a> {
    token: &'a str,
    desktop_host_token: &'a str,
}

/// Locks a mutex whose holder may have panicked: the data is still there, and a panic elsewhere must not stop this one.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running daemon: its connection, the capability only the desktop app holds, and ways to stop it.
pub struct RunningDaemon {
    pub connection: DaemonConnection,
    pub desktop_host_token: Secret,
    pid: Option<u32>,
    /// Dropping the pipe is how the daemon is told to stop: it exits when its stdin ends.
    stdin: Mutex<Option<ChildStdin>>,
    /// The process has exited.
    process_exit: Exit,
    /// The process has exited and its output pipes are closed (a descendant can hold them open after).
    closed: Exit,
    stop_grace: Duration,
    kill_probe: Option<Arc<AtomicUsize>>,
}

impl RunningDaemon {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Resolves with the exit code when the daemon has exited and its pipes have closed.
    pub async fn exited(&self) -> Option<i32> {
        finished(self.closed.clone()).await
    }

    /// Closes stdin and waits for the daemon to exit. After `stop_grace` it kills the tree, like `taskkill /T /F`.
    pub async fn stop(&self) {
        if self.process_exit.borrow().is_some() {
            self.exited().await;
            return;
        }
        locked(&self.stdin).take();
        if tokio::time::timeout(self.stop_grace, self.exited())
            .await
            .is_err()
        {
            self.kill_tree().await;
            self.exited().await;
        }
    }

    /// Kills the tree and waits for the daemon process itself to exit (its pipes may stay open).
    pub async fn force_stop(&self) {
        self.kill_tree().await;
        finished(self.process_exit.clone()).await;
    }

    async fn kill_tree(&self) {
        kill_tree(self.pid, &self.process_exit, self.kill_probe.as_ref()).await;
    }
}

/// `taskkill /PID <pid> /T /F`, unless the process has already exited.
async fn kill_tree(pid: Option<u32>, exit: &Exit, probe: Option<&Arc<AtomicUsize>>) {
    let Some(pid) = pid else { return };
    if exit.borrow().is_some() {
        return;
    }
    if let Some(probe) = probe {
        probe.fetch_add(1, Ordering::SeqCst);
    }
    let mut taskkill = spawn::command("taskkill");
    taskkill
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The process may have exited before taskkill runs.
    let _ = Command::from(std::process::Command::from(taskkill))
        .status()
        .await;
}

/// Reads a pipe, forwarding every chunk to `sink` and keeping its last `cap` bytes in `tail`.
fn pump(
    mut pipe: impl AsyncRead + Unpin + Send + 'static,
    cap: usize,
    sink: Output,
    tail: Arc<Mutex<String>>,
    heard: Option<watch::Sender<Option<u16>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = pipe.read(&mut buffer).await {
            let Some(bytes) = buffer.get(..read).filter(|bytes| !bytes.is_empty()) else {
                break;
            };
            let text = String::from_utf8_lossy(bytes).into_owned();
            sink(&text);
            let mut tail = locked(&tail);
            tail.push_str(&text);
            if tail.len() > cap {
                let from = (tail.len() - cap..tail.len())
                    .find(|at| tail.is_char_boundary(*at))
                    .unwrap_or(tail.len());
                tail.drain(..from);
            }
            if let (Some(heard), Some(port)) = (&heard, listening_port(&tail)) {
                heard.send_replace(Some(port));
            }
        }
    })
}

fn last_line(text: &str) -> &str {
    text.trim().lines().next_back().unwrap_or("")
}

/// Starts the daemon and waits until it says it is listening. Stdin stays open after the token line, so the daemon stops
/// when this process quits or crashes. A daemon that stops first reports its own last stderr line; one that is silent past
/// `start_timeout` is killed.
pub async fn start(launch: &DaemonLaunch, on_output: Output) -> Result<RunningDaemon, DaemonError> {
    let token = new_token()?;
    let desktop_host_token = new_token()?;
    let mut command = spawn::command(&launch.exe);
    command
        .arg("--token-stdin")
        .env_clear()
        .envs(daemon_env(&launch.env, launch.test_accounts_dir.as_deref()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Command::from(std::process::Command::from(command))
        .spawn()
        .map_err(|error| DaemonError::Spawn(error.to_string()))?;
    let pid = child.id();

    let mut stdin = child.stdin.take();
    let credentials = Credentials {
        token: &token,
        desktop_host_token: &desktop_host_token,
    };
    if let (Some(pipe), Ok(line)) = (stdin.as_mut(), serde_json::to_string(&credentials)) {
        // An early daemon exit is reported through `closed` below.
        let _ = pipe.write_all(format!("{line}\n").as_bytes()).await;
        let _ = pipe.flush().await;
    }

    let (heard_sender, mut heard) = watch::channel(None);
    let stdout_tail = Arc::new(Mutex::new(String::new()));
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let pumps: Vec<JoinHandle<()>> = [
        child.stdout.take().map(|pipe| {
            pump(
                pipe,
                STDOUT_TAIL,
                on_output.clone(),
                stdout_tail,
                Some(heard_sender),
            )
        }),
        child
            .stderr
            .take()
            .map(|pipe| pump(pipe, STDERR_TAIL, on_output, stderr_tail.clone(), None)),
    ]
    .into_iter()
    .flatten()
    .collect();

    let (process_sender, process_exit) = watch::channel(None);
    let (closed_sender, closed) = watch::channel(None);
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|status| status.code());
        process_sender.send_replace(Some(code));
        for pump in pumps {
            let _ = pump.await;
        }
        closed_sender.send_replace(Some(code));
    });

    let fail = tokio::time::sleep(launch.start_timeout);
    tokio::pin!(fail);
    let outcome = tokio::select! {
        biased;
        // Owned inside the block: a watch guard is not Send, and the timeout branch awaits.
        port = async { heard.wait_for(Option::is_some).await.ok().and_then(|state| *state) } => port,
        _ = finished(closed.clone()) => None,
        () = &mut fail => {
            kill_tree(pid, &process_exit, launch.kill_probe.as_ref()).await;
            return Err(DaemonError::StartTimedOut);
        }
    };
    let Some(port) = outcome else {
        let code = finished(closed).await;
        let reason = last_line(&locked(&stderr_tail)).to_owned();
        return Err(DaemonError::Exited(if reason.is_empty() {
            match code {
                Some(code) => format!("Tjommie's background service stopped (exit code {code})."),
                None => "Tjommie's background service stopped (no exit code).".to_owned(),
            }
        } else {
            reason
        }));
    };
    Ok(RunningDaemon {
        connection: DaemonConnection {
            port,
            token: Secret(token),
        },
        desktop_host_token: Secret(desktop_host_token),
        pid,
        stdin: Mutex::new(stdin),
        process_exit,
        closed,
        stop_grace: launch.stop_grace,
        kill_probe: launch.kill_probe.clone(),
    })
}

/// Fails with [`DaemonError::StopTimedOut`] if `stop` doesn't finish in `timeout`.
pub async fn stop_within(
    stop: impl Future<Output = ()>,
    timeout: Duration,
) -> Result<(), DaemonError> {
    tokio::time::timeout(timeout, stop)
        .await
        .map_err(|_| DaemonError::StopTimedOut)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    Forced,
}

/// A graceful stop within `timeout`; failing that, a forced one (10 s more) that confirms the process is gone.
pub async fn stop_before_quit(
    running: &RunningDaemon,
    timeout: Duration,
) -> Result<StopOutcome, DaemonError> {
    if stop_within(running.stop(), timeout).await.is_ok() {
        return Ok(StopOutcome::Stopped);
    }
    stop_within(running.force_stop(), FORCE_STOP).await?;
    Ok(StopOutcome::Forced)
}

// ---- restarts ----

/// At most 3 restarts in any 60 s. The first daemon start is not a restart.
#[derive(Debug, Default)]
pub struct RestartLimiter {
    restarts: Vec<Instant>,
}

impl RestartLimiter {
    /// Whether the daemon may be restarted at `now`; if so, counts it.
    pub fn allow(&mut self, now: Instant) -> bool {
        self.restarts
            .retain(|at| now.saturating_duration_since(*at) < RESTART_WINDOW);
        if self.restarts.len() >= RESTARTS_PER_WINDOW {
            return false;
        }
        self.restarts.push(now);
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    Starting,
    /// Running; a restart gives a new token, so a changed `token` means a new daemon.
    Running {
        connection: DaemonConnection,
        desktop_host_token: Secret,
    },
    /// Not running and not coming back; the text is for the person.
    Failed(String),
}

/// Keeps one daemon running: starts it, restarts it with a new token when it stops on its own (at most 3 a minute, then
/// `Failed`), and stops it before the app quits. Watch [`Supervisor::state`].
pub struct Supervisor {
    state: watch::Receiver<DaemonState>,
    quit: mpsc::UnboundedSender<Duration>,
    task: JoinHandle<Option<Result<StopOutcome, DaemonError>>>,
}

impl Supervisor {
    pub fn start(launch: DaemonLaunch, on_output: Output) -> Supervisor {
        let (state_sender, state) = watch::channel(DaemonState::Starting);
        let (quit, mut quit_receiver) = mpsc::unbounded_channel::<Duration>();
        let task = tokio::spawn(async move {
            let mut limiter = RestartLimiter::default();
            loop {
                state_sender.send_replace(DaemonState::Starting);
                let running = match start(&launch, on_output.clone()).await {
                    Ok(running) => running,
                    Err(error) => {
                        state_sender.send_replace(DaemonState::Failed(error.to_string()));
                        return None;
                    }
                };
                state_sender.send_replace(DaemonState::Running {
                    connection: running.connection.clone(),
                    desktop_host_token: running.desktop_host_token.clone(),
                });
                tokio::select! {
                    code = running.exited() => {
                        log::info!("The daemon stopped (exit code {code:?}).");
                        if !limiter.allow(Instant::now()) {
                            state_sender.send_replace(DaemonState::Failed(KEEPS_STOPPING.to_owned()));
                            return None;
                        }
                    }
                    timeout = quit_receiver.recv() => {
                        return Some(stop_before_quit(&running, timeout.unwrap_or(QUIT_TIMEOUT)).await);
                    }
                }
            }
        });
        Supervisor { state, quit, task }
    }

    pub fn state(&self) -> watch::Receiver<DaemonState> {
        self.state.clone()
    }

    /// Stops the daemon for good (a restart in progress is abandoned) and says how it stopped. `None`: there was no daemon
    /// to stop (it had failed already).
    pub async fn stop_before_quit(
        self,
        timeout: Duration,
    ) -> Option<Result<StopOutcome, DaemonError>> {
        let _ = self.quit.send(timeout);
        self.task.await.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// Order doesn't matter to an environment.
    fn sorted(mut env: Env) -> Env {
        env.sort();
        env
    }

    fn repo() -> PathBuf {
        PathBuf::from("C:\\repo")
    }

    #[test]
    fn makes_fresh_43_character_tokens_and_finds_the_port_in_the_daemons_output() {
        let token = new_token().unwrap();
        assert_eq!(token.len(), 43);
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(new_token().unwrap(), new_token().unwrap());
        assert_eq!(
            listening_port(
                "Removed a leftover development daemon-token file.\nTjommie daemon listening on http://127.0.0.1:52311\n"
            ),
            Some(52311)
        );
        assert_eq!(listening_port("starting"), None);
    }

    #[test]
    fn runs_the_daemon_in_production_mode_with_no_development_switch_and_no_node_environment() {
        assert_eq!(
            daemon_env(
                &env(&[("TJOMMIE_DEV", "1"), ("TJOMMIE_HOME", "C:\\x")]),
                None
            ),
            env(&[("TJOMMIE_HOME", "C:\\x")])
        );
    }

    #[test]
    fn launches_the_desktop_daemon_on_the_explicit_desktop_data_root() {
        let launched = daemon_launch_env(
            &env(&[("TJOMMIE_HOME", "C:\\owner"), ("PATH", "test")]),
            Path::new("C:\\disposable"),
            true,
            false,
        );
        assert_eq!(
            sorted(launched),
            env(&[
                ("PATH", "test"),
                ("TJOMMIE_HOME", "C:\\disposable"),
                ("TJOMMIE_PORT", "0")
            ])
        );
    }

    #[tokio::test]
    async fn rejects_a_daemon_stop_that_never_settles_so_a_later_quit_can_proceed() {
        let stopped = stop_within(std::future::pending(), Duration::from_millis(50)).await;
        let error = stopped.unwrap_err();
        assert_eq!(error, DaemonError::StopTimedOut);
        assert!(error.to_string().contains("did not stop in time"));
    }

    #[test]
    fn starts_the_exe_named_by_tjommie_daemon_exe_in_development_else_the_debug_build() {
        let debug_build = repo()
            .join("tjommie-rs")
            .join("target")
            .join("debug")
            .join("tjommie-daemon.exe");
        let present = |_: &Path| true;
        let named = env(&[("TJOMMIE_DAEMON_EXE", "C:\\tjommie-rs\\tjommie-daemon.exe")]);
        assert_eq!(
            daemon_command(&named, None, &repo(), present).unwrap(),
            PathBuf::from("C:\\tjommie-rs\\tjommie-daemon.exe")
        );
        assert_eq!(
            daemon_command(&[], None, &repo(), present).unwrap(),
            debug_build
        );
        let empty = env(&[("TJOMMIE_DAEMON_EXE", "")]);
        assert_eq!(
            daemon_command(&empty, None, &repo(), present).unwrap(),
            debug_build
        );
        assert_eq!(source_daemon_path(&[], &repo()), debug_build);
    }

    #[test]
    fn never_falls_back_to_node_a_missing_rust_daemon_fails_with_the_instruction_to_build_it() {
        let missing = |_: &Path| false;
        assert_eq!(
            daemon_command(&[], None, &repo(), missing)
                .unwrap_err()
                .to_string(),
            BUILD_RUST_DAEMON
        );
        assert_eq!(
            BUILD_RUST_DAEMON,
            "Build the Rust daemon first: cargo build -p daemon --bin tjommie-daemon (inside tjommie-rs)"
        );
        let empty = env(&[("TJOMMIE_DAEMON_EXE", "")]);
        assert_eq!(
            daemon_command(&empty, None, &repo(), missing)
                .unwrap_err()
                .to_string(),
            BUILD_RUST_DAEMON
        );
        // An override that points nowhere is its own mistake: say which file, not how to build the default.
        let named = env(&[("TJOMMIE_DAEMON_EXE", "C:\\rs\\tjommie-daemon.exe")]);
        let message = daemon_command(&named, None, &repo(), missing)
            .unwrap_err()
            .to_string();
        assert!(message.contains("C:\\rs\\tjommie-daemon.exe"), "{message}");
    }

    #[test]
    fn runs_the_bundled_rust_daemon_in_an_installed_app_whatever_the_environment_says() {
        let resources = Path::new("C:\\Users\\owner\\AppData\\Local\\Programs\\Tjommie\\resources");
        let bundled = packaged_daemon_path(resources);
        assert_eq!(bundled, resources.join("daemon").join("tjommie-daemon.exe"));
        let command = |env: &Env| daemon_command(env, Some(&bundled), &repo(), |_| false).unwrap();
        assert_eq!(command(&Vec::new()), bundled);
        // An installed app never runs an override, and never looks for the debug build.
        let elsewhere = env(&[("TJOMMIE_DAEMON_EXE", "C:\\elsewhere\\daemon.exe")]);
        assert_eq!(command(&elsewhere), bundled);
    }

    #[test]
    fn never_lets_the_rust_exe_inherit_electron_run_as_node() {
        let inherited = env(&[
            ("TJOMMIE_HOME", "C:\\x"),
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("TJOMMIE_DEV", "1"),
        ]);
        assert_eq!(
            daemon_env(&inherited, None),
            env(&[("TJOMMIE_HOME", "C:\\x")])
        );
        // Whatever its case.
        assert_eq!(
            daemon_env(&env(&[("electron_run_as_node", "1")]), None),
            Env::new()
        );
        assert_eq!(
            daemon_env(&inherited, Some(Path::new("C:\\disposable"))),
            env(&[
                ("TJOMMIE_HOME", "C:\\x"),
                ("TJOMMIE_ACCOUNTS_DIR", "C:\\disposable")
            ])
        );
    }

    #[test]
    fn keeps_an_inherited_port_next_to_the_data_root_so_a_second_install_never_contends_for_47100()
    {
        let second = Path::new("C:\\Users\\owner\\AppData\\Local\\Tjommie-two");
        let launched = daemon_launch_env(
            &env(&[("TJOMMIE_PORT", "47300"), ("TJOMMIE_LAN_PORT", "47301")]),
            second,
            false,
            false,
        );
        assert_eq!(
            sorted(launched),
            sorted(env(&[
                ("TJOMMIE_PORT", "47300"),
                ("TJOMMIE_LAN_PORT", "47301"),
                ("TJOMMIE_HOME", &second.to_string_lossy()),
            ]))
        );
        // A disposable test root always gets a free port, whatever was inherited.
        let test = daemon_launch_env(
            &env(&[("TJOMMIE_PORT", "47200")]),
            Path::new("C:\\disposable"),
            true,
            false,
        );
        assert_eq!(env_get(&test, "TJOMMIE_PORT"), Some("0"));
    }

    #[test]
    fn never_resolves_to_the_installed_apps_data_folder_in_development_and_an_installed_app_does() {
        let local = "C:\\Users\\owner\\AppData\\Local";
        let home = Path::new("C:\\Users\\owner");
        let with_local = env(&[("LOCALAPPDATA", local)]);
        let dev = data_root_for(&with_local, false, home);
        assert_eq!(dev, Path::new(local).join("Tjommie-rs"));
        assert_ne!(dev, Path::new(local).join("Tjommie"));
        assert_eq!(
            data_root_for(&with_local, true, home),
            Path::new(local).join("Tjommie")
        );
        // Without LOCALAPPDATA the fallback is still the development folder, not the product folder.
        assert_eq!(
            data_root_for(&[], false, home),
            home.join(".local").join("share").join("Tjommie-rs")
        );
        assert_eq!(
            data_root_for(&[], true, home),
            home.join(".local").join("share").join("Tjommie")
        );
    }

    #[test]
    fn lets_an_explicit_tjommie_home_win_in_both() {
        let chosen = env(&[("LOCALAPPDATA", "C:\\L"), ("TJOMMIE_HOME", "C:\\chosen")]);
        for production in [false, true] {
            assert_eq!(
                data_root_for(&chosen, production, Path::new("C:\\h")),
                PathBuf::from("C:\\chosen")
            );
        }
    }

    #[test]
    fn gives_a_development_run_the_debug_ports_when_it_inherits_none() {
        assert_eq!((DEV_PORT, DEV_LAN_PORT), (47200, 47201));
        let dev = Path::new("C:\\Users\\owner\\AppData\\Local\\Tjommie-rs");
        let home = dev.to_string_lossy().into_owned();
        let path = env(&[("PATH", "test")]);
        assert_eq!(
            sorted(daemon_launch_env(&path, dev, false, true)),
            sorted(env(&[
                ("PATH", "test"),
                ("TJOMMIE_HOME", &home),
                ("TJOMMIE_PORT", "47200"),
                ("TJOMMIE_LAN_PORT", "47201")
            ]))
        );
        // An installed app leaves the ports to the release daemon: 47100 and 47101.
        assert_eq!(
            sorted(daemon_launch_env(
                &path,
                Path::new("C:\\Tjommie"),
                false,
                false
            )),
            sorted(env(&[("PATH", "test"), ("TJOMMIE_HOME", "C:\\Tjommie")]))
        );
        // An inherited port wins, whatever its case, and a disposable test root still gets a free one.
        assert_eq!(
            sorted(daemon_launch_env(
                &env(&[("TJOMMIE_PORT", "47300")]),
                dev,
                false,
                true
            )),
            sorted(env(&[
                ("TJOMMIE_PORT", "47300"),
                ("TJOMMIE_LAN_PORT", "47201"),
                ("TJOMMIE_HOME", &home)
            ]))
        );
        assert_eq!(
            sorted(daemon_launch_env(
                &env(&[("tjommie_lan_port", "47301")]),
                dev,
                false,
                true
            )),
            sorted(env(&[
                ("tjommie_lan_port", "47301"),
                ("TJOMMIE_PORT", "47200"),
                ("TJOMMIE_HOME", &home)
            ]))
        );
        let test = daemon_launch_env(
            &env(&[("TJOMMIE_PORT", "47300")]),
            Path::new("C:\\disposable"),
            true,
            true,
        );
        assert_eq!(env_get(&test, "TJOMMIE_PORT"), Some("0"));
    }

    #[test]
    fn asks_only_for_what_the_daemon_cannot_guess_when_development_mode_finds_no_daemon() {
        let dev = Path::new("C:\\Users\\owner\\AppData\\Local\\Tjommie-rs");
        let start = "Start the Rust daemon in development mode first: cargo run -p daemon --bin tjommie-daemon -- --dev (inside tjommie-rs)";
        assert_eq!(
            development_daemon_message(&[], dev, 47200),
            format!("{start}.")
        );
        assert_eq!(
            development_daemon_message(&env(&[("TJOMMIE_PORT", "47300")]), dev, 47300),
            format!("{start}, with the same TJOMMIE_PORT=47300 as this app.")
        );
        assert_eq!(
            development_daemon_message(
                &env(&[("TJOMMIE_HOME", "C:\\chosen"), ("TJOMMIE_PORT", "47300")]),
                Path::new("C:\\chosen"),
                47300
            ),
            format!(
                "{start}, with the same TJOMMIE_HOME=C:\\chosen and TJOMMIE_PORT=47300 as this app."
            )
        );
    }

    #[test]
    fn connects_to_a_running_development_daemon_through_its_token_file_else_says_how_to_start_one()
    {
        let root = tempfile::tempdir().unwrap();
        let missing = development_connection(&[], root.path()).unwrap_err();
        assert!(matches!(missing, DaemonError::NotRunning(_)), "{missing}");
        std::fs::write(root.path().join("daemon-token"), "the-dev-token\r\n").unwrap();
        let connection = development_connection(&[], root.path()).unwrap();
        assert_eq!(
            (connection.port, connection.token.expose()),
            (47200, "the-dev-token")
        );
        let chosen =
            development_connection(&env(&[("TJOMMIE_PORT", "47300")]), root.path()).unwrap();
        assert_eq!(chosen.port, 47300);
        assert!(
            !format!("{connection:?}").contains("the-dev-token"),
            "a token must not reach a log"
        );
    }

    #[test]
    fn does_not_forward_an_inherited_test_hook_that_turns_off_a_check_or_a_protection() {
        let parent = env(&[
            ("TJOMMIE_SKIP_FOLDER_PROTECTION", "1"),
            ("tjommie_skip_account_check", "1"), // Whatever its case.
            ("TJOMMIE_SKIP_SOMETHING_NEW", "1"), // Every TJOMMIE_SKIP_* switch, not only today's.
            ("TJOMMIE_HOME", "C:\\x"),
        ]);
        let forwarded = daemon_env(&parent, None);
        assert!(
            forwarded
                .iter()
                .all(|(key, _)| !key.to_ascii_uppercase().starts_with("TJOMMIE_SKIP_"))
        );
        assert_eq!(env_get(&forwarded, "TJOMMIE_HOME"), Some("C:\\x")); // Other settings still pass through.
    }

    #[test]
    fn does_not_forward_an_inherited_accounts_folder_hook() {
        let parent = env(&[("TJOMMIE_ACCOUNTS_DIR", "C:\\private-accounts")]);
        assert_eq!(
            env_get(&daemon_env(&parent, None), "TJOMMIE_ACCOUNTS_DIR"),
            None
        );
        assert_eq!(
            env_get(
                &daemon_env(&parent, Some(Path::new("C:\\disposable-accounts"))),
                "TJOMMIE_ACCOUNTS_DIR"
            ),
            Some("C:\\disposable-accounts")
        );
    }

    #[test]
    fn restart_limit_three_per_minute() {
        let mut limiter = RestartLimiter::default();
        let start = Instant::now();
        let second = Duration::from_secs(1);
        assert!(limiter.allow(start));
        assert!(limiter.allow(start + second));
        assert!(limiter.allow(start + 2 * second));
        // The fourth inside a minute is refused, and stays refused until the first one is a minute old.
        assert!(!limiter.allow(start + 3 * second));
        assert!(!limiter.allow(start + Duration::from_secs(59)));
        assert!(limiter.allow(start + Duration::from_secs(61)));
    }
}
