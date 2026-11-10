//! `server.ts`: the HTTP and WebSocket server on 127.0.0.1. A client says hello with the token, then sends requests
//! and receives events. Requests are decoded against the protocol manifest before a handler sees them.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use protocol::wire::Method;
use protocol::{ClientMessage, ErrorCode, Event, PROTOCOL_VERSION, Request, ServerMessage};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// An error a handler answers with; the client gets its code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RpcError {}

/// A storage error the caller can't act on, as a thrown error in `server.ts` becomes `internal`.
impl From<rusqlite::Error> for RpcError {
    fn from(error: rusqlite::Error) -> Self {
        RpcError::new(ErrorCode::Internal, error.to_string())
    }
}

impl From<std::io::Error> for RpcError {
    fn from(error: std::io::Error) -> Self {
        RpcError::new(ErrorCode::Internal, error.to_string())
    }
}

/// Sends events to every authenticated client. Handlers keep a clone.
#[derive(Clone)]
pub struct Broadcaster(Arc<Shared>);

impl Broadcaster {
    pub fn send(&self, event: Event) {
        let text = serde_json::to_string(&ServerMessage::Event(event)).expect("events serialize");
        for outbox in self.0.clients.lock().unwrap().values() {
            let _ = outbox.send(Message::Text(text.clone().into()));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// A program on this PC that knows the token.
    Local,
    /// A paired device on the remote transport.
    Device { device_id: String },
}

/// Created by the authenticated transport, never parsed from request params.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub connection_id: String,
    pub principal: Principal,
    /// Cancelled when the connection closes or the server stops.
    pub signal: CancellationToken,
}

type HandlerFuture = Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>>;
type Handler = Arc<dyn Fn(Request, Option<RequestContext>) -> HandlerFuture + Send + Sync>;

/// The methods this daemon answers. A protocol method without a handler answers `unknown_method`.
#[derive(Clone, Default)]
pub struct Handlers(HashMap<&'static str, Handler>);

impl Handlers {
    /// Answers `M` with `handler`, which gets the decoded params and returns `M`'s result type.
    pub fn on<M: Method, Fut>(
        mut self,
        handler: impl Fn(M::Params, Option<RequestContext>) -> Fut + Send + Sync + 'static,
    ) -> Self
    where
        Fut: Future<Output = Result<M::Result, RpcError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: Handler = Arc::new(move |request, context| {
            let handler = handler.clone();
            Box::pin(async move {
                let params = M::take(request)
                    .expect("a request reaches the handler registered under its method's name");
                let result = handler(params, context).await?;
                serde_json::to_value(result)
                    .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))
            })
        });
        self.0.insert(M::NAME, erased);
        self
    }
}

pub struct ServerOptions {
    /// 0 picks a free port.
    pub port: u16,
    pub token: String,
    pub handlers: Handlers,
    /// Browser origins allowed to open the WebSocket. Clients without an Origin header are programs, not pages.
    pub allowed_origins: Vec<String>,
    /// Answer 503 until [`Server::activate`], so the port can be held while the daemon starts.
    pub defer_requests: bool,
}

struct Config {
    token: String,
    handlers: Handlers,
}

struct Gate {
    ready: bool,
    stopping: bool,
    config: Arc<Config>,
}

struct Shared {
    port: u16,
    allowed_origins: Vec<String>,
    gate: RwLock<Gate>,
    /// Authenticated connections, which receive events.
    clients: Mutex<HashMap<u64, mpsc::UnboundedSender<Message>>>,
    next_client: AtomicU64,
    /// The parent of every connection's signal.
    shutdown: CancellationToken,
}

pub struct Server {
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Server {
    /// Binds 127.0.0.1. The bound port is the daemon's lock: a second daemon fails here.
    pub async fn start(options: ServerOptions) -> std::io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", options.port)).await?;
        let shared = Arc::new(Shared {
            port: listener.local_addr()?.port(),
            allowed_origins: options.allowed_origins,
            gate: RwLock::new(Gate {
                ready: !options.defer_requests,
                stopping: false,
                config: Arc::new(Config {
                    token: options.token,
                    handlers: options.handlers,
                }),
            }),
            clients: Mutex::default(),
            next_client: AtomicU64::new(0),
            shutdown: CancellationToken::new(),
        });
        let app = Router::new().fallback(serve).with_state(shared.clone());
        let stopped = shared.shutdown.clone().cancelled_owned();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(stopped)
                .await;
        });
        Ok(Server { shared, task })
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// Sends an event to every authenticated client.
    pub fn broadcast(&self, event: Event) {
        self.broadcaster().send(event);
    }

    /// For handlers, which are built before [`Server::activate`] and send events later.
    pub fn broadcaster(&self) -> Broadcaster {
        Broadcaster(self.shared.clone())
    }

    /// Starts answering with this token and these handlers.
    pub fn activate(&self, token: impl Into<String>, handlers: Handlers) -> Result<(), RpcError> {
        let mut gate = self.shared.gate.write().unwrap();
        if gate.stopping {
            return Err(RpcError::new(ErrorCode::BadState, "Tjommie is stopping."));
        }
        gate.config = Arc::new(Config {
            token: token.into(),
            handlers,
        });
        gate.ready = true;
        Ok(())
    }

    /// Refuses new requests, on open sockets and through [`Server::dispatch`], before the transports close.
    pub fn stop_requests(&self) {
        let mut gate = self.shared.gate.write().unwrap();
        gate.stopping = true;
        gate.ready = false;
    }

    /// Calls a method from inside the daemon, without a connection.
    pub async fn dispatch(
        &self,
        method: &str,
        params: Value,
        context: Option<RequestContext>,
    ) -> Result<Value, RpcError> {
        self.shared.call(method, params, context).await
    }

    /// Drops every connection and stops listening.
    pub async fn close(self) {
        self.stop_requests();
        self.shared.shutdown.cancel();
        let _ = self.task.await;
    }
}

impl Shared {
    async fn call(
        &self,
        method: &str,
        params: Value,
        context: Option<RequestContext>,
    ) -> Result<Value, RpcError> {
        let handler = {
            let gate = self.gate.read().unwrap();
            if !gate.ready {
                let message = if gate.stopping {
                    "Tjommie is stopping."
                } else {
                    "Tjommie is starting."
                };
                return Err(RpcError::new(ErrorCode::BadState, message));
            }
            if context
                .as_ref()
                .is_some_and(|context| context.signal.is_cancelled())
            {
                return Err(RpcError::new(
                    ErrorCode::BadState,
                    "This connection is closed.",
                ));
            }
            gate.config.handlers.0.get(method).cloned()
        };
        let handler = handler.ok_or_else(|| {
            RpcError::new(ErrorCode::UnknownMethod, format!("Unknown method {method}"))
        })?;
        let request = Request::decode(method, params)
            .map_err(|error| RpcError::new(error.code(), error.to_string()))?;
        // A panicking handler answers `internal` instead of leaving the call unanswered.
        tokio::spawn(async move { handler(request, context).await })
            .await
            .unwrap_or_else(|_| {
                Err(RpcError::new(
                    ErrorCode::Internal,
                    "The request failed unexpectedly.",
                ))
            })
    }

    fn token_matches(&self, given: &str) -> bool {
        same_token(given, &self.gate.read().unwrap().config.token)
    }

    /// Blocks DNS rebinding: the Host header must name this machine's loopback on our port.
    fn local_host(&self, headers: &HeaderMap) -> bool {
        let host = headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok());
        host == Some(&format!("127.0.0.1:{}", self.port))
            || host == Some(&format!("localhost:{}", self.port))
    }

    /// Browsers always send Origin, so a web page can't skip this check. Programs send none. Two Origin headers never
    /// match: Node joins them into one value that is in no allow list.
    fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        let mut origins = headers.get_all(header::ORIGIN).iter();
        match (origins.next(), origins.next()) {
            (None, _) => true,
            (Some(_), Some(_)) => false,
            (Some(origin), None) => origin
                .to_str()
                .is_ok_and(|origin| self.allowed_origins.iter().any(|allowed| allowed == origin)),
        }
    }
}

/// Constant-time, so the token can't be guessed from response timing.
fn same_token(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0, |difference, (a, b)| difference | (a ^ b))
            == 0
}

async fn serve(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if !shared.gate.read().unwrap().ready {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Ok(upgrade) = upgrade else {
        if uri
            .path_and_query()
            .is_some_and(|path| path.as_str() == "/health")
        {
            let health = json!({ "ok": true, "protocol": PROTOCOL_VERSION }).to_string();
            return ([(header::CONTENT_TYPE, "application/json")], health).into_response();
        }
        return StatusCode::NOT_FOUND.into_response();
    };
    if !shared.local_host(&headers) || !shared.origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| connection(socket, shared))
}

fn send(outbox: &mpsc::UnboundedSender<Message>, message: &ServerMessage) {
    let _ = outbox.send(Message::Text(
        serde_json::to_string(message)
            .expect("frames serialize")
            .into(),
    ));
}

/// 1009 for a message over the size limit, 1007 for text that isn't UTF-8, 1002 for any other protocol violation.
fn read_error_code(error: axum::Error) -> u16 {
    match error
        .into_inner()
        .downcast::<tungstenite::Error>()
        .map(|error| *error)
    {
        Ok(tungstenite::Error::Capacity(_)) => 1009,
        Ok(tungstenite::Error::Utf8(_)) => 1007,
        _ => 1002,
    }
}

fn connection_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's random source works");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn connection(socket: WebSocket, shared: Arc<Shared>) {
    let context = RequestContext {
        connection_id: connection_id(),
        principal: Principal::Local,
        signal: shared.shutdown.child_token(),
    };
    let (mut sink, mut stream) = socket.split();
    let (outbox, mut queued) = mpsc::unbounded_channel::<Message>();
    let signal = context.signal.clone();
    let writer = tokio::spawn(async move {
        loop {
            // Queued frames first, so a close frame goes out before the connection ends.
            let message = tokio::select! {
                biased;
                message = queued.recv() => message,
                () = signal.cancelled() => None,
            };
            let Some(message) = message else { break };
            if matches!(message, Message::Close(_)) {
                // Sends our close frame. After the client closed first, sending is refused, but closing still
                // flushes the echo tungstenite queued for it.
                let _ = sink.send(message).await;
                let _ = sink.close().await;
                break;
            }
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });
    let close = |code: u16, reason: &str| {
        let _ = outbox.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_owned().into(),
        })));
    };
    let client = shared.next_client.fetch_add(1, Ordering::Relaxed);
    let hello_deadline = tokio::time::sleep(HELLO_TIMEOUT);
    tokio::pin!(hello_deadline);
    let mut authed = false;
    loop {
        let frame = tokio::select! {
            () = context.signal.cancelled() => break,
            () = &mut hello_deadline, if !authed => {
                close(4401, "hello required");
                break;
            }
            frame = stream.next() => frame,
        };
        let text = match frame {
            Some(Ok(Message::Text(text))) => text.to_string(),
            Some(Ok(Message::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
            // RFC 6455 §5.5.1, as ws does: answer the client's close. tungstenite has queued the echo of its frame
            // (its code and reason, or none); the writer flushes it when it closes.
            Some(Ok(Message::Close(frame))) => {
                let _ = outbox.send(Message::Close(frame));
                break;
            }
            None => break,
            Some(Ok(_)) => continue,
            // A bad frame fails the connection with the code ws uses for it.
            Some(Err(error)) => {
                close(read_error_code(error), "");
                break;
            }
        };
        let Ok(raw) = serde_json::from_str::<Value>(&text) else {
            close(4400, "bad message");
            break;
        };
        if !authed
            && !raw
                .get("token")
                .and_then(Value::as_str)
                .is_some_and(|given| shared.token_matches(given))
        {
            close(4401, "unauthorized");
            break;
        }
        let Ok(message) = protocol::parse::<ClientMessage>(raw) else {
            close(4400, "bad message");
            break;
        };
        match message {
            ClientMessage::Hello { protocol, token } if !authed => {
                if !shared.token_matches(&token) {
                    close(4401, "unauthorized");
                    break;
                }
                if protocol != i64::from(PROTOCOL_VERSION) {
                    close(4003, &format!("protocol {PROTOCOL_VERSION} required"));
                    break;
                }
                authed = true;
                shared
                    .clients
                    .lock()
                    .unwrap()
                    .insert(client, outbox.clone());
                send(
                    &outbox,
                    &ServerMessage::Welcome {
                        protocol: PROTOCOL_VERSION,
                    },
                );
            }
            ClientMessage::Req { .. } if !authed => {
                close(4401, "unauthorized");
                break;
            }
            ClientMessage::Req { id, method, params } => {
                let (shared, outbox, context) = (shared.clone(), outbox.clone(), context.clone());
                tokio::spawn(async move {
                    let response = match shared.call(&method, params, Some(context.clone())).await {
                        Ok(result) => protocol::Response::ok(id, result),
                        Err(error) => protocol::Response::error(id, error.code, error.message),
                    };
                    if !context.signal.is_cancelled() {
                        send(&outbox, &ServerMessage::Res(response));
                    }
                });
            }
            // After the hello only requests count.
            ClientMessage::Hello { .. } => {}
        }
    }
    context.signal.cancel();
    shared.clients.lock().unwrap().remove(&client);
    drop(outbox);
    let _ = writer.await;
}
