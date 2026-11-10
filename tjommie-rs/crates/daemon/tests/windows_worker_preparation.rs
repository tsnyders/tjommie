//! Port of `windows-worker-preparation.test.ts`. A fake `WorkerNative` stands in for the TypeScript tests' mocked
//! helper process: a launch waits until the test "acknowledges" it (its job exists), and the test decides when, and
//! whether, each worker tree is confirmed stopped. No Windows object is created and no credential is read.
#![cfg(windows)]

use daemon::cli::{BoxFuture, CliChild, CliExit};
use daemon::win::boundary::{
    BoundaryError, BoundaryOptions, LaunchFailure, NativeLaunch, Preparation, Receipt, Running,
    WorkerBoundary, WorkerNative,
};
use daemon::win::worker::{Purpose, WorkerConfiguration};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{Notify, watch};

struct FakeLaunch {
    pid: u32,
    purpose: Purpose,
    worker_sid: String,
    acked: watch::Sender<bool>,
    retired: watch::Sender<Option<bool>>,
    continues: AtomicUsize,
}

impl FakeLaunch {
    async fn retirement(&self) -> bool {
        let mut retired = self.retired.subscribe();
        let value = *retired
            .wait_for(Option::is_some)
            .await
            .expect("sender lives");
        value.expect("checked")
    }
}

type Verify = Arc<dyn Fn(usize) -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Default)]
struct Fake {
    requests: Mutex<Vec<Arc<FakeLaunch>>>,
    verify: Mutex<Option<Verify>>,
    verifies: AtomicUsize,
    credential_gate: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
    /// Overrides the user name in the saved worker credential.
    credential_user: Mutex<Option<String>>,
    pids: AtomicU32,
}

impl Fake {
    fn request(&self, index: usize) -> Arc<FakeLaunch> {
        self.requests.lock().unwrap()[index].clone()
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn ack(&self, index: usize) {
        self.request(index).acked.send_replace(true);
    }
    fn retire(&self, index: usize, stopped: bool) {
        self.request(index).retired.send_replace(Some(stopped));
    }
}

struct Handle(Arc<FakeLaunch>);

impl NativeLaunch for Handle {
    fn receipt(&self) -> Receipt {
        Receipt {
            purpose: self.0.purpose,
            worker_sid: self.0.worker_sid.clone(),
            worker_pid: self.0.pid,
            process: None,
        }
    }
    fn resume(
        self: Box<Self>,
        _frame: Vec<u8>,
    ) -> BoxFuture<'static, Result<Running, LaunchFailure>> {
        let launch = self.0;
        launch.continues.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let (waiter, retirer) = (launch.clone(), launch);
            Ok(Running {
                stdin: Box::new(tokio::io::sink()),
                stdout: Box::new(tokio::io::empty()),
                stderr: Box::new(tokio::io::empty()),
                exited: Box::pin(async move {
                    waiter.retirement().await;
                    Some(0)
                }),
                retire: Arc::new(move || {
                    let retirer = retirer.clone();
                    Box::pin(async move { retirer.retirement().await })
                }),
            })
        })
    }
    fn abandon(self: Box<Self>) -> BoxFuture<'static, bool> {
        Box::pin(async move { self.0.retirement().await })
    }
}

impl WorkerNative for Fake {
    fn verify<'a>(
        &'a self,
        _config: &'a WorkerConfiguration,
        known: &'a [Receipt],
    ) -> BoxFuture<'a, Result<(), String>> {
        self.verifies.fetch_add(1, Ordering::SeqCst);
        let check = self.verify.lock().unwrap().clone();
        let known = known.len();
        Box::pin(async move {
            let pass = match check {
                Some(check) => check(known).await,
                None => true,
            };
            if pass {
                Ok(())
            } else {
                Err("synthetic verification failure".into())
            }
        })
    }
    fn read_credential<'a>(
        &'a self,
        target: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        let gate = self.credential_gate.lock().unwrap().clone();
        let default = if target.ends_with("isolated-worker") {
            "TjWorker-12345678"
        } else {
            "TjReview-12345678"
        };
        let name = self
            .credential_user
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| default.into());
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            Ok(Some(
                json!({ "username": name, "password": "synthetic-not-an-os-account" }).to_string(),
            ))
        })
    }
    fn grant_directory<'a>(
        &'a self,
        _config: &'a WorkerConfiguration,
        _path: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn launch<'a>(
        &'a self,
        config: &'a WorkerConfiguration,
        _password: String,
        _cwd: &'a Path,
        _environment: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Box<dyn NativeLaunch>, LaunchFailure>> {
        let launch = Arc::new(FakeLaunch {
            pid: 10_000 + self.pids.fetch_add(1, Ordering::SeqCst),
            purpose: config.purpose,
            worker_sid: config.worker_sid.clone(),
            acked: watch::channel(false).0,
            retired: watch::channel(None).0,
            continues: AtomicUsize::new(0),
        });
        self.requests.lock().unwrap().push(launch.clone());
        Box::pin(async move {
            let (mut acked, mut retired) = (launch.acked.subscribe(), launch.retired.subscribe());
            tokio::select! {
                _ = acked.wait_for(|acked| *acked) => Ok(Box::new(Handle(launch)) as Box<dyn NativeLaunch>),
                stopped = retired.wait_for(Option::is_some) => {
                    let stopped = stopped.expect("sender lives").expect("checked");
                    Err(LaunchFailure { message: "synthetic launch failure".into(), stopped })
                }
            }
        })
    }
}

struct Fixture {
    _root: TempDir,
    native: Arc<Fake>,
    provider: WorkerBoundary,
    reviewer: WorkerBoundary,
    file: String,
    work: PathBuf,
}

impl Fixture {
    fn launch(&self) -> tokio::task::JoinHandle<Result<CliChild, BoundaryError>> {
        let (provider, file, work) = (self.provider.clone(), self.file.clone(), self.work.clone());
        tokio::spawn(async move {
            provider
                .spawn(&file, &["--synthetic".into()], &work, &[])
                .await
        })
    }
}

fn fixture() -> Fixture {
    let root = TempDir::new().unwrap();
    let path = |name: &str| root.path().join(name);
    for folder in ["owner", "runtime", "work", "review", "daemon"] {
        fs::create_dir(path(folder)).unwrap();
    }
    let file = path("runtime").join("node.exe");
    fs::write(&file, "synthetic non-executable").unwrap();
    let config = |purpose: &str| {
        let provider = purpose == "provider";
        WorkerConfiguration::parse(
            &json!({
                "version": 2, "purpose": purpose, "ownerSid": "S-1-5-21-1-2-3-1000",
                "workerSid": if provider { "S-1-5-21-1-2-3-1001" } else { "S-1-5-21-1-2-3-1002" },
                "peerSid": if provider { "S-1-5-21-1-2-3-1002" } else { "S-1-5-21-1-2-3-1001" },
                "workerName": if provider { "TjWorker-12345678" } else { "TjReview-12345678" },
                "ownerDataRoot": path("owner"), "workerRoot": path(if provider { "work" } else { "review" }),
                "peerRoot": path(if provider { "review" } else { "work" }), "runtimeRoot": path("runtime"),
                "provisionedAt": "2026-11-03T00:00:00.000Z",
                "credentialTarget": format!("Tjommie/{}/isolated-{}", "a".repeat(24), if provider { "worker" } else { "review" }),
            })
            .to_string(),
        )
        .unwrap()
    };
    let (provider, review) = (config("provider"), config("review"));
    for (current, peer) in [(&provider, &review), (&review, &provider)] {
        fs::write(
            path("owner").join(current.purpose.configuration_file()),
            serde_json::to_string(current).unwrap(),
        )
        .unwrap();
        let accepted = json!({
            "version": 2, "purpose": current.purpose, "workerSid": current.worker_sid, "peerSid": current.peer_sid,
            "configurationHash": current.hash(), "peerConfigurationHash": peer.hash(), "ownerFileDenied": true,
            "ownerCredentialDenied": true, "ownerProcessDenied": true, "ownerDesktopDenied": true, "workerWriteAllowed": true,
            "peerFileDenied": true, "peerWriteDenied": true, "peerRegistryDenied": true, "peerProcessDenied": true,
            "codexStarted": true, "claudeStarted": true, "reviewPolicyVerified": true,
        });
        let name = if current.purpose == Purpose::Provider {
            "worker-acceptance.json"
        } else {
            "review-worker-acceptance.json"
        };
        fs::write(path("owner").join(name), accepted.to_string()).unwrap();
    }
    let native = Arc::new(Fake::default());
    let boundary = |purpose: Purpose| {
        WorkerBoundary::new(BoundaryOptions {
            configuration_path: path("owner").join(purpose.configuration_file()),
            purpose,
            daemon_dir: path("daemon"),
            exclusive: Arc::new(|| true),
            native: native.clone(),
            observer: None,
        })
    };
    let (provider_boundary, reviewer) = (boundary(Purpose::Provider), boundary(Purpose::Review));
    provider_boundary.pair_with(&reviewer).unwrap();
    reviewer.pair_with(&provider_boundary).unwrap();
    Fixture {
        native,
        provider: provider_boundary,
        reviewer,
        file: file.to_string_lossy().into_owned(),
        work: path("work"),
        _root: root,
    }
}

/// Never true: a queued waiter that runs at all fails the test.
fn gate_reached_twice() -> bool {
    false
}

async fn until(what: &str, ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// Awaits something the test expects to settle: a hang (say, a lost poison) fails in 10 s instead of stalling the run.
async fn settle<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("timed out waiting for a result that should have settled")
}

#[tokio::test]
async fn serializes_paired_status_verification_behind_unreceipted_preparation_then_retries_only_a_confirmed_retired_job()
 {
    let f = fixture();
    let first = f.launch();
    until("the first launch", || f.native.count() == 1).await;
    let before = f.native.verifies.load(Ordering::SeqCst);
    let reviewer = f.reviewer.clone();
    let status = tokio::spawn(async move { reviewer.unavailable_reason().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        f.native.verifies.load(Ordering::SeqCst),
        before,
        "status ran while a launch was being prepared"
    );
    let checks = Arc::new(AtomicUsize::new(0));
    let (counted, native, provider) = (checks.clone(), f.native.clone(), f.provider.clone());
    *f.native.verify.lock().unwrap() = Some(Arc::new(move |known| {
        let check = counted.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(known, if check == 1 { 1 } else { 0 });
        let (native, provider) = (native.clone(), provider.clone());
        Box::pin(async move {
            if check == 1 {
                native.retire(0, true);
                until("the retirement", || provider.receipts().is_empty()).await;
                return false;
            }
            true
        })
    }));
    f.native.ack(0);
    let child = settle(first).await.unwrap().unwrap();
    assert_eq!(settle(status).await.unwrap(), None);
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert!(f.provider.receipts().is_empty());
    assert!(matches!(
        settle(child.exit.wait()).await,
        CliExit::Exited { .. }
    ));
}

#[tokio::test]
async fn holds_queued_preparations_out_and_poisons_both_identities_promptly_after_unknown_retirement()
 {
    let f = fixture();
    let first = f.launch();
    until("the first launch", || f.native.count() == 1).await;
    let reviewer = f.reviewer.clone();
    let status = tokio::spawn(async move { reviewer.unavailable_reason().await });
    let next = f.launch();
    tokio::time::sleep(Duration::from_millis(20)).await;
    f.native.retire(0, false);
    assert_eq!(
        settle(first).await.unwrap().err(),
        Some(BoundaryError::Termination)
    );
    assert_eq!(
        settle(next).await.unwrap().err(),
        Some(BoundaryError::Termination)
    );
    assert!(settle(status).await.unwrap().is_some());
    assert_eq!(f.native.count(), 1);
    assert_eq!(
        settle(f.launch()).await.unwrap().err(),
        Some(BoundaryError::Termination)
    );
}

#[tokio::test]
async fn retries_a_pre_ack_launch_only_after_its_stopped_proof_and_a_peers_confirmed_retirement() {
    let f = fixture();
    let first = f.launch();
    until("the first launch", || f.native.count() == 1).await;
    f.native.ack(0);
    let original = settle(first).await.unwrap().unwrap();
    let next = f.launch();
    until("the second launch", || f.native.count() == 2).await;
    assert_eq!(f.native.request(1).continues.load(Ordering::SeqCst), 0);
    f.native.retire(0, true);
    settle(original.exit.wait()).await;
    f.native.retire(1, true);
    until("the retried launch", || f.native.count() == 3).await;
    f.native.ack(2);
    let continued = settle(next).await.unwrap().unwrap();
    assert_eq!(f.native.request(1).continues.load(Ordering::SeqCst), 0);
    assert_eq!(f.native.request(2).continues.load(Ordering::SeqCst), 1);
    f.native.retire(2, true);
    assert!(matches!(
        settle(continued.exit.wait()).await,
        CliExit::Exited { .. }
    ));
}

#[tokio::test]
async fn rejects_every_queued_waiter_on_poisoning_without_waiting_for_a_blocked_preparation_to_finish()
 {
    let gate = Preparation::new();
    let (entered, finish) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let running = {
        let (gate, entered, finish) = (gate.clone(), entered.clone(), finish.clone());
        tokio::spawn(async move {
            gate.run(async {
                entered.notify_one();
                finish.notified().await;
                Ok(())
            })
            .await
        })
    };
    entered.notified().await;
    let queued = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.run(async {
                assert!(gate_reached_twice(), "a queued waiter must not enter");
                Ok(())
            })
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    gate.poison();
    // Promptly: a waiter left queued behind the blocked preparation would be the bug.
    let rejected = tokio::time::timeout(Duration::from_secs(5), queued)
        .await
        .expect("the queued waiter was not rejected");
    assert_eq!(rejected.unwrap(), Err::<(), _>(BoundaryError::Termination));
    assert_eq!(
        gate.run(async { Ok(()) }).await,
        Err(BoundaryError::Termination)
    );
    finish.notify_one();
    assert_eq!(settle(running).await.unwrap(), Ok(()));
}

#[tokio::test]
async fn refuses_a_held_verification_result_after_a_live_job_loses_retirement_proof() {
    let f = fixture();
    let first = f.launch();
    until("the first launch", || f.native.count() == 1).await;
    f.native.ack(0);
    let original = settle(first).await.unwrap().unwrap();
    let (native, exit) = (f.native.clone(), original.exit.clone());
    *f.native.verify.lock().unwrap() = Some(Arc::new(move |_| {
        native.retire(0, false);
        let exit = exit.clone();
        Box::pin(async move {
            exit.wait().await;
            true
        })
    }));
    assert_eq!(
        settle(f.launch()).await.unwrap().err(),
        Some(BoundaryError::Termination)
    );
    assert!(matches!(
        settle(original.exit.wait()).await,
        CliExit::Failed(_)
    ));
    assert_eq!(f.native.count(), 1);
}

#[tokio::test]
async fn refuses_a_credential_result_after_poisoning_without_creating_another_native_worker() {
    let f = fixture();
    let first = f.launch();
    until("the first launch", || f.native.count() == 1).await;
    f.native.ack(0);
    let original = settle(first).await.unwrap().unwrap();
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *f.native.credential_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let next = f.launch();
    entered.notified().await;
    f.native.retire(0, false);
    assert!(matches!(
        settle(original.exit.wait()).await,
        CliExit::Failed(_)
    ));
    release.notify_one();
    assert_eq!(
        settle(next).await.unwrap().err(),
        Some(BoundaryError::Termination)
    );
    assert_eq!(f.native.count(), 1);
}

#[tokio::test]
async fn settles_an_already_created_pre_ack_child_after_poisoning_without_sending_continue() {
    let f = fixture();
    let first = f.launch();
    until("the first launch", || f.native.count() == 1).await;
    f.native.ack(0);
    let original = settle(first).await.unwrap().unwrap();
    let next = f.launch();
    until("the second launch", || f.native.count() == 2).await;
    f.native.retire(0, false);
    assert!(matches!(
        settle(original.exit.wait()).await,
        CliExit::Failed(_)
    ));
    f.native.ack(1);
    f.native.retire(1, true);
    assert_eq!(
        settle(next).await.unwrap().err(),
        Some(BoundaryError::Termination)
    );
    assert_eq!(f.native.request(1).continues.load(Ordering::SeqCst), 0);
}

/// One `#spawnHeld` refusal: `Unavailable`, promptly, and no native launch at all.
async fn refused(f: &Fixture, file: &str, env: &[(String, String)]) {
    let args = ["--synthetic".to_string()];
    let launch = f.provider.spawn(file, &args, &f.work, env);
    let result = tokio::time::timeout(Duration::from_secs(10), launch)
        .await
        .expect("the launch went ahead instead of being refused");
    assert_eq!(result.err(), Some(BoundaryError::Unavailable));
    assert_eq!(f.native.count(), 0);
}

#[tokio::test]
async fn refuses_a_target_that_is_not_an_exe_without_starting_anything() {
    let f = fixture();
    let text = Path::new(&f.file).with_file_name("tool.txt");
    fs::write(&text, "synthetic").unwrap();
    refused(&f, text.to_str().unwrap(), &[]).await;
}

#[tokio::test]
async fn refuses_provider_homes_outside_the_worker_root_without_starting_anything() {
    let f = fixture();
    // A folder a worker could otherwise be granted (an external project), so only the home check refuses it.
    let elsewhere = f.work.with_file_name("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    for key in ["CODEX_HOME", "CLAUDE_CONFIG_DIR"] {
        refused(
            &f,
            &f.file,
            &[(key.into(), elsewhere.to_string_lossy().into_owned())],
        )
        .await;
    }
}

#[tokio::test]
async fn refuses_a_credential_that_names_the_other_worker_without_starting_anything() {
    let f = fixture();
    *f.native.credential_user.lock().unwrap() = Some("TjReview-12345678".into());
    refused(&f, &f.file, &[]).await;
}
