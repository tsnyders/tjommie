//! A device for the remote-host tests: a Rust port of `authenticateRemote` (`client/src/remote-peer.ts`) and
//! `connectTransport` (`client/src/client.ts`). The TypeScript client itself is exercised by `remote_interop.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::peer_wire::{PeerWire, WireWriter};
use protocol::remote::{RefusedReason, RemotePreface, remote_prologue};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use transport::Session;
use transport::channel::EncryptedMessages;

const WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// The host answered with an authenticated refusal.
    Refused(RefusedReason),
    /// The PC could not be authenticated, or the connection closed.
    Failed,
}

pub struct Pairing<'a> {
    pub preface: RemotePreface,
    pub private_key: &'a [u8],
    pub host_key: &'a [u8],
    pub psk: Option<&'a [u8]>,
    pub name: &'a str,
}

/// Authenticated and confirmed, but not yet said hello.
pub struct Authenticated {
    pub device_id: String,
    pub host_id: String,
    pub host_name: String,
    pub relay: Option<String>,
    wire: PeerWire,
    messages: EncryptedMessages<Session>,
}

/// On failure the wire comes back untouched, so a test can leave it open and watch the host close it.
pub async fn authenticate(
    mut wire: PeerWire,
    pairing: Pairing<'_>,
) -> Result<Authenticated, (AuthError, PeerWire)> {
    match handshake(&mut wire, pairing).await {
        Ok((messages, ready)) => Ok(Authenticated {
            device_id: ready["deviceId"].as_str().unwrap().into(),
            host_id: ready["hostId"].as_str().unwrap().into(),
            host_name: ready["hostName"].as_str().unwrap().into(),
            relay: ready["relay"].as_str().map(Into::into),
            wire,
            messages,
        }),
        Err(error) => Err((error, wire)),
    }
}

async fn handshake(
    wire: &mut PeerWire,
    pairing: Pairing<'_>,
) -> Result<(EncryptedMessages<Session>, Value), AuthError> {
    let prologue = remote_prologue(&pairing.preface).map_err(failed)?;
    let mut noise = Session::initiator(
        pairing.private_key,
        pairing.host_key,
        pairing.psk,
        &prologue,
    )
    .map_err(failed)?;
    let writer = wire.writer();
    writer
        .write(serde_json::to_vec(&pairing.preface).unwrap())
        .map_err(failed)?;
    writer
        .write(noise.write_handshake(b"").map_err(failed)?)
        .map_err(failed)?;
    let frame = next(wire).await?;
    if !noise.read_handshake(&frame).map_err(failed)?.is_empty() || !noise.handshake_finished() {
        return Err(AuthError::Failed);
    }
    let mut messages = EncryptedMessages::new(noise);
    let confirm = json!({ "type": "confirm", "protocol": 1, "name": pairing.name }).to_string();
    for frame in messages.encode(&confirm).map_err(failed)? {
        writer.write(frame).map_err(failed)?;
    }
    loop {
        let frame = next(wire).await?;
        let Some(text) = messages.receive(&frame).map_err(failed)? else {
            continue;
        };
        let response: Value = serde_json::from_str(&text).map_err(failed)?;
        if response["type"] == "refused" {
            let refused: protocol::remote::RemoteRefused =
                protocol::parse(response).map_err(failed)?;
            return Err(AuthError::Refused(refused.reason));
        }
        let ready: protocol::remote::RemoteReady =
            protocol::parse(response.clone()).map_err(failed)?;
        if ready.host_id != pairing.preface.host_id {
            return Err(AuthError::Failed);
        }
        return Ok((messages, response));
    }
}

fn failed<E>(_: E) -> AuthError {
    AuthError::Failed
}

async fn next(wire: &mut PeerWire) -> Result<Vec<u8>, AuthError> {
    timeout(WAIT, wire.next_frame())
        .await
        .ok()
        .flatten()
        .ok_or(AuthError::Failed)
}

type Pending = Arc<Mutex<HashMap<i64, (String, oneshot::Sender<Result<Value, (String, String)>>)>>>;

/// `connectTransport`: hello, welcome, then requests and events.
pub struct Client {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    events: mpsc::UnboundedReceiver<Value>,
    next_id: AtomicI64,
    writer: WireWriter,
}

impl Authenticated {
    pub async fn connect(self) -> Client {
        let Authenticated {
            mut wire,
            mut messages,
            ..
        } = self;
        let writer = wire.writer();
        let hello = json!({ "type": "hello", "protocol": 2, "token": "device" }).to_string();
        for frame in messages.encode(&hello).unwrap() {
            writer.write(frame).unwrap();
        }
        let welcome = loop {
            let frame = timeout(WAIT, wire.next_frame())
                .await
                .unwrap()
                .expect("welcome");
            if let Some(text) = messages.receive(&frame).unwrap() {
                break text;
            }
        };
        assert_eq!(
            serde_json::from_str::<Value>(&welcome).unwrap(),
            json!({ "type": "welcome", "protocol": 2 })
        );
        let (outgoing, mut queued) = mpsc::unbounded_channel::<String>();
        let (events, received) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let routes = pending.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    text = queued.recv() => {
                        let Some(text) = text else { break };
                        let Ok(frames) = messages.encode(&text) else { break };
                        if frames.into_iter().any(|frame| wire.writer().write(frame).is_err()) { break }
                    }
                    frame = wire.next_frame() => {
                        let Some(frame) = frame else { break };
                        let Ok(text) = messages.receive(&frame) else { break };
                        let Some(text) = text else { continue };
                        let value: Value = serde_json::from_str(&text).unwrap();
                        if value["type"] == "event" {
                            protocol::assert_valid(value["event"].as_str().unwrap(), &value["data"]);
                            let _ = events.send(value);
                        } else if value["type"] == "res" {
                            let id = value["id"].as_i64().unwrap();
                            let Some((method, reply)) = routes.lock().unwrap().remove(&id) else { continue };
                            let result = if value["ok"] == true {
                                protocol::assert_valid(&method, &value["result"]);
                                Ok(value["result"].clone())
                            } else {
                                let error = &value["error"];
                                Err((error["code"].as_str().unwrap().into(), error["message"].as_str().unwrap().into()))
                            };
                            let _ = reply.send(result);
                        }
                    }
                }
            }
            wire.writer().destroy();
            routes.lock().unwrap().clear();
        });
        Client {
            outgoing,
            pending,
            events: received,
            next_id: AtomicI64::new(1),
            writer,
        }
    }

    pub fn close(self) {
        self.wire.writer().destroy();
    }
}

impl Client {
    /// The result, or the error's code and message. Results are checked against the manifest.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, (String, String)> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(id, (method.into(), reply));
        let request =
            json!({ "type": "req", "id": id, "method": method, "params": params }).to_string();
        if self.outgoing.send(request).is_err() {
            return Err(("closed".into(), "The connection is closed.".into()));
        }
        match timeout(WAIT, answer).await {
            Ok(Ok(result)) => result,
            _ => Err(("closed".into(), "The connection is closed.".into())),
        }
    }

    pub async fn next_event(&mut self) -> Value {
        timeout(WAIT, self.events.recv())
            .await
            .unwrap()
            .expect("an event")
    }

    pub fn close(&self) {
        self.writer.destroy();
    }

    pub async fn disconnected(&self) {
        timeout(WAIT, self.writer.closed())
            .await
            .expect("the host disconnects the device");
    }

    pub fn is_connected(&self) -> bool {
        !self.writer.is_closed()
    }
}
