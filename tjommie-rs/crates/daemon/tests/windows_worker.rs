//! T2.3 port of `windows-worker.test.ts` (the startup gate) plus worker identities and provisioning.
//! Not ported yet: the review-lease cases, which test the launch orchestration's scheduling (see the T2.3 notes).
#![cfg(windows)]

use daemon::win::cred::{self, Namespace};
use daemon::win::logon;
use daemon::win::worker::{
    Purpose, WorkerConfiguration, ready, required, validate_pair, worker_name,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use windows::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW};
use windows::core::HSTRING;

#[test]
fn worker_targets_and_names_derive_from_the_root_like_provision_worker_ps1() {
    let ns = Namespace::new(Path::new(r"C:\Users\Fixture\AppData\Local\Tjommie-rs")).unwrap();
    assert_eq!(
        ns.isolated_worker_target(),
        "Tjommie/c7b7c5a986b63824c0803169/isolated-worker"
    );
    assert_eq!(
        ns.isolated_review_target(),
        "Tjommie/c7b7c5a986b63824c0803169/isolated-review"
    );
    assert_eq!(worker_name(&ns, Purpose::Provider), "TjWorker-c7b7c5a9");
    assert_eq!(worker_name(&ns, Purpose::Review), "TjReview-c7b7c5a9");
}

#[test]
fn keeps_worker_mode_required_if_configuration_disappears_but_its_protection_marker_remains() {
    let root = TempDir::new().unwrap();
    let configuration = root.path().join("review-worker.json");
    assert!(!required(&configuration));
    fs::write(root.path().join("vault-protection-required"), "1").unwrap();
    assert!(required(&configuration));
    assert!(ready(&configuration, Purpose::Review).is_err());
}

#[test]
fn rejects_an_acceptance_receipt_that_does_not_match_the_exact_protected_configuration() {
    let root = TempDir::new().unwrap();
    let configuration = root.path().join("review-worker.json");
    for folder in ["worker", "runtime"] {
        fs::create_dir(root.path().join(folder)).unwrap();
    }
    let config = json!({
        "version": 2, "purpose": "review", "ownerSid": "S-1-5-21-1-2-3-1001", "workerSid": "S-1-5-21-1-2-3-1002",
        "peerSid": "S-1-5-21-1-2-3-1003", "workerName": "TjReview-12345678", "workerRoot": root.path().join("worker"),
        "peerRoot": root.path().join("provider"), "runtimeRoot": root.path().join("runtime"), "ownerDataRoot": root.path(),
        "provisionedAt": "2026-11-09T12:00:00.000Z", "credentialTarget": format!("Tjommie/{}/isolated-review", "a".repeat(24)),
    });
    fs::write(&configuration, config.to_string()).unwrap();
    fs::write(
        root.path().join("worker-acceptance.json"),
        json!({ "version": 1, "configurationHash": "0".repeat(64) }).to_string(),
    )
    .unwrap();
    assert!(required(&configuration));
    assert!(ready(&configuration, Purpose::Review).is_err());
}

fn user_name(target: &str) -> Option<String> {
    let mut pointer: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: freed once with CredFree after copying the user name.
    unsafe {
        CredReadW(
            &HSTRING::from(target),
            CRED_TYPE_GENERIC,
            None,
            &mut pointer,
        )
        .ok()?;
        let name = (*pointer).UserName.to_string().ok();
        CredFree(pointer as *const _);
        name
    }
}

/// Admin, CI-runner only (`TJOMMIE_RS_ADMIN_TESTS=1`): provisions both identities for a disposable root, checks
/// what was made, and removes exactly that root's ns8 accounts, targets and folders.
#[test]
fn admin_provisions_both_identities_for_a_disposable_root() {
    if std::env::var("TJOMMIE_RS_ADMIN_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set TJOMMIE_RS_ADMIN_TESTS=1 on a disposable machine");
        return;
    }
    let root = TempDir::new().unwrap();
    let owner_root = root.path().join("owner");
    let production = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Tjommie");
    let ns = Namespace::new(&owner_root).unwrap();
    assert_ne!(
        ns.short(),
        Namespace::new(&production).unwrap().short(),
        "refusing the production root"
    );
    let names = [
        worker_name(&ns, Purpose::Provider),
        worker_name(&ns, Purpose::Review),
    ];
    let base = PathBuf::from(std::env::var_os("ProgramData").unwrap())
        .join(format!(r"Tjommie\Worker-{}", ns.short()));
    let result = std::panic::catch_unwind(|| {
        logon::provision(&owner_root).unwrap();
        let provider =
            WorkerConfiguration::load(&owner_root.join("worker.json"), Purpose::Provider).unwrap();
        let review =
            WorkerConfiguration::load(&owner_root.join("review-worker.json"), Purpose::Review)
                .unwrap();
        validate_pair(&provider, &review).unwrap();
        for (config, name) in [(&provider, &names[0]), (&review, &names[1])] {
            assert_eq!(&config.worker_name, name);
            assert_eq!(
                logon::account_sid(name).unwrap().as_deref(),
                Some(config.worker_sid.as_str())
            );
            let saved: serde_json::Value =
                serde_json::from_str(&cred::read(&config.credential_target).unwrap().unwrap())
                    .unwrap();
            assert_eq!(saved["username"], json!(name));
            assert_eq!(
                user_name(&config.credential_target).as_deref(),
                Some("Tjommie isolated worker")
            );
        }
        assert!(owner_root.join("vault-protection-required").is_file());
        assert!(
            logon::provision(&owner_root).is_err(),
            "provisioning twice must refuse"
        );
    });
    for name in &names {
        logon::delete_account(name).unwrap();
    }
    cred::delete(&ns.isolated_worker_target()).unwrap();
    cred::delete(&ns.isolated_review_target()).unwrap();
    if base.starts_with(PathBuf::from(std::env::var_os("ProgramData").unwrap()).join("Tjommie")) {
        let _ = fs::remove_dir_all(&base);
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
