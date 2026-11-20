//! `vault-command-gate.ts` and the window check of `main.ts:457`: every vault command passes here before the vault sees it.
//! A request from the app's window is trusted only while the window is shown, not minimized, its document is ready and
//! the app is not quitting. Work that arrives from the daemon (`/vault-host`) is fenced, not refused: a cleanup command
//! always runs, and anything delayed is dropped if the lock, a cancel, a navigation or a reconnect overtook it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::future::BoxFuture;
use protocol::vault::{
    LoginEntry, VaultDeleteResult, VaultEntryResult, VaultFillResult, VaultFinishResult,
    VaultHostAction, VaultHostCommand, VaultStatus,
};
use serde::Serialize;

use super::host::Executor;

/// Why the gate refused a command. The texts are the app's own and never carry a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// A request from the window while the window is not trusted.
    Untrusted,
    /// No vault, or the app is shutting down.
    Unavailable,
    /// A lock, cancel, disconnect or a new connection overtook the command while it waited.
    ConnectionChanged,
    /// The bot lookup or the vault refused the command; the text is for the user.
    Vault(String),
}

impl fmt::Display for GateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            GateError::Untrusted => "Untrusted Logins request.",
            GateError::Unavailable => "Logins is unavailable. Check the host application.",
            GateError::ConnectionChanged => "The Logins connection changed. Try again.",
            GateError::Vault(message) => message.as_str(),
        })
    }
}

impl std::error::Error for GateError {}

/// What the app knows about its window when a request arrives from it (`main.ts:457`). The app fills this in; the gate
/// never calls into a window itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowState {
    /// The window exists and is not destroyed.
    pub present: bool,
    pub visible: bool,
    pub minimized: bool,
    /// The app's document has loaded and can take requests.
    pub document_ready: bool,
    pub quitting: bool,
}

impl WindowState {
    /// Trusted only while the window is shown and not minimized, its document is ready, and the app is not quitting.
    pub fn check_local(self) -> Result<(), GateError> {
        if self.present && self.visible && !self.minimized && self.document_ready && !self.quitting
        {
            Ok(())
        } else {
            Err(GateError::Untrusted)
        }
    }
}

/// `{}`: what `vault.cancel` and `disconnect` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Empty {}

/// A vault command's typed result, one variant per `vault.*` result. It serializes as that result, with no wrapper.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum VaultReply {
    Status(VaultStatus),
    List(Vec<LoginEntry>),
    Entry(VaultEntryResult),
    Delete(VaultDeleteResult),
    Fill(VaultFillResult),
    Finish(VaultFinishResult),
    Empty(Empty),
}

/// The login vault the gate guards (`login-vault.ts`).
pub trait Vault: Send + Sync {
    /// Locks now: a native entry window closes and the running operation is cancelled. A private browser stays private
    /// (the person clears or closes it), as in the TypeScript.
    fn lock(&self);
    fn command(&self, command: VaultHostCommand) -> BoxFuture<'_, Result<VaultReply, GateError>>;
}

/// What the gate reads from the app, each time it reads it.
pub trait GateInputs: Send + Sync {
    /// The vault, or `None` before there is one.
    fn vault(&self) -> Option<Arc<dyn Vault>>;
    /// Whether non-cleanup commands are taken: the app is not quitting (`main.ts:87`).
    fn ready(&self) -> bool;
    /// Which protected connection is current. A different one fences the work delayed across the change.
    fn connection(&self) -> Option<u64>;
    /// Looks up the bot a command names. It may wait, and the gate checks again after it.
    fn resolve_bot(&self, id: &str) -> BoxFuture<'_, Result<(), GateError>>;
}

/// Lock, cancel and disconnect: the urgent lane, and the commands that still run while the app is shutting down.
pub fn is_urgent(action: &VaultHostAction) -> bool {
    matches!(
        action,
        VaultHostAction::Lock | VaultHostAction::Cancel | VaultHostAction::Disconnect
    )
}

#[derive(Default)]
struct Fence {
    /// Bumped by a lock and by navigation: work delayed across either is dropped.
    epoch: u64,
    /// Per controller, how many cancels and disconnects it has sent.
    // ponytail: one entry per controller ever seen; bound it if controllers churn.
    cancelled: HashMap<String, u64>,
}

/// Every vault command passes this gate before the vault sees it (`vault-command-gate.ts`).
pub struct VaultCommandGate {
    inputs: Arc<dyn GateInputs>,
    fence: Mutex<Fence>,
}

impl VaultCommandGate {
    pub fn new(inputs: Arc<dyn GateInputs>) -> VaultCommandGate {
        VaultCommandGate {
            inputs,
            fence: Mutex::default(),
        }
    }

    fn fence(&self) -> MutexGuard<'_, Fence> {
        self.fence.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A command from the app's window: the window is checked first (`main.ts:457`), then the gate.
    pub async fn execute_local(
        &self,
        window: WindowState,
        command: VaultHostCommand,
    ) -> Result<VaultReply, GateError> {
        window.check_local()?;
        self.execute(command).await
    }

    /// A command from the window or the daemon.
    pub async fn execute(&self, command: VaultHostCommand) -> Result<VaultReply, GateError> {
        let urgent = is_urgent(&command.action);
        let vault = match self.inputs.vault() {
            Some(vault) if urgent || self.inputs.ready() => vault,
            _ => return Err(GateError::Unavailable),
        };
        let controller = command.controller_id.clone();
        let (epoch, cancelled, connection) = {
            let mut fence = self.fence();
            match command.action {
                VaultHostAction::Cancel | VaultHostAction::Disconnect => {
                    *fence.cancelled.entry(controller.clone()).or_insert(0) += 1;
                }
                VaultHostAction::Lock => fence.epoch += 1,
                _ => {}
            }
            (
                fence.epoch,
                fence.cancelled.get(&controller).copied(),
                self.inputs.connection(),
            )
        };
        // A bot lookup can wait, while a controller cancels or the protected host reconnects.
        if let Some(bot) = bot_id(&command.action) {
            self.inputs.resolve_bot(bot).await?;
        }
        let fenced = {
            let fence = self.fence();
            (!urgent && !self.inputs.ready())
                || fence.epoch != epoch
                || fence.cancelled.get(&controller).copied() != cancelled
                || self.inputs.connection() != connection
        };
        if fenced {
            return Err(GateError::ConnectionChanged);
        }
        vault.command(command).await
    }
}

impl Executor for VaultCommandGate {
    fn execute(&self, command: VaultHostCommand) -> BoxFuture<'_, Result<VaultReply, GateError>> {
        Box::pin(VaultCommandGate::execute(self, command))
    }
}

/// The bot a command names, which the gate looks up before it runs the command.
fn bot_id(action: &VaultHostAction) -> Option<&str> {
    match action {
        VaultHostAction::Entry(entry) => Some(entry.bot_id.as_str()),
        VaultHostAction::Fill(fill) => Some(fill.bot_id.as_str()),
        VaultHostAction::Finish(finish) => Some(finish.bot_id.as_str()),
        VaultHostAction::List(list) => list.bot_id.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;
    use protocol::vault::{VaultEntry, VaultHostAction, VaultHostCommand};
    use tokio::sync::{Notify, watch};

    use super::{GateError, GateInputs, Vault, VaultCommandGate, VaultReply, WindowState};

    #[derive(Default)]
    struct FakeVault {
        commands: Mutex<Vec<&'static str>>,
        locks: AtomicUsize,
    }

    impl FakeVault {
        fn kinds(&self) -> Vec<&'static str> {
            self.commands.lock().expect("commands").clone()
        }
    }

    impl Vault for FakeVault {
        fn lock(&self) {
            self.locks.fetch_add(1, Ordering::SeqCst);
        }

        fn command(
            &self,
            command: VaultHostCommand,
        ) -> BoxFuture<'_, Result<VaultReply, GateError>> {
            self.commands
                .lock()
                .expect("commands")
                .push(kind(&command.action));
            Box::pin(async { Ok(VaultReply::Empty(super::Empty {})) })
        }
    }

    /// The app's side of the gate. The bot lookup is delayed until the test releases it.
    struct Inputs {
        vault: Arc<FakeVault>,
        ready: AtomicBool,
        connection: AtomicU64,
        lookup: watch::Receiver<bool>,
        started: Arc<Notify>,
    }

    impl GateInputs for Inputs {
        fn vault(&self) -> Option<Arc<dyn Vault>> {
            let vault: Arc<dyn Vault> = self.vault.clone();
            Some(vault)
        }

        fn ready(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }

        fn connection(&self) -> Option<u64> {
            Some(self.connection.load(Ordering::SeqCst))
        }

        fn resolve_bot(&self, _id: &str) -> BoxFuture<'_, Result<(), GateError>> {
            let mut lookup = self.lookup.clone();
            Box::pin(async move {
                self.started.notify_one();
                let _ = lookup.wait_for(|done| *done).await;
                Ok(())
            })
        }
    }

    struct Fixture {
        gate: Arc<VaultCommandGate>,
        vault: Arc<FakeVault>,
        inputs: Arc<Inputs>,
        started: Arc<Notify>,
        release: watch::Sender<bool>,
    }

    fn fixture() -> Fixture {
        let vault = Arc::new(FakeVault::default());
        let (release, lookup) = watch::channel(false);
        let started = Arc::new(Notify::new());
        let inputs = Arc::new(Inputs {
            vault: vault.clone(),
            ready: AtomicBool::new(true),
            connection: AtomicU64::new(1),
            lookup,
            started: started.clone(),
        });
        let gate = Arc::new(VaultCommandGate::new(inputs.clone()));
        Fixture {
            gate,
            vault,
            inputs,
            started,
            release,
        }
    }

    fn kind(action: &VaultHostAction) -> &'static str {
        match action {
            VaultHostAction::Status => "status",
            VaultHostAction::List(_) => "list",
            VaultHostAction::Unlock => "unlock",
            VaultHostAction::Lock => "lock",
            VaultHostAction::Entry(_) => "entry",
            VaultHostAction::Delete(_) => "delete",
            VaultHostAction::Fill(_) => "fill",
            VaultHostAction::Finish(_) => "finish",
            VaultHostAction::Disconnect => "disconnect",
            VaultHostAction::Cancel => "cancel",
        }
    }

    fn command(controller: &str, action: VaultHostAction) -> VaultHostCommand {
        VaultHostCommand {
            controller_id: controller.into(),
            action,
        }
    }

    fn entry(controller: &str) -> VaultHostCommand {
        command(
            controller,
            VaultHostAction::Entry(VaultEntry {
                bot_id: "bot".into(),
                id: None,
                revision: None,
            }),
        )
    }

    fn open_window() -> WindowState {
        WindowState {
            present: true,
            visible: true,
            minimized: false,
            document_ready: true,
            quitting: false,
        }
    }

    #[tokio::test]
    async fn cancels_a_delayed_bot_lookup_before_native_entry_starts_and_allows_a_later_request() {
        let f = fixture();
        let gate = f.gate.clone();
        let pending = tokio::spawn(async move { gate.execute(entry("rpc:one")).await });
        f.started.notified().await;
        f.gate
            .execute(command("rpc:one", VaultHostAction::Cancel))
            .await
            .expect("cancel");
        f.release.send_replace(true);
        assert_eq!(
            pending.await.expect("joined"),
            Err(GateError::ConnectionChanged)
        );
        assert_eq!(f.vault.kinds(), ["cancel"]);
        f.gate
            .execute(entry("rpc:one"))
            .await
            .expect("a later entry");
        assert_eq!(f.vault.kinds().last(), Some(&"entry"));
    }

    #[tokio::test]
    async fn fences_delayed_native_work_after_lock_disconnect_and_reconnect() {
        for fence in ["lock", "disconnect", "reconnect"] {
            let f = fixture();
            let gate = f.gate.clone();
            let pending = tokio::spawn(async move { gate.execute(entry("rpc:one")).await });
            f.started.notified().await;
            match fence {
                "lock" => {
                    f.gate
                        .execute(command("local", VaultHostAction::Lock))
                        .await
                        .expect("lock");
                }
                "disconnect" => {
                    f.gate
                        .execute(command("rpc:one", VaultHostAction::Disconnect))
                        .await
                        .expect("disconnect");
                }
                _ => f.inputs.connection.store(2, Ordering::SeqCst),
            }
            f.release.send_replace(true);
            assert_eq!(
                pending.await.expect("joined"),
                Err(GateError::ConnectionChanged),
                "{fence}"
            );
            assert!(!f.vault.kinds().contains(&"entry"), "{fence}");
        }
    }

    #[tokio::test]
    async fn permits_only_cleanup_commands_during_application_shutdown() {
        let f = fixture();
        f.inputs.ready.store(false, Ordering::SeqCst);
        let refused = f.gate.execute(entry("rpc:one")).await;
        assert_eq!(refused, Err(GateError::Unavailable));
        assert!(refused.unwrap_err().to_string().contains("unavailable"));
        f.gate
            .execute(command("local", VaultHostAction::Lock))
            .await
            .expect("cleanup still runs");
        assert_eq!(f.vault.kinds(), ["lock"]);
    }

    #[test]
    fn a_local_request_is_accepted_only_while_the_window_is_shown_and_the_app_is_ready() {
        let open = open_window();
        assert_eq!(open.check_local(), Ok(()));
        let refused = [
            (
                "hidden",
                WindowState {
                    visible: false,
                    ..open
                },
            ),
            (
                "minimized",
                WindowState {
                    minimized: true,
                    ..open
                },
            ),
            (
                "quitting",
                WindowState {
                    quitting: true,
                    ..open
                },
            ),
            (
                "document not ready",
                WindowState {
                    document_ready: false,
                    ..open
                },
            ),
            (
                "no window",
                WindowState {
                    present: false,
                    ..open
                },
            ),
        ];
        for (why, state) in refused {
            assert_eq!(state.check_local(), Err(GateError::Untrusted), "{why}");
        }
    }

    #[tokio::test]
    async fn a_local_command_from_a_hidden_window_never_reaches_the_vault() {
        let f = fixture();
        let hidden = WindowState {
            visible: false,
            ..open_window()
        };
        assert_eq!(
            f.gate.execute_local(hidden, entry("local")).await,
            Err(GateError::Untrusted)
        );
        assert!(f.vault.kinds().is_empty());
        f.gate
            .execute_local(open_window(), command("local", VaultHostAction::Lock))
            .await
            .expect("an open window's request runs");
        assert_eq!(f.vault.kinds(), ["lock"]);
    }
}
