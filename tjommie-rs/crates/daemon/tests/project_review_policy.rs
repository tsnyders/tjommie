//! Port of `packages/daemon/src/project-review-policy.test.ts`.

use std::fs;

use daemon::project_review_policy::{
    REVIEW_SETTINGS, review_source_hash, review_source_hash_of, review_worker_policy_hash,
};
use daemon::win::boundary::PolicyReceipt;
use serde_json::json;

#[test]
fn binds_packaged_acceptance_to_the_actual_executable_bytes_and_path() {
    // The TypeScript test binds the bundle and the external worker launcher. Rust has one executable and no launcher
    // script, so this binds the executable's bytes and path.
    let root = tempfile::tempdir().unwrap();
    let exe = root.path().join("tjommie-daemon.exe");
    fs::write(&exe, "synthetic daemon bundle\n").unwrap();
    let original = review_source_hash_of(&exe).unwrap();

    fs::write(&exe, "changed synthetic daemon bundle\n").unwrap();
    assert_ne!(review_source_hash_of(&exe).unwrap(), original);
    fs::write(&exe, "synthetic daemon bundle\n").unwrap();
    assert_eq!(review_source_hash_of(&exe).unwrap(), original);

    let moved_root = tempfile::tempdir().unwrap();
    let moved = moved_root.path().join("tjommie-daemon.exe");
    fs::write(&moved, "synthetic daemon bundle\n").unwrap();
    assert_ne!(review_source_hash_of(&moved).unwrap(), original);

    fs::remove_file(&exe).unwrap();
    assert!(review_source_hash_of(&exe).is_err());
}

#[test]
fn hashes_the_running_executable_by_default() {
    let exe = std::env::current_exe().unwrap();
    let hash = review_source_hash().unwrap();
    assert_eq!(hash.len(), 64);
    assert_eq!(hash, review_source_hash_of(&exe).unwrap());
}

#[test]
fn binds_worker_evidence_to_its_sid_observer_and_effective_machine_policy() {
    let receipt = PolicyReceipt {
        worker_sid: "S-1-5-21-1-2-3-1001".into(),
        provider_sid: "S-1-5-21-1-2-3-1003".into(),
        configuration_sha256: "e".repeat(64),
        observer_sha256: "a".repeat(64),
        policy_sha256: "b".repeat(64),
    };
    let original = review_worker_policy_hash(&receipt).unwrap();

    let changed = [
        PolicyReceipt {
            worker_sid: "S-1-5-21-1-2-3-1002".into(),
            ..receipt.clone()
        },
        PolicyReceipt {
            observer_sha256: "c".repeat(64),
            ..receipt.clone()
        },
        PolicyReceipt {
            policy_sha256: "d".repeat(64),
            ..receipt.clone()
        },
    ];
    for patch in &changed {
        assert_ne!(review_worker_policy_hash(patch).unwrap(), original);
    }

    let malformed = [
        PolicyReceipt {
            worker_sid: "owner".into(),
            ..receipt.clone()
        },
        PolicyReceipt {
            observer_sha256: "missing".into(),
            ..receipt.clone()
        },
        PolicyReceipt {
            policy_sha256: "B".repeat(64),
            ..receipt.clone()
        },
        // The worker and the provider CLI must be different accounts.
        PolicyReceipt {
            provider_sid: receipt.worker_sid.clone(),
            ..receipt.clone()
        },
    ];
    for patch in &malformed {
        assert_eq!(
            review_worker_policy_hash(patch).unwrap_err(),
            "review_worker_policy_observer_unavailable"
        );
    }
}

#[test]
fn review_settings_is_the_javascript_json_text_in_key_order() {
    let settings = json!({
        "disableClaudeAiConnectors": true,
        "enableAllProjectMcpServers": false,
        "permissions": {
            "allow": [],
            "deny": ["Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch", "Agent", "Task", "Skill", "mcp__*"],
        },
        "hooks": {},
        "enabledPlugins": {},
    });
    assert_eq!(serde_json::to_string(&settings).unwrap(), REVIEW_SETTINGS);
}
