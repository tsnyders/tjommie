//! `peer-wire.ts`: how a remote device's Noise frames reach the host. LAN devices use TCP with a 4-byte length prefix;
//! relay devices use one binary WebSocket message per frame. Either way the host sees a [`PeerWire`]: frames in, frames
//! out, and a close that is final. No frame is handed out after the wire was destroyed.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use transport::MAX_NOISE_FRAME;
use transport::channel::{FrameDecoder, length_prefixed};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireKind {
    Lan,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

impl fmt::Display for Closed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Connection closed.")
    }
}

impl std::error::Error for Closed {}

#[derive(Clone)]
enum Sink {
    Frames(mpsc::UnboundedSender<Vec<u8>>),
    Socket(mpsc::UnboundedSender<Message>),
}

/// The sending half of a wire. Clones share one connection.
#[derive(Clone)]
pub struct WireWriter {
    sink: Sink,
    buffered: Arc<AtomicUsize>,
    closed: CancellationToken,
}

impl WireWriter {
    /// Queues one frame. A bad size or a closed wire destroys the wire.
    pub fn write(&self, frame: Vec<u8>) -> Result<(), Closed> {
        if self.is_closed() || frame.is_empty() || frame.len() > MAX_NOISE_FRAME {
            self.destroy();
            return Err(Closed);
        }
        let length = frame.len();
        self.buffered.fetch_add(length, Ordering::Relaxed);
        let sent = match &self.sink {
            Sink::Frames(sink) => sink.send(frame).is_ok(),
            Sink::Socket(sink) => sink.send(Message::binary(frame)).is_ok(),
        };
        if !sent {
            self.destroy();
            return Err(Closed);
        }
        Ok(())
    }

    /// Bytes written but not yet handed to the operating system.
    pub fn buffered(&self) -> usize {
        self.buffered.load(Ordering::Relaxed)
    }

    pub fn destroy(&self) {
        self.closed.cancel();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    pub async fn closed(&self) {
        self.closed.cancelled().await;
    }

    /// Cancelled when the wire closes, for work that belongs to this connection.
    pub fn signal(&self) -> CancellationToken {
        self.closed.clone()
    }
}

pub struct PeerWire {
    pub kind: WireKind,
    pub address: String,
    incoming: mpsc::UnboundedReceiver<Vec<u8>>,
    writer: WireWriter,
}

impl PeerWire {
    pub fn writer(&self) -> WireWriter {
        self.writer.clone()
    }

    /// The next frame, or `None` once the wire is closed or destroyed.
    pub async fn next_frame(&mut self) -> Option<Vec<u8>> {
        let frame = tokio::select! {
            biased;
            () = self.writer.closed() => None,
            frame = self.incoming.recv() => frame,
        };
        if frame.is_none() {
            self.writer.destroy();
        }
        frame
    }

    /// `tcpWire`: a LAN connection. A malformed length prefix destroys it.
    pub fn tcp(stream: TcpStream) -> PeerWire {
        let address = stream
            .peer_addr()
            .map_or_else(|_| "unknown".into(), |address| address.ip().to_string());
        let _ = stream.set_nodelay(true);
        let keepalive = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30));
        let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&keepalive);
        let (mut read, mut write) = stream.into_split();
        let (frames, incoming) = mpsc::unbounded_channel();
        let (sink, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        let writer = WireWriter {
            sink: Sink::Frames(sink),
            buffered: Arc::default(),
            closed: CancellationToken::new(),
        };
        let closed = writer.closed.clone();
        tokio::spawn(async move {
            let mut decoder = FrameDecoder::default();
            let mut chunk = vec![0; 64 * 1024];
            loop {
                let read = tokio::select! {
                    biased;
                    () = closed.cancelled() => break,
                    read = read.read(&mut chunk) => read,
                };
                let Ok(length @ 1..) = read else { break };
                let Ok(decoded) = decoder.feed(&chunk[..length]) else {
                    break;
                };
                if decoded.into_iter().any(|frame| frames.send(frame).is_err()) {
                    break;
                }
            }
            closed.cancel();
        });
        let (closed, buffered) = (writer.closed.clone(), writer.buffered.clone());
        tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    biased;
                    () = closed.cancelled() => break,
                    frame = outgoing.recv() => frame,
                };
                let Some(frame) = frame else { break };
                let Ok(bytes) = length_prefixed(&frame) else {
                    break;
                };
                if write.write_all(&bytes).await.is_err() {
                    break;
                }
                buffered.fetch_sub(frame.len(), Ordering::Relaxed);
            }
            closed.cancel();
            // Dropping both halves closes the socket.
        });
        PeerWire {
            kind: WireKind::Lan,
            address,
            incoming,
            writer,
        }
    }

    /// `webSocketWire`: a relay channel. Text, empty or oversized messages destroy it.
    pub fn websocket(socket: Socket) -> PeerWire {
        let Socket {
            tx,
            mut rx,
            buffered,
            closed,
        } = socket;
        let (frames, incoming) = mpsc::unbounded_channel();
        let watch = closed.clone();
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    biased;
                    () = watch.cancelled() => break,
                    message = rx.recv() => message,
                };
                match message {
                    Some(Message::Binary(bytes))
                        if !bytes.is_empty() && bytes.len() <= MAX_NOISE_FRAME =>
                    {
                        if frames.send(bytes.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(Message::Ping(_) | Message::Pong(_)) => {}
                    _ => break,
                }
            }
            watch.cancel();
        });
        PeerWire {
            kind: WireKind::Relay,
            address: "relay".into(),
            incoming,
            writer: WireWriter {
                sink: Sink::Socket(tx),
                buffered,
                closed,
            },
        }
    }

    /// Two wires joined in this process: what one writes the other reads, and closing either closes both. The
    /// TypeScript tests' `relayWire`, with the host on the first wire and a device on the second.
    pub fn memory(kind: WireKind, address: &str) -> (PeerWire, PeerWire) {
        let (to_host, from_device) = mpsc::unbounded_channel();
        let (to_device, from_host) = mpsc::unbounded_channel();
        let closed = CancellationToken::new();
        let wire = |incoming, sink| PeerWire {
            kind,
            address: address.into(),
            incoming,
            writer: WireWriter {
                sink: Sink::Frames(sink),
                buffered: Arc::default(),
                closed: closed.clone(),
            },
        };
        (wire(from_device, to_device), wire(from_host, to_host))
    }
}

/// A WebSocket as two queues, so the relay client runs the same against the network and against a fake.
pub struct Socket {
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
    buffered: Arc<AtomicUsize>,
    closed: CancellationToken,
}

impl Socket {
    /// Opens a client WebSocket (`ws:` or `wss:`).
    pub async fn connect(url: &str) -> io::Result<Socket> {
        let (stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(io::Error::other)?;
        Ok(Socket::spawn(stream))
    }

    fn spawn<S>(stream: WebSocketStream<S>) -> Socket
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut source) = stream.split();
        let (tx, mut outgoing) = mpsc::unbounded_channel::<Message>();
        let (incoming, rx) = mpsc::unbounded_channel();
        let socket = Socket {
            tx,
            rx,
            buffered: Arc::default(),
            closed: CancellationToken::new(),
        };
        let (closed, buffered) = (socket.closed.clone(), socket.buffered.clone());
        tokio::spawn(async move {
            loop {
                // Queued messages go out before a close.
                let message = tokio::select! {
                    biased;
                    message = outgoing.recv() => message,
                    () = closed.cancelled() => None,
                };
                let Some(message) = message else { break };
                let length = message.len();
                if sink.send(message).await.is_err() {
                    break;
                }
                buffered.fetch_sub(length, Ordering::Relaxed);
            }
            closed.cancel();
            let _ = sink.close().await;
        });
        let closed = socket.closed.clone();
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    biased;
                    () = closed.cancelled() => break,
                    message = source.next() => message,
                };
                let Some(Ok(message)) = message else { break };
                let last = matches!(message, Message::Close(_));
                if incoming.send(message).is_err() || last {
                    break;
                }
            }
            closed.cancel();
        });
        socket
    }

    /// Two sockets joined in memory, for fakes: what one sends the other receives, and either closes both.
    pub fn pair() -> (Socket, Socket) {
        let (left_tx, right_rx) = mpsc::unbounded_channel();
        let (right_tx, left_rx) = mpsc::unbounded_channel();
        let closed = CancellationToken::new();
        let socket = |tx, rx| Socket {
            tx,
            rx,
            buffered: Arc::default(),
            closed: closed.clone(),
        };
        (socket(left_tx, left_rx), socket(right_tx, right_rx))
    }

    pub fn send(&self, message: Message) -> Result<(), Closed> {
        if self.is_closed() {
            return Err(Closed);
        }
        self.buffered.fetch_add(message.len(), Ordering::Relaxed);
        self.tx.send(message).map_err(|_| Closed)
    }

    /// What arrived, then `None` once the socket closed.
    pub async fn recv(&mut self) -> Option<Message> {
        tokio::select! {
            biased;
            message = self.rx.recv() => message,
            () = self.closed.cancelled() => None,
        }
    }

    pub fn close(&self) {
        self.closed.cancel();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// A handle that can close this socket from elsewhere.
    pub fn closer(&self) -> CancellationToken {
        self.closed.clone()
    }
}
