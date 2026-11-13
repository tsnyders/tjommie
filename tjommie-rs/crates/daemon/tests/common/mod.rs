//! Helpers shared by the daemon's integration tests: a WebSocket client that checks what the daemon sends against the
//! protocol manifest, raw HTTP and WebSocket probes, temporary data roots and the daemon executable as a child process.
#![allow(dead_code)] // Each test binary uses part of this.

use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const WAIT: Duration = Duration::from_secs(20);

pub struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Events that arrived while `call` waited for its response, in order.
    pub events: Vec<Value>,
}

pub async fn connect(port: u16) -> Client {
    try_connect(port, &[])
        .await
        .expect("the daemon accepts the WebSocket")
}

pub async fn try_connect(port: u16, headers: &[(&str, &str)]) -> Result<Client, Error> {
    let mut request = format!("ws://127.0.0.1:{port}/").into_client_request()?;
    let mut named = std::collections::HashSet::new();
    for (name, value) in headers {
        let (name, value) = (
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        // The first value replaces the client's own (Host); a repeated name adds a second header line.
        if named.insert(name.clone()) {
            request.headers_mut().insert(name, value);
        } else {
            request.headers_mut().append(name, value);
        }
    }
    let (ws, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(Client {
        ws,
        events: Vec::new(),
    })
}

/// The HTTP status of a refused WebSocket upgrade.
pub fn refused(result: Result<Client, Error>) -> u16 {
    match result {
        Err(Error::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("expected an HTTP refusal, got {other}"),
        Ok(_) => panic!("expected the upgrade to be refused"),
    }
}

impl Client {
    pub async fn send(&mut self, value: Value) {
        self.send_text(&value.to_string()).await;
    }

    pub async fn send_text(&mut self, text: &str) {
        self.ws
            .send(Message::text(text))
            .await
            .expect("the frame is sent");
    }

    /// The next frame. Event data is checked against the manifest.
    pub async fn next(&mut self) -> Value {
        loop {
            let frame = timeout(WAIT, self.ws.next())
                .await
                .expect("a frame arrives in time");
            match frame {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).expect("frames are JSON");
                    if value["type"] == "event" {
                        protocol::assert_valid(value["event"].as_str().unwrap(), &value["data"]);
                    }
                    return value;
                }
                Some(Ok(Message::Close(frame))) => {
                    panic!("closed while waiting for a frame: {frame:?}")
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => panic!("WebSocket error while waiting for a frame: {error}"),
                None => panic!("the stream ended while waiting for a frame"),
            }
        }
    }

    pub async fn hello(&mut self, token: &str) -> Value {
        self.send(
            json!({ "type": "hello", "protocol": protocol::PROTOCOL_VERSION, "token": token }),
        )
        .await;
        self.next().await
    }

    /// Sends a request and returns its `res` frame, keeping events that arrive first in `events`. A successful result
    /// is checked against the method's manifest schema.
    pub async fn call(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({ "type": "req", "id": id, "method": method, "params": params }))
            .await;
        let response = loop {
            let frame = self.next().await;
            if frame["type"] != "event" {
                break frame;
            }
            self.events.push(frame);
        };
        assert_eq!(response["id"], id, "{response}");
        if response["ok"] == true {
            protocol::assert_valid(method, &response["result"]);
        }
        response
    }

    /// The close code and reason the server ends the socket with, or `None` when the connection just drops.
    pub async fn closed(mut self) -> Option<(u16, String)> {
        loop {
            match timeout(WAIT, self.ws.next())
                .await
                .expect("the socket closes in time")
            {
                Some(Ok(Message::Close(frame))) => {
                    return frame.map(|frame| (u16::from(frame.code), frame.reason.to_string()));
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return None,
            }
        }
    }

    pub async fn close(mut self) {
        self.ws.close(None).await.expect("the close frame is sent");
    }

    /// Closes first and returns the server's reply: `Some(None)` for a close frame without a code, `None` when the
    /// connection just drops.
    pub async fn close_with(mut self, frame: Option<(u16, &str)>) -> Option<Option<(u16, String)>> {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let frame = frame.map(|(code, reason)| CloseFrame {
            code: code.into(),
            reason: reason.to_owned().into(),
        });
        self.ws
            .send(Message::Close(frame))
            .await
            .expect("the close frame is sent");
        loop {
            match timeout(WAIT, self.ws.next())
                .await
                .expect("the reply arrives in time")
            {
                Some(Ok(Message::Close(frame))) => {
                    return Some(
                        frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())),
                    );
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return None,
            }
        }
    }

    /// The next frame if one arrives within `wait`.
    pub async fn try_next(&mut self, wait: Duration) -> Option<Value> {
        match timeout(wait, self.ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                Some(serde_json::from_str(&text).expect("frames are JSON"))
            }
            _ => None,
        }
    }
}

/// A plain HTTP GET: the status and the body.
pub async fn http_get(port: u16, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    timeout(WAIT, stream.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    let status = response[9..12].parse().unwrap();
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// An unmasked text frame, which clients must never send.
pub const UNMASKED_FRAME: &[u8] = &[0x81, 0x01, b'x'];

/// A masked text frame whose payload, 0xFF 0xFE, isn't UTF-8.
pub const INVALID_UTF8_FRAME: &[u8] = &[0x81, 0x82, 1, 2, 3, 4, 0xff ^ 1, 0xfe ^ 2];

/// The header of a masked text frame announcing 16 MiB + 1 bytes, one more than the server takes. The payload isn't
/// sent: unread payload would make the server's socket close with a reset that can discard its close frame.
pub const OVERSIZED_FRAME_HEADER: &[u8] =
    &[0x81, 0xff, 0, 0, 0, 0, 0x01, 0x00, 0x00, 0x01, 1, 2, 3, 4];

/// Opens a WebSocket by hand and sends one raw frame. Returns the close code the server answers with, or `None` if it
/// only drops the connection.
pub async fn send_raw_frame(port: u16, raw: &[u8]) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let handshake = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(handshake.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    assert!(
        head.starts_with(b"HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    stream.write_all(raw).await.unwrap();
    let mut frame = [0; 4];
    match timeout(WAIT, stream.read_exact(&mut frame)).await.unwrap() {
        Ok(_) if frame[0] == 0x88 && frame[1] >= 2 => {
            Some(u16::from_be_bytes([frame[2], frame[3]]))
        }
        _ => None,
    }
}

/// A data root under the temp folder, removed afterwards.
pub struct TempRoot(pub PathBuf);

impl TempRoot {
    pub fn new() -> Self {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("tjommie-rs-test-{name}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `tjommie-daemon.exe` on a temporary data root and a free port, started through `spawn::command`.
pub struct Daemon {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Arc<Mutex<String>>,
    err: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
    pub root: TempRoot,
}

pub fn launch(args: &[&str], root: TempRoot, env: &[(&str, &str)]) -> Daemon {
    let exe = env!("CARGO_BIN_EXE_tjommie-daemon");
    let mut command =
        tokio::process::Command::from(std::process::Command::from(daemon::spawn::command(exe)));
    command
        .args(args)
        .env("TJOMMIE_HOME", &root.0)
        .env("TJOMMIE_PORT", "0");
    for key in ["TJOMMIE_DEV", "TJOMMIE_ACCOUNTS_DIR", "TJOMMIE_LAN_PORT"] {
        command.env_remove(key);
    }
    // Never the production 0.0.0.0 bind on this PC: it would raise a Windows Firewall prompt. Loopback, and an
    // ephemeral port in case a test pairs a device.
    command.env("TJOMMIE_LAN_BIND", "127.0.0.1");
    // Nor a real Claude or Codex: the daemon probes the provider CLIs at start unless told not to.
    command.env("TJOMMIE_SKIP_ACCOUNT_CHECK", "1");
    // Nor the owner's real `~/.codex/.sandbox`: the Codex data ACL pass reads and changes it at every start.
    command.env("TJOMMIE_SKIP_FOLDER_PROTECTION", "1");
    command.env("TJOMMIE_LAN_PORT", "0");
    command.envs(env.iter().copied());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().expect("the daemon starts");
    let (out, err) = (
        Arc::new(Mutex::new(String::new())),
        Arc::new(Mutex::new(String::new())),
    );
    let readers = vec![
        collect(child.stdout.take().unwrap(), out.clone()),
        collect(child.stderr.take().unwrap(), err.clone()),
    ];
    Daemon {
        stdin: child.stdin.take(),
        child,
        out,
        err,
        readers,
        root,
    }
}

fn collect(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    into: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0; 4096];
        while let Ok(read) = pipe.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            into.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
    })
}

pub const LISTENING: &str = "Tjommie daemon listening on http://127.0.0.1:";

impl Daemon {
    pub fn stdout(&self) -> String {
        self.out.lock().unwrap().clone()
    }

    pub fn stderr(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    /// The port from the listening line.
    pub async fn ready(&self) -> u16 {
        let port = async {
            loop {
                let out = self.stdout();
                if let Some(port) = out
                    .split_once(LISTENING)
                    .and_then(|(_, rest)| rest.lines().next()?.parse().ok())
                {
                    return port;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        timeout(WAIT, port).await.unwrap_or_else(|_| {
            panic!(
                "not listening\nstdout: {}\nstderr: {}",
                self.stdout(),
                self.stderr()
            )
        })
    }

    pub async fn write(&mut self, text: &str) {
        self.stdin
            .as_mut()
            .unwrap()
            .write_all(text.as_bytes())
            .await
            .unwrap();
    }

    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    pub fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    /// Waits for the exit, then for the rest of its output.
    pub async fn exited(&mut self) -> ExitStatus {
        let status = timeout(WAIT, self.child.wait())
            .await
            .expect("the daemon exits in time")
            .unwrap();
        for reader in self.readers.drain(..) {
            reader.await.unwrap();
        }
        status
    }

    pub async fn kill(&mut self) {
        self.child.kill().await.unwrap();
        self.exited().await;
    }
}

/// A fresh production token: 32 random bytes as 43 base64url characters.
pub fn token() -> String {
    daemon::paths::new_token()
}

/// `openDb(join(root, "t.db"))`: a migrated database file in `dir`, as the TypeScript tests open theirs.
pub fn test_db(dir: &std::path::Path) -> rusqlite::Connection {
    daemon::db::open(&dir.join("t.db")).unwrap()
}

/// An account row as accounts-store.ts writes it. The first account of a provider is its default.
pub fn insert_account(
    connection: &rusqlite::Connection,
    id: &str,
    provider: &str,
    folder: &std::path::Path,
) {
    let created = connection
        .query_row("SELECT COUNT(*) FROM accounts", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    connection
        .execute(
            "INSERT INTO accounts (id, provider, label, kind, folder, created_at, is_default)
             VALUES (?1, ?2, ?1, 'subscription', ?3, ?4, NOT EXISTS (SELECT 1 FROM accounts WHERE provider = ?2 AND is_default = 1))",
            rusqlite::params![id, provider, folder.display().to_string(), format!("2026-01-01T00:00:{created:02}.000Z")],
        )
        .unwrap();
}

/// accounts-store.ts updateAccount(db, id, { isDefault: true }).
pub fn make_default_account(connection: &rusqlite::Connection, id: &str) {
    connection
        .execute_batch(&format!(
            "UPDATE accounts SET is_default = 0 WHERE provider = (SELECT provider FROM accounts WHERE id = '{id}');
             UPDATE accounts SET is_default = 1 WHERE id = '{id}';"
        ))
        .unwrap();
}

fn run(program: &str, args: &[&std::ffi::OsStr]) {
    let status = std::process::Command::from(daemon::spawn::command(program))
        .args(args)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{program} {args:?} failed");
}

/// A directory junction, as `symlinkSync(target, link, "junction")` makes it.
pub fn junction(target: &std::path::Path, link: &std::path::Path) {
    run(
        "cmd",
        &[
            "/c".as_ref(),
            "mklink".as_ref(),
            "/J".as_ref(),
            link.as_os_str(),
            target.as_os_str(),
        ],
    );
}

/// Denies this user deleting a file and the children of its folder, so a recursive removal fails partway while the
/// folders above it can still be renamed. Dropping the guard lifts the denial.
pub struct DenyDelete {
    home: PathBuf,
    relative: PathBuf,
}

impl DenyDelete {
    /// Denies deleting `home/relative` and the children of its folder.
    pub fn new(home: &std::path::Path, relative: &str) -> Self {
        let file = home.join(relative);
        let user = std::env::var("USERNAME").unwrap();
        run(
            "icacls",
            &[
                file.as_os_str(),
                "/deny".as_ref(),
                format!("{user}:(D)").as_ref(),
            ],
        );
        run(
            "icacls",
            &[
                file.parent().unwrap().as_os_str(),
                "/deny".as_ref(),
                format!("{user}:(DC)").as_ref(),
            ],
        );
        Self {
            home: home.to_path_buf(),
            relative: PathBuf::from(relative),
        }
    }
}

/// Lifts the denial wherever the file is now: in the home folder, or in a `<home>.deleting-*` folder it was renamed to.
/// Best effort and never panicking, so it also runs while a failed test unwinds and leaves nothing undeletable in %TEMP%.
impl Drop for DenyDelete {
    fn drop(&mut self) {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let name = self
            .home
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let parent = self
            .home
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let renamed = std::fs::read_dir(&parent)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().is_some_and(|file| {
                    file.to_string_lossy()
                        .starts_with(&format!("{name}.deleting-"))
                })
            });
        for home in std::iter::once(self.home.clone()).chain(renamed) {
            let file = home.join(&self.relative);
            if !file.exists() {
                continue;
            }
            for target in [file.parent().unwrap_or(&home), file.as_path()] {
                let _ = std::process::Command::from(daemon::spawn::command("icacls"))
                    .args([target.as_os_str(), "/remove:d".as_ref(), user.as_ref()])
                    .stdout(Stdio::null())
                    .status();
            }
        }
    }
}

/// `createBot` with the fields the store tests don't care about: blue, a circle, no instructions.
pub fn new_bot(
    db: &rusqlite::Connection,
    bots_dir: &std::path::Path,
    name: &str,
    provider: &str,
) -> daemon::bots::StoredBot {
    let model = if provider == "codex" {
        "gpt-5.6-sol"
    } else {
        "sonnet"
    };
    let input = json!({ "name": name, "color": "blue", "shape": "circle", "instructions": "", "provider": provider, "model": model });
    daemon::bots::create_bot(db, bots_dir, &serde_json::from_value(input).unwrap()).unwrap()
}

/// A CLI host that finds no provider CLI, so nothing reaches a real Claude or Codex.
pub struct NoCli;

impl daemon::cli::CliHost for NoCli {
    fn launch<'a>(
        &'a self,
        _cmd: &'a daemon::cli::CliCommand,
        _args: Vec<String>,
        _options: daemon::cli::LaunchOptions,
    ) -> daemon::cli::BoxFuture<'a, std::io::Result<daemon::cli::CliChild>> {
        Box::pin(async { Err(std::io::ErrorKind::NotFound.into()) })
    }
}

/// Usage reads without a network.
pub struct NoFetch;

impl daemon::account_quota::UsageFetch for NoFetch {
    fn get<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(&'static str, String)],
        _timeout: Duration,
    ) -> daemon::cli::BoxFuture<'a, Result<daemon::account_quota::UsageResponse, String>> {
        Box::pin(async { Err("no network in tests".to_string()) })
    }
}

pub fn no_cli_setup() -> daemon::setup::Setup {
    daemon::setup::Setup::new(Arc::new(NoCli), Arc::new(NoFetch))
}

/// API keys in memory instead of Credential Manager.
#[derive(Default)]
pub struct MemoryKeys(pub Mutex<std::collections::HashMap<String, String>>);

impl daemon::account_runtime::CredentialStore for MemoryKeys {
    fn reference(&self, account_id: &str) -> Result<String, daemon::server::RpcError> {
        Ok(format!("synthetic/{account_id}"))
    }

    fn read(
        &self,
        account_id: &str,
    ) -> daemon::cli::BoxFuture<'_, Result<Option<String>, daemon::server::RpcError>> {
        let key = self.0.lock().unwrap().get(account_id).cloned();
        Box::pin(async move { Ok(key) })
    }

    fn write(
        &self,
        account_id: &str,
        key: &str,
    ) -> daemon::cli::BoxFuture<'_, Result<(), daemon::server::RpcError>> {
        self.0.lock().unwrap().insert(account_id.into(), key.into());
        Box::pin(async { Ok(()) })
    }

    fn delete(
        &self,
        account_id: &str,
    ) -> daemon::cli::BoxFuture<'_, Result<(), daemon::server::RpcError>> {
        self.0.lock().unwrap().remove(account_id);
        Box::pin(async { Ok(()) })
    }
}

/// The handlers' context over a data root as startup prepares it: each provider's main account exists. Accounts use
/// `ops` and `credentials`.
pub fn test_context(
    root: &std::path::Path,
    events: daemon::server::Broadcaster,
    ops: Arc<dyn daemon::account_runtime::AccountOps>,
    credentials: Arc<dyn daemon::account_runtime::CredentialStore>,
) -> daemon::context::Context {
    use daemon::account_handlers::{Accounts, AccountsOptions};
    use daemon::context::{BotView, StoredState};
    let db = Arc::new(Mutex::new(test_db(root)));
    let setup = no_cli_setup();
    let accounts_dir = root.join("accounts");
    setup
        .ensure_accounts(&db.lock().unwrap(), &accounts_dir, root)
        .unwrap();
    let bot_view: Arc<dyn BotView> = Arc::new(StoredState);
    let accounts = Accounts::new(AccountsOptions {
        db: db.clone(),
        root: root.to_path_buf(),
        accounts_dir,
        events: events.clone(),
        bot_view: bot_view.clone(),
        setup,
        ops,
        credentials,
    });
    daemon::context::Context {
        db,
        bots_dir: root.join("bots"),
        events,
        bot_view,
        app_version: None,
        accounts,
        late: daemon::context::Late::default(),
    }
}

/// Healthy providers, as the TypeScript integration fixtures' probe reports them: installed, signed in, Codex's
/// sandbox ready. Account folders are made on demand, and `account_env` names the folder as the CLI's config home,
/// as `Setup::account_env` does.
#[derive(Default)]
pub struct HealthyOps;

impl daemon::account_runtime::AccountOps for HealthyOps {
    fn check(
        &self,
        account: &daemon::setup::Account,
    ) -> daemon::cli::BoxFuture<'_, protocol::model::ProviderSetup> {
        use protocol::model::{
            CodexSandboxReadiness, CodexSetup, Provider, ProviderSetup, SetupFields,
        };
        let fields = |version: &str| SetupFields {
            installed: true,
            version: Some(version.into()),
            signed_in: true,
            account: Some(format!("{}@example.test", account.id)),
            plan: Some("synthetic".into()),
            error: None,
        };
        let setup = match account.provider {
            Provider::Claude => ProviderSetup::Claude(fields("2.1.304")),
            Provider::Codex => ProviderSetup::Codex(CodexSetup {
                setup: fields("0.160.0"),
                sandbox: CodexSandboxReadiness::Ready,
            }),
        };
        Box::pin(async move { setup })
    }

    fn check_usage(
        &self,
        _: &daemon::setup::Account,
        _: &std::path::Path,
    ) -> daemon::cli::BoxFuture<'_, daemon::account_quota::AccountUsageRead> {
        Box::pin(async { daemon::account_quota::AccountUsageRead::Unavailable { error: None } })
    }

    fn sign_in(
        &self,
        _: &daemon::setup::Account,
        _: daemon::setup::OnDone,
    ) -> daemon::cli::BoxFuture<'_, Result<protocol::accounts::SignInStart, daemon::server::RpcError>>
    {
        Box::pin(async {
            Ok(protocol::accounts::SignInStart {
                url: None,
                attempt: "synthetic-attempt".into(),
            })
        })
    }

    fn cancel_sign_in(
        &self,
        _: &daemon::setup::Account,
        _: &str,
    ) -> daemon::cli::BoxFuture<'_, Result<(), daemon::server::RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_sandbox(
        &self,
        _: &daemon::setup::Account,
    ) -> daemon::cli::BoxFuture<'_, Result<(), daemon::server::RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn logout(
        &self,
        _: &daemon::setup::Account,
    ) -> daemon::cli::BoxFuture<'_, Result<(), daemon::server::RpcError>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_folder(
        &self,
        account: &daemon::setup::Account,
        _: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(&account.folder)
    }

    fn account_env(&self, account: &daemon::setup::Account) -> daemon::cli::Env {
        let key = if account.provider == protocol::model::Provider::Claude {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        daemon::cli::Env::from([(key.to_owned(), account.folder.clone())])
    }
}
