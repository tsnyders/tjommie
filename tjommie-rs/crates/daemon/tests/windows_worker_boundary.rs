//! The review-lease cases of `windows-worker.test.ts` and "refuses review launches outside policy" from
//! `windows-worker-pair.test.ts`. The boundary runs with a stubbed policy observer (the TypeScript tests spy on it)
//! and a native layer that refuses every call, so no process can start.
#![cfg(windows)]

use daemon::cli::BoxFuture;
use daemon::win::boundary::{
    BoundaryError, BoundaryOptions, LaunchFailure, NativeLaunch, Observer, PolicyReceipt, Receipt,
    WorkerBoundary, WorkerNative,
};
use daemon::win::policy_observer::WORKER_POLICY_OBSERVER;
use daemon::win::worker::{Purpose, WorkerConfiguration};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::Notify;

/// No native process is permitted in these fixtures.
struct Refuse;

impl WorkerNative for Refuse {
    fn verify<'a>(
        &'a self,
        _: &'a WorkerConfiguration,
        _: &'a [Receipt],
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("no native call is permitted".into()) })
    }
    fn read_credential<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
        panic!("no credential may be read")
    }
    fn grant_directory<'a>(
        &'a self,
        _: &'a WorkerConfiguration,
        _: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        panic!("no ACL may change")
    }
    fn launch<'a>(
        &'a self,
        _: &'a WorkerConfiguration,
        _: String,
        _: &'a Path,
        _: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Box<dyn NativeLaunch>, LaunchFailure>> {
        panic!("no process may start")
    }
}

fn receipt() -> PolicyReceipt {
    PolicyReceipt {
        worker_sid: "S-1-5-21-1-2-3-1002".into(),
        provider_sid: "S-1-5-21-1-2-3-1003".into(),
        configuration_sha256: "d".repeat(64),
        observer_sha256: "a".repeat(64),
        policy_sha256: "b".repeat(64),
    }
}

/// Observations answer from `script` in order, then repeat the last; the call count comes back too.
fn scripted(script: Vec<Result<PolicyReceipt, BoundaryError>>) -> (Observer, Arc<AtomicUsize>) {
    let (calls, script) = (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(script)));
    let counted = calls.clone();
    let observer: Observer = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        let mut script = script.lock().unwrap();
        let next = if script.len() > 1 {
            script.remove(0)
        } else {
            script[0].clone()
        };
        Box::pin(async move { next })
    });
    (observer, calls)
}

fn boundary(
    configuration_path: PathBuf,
    purpose: Purpose,
    daemon_dir: PathBuf,
    observer: Option<Observer>,
) -> WorkerBoundary {
    WorkerBoundary::new(BoundaryOptions {
        configuration_path,
        purpose,
        daemon_dir,
        exclusive: Arc::new(|| false),
        native: Arc::new(Refuse),
        observer,
    })
}

fn review(observer: Option<Observer>) -> (TempDir, WorkerBoundary) {
    let root = TempDir::new().unwrap();
    let worker = boundary(
        root.path().join("review-worker.json"),
        Purpose::Review,
        root.path().join("daemon"),
        observer,
    );
    (root, worker)
}

async fn launch(boundary: &WorkerBoundary) -> Result<(), BoundaryError> {
    boundary
        .spawn("unused", &[], Path::new("unused"), &[])
        .await
        .map(|_| ())
}

#[test]
fn the_policy_observer_is_the_typescript_one_byte_for_byte() {
    let file = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/daemon/src/worker-policy-observer.ts"),
    )
    .unwrap();
    // JavaScript reads template literals with LF line ends, raw ones included, whatever the checkout uses.
    let source = file
        .lines()
        .collect::<Vec<_>>()
        .join(&char::from(10).to_string());
    assert!(source.contains(&format!("String.raw`{WORKER_POLICY_OBSERVER}`;")));
}

#[tokio::test]
async fn rejects_policy_observation_without_an_exclusive_review_scope() {
    let (_root, worker) = review(None);
    assert_eq!(
        worker.observe_review_policy().await,
        Err(BoundaryError::Busy)
    );
}

#[tokio::test]
async fn holds_unrelated_launches_out_while_permitting_the_trusted_scope_and_releases_after_matching_observations()
 {
    let (observer, calls) = scripted(vec![Ok(receipt())]);
    let (_root, worker) = review(Some(observer));
    let (entered, finish) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let result = {
        let (scoped, entered, finish) = (worker.clone(), entered.clone(), finish.clone());
        tokio::spawn(async move {
            let inner = scoped.clone();
            scoped
                .with_review_policy(|proof| async move {
                    assert_eq!(proof, receipt());
                    // It reaches configuration, proving this scope passed the Busy gate; no process starts.
                    assert_eq!(launch(&inner).await, Err(BoundaryError::Unavailable));
                    entered.notify_one();
                    finish.notified().await;
                    Ok("reviewed")
                })
                .await
        })
    };
    entered.notified().await;
    assert_eq!(launch(&worker).await, Err(BoundaryError::Busy));
    assert_eq!(
        worker.with_review_policy(|_| async { Ok("other") }).await,
        Err(BoundaryError::Busy)
    );
    finish.notify_one();
    assert_eq!(result.await.unwrap(), Ok("reviewed"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        worker.with_review_policy(|_| async { Ok("next") }).await,
        Ok("next")
    );
}

#[tokio::test]
async fn retains_quiescence_and_invalidates_future_work_after_an_action_reports_unknown_termination()
 {
    let (observer, _) = scripted(vec![Ok(receipt())]);
    let (_root, worker) = review(Some(observer));
    let unknown = worker
        .with_review_policy(|_| async { Err::<(), _>(BoundaryError::Termination) })
        .await;
    assert_eq!(unknown, Err(BoundaryError::Termination));
    assert_eq!(launch(&worker).await, Err(BoundaryError::Busy));
    assert_eq!(
        worker.with_review_policy(|_| async { Ok("unsafe") }).await,
        Err(BoundaryError::Busy)
    );
    assert!(
        worker
            .unavailable_reason()
            .await
            .unwrap()
            .contains("verified Windows worker account")
    );
}

#[tokio::test]
async fn retains_quiescence_when_the_post_observer_loses_its_process_tree_proof() {
    let (observer, _) = scripted(vec![Ok(receipt()), Err(BoundaryError::Termination)]);
    let (_root, worker) = review(Some(observer));
    assert_eq!(
        worker
            .with_review_policy(|_| async { Ok("not accepted") })
            .await,
        Err(BoundaryError::Termination)
    );
    assert_eq!(
        worker.with_review_policy(|_| async { Ok("unsafe") }).await,
        Err(BoundaryError::Busy)
    );
}

#[tokio::test]
async fn refuses_changed_worker_policy_and_permits_a_later_fresh_attempt_after_a_settled_failure() {
    let changed = PolicyReceipt {
        policy_sha256: "c".repeat(64),
        ..receipt()
    };
    let (observer, _) = scripted(vec![Ok(receipt()), Ok(changed), Ok(receipt())]);
    let (_root, worker) = review(Some(observer));
    assert_eq!(
        worker.with_review_policy(|_| async { Ok("stale") }).await,
        Err(BoundaryError::PolicyChanged)
    );
    let ordinary = || BoundaryError::Other("ordinary provider failure".into());
    assert_eq!(
        worker
            .with_review_policy(|_| async move { Err::<(), _>(ordinary()) })
            .await,
        Err(ordinary())
    );
    assert_eq!(
        worker.with_review_policy(|_| async { Ok("fresh") }).await,
        Ok("fresh")
    );
}

#[tokio::test]
async fn keeps_worker_mode_required_and_unavailable_when_only_the_protection_marker_remains() {
    let (root, worker) = review(None);
    std::fs::write(root.path().join("vault-protection-required"), "1").unwrap();
    assert!(worker.required());
    assert!(worker.unavailable_reason().await.is_some());
    assert!(launch(&worker).await.is_err());
}

/// `windows-worker-pair.test.ts`: a review launch needs the review scope, and the provider never observes policy.
#[tokio::test]
async fn refuses_review_launches_outside_policy_and_never_permits_provider_policy_observations() {
    let root = TempDir::new().unwrap();
    let owner = root.path().join("owner");
    for folder in ["owner", "runtime", "provider", "review", "daemon"] {
        std::fs::create_dir(root.path().join(folder)).unwrap();
    }
    for (purpose, worker, peer) in [("provider", "1001", "1002"), ("review", "1002", "1001")] {
        let provider = purpose == "provider";
        let config = json!({
            "version": 2, "purpose": purpose, "ownerSid": "S-1-5-21-1-2-3-1000",
            "workerSid": format!("S-1-5-21-1-2-3-{worker}"), "peerSid": format!("S-1-5-21-1-2-3-{peer}"),
            "workerName": if provider { "TjWorker-12345678" } else { "TjReview-12345678" },
            "workerRoot": root.path().join(purpose), "peerRoot": root.path().join(if provider { "review" } else { "provider" }),
            "runtimeRoot": root.path().join("runtime"), "ownerDataRoot": owner, "provisionedAt": "2026-11-03T00:00:00.000Z",
            "credentialTarget": format!("Tjommie/{}/isolated-{}", "a".repeat(24), if provider { "worker" } else { "review" }),
        });
        let file = if provider {
            "worker.json"
        } else {
            "review-worker.json"
        };
        std::fs::write(owner.join(file), config.to_string()).unwrap();
    }
    let daemon = root.path().join("daemon");
    let provider = boundary(
        owner.join("worker.json"),
        Purpose::Provider,
        daemon.clone(),
        None,
    );
    let reviewer = boundary(
        owner.join("review-worker.json"),
        Purpose::Review,
        daemon,
        None,
    );
    let cwd = root.path().join("review");
    let outside = reviewer
        .spawn("unused.exe", &[], &cwd, &[])
        .await
        .map(|_| ());
    assert_eq!(outside, Err(BoundaryError::ReviewIdentityUnavailable));
    assert_eq!(
        provider
            .with_review_policy(|_| async { Ok("unsafe") })
            .await,
        Err(BoundaryError::ReviewIdentityUnavailable)
    );
}
