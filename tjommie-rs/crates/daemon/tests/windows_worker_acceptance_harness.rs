//! The real acceptance harness (`win::acceptance::WindowsSteps`) over a fake Windows.
//!
//! `WorkerNative` is the seam under every worker launch. The fake here creates no account, no process and no
//! Credential Manager read of its own: each "launch" recognises which fixed program it was asked to run (the policy
//! observer, the peer holder or the denial probe) and plays it. That runs everything the real check does around the OS:
//! nesting inside the review lease, holding the peer live, the probe payloads, the collection limits, the stop proofs,
//! the receipts, and `ready` reading them back. What only real accounts can show (that Windows refuses what the probe
//! tries) is the admin-gated test in `windows_worker_acceptance.rs`.
#![cfg(windows)]

use daemon::cli::BoxFuture;
use daemon::win::acceptance::{
    CliProbe, Credentials, Denials, Failure, OwnerDesktop, PROBE_INNER_TIMEOUT, Round, Seams,
    accept_with, receipt_path, receipts,
};
use daemon::win::boundary::{LaunchFailure, NativeLaunch, Receipt, Running, WorkerNative};
use daemon::win::logon::DesktopRefusal;
use daemon::win::worker::{Purpose, WorkerConfiguration};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

const OBSERVED: &str = r#"{"version":1,"registryAbsent":true,"files":[["managed-settings.json",null],["managed-mcp.json",null]]}"#;

fn all_denied() -> String {
    json!({
        "ownerFileDenied": true, "ownerCredentialDenied": true, "ownerProcessDenied": true, "ownerDesktopDenied": true,
        "workerWriteAllowed": true, "peerFileDenied": true, "peerWriteDenied": true, "peerRegistryDenied": true,
        "peerProcessDenied": true,
    })
    .to_string()
}

fn proven() -> Round {
    Round {
        denials: Denials::parse(&all_denied()).unwrap(),
        claude_started: true,
        codex_started: true,
        review_policy_verified: true,
    }
}

/// "Both CLIs start", mocked: no Claude or Codex is installed, signed in or asked anything.
/// It counts how often it is asked to start a CLI.
struct Cli(Result<(), Failure>, Arc<AtomicUsize>);

impl CliProbe for Cli {
    fn started<'a>(
        &'a self,
        _active: Purpose,
        _cli: protocol::model::Provider,
        _account: &'a Path,
    ) -> BoxFuture<'a, Result<(), Failure>> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { self.0.clone() })
    }
}

/// The owner's desktop, scripted: it is there unless told otherwise, and it counts how often it is asked.
#[derive(Default)]
struct Desktop {
    /// The call and Win32 error the control fails with, if it is to fail.
    refusal: Mutex<Option<DesktopRefusal>>,
    asked: AtomicUsize,
}

impl OwnerDesktop for Desktop {
    fn reachable(&self) -> Result<(), DesktopRefusal> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        match self.refusal.lock().unwrap().clone() {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        }
    }
}

/// What the fake Windows did and what it is told to do.
struct State {
    log: Mutex<Vec<String>>,
    payloads: Mutex<Vec<Value>>,
    holder_pids: Mutex<Vec<u32>>,
    live_holders: AtomicUsize,
    /// What the denial probe prints, by who runs it: provider, then review.
    probe: Mutex<[String; 2]>,
    /// How long the denial probe runs before it ends, and the exit code it ends with.
    probe_delay: Mutex<Duration>,
    probe_exit: AtomicI32,
    /// No worker credential is saved.
    no_credential: AtomicBool,
    /// What the peer holder prints as it starts.
    holder_says: Mutex<String>,
    /// The holder's tree cannot be confirmed stopped.
    holder_unconfirmed: AtomicBool,
    pids: AtomicU32,
    vault: Arc<Vault>,
    /// How many CLI `--version` launches the check asked for.
    cli_launches: Arc<AtomicUsize>,
    /// The owner's desktop, for the positive control of the desktop denial.
    desktop: Arc<Desktop>,
}

/// An in-memory stand-in for Credential Manager: nothing here ever reaches the real vault.
#[derive(Default)]
struct Vault {
    written: Mutex<Vec<String>>,
    deleted: Mutex<Vec<String>>,
}

impl Credentials for Vault {
    fn write(&self, target: &str, _value: &str) -> std::io::Result<()> {
        self.written.lock().unwrap().push(target.to_owned());
        Ok(())
    }

    fn delete(&self, target: &str) -> std::io::Result<()> {
        self.deleted.lock().unwrap().push(target.to_owned());
        Ok(())
    }
}

struct Fake(Arc<State>);

impl Fake {
    fn new() -> Self {
        Fake(Arc::new(State {
            log: Mutex::default(),
            payloads: Mutex::default(),
            holder_pids: Mutex::default(),
            live_holders: AtomicUsize::new(0),
            probe: Mutex::new([all_denied(), all_denied()]),
            probe_delay: Mutex::new(Duration::ZERO),
            probe_exit: AtomicI32::new(0),
            no_credential: AtomicBool::new(false),
            holder_says: Mutex::new("ready\n".into()),
            holder_unconfirmed: AtomicBool::new(false),
            pids: AtomicU32::new(40_000),
            vault: Arc::new(Vault::default()),
            cli_launches: Arc::new(AtomicUsize::new(0)),
            desktop: Arc::new(Desktop::default()),
        }))
    }
}

struct Launch {
    state: Arc<State>,
    pid: u32,
    purpose: Purpose,
    sid: String,
}

impl NativeLaunch for Launch {
    fn receipt(&self) -> Receipt {
        Receipt {
            purpose: self.purpose,
            worker_sid: self.sid.clone(),
            worker_pid: self.pid,
            process: None,
        }
    }

    fn abandon(self: Box<Self>) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }

    fn resume(
        self: Box<Self>,
        frame: Vec<u8>,
    ) -> BoxFuture<'static, Result<Running, LaunchFailure>> {
        Box::pin(async move {
            let target: Value = serde_json::from_slice(&frame[4..]).unwrap();
            let args: Vec<String> = serde_json::from_value(target["args"].clone()).unwrap();
            let script = args.get(1).cloned().unwrap_or_default();
            let (stdout, mut out) = tokio::io::duplex(8192);
            let (stdin, mut fake_in) = tokio::io::duplex(8192);
            let stopped = Arc::new(watch::channel(false).0);
            let (state, who) = (self.state.clone(), self.purpose);
            let index = usize::from(who == Purpose::Review);
            let mut unconfirmed = false;
            let ended_with = Arc::new(AtomicI32::new(0));
            if script.contains("ClaudeCode") {
                state
                    .log
                    .lock()
                    .unwrap()
                    .push(format!("observer {}", who.as_str()));
                let stopped = stopped.clone();
                tokio::spawn(async move {
                    out.write_all(OBSERVED.as_bytes()).await.unwrap();
                    drop(out);
                    stopped.send_replace(true);
                });
            } else if script.contains("process.stdin.pipe") {
                state
                    .log
                    .lock()
                    .unwrap()
                    .push(format!("holder {}", who.as_str()));
                state.holder_pids.lock().unwrap().push(self.pid);
                unconfirmed = state.holder_unconfirmed.load(Ordering::SeqCst);
                state.live_holders.fetch_add(1, Ordering::SeqCst);
                let (stopped, says) = (stopped.clone(), state.holder_says.lock().unwrap().clone());
                tokio::spawn(async move {
                    let _ = out.write_all(says.as_bytes()).await;
                    // A holder that says nothing has died; otherwise it is held until the daemon closes its stdin,
                    // like the real one.
                    if !says.is_empty() {
                        let _ = fake_in.read_to_end(&mut Vec::new()).await;
                    }
                    state.live_holders.fetch_sub(1, Ordering::SeqCst);
                    drop(out);
                    stopped.send_replace(true);
                });
            } else {
                assert!(
                    script.contains("TjWorkerProbe"),
                    "an unknown fixed program was launched"
                );
                state
                    .payloads
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&args[2]).unwrap());
                state.log.lock().unwrap().push(format!(
                    "probe {} peers={}",
                    who.as_str(),
                    state.live_holders.load(Ordering::SeqCst)
                ));
                let (stopped, says) = (stopped.clone(), state.probe.lock().unwrap()[index].clone());
                let (delay, code, ended_with) = (
                    *state.probe_delay.lock().unwrap(),
                    state.probe_exit.load(Ordering::SeqCst),
                    ended_with.clone(),
                );
                tokio::spawn(async move {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    let _ = out.write_all(says.as_bytes()).await;
                    drop(out);
                    ended_with.store(code, Ordering::SeqCst);
                    stopped.send_replace(true);
                });
            }
            let (mut waiting, stopping) = (stopped.subscribe(), stopped.clone());
            Ok(Running {
                stdin: Box::new(stdin),
                stdout: Box::new(stdout),
                stderr: Box::new(tokio::io::empty()),
                exited: Box::pin(async move {
                    let _ = waiting.wait_for(|stopped| *stopped).await;
                    Some(ended_with.load(Ordering::SeqCst))
                }),
                retire: Arc::new(move || {
                    let stopping = stopping.clone();
                    Box::pin(async move {
                        stopping.send_replace(true);
                        !unconfirmed
                    })
                }),
            })
        })
    }
}

impl WorkerNative for Fake {
    fn verify<'a>(
        &'a self,
        _: &'a WorkerConfiguration,
        _: &'a [Receipt],
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn read_credential<'a>(
        &'a self,
        target: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        let name = if target.ends_with("isolated-worker") {
            "TjWorker-12345678"
        } else {
            "TjReview-12345678"
        };
        let none = self.0.no_credential.load(Ordering::SeqCst);
        Box::pin(async move {
            if none {
                return Ok(None);
            }
            Ok(Some(
                json!({ "username": name, "password": "not-an-os-password" }).to_string(),
            ))
        })
    }

    fn grant_directory<'a>(
        &'a self,
        _: &'a WorkerConfiguration,
        _: &'a Path,
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
        let launch = Launch {
            state: self.0.clone(),
            pid: self.0.pids.fetch_add(1, Ordering::SeqCst),
            purpose: config.purpose,
            sid: config.worker_sid.clone(),
        };
        Box::pin(async move { Ok(Box::new(launch) as Box<dyn NativeLaunch>) })
    }
}

/// A provisioned-looking install in a temporary folder: two configurations, a runtime with a `node.exe`, both worker
/// roots and a daemon folder, under a credential namespace of its own so nothing resembles a real install.
struct World {
    _folder: TempDir,
    owner: PathBuf,
    daemon_dir: PathBuf,
    work: PathBuf,
    review: PathBuf,
}

const NAMESPACE: &str = "5ca1ab1e5ca1ab1e5ca1ab1e";

fn world() -> World {
    let folder = TempDir::new().unwrap();
    let path = |name: &str| folder.path().join(name);
    for name in ["owner", "runtime", "work", "review", "daemon"] {
        fs::create_dir(path(name)).unwrap();
    }
    fs::write(path("runtime").join("node.exe"), "not a program").unwrap();
    for purpose in ["provider", "review"] {
        let provider = purpose == "provider";
        let config = json!({
            "version": 2, "purpose": purpose, "ownerSid": "S-1-5-21-1-2-3-1000",
            "workerSid": if provider { "S-1-5-21-1-2-3-1001" } else { "S-1-5-21-1-2-3-1002" },
            "peerSid": if provider { "S-1-5-21-1-2-3-1002" } else { "S-1-5-21-1-2-3-1001" },
            "workerName": if provider { "TjWorker-12345678" } else { "TjReview-12345678" },
            "workerRoot": path(if provider { "work" } else { "review" }),
            "peerRoot": path(if provider { "review" } else { "work" }),
            "runtimeRoot": path("runtime"), "ownerDataRoot": path("owner"),
            "provisionedAt": "2026-11-03T00:00:00.000Z",
            "credentialTarget": format!("Tjommie/{NAMESPACE}/isolated-{}", if provider { "worker" } else { "review" }),
        });
        let file = if provider {
            "worker.json"
        } else {
            "review-worker.json"
        };
        fs::write(path("owner").join(file), config.to_string()).unwrap();
    }
    World {
        owner: path("owner"),
        daemon_dir: path("daemon"),
        work: path("work"),
        review: path("review"),
        _folder: folder,
    }
}

fn configs(world: &World) -> (WorkerConfiguration, WorkerConfiguration) {
    (
        WorkerConfiguration::load(&world.owner.join("worker.json"), Purpose::Provider).unwrap(),
        WorkerConfiguration::load(&world.owner.join("review-worker.json"), Purpose::Review)
            .unwrap(),
    )
}

fn starting_with(root: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix))
        .collect()
}

fn scratch_folders(world: &World) -> usize {
    [&world.work, &world.review]
        .iter()
        .map(|root| starting_with(root, "acceptance-").len())
        .sum()
}

/// Runs the check against the fake Windows and says nothing more.
async fn accept_unchecked(
    world: &World,
    windows: &Fake,
    cli: Result<(), Failure>,
) -> Result<String, String> {
    let native: Arc<dyn WorkerNative> = Arc::new(Fake(windows.0.clone()));
    let seams = Seams {
        cli: Some(Arc::new(Cli(cli, windows.0.cli_launches.clone()))),
        credentials: Some(windows.0.vault.clone()),
        desktop: Some(windows.0.desktop.clone()),
    };
    accept_with(&world.owner, &world.daemon_dir, native, seams).await
}

/// [`accept_unchecked`], and whatever the outcome checks the synthetic credential was cleaned up.
async fn accept(world: &World, windows: &Fake, cli: Result<(), Failure>) -> Result<String, String> {
    let result = accept_unchecked(world, windows, cli).await;
    // Whatever the outcome, every synthetic credential the check saved was deleted again.
    let (written, deleted) = (
        windows.0.vault.written.lock().unwrap().clone(),
        windows.0.vault.deleted.lock().unwrap().clone(),
    );
    assert!(
        !written.is_empty(),
        "the check never saved its synthetic credential"
    );
    for target in &written {
        assert!(
            deleted.contains(target),
            "{target} was left in Credential Manager"
        );
        assert!(
            target.starts_with(&format!("Tjommie/{NAMESPACE}/acceptance/")),
            "{target}"
        );
        assert!(!target.ends_with("isolated-worker"), "{target}");
    }
    result
}

fn receipts_present(world: &World) -> [bool; 2] {
    [Purpose::Provider, Purpose::Review].map(|purpose| receipt_path(&world.owner, purpose).exists())
}

#[tokio::test]
async fn the_real_harness_runs_both_rounds_nested_in_the_review_lease_and_writes_receipts_the_daemon_accepts()
 {
    let world = world();
    let windows = Fake::new();
    let done = accept(&world, &windows, Ok(())).await.unwrap();
    assert!(done.starts_with("Acceptance passed"), "{done}");
    // The owner's own desktop was checked once, as the positive control of the desktop denial.
    assert_eq!(windows.0.desktop.asked.load(Ordering::SeqCst), 1);
    // The review identity is only ever launched inside its observed lease, the other identity's fixture is live while
    // an identity proves what it cannot reach, and the second observation comes after that fixture is gone.
    assert_eq!(
        *windows.0.log.lock().unwrap(),
        [
            "observer review",
            "holder review",
            "probe provider peers=1",
            "observer review",
            "holder provider",
            "observer review",
            "probe review peers=1",
            "observer review",
        ]
    );
    assert_eq!(
        windows.0.live_holders.load(Ordering::SeqCst),
        0,
        "a fixture was left running"
    );
    // The payloads carry what the probe needs, and nothing of the owner's.
    let payloads = windows.0.payloads.lock().unwrap().clone();
    let holders = windows.0.holder_pids.lock().unwrap().clone();
    assert_eq!(payloads.len(), 2);
    let slash = |text: &str| text.replace('/', "\\");
    for (payload, (active, peer_sid, holder)) in payloads.iter().zip([
        ("work", "S-1-5-21-1-2-3-1002", holders[0]),
        ("review", "S-1-5-21-1-2-3-1001", holders[1]),
    ]) {
        assert_eq!(payload["ownerPid"], json!(std::process::id()));
        assert_eq!(
            payload["peerPid"],
            json!(holder),
            "the probe is told the live fixture's process"
        );
        assert!(
            slash(payload["marker"].as_str().unwrap())
                .starts_with(&slash(&world.owner.to_string_lossy()))
        );
        assert!(
            slash(payload["write"].as_str().unwrap()).contains(&format!("{active}\\acceptance-")),
            "{payload}"
        );
        let peer_registry = payload["peerRegistry"].as_str().unwrap();
        assert!(
            peer_registry.starts_with(&format!("{peer_sid}\\Software\\Tjommie\\Acceptance\\")),
            "{peer_registry}"
        );
        let target = payload["target"].as_str().unwrap();
        assert!(
            target.starts_with(&format!("Tjommie/{NAMESPACE}/acceptance/")),
            "{target}"
        );
        // The probe was told the synthetic credential's name (accept() checks it was deleted again).
        assert!(
            windows
                .0
                .vault
                .written
                .lock()
                .unwrap()
                .contains(&target.to_owned()),
            "{target}"
        );
    }
    // The receipts are the pair the daemon's own readiness check (`ready`, then the boundary's verify) accepts.
    assert_eq!(receipts_present(&world), [true, true]);
    daemon::win::worker::ready(&world.owner.join("worker.json"), Purpose::Provider).unwrap();
    daemon::win::worker::ready(&world.owner.join("review-worker.json"), Purpose::Review).unwrap();
    // Nothing of the check is left: no scratch folder, no owner marker.
    assert_eq!(scratch_folders(&world), 0);
    assert_eq!(
        starting_with(&world.owner, "acceptance-"),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn a_probe_that_reports_a_failure_writes_no_receipt_and_releases_the_peer_fixture() {
    for (active, field) in [
        (0, "peerRegistryDenied"),
        (1, "ownerProcessDenied"),
        (0, "workerWriteAllowed"),
    ] {
        let world = world();
        let windows = Fake::new();
        let mut report: Value = serde_json::from_str(&all_denied()).unwrap();
        report[field] = json!(false);
        windows.0.probe.lock().unwrap()[active] = report.to_string();
        // A valid pair from an earlier run must not survive either.
        let (provider, review) = configs(&world);
        let [stale, other] = receipts(&provider, &review, &proven(), &proven()).unwrap();
        fs::write(receipt_path(&world.owner, Purpose::Provider), stale).unwrap();
        fs::write(receipt_path(&world.owner, Purpose::Review), other).unwrap();
        let error = accept(&world, &windows, Ok(())).await.unwrap_err();
        assert!(error.contains(field), "{field}: {error}");
        assert_eq!(receipts_present(&world), [false, false], "{field}");
        assert_eq!(
            windows.0.live_holders.load(Ordering::SeqCst),
            0,
            "{field}: the peer fixture kept running"
        );
        assert_eq!(
            scratch_folders(&world),
            0,
            "{field}: an ordinary failure cleans up after itself"
        );
        // An identity that has just shown it is not isolated is not asked to start either CLI (0.4.2 threw first). The
        // provider round, which came first, did start both.
        assert_eq!(
            windows.0.cli_launches.load(Ordering::SeqCst),
            2 * active,
            "{field}: a CLI was launched under the identity whose denial failed"
        );
    }
}

/// The positive control of the desktop denial: a worker's refusal at a desktop that is not there proves nothing, so a PC
/// whose owner cannot reach its own desktop (nobody signed in, a session with no desktop, a locked screen) stops the run
/// first, in plain words with the Win32 error, before a worker is launched or anything is made, and leaves no receipt.
/// What the owner is told depends on which call failed: no desktop at all (the station), or a desktop that is not theirs
/// to open right now (the input desktop: most likely a locked screen), and each says what to do.
#[tokio::test]
async fn a_pc_with_no_signed_in_desktop_fails_closed_before_anything_is_launched_or_made() {
    let station = "OpenWindowStation(WinSta0) failed with Windows error 5: Access is denied";
    let input = "OpenInputDesktop failed with Windows error 5: Access is denied";
    for (refusal, reason, expected) in [
        (
            DesktopRefusal::Station(station.into()),
            station,
            format!(
                "This PC has no signed-in desktop to protect, so the desktop check can't be proven here ({station}). Run accept-worker from your own signed-in session."
            ),
        ),
        (
            DesktopRefusal::InputDesktop(input.into()),
            input,
            format!(
                "Your signed-in desktop can't be reached right now, most likely because the screen is locked or the sign-in screen is showing, so the desktop check can't be proven here ({input}). Unlock the screen and run accept-worker again."
            ),
        ),
    ] {
        let world = world();
        let windows = Fake::new();
        *windows.0.desktop.refusal.lock().unwrap() = Some(refusal);
        // A valid pair from an earlier run must not survive it either.
        let (provider, review) = configs(&world);
        let [stale, other] = receipts(&provider, &review, &proven(), &proven()).unwrap();
        fs::write(receipt_path(&world.owner, Purpose::Provider), stale).unwrap();
        fs::write(receipt_path(&world.owner, Purpose::Review), other).unwrap();
        let error = accept_unchecked(&world, &windows, Ok(()))
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(
            error.contains("Windows error 5"),
            "{reason}: the Win32 error is in the message: {error}"
        );
        assert_eq!(windows.0.desktop.asked.load(Ordering::SeqCst), 1);
        assert_eq!(receipts_present(&world), [false, false], "{reason}");
        assert!(
            windows.0.log.lock().unwrap().is_empty(),
            "{reason}: a worker was launched"
        );
        assert!(windows.0.payloads.lock().unwrap().is_empty(), "{reason}");
        assert_eq!(windows.0.cli_launches.load(Ordering::SeqCst), 0, "{reason}");
        assert!(
            windows.0.vault.written.lock().unwrap().is_empty(),
            "{reason}: the synthetic credential was saved"
        );
        assert_eq!(scratch_folders(&world), 0, "{reason}");
        assert_eq!(
            starting_with(&world.owner, "acceptance-"),
            Vec::<String>::new(),
            "{reason}: the owner marker was made"
        );
    }
}

#[tokio::test]
async fn a_cli_that_does_not_start_writes_no_receipt() {
    let world = world();
    let windows = Fake::new();
    let refusal = Failure::Check("claude could not start in the review identity.".into());
    let error = accept(&world, &windows, Err(refusal)).await.unwrap_err();
    assert_eq!(error, "claude could not start in the review identity.");
    assert_eq!(receipts_present(&world), [false, false]);
    assert_eq!(windows.0.live_holders.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_peer_fixture_that_does_not_come_up_fails_the_run_instead_of_probing_an_absent_peer() {
    for (says, expected) in [
        ("", "The peer fixture closed before ready."),
        (&"x".repeat(200), "The peer fixture did not initialize."),
    ] {
        let world = world();
        let windows = Fake::new();
        *windows.0.holder_says.lock().unwrap() = says.into();
        let error = accept(&world, &windows, Ok(())).await.unwrap_err();
        assert_eq!(error, expected);
        assert!(
            windows.0.payloads.lock().unwrap().is_empty(),
            "the probe ran with no live peer"
        );
        assert_eq!(receipts_present(&world), [false, false]);
        assert_eq!(windows.0.live_holders.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn a_probe_that_prints_more_than_it_may_fails_the_run() {
    let world = world();
    let windows = Fake::new();
    windows.0.probe.lock().unwrap()[0] = "x".repeat(5000);
    let error = accept(&world, &windows, Ok(())).await.unwrap_err();
    assert_eq!(error, "The synthetic worker denial probe failed.");
    assert_eq!(receipts_present(&world), [false, false]);
    assert_eq!(windows.0.live_holders.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_probe_report_that_is_not_the_nine_fields_fails_the_run() {
    for report in ["not json", "{}", r#"{"ownerFileDenied":true}"#] {
        let world = world();
        let windows = Fake::new();
        windows.0.probe.lock().unwrap()[0] = report.into();
        assert!(accept(&world, &windows, Ok(())).await.is_err(), "{report}");
        assert_eq!(receipts_present(&world), [false, false], "{report}");
    }
}

#[tokio::test]
async fn a_worker_tree_that_cannot_be_confirmed_stopped_writes_no_receipt_and_keeps_its_scratch_folders()
 {
    let world = world();
    let windows = Fake::new();
    windows.0.holder_unconfirmed.store(true, Ordering::SeqCst);
    let error = accept(&world, &windows, Ok(())).await.unwrap_err();
    // The fixed sentence stays first, so anything that matches on it still does; what follows is the evidence.
    assert!(
        error.starts_with(daemon::cli::WORKER_TERMINATION),
        "{error}"
    );
    // The cause is in the error itself, short; the full record is only for TJOMMIE_WORKER_DIAG=1.
    assert!(error.contains("What went wrong"), "{error}");
    assert!(
        !error.contains("Everything the worker launch path recorded"),
        "the full record is verbose-only: {error}"
    );
    // The first cause is on record under its route, so a termination can be diagnosed from its own log. (The record
    // is shared by the tests running in parallel here, so it is read whole rather than from the error's tail.)
    let recorded = daemon::win::diag::recent(3000);
    assert!(
        recorded
            .iter()
            .any(|line| line.contains("First cause: ROUTE 4")),
        "no ROUTE 4 line among {} recorded",
        recorded.len()
    );
    assert_eq!(receipts_present(&world), [false, false]);
    // Kept for the owner to reconcile; the synthetic credential and the owner marker are still removed.
    assert_eq!(
        scratch_folders(&world),
        2,
        "the scratch folders of an unconfirmed tree must stay"
    );
    assert_eq!(
        starting_with(&world.owner, "acceptance-"),
        Vec::<String>::new()
    );
}

/// What is wrong, how to make it so, and what the error must say.
type EarlyFailure = (&'static str, fn(&World), &'static str);

/// `accept_with` fails before it reaches `run` when the configuration is missing or is not a pair. A pair of receipts from
/// an earlier run must be gone on those exits too, so that every way out starts from "no receipts".
#[tokio::test]
async fn an_early_failure_still_removes_an_old_receipt_pair() {
    let cases: [EarlyFailure; 3] = [
        (
            "a missing configuration",
            |world| fs::remove_file(world.owner.join("review-worker.json")).unwrap(),
            "There is no worker configuration in",
        ),
        (
            "a configuration that is not a pair",
            |world| fs::write(world.owner.join("review-worker.json"), "{}").unwrap(),
            "is not a consistent provider and review pair",
        ),
        (
            // Clearing comes before the configuration is even read, so this is what is reported.
            "an old receipt that cannot be removed",
            |world| {
                fs::remove_file(world.owner.join("worker.json")).unwrap();
                fs::remove_file(receipt_path(&world.owner, Purpose::Provider)).unwrap();
                fs::create_dir(receipt_path(&world.owner, Purpose::Provider)).unwrap();
            },
            "could not be removed",
        ),
    ];
    for (what, spoil, expected) in cases {
        let world = world();
        let windows = Fake::new();
        let (provider, review) = configs(&world);
        let [stale, other] = receipts(&provider, &review, &proven(), &proven()).unwrap();
        let (one, two) = (
            receipt_path(&world.owner, Purpose::Provider),
            receipt_path(&world.owner, Purpose::Review),
        );
        fs::write(&one, stale).unwrap();
        fs::write(&two, other).unwrap();
        fs::write(format!("{}.pending", two.display()), "half written").unwrap();
        spoil(&world);
        let error = accept_unchecked(&world, &windows, Ok(()))
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{what}: {error}");
        // The review receipt and its half-written twin are gone; the provider one is gone unless it was made a folder
        // to stand for a receipt that cannot be removed.
        assert!(!two.exists(), "{what}: the review receipt survived");
        assert!(
            !std::path::Path::new(&format!("{}.pending", two.display())).exists(),
            "{what}"
        );
        assert!(!one.is_file(), "{what}: the provider receipt survived");
        assert!(
            windows.0.log.lock().unwrap().is_empty(),
            "{what}: something was launched"
        );
        assert!(
            windows.0.vault.written.lock().unwrap().is_empty(),
            "{what}: a credential was saved"
        );
    }
}

/// The probe's own time limit (the 0.4.2 wrapper had 15 s, which a first PowerShell start under a worker's environment
/// overran) kills PowerShell and ends the probe with exit code 1 and no output, which is all the wrapper can tell. That
/// has to be reported for what it is, with the limit, and not as a failed denial.
#[tokio::test(start_paused = true)]
async fn a_probe_killed_by_its_own_time_limit_is_reported_as_a_timeout() {
    let world = world();
    let windows = Fake::new();
    *windows.0.probe_delay.lock().unwrap() = PROBE_INNER_TIMEOUT;
    windows.0.probe_exit.store(1, Ordering::SeqCst);
    windows.0.probe.lock().unwrap()[0] = String::new();
    let error = accept(&world, &windows, Ok(())).await.unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert!(
        error.contains(&format!("{} s", PROBE_INNER_TIMEOUT.as_secs())),
        "the limit is named: {error}"
    );
    assert!(!error.contains("probe failed"), "{error}");
    assert_eq!(receipts_present(&world), [false, false]);
    assert_eq!(windows.0.live_holders.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_probe_that_exits_nonzero_at_once_says_what_it_saw() {
    let world = world();
    let windows = Fake::new();
    windows.0.probe_exit.store(1, Ordering::SeqCst);
    windows.0.probe.lock().unwrap()[0] = String::new();
    let error = accept(&world, &windows, Ok(())).await.unwrap_err();
    assert!(error.contains("exited with code 1"), "{error}");
    assert!(error.contains("printed nothing"), "{error}");
    assert!(
        !error.contains("timed out"),
        "it did not run for its limit: {error}"
    );
    assert_eq!(receipts_present(&world), [false, false]);
}

/// A refused launch answers with one fixed sentence; the reason is recorded and carried in the error, so a failure can
/// be told apart without turning any tracing on.
#[tokio::test]
async fn a_refused_launch_says_why() {
    let world = world();
    let windows = Fake::new();
    windows.0.no_credential.store(true, Ordering::SeqCst);
    let error = accept(&world, &windows, Ok(())).await.unwrap_err();
    assert!(
        error.starts_with("The isolated Windows provider could not start."),
        "{error}"
    );
    assert!(error.contains("What went wrong"), "{error}");
    // (Read from the whole record: tests running in parallel here record into the same one.)
    assert!(
        daemon::win::diag::recent(3000)
            .iter()
            .any(|line| line.contains("refused: no worker credential is saved at")),
        "the reason is not on record"
    );
    assert_eq!(receipts_present(&world), [false, false]);
}
