//! `main.ts` and the start of `startDaemon`: the mode, ports and stdin token, then the server held on its port until
//! the daemon is ready.

use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use serde::Deserialize;

use crate::context::{self, Context, StoredState};
use crate::server::{Handlers, Server, ServerOptions};
use crate::{bots, db, diagnostics, paths};

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
        accounts_dir: env("TJOMMIE_ACCOUNTS_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    })
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
}

impl Daemon {
    pub fn port(&self) -> u16 {
        self.server.port()
    }

    /// The data root this daemon uses.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn close(self) {
        self.server.close().await;
    }
}

/// `startDaemon`, as far as T3.1 goes: check the tokens, hold the port, settle the token file, then answer.
/// T3.2 and later add the database and the handlers.
pub async fn start(launch: &Launch, auth: &Auth) -> Result<Daemon, StartupError> {
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
    let root = paths::data_root();
    let (token, context) = match open_data(launch, auth, &root, &server) {
        Ok(opened) => opened,
        Err(error) => {
            server.close().await;
            return Err(error);
        }
    };
    server
        .activate(token, context::handlers(&context))
        .map_err(|error| plain(error.message))?;
    Ok(Daemon { server, root })
}

/// `initializeDaemon` up to `activate`, as far as the port has come. T3.4 adds the accounts and M4 the team and
/// scheduler; the runner replaces [`StoredState`] once T3.4 gives it a CLI session factory.
fn open_data(
    launch: &Launch,
    auth: &Auth,
    root: &Path,
    server: &Server,
) -> Result<(String, Context), StartupError> {
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
    let db = db::open(&paths.db)?;
    let token = match auth {
        Auth::Production { token, .. } => token.clone(),
        Auth::Development => paths::read_or_create_token(&paths.token)?,
    };
    bots::sweep_deleted_bot_folders(&db, &paths.bots);
    let exe = std::env::current_exe()?;
    let context = Context {
        db: Arc::new(Mutex::new(db)),
        bots_dir: paths.bots,
        events: server.broadcaster(),
        bot_view: Arc::new(StoredState),
        // Beside the exe, as `build-info.json` sat beside the Node daemon's bundle.
        app_version: exe.parent().and_then(diagnostics::read_app_version),
    };
    Ok((token, context))
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
