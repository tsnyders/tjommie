//! The login vault's command layer: the command-level cases of `packages/desktop/src/login-vault.test.ts`, the fences
//! and the fill rules, and the lock that closes the entry window and leaves a private browser private. Everything outside the vault is a fake: the browser,
//! the entry window, the isolation and the secret store. Records live in a folder under %TEMP%. Credential Manager is
//! never touched, and no account or network is involved.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use desktop_host::owner_safety::test_root;
use desktop_host::vault::entry::{EntryForm, EntryInput, WindowAnswer, accept_entry};
use desktop_host::vault::gate::{GateError, Vault, VaultReply};
use desktop_host::vault::login::{
    Cancel, Check, Confirmation, EntryWindow, LoginBrowser, LoginError, NativeLoginVault,
    SecretStore, VaultParts,
};
use desktop_host::vault::storage::{VaultError, VaultRecords, VaultSecret};
use desktop_host::vault::worker::{IsolationError, VaultIsolation, VaultIsolationLease};
use futures::future::BoxFuture;
use protocol::vault::{
    DeleteStatus, EntryStatus, EntryVersion, FillStatus, FinishMode, FinishStatus, LoginEntry,
    LoginOrigin, VaultDeleteResult, VaultEntry, VaultEntryResult, VaultFill, VaultFillResult,
    VaultFillTarget, VaultFinish, VaultFinishResult, VaultHostAction, VaultHostCommand, VaultList,
    VaultState, VaultStatus,
};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::Notify;

const TYPED: &str = "Synthetic-Login-Secret-ABSENT-FROM-OUTPUT-7d14";
const USER: &str = "synthetic@example.invalid";
const GENERIC: &str = "Logins could not finish this operation safely. Refresh Logins or check the host. Any private browser stays private until you clear its fields or close it.";
const CHANGED: &str = "The Logins operation was cancelled or its security boundary changed.";
const IN_FLIGHT: &str = "Finish the current Logins operation on the host first.";
const UNAVAILABLE: &str = "Verified worker unavailable.";

fn generic() -> GateError {
    GateError::Vault(GENERIC.to_owned())
}

fn origin(text: &str) -> LoginOrigin {
    LoginOrigin::new(text).expect("a valid origin")
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

// --- The fakes -------------------------------------------------------------------------------------------------------

/// The secret store. A failed delete happens once, when asked.
#[derive(Default)]
struct FakeSecrets {
    values: Mutex<HashMap<String, (String, String)>>,
    reads: AtomicUsize,
    writes: AtomicUsize,
    deletes: AtomicUsize,
    fail_next_delete: AtomicBool,
    /// The next writes that report success and keep nothing, as Credential Manager sometimes does.
    drop_writes: AtomicUsize,
    /// The next reads that find nothing although the entry is there.
    miss_reads: AtomicUsize,
}

impl FakeSecrets {
    fn count(&self) -> usize {
        self.values.lock().unwrap().len()
    }
}

/// Takes one from a countdown, or says it has run out.
fn take(countdown: &AtomicUsize) -> bool {
    countdown
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok()
}

impl SecretStore for FakeSecrets {
    fn write(&self, reference: &str, secret: &VaultSecret) -> Result<(), VaultError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if take(&self.drop_writes) {
            return Ok(());
        }
        self.values.lock().unwrap().insert(
            reference.to_owned(),
            (secret.username().to_owned(), secret.password().to_owned()),
        );
        Ok(())
    }

    fn read(&self, reference: &str) -> Result<Option<VaultSecret>, VaultError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if take(&self.miss_reads) {
            return Ok(None);
        }
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(reference)
            .map(|(user, pass)| VaultSecret::new(user.clone(), pass.clone())))
    }

    fn delete(&self, reference: &str) -> Result<(), VaultError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        if self.fail_next_delete.swap(false, Ordering::SeqCst) {
            return Err(VaultError::Storage);
        }
        self.values.lock().unwrap().remove(reference);
        Ok(())
    }
}

/// The isolation and its leases. Setting `available` false is a lost isolation: status says so and every lease fails.
struct FakeIsolation {
    available: Arc<AtomicBool>,
    held: Arc<AtomicUsize>,
    acquires: AtomicUsize,
}

impl VaultIsolation for FakeIsolation {
    fn unavailable_reason(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            if self.available.load(Ordering::SeqCst) {
                None
            } else {
                Some(UNAVAILABLE.to_owned())
            }
        })
    }

    fn acquire(&self) -> BoxFuture<'_, Result<Box<dyn VaultIsolationLease>, IsolationError>> {
        Box::pin(async move {
            if !self.available.load(Ordering::SeqCst) {
                return Err(IsolationError);
            }
            self.acquires.fetch_add(1, Ordering::SeqCst);
            self.held.fetch_add(1, Ordering::SeqCst);
            let lease: Box<dyn VaultIsolationLease> = Box::new(FakeLease {
                available: self.available.clone(),
                held: self.held.clone(),
                released: AtomicBool::new(false),
            });
            Ok(lease)
        })
    }
}

struct FakeLease {
    available: Arc<AtomicBool>,
    held: Arc<AtomicUsize>,
    released: AtomicBool,
}

impl VaultIsolationLease for FakeLease {
    fn assert(&self) -> Result<(), IsolationError> {
        if self.available.load(Ordering::SeqCst) && !self.released.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(IsolationError)
        }
    }

    fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            self.held.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// One answer the entry window gives.
enum Answer {
    Input {
        name: String,
        origin: String,
        user: String,
        pass: String,
    },
    /// The person cancels in the window.
    Cancel,
    /// The window stays open until its cancel signal fires.
    Silent,
}

fn input(name: &str, origin: &str) -> Answer {
    Answer::Input {
        name: name.to_owned(),
        origin: origin.to_owned(),
        user: USER.to_owned(),
        pass: TYPED.to_owned(),
    }
}

/// The entry and confirmation windows.
#[derive(Default)]
struct FakeWindow {
    answers: Mutex<VecDeque<Answer>>,
    confirms: Mutex<VecDeque<bool>>,
    /// When set, an entry answer waits for this before it arrives.
    entry_gate: Mutex<Option<Arc<Notify>>>,
    asked_entries: Mutex<Vec<(String, Option<String>)>>,
    asked_signal: Notify,
    confirmations: Mutex<Vec<Confirmation>>,
    closes: AtomicUsize,
}

impl FakeWindow {
    fn queue(&self, answer: Answer) {
        self.answers.lock().unwrap().push_back(answer);
    }

    fn queue_confirm(&self, answer: bool) {
        self.confirms.lock().unwrap().push_back(answer);
    }

    fn entry_asks(&self) -> usize {
        self.asked_entries.lock().unwrap().len()
    }

    fn confirm_asks(&self) -> usize {
        self.confirmations.lock().unwrap().len()
    }

    fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }
}

impl EntryWindow for FakeWindow {
    fn entry(
        &self,
        bot_id: String,
        previous: Option<LoginEntry>,
        cancel: Cancel,
    ) -> BoxFuture<'_, Result<Option<EntryInput>, LoginError>> {
        Box::pin(async move {
            self.asked_entries
                .lock()
                .unwrap()
                .push((bot_id, previous.map(|entry| entry.id)));
            self.asked_signal.notify_one();
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Answer::Cancel);
            let gate = self.entry_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            match answer {
                Answer::Input {
                    name,
                    origin,
                    user,
                    pass,
                } => {
                    let form = EntryForm::new(name, origin, user, pass);
                    accept_entry(WindowAnswer::Form(form)).map_err(|_| LoginError::Failed)
                }
                Answer::Cancel => Ok(None),
                Answer::Silent => {
                    cancel.cancelled().await;
                    Ok(None)
                }
            }
        })
    }

    fn confirm(
        &self,
        request: Confirmation,
        _cancel: Cancel,
    ) -> BoxFuture<'_, Result<bool, LoginError>> {
        Box::pin(async move {
            self.confirmations.lock().unwrap().push(request);
            Ok(self.confirms.lock().unwrap().pop_front().unwrap_or(true))
        })
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

/// The browser. `private` is true while a private interaction is open. Gates hold `begin` or `fill` until the test
/// releases them, so a lock can land in the middle.
struct FakeBrowser {
    default_origin: LoginOrigin,
    destinations: Mutex<VecDeque<LoginOrigin>>,
    calls: Mutex<Vec<String>>,
    private: AtomicBool,
    begin_gate: Mutex<Option<Arc<Notify>>>,
    begin_started: Notify,
    fill_gate: Mutex<Option<Arc<Notify>>>,
    fill_started: Notify,
    filled: Mutex<Vec<(String, u64, String, String)>>,
    finishes: AtomicUsize,
    /// A lease lapses as soon as the private interaction begins.
    lapse_after_begin: AtomicBool,
    available: Arc<AtomicBool>,
}

impl FakeBrowser {
    fn new(available: Arc<AtomicBool>) -> Self {
        Self {
            default_origin: origin("https://login.invalid"),
            destinations: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            private: AtomicBool::new(false),
            begin_gate: Mutex::new(None),
            begin_started: Notify::new(),
            fill_gate: Mutex::new(None),
            fill_started: Notify::new(),
            filled: Mutex::new(Vec::new()),
            finishes: AtomicUsize::new(0),
            lapse_after_begin: AtomicBool::new(false),
            available,
        }
    }

    fn log(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }

    fn is_private(&self) -> bool {
        self.private.load(Ordering::SeqCst)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn finishes(&self) -> usize {
        self.finishes.load(Ordering::SeqCst)
    }
}

impl LoginBrowser for FakeBrowser {
    fn destination(
        &self,
        target: VaultFillTarget,
        owner: String,
    ) -> BoxFuture<'_, Result<LoginOrigin, LoginError>> {
        Box::pin(async move {
            self.log(format!("destination:{owner}:{}", target.bot_id));
            let next = self.destinations.lock().unwrap().pop_front();
            Ok(next.unwrap_or_else(|| self.default_origin.clone()))
        })
    }

    fn begin(
        &self,
        target: VaultFillTarget,
        _owner: String,
        _origin: LoginOrigin,
        _check: Check,
    ) -> BoxFuture<'_, Result<u64, LoginError>> {
        Box::pin(async move {
            self.log(format!("begin:{}", target.bot_id));
            let gate = self.begin_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                self.begin_started.notify_one();
                gate.notified().await;
            }
            self.private.store(true, Ordering::SeqCst);
            if self.lapse_after_begin.load(Ordering::SeqCst) {
                self.available.store(false, Ordering::SeqCst);
            }
            Ok(4)
        })
    }

    fn fill(
        &self,
        bot_id: String,
        private_epoch: u64,
        secret: VaultSecret,
        check: Check,
    ) -> BoxFuture<'_, Result<(), LoginError>> {
        Box::pin(async move {
            self.log(format!("fill:{private_epoch}"));
            let gate = self.fill_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                self.fill_started.notify_one();
                gate.notified().await;
            }
            check()?;
            self.filled.lock().unwrap().push((
                bot_id,
                private_epoch,
                secret.username().to_owned(),
                secret.password().to_owned(),
            ));
            Ok(())
        })
    }

    fn finish(
        &self,
        _bot_id: String,
        _generation: String,
        private_epoch: u64,
        mode: FinishMode,
        check: Check,
    ) -> BoxFuture<'_, Result<(), LoginError>> {
        Box::pin(async move {
            self.log(format!("finish:{mode:?}"));
            if !self.is_private() || private_epoch != 4 {
                return Err(LoginError::Failed);
            }
            check()?;
            self.private.store(false, Ordering::SeqCst);
            self.finishes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

// --- The fixture -----------------------------------------------------------------------------------------------------

struct Fixture {
    root: TempDir,
    secrets: Arc<FakeSecrets>,
    browser: Arc<FakeBrowser>,
    window: Arc<FakeWindow>,
    isolation: Arc<FakeIsolation>,
}

impl Fixture {
    fn new() -> Self {
        let available = Arc::new(AtomicBool::new(true));
        let held = Arc::new(AtomicUsize::new(0));
        Self {
            root: test_root().expect("a folder under %TEMP%"),
            secrets: Arc::new(FakeSecrets::default()),
            browser: Arc::new(FakeBrowser::new(available.clone())),
            window: Arc::new(FakeWindow::default()),
            isolation: Arc::new(FakeIsolation {
                available,
                held,
                acquires: AtomicUsize::new(0),
            }),
        }
    }

    fn records_path(&self) -> PathBuf {
        self.root.path().join("data").join("logins.sqlite")
    }

    /// A vault over the same records and fakes. A second call is a restart: the vault starts locked.
    fn vault(&self) -> NativeLoginVault {
        NativeLoginVault::new(VaultParts {
            records: VaultRecords::open(&self.records_path()).expect("the records open"),
            secrets: self.secrets.clone(),
            isolation: self.isolation.clone(),
            entry: self.window.clone(),
            browser: self.browser.clone(),
        })
    }

    fn held(&self) -> usize {
        self.isolation.held.load(Ordering::SeqCst)
    }

    fn acquires(&self) -> usize {
        self.isolation.acquires.load(Ordering::SeqCst)
    }

    /// Whether the typed password is in the records file, its write-ahead log or its shared memory file.
    fn disk_contains(&self, needle: &str) -> bool {
        let path = self.records_path();
        [
            path.clone(),
            with_suffix(&path, "-wal"),
            with_suffix(&path, "-shm"),
        ]
        .iter()
        .any(|file| fs::read(file).is_ok_and(|bytes| contains(&bytes, needle)))
    }

    /// Unlocks and saves one login through the entry window. Returns it. The read count starts again from here: a save
    /// reads its own secret back, and the cases count the reads that come after.
    async fn save(&self, vault: &NativeLoginVault) -> LoginEntry {
        run(vault, "local", VaultHostAction::Unlock)
            .await
            .expect("unlock");
        self.window
            .queue(input("Synthetic account", "https://login.invalid"));
        let entry = match run(vault, "local", entry_action(None)).await {
            Ok(VaultReply::Entry(VaultEntryResult {
                status: EntryStatus::Saved,
                entry: Some(entry),
            })) => entry,
            other => panic!("the login did not save: {other:?}"),
        };
        self.secrets.reads.store(0, Ordering::SeqCst);
        entry
    }
}

fn command(owner: &str, action: VaultHostAction) -> VaultHostCommand {
    VaultHostCommand {
        controller_id: owner.to_owned(),
        action,
    }
}

async fn run(
    vault: &NativeLoginVault,
    owner: &str,
    action: VaultHostAction,
) -> Result<VaultReply, GateError> {
    vault.command(command(owner, action)).await
}

fn entry_action(previous: Option<&LoginEntry>) -> VaultHostAction {
    VaultHostAction::Entry(VaultEntry {
        bot_id: "bot".to_owned(),
        id: previous.map(|entry| entry.id.clone()),
        revision: previous.map(|entry| entry.revision),
    })
}

fn fill_action(entry: &LoginEntry) -> VaultHostAction {
    VaultHostAction::Fill(VaultFill {
        bot_id: entry.bot_id.clone(),
        generation: "g".to_owned(),
        page_id: "p".to_owned(),
        control_epoch: 1,
        id: entry.id.clone(),
        revision: entry.revision,
    })
}

fn finish_action(mode: FinishMode) -> VaultHostAction {
    VaultHostAction::Finish(VaultFinish {
        bot_id: "bot".to_owned(),
        generation: "g".to_owned(),
        private_epoch: 4,
        mode,
    })
}

fn filled(status: FillStatus, epoch: Option<u64>) -> Result<VaultReply, GateError> {
    Ok(VaultReply::Fill(VaultFillResult {
        status,
        private_epoch: epoch,
    }))
}

// --- Cases -----------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_unavailable_isolation_offers_no_entry_and_reads_no_secret() {
    let f = Fixture::new();
    let vault = f.vault();
    f.isolation.available.store(false, Ordering::SeqCst);

    let status = run(&vault, "local", VaultHostAction::Status).await;
    assert_eq!(
        status,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Unavailable,
            reason: Some(UNAVAILABLE.to_owned()),
            pending_deletes: 0,
        }))
    );
    assert_eq!(
        run(&vault, "local", VaultHostAction::Unlock).await,
        Err(generic())
    );
    assert_eq!(f.window.entry_asks() + f.window.confirm_asks(), 0);
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.held(), 0);
}

#[tokio::test]
async fn saves_metadata_and_opaque_references_restarts_locked_and_cleans_replaced_secrets() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    assert_eq!(f.secrets.count(), 1);
    assert_eq!(f.held(), 0, "the lease is released at the end");
    assert!(
        !f.disk_contains(TYPED),
        "the records never hold the password"
    );

    f.window
        .queue(input("Synthetic account", "https://login.invalid"));
    let edited = run(&vault, "local", entry_action(Some(&entry))).await;
    let Ok(VaultReply::Entry(VaultEntryResult {
        status: EntryStatus::Saved,
        entry: Some(edited),
    })) = edited
    else {
        panic!("the edit did not save");
    };
    assert_eq!(edited.revision, 2);
    assert_eq!(f.secrets.count(), 1, "the replaced secret is gone");
    assert_eq!(f.secrets.deletes.load(Ordering::SeqCst), 1);

    f.secrets.reads.store(0, Ordering::SeqCst);
    let restarted = f.vault();
    assert_eq!(
        run(&restarted, "local", VaultHostAction::Status).await,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Locked,
            reason: None,
            pending_deletes: 0,
        }))
    );
    assert_eq!(
        run(&restarted, "local", fill_action(&edited)).await,
        Err(generic())
    );
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    assert!(
        !format!(
            "{:?}",
            run(&restarted, "local", VaultHostAction::Status).await
        )
        .contains(TYPED)
    );
}

#[tokio::test]
async fn confirms_the_exact_origin_before_reading_and_fills_privately() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;

    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        filled(FillStatus::Filled, Some(4))
    );
    assert_eq!(
        *f.browser.filled.lock().unwrap(),
        [("bot".to_owned(), 4, USER.to_owned(), TYPED.to_owned())]
    );
    assert!(f.browser.is_private());
    assert_eq!(f.held(), 0);

    let acquires = f.acquires();
    assert_eq!(
        run(&vault, "local", finish_action(FinishMode::Clear)).await,
        Ok(VaultReply::Finish(VaultFinishResult {
            status: FinishStatus::Cleared,
        }))
    );
    assert!(!f.browser.is_private());
    assert_eq!(f.acquires(), acquires, "finishing takes no lease");
    assert!(!format!("{:?}", f.browser.calls()).contains(TYPED));
}

#[tokio::test]
async fn a_dropped_write_is_written_again_before_the_login_is_saved() {
    let f = Fixture::new();
    let vault = f.vault();
    f.secrets.drop_writes.store(1, Ordering::SeqCst);
    let entry = f.save(&vault).await;

    assert_eq!(f.secrets.count(), 1, "the saved password is there");
    assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 2);
    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        filled(FillStatus::Filled, Some(4))
    );
}

#[tokio::test]
async fn a_password_that_is_not_kept_fails_the_save_and_records_nothing() {
    let f = Fixture::new();
    let vault = f.vault();
    run(&vault, "local", VaultHostAction::Unlock)
        .await
        .expect("unlock");
    f.secrets.drop_writes.store(2, Ordering::SeqCst);
    f.window
        .queue(input("Synthetic account", "https://login.invalid"));

    assert_eq!(
        run(&vault, "local", entry_action(None)).await,
        Err(GateError::Vault(
            "Windows did not keep the password, so the login was not saved. Try again.".to_owned()
        ))
    );
    assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 2, "one retry only");
    assert!(vault_logins(&f).is_empty(), "nothing is recorded");
}

#[tokio::test]
async fn a_momentary_miss_does_not_fail_the_fill() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    f.secrets.miss_reads.store(2, Ordering::SeqCst);

    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        filled(FillStatus::Filled, Some(4))
    );
}

#[tokio::test]
async fn a_fill_says_when_the_saved_password_is_missing() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    f.secrets.values.lock().unwrap().clear();

    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        Err(GateError::Vault(
            "The saved password for this login is missing. Save the login again. The browser stays private until you clear its fields or close it.".to_owned()
        ))
    );
    assert!(f.browser.filled.lock().unwrap().is_empty());
    assert!(f.browser.is_private(), "what the message says is true");
}

#[tokio::test]
async fn a_fill_for_the_wrong_origin_is_refused_and_reads_no_secret() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    f.browser
        .destinations
        .lock()
        .unwrap()
        .push_back(origin("https://other.invalid"));

    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        Err(generic())
    );
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    assert!(
        !f.browser
            .calls()
            .iter()
            .any(|call| call.starts_with("begin"))
    );
    assert!(!f.browser.is_private());
}

#[tokio::test]
async fn rechecks_the_destination_after_the_confirmation_and_refuses_a_wrong_profile() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    {
        let mut destinations = f.browser.destinations.lock().unwrap();
        destinations.push_back(origin("https://login.invalid"));
        destinations.push_back(origin("https://navigation.invalid"));
    }
    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        Err(generic())
    );

    let mut other_bot = fill_action(&entry);
    if let VaultHostAction::Fill(fill) = &mut other_bot {
        fill.bot_id = "another-bot".to_owned();
    }
    assert_eq!(run(&vault, "local", other_bot).await, Err(generic()));
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    assert!(
        !f.browser
            .calls()
            .iter()
            .any(|call| call.starts_with("begin"))
    );
}

#[tokio::test]
async fn an_entry_arriving_after_a_lock_is_never_saved() {
    let f = Fixture::new();
    let vault = Arc::new(f.vault());
    run(&vault, "local", VaultHostAction::Unlock)
        .await
        .expect("unlock");
    let gate = Arc::new(Notify::new());
    *f.window.entry_gate.lock().unwrap() = Some(gate.clone());
    f.window.queue(input("Late entry", "https://login.invalid"));

    let pending = tokio::spawn({
        let vault = vault.clone();
        async move { run(&vault, "local", entry_action(None)).await }
    });
    f.window.asked_signal.notified().await;
    vault.lock();
    gate.notify_one();

    assert_eq!(pending.await.expect("joined"), Err(generic()));
    assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 0);
    assert!(f.window.closes() >= 1, "the window closes with the lock");
    assert_eq!(f.held(), 0);
}

#[tokio::test]
async fn a_cancel_in_the_entry_window_saves_nothing_and_reports_cancelled() {
    let f = Fixture::new();
    let vault = f.vault();
    run(&vault, "local", VaultHostAction::Unlock)
        .await
        .expect("unlock");
    f.window.queue(Answer::Cancel);

    assert_eq!(
        run(&vault, "local", entry_action(None)).await,
        Ok(VaultReply::Entry(VaultEntryResult {
            status: EntryStatus::Cancelled,
            entry: None,
        }))
    );
    assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 0);
    assert!(vault_logins(&f).is_empty());
}

#[tokio::test]
async fn a_lock_while_filling_closes_the_entry_window_and_cancels_the_fill_but_leaves_the_browser_private()
 {
    let f = Fixture::new();
    let vault = Arc::new(f.vault());
    let entry = f.save(&vault).await;
    let gate = Arc::new(Notify::new());
    *f.browser.fill_gate.lock().unwrap() = Some(gate.clone());

    let filling = tokio::spawn({
        let vault = vault.clone();
        let action = fill_action(&entry);
        async move { run(&vault, "local", action).await }
    });
    f.browser.fill_started.notified().await;
    assert!(f.browser.is_private());
    assert_eq!(f.held(), 1, "the lease is held while the fill runs");
    let closes = f.window.closes();

    assert!(matches!(
        run(&vault, "local", VaultHostAction::Lock).await,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Locked,
            ..
        }))
    ));
    // The bot stays shut out: a lock never gives a private page back. The person clears it or closes it.
    assert!(f.browser.is_private(), "the browser stays private");
    assert_eq!(
        f.browser.finishes(),
        0,
        "the vault closes nothing on a lock"
    );
    assert!(
        f.window.closes() > closes,
        "the lock closes the entry window"
    );

    gate.notify_one();
    assert_eq!(filling.await.expect("joined"), Err(generic()));
    assert!(
        f.browser.filled.lock().unwrap().is_empty(),
        "nothing was filled"
    );
    assert_eq!(f.held(), 0);

    // Clearing a private browser stays possible after a lock.
    assert_eq!(
        run(&vault, "local", finish_action(FinishMode::Clear)).await,
        Ok(VaultReply::Finish(VaultFinishResult {
            status: FinishStatus::Cleared,
        }))
    );
    assert!(!f.browser.is_private());
}

#[tokio::test]
async fn a_lock_from_navigation_closes_the_entry_window_and_leaves_a_private_browser_private() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        filled(FillStatus::Filled, Some(4))
    );
    let closes = f.window.closes();

    vault.lock();
    assert!(f.window.closes() > closes, "the entry window closes");
    assert!(f.browser.is_private(), "the private browser stays private");
    assert_eq!(f.browser.finishes(), 0);
    assert!(matches!(
        run(&vault, "local", VaultHostAction::Status).await,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Locked,
            ..
        }))
    ));
}

#[tokio::test]
async fn a_lock_during_begin_fences_the_fill_and_the_browser_that_began_stays_private() {
    let f = Fixture::new();
    let vault = Arc::new(f.vault());
    let entry = f.save(&vault).await;
    let gate = Arc::new(Notify::new());
    *f.browser.begin_gate.lock().unwrap() = Some(gate.clone());

    let filling = tokio::spawn({
        let vault = vault.clone();
        let action = fill_action(&entry);
        async move { run(&vault, "local", action).await }
    });
    f.browser.begin_started.notified().await;
    assert!(matches!(
        run(&vault, "local", VaultHostAction::Lock).await,
        Ok(VaultReply::Status(_))
    ));

    gate.notify_one();
    assert_eq!(filling.await.expect("joined"), Err(generic()));
    assert!(
        f.browser.is_private(),
        "the interaction that began is left private for the person to clear or close"
    );
    assert_eq!(f.browser.finishes(), 0);
    assert!(f.browser.filled.lock().unwrap().is_empty());
    assert_eq!(
        f.secrets.reads.load(Ordering::SeqCst),
        0,
        "the lock fenced the read of the secret"
    );
}

#[tokio::test]
async fn a_lease_that_lapses_mid_fill_aborts_before_the_secret_is_read() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    f.browser.lapse_after_begin.store(true, Ordering::SeqCst);

    assert_eq!(
        run(&vault, "local", fill_action(&entry)).await,
        Err(generic())
    );
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);
    assert!(f.browser.filled.lock().unwrap().is_empty());
    assert_eq!(f.held(), 0, "the lapsed lease is released");
    assert!(
        f.browser.is_private(),
        "a failed fill stays private until it is cleared"
    );
}

#[tokio::test]
async fn a_disconnected_controller_is_refused_until_a_cancel_from_a_new_controller() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;

    assert_eq!(
        run(&vault, "rpc:retired", VaultHostAction::Disconnect).await,
        Ok(VaultReply::Empty(desktop_host::vault::gate::Empty {}))
    );
    assert_eq!(
        run(&vault, "rpc:retired", fill_action(&entry)).await,
        Err(GateError::Vault(CHANGED.to_owned()))
    );
    assert!(
        !f.browser
            .calls()
            .iter()
            .any(|call| call.starts_with("destination"))
    );
    assert_eq!(f.secrets.reads.load(Ordering::SeqCst), 0);

    run(&vault, "rpc:current", VaultHostAction::Cancel)
        .await
        .expect("cancel");
    assert_eq!(
        run(&vault, "rpc:current", fill_action(&entry)).await,
        filled(FillStatus::Filled, Some(4))
    );
}

#[tokio::test]
async fn a_failed_secret_deletion_is_journalled_across_restart_without_removing_the_private_browser()
 {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    run(&vault, "local", fill_action(&entry))
        .await
        .expect("fill");

    f.secrets.fail_next_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        run(
            &vault,
            "local",
            VaultHostAction::Delete(EntryVersion {
                id: entry.id.clone(),
                revision: entry.revision,
            })
        )
        .await,
        Ok(VaultReply::Delete(VaultDeleteResult {
            status: DeleteStatus::Pending,
        }))
    );
    assert_eq!(
        run(
            &vault,
            "local",
            VaultHostAction::List(VaultList { bot_id: None })
        )
        .await,
        Ok(VaultReply::List(Vec::new()))
    );
    assert_eq!(f.secrets.count(), 1);
    assert!(
        f.browser.is_private(),
        "a delete does not touch the private browser"
    );
    assert_eq!(f.browser.finishes(), 0);

    let restarted = f.vault();
    assert_eq!(
        run(&restarted, "local", VaultHostAction::Status).await,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Locked,
            reason: None,
            pending_deletes: 1,
        }))
    );
    run(&restarted, "local", VaultHostAction::Unlock)
        .await
        .expect("unlock");
    assert_eq!(
        run(&restarted, "local", VaultHostAction::Status).await,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Unlocked,
            reason: None,
            pending_deletes: 0,
        }))
    );
    assert_eq!(f.secrets.count(), 0);
    assert_eq!(f.browser.finishes(), 0);
}

#[tokio::test]
async fn a_second_operation_is_refused_while_one_is_in_flight() {
    let f = Fixture::new();
    let vault = Arc::new(f.vault());
    run(&vault, "local", VaultHostAction::Unlock)
        .await
        .expect("unlock");
    f.window.queue(Answer::Silent);

    let first = tokio::spawn({
        let vault = vault.clone();
        async move { run(&vault, "local", entry_action(None)).await }
    });
    f.window.asked_signal.notified().await;

    assert_eq!(
        run(&vault, "rpc:other", entry_action(None)).await,
        Err(GateError::Vault(IN_FLIGHT.to_owned()))
    );
    run(&vault, "local", VaultHostAction::Cancel)
        .await
        .expect("cancel");
    assert_eq!(first.await.expect("joined"), Err(generic()));
}

#[tokio::test]
async fn list_and_delete_work_while_locked_but_a_new_entry_does_not() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    assert!(matches!(
        run(&vault, "local", VaultHostAction::Lock).await,
        Ok(VaultReply::Status(VaultStatus {
            state: VaultState::Locked,
            ..
        }))
    ));
    let asks = f.window.entry_asks();

    assert_eq!(
        run(
            &vault,
            "local",
            VaultHostAction::List(VaultList { bot_id: None })
        )
        .await,
        Ok(VaultReply::List(vec![entry.clone()]))
    );
    assert_eq!(
        run(&vault, "local", entry_action(None)).await,
        Err(generic())
    );
    assert_eq!(
        f.window.entry_asks(),
        asks,
        "no entry window opens while locked"
    );
    assert_eq!(
        run(
            &vault,
            "local",
            VaultHostAction::Delete(EntryVersion {
                id: entry.id.clone(),
                revision: entry.revision,
            })
        )
        .await,
        Ok(VaultReply::Delete(VaultDeleteResult {
            status: DeleteStatus::Deleted,
        }))
    );
}

#[tokio::test]
async fn a_declined_delete_is_cancelled_and_keeps_the_login() {
    let f = Fixture::new();
    let vault = f.vault();
    let entry = f.save(&vault).await;
    f.window.queue_confirm(false);

    assert_eq!(
        run(
            &vault,
            "local",
            VaultHostAction::Delete(EntryVersion {
                id: entry.id.clone(),
                revision: entry.revision,
            })
        )
        .await,
        Ok(VaultReply::Delete(VaultDeleteResult {
            status: DeleteStatus::Cancelled,
        }))
    );
    assert_eq!(vault_logins(&f).len(), 1);
    assert_eq!(f.secrets.count(), 1);
}

#[tokio::test]
async fn a_closed_vault_refuses_every_command() {
    let f = Fixture::new();
    let vault = f.vault();
    vault.close();
    for action in [
        VaultHostAction::Status,
        VaultHostAction::Lock,
        VaultHostAction::List(VaultList { bot_id: None }),
    ] {
        assert_eq!(
            run(&vault, "local", action).await,
            Err(GateError::Vault(CHANGED.to_owned()))
        );
    }
}

#[tokio::test]
async fn a_malformed_command_is_refused_before_anything_runs() {
    let f = Fixture::new();
    let vault = f.vault();
    let too_long_bot = "b".repeat(257);
    let refused = [
        command(
            "local",
            VaultHostAction::Entry(VaultEntry {
                bot_id: too_long_bot,
                id: None,
                revision: None,
            }),
        ),
        command("", VaultHostAction::Status),
        command(
            "local",
            VaultHostAction::Fill(VaultFill {
                bot_id: "bot".to_owned(),
                generation: "g".repeat(101),
                page_id: "p".to_owned(),
                control_epoch: 1,
                id: "00000000-0000-4000-8000-000000000123".to_owned(),
                revision: 1,
            }),
        ),
    ];
    for bad in refused {
        assert_eq!(vault.command(bad).await, Err(generic()));
    }
    assert_eq!(f.window.entry_asks() + f.window.confirm_asks(), 0);
    assert_eq!(f.acquires(), 0);
}

#[test]
fn no_debug_output_shows_the_typed_secret() {
    let form = EntryForm::new(
        "Synthetic".to_owned(),
        "https://login.invalid".to_owned(),
        USER.to_owned(),
        TYPED.to_owned(),
    );
    assert!(!format!("{form:?}").contains(TYPED));
    let answer = WindowAnswer::Form(EntryForm::new(
        "Synthetic".to_owned(),
        "https://login.invalid".to_owned(),
        USER.to_owned(),
        TYPED.to_owned(),
    ));
    assert!(!format!("{answer:?}").contains(TYPED));
    let accepted: EntryInput = accept_entry(WindowAnswer::Form(EntryForm::new(
        "Synthetic".to_owned(),
        "https://login.invalid".to_owned(),
        USER.to_owned(),
        TYPED.to_owned(),
    )))
    .expect("the fields pass")
    .expect("an input");
    assert!(!format!("{accepted:?}").contains(TYPED));
    assert!(!format!("{:?}", VaultSecret::new(USER, TYPED)).contains(TYPED));
    let debugged_reply = format!("{:?}", VaultReply::List(Vec::new()));
    assert!(!debugged_reply.contains(TYPED));
}

/// The records as the vault lists them, read straight from the file.
fn vault_logins(f: &Fixture) -> Vec<LoginEntry> {
    VaultRecords::open(&f.records_path())
        .expect("the records open")
        .list(None)
        .expect("the records list")
}
