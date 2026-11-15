//! Port of `packages/ui/lib/logins.ts`: the client the Logins screen uses to reach a PC's vault host. It captures the selected
//! PC and the transport when it is created, so an answer that arrives after the PC or the connection changed reads as
//! cancelled, and never reaches a PC selected since. Three things it needs come from outside, as traits: the app's state
//! and connection (`LiveState`: `getState()` in store.ts, `daemon()` in connection.ts), and the desktop's bridge to its own
//! vault (`Desktop` and `NativeVault`: desktop.ts, the Electron preload, n/a in Rust).

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use client::Rpc;
use protocol::ClientMethod;
use protocol::vault::{
    EntryVersion, LoginEntry, VaultDeleteResult, VaultEntry, VaultEntryResult, VaultFill,
    VaultFillResult, VaultFinish, VaultFinishResult, VaultHostAction, VaultList, VaultStatus,
};
use protocol::vault_host::vault_host_result;
use protocol::wire::{Empty, Method, Request};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What every failure shows. It never says what went wrong: a vault's answer can carry secrets.
pub const LOGINS_PROBLEM: &str = "Logins could not complete the request safely. Check the selected host. Any private browser remains private until its fields are cleared or it is closed.";

/// Why a request failed. `Cancelled` is the TypeScript's `AbortError`; `Problem` is `LOGINS_PROBLEM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginsError {
    Cancelled,
    Problem,
}

impl fmt::Display for LoginsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("Logins request cancelled."),
            Self::Problem => formatter.write_str(LOGINS_PROBLEM),
        }
    }
}

impl std::error::Error for LoginsError {}

/// Why the vault gave no answer. `Aborted` is the TypeScript's `AbortError` (the call was stopped); `Failed` is anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFailure {
    Aborted,
    Failed,
}

/// What the vault host answered, not yet checked. It stays JSON until `vault_host_result` has checked it against the method
/// it answers, so a field the check doesn't know never reaches the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawAnswer(Value);

impl RawAnswer {
    /// Wraps an answer as the bridge or the daemon sent it.
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    fn into_value(self) -> Value {
        self.0
    }
}

/// A checked answer, typed by the action it answers. `vault.cancel` and `disconnect` answer `{}`, which is `Done`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultReply {
    /// `status`, `unlock` and `lock`.
    Status(VaultStatus),
    List(Vec<LoginEntry>),
    Entry(VaultEntryResult),
    Delete(VaultDeleteResult),
    Fill(VaultFillResult),
    Finish(VaultFinishResult),
    Done,
}

/// The app's state and connection as the logins client reads them: `getState()` (store.ts) and `daemon()` (connection.ts).
pub trait LiveState {
    /// `getState().host`: the saved PC this device is on, or `None` for the desktop's own Tjommie.
    fn selected_host(&self) -> Option<String>;
    /// `getState().connection.status === "ready"`.
    fn is_ready(&self) -> bool;
    /// Which daemon connection `daemon()` gives now, as a number that changes when the connection is replaced. `None` while
    /// there is none (the TypeScript's `daemon()` throws offline).
    fn daemon_generation(&self) -> Option<u64>;
}

/// The desktop's bridge as the logins client uses it: `desktop()?.vault` (desktop.ts).
pub trait Desktop {
    type Vault: NativeVault;
    /// The vault the bridge has now, or `None` without a bridge or a vault. Two handles to the same vault are equal.
    fn vault(&self) -> Option<Self::Vault>;
}

/// The desktop's own vault, reached directly (`window.tjommie.vault.command` in the TypeScript).
pub trait NativeVault: Clone + PartialEq {
    /// Sends one action and returns the answer unchecked. `Err(CommandFailure::Aborted)` is the TypeScript's `AbortError`.
    fn command(
        &self,
        action: &VaultHostAction,
    ) -> impl Future<Output = Result<RawAnswer, CommandFailure>>;
}

/// A `vault.*` method whose answer is read raw: `vault_host_result` checks it, as the TypeScript does, instead of a typed
/// result deserializing it and dropping what it doesn't know. The name and params are the protocol's.
macro_rules! raw_method {
    ($($ty:ident = $name:literal, $params:ty, $variant:ident;)*) => {
        $(
            struct $ty;

            impl Method for $ty {
                const NAME: &'static str = $name;
                type Params = $params;
                type Result = RawAnswer;

                fn take(request: Request) -> Option<$params> {
                    match request {
                        Request::$variant(params) => Some(params),
                        _ => None,
                    }
                }
            }

            impl ClientMethod for $ty {}
        )*
    };
}

raw_method! {
    StatusCall = "vault.status", Empty, VaultStatus;
    ListCall = "vault.list", VaultList, VaultList;
    UnlockCall = "vault.unlock", Empty, VaultUnlock;
    LockCall = "vault.lock", Empty, VaultLock;
    CancelCall = "vault.cancel", Empty, VaultCancel;
    EntryCall = "vault.entry", VaultEntry, VaultEntry;
    DeleteCall = "vault.delete", EntryVersion, VaultDelete;
    FillCall = "vault.fill", VaultFill, VaultFill;
    FinishCall = "vault.finish", VaultFinish, VaultFinish;
}

/// The client `createLoginsClient` returns. It calls through the transport it captured when it was created.
pub struct LoginsClient<'a, S: LiveState, D: Desktop, R: Rpc> {
    live: &'a S,
    desktop: &'a D,
    /// `getState().host` when the client was created.
    host: Option<String>,
    /// This PC's own vault, when the client was created with no host selected and the bridge has a vault.
    native: Option<D::Vault>,
    /// The daemon client and its generation, when the client talks to a daemon.
    daemon: Option<(u64, &'a R)>,
    /// Goes up on each lock, cancel and disconnect: an answer from before one of those reads as cancelled.
    epoch: AtomicU64,
    /// Interactive calls in flight. `cancel` only sends a cancel when one is.
    pending: AtomicUsize,
}

/// `createLoginsClient()`: captures the selected PC now, and the transport it talks through. `daemon` is the connection's
/// client with its generation, `None` while offline; it is ignored when this PC's own vault is used.
pub fn create_logins_client<'a, S: LiveState, D: Desktop, R: Rpc>(
    live: &'a S,
    desktop: &'a D,
    daemon: Option<(u64, &'a R)>,
) -> LoginsClient<'a, S, D, R> {
    let host = live.selected_host();
    // `host === null ? desktop()?.vault : undefined`: only the desktop's own Tjommie talks to its vault directly.
    let native = if host.is_none() {
        desktop.vault()
    } else {
        None
    };
    let daemon = if native.is_none() { daemon } else { None };
    LoginsClient {
        live,
        desktop,
        host,
        native,
        daemon,
        epoch: AtomicU64::new(0),
        pending: AtomicUsize::new(0),
    }
}

impl<S: LiveState, D: Desktop, R: Rpc> LoginsClient<'_, S, D, R> {
    /// Whether the PC and the transport captured at creation are still the ones in use.
    pub fn current(&self) -> bool {
        if self.live.selected_host() != self.host {
            return false;
        }
        if let Some(native) = &self.native {
            // `desktop()?.vault === native`
            return self.desktop.vault().is_some_and(|vault| vault == *native);
        }
        match self.daemon {
            // `getState().connection.status === "ready" && daemon() === client`
            Some((generation, _)) => {
                self.live.is_ready() && self.live.daemon_generation() == Some(generation)
            }
            None => false,
        }
    }

    /// Sends one action (`send`). A check that fails, or an answer after a lock, cancel or disconnect, reads as cancelled.
    pub async fn send(&self, action: VaultHostAction) -> Result<VaultReply, LoginsError> {
        if !self.current() {
            return Err(LoginsError::Problem);
        }
        if matches!(
            action,
            VaultHostAction::Lock | VaultHostAction::Cancel | VaultHostAction::Disconnect
        ) {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        let revision = self.epoch.load(Ordering::SeqCst);
        // Reads and the actions that end things don't count as in flight: only what the person is waiting on does.
        let interactive = !matches!(
            action,
            VaultHostAction::Status
                | VaultHostAction::List(_)
                | VaultHostAction::Lock
                | VaultHostAction::Cancel
                | VaultHostAction::Disconnect
        );
        if interactive {
            self.pending.fetch_add(1, Ordering::SeqCst);
        }
        let outcome = match self.invoke(&action).await {
            Ok(answer) => typed_reply(&action, answer),
            Err(failure) => Err(failure),
        };
        let result = match outcome {
            Ok(reply) if self.current() && revision == self.epoch.load(Ordering::SeqCst) => {
                Ok(reply)
            }
            Ok(_) => Err(LoginsError::Cancelled),
            Err(failure)
                if !self.current()
                    || revision != self.epoch.load(Ordering::SeqCst)
                    || failure == CommandFailure::Aborted =>
            {
                Err(LoginsError::Cancelled)
            }
            Err(_) => Err(LoginsError::Problem),
        };
        if interactive {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }

    /// Cancels the operation in flight, if there is one (`cancel`). Doesn't end the connection: a later request works.
    pub async fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if self.pending.load(Ordering::SeqCst) > 0 {
            // The TypeScript ignores this call's answer: the cancelled request reports its own outcome.
            let _ = self.invoke(&VaultHostAction::Cancel).await;
        }
    }

    /// Calls the captured transport: this PC's vault, or the daemon.
    async fn invoke(&self, action: &VaultHostAction) -> Result<RawAnswer, CommandFailure> {
        if let Some(native) = &self.native {
            return native.command(action).await;
        }
        let Some((_, rpc)) = self.daemon else {
            return Err(CommandFailure::Failed);
        };
        let answer = match action {
            VaultHostAction::Status => rpc.call::<StatusCall>(Empty::default()).await,
            VaultHostAction::List(params) => rpc.call::<ListCall>(params.clone()).await,
            VaultHostAction::Unlock => rpc.call::<UnlockCall>(Empty::default()).await,
            VaultHostAction::Lock => rpc.call::<LockCall>(Empty::default()).await,
            VaultHostAction::Entry(params) => rpc.call::<EntryCall>(params.clone()).await,
            VaultHostAction::Delete(params) => rpc.call::<DeleteCall>(params.clone()).await,
            VaultHostAction::Fill(params) => rpc.call::<FillCall>(params.clone()).await,
            VaultHostAction::Finish(params) => rpc.call::<FinishCall>(params.clone()).await,
            // `disconnect` is sent as `vault.cancel`, as the TypeScript does.
            VaultHostAction::Cancel | VaultHostAction::Disconnect => {
                rpc.call::<CancelCall>(Empty::default()).await
            }
        };
        answer.map_err(|_| CommandFailure::Failed)
    }
}

/// Checks an answer against its action (`vaultHostResult`), then reads it as that action's reply. An answer the check
/// refuses, or that doesn't read as its reply type, is a failure.
fn typed_reply(action: &VaultHostAction, answer: RawAnswer) -> Result<VaultReply, CommandFailure> {
    let value =
        vault_host_result(action, answer.into_value()).map_err(|_| CommandFailure::Failed)?;
    Ok(match action {
        VaultHostAction::Status | VaultHostAction::Unlock | VaultHostAction::Lock => {
            VaultReply::Status(read(value)?)
        }
        VaultHostAction::List(_) => VaultReply::List(read(value)?),
        VaultHostAction::Entry(_) => VaultReply::Entry(read(value)?),
        VaultHostAction::Delete(_) => VaultReply::Delete(read(value)?),
        VaultHostAction::Fill(_) => VaultReply::Fill(read(value)?),
        VaultHostAction::Finish(_) => VaultReply::Finish(read(value)?),
        VaultHostAction::Cancel | VaultHostAction::Disconnect => VaultReply::Done,
    })
}

fn read<T: DeserializeOwned>(value: Value) -> Result<T, CommandFailure> {
    serde_json::from_value(value).map_err(|_| CommandFailure::Failed)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};

    use client::RpcError;
    use protocol::vault::VaultState;
    use serde_json::json;
    use tokio::sync::oneshot;

    use super::*;
    use crate::model::testing::FakeRpc;

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().expect("the test's lock isn't poisoned")
    }

    fn status() -> VaultStatus {
        VaultStatus {
            state: VaultState::Unlocked,
            reason: None,
            pending_deletes: 0,
        }
    }

    fn status_json() -> Value {
        json!({ "state": "unlocked", "reason": null, "pendingDeletes": 0 })
    }

    fn entry_action() -> VaultHostAction {
        VaultHostAction::Entry(VaultEntry {
            bot_id: "one".to_owned(),
            id: None,
            revision: None,
        })
    }

    /// The app's state as the logins client reads it. A test changes the host and the connection.
    #[derive(Default)]
    struct FakeLive {
        host: RefCell<Option<String>>,
        ready: Cell<bool>,
        generation: Cell<Option<u64>>,
    }

    impl FakeLive {
        /// Selects a remote PC and connects to it.
        fn select_remote(&self, host: &str) {
            self.set_host(Some(host));
            self.ready.set(true);
            self.generation.set(Some(1));
        }

        fn set_host(&self, host: Option<&str>) {
            *self.host.borrow_mut() = host.map(str::to_owned);
        }
    }

    impl LiveState for FakeLive {
        fn selected_host(&self) -> Option<String> {
            self.host.borrow().clone()
        }

        fn is_ready(&self) -> bool {
            self.ready.get()
        }

        fn daemon_generation(&self) -> Option<u64> {
            self.generation.get()
        }
    }

    struct FakeVaultInner {
        asked: Mutex<Vec<VaultHostAction>>,
        answer: Result<RawAnswer, CommandFailure>,
    }

    /// The desktop's vault: it records what it's asked and answers with a fixed answer.
    #[derive(Clone)]
    struct FakeVault(Arc<FakeVaultInner>);

    impl PartialEq for FakeVault {
        fn eq(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.0, &other.0)
        }
    }

    impl NativeVault for FakeVault {
        fn command(
            &self,
            action: &VaultHostAction,
        ) -> impl Future<Output = Result<RawAnswer, CommandFailure>> {
            locked(&self.0.asked).push(action.clone());
            let answer = self.0.answer.clone();
            async move { answer }
        }
    }

    #[derive(Default)]
    struct FakeDesktop {
        vault: RefCell<Option<FakeVault>>,
    }

    impl FakeDesktop {
        fn with_vault(answer: Result<RawAnswer, CommandFailure>) -> Self {
            let desktop = Self::default();
            *desktop.vault.borrow_mut() = Some(FakeVault(Arc::new(FakeVaultInner {
                asked: Mutex::new(Vec::new()),
                answer,
            })));
            desktop
        }

        /// Every action the vault was asked, in order.
        fn asked(&self) -> Vec<VaultHostAction> {
            self.vault
                .borrow()
                .as_ref()
                .map(|vault| locked(&vault.0.asked).clone())
                .unwrap_or_default()
        }
    }

    impl Desktop for FakeDesktop {
        type Vault = FakeVault;

        fn vault(&self) -> Option<FakeVault> {
            self.vault.borrow().clone()
        }
    }

    /// A daemon that answers from a table, as `FakeRpc` does, except that a method can be held until the test releases
    /// it. `FakeRpc` answers at once, and these tests need a call to stay open while another one runs.
    struct HeldRpc {
        answers: HashMap<&'static str, Value>,
        held: Mutex<HashMap<&'static str, oneshot::Receiver<Value>>>,
        calls: Mutex<Vec<(&'static str, Value)>>,
    }

    impl HeldRpc {
        fn new(answers: &[(&'static str, Value)]) -> Self {
            Self {
                answers: answers.iter().cloned().collect(),
                held: Mutex::new(HashMap::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Holds `method`'s next answer until the returned sender sends one.
        fn hold(&self, method: &'static str) -> oneshot::Sender<Value> {
            let (sender, receiver) = oneshot::channel();
            locked(&self.held).insert(method, receiver);
            sender
        }

        /// Every call so far: the method's name and its params.
        fn calls(&self) -> Vec<(&'static str, Value)> {
            locked(&self.calls).clone()
        }
    }

    impl Rpc for HeldRpc {
        fn call<M: ClientMethod>(
            &self,
            params: M::Params,
        ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
            let name = M::NAME;
            let params = serde_json::to_value(params).unwrap_or(Value::Null);
            locked(&self.calls).push((name, params));
            let held = locked(&self.held).remove(name);
            let answer = self.answers.get(name).cloned().unwrap_or(Value::Null);
            async move {
                let value = match held {
                    Some(receiver) => receiver
                        .await
                        .map_err(|_| RpcError::BadResult("the answer was dropped".to_owned()))?,
                    None => answer,
                };
                serde_json::from_value::<M::Result>(value)
                    .map_err(|error| RpcError::BadResult(error.to_string()))
            }
        }
    }

    #[tokio::test]
    async fn captures_the_selected_host_and_refuses_a_late_response_after_host_client_replacement()
    {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let rpc = HeldRpc::new(&[]);
        let release = rpc.hold("vault.status");
        let client = create_logins_client(&live, &desktop, Some((1, &rpc)));
        let answered_late = client.send(VaultHostAction::Status);
        let host_changes = async {
            live.set_host(Some("different-host"));
            let _ = release.send(status_json());
        };
        let (result, ()) = tokio::join!(answered_late, host_changes);
        assert_eq!(result, Err(LoginsError::Cancelled));
        assert_eq!(
            client
                .send(VaultHostAction::List(VaultList { bot_id: None }))
                .await,
            Err(LoginsError::Problem)
        );
        assert_eq!(rpc.calls().len(), 1);
    }

    #[tokio::test]
    async fn cancels_the_captured_operation_without_tombstoning_the_connection_and_accepts_a_later_fresh_request()
     {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let rpc = HeldRpc::new(&[("vault.status", status_json()), ("vault.cancel", json!({}))]);
        let release = rpc.hold("vault.entry");
        let client = create_logins_client(&live, &desktop, Some((1, &rpc)));
        let entry = client.send(entry_action());
        let cancel = async {
            client.cancel().await;
            assert!(
                rpc.calls().contains(&("vault.cancel", json!({}))),
                "the cancel is sent as vault.cancel"
            );
            let _ = release.send(json!({ "status": "cancelled", "entry": null }));
        };
        let (result, ()) = tokio::join!(entry, cancel);
        assert_eq!(result, Err(LoginsError::Cancelled));
        assert_eq!(
            client.send(VaultHostAction::Status).await,
            Ok(VaultReply::Status(status()))
        );
    }

    #[tokio::test]
    async fn uses_the_direct_native_api_only_for_this_pc_and_rejects_secret_bearing_responses_without_repeating_them()
     {
        let live = FakeLive::default();
        let desktop = FakeDesktop::with_vault(Ok(RawAnswer::new(status_json())));
        let no_daemon: Option<(u64, &FakeRpc)> = None;
        let native = create_logins_client(&live, &desktop, no_daemon);
        assert_eq!(
            native.send(VaultHostAction::Status).await,
            Ok(VaultReply::Status(status()))
        );
        assert_eq!(desktop.asked(), vec![VaultHostAction::Status]);

        live.select_remote("selected-host");
        let secret = json!({
            "state": "unlocked",
            "reason": null,
            "pendingDeletes": 0,
            "password": "SYNTHETIC_NEVER_DISPLAY"
        });
        let rpc = FakeRpc::new().on::<StatusCall>(move |_| Ok(RawAnswer::new(secret.clone())));
        let daemon = create_logins_client(&live, &desktop, Some((1, &rpc)));
        let error = daemon
            .send(VaultHostAction::Status)
            .await
            .expect_err("a secret-bearing answer is refused");
        assert_eq!(error, LoginsError::Problem);
        assert_eq!(error.to_string(), LOGINS_PROBLEM);
        assert!(!format!("{error:?} {error}").contains("SYNTHETIC_NEVER_DISPLAY"));
        assert_eq!(desktop.asked().len(), 1);
    }

    // Decisions the TypeScript doesn't test.

    #[tokio::test]
    async fn a_native_abort_reads_as_cancelled_and_any_other_native_failure_as_the_problem() {
        let live = FakeLive::default();
        let no_daemon: Option<(u64, &FakeRpc)> = None;
        let aborted = FakeDesktop::with_vault(Err(CommandFailure::Aborted));
        let client = create_logins_client(&live, &aborted, no_daemon);
        assert_eq!(
            client.send(VaultHostAction::Unlock).await,
            Err(LoginsError::Cancelled)
        );
        let failed = FakeDesktop::with_vault(Err(CommandFailure::Failed));
        let client = create_logins_client(&live, &failed, no_daemon);
        assert_eq!(
            client.send(VaultHostAction::Unlock).await,
            Err(LoginsError::Problem)
        );
    }

    #[tokio::test]
    async fn disconnect_is_sent_as_vault_cancel_and_answers_done() {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let rpc = HeldRpc::new(&[("vault.cancel", json!({}))]);
        let client = create_logins_client(&live, &desktop, Some((1, &rpc)));
        assert_eq!(
            client.send(VaultHostAction::Disconnect).await,
            Ok(VaultReply::Done)
        );
        assert_eq!(rpc.calls(), vec![("vault.cancel", json!({}))]);
    }

    #[tokio::test]
    async fn cancel_sends_nothing_when_no_operation_is_running() {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let rpc = HeldRpc::new(&[]);
        let client = create_logins_client(&live, &desktop, Some((1, &rpc)));
        client.cancel().await;
        assert!(rpc.calls().is_empty());
    }

    #[tokio::test]
    async fn a_lock_during_an_entry_cancels_the_entry() {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let rpc = HeldRpc::new(&[("vault.lock", status_json())]);
        let release = rpc.hold("vault.entry");
        let client = create_logins_client(&live, &desktop, Some((1, &rpc)));
        let entry = client.send(entry_action());
        let lock = async {
            let outcome = client.send(VaultHostAction::Lock).await;
            let _ = release.send(json!({ "status": "cancelled", "entry": null }));
            outcome
        };
        let (entry_result, lock_result) = tokio::join!(entry, lock);
        assert_eq!(entry_result, Err(LoginsError::Cancelled));
        assert_eq!(lock_result, Ok(VaultReply::Status(status())));
    }

    #[tokio::test]
    async fn a_daemon_answer_of_the_wrong_shape_reads_as_the_problem() {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let rpc = HeldRpc::new(&[("vault.status", json!({ "state": "nope" }))]);
        let client = create_logins_client(&live, &desktop, Some((1, &rpc)));
        assert_eq!(
            client.send(VaultHostAction::Status).await,
            Err(LoginsError::Problem)
        );
    }

    #[tokio::test]
    async fn with_no_daemon_while_offline_nothing_is_sent_and_the_request_reads_as_the_problem() {
        let live = FakeLive::default();
        live.select_remote("selected-host");
        let desktop = FakeDesktop::default();
        let no_daemon: Option<(u64, &HeldRpc)> = None;
        let client = create_logins_client(&live, &desktop, no_daemon);
        assert_eq!(
            client.send(VaultHostAction::Status).await,
            Err(LoginsError::Problem)
        );
    }
}
