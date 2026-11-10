//! T2.2 native ACL pass (the PowerShell + icacls pass in `windows-acl.ts`, ported to GetNamedSecurityInfoW,
//! SetEntriesInAclW and SetNamedSecurityInfoW). Only temp folders made here are touched.
//! `windows-acl.driver.test.ts` cases that were about PowerShell (module path, spawn timeouts) don't apply.
#![cfg(windows)]

use daemon::spawn;
use daemon::win::acl::{AclTarget, Mode, PassResult, run_pass, run_pass_for};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::TempDir;
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};
use windows::core::{HSTRING, PWSTR};

/// A well-formed SID that no account has, so CI can exercise the native writes without CodexSandboxUsers.
const FAKE: &str = "S-1-5-21-1-2-3-1002";
const RXW: &str = "0x1201bf"; // what `icacls … (RX,W)` writes, measured on Windows 11

fn target(path: &Path, folder: bool, mode: Mode) -> AclTarget {
    AclTarget {
        path: path.to_path_buf(),
        folder,
        mode,
        codex_dependent: false,
    }
}

fn sddl(path: &Path) -> String {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut text = PWSTR::null();
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    // SAFETY: both buffers come from the system and are freed once with LocalFree.
    unsafe {
        let error = GetNamedSecurityInfoW(
            &HSTRING::from(path),
            SE_FILE_OBJECT,
            info,
            None,
            None,
            None,
            None,
            &mut descriptor,
        );
        assert!(
            error.is_ok(),
            "GetNamedSecurityInfoW {}: {error:?}",
            path.display()
        );
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            info,
            &mut text,
            None,
        )
        .unwrap();
        let value = text.to_string().unwrap();
        LocalFree(Some(HLOCAL(text.0.cast())));
        LocalFree(Some(HLOCAL(descriptor.0)));
        value
    }
}

/// ACEs in an SDDL string that name `sid`.
fn aces(sddl: &str, sid: &str) -> Vec<String> {
    let dacl = &sddl[sddl.find("D:").unwrap()..];
    dacl.split('(')
        .filter(|ace| ace.trim_end_matches(')').ends_with(&format!(";;;{sid}")))
        .map(|ace| format!("({ace}"))
        .collect()
}

fn icacls(args: &[&str]) {
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
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

/// A `.sandbox-bin` like Codex leaves it: inheritance off and an allow for the sandbox group. Codex grants RX; the
/// pass's (RX,W) satisfies the same check, and icacls can't grant to a SID that no account has.
fn codex_bin(path: &Path, sid: &str) {
    fs::create_dir_all(path).unwrap();
    icacls(&[path.to_str().unwrap(), "/inheritance:d"]);
    assert_eq!(
        run_pass_for(sid, &[target(path, true, Mode::Allow)])
            .unwrap()
            .failures,
        Vec::<String>::new()
    );
}

#[test]
fn native_pass_writes_what_icacls_writes() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let (deny, allow, undeny, both) = (
        root.join("deny"),
        root.join("allow"),
        root.join("undeny"),
        root.join("both"),
    );
    let reverse = root.join("reverse");
    for folder in [&deny, &allow, &undeny, &both, &reverse] {
        fs::create_dir_all(folder.join("child")).unwrap();
    }
    let file = root.join("file.txt");
    fs::write(&file, "x").unwrap();

    let first = run_pass_for(
        FAKE,
        &[
            target(&deny, true, Mode::Deny),
            target(&file, false, Mode::Deny),
            target(&allow, true, Mode::Allow),
            target(&undeny, true, Mode::Deny),
            target(&both, true, Mode::Allow),
            target(&reverse, true, Mode::Deny),
        ],
    )
    .unwrap();
    assert_eq!(
        first,
        PassResult {
            sid: Some(FAKE.into()),
            group_missing: false,
            failures: vec![],
            applied: 6
        }
    );
    assert_eq!(
        aces(&sddl(&deny), FAKE),
        [format!("(D;OICI;{RXW};;;{FAKE})")]
    );
    assert_eq!(
        aces(&sddl(&deny.join("child")), FAKE),
        [format!("(D;OICIID;{RXW};;;{FAKE})")]
    );
    assert_eq!(aces(&sddl(&file), FAKE), [format!("(D;;{RXW};;;{FAKE})")]);
    assert_eq!(
        aces(&sddl(&allow), FAKE),
        [format!("(A;OICI;{RXW};;;{FAKE})")]
    );
    // Explicit ACEs come first, before the inherited ones (canonical order, as icacls leaves it).
    assert!(
        sddl(&deny).contains(&format!("D:AI(D;OICI;{RXW};;;{FAKE})")),
        "{}",
        sddl(&deny)
    );

    // A second pass finds everything in place and changes nothing.
    let unchanged = sddl(&deny);
    let again = run_pass_for(
        FAKE,
        &[
            target(&deny, true, Mode::Deny),
            target(&allow, true, Mode::Allow),
        ],
    )
    .unwrap();
    assert_eq!(again.applied, 0);
    assert_eq!(sddl(&deny), unchanged);

    let second = run_pass_for(
        FAKE,
        &[
            target(&undeny, true, Mode::RemoveDeny),
            target(&both, true, Mode::Deny),
            target(&deny, true, Mode::CheckDeny),
            target(&allow, true, Mode::CheckAllow),
            target(&undeny.join("child"), true, Mode::CheckNoDeny),
            target(&reverse, true, Mode::Allow),
        ],
    )
    .unwrap();
    assert_eq!(second.failures, Vec::<String>::new());
    assert_eq!(second.applied, 3);
    // icacls /remove:d also clears the inherited copies below.
    assert_eq!(aces(&sddl(&undeny), FAKE), Vec::<String>::new());
    assert_eq!(
        aces(&sddl(&undeny.join("child")), FAKE),
        Vec::<String>::new()
    );
    // icacls /deny drops an explicit allow of the same rights.
    assert_eq!(
        aces(&sddl(&both), FAKE),
        [format!("(D;OICI;{RXW};;;{FAKE})")]
    );
    // And icacls /grant drops an explicit deny of the same rights.
    assert_eq!(
        aces(&sddl(&reverse), FAKE),
        [format!("(A;OICI;{RXW};;;{FAKE})")]
    );

    // Checks only report; they never change anything.
    let before = sddl(&allow);
    let checks = run_pass_for(
        FAKE,
        &[
            target(&allow, true, Mode::CheckDeny),
            target(&deny, true, Mode::CheckNoDeny),
        ],
    )
    .unwrap();
    assert_eq!(checks.applied, 0);
    assert_eq!(
        checks.failures,
        [
            format!("{}: expected ACL state was not found", allow.display()),
            format!("{}: expected ACL state was not found", deny.display()),
        ]
    );
    assert_eq!(sddl(&allow), before);
}

#[test]
fn codex_dependent_targets_wait_for_a_protected_sandbox_bin() {
    let dir = TempDir::new().unwrap();
    let (ready, open, home) = (
        dir.path().join("ready-bin"),
        dir.path().join("open-bin"),
        dir.path().join("home"),
    );
    codex_bin(&ready, FAKE);
    fs::create_dir_all(&open).unwrap();
    fs::create_dir_all(&home).unwrap();
    let dependent = |path: &Path| AclTarget {
        codex_dependent: true,
        ..target(path, true, Mode::Deny)
    };

    let fine = run_pass_for(
        FAKE,
        &[
            target(&ready, true, Mode::VerifyBin),
            target(&ready, true, Mode::CheckNoDeny),
        ],
    )
    .unwrap();
    assert_eq!(fine.failures, Vec::<String>::new());

    let blocked = run_pass_for(
        FAKE,
        &[target(&open, true, Mode::VerifyBin), dependent(&home)],
    )
    .unwrap();
    assert_eq!(
        blocked.failures,
        [format!(
            "{}: .sandbox-bin needs a protected CodexSandboxUsers ReadAndExecute allow",
            open.display()
        )]
    );
    assert_eq!(blocked.applied, 0);
    assert_eq!(aces(&sddl(&home), FAKE), Vec::<String>::new());
}

#[test]
fn a_failing_target_is_reported_without_stopping_the_pass() {
    let dir = TempDir::new().unwrap();
    let (missing, present) = (dir.path().join("missing"), dir.path().join("present"));
    fs::create_dir(&present).unwrap();
    let result = run_pass_for(
        FAKE,
        &[
            target(&missing, true, Mode::Deny),
            target(&present, true, Mode::Deny),
        ],
    )
    .unwrap();
    assert_eq!(result.applied, 1);
    assert_eq!(result.failures.len(), 1);
    assert!(
        result.failures[0].starts_with(&format!("{}: ", missing.display())),
        "{:?}",
        result.failures
    );
    assert_eq!(aces(&sddl(&present), FAKE).len(), 1);
}

#[test]
fn resolves_the_live_sandbox_group_on_every_pass() {
    let first = run_pass(&[]).unwrap();
    let second = run_pass(&[]).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.group_missing, first.sid.is_none());
    if let Some(sid) = &first.sid {
        assert!(
            sid.starts_with("S-1-") && sid[4..].split('-').all(|part| part.parse::<u64>().is_ok()),
            "{sid}"
        );
    }
    assert_eq!((first.failures.len(), first.applied), (0, 0));
}

/// TypeScript (`tjommie-rs/tools/acl-probe.ts`) prepares one copy of a folder tree and Rust the other; every node
/// ends with the same SDDL. Needs the CodexSandboxUsers group, which the TypeScript pass always looks up.
#[test]
fn typescript_and_rust_passes_leave_equal_sddl() {
    let Some(sid) = run_pass(&[]).unwrap().sid else {
        eprintln!("skipped: CodexSandboxUsers does not exist on this machine");
        return;
    };
    let dir = TempDir::new().unwrap();
    let twins = [dir.path().join("ts"), dir.path().join("rs")];
    for root in &twins {
        for folder in [
            "deny/child",
            "allow/child",
            "undeny/child",
            "both/child",
            "reverse/child",
            "modify/child",
            "home/.sandbox/child",
        ] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        // An explicit Modify allow: /deny (RX,W) leaves DELETE and SYNCHRONIZE of it.
        icacls(&[
            root.join("modify").to_str().unwrap(),
            "/grant",
            &format!("*{sid}:(OI)(CI)(M)"),
        ]);
        fs::write(root.join("file.txt"), "x").unwrap();
        codex_bin(&root.join("home/.sandbox-bin"), &sid);
    }
    let first = |root: &Path| {
        vec![
            target(&root.join("deny"), true, Mode::Deny),
            target(&root.join("file.txt"), false, Mode::Deny),
            target(&root.join("allow"), true, Mode::Allow),
            target(&root.join("undeny"), true, Mode::Deny),
            target(&root.join("both"), true, Mode::Allow),
            target(&root.join("reverse"), true, Mode::Deny),
            target(&root.join("modify"), true, Mode::Deny),
            target(&root.join("home/.sandbox-bin"), true, Mode::VerifyBin),
            AclTarget {
                codex_dependent: true,
                ..target(&root.join("home"), true, Mode::Deny)
            },
            AclTarget {
                codex_dependent: true,
                ..target(&root.join("home/.sandbox"), true, Mode::Allow)
            },
        ]
    };
    let second = |root: &Path| {
        vec![
            target(&root.join("undeny"), true, Mode::RemoveDeny),
            target(&root.join("both"), true, Mode::Deny),
            target(&root.join("reverse"), true, Mode::Allow),
            target(&root.join("deny"), true, Mode::CheckDeny),
            target(&root.join("allow"), true, Mode::CheckAllow),
            target(&root.join("home/.sandbox-bin"), true, Mode::CheckNoDeny),
            target(&root.join("undeny/child"), true, Mode::CheckNoDeny),
            target(&root.join("allow"), true, Mode::CheckDeny), // a deliberate failure, to compare messages
        ]
    };
    for pass in [first, second] {
        let ts = probe(&pass(&twins[0]));
        let rs = run_pass(&pass(&twins[1])).unwrap();
        assert_eq!(ts["sid"], json!(sid));
        assert_eq!(ts["groupMissing"], json!(false));
        assert_eq!(ts["applied"], json!(rs.applied));
        let ts_failures: Vec<String> = serde_json::from_value(ts["failures"].clone()).unwrap();
        let normalised: Vec<String> = ts_failures
            .iter()
            .map(|line| line.replace(path(&twins[0]), path(&twins[1])))
            .collect();
        assert_eq!(normalised, rs.failures);
    }
    let mut nodes = 0;
    for relative in walk(&twins[0], Path::new("")) {
        assert_eq!(
            sddl(&twins[0].join(&relative)),
            sddl(&twins[1].join(&relative)),
            "{}",
            relative.display()
        );
        nodes += 1;
    }
    assert_eq!(nodes, 17);
}

fn path(root: &Path) -> &str {
    root.to_str().unwrap()
}

/// Every file and folder below `root`, relative to it.
fn walk(root: &Path, relative: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    for entry in fs::read_dir(root.join(relative)).unwrap() {
        let child = relative.join(entry.unwrap().file_name());
        found.push(child.clone());
        if root.join(&child).is_dir() {
            found.extend(walk(root, &child));
        }
    }
    found
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Deny => "deny",
        Mode::Allow => "allow",
        Mode::RemoveDeny => "removeDeny",
        Mode::VerifyBin => "verifyBin",
        Mode::CheckDeny => "checkDeny",
        Mode::CheckAllow => "checkAllow",
        Mode::CheckNoDeny => "checkNoDeny",
    }
}

fn probe(targets: &[AclTarget]) -> Value {
    let input: Vec<Value> = targets
        .iter()
        .map(|t| json!({ "path": t.path, "folder": t.folder, "mode": mode_name(t.mode), "codexDependent": t.codex_dependent }))
        .collect();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/acl-probe.ts");
    let mut command = spawn::command("node");
    command.arg(script).env("NODE_NO_WARNINGS", "1");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("node is on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(Value::Array(input).to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "acl probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
