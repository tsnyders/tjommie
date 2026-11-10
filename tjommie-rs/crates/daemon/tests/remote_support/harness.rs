//! A server with the remote methods, and a remote host that dispatches device requests into it, like `startDaemon`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use daemon::remote::{Remote, RemoteOptions};
use daemon::remote_host::{Dispatch, RemoteHostOptions};
use daemon::server::{Handlers, RequestContext, RpcError, Server, ServerOptions};
use protocol::Event;
use rusqlite::Connection;
use serde_json::Value;

use super::{MemoryVault, shared_db};

pub struct Harness {
    pub server: Arc<Server>,
    pub remote: Arc<Remote>,
    pub db: Arc<Mutex<Connection>>,
    pub events: Arc<Mutex<Vec<Event>>>,
}

pub struct Setup {
    pub db: Arc<Mutex<Connection>>,
    pub vault: Arc<MemoryVault>,
    pub relay_url: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// Handlers beyond the remote methods.
    pub extra: fn(Handlers) -> Handlers,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            db: shared_db(),
            vault: Arc::default(),
            relay_url: Arc::new(|| None),
            extra: |handlers| handlers,
        }
    }
}

impl Harness {
    pub async fn start(setup: Setup) -> Harness {
        let slot: Arc<OnceLock<Arc<Server>>> = Arc::default();
        let to_server = slot.clone();
        let dispatch: Dispatch = Arc::new(
            move |method: String, params: Value, context: RequestContext| {
                let slot = to_server.clone();
                Box::pin(async move {
                    slot.get()
                        .unwrap()
                        .dispatch(&method, params, Some(context))
                        .await
                })
            },
        );
        let mut host = RemoteHostOptions::new(
            setup.db.clone(),
            PathBuf::from("synthetic-handlers-root"),
            dispatch,
        );
        // Never the production 0.0.0.0:47101 bind on this PC: loopback on an ephemeral port.
        host.port = 0;
        host.bind = [127, 0, 0, 1].into();
        host.addresses = Some(Arc::new(|| vec!["127.0.0.1".into()]));
        host.vault = setup.vault;
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let seen = events.clone();
        let remote = Remote::new(RemoteOptions {
            host,
            relay_url: setup.relay_url,
            broadcast: Arc::new(move |event: Event| seen.lock().unwrap().push(event)),
        });
        let server = Arc::new(
            Server::start(ServerOptions {
                port: 0,
                token: "synthetic-token".into(),
                handlers: (setup.extra)(remote.handlers(Handlers::default())),
                allowed_origins: Vec::new(),
                defer_requests: false,
            })
            .await
            .unwrap(),
        );
        assert!(slot.set(server.clone()).is_ok());
        Harness {
            server,
            remote,
            db: setup.db,
            events,
        }
    }

    /// A local client's call. Results are checked against the manifest.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let result = self.server.dispatch(method, params, None).await;
        if let Ok(value) = &result {
            protocol::assert_valid(method, value);
        }
        result
    }

    pub fn changes(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, Event::DevicesChanged(_)))
            .count()
    }
}
