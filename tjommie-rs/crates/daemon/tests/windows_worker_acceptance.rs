//! The isolated-worker acceptance check (`scripts/vault/accept-worker.ts` in 0.4.2, ported to `win::acceptance`).
//!
//! Most of this is the protocol around the check, run against a fake `Steps`: every denial and both CLI starts must be
//! proven before either receipt is written, a failure leaves no receipt (not even an old one), and the receipts are the
//! bytes 0.4.2 wrote (`fixtures/compat/worker-acceptance.json`) that the Rust validator accepts. The real probe
//! scripts are run once as the current user, who must not be denied anything. The one test that creates real worker
//! accounts is admin-gated and only runs on the CI runner.
#![cfg(windows)]

use daemon::cli::{BoxFuture, CliOutput};
use daemon::spawn;
use daemon::win::acceptance::{
    CliProbe, Denials, Failure, Round, Seams, Steps, accept_with, holder_args, probe_args,
    receipt_path, receipts, removable_scratch, run, version_started,
};
use daemon::win::boundary::{WindowsNative, WorkerNative};
use daemon::win::worker::{
    Purpose, WorkerConfiguration, command_line, launch_frame, validate_acceptance,
};
use daemon::win::{cred, logon};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use tempfile::TempDir;

fn golden() -> Value {
    serde_json::from_str(include_str!("fixtures/compat/worker-acceptance.json")).unwrap()
}

fn golden_configs() -> (WorkerConfiguration, WorkerConfiguration) {
    let golden = golden();
    let load = |purpose: &str| {
        WorkerConfiguration::parse(&golden["configurations"][purpose].to_string()).unwrap()
    };
    (load("provider"), load("review"))
}

fn proven() -> Round {
    Round {
        denials: Denials {
            owner_file_denied: true,
            owner_credential_denied: true,
            owner_process_denied: true,
            owner_desktop_denied: true,
            worker_write_allowed: true,
            peer_file_denied: true,
            peer_write_denied: true,
            peer_registry_denied: true,
            peer_process_denied: true,
        },
        claude_started: true,
        codex_started: true,
        review_policy_verified: true,
    }
}

/// A way to make one check of a round not hold.
type Break = fn(&mut Round);

/// Every one of the twelve things a receipt claims, with the way to make the check not hold.
fn every_check() -> Vec<(&'static str, Break)> {
    vec![
        ("ownerFileDenied", |r| r.denials.owner_file_denied = false),
        ("ownerCredentialDenied", |r| {
            r.denials.owner_credential_denied = false
        }),
        ("ownerProcessDenied", |r| {
            r.denials.owner_process_denied = false
        }),
        ("ownerDesktopDenied", |r| {
            r.denials.owner_desktop_denied = false
        }),
        ("workerWriteAllowed", |r| {
            r.denials.worker_write_allowed = false
        }),
        ("peerFileDenied", |r| r.denials.peer_file_denied = false),
        ("peerWriteDenied", |r| r.denials.peer_write_denied = false),
        ("peerRegistryDenied", |r| {
            r.denials.peer_registry_denied = false
        }),
        ("peerProcessDenied", |r| {
            r.denials.peer_process_denied = false
        }),
        ("claudeStarted", |r| r.claude_started = false),
        ("codexStarted", |r| r.codex_started = false),
        ("reviewPolicyVerified", |r| r.review_policy_verified = false),
    ]
}

// ---- receipts -------------------------------------------------------------------------------------------------

#[test]
fn the_receipts_are_the_bytes_the_typescript_acceptance_wrote() {
    let golden = golden();
    let (provider, review) = golden_configs();
    // The configuration hashes in the golden file come from the zod schema and workerConfigurationHash on 0.4.2.
    assert_eq!(
        provider.hash(),
        golden["configurationHashes"]["provider"].as_str().unwrap()
    );
    assert_eq!(
        review.hash(),
        golden["configurationHashes"]["review"].as_str().unwrap()
    );
    let denials = Denials::parse(&golden["denials"].to_string()).unwrap();
    let round = Round {
        denials,
        ..proven()
    };
    let [provider_receipt, review_receipt] = receipts(&provider, &review, &round, &round).unwrap();
    assert_eq!(
        provider_receipt,
        golden["receipts"]["provider"].as_str().unwrap()
    );
    assert_eq!(
        review_receipt,
        golden["receipts"]["review"].as_str().unwrap()
    );
}

#[test]
fn the_rust_validator_accepts_the_receipts_and_only_for_their_own_pair() {
    let (provider, review) = golden_configs();
    let [provider_receipt, review_receipt] =
        receipts(&provider, &review, &proven(), &proven()).unwrap();
    validate_acceptance(&provider, &review, &provider_receipt).unwrap();
    validate_acceptance(&review, &provider, &review_receipt).unwrap();
    // A receipt is for one identity of one pair: the other identity's configuration refuses it.
    assert!(validate_acceptance(&review, &provider, &provider_receipt).is_err());
    assert!(validate_acceptance(&provider, &review, &review_receipt).is_err());
    // A receipt written for a pair whose configuration later changed stops validating.
    let mut moved = provider.clone();
    moved.provisioned_at = "2026-11-10T12:00:00.000Z".into();
    assert!(validate_acceptance(&moved, &review, &provider_receipt).is_err());
}

#[test]
fn no_receipt_is_built_unless_every_check_held_in_both_rounds() {
    let (provider, review) = golden_configs();
    for (name, break_it) in every_check() {
        for broken_round in 0..2 {
            let (mut first, mut second) = (proven(), proven());
            break_it(if broken_round == 0 {
                &mut first
            } else {
                &mut second
            });
            let error = receipts(&provider, &review, &first, &second).unwrap_err();
            assert!(
                matches!(error, Failure::Check(_)),
                "an ordinary failure: {error}"
            );
            assert!(
                error.to_string().contains(name),
                "{name} (round {broken_round}): {error}"
            );
        }
    }
}

#[test]
fn the_probe_report_must_be_exactly_the_nine_fields_as_booleans() {
    let all = json!({
        "ownerFileDenied": true, "ownerCredentialDenied": true, "ownerProcessDenied": true, "ownerDesktopDenied": true,
        "workerWriteAllowed": true, "peerFileDenied": true, "peerWriteDenied": true, "peerRegistryDenied": true,
        "peerProcessDenied": true,
    });
    assert_eq!(Denials::parse(&all.to_string()).unwrap(), proven().denials);
    let mut missing = all.clone();
    missing.as_object_mut().unwrap().remove("peerProcessDenied");
    let mut extra = all.clone();
    extra["somethingElse"] = json!(true);
    let mut not_boolean = all.clone();
    not_boolean["ownerFileDenied"] = json!("true");
    for bad in [
        missing.to_string(),
        extra.to_string(),
        not_boolean.to_string(),
        "not json".into(),
        String::new(),
    ] {
        assert!(Denials::parse(&bad).is_err(), "{bad}");
    }
    // A false is a well-formed report that proves nothing.
    let mut denied_nothing = all;
    denied_nothing["peerRegistryDenied"] = json!(false);
    let report = Denials::parse(&denied_nothing.to_string()).unwrap();
    assert_eq!(report.unproven(), ["peerRegistryDenied"]);
}

#[test]
fn a_cli_started_only_if_it_exited_zero_and_printed_a_version() {
    let output = |code: Option<i32>, stdout: &str| CliOutput {
        code,
        stdout: stdout.into(),
        stderr: String::new(),
    };
    assert!(version_started(&output(Some(0), "2.1.3 (Claude Code)\n")));
    assert!(version_started(&output(Some(0), "codex-cli 0.46.0\r\n")));
    assert!(!version_started(&output(Some(1), "2.1.3")));
    assert!(!version_started(&output(None, "2.1.3")));
    assert!(!version_started(&output(Some(0), "")));
    assert!(!version_started(&output(Some(0), "version 2.1")));
}

// ---- the protocol ----------------------------------------------------------------------------------------------

/// A scripted `Steps`: what each step answers, and what happened, in order.
struct Fake {
    root: PathBuf,
    log: Mutex<Vec<String>>,
    prepare: Result<(), Failure>,
    provider: Result<Round, Failure>,
    review: Result<Round, Failure>,
    confirm: Result<(), Failure>,
    /// Runs inside the review round, after the first round and before the receipts are written.
    in_review_round: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Fake {
    fn passing(root: &Path) -> Self {
        Fake {
            root: root.to_path_buf(),
            log: Mutex::default(),
            prepare: Ok(()),
            provider: Ok(proven()),
            review: Ok(proven()),
            confirm: Ok(()),
            in_review_round: None,
        }
    }
    fn note(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn present(root: &Path) -> [bool; 2] {
    [Purpose::Provider, Purpose::Review].map(|purpose| receipt_path(root, purpose).exists())
}

impl Steps for Fake {
    fn prepare(&self) -> BoxFuture<'_, Result<(), Failure>> {
        self.note(format!("prepare {:?}", present(&self.root)));
        Box::pin(async move { self.prepare.clone() })
    }
    fn round(&self, active: Purpose) -> BoxFuture<'_, Result<Round, Failure>> {
        self.note(format!("round {}", active.as_str()));
        if active == Purpose::Review
            && let Some(hook) = &self.in_review_round
        {
            hook();
        }
        Box::pin(async move {
            match active {
                Purpose::Provider => self.provider.clone(),
                Purpose::Review => self.review.clone(),
            }
        })
    }
    fn confirm(&self) -> BoxFuture<'_, Result<(), Failure>> {
        self.note(format!("confirm {:?}", present(&self.root)));
        Box::pin(async move { self.confirm.clone() })
    }
    fn cleanup(&self, keep_scratch: bool) -> BoxFuture<'_, ()> {
        self.note(format!("cleanup keep_scratch={keep_scratch}"));
        Box::pin(async {})
    }
}

/// Configurations whose owner data root is a real temporary folder, so the receipts have somewhere to go.
fn configs_in(root: &Path) -> (WorkerConfiguration, WorkerConfiguration) {
    let (mut provider, mut review) = golden_configs();
    provider.owner_data_root = root.to_path_buf();
    review.owner_data_root = root.to_path_buf();
    (provider, review)
}

fn seed_stale_receipts(root: &Path, provider: &WorkerConfiguration, review: &WorkerConfiguration) {
    let [one, two] = receipts(provider, review, &proven(), &proven()).unwrap();
    fs::write(receipt_path(root, Purpose::Provider), one).unwrap();
    fs::write(receipt_path(root, Purpose::Review), two).unwrap();
    fs::write(
        format!("{}.pending", receipt_path(root, Purpose::Review).display()),
        "half written",
    )
    .unwrap();
}

fn leftovers(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("acceptance"))
        .collect()
}

#[tokio::test]
async fn when_everything_holds_both_valid_receipts_are_written_and_nothing_is_left_over() {
    let root = TempDir::new().unwrap();
    let (provider, review) = configs_in(root.path());
    let fake = Fake::passing(root.path());
    run(&fake, &provider, &review).await.unwrap();
    // The provider proves itself against a live review peer first, then the review identity against the provider.
    assert_eq!(
        fake.log(),
        [
            "prepare [false, false]",
            "round provider",
            "round review",
            "confirm [true, true]",
            "cleanup keep_scratch=false"
        ]
    );
    assert_eq!(
        leftovers(root.path()),
        ["review-worker-acceptance.json", "worker-acceptance.json"]
    );
    let read = |purpose| fs::read_to_string(receipt_path(root.path(), purpose)).unwrap();
    validate_acceptance(&provider, &review, &read(Purpose::Provider)).unwrap();
    validate_acceptance(&review, &provider, &read(Purpose::Review)).unwrap();
}

#[tokio::test]
async fn an_old_receipt_is_gone_before_any_check_runs() {
    let root = TempDir::new().unwrap();
    let (provider, review) = configs_in(root.path());
    seed_stale_receipts(root.path(), &provider, &review);
    let fake = Fake::passing(root.path());
    run(&fake, &provider, &review).await.unwrap();
    assert_eq!(
        fake.log()[0],
        "prepare [false, false]",
        "stale receipts were still valid while checks ran"
    );
}

#[tokio::test]
async fn a_failed_check_writes_no_receipt_and_removes_an_old_one() {
    for (name, break_it) in every_check() {
        for broken_round in 0..2 {
            let root = TempDir::new().unwrap();
            let (provider, review) = configs_in(root.path());
            seed_stale_receipts(root.path(), &provider, &review);
            let mut fake = Fake::passing(root.path());
            let target = if broken_round == 0 {
                &mut fake.provider
            } else {
                &mut fake.review
            };
            break_it(target.as_mut().unwrap());
            let error = run(&fake, &provider, &review).await.unwrap_err();
            assert!(error.to_string().contains(name), "{name}: {error}");
            assert_eq!(
                leftovers(root.path()),
                Vec::<String>::new(),
                "{name} (round {broken_round}) left a file"
            );
            assert_eq!(fake.log().last().unwrap(), "cleanup keep_scratch=false");
            // A failure in the first round does not go on to the second.
            assert_eq!(
                fake.log()
                    .iter()
                    .filter(|line| line.starts_with("round"))
                    .count(),
                1 + broken_round
            );
        }
    }
}

#[tokio::test]
async fn a_check_that_errors_writes_no_receipt_and_removes_an_old_one() {
    for failing in ["prepare", "provider", "review"] {
        let root = TempDir::new().unwrap();
        let (provider, review) = configs_in(root.path());
        seed_stale_receipts(root.path(), &provider, &review);
        let mut fake = Fake::passing(root.path());
        let error = Failure::Check("the denial probe could not run".into());
        match failing {
            "prepare" => fake.prepare = Err(error.clone()),
            "provider" => fake.provider = Err(error.clone()),
            _ => fake.review = Err(error.clone()),
        }
        assert_eq!(
            run(&fake, &provider, &review).await.unwrap_err(),
            error,
            "{failing}"
        );
        assert_eq!(
            leftovers(root.path()),
            Vec::<String>::new(),
            "{failing} left a file"
        );
        assert_eq!(
            fake.log().last().unwrap(),
            "cleanup keep_scratch=false",
            "cleanup runs whatever happened"
        );
    }
}

#[tokio::test]
async fn the_final_paired_verification_failing_takes_the_receipts_back() {
    let root = TempDir::new().unwrap();
    let (provider, review) = configs_in(root.path());
    let mut fake = Fake::passing(root.path());
    fake.confirm = Err(Failure::Check(
        "The paired protected acceptance receipts were not accepted.".into(),
    ));
    assert!(run(&fake, &provider, &review).await.is_err());
    // They were really there while it was checked, so this is the take-back and not a receipt never written.
    assert!(
        fake.log().contains(&"confirm [true, true]".to_string()),
        "{:?}",
        fake.log()
    );
    assert_eq!(leftovers(root.path()), Vec::<String>::new());
}

#[tokio::test]
async fn an_unconfirmed_process_stop_keeps_the_scratch_folders_for_reconciliation() {
    for failing in ["provider", "review"] {
        let root = TempDir::new().unwrap();
        let (provider, review) = configs_in(root.path());
        let mut fake = Fake::passing(root.path());
        if failing == "provider" {
            fake.provider = Err(Failure::Termination);
        } else {
            fake.review = Err(Failure::Termination);
        }
        assert_eq!(
            run(&fake, &provider, &review).await.unwrap_err(),
            Failure::Termination
        );
        assert_eq!(
            fake.log().last().unwrap(),
            "cleanup keep_scratch=true",
            "{failing}"
        );
        assert_eq!(
            leftovers(root.path()),
            Vec::<String>::new(),
            "still no receipt"
        );
    }
}

#[tokio::test]
async fn an_old_receipt_that_cannot_be_removed_stops_the_run_before_anything_starts() {
    let root = TempDir::new().unwrap();
    let (provider, review) = configs_in(root.path());
    // A folder cannot be deleted as a file, so this stands for a receipt that is locked or protected.
    fs::create_dir(receipt_path(root.path(), Purpose::Provider)).unwrap();
    let fake = Fake::passing(root.path());
    let error = run(&fake, &provider, &review).await.unwrap_err();
    assert!(matches!(error, Failure::Check(_)), "{error}");
    assert!(
        !fake
            .log()
            .iter()
            .any(|line| line.starts_with("prepare") || line.starts_with("round")),
        "checks ran with a receipt in place: {:?}",
        fake.log()
    );
}

#[tokio::test]
async fn a_receipt_that_cannot_be_written_leaves_no_receipt_at_all() {
    let root = TempDir::new().unwrap();
    let (provider, review) = configs_in(root.path());
    let mut fake = Fake::passing(root.path());
    // After the start-up clearing and before the writes, the review receipt's place becomes unwritable.
    let blocked = receipt_path(root.path(), Purpose::Review);
    fake.in_review_round = Some(Box::new(move || fs::create_dir(&blocked).unwrap()));
    assert!(run(&fake, &provider, &review).await.is_err());
    assert!(
        !receipt_path(root.path(), Purpose::Provider).exists(),
        "the provider receipt stayed behind without its peer's"
    );
    assert!(
        !fake.log().iter().any(|line| line.starts_with("confirm")),
        "{:?}",
        fake.log()
    );
}

// ---- scratch folders -------------------------------------------------------------------------------------------

#[test]
fn only_this_runs_own_scratch_folder_inside_the_worker_root_may_be_deleted() {
    let base = TempDir::new().unwrap();
    let root = base.path().join("work");
    let id = "0f6e4c1a-6b0e-4c53-9e0e-0d8a3f4d9a11";
    let scratch = root.join(format!("acceptance-{id}"));
    fs::create_dir_all(&scratch).unwrap();
    fs::create_dir_all(root.join("accounts")).unwrap();
    fs::create_dir_all(base.path().join(format!("acceptance-{id}"))).unwrap();
    assert!(removable_scratch(&root, &scratch, id));
    assert!(
        !removable_scratch(&root, &root, id),
        "the worker root itself"
    );
    assert!(
        !removable_scratch(&root, &root.join("accounts"), id),
        "another folder in the root"
    );
    assert!(
        !removable_scratch(
            &root,
            &root.join(format!("acceptance-{}", "x".repeat(id.len()))),
            id
        ),
        "another run's folder"
    );
    assert!(
        !removable_scratch(&root, &base.path().join(format!("acceptance-{id}")), id),
        "the right name outside the worker root"
    );
    assert!(
        !removable_scratch(&root, &root.join("..").join(format!("acceptance-{id}")), id),
        "an escape through .."
    );
    assert!(!removable_scratch(&root, &root.join("missing"), id));
}

// ---- the real probe scripts ------------------------------------------------------------------------------------

fn which_node() -> PathBuf {
    let output = spawn::command("where").arg("node").output().unwrap();
    PathBuf::from(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .trim(),
    )
}

#[test]
fn the_fixed_programs_fit_the_logon_launch_frame() {
    let node = which_node();
    for args in [
        probe_args(&json!({ "marker": "C:\\x" }).to_string()),
        holder_args(
            &json!({ "marker": "C:\\x", "registry": "Software\\Tjommie\\Acceptance\\y" })
                .to_string(),
        ),
    ] {
        assert_eq!(args[0], "-e");
        command_line(node.to_str().unwrap(), &args).unwrap();
        launch_frame(node.to_str().unwrap(), &args).unwrap();
    }
}

/// Node must accept both wrapper programs and PowerShell both embedded scripts, so a typo cannot hide until the
/// accounts exist. (The holder is only parsed: running it would make a registry key.)
#[test]
fn the_wrapper_programs_and_scripts_parse() {
    let node = which_node();
    for args in [probe_args("{}"), holder_args("{}")] {
        let mut check = spawn::command(&node);
        check
            .arg("--check")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = check.spawn().unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), args[1].as_bytes()).unwrap();
        let done = child.wait_with_output().unwrap();
        assert!(
            done.status.success(),
            "node rejected a wrapper: {}",
            String::from_utf8_lossy(&done.stderr)
        );
        // The PowerShell text is the one string literal in the wrapper: JSON.parse of it gives the script back.
        let literal = args[1]
            .rsplit_once("'-Command',")
            .unwrap()
            .1
            .split_once("];const p=")
            .unwrap()
            .0;
        let script: String = serde_json::from_str(literal).unwrap();
        let parsed = spawn::command("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$e=$null;$t=$null;[void][Management.Automation.Language.Parser]::ParseInput($env:TJ_SCRIPT,[ref]$t,[ref]$e);if($e.Count){$e|ForEach-Object{$_.Message};exit 1}",
            ])
            .env("TJ_SCRIPT", &script)
            .output()
            .unwrap();
        assert!(
            parsed.status.success(),
            "PowerShell rejected a script: {}",
            String::from_utf8_lossy(&parsed.stdout)
        );
    }
}

/// The real probe, run as the account that owns every synthetic object: nothing is denied it, so the report must say
/// so and the check must not hold. This runs the actual node, PowerShell and C# of the probe.
#[test]
fn the_real_probe_run_as_the_owner_itself_proves_nothing() {
    let scratch = TempDir::new().unwrap();
    let marker = scratch.path().join("owner-marker.txt");
    let peer_marker = scratch.path().join("peer-marker.txt");
    fs::write(&marker, "synthetic").unwrap();
    fs::write(&peer_marker, "synthetic").unwrap();
    let me = daemon::win::logon::current_user_sid().unwrap();
    let payload = json!({
        "marker": marker, "target": "Tjommie/ffffffffffffffffffffffff/acceptance/never-written",
        "ownerPid": std::process::id(), "peerPid": std::process::id(), "peerMarker": peer_marker,
        "peerRegistry": format!("{me}\\Software\\Tjommie\\Acceptance\\never-created"),
        "write": scratch.path().join("write.txt"),
    });
    let mut command = spawn::command(which_node());
    command
        .args(probe_args(&payload.to_string()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let done = command.spawn().unwrap().wait_with_output().unwrap();
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let report = Denials::parse(std::str::from_utf8(&done.stdout).unwrap()).unwrap();
    // Its own file and process are open to it, its own folder is writable: the checks of what must be denied failed.
    assert!(!report.owner_file_denied && !report.owner_process_denied && !report.peer_file_denied);
    assert!(
        !report.peer_write_denied && !report.peer_process_denied && !report.peer_registry_denied
    );
    assert!(report.worker_write_allowed);
    assert!(!report.unproven().is_empty());
    assert!(
        scratch.path().join("write.txt").exists(),
        "the probe writes its own folder"
    );
    // And so a round that reported this can never become a receipt.
    let round = Round {
        denials: report,
        ..proven()
    };
    let (provider, review) = golden_configs();
    assert!(receipts(&provider, &review, &round, &proven()).is_err());
}

// ---- admin, CI-runner only -------------------------------------------------------------------------------------

/// "Both CLIs start", mocked: no Claude or Codex is installed, signed in or asked anything. `Err` makes the CLI check fail.
struct Cli(Result<(), Failure>);

impl CliProbe for Cli {
    fn started<'a>(
        &'a self,
        _active: Purpose,
        _cli: protocol::model::Provider,
        _account: &'a Path,
    ) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move { self.0.clone() })
    }
}

/// Backdates both configurations' `provisionedAt` (kept equal, as a pair must be): readiness refuses a boot that
/// predates provisioning, and a CI runner provisions right after it boots.
fn backdate(owner_root: &Path) {
    for file in ["worker.json", "review-worker.json"] {
        let path = owner_root.join(file);
        let mut config: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        config["provisionedAt"] = json!("2000-01-01T00:00:00.000Z");
        fs::write(&path, config.to_string()).unwrap();
    }
}

/// Admin, CI-runner only (`TJOMMIE_RS_ADMIN_TESTS=1`): the real check against two disposable accounts, with the CLI
/// check mocked. A failing run writes no receipt and removes an old one; a passing run writes two receipts the daemon's
/// own readiness check accepts, and leaves no scratch folder, marker or synthetic credential. Cleanup is by the
/// disposable root's ns8: its two accounts, its two credential targets and its folders.
#[tokio::test]
#[ignore = "admin: needs elevation; run with TJOMMIE_RS_ADMIN_TESTS=1 cargo test -- --ignored"]
async fn admin_acceptance_checks_two_real_worker_identities() {
    if std::env::var("TJOMMIE_RS_ADMIN_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set TJOMMIE_RS_ADMIN_TESTS=1 on a disposable machine");
        return;
    }
    let folder = TempDir::new().unwrap();
    let owner_root = folder.path().join("owner");
    assert!(
        !cred::is_production_root(&owner_root),
        "refusing the production root"
    );
    let ns = cred::Namespace::new(&owner_root).unwrap();
    let names = [
        daemon::win::worker::worker_name(&ns, Purpose::Provider),
        daemon::win::worker::worker_name(&ns, Purpose::Review),
    ];
    let program_data = PathBuf::from(std::env::var_os("ProgramData").unwrap());
    let base = program_data.join(format!(r"Tjommie\Worker-{}", ns.short()));
    for name in &names {
        assert!(
            logon::account_sid(name).unwrap().is_none(),
            "{name} already exists"
        );
    }
    let (root, runtime) = (owner_root.clone(), base.join("runtime"));
    let run = tokio::spawn(async move {
        logon::provision(&root).unwrap();
        fs::copy(which_node(), runtime.join("node.exe")).unwrap();
        backdate(&root);
        let (provider, review) = (
            WorkerConfiguration::load(&root.join("worker.json"), Purpose::Provider).unwrap(),
            WorkerConfiguration::load(&root.join("review-worker.json"), Purpose::Review).unwrap(),
        );
        let exe = std::env::current_exe().unwrap();
        let exe_dir = exe.parent().unwrap();
        let native = || -> Arc<dyn WorkerNative> { Arc::new(WindowsNative) };
        let receipts_present = || {
            [Purpose::Provider, Purpose::Review]
                .map(|purpose| receipt_path(&root, purpose).exists())
        };

        // 1. A failing check: no receipt, and a receipt from before is gone.
        let [stale, other] = receipts(&provider, &review, &proven(), &proven()).unwrap();
        fs::write(receipt_path(&root, Purpose::Provider), stale).unwrap();
        fs::write(receipt_path(&root, Purpose::Review), other).unwrap();
        let failed = accept_with(
            &root,
            exe_dir,
            native(),
            Seams {
                cli: Some(Arc::new(Cli(Err(Failure::Check(
                    "a CLI could not start".into(),
                ))))),
                ..Seams::default()
            },
        )
        .await;
        assert!(failed.is_err(), "{failed:?}");
        assert_eq!(
            receipts_present(),
            [false, false],
            "a failed run left a receipt"
        );

        // 2. Everything holds: two receipts the daemon's own readiness check accepts.
        let done = accept_with(
            &root,
            exe_dir,
            native(),
            Seams {
                cli: Some(Arc::new(Cli(Ok(())))),
                ..Seams::default()
            },
        )
        .await;
        assert!(done.is_ok(), "{done:?}");
        assert_eq!(receipts_present(), [true, true]);
        daemon::win::worker::ready(&root.join("worker.json"), Purpose::Provider).unwrap();
        daemon::win::worker::ready(&root.join("review-worker.json"), Purpose::Review).unwrap();
        for config in [&provider, &review] {
            assert_eq!(
                fs::read_dir(&config.worker_root)
                    .unwrap()
                    .filter(|entry| entry
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("acceptance-"))
                    .count(),
                0,
                "a scratch folder was left in {}",
                config.worker_root.display()
            );
        }
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("acceptance-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    });
    let result = run.await;
    // Cleanup by ns8: the two accounts, the two credential targets, and the folders under ProgramData\Tjommie.
    for name in &names {
        let _ = logon::delete_account(name);
    }
    let _ = cred::delete(&ns.isolated_worker_target());
    let _ = cred::delete(&ns.isolated_review_target());
    if base.starts_with(program_data.join("Tjommie")) {
        let _ = fs::remove_dir_all(&base);
    }
    for name in &names {
        assert!(
            logon::account_sid(name).unwrap().is_none(),
            "{name} was not removed"
        );
    }
    if let Err(error) = result {
        std::panic::resume_unwind(error.into_panic());
    }
}
