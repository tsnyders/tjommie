//! T2.3 port of `windows-worker-runtime.test.ts`: the runtime ACL checks, the untracked-provider rules and the
//! review mutex fence. Descriptors are built in memory from SDDL; the tree walk uses a temp folder.
#![cfg(windows)]

use daemon::spawn;
use daemon::win::logon::{self, Descriptor};
use daemon::win::worker::{
    MutexWait, assert_mutex_descriptor, assert_runtime_descriptor, assert_runtime_tree,
    enter_review_mutex, unreceipted_provider,
};
use std::cell::Cell;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const OWNER: &str = "S-1-5-21-1-2-3-1000";
const PROVIDER: &str = "S-1-5-21-1-2-3-1001";
const REVIEW: &str = "S-1-5-21-1-2-3-1002";

/// A protected descriptor: the owner has full control and `who` has `mask`. .NET adds SYNCHRONIZE to allows.
fn descriptor(who: &str, mask: u32, owned: &str) -> Descriptor {
    Descriptor::from_sddl(&format!(
        "O:{owned}D:P(A;;FA;;;{OWNER})(A;;{:#x};;;{who})",
        mask | 0x0010_0000
    ))
    .unwrap()
}

#[test]
fn rejects_unsafe_runtime_acls() {
    let read_and_execute = 0x0002_00a9;
    let unsafe_rights = [
        0x0003_01bf,
        0x0001_0000,
        0x40,
        0x0004_0000,
        0x0008_0000,
        0x2,
        0x4,
        0x100,
        0x10,
    ];
    let mut passed = 0;
    for sid in [PROVIDER, REVIEW, "S-1-5-32-545"] {
        assert_runtime_descriptor(&descriptor(sid, read_and_execute, OWNER), OWNER).unwrap();
        passed += 1;
        for mask in unsafe_rights {
            assert!(
                assert_runtime_descriptor(&descriptor(sid, mask, OWNER), OWNER).is_err(),
                "{sid} {mask:#x}"
            );
            passed += 1;
        }
    }
    assert!(
        assert_runtime_descriptor(&descriptor(PROVIDER, read_and_execute, PROVIDER), OWNER)
            .is_err(),
        "worker owner"
    );
    let null_dacl = Descriptor::from_sddl(&format!("O:{OWNER}D:NO_ACCESS_CONTROL")).unwrap();
    assert!(
        assert_runtime_descriptor(&null_dacl, OWNER).is_err(),
        "null DACL"
    );
    assert_eq!(passed + 2, 32);
}

fn icacls(args: &[&str]) {
    let system = std::env::var("SystemRoot").unwrap();
    let output = spawn::command(Path::new(&system).join(r"System32\icacls.exe"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "icacls {args:?}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn walks_the_runtime_tree_and_rejects_writable_descendants_and_links() {
    let dir = TempDir::new().unwrap();
    let runtime = dir.path().join("runtime");
    let (nested, executable) = (
        runtime.join("nested"),
        runtime.join("nested").join("tool.exe"),
    );
    fs::create_dir_all(&nested).unwrap();
    fs::write(&executable, "synthetic").unwrap();
    // Only the owner, by inheritance, then a read-only grant for a worker: what provisioning leaves.
    let owner = logon::current_user_sid().unwrap();
    let path = |p: &Path| p.to_str().unwrap().to_owned();
    icacls(&[
        &path(&runtime),
        "/inheritance:r",
        "/grant:r",
        &format!("*{owner}:(OI)(CI)F"),
    ]);
    // icacls only takes SIDs that resolve, so the "workers" here are BUILTIN\Users and BUILTIN\Guests.
    let (worker, other) = ("S-1-5-32-545", "S-1-5-32-546");
    icacls(&[
        &path(&runtime),
        "/grant",
        &format!("*{worker}:(OI)(CI)(RX)"),
    ]);
    assert_runtime_tree(&runtime, &owner).unwrap();
    for target in [&nested, &executable] {
        icacls(&[&path(target), "/grant", &format!("*{other}:(M)")]);
        assert!(
            assert_runtime_tree(&runtime, &owner).is_err(),
            "{}",
            target.display()
        );
        icacls(&[&path(target), "/remove:g", &format!("*{other}")]);
        assert_runtime_tree(&runtime, &owner).unwrap();
    }
    let link = runtime.join("link");
    let output = spawn::command("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(&link)
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        assert_runtime_tree(&runtime, &owner).is_err(),
        "runtime reparse target"
    );
}

#[test]
fn classifies_untracked_providers_without_trusting_redacted_argv() {
    assert!(
        !unreceipted_provider("node.exe", None, true).unwrap(),
        "native receipt was not honored before argv"
    );
    assert!(
        unreceipted_provider("node.exe", None, false).is_err(),
        "unknown redacted Node was accepted"
    );
    let cli = r"node C:\runtime\node_modules\@anthropic-ai\claude-code\cli.js";
    assert!(unreceipted_provider("node.exe", Some(cli), false).unwrap());
    assert!(unreceipted_provider("node", Some("node C:/x/@openai/codex/bin.js"), false).unwrap());
    assert!(
        !unreceipted_provider("node.exe", Some("node fixed-synthetic-fixture.js"), false).unwrap()
    );
    assert!(
        unreceipted_provider("claude.exe", None, false).unwrap(),
        "untracked native provider skipped"
    );
    assert!(
        !unreceipted_provider("claude.exe", None, true).unwrap(),
        "native provider receipt was ignored"
    );
}

#[test]
fn trusts_only_owner_system_and_administrators_on_the_review_mutex() {
    let safe = Descriptor::from_sddl(&format!("O:{OWNER}D:P(A;;GA;;;{OWNER})")).unwrap();
    assert_mutex_descriptor(&safe, OWNER).unwrap();
    for sddl in [
        format!("O:{OWNER}D:P(A;;GA;;;{OWNER})(A;;GA;;;{PROVIDER})"),
        format!("O:{PROVIDER}D:P(A;;GA;;;{OWNER})"),
    ] {
        assert!(
            assert_mutex_descriptor(&Descriptor::from_sddl(&sddl).unwrap(), OWNER).is_err(),
            "{sddl}"
        );
    }
}

#[test]
fn fences_cross_host_review_ownership_and_abandoned_locks() {
    let dir = TempDir::new().unwrap();
    let marker = dir.path().join("review-worker-active.json");
    let released = Cell::new(0);
    let enter =
        |wait| enter_review_mutex(wait, &marker, PROVIDER, || released.set(released.get() + 1));

    enter(MutexWait::Acquired).unwrap();
    assert_eq!(
        (released.get(), marker.exists()),
        (0, false),
        "successful launch did not retain ownership"
    );
    assert!(enter(MutexWait::Busy).is_err());
    assert_eq!(
        (released.get(), marker.exists()),
        (0, false),
        "busy mutex not fenced"
    );
    fs::write(&marker, "old").unwrap();
    assert!(enter(MutexWait::Acquired).is_err());
    assert_eq!(
        (released.get(), marker.exists()),
        (1, true),
        "old marker not retained"
    );
    fs::remove_file(&marker).unwrap();
    assert!(enter(MutexWait::Abandoned).is_err());
    assert_eq!(
        (released.get(), marker.exists()),
        (2, true),
        "abandoned job not durably fenced"
    );
    assert!(fs::read_to_string(&marker).unwrap().contains(PROVIDER));
    assert!(enter(MutexWait::Acquired).is_err());
    assert_eq!(
        (released.get(), marker.exists()),
        (3, true),
        "later host reused unknown job"
    );
}
