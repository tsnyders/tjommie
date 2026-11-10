//! The remote methods and lifecycle from `daemon.ts:334-404`: `remote.*`, `devices.*`, `device.registerPush` (which only
//! paired devices may call, through the remote host) and `pairing.*`. The daemon publishes every event through
//! [`Remote::publish`] so devices get them too.

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use protocol::remote::{PairingStart, RelayState, RelayStatus, RelayUrl, RemoteStatus};
use protocol::wire::{Empty, Id, method};
use protocol::{ErrorCode, Event};
use rusqlite::Connection;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::devices_store::{list_devices, rename_device};
use crate::remote_host::{Db, INVITE_LIFETIME, RemoteHost, RemoteHostOptions};
use crate::server::{Handlers, RequestContext, RpcError};

pub struct RemoteOptions {
    /// Its `on_changed` and `relay_url` are set by [`Remote`].
    pub host: RemoteHostOptions,
    /// `settings.relayUrl`.
    pub relay_url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// The daemon's broadcast: every local client, then [`Remote::publish`].
    pub broadcast: Arc<dyn Fn(Event) + Send + Sync>,
}

struct Invite {
    id: String,
    timer: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    invite: Option<Invite>,
}

pub struct Remote {
    host: RemoteHost,
    db: Db,
    relay_url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    broadcast: Arc<dyn Fn(Event) + Send + Sync>,
    state: Mutex<State>,
}

fn devices_changed() -> Event {
    Event::DevicesChanged(Empty {})
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
                broadcast,
            } = options;
            let changed = remote.clone();
            host.on_changed = Arc::new(move || {
                if let Some(remote) = changed.upgrade() {
                    remote.send(devices_changed());
                }
            });
            let settings = relay_url.clone();
            // ponytail: no relay client yet (checkpoint 2), so a pairing code never names the relay.
            host.relay_url =
                Arc::new(move |for_pairing| if for_pairing { None } else { settings() });
            let db = host.db.clone();
            Remote {
                host: RemoteHost::new(host),
                db,
                relay_url,
                broadcast,
                state: Mutex::default(),
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

    fn send(&self, event: Event) {
        (self.broadcast)(event);
    }

    /// Delivers a daemon event to paired devices.
    pub fn publish(&self, event: &Event) {
        self.host.broadcast(event);
    }

    pub fn status(&self) -> RemoteStatus {
        RemoteStatus {
            relay: RelayStatus {
                url: (self.relay_url)().and_then(|url| RelayUrl::new(url).ok()),
                state: RelayState::Off,
                push: false,
            },
            ..self.host.status()
        }
    }

    /// At daemon start: devices are already paired, so listen for them.
    pub async fn start(&self) {
        if self.host.has_devices() && self.host.ensure_listening().await.is_err() {
            eprintln!("The phone connection could not start. Retry from Devices.");
        }
    }

    async fn create_pairing(self: &Arc<Self>) -> Result<PairingStart, RpcError> {
        self.host.ensure_listening().await?;
        let start = self.host.create_pairing().await?;
        let remote = Arc::downgrade(self);
        let id = start.id.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep_until(Instant::now() + INVITE_LIFETIME).await;
            if let Some(remote) = remote.upgrade() {
                remote.clear_invite(Some(&id));
            }
        });
        if let Some(previous) = self.state().invite.replace(Invite {
            id: start.id.clone(),
            timer,
        }) {
            previous.timer.abort();
        }
        Ok(start)
    }

    fn clear_invite(&self, id: Option<&str>) {
        let mut state = self.state();
        if id.is_some_and(|id| state.invite.as_ref().is_some_and(|invite| invite.id != id)) {
            return;
        }
        if let Some(invite) = state.invite.take() {
            invite.timer.abort();
        }
    }

    pub async fn close(&self) {
        self.clear_invite(None);
        self.host.close().await;
    }

    /// Answers the remote methods. `device.registerPush` is refused here: only the remote host takes it, from a device.
    pub fn handlers(self: &Arc<Self>, handlers: Handlers) -> Handlers {
        handlers
            .on::<method::RemoteStatus, _>(with(
                self,
                |remote, _| async move { Ok(remote.status()) },
            ))
            .on::<method::RemotePing, _>(with(self, |_, _| async { Ok(Empty {}) }))
            .on::<method::DeviceRegisterPush, _>(with(self, |_, _| async {
                Err(RpcError::new(
                    ErrorCode::BadState,
                    "Push registration is available only to paired devices.",
                ))
            }))
            .on::<method::DevicesList, _>(with(self, |remote, _| async move {
                list_devices(&remote.db())
            }))
            .on::<method::DevicesRename, _>(with(
                self,
                |remote, params: protocol::remote::DevicesRename| async move {
                    let device = rename_device(&remote.db(), &params.id, params.name.as_str())?;
                    remote.send(devices_changed());
                    Ok(device)
                },
            ))
            .on::<method::DevicesRevoke, _>(with(self, |remote, Id { id }| async move {
                remote.host.revoke(&id)
            }))
            .on::<method::PairingCreate, _>(with(self, |remote, _| async move {
                remote.create_pairing().await
            }))
            .on::<method::PairingCancel, _>(with(self, |remote, Id { id }| async move {
                remote.host.cancel_pairing(&id);
                remote.clear_invite(Some(&id));
                Ok(Empty {})
            }))
    }
}
