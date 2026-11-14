//! `daemon.ts`'s M6 wiring in one call: attachments, the Logins host (`/vault-host`, `vault.*`), the native browser
//! (`/browser-host`, `browser.*`, `/mcp/browser`) and the worker boundary (`/worker-host`). The host routes and their
//! methods exist only with the desktop host token. `/mcp/browser` is always mounted: without the desktop (`--dev`) its
//! tools answer that the desktop app is needed, because there is no Playwright fallback.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use protocol::ErrorCode;
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use crate::attachments::Attachments;
use crate::bots::get_stored_bot;
use crate::browser_stream::BrowserStreams;
use crate::context::Db;
use crate::ids::new_id;
use crate::native_browser_broker::{BROWSER_TIMEOUT, BrowserError, NativeBrowserBroker};
use crate::native_browser_handlers::{self, NativeRequest};
use crate::native_browser_mcp::{ForBot, NativeBrowserMcp, NativeBrowserMcpOptions};
use crate::remote::Remote;
use crate::server::{Broadcaster, Handlers, Routes, RpcError};
use crate::vault_handlers;
use crate::vault_host_broker::VaultHostBroker;
use crate::worker_boundary_host::{WorkerBoundary, WorkerBoundaryHost};

/// `vault-host-broker.ts`'s default wait for the desktop's Logins host.
const VAULT_TIMEOUT: Duration = Duration::from_secs(150);

pub struct M6Deps {
    pub db: Db,
    /// The data root. Its `shared-files` folder must already exist (`paths::ensure`).
    pub data_root: PathBuf,
    /// `Auth::Production`'s `desktop_host_token`; `None` in `--dev` or from a bare-token launch.
    pub desktop_host_token: Option<String>,
    /// `McpTokens.botFor`, shared with `/mcp`: the bot a CLI session token was issued to.
    pub bot_for: ForBot<Option<String>>,
    /// Lane B's `WindowsWorkerBoundary`, when worker isolation is set up.
    pub worker: Option<Arc<dyn WorkerBoundary>>,
}

/// What the bot lifecycle and shutdown still need after [`install`].
pub struct M6 {
    /// `cleanup()` after a bot is deleted.
    pub attachments: Arc<Attachments>,
    /// `close(bot)` when the bot's CLI session token is revoked.
    pub browser_mcp: Arc<NativeBrowserMcp>,
    /// The live browser stream (M8). Until [`M6::connect_events`] runs it is off: `browser.stream` answers `unknown_method`.
    pub streams: Arc<BrowserStreams>,
    native_broker: Option<Arc<NativeBrowserBroker>>,
    native_request: NativeRequest,
    vault_broker: Option<Arc<VaultHostBroker>>,
    worker_host: Option<Arc<WorkerBoundaryHost>>,
    runtime: tokio::runtime::Handle,
}

/// Registers M6's methods and routes, backfills legacy attachments and removes unused shared files. Call it inside the
/// runtime, after `sweep_deleted_bot_folders` and before `activate_with`.
pub fn install(handlers: &mut Handlers, routes: &mut Routes, deps: M6Deps) -> Result<M6, RpcError> {
    let attachments = Arc::new(Attachments::new(
        deps.db.clone(),
        &deps.data_root,
        &attachment_host_id(&deps.db)?,
    )?);
    attachments.backfill_legacy()?;
    if attachments.cleanup().is_err() {
        eprintln!("Unused shared files could not be removed. Cleanup will retry after restart.");
    }
    let token = deps.desktop_host_token;
    let native_broker = token
        .as_ref()
        .map(|token| NativeBrowserBroker::new(token.clone(), BROWSER_TIMEOUT));
    let vault_broker = token
        .as_ref()
        .map(|token| VaultHostBroker::new(token.clone(), VAULT_TIMEOUT));
    let worker_host = token
        .as_ref()
        .zip(deps.worker)
        .map(|(token, worker)| WorkerBoundaryHost::new(worker, token.clone()));
    let native_request = native_request(deps.db.clone(), native_broker.clone());
    let runtime = tokio::runtime::Handle::current();
    let db = deps.db;
    let stored =
        move |id: &str| get_stored_bot(&db.lock().unwrap_or_else(PoisonError::into_inner), id).ok();
    let (browser_of, paused_of) = (stored.clone(), stored);
    let browser_mcp = NativeBrowserMcp::new(NativeBrowserMcpOptions {
        bot_for: deps.bot_for,
        browser_enabled: Arc::new(move |id| browser_of(id).map(|bot| bot.browser)),
        request: native_request.clone(),
        cancel_tools: native_broker.clone().map(|broker| {
            let (request, runtime) = (native_request.clone(), runtime.clone());
            Arc::new(move |id: &str| cancel_tools(&broker, &request, &runtime, id)) as ForBot<()>
        }),
        can_run_tools: Some(Arc::new(move |id| {
            paused_of(id).is_some_and(|bot| !bot.paused)
        })),
    });

    let streams = BrowserStreams::new();
    let mut table = attachments.handlers(std::mem::take(handlers));
    let mut mounted = std::mem::take(routes).route("/mcp/browser", browser_mcp.route());
    if let Some(broker) = &native_broker {
        table = native_browser_handlers::register_streaming(
            table,
            native_request.clone(),
            streams.clone(),
        );
        let watchers = streams.clone();
        broker.on_stream(Arc::new(move |message| watchers.ingest(message)));
        mounted = mounted.route("/browser-host", broker.route());
    }
    if let Some(broker) = &vault_broker {
        table = vault_handlers::register(table, broker.requester());
        mounted = mounted.route("/vault-host", broker.route());
    }
    if let Some(host) = &worker_host {
        mounted = mounted.route("/worker-host", host.route());
    }
    *handlers = table;
    *routes = mounted;
    Ok(M6 {
        attachments,
        browser_mcp,
        streams,
        native_broker,
        native_request,
        vault_broker,
        worker_host,
        runtime,
    })
}

/// A stable id for this PC's attachment references, kept in settings.
fn attachment_host_id(db: &Db) -> Result<String, RpcError> {
    let db = db.lock().unwrap_or_else(PoisonError::into_inner);
    let saved: Option<String> = db
        .query_row(
            "SELECT value FROM settings WHERE key = 'attachmentHostId'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = saved {
        return Ok(id);
    }
    let id = new_id();
    db.execute(
        "INSERT INTO settings(key, value) VALUES('attachmentHostId', ?1)",
        [&id],
    )?;
    Ok(id)
}

/// `nativeRequest`: the stored bot's command to the desktop, refused while its browser is off. Calling it only builds
/// the request: the database is read when the future first runs, never at the call (see [`NativeRequest`]).
fn native_request(db: Db, broker: Option<Arc<NativeBrowserBroker>>) -> NativeRequest {
    Arc::new(move |id: String, command: Value, authorized| {
        let (db, broker) = (db.clone(), broker.clone());
        Box::pin(async move {
            // The guard is a temporary of this statement: it is gone before the first await.
            let bot = get_stored_bot(&db.lock().unwrap_or_else(PoisonError::into_inner), &id)?;
            let teardown = matches!(
                command["kind"].as_str(),
                Some("close" | "cancel_tools" | "delete_profile")
            );
            if !teardown && !bot.browser {
                return Err(RpcError::new(
                    ErrorCode::BadState,
                    "This agent's browser is off.",
                ));
            }
            let Some(broker) = broker else {
                return Err(RpcError::new(
                    ErrorCode::Unavailable,
                    BrowserError::unavailable().0,
                ));
            };
            broker
                .request((&bot).into(), command, authorized)
                .await
                .map_err(|error| RpcError::new(ErrorCode::Internal, error.0))
        })
    })
}

/// Stops the bot's running and queued tools here at once, and tells the desktop to stop them too.
fn cancel_tools(
    broker: &NativeBrowserBroker,
    request: &NativeRequest,
    runtime: &tokio::runtime::Handle,
    id: &str,
) {
    broker.cancel_tools(id);
    runtime.spawn(request(
        id.to_owned(),
        json!({ "kind": "cancel_tools" }),
        None,
    ));
}

impl M6 {
    /// `bots.update` turned the browser off: fence its tool session and close the desktop's browser.
    pub async fn browser_off(&self, id: &str) -> Result<(), RpcError> {
        self.browser_mcp.close(id);
        self.streams.ended(id);
        let Some(broker) = &self.native_broker else {
            return Ok(());
        };
        (self.native_request)(id.to_owned(), json!({ "kind": "close" }), None)
            .await
            .map_err(|_| {
                RpcError::new(
                    ErrorCode::Internal,
                    "The browser was disabled, but some browser resources could not close.",
                )
            })?;
        broker.close_bot(id);
        Ok(())
    }

    /// `bots.delete`, before the row goes: the desktop deletes the bot's browser profile.
    pub async fn bot_deleting(&self, id: &str) -> Result<(), RpcError> {
        self.streams.ended(id);
        if let Some(broker) = &self.native_broker {
            (self.native_request)(id.to_owned(), json!({ "kind": "delete_profile" }), None).await?;
            broker.close_bot(id);
        }
        Ok(())
    }

    /// `bots.pause` with `paused: true`.
    pub fn bot_paused(&self, id: &str) {
        if let Some(broker) = &self.native_broker {
            cancel_tools(broker, &self.native_request, &self.runtime, id);
        }
    }

    /// Turns the live stream on: frames go to the connection that asked, by its id, over the local server's WebSocket or
    /// a paired device's channel. Call once, after the server and `Remote` exist.
    pub fn connect_events(&self, events: Broadcaster, remote: Arc<Remote>) {
        self.streams.connect(Arc::new(move |connection, event| {
            events.send_to(connection, event) || remote.host().send_to(connection, event)
        }));
    }

    /// `bots.showBrowser`, after it checked the browser is on.
    pub async fn show_browser(&self, id: &str) -> Result<(), RpcError> {
        (self.native_request)(id.to_owned(), json!({ "kind": "open" }), None).await?;
        Ok(())
    }

    /// Shutdown's first step (`daemon.ts:670-671`), before anything waits for the runner: the desktop's worker and Logins
    /// hosts stop at once. The server should have stopped taking requests.
    pub fn close_hosts(&self) {
        if let Some(host) = &self.worker_host {
            host.close();
        }
        if let Some(broker) = &self.vault_broker {
            broker.close();
        }
    }

    /// Shutdown's browser step (`daemon.ts:681-682`), after the runner and the accounts have stopped: while the runner waits
    /// for its processes a turn can still end, and its hooks still fence the bot's browser tools through these.
    pub fn close_browser(&self) {
        self.browser_mcp.close_all();
        if let Some(broker) = &self.native_broker {
            broker.close();
        }
    }

    /// Shutdown's last step (`daemon.ts:685`), after the server: the shared files' transfer leases.
    pub fn close_files(&self) {
        self.attachments.close();
    }

    /// All three, in `daemon.ts`'s order, for a caller with nothing to wait for in between.
    pub fn close(&self) {
        self.close_hosts();
        self.close_browser();
        self.close_files();
    }
}
