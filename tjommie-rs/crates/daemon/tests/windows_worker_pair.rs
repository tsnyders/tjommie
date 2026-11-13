//! T2.3 port of `windows-worker-pair.test.ts`: identity pairing, acceptance, and which folders a worker may be
//! granted. The native grant runs only on temp folders with SIDs no account has.
//! Not ported here: "refuses review launches outside policy", which belongs to the launch orchestration.
#![cfg(windows)]

mod common;

use daemon::spawn;
use daemon::win::logon;
use daemon::win::worker::{
    Purpose, WorkerConfiguration, grant_target, validate_acceptance, validate_pair,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    provider: WorkerConfiguration,
    review: WorkerConfiguration,
}

fn config(root: &Path, purpose: Purpose) -> WorkerConfiguration {
    let (worker, peer) = match purpose {
        Purpose::Provider => ("provider", "review"),
        Purpose::Review => ("review", "provider"),
    };
    let sid = |purpose| {
        if purpose == "provider" {
            "S-1-5-21-1-2-3-1001"
        } else {
            "S-1-5-21-1-2-3-1002"
        }
    };
    WorkerConfiguration::parse(
        &json!({
            "version": 2, "purpose": worker, "ownerSid": "S-1-5-21-1-2-3-1000", "workerSid": sid(worker), "peerSid": sid(peer),
            "workerName": if worker == "provider" { "TjWorker-12345678" } else { "TjReview-12345678" },
            "workerRoot": root.join(worker), "peerRoot": root.join(peer), "runtimeRoot": root.join("runtime"),
            "ownerDataRoot": root.join("owner"), "provisionedAt": "2026-11-03T00:00:00.000Z",
            "credentialTarget": format!("Tjommie/{}/isolated-{}", "a".repeat(24), if worker == "provider" { "worker" } else { "review" }),
        })
        .to_string(),
    )
    .unwrap()
}

fn fixture() -> Fixture {
    let root = TempDir::new().unwrap();
    // "daemon" stands for the daemon's own folder, which is never granted and must exist to be compared.
    for folder in ["owner", "runtime", "provider", "review", "daemon"] {
        fs::create_dir(root.path().join(folder)).unwrap();
    }
    let (provider, review) = (
        config(root.path(), Purpose::Provider),
        config(root.path(), Purpose::Review),
    );
    fs::write(
        root.path().join("owner/worker.json"),
        serde_json::to_string(&provider).unwrap(),
    )
    .unwrap();
    fs::write(
        root.path().join("owner/review-worker.json"),
        serde_json::to_string(&review).unwrap(),
    )
    .unwrap();
    Fixture {
        root,
        provider,
        review,
    }
}

fn junction(link: &Path, target: &Path) {
    let output = spawn::command("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mklink /J: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `windows-worker.ts` `workerConfigurationHash` for this configuration, from the TypeScript module.
#[test]
fn configuration_hash_matches_typescript() {
    let root = PathBuf::from(r"C:\fixture");
    assert_eq!(
        config(&root, Purpose::Provider).hash(),
        "3793fcd523230963ff76cad9c39082c8a37cafff26283d1e9344991b7b978784"
    );
}

#[test]
fn configurations_are_strict() {
    let f = fixture();
    let good = serde_json::to_value(&f.provider).unwrap();
    let mut extra = good.clone();
    extra["extra"] = json!(1);
    for (field, value) in [
        ("version", json!(1)),
        ("ownerSid", json!("S-1-5-32-544")),
        ("workerName", json!("TjWorker-1234567")),
        ("provisionedAt", json!("yesterday")),
        ("credentialTarget", json!("Tjommie/x/isolated-worker")),
        // Review MINOR 3: byte 24 falls inside "é"; this must be refused, not panic.
        (
            "credentialTarget",
            json!(format!("Tjommie/{}é/isolated-worker", "a".repeat(23))),
        ),
        ("workerRoot", json!("")),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(
            WorkerConfiguration::parse(&bad.to_string()).is_err(),
            "{field}"
        );
    }
    assert!(WorkerConfiguration::parse(&extra.to_string()).is_err());
}

#[test]
fn binds_both_purpose_sid_and_canonical_root_pairs_and_refuses_aliasing() {
    let f = fixture();
    validate_pair(&f.provider, &f.review).unwrap();
    let mut changes: Vec<WorkerConfiguration> = vec![];
    for change in [
        json!({ "workerSid": f.provider.worker_sid }),
        json!({ "purpose": "provider", "workerName": "TjWorker-12345678", "credentialTarget": f.provider.credential_target }),
        json!({ "workerRoot": f.provider.worker_root }),
        json!({ "workerRoot": f.provider.worker_root.join("nested") }),
        json!({ "ownerSid": f.provider.worker_sid }),
    ] {
        let mut value = serde_json::to_value(&f.review).unwrap();
        for (key, patch) in change.as_object().unwrap() {
            value[key] = patch.clone();
        }
        changes.push(WorkerConfiguration::parse(&value.to_string()).unwrap());
    }
    for changed in &changes {
        assert!(validate_pair(&f.provider, changed).is_err(), "{changed:?}");
    }
    let owner = f.root.path().join("owner");
    assert_eq!(
        WorkerConfiguration::load(&owner.join("worker.json"), Purpose::Provider).unwrap(),
        f.provider
    );
    assert_eq!(
        WorkerConfiguration::load(&owner.join("review-worker.json"), Purpose::Review).unwrap(),
        f.review
    );
    let mut aliased = serde_json::to_value(&f.review).unwrap();
    aliased["purpose"] = json!("provider");
    fs::write(owner.join("review-worker.json"), aliased.to_string()).unwrap();
    assert!(WorkerConfiguration::load(&owner.join("worker.json"), Purpose::Provider).is_err());
}

#[test]
fn never_grants_either_peer_profile_including_through_a_junction_or_ancestor() {
    let f = fixture();
    let trusted = f.root.path().join("daemon");
    let alias = f.provider.worker_root.join("peer-link");
    junction(&alias, &f.review.worker_root);
    for path in [
        &f.review.worker_root,
        &alias,
        &f.root.path().to_path_buf(),
        &f.provider.runtime_root,
        &f.provider.owner_data_root,
    ] {
        assert!(
            grant_target(&f.provider, &trusted, path).is_err(),
            "{}",
            path.display()
        );
    }
    for path in [
        &f.provider.worker_root,
        &f.root.path().to_path_buf(),
        &f.review.runtime_root,
        &f.review.owner_data_root,
    ] {
        assert!(
            grant_target(&f.review, &trusted, path).is_err(),
            "{}",
            path.display()
        );
    }
}

#[test]
fn never_grants_owner_metadata_credentials_attachments_or_trusted_executable_descendants() {
    let f = fixture();
    let daemon = f.root.path().join("daemon");
    let mut paths = vec![
        daemon.clone(),
        daemon.join("scripts"),
        f.provider.runtime_root.join("claude"),
    ];
    for name in [
        "data",
        "accounts",
        "logs",
        "shared-files",
        "worker-receipts",
        "browser-profiles",
        "bots",
        "workspaces",
        "workspaces/.owners",
        "workspaces/.empty-hooks",
    ] {
        paths.push(f.provider.owner_data_root.join(name));
    }
    for path in &paths {
        fs::create_dir_all(path).unwrap();
        assert!(
            grant_target(&f.provider, &daemon, path).is_err(),
            "{}",
            path.display()
        );
    }
    let bot = f.provider.owner_data_root.join("bots").join("bot-1");
    fs::create_dir(&bot).unwrap();
    for (name, target) in [
        ("data-link", &paths[3]),
        ("runtime-link", &f.provider.runtime_root),
        ("daemon-link", &daemon),
    ] {
        let alias = bot.join(name);
        junction(&alias, target);
        assert!(
            grant_target(&f.provider, &daemon, &alias).is_err(),
            "{name}"
        );
    }
}

/// Review MAJOR 1: a loopback-share spelling of the peer's root or the owner's metadata is the same folder.
#[test]
fn never_grants_through_a_loopback_share_alias() {
    let f = fixture();
    let daemon = f.root.path().join("daemon");
    let metadata = f.provider.owner_data_root.join("data");
    fs::create_dir_all(&metadata).unwrap();
    for path in [&f.review.worker_root, &metadata, &f.provider.runtime_root] {
        let text = path.to_str().unwrap();
        let alias = PathBuf::from(format!(r"\\localhost\{}$\{}", &text[..1], &text[3..]));
        assert!(
            grant_target(&f.provider, &daemon, &alias).is_err(),
            "{}",
            alias.display()
        );
    }
}

#[test]
fn refuses_owner_metadata_reached_through_an_aliased_managed_root() {
    let f = fixture();
    let metadata = f.provider.owner_data_root.join("data");
    fs::create_dir_all(metadata.join("looks-like-a-bot")).unwrap();
    junction(&f.provider.owner_data_root.join("bots"), &metadata);
    let path = f.provider.owner_data_root.join("bots/looks-like-a-bot");
    assert!(grant_target(&f.provider, &f.root.path().join("daemon"), &path).is_err());
}

#[test]
fn preserves_assigned_bot_and_task_folders_and_explicit_external_projects_without_granting_their_parents()
 {
    let f = fixture();
    let daemon = f.root.path().join("daemon");
    let paths = [
        f.provider.worker_root.clone(),
        f.provider.owner_data_root.join("bots/bot-1/sessions"),
        f.provider.owner_data_root.join("workspaces/task-1"),
        f.root.path().join("assigned-project"),
    ];
    for path in &paths {
        fs::create_dir_all(path).unwrap();
        assert_eq!(
            grant_target(&f.provider, &daemon, path).unwrap(),
            fs::canonicalize(path).unwrap()
        );
    }
    assert!(grant_target(&f.review, &daemon, &paths[2]).is_err());
}

#[test]
fn requires_both_purpose_sid_configuration_hashes_and_every_actual_peer_denial_in_acceptance() {
    let f = fixture();
    let accepted = json!({
        "version": 2, "purpose": "provider", "workerSid": f.provider.worker_sid, "peerSid": f.review.worker_sid,
        "configurationHash": f.provider.hash(), "peerConfigurationHash": f.review.hash(),
        "ownerFileDenied": true, "ownerCredentialDenied": true, "ownerProcessDenied": true, "ownerDesktopDenied": true,
        "workerWriteAllowed": true, "peerFileDenied": true, "peerWriteDenied": true, "peerRegistryDenied": true,
        "peerProcessDenied": true, "claudeStarted": true, "codexStarted": true, "reviewPolicyVerified": true,
    });
    validate_acceptance(&f.provider, &f.review, &accepted.to_string()).unwrap();
    for patch in [
        json!({ "purpose": "review" }),
        json!({ "workerSid": f.review.worker_sid }),
        json!({ "peerSid": f.provider.worker_sid }),
        json!({ "configurationHash": "0".repeat(64) }),
        json!({ "peerConfigurationHash": "0".repeat(64) }),
        json!({ "peerRegistryDenied": false }),
        json!({ "peerProcessDenied": false }),
        json!({ "peerWriteDenied": false }),
        json!({ "reviewPolicyVerified": false }),
        json!({ "version": 1 }),
    ] {
        let mut changed = accepted.clone();
        for (key, value) in patch.as_object().unwrap() {
            changed[key] = value.clone();
        }
        assert!(
            validate_acceptance(&f.provider, &f.review, &changed.to_string()).is_err(),
            "{patch}"
        );
    }
    assert!(validate_acceptance(&f.review, &f.provider, &accepted.to_string()).is_err());
}

/// The native grant (the C# `TjWorkerDirectoryGrant` in `windows-worker.ts`): a Modify allow, inherited below.
#[test]
fn grants_modify_on_the_exact_folder_and_refuses_a_junction() {
    // The grant refuses a folder whose path isn't its canonical one ("Directory target changed"), as it must for a link.
    // A runner's TEMP is an 8.3 spelling, so the test's own folders live under the canonical temp path.
    let dir = tempfile::Builder::new()
        .tempdir_in(common::canonical(&std::env::temp_dir()))
        .unwrap();
    let (folder, elsewhere) = (dir.path().join("work"), dir.path().join("elsewhere"));
    fs::create_dir_all(folder.join("child")).unwrap();
    fs::create_dir(&elsewhere).unwrap();
    let (worker, peer) = ("S-1-5-21-1-2-3-1001", "S-1-5-21-1-2-3-1002");
    logon::grant_directory(&folder, worker, peer).unwrap();
    assert!(
        logon::sddl(&folder)
            .unwrap()
            .contains(&format!("(A;OICI;0x1301bf;;;{worker})"))
    );
    assert!(
        logon::sddl(&folder.join("child"))
            .unwrap()
            .contains(&format!("(A;OICIID;0x1301bf;;;{worker})"))
    );
    let link = dir.path().join("link");
    junction(&link, &elsewhere);
    assert!(logon::grant_directory(&link, worker, peer).is_err());
    assert!(!logon::sddl(&elsewhere).unwrap().contains(worker));
}
