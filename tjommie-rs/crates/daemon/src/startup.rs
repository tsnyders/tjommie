//! `main.ts` and the start of `startDaemon`: the mode, ports and stdin token, then the server held on its port until
//! the daemon is ready.

use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};

use futures_util::FutureExt;
use protocol::Event;
use protocol::model::{Autonomy, Message, Provider};
use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::account_handlers::{Accounts, AccountsOptions};
use crate::account_quota::HttpsFetch;
use crate::bots::StoredBot;
use crate::cli::{InstalledWorkers, SystemCli, WorkerInstall, WorkerPurpose};
use crate::context::{self, Context, Db, Late, LateBotView};
use crate::m4::{M4, M4Deps};
use crate::m5::{self, LateTasks, M5, M5Deps};
use crate::m6::{self, M6, M6Deps};
use crate::protect::{FolderProtection, ListAccounts};
use crate::queue::WorkItem;
use crate::relay_client::default_connector;
use crate::remote::{Remote, RemoteOptions};
use crate::remote_host::{Dispatch, RemoteHostOptions};
use crate::runner::{Core, Runner, RunnerDeps, RunnerHooks, TurnResult, cli_session_factory};
use crate::server::{
    Broadcaster, Handlers, RequestContext, Routes, RpcError, Server, ServerOptions,
};
use crate::setup::Setup;
use crate::turn_notices::turn_notice;
use crate::win::acl::{ProtectedAccount, ProtectedPaths, Protection};
use crate::win::boundary::{BoundaryOptions, WindowsNative, WorkerBoundary};
use crate::win::cred::Namespace;
use crate::win::worker::Purpose;
use crate::{accounts_store, bots, db, diagnostics, paths, settings};

/// The phone's LAN port, now set by `TJOMMIE_LAN_PORT` (`remote-host.ts:24` fixes it at 47101). A debug build uses the
/// spec §4 dev port.
pub const DEFAULT_LAN_PORT: u16 = if cfg!(debug_assertions) { 47201 } else { 47101 };

const NO_MODE: &str =
    "Start Tjommie from the desktop app, or run npm start -w @tjommie/daemon for development.";

#[derive(Debug)]
pub enum StartupError {
    /// Printed as it is: the user can act on it.
    Plain(String),
    /// Printed with its details: something nobody expected.
    Unexpected(io::Error),
}

fn plain(message: impl Into<String>) -> StartupError {
    StartupError::Plain(message.into())
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartupError::Plain(message) => formatter.write_str(message),
            StartupError::Unexpected(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for StartupError {}

impl From<io::Error> for StartupError {
    fn from(error: io::Error) -> Self {
        StartupError::Unexpected(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `--dev` or `TJOMMIE_DEV=1`: the token lives in the data root's `daemon-token` file.
    Development,
    /// `--token-stdin`: the desktop app pipes the token in and keeps stdin open while it runs.
    Production,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub mode: Mode,
    pub port: u16,
    pub lan_port: u16,
    /// The address the phone's LAN host binds. Production listens on every interface so phones on the network reach
    /// this PC; tests and manual runs set `TJOMMIE_LAN_BIND=127.0.0.1`, which keeps Windows Firewall from prompting.
    pub lan_bind: IpAddr,
    /// A test hook (`TJOMMIE_SKIP_ACCOUNT_CHECK=1`): don't probe the provider CLIs when the daemon starts, so tests
    /// never run a real `claude` or `codex`. Only a debug build honours it (see [`test_hook`]).
    pub skip_account_check: bool,
    /// A test hook (`TJOMMIE_SKIP_FOLDER_PROTECTION=1`): don't run the Codex data ACL pass, at start or later. The pass
    /// reads and changes the real `~/.codex/.sandbox` permissions, which no test may touch; skipped, the protection
    /// reads as ready. Only a debug build honours it (see [`test_hook`]).
    pub skip_folder_protection: bool,
    /// A test hook: the desktop's real-CLI tests point a disposable root at signed-in account folders.
    pub accounts_dir: Option<PathBuf>,
}

/// Reads the arguments and environment the way `main.ts` does. `env` looks up a variable.
pub fn parse_launch(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Launch, StartupError> {
    if let Some(unknown) = args
        .iter()
        .find(|arg| *arg != "--dev" && *arg != "--token-stdin")
    {
        return Err(plain(format!("Unknown argument: {unknown}")));
    }
    let development =
        env("TJOMMIE_DEV").as_deref() == Some("1") || args.iter().any(|arg| arg == "--dev");
    let production = args.iter().any(|arg| arg == "--token-stdin");
    if development == production {
        return Err(plain(if development {
            "Choose either --dev or --token-stdin."
        } else {
            NO_MODE
        }));
    }
    Ok(Launch {
        mode: if development {
            Mode::Development
        } else {
            Mode::Production
        },
        port: port(env("TJOMMIE_PORT"), paths::DEFAULT_PORT, "TJOMMIE_PORT")?,
        lan_port: port(
            env("TJOMMIE_LAN_PORT"),
            DEFAULT_LAN_PORT,
            "TJOMMIE_LAN_PORT",
        )?,
        lan_bind: lan_bind(env("TJOMMIE_LAN_BIND"))?,
        skip_account_check: test_hook(&env, "TJOMMIE_SKIP_ACCOUNT_CHECK"),
        skip_folder_protection: test_hook(&env, "TJOMMIE_SKIP_FOLDER_PROTECTION"),
        accounts_dir: env("TJOMMIE_ACCOUNTS_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    })
}

/// A test hook that switches off a real check: on only when `name` is exactly `1`, and only in a debug build. Tests
/// build debug executables; a release build (what the owner installs) ignores the variable, so one left in a PC's
/// environment, or forwarded by an app that doesn't strip it, can never silently turn off the folder protection while
/// setup reports it ready.
fn test_hook(env: &impl Fn(&str) -> Option<String>, name: &str) -> bool {
    cfg!(debug_assertions) && env(name).as_deref() == Some("1")
}

/// `TJOMMIE_LAN_BIND`: unset or empty is every interface.
fn lan_bind(value: Option<String>) -> Result<IpAddr, StartupError> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        Some(value) => value
            .parse()
            .map_err(|_| plain("TJOMMIE_LAN_BIND must be an IP address.")),
    }
}

/// `Number(value)` in `main.ts`: unset is the default, empty is 0 (any free port).
fn port(value: Option<String>, default: u16, name: &str) -> Result<u16, StartupError> {
    match value.as_deref().map(str::trim) {
        None => Ok(default),
        Some("") => Ok(0),
        Some(value) => value
            .parse()
            .map_err(|_| plain(format!("{name} must be a port number."))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinEvent {
    /// The first line, without its line ending.
    Line(String),
    /// Stdin ended or failed.
    Closed,
}

/// Reads the token line, then keeps reading until stdin ends or fails, so the daemon stops with the app that started
/// it: when the desktop quits or crashes, the pipe closes.
pub fn watch_stdin(input: impl Read + Send + 'static) -> mpsc::Receiver<StdinEvent> {
    let (events, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = BufReader::new(input);
        let mut line = Vec::new();
        if matches!(input.read_until(b'\n', &mut line), Ok(read) if read > 0) {
            if line.ends_with(b"\n") {
                line.pop();
            }
            if line.ends_with(b"\r") {
                line.pop();
            }
            let _ = events.send(StdinEvent::Line(
                String::from_utf8_lossy(&line).into_owned(),
            ));
            let _ = io::copy(&mut input, &mut io::sink());
        }
        let _ = events.send(StdinEvent::Closed);
    });
    receiver
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Development,
    Production {
        token: String,
        /// The desktop app's own capability for `/browser-host`, `/worker-host` and `/vault-host` (M6).
        desktop_host_token: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopCredentials {
    token: String,
    desktop_host_token: String,
}

/// The production stdin line: a bare token, or the desktop app's `{ token, desktopHostToken }`.
pub fn production_auth(line: &str) -> Result<Auth, StartupError> {
    if !line.starts_with('{') {
        return Ok(Auth::Production {
            token: line.to_owned(),
            desktop_host_token: None,
        });
    }
    match serde_json::from_str::<DesktopCredentials>(line) {
        Ok(credentials) if is_capability(&credentials.desktop_host_token) => Ok(Auth::Production {
            token: credentials.token,
            desktop_host_token: Some(credentials.desktop_host_token),
        }),
        _ => Err(plain("Invalid desktop startup credentials.")),
    }
}

/// At least 43 base64url characters: 32 random bytes or more.
fn is_capability(token: &str) -> bool {
    token.len() >= 43
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The desktop app loads the UI from app://tjommie; `npm run dev` serves it on localhost:3000.
fn allowed_origins(auth: &Auth) -> Vec<String> {
    let mut origins = vec!["app://tjommie".to_owned()];
    if *auth == Auth::Development {
        origins.push("http://localhost:3000".to_owned());
    }
    origins
}

pub struct Daemon {
    server: Server,
    root: PathBuf,
    runner: Runner,
    accounts: Accounts,
    remote: Arc<Remote>,
    m4: Arc<M4>,
    m5: Arc<M5>,
    m6: Arc<M6>,
    /// Delivers every broadcast event to paired devices, in order.
    publisher: JoinHandle<()>,
    /// Dropping this uninstalls the Windows worker pair.
    _workers: Option<InstalledWorkers>,
}

impl Daemon {
    pub fn port(&self) -> u16 {
        self.server.port()
    }

    /// The data root this daemon uses.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stops answering, then closes what the daemon runs, in `daemon.ts`'s order: the phone connection, the M6
    /// brokers, the project services, the runner (and the CLI processes it owns), the accounts, and last the server.
    pub async fn close(self) {
        self.server.stop_requests();
        self.remote.close().await;
        self.m6.close();
        self.m4.stop();
        self.m5.shutdown().await;
        if let Err(error) = self.runner.shutdown().await {
            eprintln!("{error}");
        }
        self.accounts.detach_runner();
        self.accounts.runtime.close().await;
        self.publisher.abort();
        self.server.close().await;
    }
}

fn rpc(error: RpcError) -> StartupError {
    plain(error.message)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `startDaemon`: check the tokens, hold the port, open the data root and its database, build the daemon's services and
/// answer.
pub async fn start(launch: &Launch, auth: &Auth) -> Result<Daemon, StartupError> {
    start_in(launch, auth, paths::data_root()).await
}

/// [`start`] on a given data root, which is how a test runs a whole daemon on a temporary one.
pub async fn start_in(launch: &Launch, auth: &Auth, root: PathBuf) -> Result<Daemon, StartupError> {
    if let Auth::Production {
        token,
        desktop_host_token,
    } = auth
    {
        if !is_capability(token) {
            return Err(plain(
                "Daemon token must be at least 43 base64url characters.",
            ));
        }
        if desktop_host_token
            .as_ref()
            .is_some_and(|host| !is_capability(host) || host == token)
        {
            return Err(plain(
                "The native browser host requires an independent private capability.",
            ));
        }
    }
    // The listening port is the lock: a second daemon stops here, before it touches any data.
    let options = ServerOptions {
        port: launch.port,
        token: String::new(),
        handlers: Handlers::default(),
        allowed_origins: allowed_origins(auth),
        defer_requests: true,
    };
    let server = Server::start(options)
        .await
        .map_err(|error| match error.kind() {
            io::ErrorKind::AddrInUse => plain(format!(
                "Tjommie is already running: port {} is in use.",
                launch.port
            )),
            _ => StartupError::Unexpected(error),
        })?;
    let parts = match assemble(launch, auth, &root, &server) {
        Ok(parts) => parts,
        Err(error) => {
            server.close().await;
            return Err(error);
        }
    };
    // Account and token creation, plus existing bot homes, need a fresh pass (`daemon.ts:134-135`). It runs here, where
    // a failure closes everything built so far, rather than inside `assemble`, which is not async.
    let protected = parts.accounts.protect(true, None).await.map_err(rpc);
    let Parts {
        token,
        handlers,
        routes,
        runner,
        accounts,
        remote,
        m4,
        m5,
        m6,
        publisher,
        workers,
    } = parts;
    // The folder protection pass, then `runner.start()` and `scheduler.start()`, run before the server answers, as in
    // `daemon.ts:646-648`.
    let activated = protected
        .and_then(|()| {
            runner
                .start()
                .and_then(|()| m4.start_scheduler())
                .map_err(rpc)
        })
        .and_then(|()| server.activate_with(token, handlers, routes).map_err(rpc));
    let daemon = Daemon {
        server,
        root,
        runner,
        accounts,
        remote,
        m4,
        m5,
        m6,
        publisher,
        _workers: workers,
    };
    if let Err(error) = activated {
        daemon.close().await;
        return Err(error);
    }
    if !launch.skip_account_check {
        let checking = daemon.accounts.runtime.check_all();
        tokio::spawn(async move {
            if checking.await.is_err() {
                eprintln!("Some accounts could not be checked. Retry from Accounts.");
            }
        });
    }
    let remote = daemon.remote.clone();
    tokio::spawn(async move { remote.start().await });
    Ok(daemon)
}

/// `daemon.ts`'s `turnEnded` for what M4 hears: a routine run is settled, the team answers, and the turn notice goes
/// out, each even if the one before failed. The first error is the one reported. (M5's task hook runs first.)
fn turn_ended(
    m4: &M4,
    notices: &Broadcaster,
    core: &mut Core<'_>,
    item: &WorkItem,
    result: &TurnResult,
) -> Result<(), String> {
    let settled = m4.turn_ended(core, item, result);
    let noticed = turn_notice(core.db(), item, result)
        .map_err(|error| error.message)
        .map(|notice| {
            if let Some(notice) = notice {
                notices.send(Event::Notify(notice));
            }
        });
    settled.and(noticed)
}

/// Everything `start` builds before the server answers.
struct Parts {
    token: String,
    handlers: Handlers,
    routes: Routes,
    runner: Runner,
    accounts: Accounts,
    remote: Arc<Remote>,
    m4: Arc<M4>,
    m5: Arc<M5>,
    m6: Arc<M6>,
    publisher: JoinHandle<()>,
    workers: Option<InstalledWorkers>,
}

/// The provider and review worker boundaries (`startDaemon`'s `WindowsWorkerBoundary` pair), installed as the daemon's
/// launch path when provider isolation is set up. Returns the provider boundary, which `SystemCli` and M6 use even
/// when isolation is off (a boundary that isn't required launches nothing), and the review boundary, which M5's
/// reviews use the same way.
fn install_workers(
    root: &Path,
    exe_dir: &Path,
    install: &Arc<WorkerInstall>,
) -> Result<
    (
        WorkerBoundary,
        Arc<WorkerBoundary>,
        Option<InstalledWorkers>,
    ),
    StartupError,
> {
    let boundary = |file: &str, purpose: Purpose, kind: WorkerPurpose| {
        let install = install.clone();
        WorkerBoundary::new(BoundaryOptions {
            configuration_path: root.join(file),
            purpose,
            daemon_dir: exe_dir.to_path_buf(),
            exclusive: Arc::new(move || install.exclusive(kind)),
            native: Arc::new(WindowsNative),
            observer: None,
        })
    };
    let provider = boundary("worker.json", Purpose::Provider, WorkerPurpose::Provider);
    let review = Arc::new(boundary(
        "review-worker.json",
        Purpose::Review,
        WorkerPurpose::Review,
    ));
    let installed = if provider.required() {
        // Pairing runs the rest of `validateWorkerPair` and lets each identity see the other's receipts.
        provider
            .pair_with(&review)
            .and_then(|()| review.pair_with(&provider))
            .map_err(|error| plain(error.to_string()))?;
        Some(
            install
                .install(Arc::new(provider.clone()), review.clone())
                .map_err(plain)?,
        )
    } else {
        None
    };
    Ok((provider, review, installed))
}

/// `initializeDaemon` as far as the port has come, up to activating the server: open the data root and database, bring
/// up the accounts and the runner (recovering what a restart cut off), the phone connection and M6. M4 (the team,
/// routines and `/mcp`) and M5 (projects and tasks) add their handlers and runner hooks where marked.
fn assemble(
    launch: &Launch,
    auth: &Auth,
    root: &Path,
    server: &Server,
) -> Result<Parts, StartupError> {
    let mut paths = paths::data_paths(root)?;
    if let Some(accounts) = &launch.accounts_dir {
        paths.accounts = accounts.clone();
    }
    // Never open SQLite before the legacy database has moved: a split WAL and database must not be used.
    paths::migrate_legacy_db(&paths)?;
    // Production removes a development token file left behind, so no stale token can open the daemon.
    if matches!(auth, Auth::Production { .. }) && remove_leftover_token(&paths.token)? {
        println!("Removed a leftover development daemon-token file.");
    }
    // The only synchronous ACL pass (`daemon.ts:127-128`): it finishes before SQLite opens. A test hook skips it.
    let protected = ProtectedPaths::new(&paths.root);
    let first = if launch.skip_folder_protection {
        Protection::Ready
    } else {
        FolderProtection::first_pass(&protected)
    };
    let db: Db = Arc::new(Mutex::new(db::open(&paths.db)?));
    let token = match auth {
        Auth::Production { token, .. } => token.clone(),
        Auth::Development => paths::read_or_create_token(&paths.token)?,
    };
    let exe = std::env::current_exe()?;
    let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();

    // How CLIs launch: inside the Windows worker when it is set up, else directly.
    let install = Arc::new(WorkerInstall::default());
    let (provider_worker, review_worker, workers) =
        install_workers(&paths.root, &exe_dir, &install)?;
    let cli =
        Arc::new(SystemCli::with_worker(Arc::new(provider_worker.clone())).reporting_to(install));
    let setup = Setup::new(cli.clone(), Arc::new(HttpsFetch::new().map_err(plain)?));

    // 1. One account per provider, then the accounts runtime.
    setup
        .ensure_accounts(&lock(&db), &paths.accounts, &paths.root)
        .map_err(rpc)?;
    let events = server.broadcaster();
    let view = LateBotView::default();
    let accounts = Accounts::new(AccountsOptions {
        db: db.clone(),
        root: paths.root.clone(),
        accounts_dir: paths.accounts.clone(),
        events: events.clone(),
        bot_view: Arc::new(view.clone()),
        setup: setup.clone(),
        ops: Arc::new(setup.clone()),
        credentials: Arc::new(Namespace::new(&paths.root)?),
    });
    // The Codex data ACL pass, which the account and bot handlers and `setup.status` run again. A test hook swaps the
    // real pass for one that finds nothing to do.
    let list_accounts: ListAccounts = Arc::new(|db| {
        Ok(accounts_store::list_accounts(db, false)?
            .into_iter()
            .map(|account| ProtectedAccount {
                id: account.id,
                provider: account.provider,
                folder: PathBuf::from(account.folder),
            })
            .collect())
    });
    let protection = if launch.skip_folder_protection {
        FolderProtection::with_pass(
            protected,
            db.clone(),
            list_accounts,
            first,
            Arc::new(|_, _, _| Protection::Ready),
        )
    } else {
        FolderProtection::new(protected, db.clone(), list_accounts, first)
    };
    accounts.attach_protection(protection.clone());

    // 2. The runner, on those accounts.
    let late = Late::default();
    let late_tasks = LateTasks::default();
    let (held, issue) = (late.switching.clone(), late.switching.clone());
    let broadcast = events.clone();
    let native_browser = late.m6.clone();

    // M4 (the team, routines, the scheduler and `/mcp`) is made first because the runner's hooks call it; it gets the
    // runner right after (`m4.attach`). M5 adds its task hooks beside these, running first in `turn_started`/`turn_ended`.
    let mcp_url = format!("http://127.0.0.1:{}/mcp", server.port());
    let m4 = {
        let (sent, cleanup, notices) = (events.clone(), late.m6.clone(), events.clone());
        Arc::new(M4::new(M4Deps {
            db: db.clone(),
            broadcast: Arc::new(move |event| {
                sent.send(event);
                Ok(())
            }),
            log: Arc::new(|context, error| eprintln!("{context}: {error}")),
            shared_files: paths.shared_files.clone(),
            // M6 makes the real `Attachments` after the runner: `m4.attach_attachments` gives them over.
            attachments: None,
            cleanup_shared: Arc::new(move || {
                if let Some(m6) = cleanup.get()
                    && m6.attachments.cleanup().is_err()
                {
                    eprintln!(
                        "Unused shared files could not be removed. Cleanup will retry after restart."
                    );
                }
            }),
            // `routines.delete` ends the work its routine had queued through the daemon's own `turnEnded`. M4 is handed
            // in, not captured: M4 holds this closure.
            turn_ended: Arc::new(
                move |m4: &M4, core: &mut Core<'_>, item: &WorkItem, result: &TurnResult| {
                    turn_ended(m4, &notices, core, item, result)
                },
            ),
        }))
    };
    let hooks = {
        let (servers, closed, started, requeued, retry, ended) = (
            m4.clone(),
            m4.clone(),
            m4.clone(),
            m4.clone(),
            m4.clone(),
            m4.clone(),
        );
        let (browser, notices) = (late.m6.clone(), events.clone());
        RunnerHooks {
            mcp_servers: Some(Arc::new(
                move |_core: &mut Core<'_>, bot: &StoredBot, _item: &WorkItem| {
                    Ok(servers.mcp_servers(bot, &mcp_url))
                },
            )),
            session_closed: Some(Arc::new(move |_core: &mut Core<'_>, bot_id: &str| {
                closed.session_closed(bot_id);
                if let Some(m6) = browser.get() {
                    m6.browser_mcp.close(bot_id);
                }
                Ok(())
            })),
            turn_started: Some(Arc::new(move |core: &mut Core<'_>, item: &WorkItem| {
                started.turn_started(core, item)
            })),
            turn_requeued: Some(Arc::new(move |_core: &mut Core<'_>, item: &WorkItem| {
                requeued.turn_requeued(item);
                Ok(())
            })),
            turn_ended: Some(Arc::new(
                move |core: &mut Core<'_>, item: &WorkItem, result: &TurnResult| {
                    turn_ended(&ended, &notices, core, item, result)
                },
            )),
            retry_routine: Some(Arc::new(move |core: &mut Core<'_>, message: &Message| {
                retry.retry_routine(core, message)
            })),
            ..RunnerHooks::default()
        }
    };
    let deps = RunnerDeps {
        accounts: Some(Arc::new(accounts.runtime.clone())),
        // TypeScript's `closeBrowser` is the headless registry, which the desktop host leaves empty and this port has no
        // equivalent of. The native tools are fenced there by the `sessionClosed` hook, so a deleting bot's are fenced
        // here, in the future deletion awaits with the process's exit. M6 unbound (the handler tests): nothing to close.
        close_browser: Arc::new(move |bot_id| {
            let (m6, bot_id) = (native_browser.get().cloned(), bot_id.to_owned());
            async move {
                if let Some(m6) = m6 {
                    m6.browser_mcp.close(&bot_id);
                }
            }
            .boxed()
        }),
        is_held: Arc::new(move |bot_id| lock(&held).contains(bot_id)),
        // `daemon.ts:234-239`: a bot changing provider waits; a Codex bot that is confined to its folder also waits for
        // the folder protection to be in place.
        setup_issue: Arc::new(move |bot| {
            if lock(&issue).contains(&bot.id) {
                return Some("This bot is changing provider. Try again shortly.".to_owned());
            }
            if bot.provider != Provider::Codex || bot.autonomy != Autonomy::Folder {
                return None;
            }
            protection.reason().map(str::to_owned)
        }),
        kill_tree: Arc::new(crate::cli::kill_tree),
        // M5's task hooks run first (`tasks.turnEnded`, then the rest). M4's hooks follow: `scheduler.runEnded` and
        // `team.turnEnded` ahead of the notice.
        hooks: late_tasks.hooks(hooks),
        ..RunnerDeps::new(
            db.clone(),
            Arc::new(move |event| {
                broadcast.send(event);
                Ok(())
            }),
            cli_session_factory(cli.clone()),
            setup.clone(),
        )
    };
    let runner = Runner::new(deps);
    view.bind(Arc::new(runner.clone()));
    accounts.attach_runner(runner.clone());
    m4.attach(runner.clone());
    let _ = late.runner.set(runner.clone());

    // 3. The handlers.
    let context = Context {
        db: db.clone(),
        bots_dir: paths.bots.clone(),
        events: events.clone(),
        bot_view: Arc::new(runner.clone()),
        // Beside the exe, as `build-info.json` sat beside the Node daemon's bundle.
        app_version: diagnostics::read_app_version(&exe_dir),
        accounts: accounts.clone(),
        late,
    };
    let mut handlers = crate::runner::register(context::handlers(&context), &context, &runner);

    // 4. The phone connection (M7). Events reach paired devices through one ordered queue: publishing reads the
    // database, and the runner and the accounts broadcast with it locked.
    let (publish, mut published) = tokio::sync::mpsc::unbounded_channel::<Event>();
    server.tap_events(move |event| {
        let _ = publish.send(event.clone());
    });
    let dispatcher = server.dispatcher();
    let dispatch: Dispatch = Arc::new(
        move |method: String, params: Value, request: RequestContext| {
            let dispatcher = dispatcher.clone();
            Box::pin(async move { dispatcher.dispatch(&method, params, Some(request)).await })
        },
    );
    let mut host = RemoteHostOptions::new(db.clone(), paths.root.clone(), dispatch);
    host.port = launch.lan_port;
    host.bind = launch.lan_bind;
    let relay_db = db.clone();
    let remote = Remote::new(RemoteOptions {
        host,
        relay_url: Arc::new(move || {
            settings::get_settings(&lock(&relay_db))
                .ok()
                .and_then(|settings| settings.relay_url)
                .map(|url| url.as_str().to_owned())
        }),
        relay_connect: default_connector(),
        broadcast: {
            let events = events.clone();
            Arc::new(move |event| events.send(event))
        },
    });
    let _ = context.late.remote.set(remote.clone());
    handlers = remote.handlers(handlers);
    let publisher = tokio::spawn({
        let remote = remote.clone();
        async move {
            while let Some(event) = published.recv().await {
                remote.publish(&event);
            }
        }
    });

    // 5. Attachments, Logins, the native browser and the worker host (M6). After the sweep, before activating.
    bots::sweep_deleted_bot_folders(&lock(&db), &paths.bots);
    let mut routes = Routes::default();
    let desktop_host_token = match auth {
        Auth::Production {
            desktop_host_token, ..
        } => desktop_host_token.clone(),
        Auth::Development => None,
    };
    let m6 = Arc::new(
        m6::install(
            &mut handlers,
            &mut routes,
            M6Deps {
                db: db.clone(),
                data_root: paths.root.clone(),
                desktop_host_token,
                // `McpTokens::bot_for`, shared with `/mcp`: the bot a tool session token was issued to.
                bot_for: {
                    let tokens = m4.tokens.clone();
                    Arc::new(move |token| tokens.bot_for(token))
                },
                worker: Some(Arc::new(provider_worker)),
            },
        )
        .map_err(rpc)?,
    );
    let _ = context.late.m6.set(m6.clone());
    // `browser.stream` frames go to the connection that asked, over the local WebSocket or a paired device's channel.
    m6.connect_events(events.clone(), remote.clone());

    // 5b. The team, routines, the scheduler's handlers and `/mcp` (M4). The team takes M6's shared-file links.
    m4.attach_attachments(m6.attachments.clone());
    let _ = context.late.m4.set(m4.clone());
    m4.install(&mut handlers, &mut routes, &context, Vec::new())
        .map_err(rpc)?;

    // 5c. Projects, tasks and reviews (M5). The runner's task hooks reach the service through `late_tasks`.
    let m5 = Arc::new(
        m5::install(
            &mut handlers,
            M5Deps {
                db: db.clone(),
                root: paths.root.clone(),
                events: events.clone(),
                switching: context.late.switching.clone(),
                deleting: context.late.deleting.clone(),
                runner: runner.clone(),
                accounts: accounts.runtime.clone(),
                host: cli,
                review_worker: Some(review_worker),
                late: late_tasks,
            },
        )
        .map_err(rpc)?,
    );
    let _ = context.late.m5.set(m5.clone());

    // 6. What a restart cut off is settled before anything listens (`runner.start()` then runs the queue).
    m4.start_team();
    runner.recover().map_err(rpc)?;
    m5.recover(&lock(&db)).map_err(rpc)?;
    Ok(Parts {
        token,
        handlers,
        routes,
        runner,
        accounts,
        remote,
        m4,
        m5,
        m6,
        publisher,
        workers,
    })
}

/// Removes a development token file. True if there was one. Retries a file that is briefly locked, as
/// `rmSync(..., { maxRetries: 3 })` does with its default 100 ms linear backoff.
pub fn remove_leftover_token(file: &Path) -> Result<bool, StartupError> {
    let leftover = file.exists();
    let mut attempt = 0;
    loop {
        match remove_regular_file(file, leftover) {
            Ok(()) => return Ok(leftover),
            // Access denied, or a sharing or lock violation (Windows errors 32 and 33).
            Err(error)
                if attempt < 3
                    && (error.kind() == io::ErrorKind::PermissionDenied
                        || matches!(error.raw_os_error(), Some(32 | 33))) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(100 * attempt));
            }
            Err(_) => {
                return Err(plain(
                    "Could not remove the development daemon-token file. Close programs using it and start Tjommie again.",
                ));
            }
        }
    }
}

fn remove_regular_file(file: &Path, exists: bool) -> io::Result<()> {
    if exists && !fs::symlink_metadata(file)?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    match fs::remove_file(file) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
