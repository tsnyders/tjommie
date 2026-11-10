//! The remote methods and lifecycle from `daemon.ts:334-404`: `remote.*`, `devices.*`, `device.registerPush` (which only
//! paired devices may call, through the remote host) and `pairing.*`, plus the relay. The relay runs while it is set
//! and something needs it: a paired device (revoked ones count), a pairing being created, or a live invite. The daemon
//! publishes every event through [`Remote::publish`], so devices get them and `notify` becomes a push.

use std::future::ready;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use protocol::remote::{PairingStart, RelayState, RelayStatus, RelayUrl, RemoteStatus};
use protocol::wire::{Empty, Id, method};
use protocol::{ErrorCode, Event};
use rusqlite::Connection;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::devices_store::{list_devices, rename_device};
use crate::push::Push;
use crate::relay_client::{Connector, RelayClient, RelayClientOptions};
use crate::relay_identity::load_relay_identity;
use crate::remote_host::{Db, INVITE_LIFETIME, RemoteHost, RemoteHostOptions};
use crate::remote_identity::Vault;
use crate::server::{Handlers, RequestContext, RpcError};

pub struct RemoteOptions {
    /// Its `on_changed` and `relay_url` are set by [`Remote`].
    pub host: RemoteHostOptions,
    /// `settings.relayUrl`.
    pub relay_url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// How relay sockets open: `relay_client::default_connector()` in the daemon.
    pub relay_connect: Connector,
    /// The daemon's broadcast: every local client, then [`Remote::publish`].
    pub broadcast: Arc<dyn Fn(Event) + Send + Sync>,
}

struct Invite {
    id: String,
    timer: JoinHandle<()>,
}

struct State {
    pairing_pending: u32,
    invite: Option<Invite>,
    closing: bool,
    /// What status shows while the relay client is off: `Off`, `Connecting` or `Error`.
    relay_setup: RelayState,
    relay: Option<RelayClient>,
    push: Option<Push>,
}

pub struct Remote {
    host: RemoteHost,
    db: Db,
    root: PathBuf,
    vault: Arc<dyn Vault>,
    relay_url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    relay_connect: Connector,
    broadcast: Arc<dyn Fn(Event) + Send + Sync>,
    state: Mutex<State>,
    /// One relay identity load at a time (`relayOpening`).
    relay_opening: tokio::sync::Mutex<()>,
}

fn devices_changed() -> Event {
    Event::DevicesChanged(Empty {})
}

fn relay_failed(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(ErrorCode::Internal, error.to_string())
}

/// Adapts `handler(remote, params)` to [`Handlers::on`].
fn with<P, Fut>(
    remote: &Arc<Remote>,
    handler: impl Fn(Arc<Remote>, P) -> Fut + Send + Sync + 'static,
) -> impl Fn(P, Option<RequestContext>) -> Fut + Send + Sync + 'static {
    let remote = remote.clone();
    move |params, _| handler(remote.clone(), params)
}

impl Remote {
    pub fn new(options: RemoteOptions) -> Arc<Remote> {
        Arc::new_cyclic(|remote: &Weak<Remote>| {
            let RemoteOptions {
                mut host,
                relay_url,
                relay_connect,
                broadcast,
            } = options;
            let changed = remote.clone();
            host.on_changed = Arc::new(move || {
                if let Some(remote) = changed.upgrade() {
                    remote.spawn_sync();
                    remote.send(devices_changed());
                }
            });
            // A pairing code names the relay only once this PC is logged in to it.
            let (settings, relay) = (relay_url.clone(), remote.clone());
            host.relay_url = Arc::new(move |for_pairing| {
                let online = relay
                    .upgrade()
                    .and_then(|remote| remote.relay())
                    .is_some_and(|relay| relay.state() == RelayState::Online);
                if for_pairing && !online {
                    None
                } else {
                    settings()
                }
            });
            let (db, root, vault) = (host.db.clone(), host.root.clone(), host.vault.clone());
            Remote {
                host: RemoteHost::new(host),
                db,
                root,
                vault,
                relay_url,
                relay_connect,
                broadcast,
                state: Mutex::new(State {
                    pairing_pending: 0,
                    invite: None,
                    closing: false,
                    relay_setup: RelayState::Off,
                    relay: None,
                    push: None,
                }),
                relay_opening: tokio::sync::Mutex::new(()),
            }
        })
    }

    pub fn host(&self) -> &RemoteHost {
        &self.host
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn relay(&self) -> Option<RelayClient> {
        self.state().relay.clone()
    }

    fn send(&self, event: Event) {
        (self.broadcast)(event);
    }

    /// Delivers a daemon event to paired devices, and a `notify` to their phones as a push.
    pub fn publish(&self, event: &Event) {
        self.host.broadcast(event);
        if let Event::Notify(notify) = event {
            let push = self.state().push.clone();
            if let Some(push) = push {
                push.send(notify);
            }
        }
    }

    pub fn status(&self) -> RemoteStatus {
        let (relay, setup) = {
            let state = self.state();
            (state.relay.clone(), state.relay_setup)
        };
        let state = match &relay {
            Some(relay) if relay.state() != RelayState::Off => relay.state(),
            _ => setup,
        };
        RemoteStatus {
            relay: RelayStatus {
                url: (self.relay_url)().and_then(|url| RelayUrl::new(url).ok()),
                state,
                push: relay.is_some_and(|relay| relay.push_enabled()),
            },
            ..self.host.status()
        }
    }

    /// At daemon start: devices are already paired, so listen for them and bring up the relay.
    pub async fn start(self: &Arc<Self>) {
        if !self.host.has_devices() {
            return;
        }
        if self.host.ensure_listening().await.is_err() {
            eprintln!("The phone connection could not start. Retry from Devices.");
        }
        self.spawn_sync();
    }

    /// `settings.relayUrl` changed: log in again with the new URL, or stop.
    pub async fn relay_url_changed(self: &Arc<Self>) {
        if let Some(relay) = self.relay() {
            relay.stop();
        }
        let _ = self.sync_relay(false).await;
    }

    fn relay_needed(&self) -> bool {
        let devices = self.host.has_devices();
        let state = self.state();
        !state.closing
            && (self.relay_url)().is_some()
            && (devices || state.pairing_pending > 0 || state.invite.is_some())
    }

    /// Returns whether the setup state changed.
    fn set_setup(&self, setup: RelayState) -> bool {
        let mut state = self.state();
        let changed = state.relay_setup != setup;
        state.relay_setup = setup;
        changed
    }

    fn spawn_sync(self: &Arc<Self>) {
        let remote = self.clone();
        tokio::spawn(async move {
            let _ = remote.sync_relay(false).await;
        });
    }

    /// `syncRelay`: starts or stops the relay to match what needs it.
    async fn sync_relay(self: &Arc<Self>, wait_online: bool) -> Result<(), RpcError> {
        if !self.relay_needed() {
            if let Some(relay) = self.relay() {
                relay.stop();
            }
            if self.set_setup(RelayState::Off) {
                self.send(devices_changed());
            }
            return Ok(());
        }
        if self.set_setup(RelayState::Connecting) {
            self.send(devices_changed());
        }
        let result = self.connect_relay(wait_online).await;
        if result.is_err() {
            self.set_setup(RelayState::Error);
            self.send(devices_changed());
        }
        result
    }

    async fn connect_relay(self: &Arc<Self>, wait_online: bool) -> Result<(), RpcError> {
        self.host.ensure_listening().await?;
        if !self.relay_needed() {
            self.set_setup(RelayState::Off);
            return Ok(());
        }
        let relay = self.open_relay().await?;
        if !self.relay_needed() {
            relay.stop();
            self.set_setup(RelayState::Off);
            return Ok(());
        }
        relay.start();
        if wait_online {
            relay.wait_until_online().await.map_err(relay_failed)?;
        }
        Ok(())
    }

    /// `openRelay`: one client for the life of the daemon, with its push sender.
    async fn open_relay(self: &Arc<Self>) -> Result<RelayClient, RpcError> {
        let _opening = self.relay_opening.lock().await;
        if let Some(relay) = self.relay() {
            return Ok(relay);
        }
        // Credential Manager I/O: off the async workers.
        let (root, vault) = (self.root.clone(), self.vault.clone());
        let identity =
            tokio::task::spawn_blocking(move || load_relay_identity(&root, vault.as_ref()))
                .await
                .map_err(relay_failed)?
                .map_err(relay_failed)?;
        let host_id = self.host.status().host_id.ok_or_else(|| {
            RpcError::new(ErrorCode::BadState, "The phone connection is unavailable.")
        })?;
        let (host, remote) = (self.host.clone(), Arc::downgrade(self));
        let mut options = RelayClientOptions::new(
            self.relay_url.clone(),
            host_id,
            Arc::new(identity),
            Arc::new(move |wire| host.accept_wire(wire)),
            Arc::new(move || {
                if let Some(remote) = remote.upgrade() {
                    remote.send(devices_changed());
                }
            }),
        );
        options.connect = self.relay_connect.clone();
        let relay = RelayClient::new(options);
        let push = Push::new(Arc::new(self.host.clone()), Arc::new(relay.clone()));
        let mut state = self.state();
        state.relay = Some(relay.clone());
        state.push = Some(push);
        Ok(relay)
    }

    async fn create_pairing(self: &Arc<Self>) -> Result<PairingStart, RpcError> {
        self.state().pairing_pending += 1;
        let result = self.pairing().await;
        self.state().pairing_pending -= 1;
        self.spawn_sync();
        result
    }

    async fn pairing(self: &Arc<Self>) -> Result<PairingStart, RpcError> {
        self.host.ensure_listening().await?;
        // A relay that doesn't come up leaves a LAN-only code.
        let _ = self.sync_relay(true).await;
        let start = self.host.create_pairing().await?;
        let remote = Arc::downgrade(self);
        let id = start.id.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep_until(Instant::now() + INVITE_LIFETIME).await;
            if let Some(remote) = remote.upgrade() {
                remote.clear_invite(Some(&id));
            }
        });
        let previous = self.state().invite.replace(Invite {
            id: start.id.clone(),
            timer,
        });
        if let Some(previous) = previous {
            previous.timer.abort();
        }
        Ok(start)
    }

    fn clear_invite(self: &Arc<Self>, id: Option<&str>) {
        {
            let mut state = self.state();
            if id.is_some_and(|id| state.invite.as_ref().is_some_and(|invite| invite.id != id)) {
                return;
            }
            if let Some(invite) = state.invite.take() {
                invite.timer.abort();
            }
        }
        self.spawn_sync();
    }

    pub async fn close(self: &Arc<Self>) {
        let relay = {
            let mut state = self.state();
            state.closing = true;
            if let Some(invite) = state.invite.take() {
                invite.timer.abort();
            }
            state.relay.clone()
        };
        if let Some(relay) = relay {
            relay.stop();
        }
        self.host.close().await;
    }

    /// Answers the remote methods. `device.registerPush` is refused here: only the remote host takes it, from a device.
    /// Handlers run on the blocking pool, so database work happens before the returned future; only pairing, which
    /// waits on the listener and the relay, does its work in the future.
    pub fn handlers(self: &Arc<Self>, handlers: Handlers) -> Handlers {
        handlers
            .on::<method::RemoteStatus, _>(with(self, |remote, _| ready(Ok(remote.status()))))
            .on::<method::RemotePing, _>(with(self, |_, _| ready(Ok(Empty {}))))
            .on::<method::DeviceRegisterPush, _>(with(self, |_, _| {
                ready(Err(RpcError::new(
                    ErrorCode::BadState,
                    "Push registration is available only to paired devices.",
                )))
            }))
            .on::<method::DevicesList, _>(with(self, |remote, _| ready(list_devices(&remote.db()))))
            .on::<method::DevicesRename, _>(with(
                self,
                |remote, params: protocol::remote::DevicesRename| {
                    let renamed = rename_device(&remote.db(), &params.id, params.name.as_str());
                    if renamed.is_ok() {
                        remote.send(devices_changed());
                    }
                    ready(renamed)
                },
            ))
            .on::<method::DevicesRevoke, _>(with(self, |remote, Id { id }| {
                ready(remote.host.revoke(&id))
            }))
            .on::<method::PairingCreate, _>(with(self, |remote, _| async move {
                remote.create_pairing().await
            }))
            .on::<method::PairingCancel, _>(with(self, |remote, Id { id }| {
                remote.host.cancel_pairing(&id);
                remote.clear_invite(Some(&id));
                ready(Ok(Empty {}))
            }))
    }
}
