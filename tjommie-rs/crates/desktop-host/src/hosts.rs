//! The desktop's three clients of this PC's daemon, started and stopped as one (`main.ts` `launchDaemon`, `onDaemonExit`,
//! `will-quit`; Phase 3 host wiring): `/browser-host` (a bot's browser jobs), `/vault-host` (the Logins jobs) and
//! `/worker-host` (the isolation lease the vault holds while it works). All three use the private desktop host token, which
//! only the app holds: it is what makes the app the daemon's host (spec §2).
//!
//! A daemon has its own port and tokens, so every new one gets new clients and the old ones are let go first
//! ([`Hosts::stop`]). A client that loses the daemon locks the vault (`on_error`), and so does stopping.
//!
//! Nothing here touches WebView2 or a window. What needs the UI thread crosses as typed values: the browser's jobs through
//! the [`BrowserExecutor`] the app gives, the vault's browser steps through [`EngineLogin`]'s requests, the person's
//! entries and confirmations through the [`EntryWindow`]. The vault itself is built once and outlives the daemons, as the
//! TypeScript's `loginVault` does (`isolation` forwards to whichever daemon's worker client is current).

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_channel::Receiver;
use client::{Client, Endpoint};
use futures::future::BoxFuture;
use protocol::vault::{VaultHostAction, VaultHostCommand};
use protocol::wire::{Empty, method};

use crate::browser::BotIdentity;
use crate::browser::host_bridge::{BridgeConfig, Executor as BrowserExecutor, HostBridge};
use crate::browser::login::{BotLookup, EngineLogin, Request};
use crate::daemon_process::locked;
use crate::vault::gate::{GateError, GateInputs, Vault, VaultCommandGate, VaultReply, WindowState};
use crate::vault::host::{HostConfig, HostEvent, HostToken, VaultHost};
use crate::vault::login::{EntryWindow, LoginError, NativeLoginVault, SecretStore, VaultParts};
use crate::vault::storage::VaultRecords;
use crate::vault::worker::{
    IsolationError, RemoteWorkerIsolation, VaultIsolation, VaultIsolationLease,
};

/// The controller of the app's own requests (`controllerId: "local"`).
const LOCAL: &str = "local";
/// What the vault says while there is no daemon to hold its isolation (`main.ts`: `vaultIsolation` unset).
const NO_ISOLATION: &str = "The protected provider host is unavailable.";
/// Why a Logins step can't name its bot.
const BOT_GONE: &str = "This bot's browser is off or the bot no longer exists.";

/// This PC's daemon: where it listens and the two tokens of one run. A restart gives all new ones.
#[derive(Clone)]
pub struct Local {
    pub port: u16,
    /// The ordinary token the app's own connection uses; the bot directory connects with it.
    pub token: String,
    /// The private capability for the three host routes: wiped from memory when dropped, never printed.
    pub host_token: HostToken,
}

impl fmt::Debug for Local {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Local")
            .field("port", &self.port)
            .field("token", &"<redacted>")
            .field("host_token", &"<redacted>")
            .finish()
    }
}

/// A bot as the daemon lists it, so far as the hosts need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotInfo {
    pub identity: BotIdentity,
    /// Its browser is switched on.
    pub browser: bool,
}

/// The daemon's bot list could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryError;

impl fmt::Display for DirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("The agents' list could not be read.")
    }
}

impl std::error::Error for DirectoryError {}

/// Who the daemon's bots are (`resolveNativeBot`): asked on every Logins step, so it is always the daemon's current word.
pub trait BotDirectory: Send + Sync {
    fn find(&self, id: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>>;
}

/// The bot directory over a connection to this PC's daemon (the TypeScript's `nativeClient`), made when it is first asked and
/// again after a call finds it closed, so a daemon that is slow to come up costs one failed step and not the host.
pub struct DaemonBots {
    port: u16,
    /// The ordinary token the app's own connection uses (not the host token, which this never needs).
    token: String,
    client: tokio::sync::Mutex<Option<Arc<Client>>>,
}

impl DaemonBots {
    pub fn new(local: &Local) -> Self {
        Self {
            port: local.port,
            token: local.token.clone(),
            client: tokio::sync::Mutex::new(None),
        }
    }

    async fn connection(&self) -> Result<Arc<Client>, DirectoryError> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        let client = Client::connect(Endpoint::Local { port: self.port }, &self.token)
            .await
            .map_err(|_| DirectoryError)?;
        let client = Arc::new(client);
        *slot = Some(client.clone());
        Ok(client)
    }
}

impl BotDirectory for DaemonBots {
    fn find(&self, id: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>> {
        let id = id.to_owned();
        Box::pin(async move {
            let client = self.connection().await?;
            let bots = match client.call::<method::BotsList>(Empty {}).await {
                Ok(bots) => bots,
                Err(_) => {
                    // Closed or confused: the next step connects again.
                    *self.client.lock().await = None;
                    return Err(DirectoryError);
                }
            };
            Ok(bots
                .into_iter()
                .find(|bot| bot.id == id)
                .map(|bot| BotInfo {
                    identity: BotIdentity {
                        id: bot.id,
                        name: bot.name,
                        home_folder: bot.home_folder,
                        project_folder: bot.project_folder,
                    },
                    browser: bot.browser,
                }))
        })
    }
}

/// What the app's window says of itself, set from the UI thread and read by the gate (`main.ts:457`).
#[derive(Default)]
pub struct WindowFacts {
    present: AtomicBool,
    visible: AtomicBool,
    focused: AtomicBool,
    quitting: AtomicBool,
}

impl WindowFacts {
    /// The window exists and is shown. It has no document that loads after that, so it is ready from here on.
    pub fn show(&self) {
        self.present.store(true, Ordering::SeqCst);
        self.visible.store(true, Ordering::SeqCst);
    }

    /// Whether the window is being presented (shown and not minimized, as GPUI reports it): a hidden or minimized window is not
    /// trusted.
    pub fn set_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::SeqCst);
    }

    /// Whether the window has the keyboard (`main.ts` `win.isFocused()`).
    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::SeqCst);
    }

    /// The window is up front: shown and focused, so a notice is already in the app's history and needs no toast.
    pub fn visible_and_focused(&self) -> bool {
        self.visible.load(Ordering::SeqCst) && self.focused.load(Ordering::SeqCst)
    }

    fn quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    fn state(&self) -> WindowState {
        WindowState {
            present: self.present.load(Ordering::SeqCst),
            visible: self.visible.load(Ordering::SeqCst),
            // GPUI reports a minimized window as not visible, which `visible` already covers.
            minimized: false,
            document_ready: self.present.load(Ordering::SeqCst),
            quitting: self.quitting(),
        }
    }
}

/// The current daemon's bot directory.
#[derive(Default)]
struct Current(Mutex<Option<Arc<dyn BotDirectory>>>);

impl Current {
    fn get(&self) -> Option<Arc<dyn BotDirectory>> {
        locked(&self.0).clone()
    }

    fn set(&self, directory: Option<Arc<dyn BotDirectory>>) {
        *locked(&self.0) = directory;
    }
}

impl BotLookup for Current {
    /// A browser step needs the bot, with its browser on.
    fn bot(&self, id: &str) -> BoxFuture<'_, Result<BotIdentity, LoginError>> {
        let (directory, id) = (self.get(), id.to_owned());
        Box::pin(async move {
            let directory = directory.ok_or(LoginError::Failed)?;
            match directory.find(&id).await {
                Ok(Some(bot)) if bot.browser => Ok(bot.identity),
                _ => Err(LoginError::Failed),
            }
        })
    }
}

/// The worker client of the daemon that is current, behind the vault that outlives every daemon.
#[derive(Default)]
struct Isolation {
    current: Mutex<(u64, Option<Arc<RemoteWorkerIsolation>>)>,
}

impl Isolation {
    /// Makes `next` the current client (or none), which is a new connection to anything that fenced on the old one.
    fn swap(&self, next: Option<Arc<RemoteWorkerIsolation>>) {
        let mut current = locked(&self.current);
        current.0 += 1;
        current.1 = next;
    }

    fn get(&self) -> Option<Arc<RemoteWorkerIsolation>> {
        locked(&self.current).1.clone()
    }

    /// Which connection is current; `None` while there is none.
    fn connection(&self) -> Option<u64> {
        let current = locked(&self.current);
        current.1.as_ref().map(|_| current.0)
    }
}

impl VaultIsolation for Isolation {
    fn unavailable_reason(&self) -> BoxFuture<'_, Option<String>> {
        let current = self.get();
        Box::pin(async move {
            match current {
                Some(isolation) => isolation.unavailable_reason().await,
                None => Some(NO_ISOLATION.to_owned()),
            }
        })
    }

    fn acquire(&self) -> BoxFuture<'_, Result<Box<dyn VaultIsolationLease>, IsolationError>> {
        let current = self.get();
        Box::pin(async move {
            match current {
                Some(isolation) => isolation.acquire().await,
                None => Err(IsolationError),
            }
        })
    }
}

/// What the gate reads from the app, each time it reads it (`vault-command-gate.ts`'s options).
struct Inputs {
    vault: Option<Arc<NativeLoginVault>>,
    window: Arc<WindowFacts>,
    isolation: Arc<Isolation>,
    directory: Arc<Current>,
}

impl GateInputs for Inputs {
    fn vault(&self) -> Option<Arc<dyn Vault>> {
        self.vault.clone().map(|vault| -> Arc<dyn Vault> { vault })
    }

    fn ready(&self) -> bool {
        !self.window.quitting()
    }

    fn connection(&self) -> Option<u64> {
        self.isolation.connection()
    }

    /// `resolveNativeBot(id, false)`: the bot must exist; whether its browser is on is the browser step's to check.
    fn resolve_bot(&self, id: &str) -> BoxFuture<'_, Result<(), GateError>> {
        let (directory, id) = (self.directory.get(), id.to_owned());
        Box::pin(async move {
            let Some(directory) = directory else {
                return Err(GateError::Unavailable);
            };
            match directory.find(&id).await {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(GateError::Vault(BOT_GONE.to_owned())),
                // The daemon could not be asked: that is not the bot being gone.
                Err(_) => Err(GateError::Unavailable),
            }
        })
    }
}

/// What the hosts are made of that only the app can give.
pub struct Parts {
    /// The data root: the vault's records are `<root>/data/logins.sqlite`.
    pub root: PathBuf,
    /// Where the logins' secrets go (Credential Manager in the app); without a store there is no vault.
    pub secrets: Option<Arc<dyn SecretStore>>,
    /// The trusted windows the person answers.
    pub entry: Arc<dyn EntryWindow>,
    /// The runtime the clients' blocking close and logging run on.
    pub runtime: tokio::runtime::Handle,
}

/// The three clients, started for one daemon. Give it back to [`Hosts::stop`].
#[must_use = "a running set of clients is stopped with Hosts::stop"]
pub struct Running {
    bridge: HostBridge,
    vault_host: VaultHost,
    isolation: Arc<RemoteWorkerIsolation>,
}

/// The vault, its gate and the clients' lifecycle. Built once per run; `Send + Sync`, so any thread may ask.
pub struct Hosts {
    vault: Option<Arc<NativeLoginVault>>,
    gate: Arc<VaultCommandGate>,
    inputs: Arc<Inputs>,
    runtime: tokio::runtime::Handle,
}

impl Hosts {
    /// The vault, built once, and the requests its browser steps send to the UI thread (`login::serve` runs them). A vault
    /// whose storage can't be opened leaves the app without Logins and nothing else.
    pub fn new(parts: Parts) -> (Hosts, Receiver<Request>) {
        let isolation = Arc::new(Isolation::default());
        let directory = Arc::new(Current::default());
        let window = Arc::new(WindowFacts::default());
        let (login, requests) = EngineLogin::new(directory.clone());
        let records = VaultRecords::open(&parts.root.join("data").join("logins.sqlite"));
        let vault = match (records, parts.secrets) {
            (Ok(records), Some(secrets)) => Some(Arc::new(NativeLoginVault::new(VaultParts {
                records,
                secrets,
                isolation: isolation.clone(),
                entry: parts.entry,
                browser: Arc::new(login),
            }))),
            _ => {
                log::warn!(
                    "Logins storage could not be opened. Other app features remain available."
                );
                None
            }
        };
        let inputs = Arc::new(Inputs {
            vault: vault.clone(),
            window,
            isolation,
            directory,
        });
        let gate = Arc::new(VaultCommandGate::new(inputs.clone()));
        (
            Hosts {
                vault,
                gate,
                inputs,
                runtime: parts.runtime,
            },
            requests,
        )
    }

    pub fn has_vault(&self) -> bool {
        self.vault.is_some()
    }

    /// The window facts the gate trusts a local request by: the UI thread keeps them true.
    pub fn window(&self) -> &WindowFacts {
        &self.inputs.window
    }

    /// The app is quitting: Logins takes no more requests but the cleanup ones (`will-quit`).
    pub fn begin_quit(&self) {
        self.inputs.window.quitting.store(true, Ordering::SeqCst);
    }

    /// The quit was given up (an update was not installed): the app stays, and its window is trusted again.
    pub fn cancel_quit(&self) {
        self.inputs.window.quitting.store(false, Ordering::SeqCst);
    }

    /// Locks the vault now: the entry window closes and the running operation is cancelled.
    pub fn lock(&self) {
        if let Some(vault) = &self.vault {
            vault.lock();
        }
    }

    /// A Logins request from the app's own window (`vault:command`): trusted only while the window is shown, not
    /// minimized, its document is ready and the app is not quitting (`main.ts:457`).
    pub fn execute_local(
        &self,
        action: VaultHostAction,
    ) -> BoxFuture<'_, Result<VaultReply, GateError>> {
        let window = self.inputs.window.state();
        Box::pin(self.gate.execute_local(
            window,
            VaultHostCommand {
                controller_id: LOCAL.to_owned(),
                action,
            },
        ))
    }

    /// Starts the three clients for `local` (a new daemon has new ports and tokens, so a running set is [`stopped`](
    /// Hosts::stop) first). `directory` is who the daemon's bots are; `jobs` takes the browser's jobs to the UI thread.
    pub fn start(
        &self,
        local: &Local,
        directory: Arc<dyn BotDirectory>,
        jobs: Arc<dyn BrowserExecutor>,
    ) -> io::Result<Running> {
        let isolation = RemoteWorkerIsolation::new(local.port, local.host_token.clone())
            .map(Arc::new)
            .map_err(|_| io::Error::other("The protected provider host could not be reached."))?;
        // What the gate reads is in place before any client can ask for it, so no early `/vault-host` request meets a vault
        // with no directory or no isolation. Anything that fails after this takes it away again.
        self.inputs.directory.set(Some(directory));
        self.inputs.isolation.swap(Some(isolation.clone()));
        let unpublish = |isolation: &RemoteWorkerIsolation| {
            self.inputs.isolation.swap(None);
            self.inputs.directory.set(None);
            isolation.close();
        };
        // The Logins host first: closing it later is a signal, not a join. The bridge starts last, so the one step that
        // joins a thread when it is dropped can no longer be dropped by a failure after it, inside the caller's update.
        let (events, heard) = async_channel::bounded::<HostEvent>(8);
        let vault_host = match VaultHost::start(
            HostConfig {
                port: local.port,
                token: local.host_token.clone(),
            },
            self.gate.clone(),
            events,
        ) {
            Ok(host) => host,
            Err(error) => {
                unpublish(&isolation);
                return Err(error);
            }
        };
        let vault = self.vault.clone();
        let bridge = match HostBridge::start(
            BridgeConfig {
                port: local.port,
                token: local.host_token.expose().to_owned(),
            },
            jobs,
            // A host that can't reach the daemon, or is refused, locks the vault (`onError`).
            Arc::new(move || {
                if let Some(vault) = &vault {
                    vault.lock();
                }
            }),
        ) {
            Ok(bridge) => bridge,
            Err(error) => {
                vault_host.close();
                unpublish(&isolation);
                return Err(error);
            }
        };
        // unguarded: a tokio task, not a UI handler; it only logs
        self.runtime.spawn(async move {
            while let Ok(HostEvent::Failed) = heard.recv().await {
                log::warn!("The Logins host lost the daemon, so Logins locked.");
            }
        });
        Ok(Running {
            bridge,
            vault_host,
            isolation,
        })
    }

    /// Stops the clients and locks the vault, before a daemon restarts or the app quits. The lock comes first, so nothing
    /// the clients were waiting on can use the vault while they wind down; the bridge's thread is joined off the caller's.
    pub fn stop(&self, running: Running) -> BoxFuture<'static, ()> {
        self.lock();
        self.inputs.isolation.swap(None);
        self.inputs.directory.set(None);
        running.isolation.close();
        running.vault_host.close();
        let closing = self.runtime.spawn_blocking(move || running.bridge.close());
        Box::pin(async move {
            let _ = closing.await;
        })
    }
}
