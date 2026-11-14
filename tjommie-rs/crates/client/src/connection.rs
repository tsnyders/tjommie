use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMethod, ErrorCode, Event, PROTOCOL_VERSION};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::error::{CloseReason, ConnectError, RpcError};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Where a daemon is. T1.3 adds the other PCs (LAN and relay).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The daemon on this PC, at 127.0.0.1.
    Local { port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectOptions {
    /// How long to wait for the welcome after the hello. The TypeScript client waits 5 s.
    pub welcome_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            welcome_timeout: Duration::from_secs(5),
        }
    }
}

/// How many events a receiver may fall behind before it loses the oldest (`RecvError::Lagged`). A receiver that lags has
/// missed state, so its owner must refetch rather than carry on.
const EVENT_BACKLOG: usize = 1024;

/// How long [`Client::close`] waits for the daemon to finish the close handshake.
const GOODBYE: Duration = Duration::from_secs(2);

struct Pending {
    method: &'static str,
    reply: oneshot::Sender<Result<Value, RpcError>>,
}

enum Command {
    Call {
        id: i64,
        method: &'static str,
        text: String,
        reply: oneshot::Sender<Result<Value, RpcError>>,
    },
    Close,
}

/// A connection to one daemon. Cheap to clone; every clone shares the connection, which ends when the daemon closes it,
/// when any clone calls [`Client::close`], or when the last clone is dropped.
#[derive(Clone)]
pub struct Client {
    commands: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<Event>,
    closed: watch::Receiver<Option<CloseReason>>,
    next_id: Arc<AtomicI64>,
}

impl Client {
    /// Opens the WebSocket, says hello with `token` and waits for the welcome.
    pub async fn connect(endpoint: Endpoint, token: &str) -> Result<Client, ConnectError> {
        Self::connect_with(endpoint, token, ConnectOptions::default()).await
    }

    pub async fn connect_with(
        endpoint: Endpoint,
        token: &str,
        options: ConnectOptions,
    ) -> Result<Client, ConnectError> {
        let socket = open(&endpoint, token, options).await?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(EVENT_BACKLOG);
        let (closed_sender, closed) = watch::channel(None);
        tokio::spawn(run(socket, receiver, events.clone(), closed_sender));
        Ok(Client {
            commands,
            events,
            closed,
            next_id: Arc::new(AtomicI64::new(1)),
        })
    }

    /// Sends one request and returns its result. Calls may overlap.
    pub async fn call<M: ClientMethod>(&self, params: M::Params) -> Result<M::Result, RpcError> {
        let params =
            serde_json::to_value(params).map_err(|error| RpcError::BadParams(error.to_string()))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let text =
            json!({ "type": "req", "id": id, "method": M::NAME, "params": params }).to_string();
        let (reply, answer) = oneshot::channel();
        let call = Command::Call {
            id,
            method: M::NAME,
            text,
            reply,
        };
        if self.commands.send(call).is_err() {
            return Err(RpcError::Closed(self.reason()));
        }
        let result = match answer.await {
            Ok(result) => result?,
            Err(_) => return Err(RpcError::Closed(self.reason())),
        };
        serde_json::from_value(result).map_err(|error| RpcError::BadResult(error.to_string()))
    }

    /// A receiver of every event the daemon pushes from now on, in order.
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Resolves, once, when the connection has ended, with why.
    pub fn closed(&self) -> impl Future<Output = CloseReason> + Send + 'static {
        let mut closed = self.closed.clone();
        async move {
            loop {
                let current = closed.borrow_and_update().clone();
                if let Some(reason) = current {
                    return reason;
                }
                if closed.changed().await.is_err() {
                    return CloseReason::new(1006, "The connection ended", true);
                }
            }
        }
    }

    /// Says goodbye (1000) and waits for the connection to end. Calls still waiting fail with [`RpcError::Closed`].
    pub async fn close(&self) {
        let _ = self.commands.send(Command::Close);
        self.closed().await;
    }

    fn reason(&self) -> CloseReason {
        self.closed
            .borrow()
            .clone()
            .unwrap_or_else(|| CloseReason::new(1006, "The connection is closed", true))
    }
}

async fn open(
    endpoint: &Endpoint,
    token: &str,
    options: ConnectOptions,
) -> Result<Socket, ConnectError> {
    let Endpoint::Local { port } = endpoint;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
        .await
        .map_err(|error| match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                ConnectError::Refused(response.status().as_u16())
            }
            other => ConnectError::Unreachable(other.to_string()),
        })?;
    let hello = json!({ "type": "hello", "protocol": PROTOCOL_VERSION, "token": token });
    if let Err(error) = socket.send(Message::text(hello.to_string())).await {
        return Err(ConnectError::Unreachable(error.to_string()));
    }
    match tokio::time::timeout(options.welcome_timeout, welcomed(&mut socket)).await {
        Ok(Ok(())) => Ok(socket),
        Ok(Err(error)) => Err(error),
        Err(_) => {
            let _ = socket.close(None).await;
            Err(ConnectError::TimedOut)
        }
    }
}

/// Waits for the welcome. The daemon says why it refuses with a close code: 4401 (token) or 4003 (protocol).
async fn welcomed(socket: &mut Socket) -> Result<(), ConnectError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let frame: Option<Value> = serde_json::from_str(&text).ok();
                let welcome = frame.as_ref().is_some_and(|frame| {
                    frame.get("type").and_then(Value::as_str) == Some("welcome")
                        && frame.get("protocol").and_then(Value::as_u64)
                            == Some(u64::from(PROTOCOL_VERSION))
                });
                if welcome {
                    return Ok(());
                }
                hang_up(socket, 4400, "Invalid server frame").await;
                return Err(ConnectError::BadFrame(
                    if frame.is_some() {
                        "Unexpected welcome protocol"
                    } else {
                        "Could not parse message"
                    }
                    .to_owned(),
                ));
            }
            Some(Ok(Message::Close(frame))) => {
                let reason = close_reason(frame, false);
                return Err(match reason.code {
                    4401 => ConnectError::Unauthorized,
                    4003 => ConnectError::ProtocolMismatch,
                    _ => ConnectError::Closed(reason),
                });
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(_)) => {
                hang_up(socket, 4400, "Invalid server frame").await;
                return Err(ConnectError::BadFrame("Expected a text frame".to_owned()));
            }
            Some(Err(error)) => {
                return Err(ConnectError::Closed(CloseReason::new(
                    1006,
                    error.to_string(),
                    false,
                )));
            }
            None => {
                return Err(ConnectError::Closed(CloseReason::new(
                    1006,
                    "The connection ended",
                    false,
                )));
            }
        }
    }
}

fn close_reason(frame: Option<CloseFrame>, opened: bool) -> CloseReason {
    match frame {
        Some(frame) => CloseReason::new(u16::from(frame.code), frame.reason.to_string(), opened),
        None => CloseReason::new(1005, "", opened),
    }
}

async fn hang_up(socket: &mut Socket, code: u16, reason: &str) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: reason.to_owned().into(),
    };
    let _ = socket.close(Some(frame)).await;
}

/// The connection's own task: it owns the socket, writes requests, and routes what arrives. It ends the connection when
/// anything goes wrong, so the caller can't be left waiting on a dead socket.
async fn run(
    mut socket: Socket,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: broadcast::Sender<Event>,
    closed: watch::Sender<Option<CloseReason>>,
) {
    let mut pending: HashMap<i64, Pending> = HashMap::new();
    let reason = loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Call { id, method, text, reply }) => {
                    pending.insert(id, Pending { method, reply });
                    if let Err(error) = socket.send(Message::text(text)).await {
                        break CloseReason::new(1006, error.to_string(), true);
                    }
                }
                Some(Command::Close) | None => {
                    hang_up(&mut socket, 1000, "Client closed").await;
                    finish_handshake(&mut socket).await;
                    break CloseReason::new(1000, "Client closed", true);
                }
            },
            frame = socket.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if let Err(why) = route(&text, &mut pending, &events) {
                        let reason = format!("Invalid server frame: {why}");
                        hang_up(&mut socket, 4400, "Invalid server frame").await;
                        break CloseReason::new(4400, reason, true);
                    }
                }
                Some(Ok(Message::Close(frame))) => break close_reason(frame, true),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(_)) => {
                    hang_up(&mut socket, 4400, "Invalid server frame").await;
                    break CloseReason::new(4400, "Invalid server frame: Expected a text frame", true);
                }
                Some(Err(error)) => break CloseReason::new(1006, error.to_string(), true),
                None => break CloseReason::new(1006, "The connection ended", true),
            },
        }
    };
    // Set before the calls hear about it, so a caller that wakes up reads the same reason.
    closed.send_replace(Some(reason.clone()));
    for (_, call) in pending.drain() {
        let _ = call.reply.send(Err(RpcError::Closed(reason.clone())));
    }
}

/// After our close frame, reads until the daemon's reply (or a short while), so it sees a clean close.
async fn finish_handshake(socket: &mut Socket) {
    let _ = tokio::time::timeout(GOODBYE, async {
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
}

/// Hands one frame to the call or the receivers it is for. An `Err` is a frame the client can't accept, and ends the connection.
fn route(
    text: &str,
    pending: &mut HashMap<i64, Pending>,
    events: &broadcast::Sender<Event>,
) -> Result<(), String> {
    let frame: Value =
        serde_json::from_str(text).map_err(|_| "Could not parse message".to_owned())?;
    let Some(envelope) = frame.as_object() else {
        return Err("Expected an envelope".to_owned());
    };
    match envelope.get("type").and_then(Value::as_str) {
        Some("res") => {
            let id = envelope.get("id").and_then(Value::as_i64);
            let ok = envelope.get("ok").and_then(Value::as_bool);
            let (Some(id), Some(ok)) = (id, ok) else {
                return Err("Invalid response envelope".to_owned());
            };
            if ok {
                let result = envelope
                    .get("result")
                    .ok_or_else(|| "Missing response result".to_owned())?;
                // An answer to nothing we asked is ignored, like the TypeScript client.
                if let Some(call) = pending.get(&id) {
                    protocol::validate(call.method, result)
                        .map_err(|invalid| format!("{}: {invalid}", call.method))?;
                    if let Some(call) = pending.remove(&id) {
                        let _ = call.reply.send(Ok(result.clone()));
                    }
                }
            } else {
                let error = envelope.get("error").and_then(Value::as_object);
                let code = error
                    .and_then(|error| error.get("code"))
                    .and_then(|code| serde_json::from_value::<ErrorCode>(code.clone()).ok());
                let message = error
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str);
                let (Some(code), Some(message)) = (code, message) else {
                    return Err("Invalid response error".to_owned());
                };
                if let Some(call) = pending.remove(&id) {
                    let _ = call.reply.send(Err(RpcError::Daemon {
                        code,
                        message: message.to_owned(),
                    }));
                }
            }
            Ok(())
        }
        Some("event") => {
            let name = envelope.get("event").and_then(Value::as_str);
            let data = envelope.get("data");
            let (Some(name), Some(data)) = (name, data) else {
                return Err("Invalid event envelope".to_owned());
            };
            protocol::validate(name, data).map_err(|invalid| format!("{name}: {invalid}"))?;
            let event: Event = serde_json::from_value(frame.clone())
                .map_err(|error| format!("{name}: {error}"))?;
            let _ = events.send(event); // no receivers is fine
            Ok(())
        }
        _ => Err("Unknown envelope type".to_owned()),
    }
}
