//! `login-vault.ts`: [`NativeLoginVault`], the command layer of the login vault. It decides what a Logins command may do:
//! list, save, fill, delete, finish, lock and unlock. Each one runs under the isolation lease, the fences of epochs and
//! connections, and the native confirmation. Secrets pass only between the entry window, the secret store and the
//! browser, as [`VaultSecret`] values that wipe themselves.
//!
//! The browser, the entry window and the secret store are traits, so the command layer runs under test with fakes. The
//! app plugs in the real engine, the GPUI entry window and Credential Manager ([`LoginCredentials`]).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use protocol::vault::{
    DeleteStatus, EntryStatus, EntryVersion, FillStatus, FinishMode, FinishStatus, LoginEntry,
    LoginOrigin, VaultDeleteResult, VaultEntry, VaultEntryResult, VaultFill, VaultFillResult,
    VaultFillTarget, VaultFinish, VaultFinishResult, VaultHostAction, VaultHostCommand, VaultState,
    VaultStatus,
};
use tokio::sync::watch;
use uuid::Uuid;

use super::credential::LoginCredentials;
use super::entry::{ENTRY_TIMEOUT, EntryInput, answer_or_cancel};
use super::gate::{Empty, GateError, Vault, VaultReply};
use super::storage::{StoredLogin, VaultError, VaultRecords, VaultSecret, validate_vault_secret};
use super::worker::{VaultIsolation, VaultIsolationLease};

/// The generic failure. The TypeScript's text, which a failure inside an operation becomes, except the two below.
const GENERIC: &str = "Logins could not finish this operation safely. Refresh Logins or check the host. Any private browser stays private until you clear its fields or close it.";
/// Refused for a closed vault, a refused controller, and an operation a cancel or a lock overtook.
const CHANGED: &str = "The Logins operation was cancelled or its security boundary changed.";
/// Refused when a second operation starts while one is in flight.
const IN_FLIGHT: &str = "Finish the current Logins operation on the host first.";
/// A fill that finds the login's record but not its secret. The browser is already private by then, and stays so.
const SECRET_MISSING: &str = "The saved password for this login is missing. Save the login again. The browser stays private until you clear its fields or close it.";
/// A save whose secret was not there afterwards, although it was written twice.
const SECRET_NOT_KEPT: &str =
    "Windows did not keep the password, so the login was not saved. Try again.";
/// How long a new secret is left before it is looked for. A dropped one was gone within about 20 ms.
const SETTLE: Duration = Duration::from_millis(50);
/// The waits before an entry that is not found is believed missing. One hidden by another write was back within 500 ms.
const READ_RETRIES: [Duration; 3] = [
    Duration::from_millis(20),
    Duration::from_millis(80),
    Duration::from_millis(400),
];
/// The most controllers that may disconnect before the vault refuses every remote one (`vault-command-gate.ts`).
const CLOSED_CONTROLLER_LIMIT: usize = 4096;

/// Why a step of a Logins operation failed. The vault shows [`GENERIC`] for each, except the two about a missing secret,
/// which say what to do. None carries a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginError {
    /// The operation was cancelled, or its security boundary changed (`changed()`).
    Changed,
    /// Logins is locked (`locked()`).
    Locked,
    /// A browser, store, window or isolation step failed. Its cause is not kept.
    Failed,
    /// The record is there and its secret is not ([`SECRET_MISSING`]).
    SecretMissing,
    /// A new secret did not stay in the store ([`SECRET_NOT_KEPT`]).
    SecretNotKept,
}

impl From<VaultError> for LoginError {
    fn from(_: VaultError) -> Self {
        LoginError::Failed
    }
}

/// The cancel signal of one operation. It fires when the operation is cancelled, its controller disconnects, or the
/// vault locks. The entry window and the confirmation must stop when it fires.
#[derive(Clone)]
pub struct Cancel(watch::Receiver<bool>);

impl Cancel {
    /// A new signal, and the sender that fires it.
    pub fn pair() -> (watch::Sender<bool>, Cancel) {
        let (sender, receiver) = watch::channel(false);
        (sender, Cancel(receiver))
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.0.clone();
        let _ = receiver.wait_for(|done| *done).await;
    }
}

/// Checks that the operation may still go on. The browser calls it between its steps: an error means the operation was
/// cancelled, locked or lost its isolation, and the browser must stop.
pub type Check = Arc<dyn Fn() -> Result<(), LoginError> + Send + Sync>;

/// A native yes-or-no the person answers before a Logins step runs (`VaultConfirmation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmation {
    Unlock,
    Delete {
        entry: LoginEntry,
    },
    Fill {
        entry: LoginEntry,
        destination: LoginOrigin,
    },
    Finish {
        mode: FinishMode,
    },
}

/// The secret store: a login's username and password, by opaque reference. [`LoginCredentials`] is the real one (Windows
/// Credential Manager). Tests use a fake.
// ponytail: the store is synchronous, so Credential Manager calls run inline on the command's thread; a store behind
// a worker thread if they ever stall the runtime.
pub trait SecretStore: Send + Sync {
    fn write(&self, reference: &str, secret: &VaultSecret) -> Result<(), VaultError>;
    fn read(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError>;
    fn delete(&self, reference: &str) -> Result<(), VaultError>;

    /// `read`, asking again for a short while before believing there is nothing: Credential Manager can miss an entry
    /// while another one is being written.
    fn read_settled(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError> {
        let mut found = self.read(reference)?;
        for wait in READ_RETRIES {
            if found.is_some() {
                break;
            }
            std::thread::sleep(wait);
            found = self.read(reference)?;
        }
        Ok(found)
    }

    /// `write`, then reads the secret back once it has settled, and writes it once more if it is not there as written:
    /// Credential Manager can drop a new entry while another process is reading credentials. `false` when it is still
    /// not there after that.
    // ponytail: an entry dropped later than SETTLE slips through, and the fill then says the password is missing.
    fn write_kept(&self, reference: &str, secret: &VaultSecret) -> Result<bool, VaultError> {
        for _ in 0..2 {
            self.write(reference, secret)?;
            std::thread::sleep(SETTLE);
            if self.read_settled(reference)?.is_some_and(|kept| {
                kept.username() == secret.username() && kept.password() == secret.password()
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl SecretStore for LoginCredentials {
    fn write(&self, reference: &str, secret: &VaultSecret) -> Result<(), VaultError> {
        LoginCredentials::write(self, reference, secret)
    }

    fn read(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError> {
        LoginCredentials::read(self, reference)
    }

    fn delete(&self, reference: &str) -> Result<(), VaultError> {
        LoginCredentials::delete(self, reference)
    }
}

/// The browser side of a private fill (`native-browser.ts`: `vaultDestination`, `beginVault`, `writeVault`,
/// `finishVault`). The engine implements it. The vault never touches a page itself.
pub trait LoginBrowser: Send + Sync {
    /// The exact origin of the bot's page for `target`, when that page is a fillable HTTPS page.
    fn destination(
        &self,
        target: VaultFillTarget,
        owner: String,
    ) -> BoxFuture<'_, Result<LoginOrigin, LoginError>>;

    /// Starts a private interaction for `origin` on the target's page, and returns its private epoch.
    fn begin(
        &self,
        target: VaultFillTarget,
        owner: String,
        origin: LoginOrigin,
        check: Check,
    ) -> BoxFuture<'_, Result<u64, LoginError>>;

    /// Writes the secret into the fields of the private interaction `private_epoch`. The secret is consumed.
    fn fill(
        &self,
        bot_id: String,
        private_epoch: u64,
        secret: VaultSecret,
        check: Check,
    ) -> BoxFuture<'_, Result<(), LoginError>>;

    /// Clears the private fields, or closes the private browser, for `private_epoch`.
    fn finish(
        &self,
        bot_id: String,
        generation: String,
        private_epoch: u64,
        mode: FinishMode,
        check: Check,
    ) -> BoxFuture<'_, Result<(), LoginError>>;
}

/// The native windows the person answers: the secure entry for a login, and the yes-or-no confirmations. The window
/// answers on the trusted surface. The vault only opens it and reads its answer.
pub trait EntryWindow: Send + Sync {
    /// Opens the secure entry for `bot_id`, prefilled from `previous` when editing. Answers with the typed fields, or
    /// `None` on a cancel. The window must stop when `cancel` fires.
    fn entry(
        &self,
        bot_id: String,
        previous: Option<LoginEntry>,
        cancel: Cancel,
    ) -> BoxFuture<'_, Result<Option<EntryInput>, LoginError>>;

    /// Asks one confirmation: `false` on a cancel. The window must stop when `cancel` fires.
    fn confirm(
        &self,
        request: Confirmation,
        cancel: Cancel,
    ) -> BoxFuture<'_, Result<bool, LoginError>>;

    /// Closes any open Logins window. A lock closes it.
    fn close(&self);
}

/// Everything a [`NativeLoginVault`] is built from.
pub struct VaultParts {
    pub records: VaultRecords,
    pub secrets: Arc<dyn SecretStore>,
    pub isolation: Arc<dyn VaultIsolation>,
    pub entry: Arc<dyn EntryWindow>,
    pub browser: Arc<dyn LoginBrowser>,
}

/// The login vault. It starts locked, as it does after every restart.
pub struct NativeLoginVault {
    inner: Arc<Inner>,
}

impl NativeLoginVault {
    pub fn new(parts: VaultParts) -> Self {
        Self {
            inner: Arc::new(Inner {
                records: Mutex::new(parts.records),
                secrets: parts.secrets,
                isolation: parts.isolation,
                entry: parts.entry,
                browser: parts.browser,
                state: Mutex::new(State::new()),
            }),
        }
    }

    /// Locks, then refuses every later command.
    pub fn close(&self) {
        self.lock();
        self.inner.state().closed = true;
    }

    async fn run(&self, command: VaultHostCommand) -> Result<VaultReply, GateError> {
        let VaultHostCommand {
            controller_id: owner,
            action,
        } = command;
        if !well_formed(&owner, &action) {
            return Err(generic());
        }
        if self.inner.state().closed {
            return Err(changed());
        }
        match action {
            VaultHostAction::Cancel => {
                self.inner.cancel_owner(&owner);
                return Ok(empty());
            }
            VaultHostAction::Disconnect => {
                self.inner.disconnect(&owner);
                return Ok(empty());
            }
            _ => {}
        }
        if owner.starts_with("rpc:") && self.inner.refuses(&owner) {
            return Err(changed());
        }
        match action {
            VaultHostAction::Lock => self.lock_command().await,
            VaultHostAction::Status => status_reply(&self.inner).await,
            VaultHostAction::List(list) => self.list(list.bot_id.as_deref()),
            action => self.operate(&owner, action).await,
        }
    }

    /// `lock` answers the status. It fences every delayed step, cancels the running operation and closes the entry window.
    /// A private browser this vault opened is left private, as the TypeScript leaves it: the bot stays shut out, and the
    /// person clears its fields or closes it.
    async fn lock_command(&self) -> Result<VaultReply, GateError> {
        self.inner.lock_now();
        self.inner.entry.close();
        status_reply(&self.inner).await
    }

    fn list(&self, bot_id: Option<&str>) -> Result<VaultReply, GateError> {
        self.inner
            .records()
            .list(bot_id)
            .map(VaultReply::List)
            .map_err(|error| GateError::Vault(error.to_string()))
    }

    /// Everything else: one operation at a time, under a lease, with the fence held from its start to its end.
    async fn operate(&self, owner: &str, action: VaultHostAction) -> Result<VaultReply, GateError> {
        // The fence clears the operation slot and releases the lease when it drops, even if this future is dropped.
        let fence = Arc::new(self.inner.open(owner)?);
        perform(&fence, owner, action)
            .await
            .map_err(|error| match error {
                LoginError::SecretMissing => GateError::Vault(SECRET_MISSING.to_owned()),
                LoginError::SecretNotKept => GateError::Vault(SECRET_NOT_KEPT.to_owned()),
                LoginError::Changed | LoginError::Locked | LoginError::Failed => generic(),
            })
    }
}

impl Vault for NativeLoginVault {
    /// Locks now: the entry window closes and the running operation is cancelled. A private browser stays private.
    fn lock(&self) {
        self.inner.lock_now();
        self.inner.entry.close();
    }

    fn command(&self, command: VaultHostCommand) -> BoxFuture<'_, Result<VaultReply, GateError>> {
        Box::pin(async move { self.run(command).await })
    }
}

/// The state shared by the vault's commands. Each lock is taken for one read or write and never held across an await.
struct Inner {
    records: Mutex<VaultRecords>,
    secrets: Arc<dyn SecretStore>,
    isolation: Arc<dyn VaultIsolation>,
    entry: Arc<dyn EntryWindow>,
    browser: Arc<dyn LoginBrowser>,
    state: Mutex<State>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn records(&self) -> MutexGuard<'_, VaultRecords> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the operation slot for `owner`, or refuses while another operation holds it.
    fn open(self: &Arc<Self>, owner: &str) -> Result<Fence, GateError> {
        let mut state = self.state();
        if state.operation.is_some() {
            return Err(GateError::Vault(IN_FLIGHT.to_owned()));
        }
        state.next_operation = state.next_operation.saturating_add(1);
        let id = state.next_operation;
        let (sender, cancel) = Cancel::pair();
        state.operation = Some(Running {
            id,
            owner: owner.to_owned(),
            cancel: sender,
        });
        Ok(Fence {
            inner: self.clone(),
            id,
            epoch: state.epoch,
            cancel,
            lease: OnceLock::new(),
        })
    }

    fn clear_operation(&self, id: u64) {
        let mut state = self.state();
        if state
            .operation
            .as_ref()
            .is_some_and(|running| running.id == id)
        {
            state.operation = None;
        }
    }

    /// `cancel` from a controller: stops its own operation.
    fn cancel_owner(&self, owner: &str) {
        let state = self.state();
        if let Some(running) = &state.operation
            && running.owner == owner
        {
            running.cancel.send_replace(true);
        }
    }

    /// `disconnect` from a controller: tombstones it when remote, and stops its own operation.
    fn disconnect(&self, owner: &str) {
        let mut state = self.state();
        if owner.starts_with("rpc:") {
            if state.closed_controllers.len() >= CLOSED_CONTROLLER_LIMIT {
                state.remote_closed = true;
            } else {
                state.closed_controllers.insert(owner.to_owned());
            }
        }
        if let Some(running) = &state.operation
            && running.owner == owner
        {
            running.cancel.send_replace(true);
        }
    }

    fn refuses(&self, owner: &str) -> bool {
        let state = self.state();
        state.remote_closed || state.closed_controllers.contains(owner)
    }

    /// Locks: bumps the epoch so every delayed step is fenced, and cancels the running operation. The private browser this
    /// vault opened, if any, stays tracked: finishing it (clear or close) remains possible.
    fn lock_now(&self) {
        let mut state = self.state();
        state.locked = true;
        state.epoch = state.epoch.saturating_add(1);
        if let Some(running) = &state.operation {
            running.cancel.send_replace(true);
        }
    }
}

struct State {
    locked: bool,
    epoch: u64,
    closed: bool,
    operation: Option<Running>,
    next_operation: u64,
    closed_controllers: HashSet<String>,
    remote_closed: bool,
    private: Option<PrivateSession>,
}

impl State {
    fn new() -> Self {
        Self {
            locked: true,
            epoch: 0,
            closed: false,
            operation: None,
            next_operation: 0,
            closed_controllers: HashSet::new(),
            remote_closed: false,
            private: None,
        }
    }
}

/// The one operation in flight.
struct Running {
    id: u64,
    owner: String,
    cancel: watch::Sender<bool>,
}

/// A private browser interaction this vault started and has not finished. A lock closes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrivateSession {
    bot_id: String,
    generation: String,
    epoch: u64,
}

/// One operation's fence: the lock epoch it began in, its cancel signal and its isolation lease once acquired. Dropping
/// it releases the lease and frees the operation slot, so an operation that is dropped midway cannot wedge the vault.
struct Fence {
    inner: Arc<Inner>,
    id: u64,
    epoch: u64,
    cancel: Cancel,
    lease: OnceLock<Arc<dyn VaultIsolationLease>>,
}

impl Drop for Fence {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.get() {
            lease.release();
        }
        self.inner.clear_operation(self.id);
    }
}

impl Fence {
    /// `Changed` after a close, a lock, or a cancel. `Failed` once the isolation lease has lapsed.
    fn check(&self) -> Result<(), LoginError> {
        let stale = {
            let state = self.inner.state();
            state.closed || state.epoch != self.epoch
        };
        if stale || self.cancel.is_cancelled() {
            return Err(LoginError::Changed);
        }
        if let Some(lease) = self.lease.get()
            && lease.assert().is_err()
        {
            return Err(LoginError::Failed);
        }
        Ok(())
    }

    /// Whether a lock has come since the operation began.
    fn check_fn(self: &Arc<Self>) -> Check {
        let fence = self.clone();
        Arc::new(move || fence.check())
    }

    /// The entry window, which gives up as a cancel when this operation's signal fires or the window times out.
    async fn ask_entry(
        &self,
        bot_id: String,
        previous: Option<LoginEntry>,
    ) -> Result<Option<EntryInput>, LoginError> {
        let answer = self
            .inner
            .entry
            .entry(bot_id, previous, self.cancel.clone());
        match answer_or_cancel(answer, &self.cancel, ENTRY_TIMEOUT).await {
            Some(result) => result,
            None => {
                self.inner.entry.close();
                Ok(None)
            }
        }
    }

    /// A confirmation. A cancel is a `false`.
    async fn ask_confirm(&self, request: Confirmation) -> Result<bool, LoginError> {
        let answer = self.inner.entry.confirm(request, self.cancel.clone());
        match answer_or_cancel(answer, &self.cancel, ENTRY_TIMEOUT).await {
            Some(result) => result,
            None => {
                self.inner.entry.close();
                Ok(false)
            }
        }
    }
}

/// The steps of one operation, in the TypeScript's order. The caller turns an error into the sentence the person reads.
async fn perform(
    fence: &Arc<Fence>,
    owner: &str,
    action: VaultHostAction,
) -> Result<VaultReply, LoginError> {
    // Deletion and finishing still run when the isolation is gone, so they take no lease.
    if !matches!(
        action,
        VaultHostAction::Delete(_) | VaultHostAction::Finish(_)
    ) {
        let lease = fence
            .inner
            .isolation
            .acquire()
            .await
            .map_err(|_| LoginError::Failed)?;
        let _ = fence.lease.set(Arc::from(lease));
    }
    fence.check()?;
    match action {
        VaultHostAction::Unlock => unlock(fence).await,
        VaultHostAction::Delete(version) => delete(fence, version).await,
        VaultHostAction::Finish(finish) => finish_private(fence, finish).await,
        VaultHostAction::Entry(entry) => {
            require_unlocked(fence)?;
            save(fence, entry).await
        }
        VaultHostAction::Fill(fill) => {
            require_unlocked(fence)?;
            fill_login(fence, owner, fill).await
        }
        VaultHostAction::Status
        | VaultHostAction::List(_)
        | VaultHostAction::Lock
        | VaultHostAction::Disconnect
        | VaultHostAction::Cancel => Err(LoginError::Changed),
    }
}

fn require_unlocked(fence: &Fence) -> Result<(), LoginError> {
    if fence.inner.state().locked {
        Err(LoginError::Locked)
    } else {
        Ok(())
    }
}

async fn unlock(fence: &Arc<Fence>) -> Result<VaultReply, LoginError> {
    if fence.ask_confirm(Confirmation::Unlock).await? {
        fence.check()?;
        fence.inner.state().locked = false;
        cleanup(fence)?;
    }
    Ok(VaultReply::Status(status(&fence.inner).await?))
}

async fn delete(fence: &Arc<Fence>, version: EntryVersion) -> Result<VaultReply, LoginError> {
    let stored = fence
        .inner
        .records()
        .get(&version.id, Some(version.revision))?;
    if !fence
        .ask_confirm(Confirmation::Delete {
            entry: stored.entry.clone(),
        })
        .await?
    {
        return Ok(VaultReply::Delete(VaultDeleteResult {
            status: DeleteStatus::Cancelled,
        }));
    }
    fence.check()?;
    fence
        .inner
        .records()
        .deleting(&version.id, version.revision)?;
    cleanup(fence)?;
    let pending = fence.inner.records().pending()?.len();
    let status = if pending > 0 {
        DeleteStatus::Pending
    } else {
        DeleteStatus::Deleted
    };
    Ok(VaultReply::Delete(VaultDeleteResult { status }))
}

async fn finish_private(fence: &Arc<Fence>, finish: VaultFinish) -> Result<VaultReply, LoginError> {
    if !fence
        .ask_confirm(Confirmation::Finish { mode: finish.mode })
        .await?
    {
        return Ok(VaultReply::Finish(VaultFinishResult {
            status: FinishStatus::Cancelled,
        }));
    }
    fence.check()?;
    fence
        .inner
        .browser
        .finish(
            finish.bot_id.clone(),
            finish.generation.clone(),
            finish.private_epoch,
            finish.mode,
            fence.check_fn(),
        )
        .await?;
    fence.check()?;
    let done = PrivateSession {
        bot_id: finish.bot_id,
        generation: finish.generation,
        epoch: finish.private_epoch,
    };
    {
        let mut state = fence.inner.state();
        if state.private.as_ref() == Some(&done) {
            state.private = None;
        }
    }
    let status = match finish.mode {
        FinishMode::Clear => FinishStatus::Cleared,
        FinishMode::Close => FinishStatus::Closed,
    };
    Ok(VaultReply::Finish(VaultFinishResult { status }))
}

async fn save(fence: &Arc<Fence>, action: VaultEntry) -> Result<VaultReply, LoginError> {
    if action.id.is_some() != action.revision.is_some() {
        return Err(LoginError::Changed);
    }
    let previous = match &action.id {
        Some(id) if !id.is_empty() => Some(fence.inner.records().get(id, action.revision)?),
        _ => None,
    };
    if previous
        .as_ref()
        .is_some_and(|stored| stored.entry.bot_id != action.bot_id)
    {
        return Err(LoginError::Changed);
    }
    let prefill = previous.as_ref().map(|stored| stored.entry.clone());
    let Some(input) = fence.ask_entry(action.bot_id.clone(), prefill).await? else {
        fence.check()?;
        return Ok(VaultReply::Entry(VaultEntryResult {
            status: EntryStatus::Cancelled,
            entry: None,
        }));
    };
    let mut journal = None;
    let outcome = write_login(
        fence,
        input,
        &action.bot_id,
        previous.as_ref(),
        &mut journal,
    )
    .await;
    if let Some(reference) = journal {
        discard_orphan(fence, &reference);
    }
    outcome.map(|entry| {
        VaultReply::Entry(VaultEntryResult {
            status: EntryStatus::Saved,
            entry: Some(entry),
        })
    })
}

/// Journals a reference, writes the secret under it, then commits the record. `journal` names the reference, so the
/// caller can remove an orphaned secret after a failure. `input` is consumed, so its secret wipes when this returns.
async fn write_login(
    fence: &Arc<Fence>,
    input: EntryInput,
    bot_id: &str,
    previous: Option<&StoredLogin>,
    journal: &mut Option<String>,
) -> Result<LoginEntry, LoginError> {
    let EntryInput {
        name,
        origin,
        secret,
    } = input;
    fence.check()?;
    validate_vault_secret(&secret)?;
    let now = now_stamp();
    let entry = LoginEntry {
        id: previous.map_or_else(
            || Uuid::new_v4().to_string(),
            |stored| stored.entry.id.clone(),
        ),
        name,
        origin,
        bot_id: bot_id.to_owned(),
        revision: previous.map_or(1, |stored| stored.entry.revision.saturating_add(1)),
        created_at: previous.map_or_else(|| now.clone(), |stored| stored.entry.created_at.clone()),
        updated_at: now,
    };
    fence.check()?;
    let reference = fence.inner.records().prepare()?;
    *journal = Some(reference.clone());
    if !fence.inner.secrets.write_kept(&reference, &secret)? {
        return Err(LoginError::SecretNotKept);
    }
    fence.check()?;
    fence.inner.records().saved(
        &entry,
        &reference,
        previous.map(|stored| stored.entry.revision),
    )?;
    cleanup(fence)?;
    Ok(entry)
}

/// The secret of a failed save is removed if it is still journalled. Failure is kept: the journal retries on restart.
fn discard_orphan(fence: &Fence, reference: &str) {
    let pending = fence
        .inner
        .records()
        .pending()
        .map(|list| list.iter().any(|item| item == reference))
        .unwrap_or(false);
    if pending && fence.inner.secrets.delete(reference).is_ok() {
        let _ = fence.inner.records().deleted(reference);
    }
}

async fn fill_login(
    fence: &Arc<Fence>,
    owner: &str,
    fill: VaultFill,
) -> Result<VaultReply, LoginError> {
    let stored = fence.inner.records().get(&fill.id, Some(fill.revision))?;
    if stored.entry.bot_id != fill.bot_id {
        return Err(LoginError::Changed);
    }
    let target = VaultFillTarget {
        bot_id: fill.bot_id.clone(),
        generation: fill.generation.clone(),
        page_id: fill.page_id.clone(),
        control_epoch: fill.control_epoch,
    };
    let destination = fence
        .inner
        .browser
        .destination(target.clone(), owner.to_owned())
        .await?;
    fence.check()?;
    if destination != stored.entry.origin {
        return Err(LoginError::Changed);
    }
    if !fence
        .ask_confirm(Confirmation::Fill {
            entry: stored.entry.clone(),
            destination: destination.clone(),
        })
        .await?
    {
        return Ok(VaultReply::Fill(VaultFillResult {
            status: FillStatus::Cancelled,
            private_epoch: None,
        }));
    }
    fence.check()?;
    // The page may have navigated while the person read the confirmation.
    if fence
        .inner
        .browser
        .destination(target.clone(), owner.to_owned())
        .await?
        != stored.entry.origin
    {
        return Err(LoginError::Changed);
    }
    fence.check()?;
    let private_epoch = fence
        .inner
        .browser
        .begin(
            target,
            owner.to_owned(),
            stored.entry.origin.clone(),
            fence.check_fn(),
        )
        .await?;
    // Tracked before the next check, so a lock that lands from here on leaves it tracked for the person to finish.
    fence.inner.state().private = Some(PrivateSession {
        bot_id: fill.bot_id.clone(),
        generation: fill.generation.clone(),
        epoch: private_epoch,
    });
    fence.check()?;
    let Some(secret) = fence.inner.secrets.read_settled(&stored.reference)? else {
        return Err(LoginError::SecretMissing);
    };
    fence.check()?;
    fence
        .inner
        .browser
        .fill(fill.bot_id, private_epoch, secret, fence.check_fn())
        .await?;
    fence.check()?;
    Ok(VaultReply::Fill(VaultFillResult {
        status: FillStatus::Filled,
        private_epoch: Some(private_epoch),
    }))
}

/// Removes every journalled secret whose deletion did not finish. A failed removal is kept for the next try.
fn cleanup(fence: &Fence) -> Result<(), LoginError> {
    let pending = fence.inner.records().pending()?;
    for reference in pending {
        fence.check()?;
        let removed = match fence.inner.secrets.delete(&reference) {
            Ok(()) => {
                fence.check()?;
                fence.inner.records().deleted(&reference).is_ok()
            }
            Err(_) => false,
        };
        if !removed {
            fence.check()?;
        }
    }
    Ok(())
}

async fn status(inner: &Inner) -> Result<VaultStatus, VaultError> {
    let reason = inner.isolation.unavailable_reason().await;
    let pending_deletes = inner.records().pending()?.len() as u64;
    let state = if reason.is_some() {
        VaultState::Unavailable
    } else if inner.state().locked {
        VaultState::Locked
    } else {
        VaultState::Unlocked
    };
    Ok(VaultStatus {
        state,
        reason,
        pending_deletes,
    })
}

async fn status_reply(inner: &Inner) -> Result<VaultReply, GateError> {
    status(inner)
        .await
        .map(VaultReply::Status)
        .map_err(|error| GateError::Vault(error.to_string()))
}

/// The bounds the protocol's schemas set (`protocol/src/vault.ts`): the lengths of ids, the positive revisions. A
/// command outside them is refused before anything runs. Lengths are counted in code points, as zod counts them.
// ponytail: entry, fill and delete ids are not checked as UUIDs; a malformed one simply finds no login.
fn well_formed(owner: &str, action: &VaultHostAction) -> bool {
    let text = |value: &str, max: usize| (1..=max).contains(&value.chars().count());
    text(owner, 256)
        && match action {
            VaultHostAction::Status
            | VaultHostAction::Unlock
            | VaultHostAction::Lock
            | VaultHostAction::Disconnect
            | VaultHostAction::Cancel => true,
            VaultHostAction::List(list) => list.bot_id.as_deref().is_none_or(|bot| text(bot, 256)),
            VaultHostAction::Entry(entry) => {
                text(&entry.bot_id, 256) && entry.revision.is_none_or(|revision| revision >= 1)
            }
            VaultHostAction::Delete(version) => version.revision >= 1,
            VaultHostAction::Fill(fill) => {
                text(&fill.bot_id, 256)
                    && text(&fill.generation, 100)
                    && text(&fill.page_id, 100)
                    && fill.revision >= 1
            }
            VaultHostAction::Finish(finish) => {
                text(&finish.bot_id, 256) && text(&finish.generation, 100)
            }
        }
}

/// `new Date().toISOString()`: UTC with milliseconds, as the TypeScript writes the login's timestamps.
fn now_stamp() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn generic() -> GateError {
    GateError::Vault(GENERIC.to_owned())
}

fn changed() -> GateError {
    GateError::Vault(CHANGED.to_owned())
}

fn empty() -> VaultReply {
    VaultReply::Empty(Empty {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::vault::{VaultEntry, VaultList};

    #[test]
    fn the_protocol_bounds_refuse_what_the_schemas_refuse() {
        let entry = |bot: &str, revision: Option<u64>| {
            VaultHostAction::Entry(VaultEntry {
                bot_id: bot.to_owned(),
                id: None,
                revision,
            })
        };
        assert!(well_formed("local", &entry("bot", None)));
        assert!(well_formed("local", &entry(&"b".repeat(256), Some(1))));
        assert!(!well_formed("local", &entry(&"b".repeat(257), None)));
        assert!(!well_formed("local", &entry("bot", Some(0))));
        assert!(!well_formed("", &VaultHostAction::Status));
        assert!(well_formed(
            "local",
            &VaultHostAction::List(VaultList { bot_id: None })
        ));
    }

    #[test]
    fn a_timestamp_has_the_shape_javascript_writes() {
        let stamp = now_stamp();
        let bytes = stamp.as_bytes();
        assert_eq!(bytes.len(), 24, "{stamp}");
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[19], b'.');
        assert_eq!(bytes[23], b'Z');
        assert!(bytes[20..23].iter().all(u8::is_ascii_digit));
    }

    #[tokio::test]
    async fn a_cancel_fires_once_for_every_waiter() {
        let (sender, cancel) = Cancel::pair();
        assert!(!cancel.is_cancelled());
        let waiter = tokio::spawn({
            let cancel = cancel.clone();
            async move { cancel.cancelled().await }
        });
        sender.send_replace(true);
        waiter.await.expect("the waiter ends once the signal fires");
        assert!(cancel.is_cancelled());
    }
}
