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

/// A database file in `dir` with the tables T3.2 uses.
/// TODO(lane A merge): open it with daemon::db::open instead, and delete schema.sql.
pub fn test_db(dir: &std::path::Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection
        .execute_batch(include_str!("schema.sql"))
        .unwrap();
    connection
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

/// Denies this user deleting `file` and the children of its folder, so a recursive removal fails partway while the
/// folders above it can still be renamed. Undo it with `allow_delete`.
pub fn deny_delete(file: &std::path::Path) {
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
}

pub fn allow_delete(file: &std::path::Path) {
    let user = std::env::var("USERNAME").unwrap();
    run(
        "icacls",
        &[
            file.parent().unwrap().as_os_str(),
            "/remove:d".as_ref(),
            user.as_ref(),
        ],
    );
    run(
        "icacls",
        &[file.as_os_str(), "/remove:d".as_ref(), user.as_ref()],
    );
}
