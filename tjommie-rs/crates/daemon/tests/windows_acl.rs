//! T2.2 port of `windows-acl.test.ts`: which targets the Codex data ACL pass gets, and how its result becomes a
//! protection status. No ACL is changed here; the native pass is in `windows_acl_driver.rs`.
//! Not ported: the icacls command builders (the pass is native now) and the separate async entry point (the Rust
//! pass is one blocking call; callers run it on a blocking thread).
#![cfg(windows)]

use daemon::spawn;
use daemon::win::acl::{
    BotHome, Mode, PassResult, Plan, ProtectedAccount, ProtectedPaths, Protection, Provider,
    finish, plan_targets,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const SID: &str = "S-1-5-21-1-2-3-1002";

fn success() -> PassResult {
    PassResult {
        sid: Some(SID.into()),
        group_missing: false,
        failures: vec![],
        applied: 0,
    }
}

/// `dataPaths(tempRoot())`: the root and its folders exist.
fn data_paths(dir: &TempDir) -> ProtectedPaths {
    let paths = ProtectedPaths::new(dir.path());
    for folder in [
        &paths.bots,
        &paths.accounts,
        &paths.logs,
        &paths.data,
        &paths.shared_files,
    ] {
        fs::create_dir_all(folder).unwrap();
    }
    paths
}

struct Fixture {
    _dir: TempDir,
    paths: ProtectedPaths,
    codex_home: PathBuf,
    claude: BotHome,
    codex: BotHome,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let paths = data_paths(&dir);
    fs::create_dir(paths.accounts.join("claude-main")).unwrap();
    let codex_home = paths.accounts.join("codex-main");
    fs::create_dir_all(codex_home.join(".sandbox")).unwrap();
    fs::create_dir(codex_home.join(".sandbox-bin")).unwrap();
    fs::write(&paths.token, "fixture").unwrap();
    for name in ["tjommie.db", "tjommie.db-wal", "tjommie.db-shm"] {
        fs::write(paths.data.join(name), "fixture").unwrap();
    }
    let claude = bot(&paths, "claude-bot", Provider::Claude);
    let codex = bot(&paths, "codex-bot", Provider::Codex);
    fs::create_dir(&claude.home_folder).unwrap();
    fs::create_dir_all(codex.home_folder.join("inbox")).unwrap();
    fs::write(codex.home_folder.join("notes.md"), "fixture").unwrap();
    Fixture {
        _dir: dir,
        paths,
        codex_home,
        claude,
        codex,
    }
}

fn bot(paths: &ProtectedPaths, id: &str, provider: Provider) -> BotHome {
    BotHome {
        id: id.into(),
        home_folder: paths.bots.join(id),
        provider,
    }
}

fn account(id: &str, provider: Provider, folder: &Path) -> ProtectedAccount {
    ProtectedAccount {
        id: id.into(),
        provider,
        folder: folder.to_path_buf(),
    }
}

fn at(plan: &Plan, path: &Path) -> Option<(Mode, bool)> {
    plan.targets
        .iter()
        .find(|target| target.path == path)
        .map(|target| (target.mode, target.folder))
}

/// Junctions need no elevation, unlike directory symlinks.
fn junction(link: &Path, target: &Path) {
    let status = spawn::command("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

#[test]
fn sends_all_protected_folders_through_one_pass_including_shared_files_and_claude_bot_homes() {
    let f = fixture();
    assert!(f.paths.shared_files.is_dir());
    let plan = plan_targets(&f.paths, &[f.claude.clone(), f.codex.clone()], None);
    assert_eq!(finish(&plan, Ok(success())).protection, Protection::Ready);
    assert_eq!(
        at(&plan, &f.paths.accounts.join("claude-main")),
        Some((Mode::Deny, true))
    );
    assert_eq!(at(&plan, &f.paths.shared_files), Some((Mode::Deny, true)));
    assert_eq!(at(&plan, &f.claude.home_folder), Some((Mode::Deny, true)));
    assert_eq!(
        at(&plan, &f.codex.home_folder),
        Some((Mode::RemoveDeny, true))
    );
    assert_eq!(
        at(&plan, &f.codex.home_folder.join("notes.md")),
        Some((Mode::CheckNoDeny, false))
    );
    assert_eq!(at(&plan, &f.codex_home), Some((Mode::Deny, true)));
    assert_eq!(
        at(&plan, &f.codex_home.join(".sandbox")),
        Some((Mode::Allow, true))
    );
    assert_eq!(
        at(&plan, &f.codex_home.join(".sandbox-bin")),
        Some((Mode::VerifyBin, true))
    );
    assert_eq!(at(&plan, &f.paths.bots), None);
    assert_eq!(
        plan.targets
            .iter()
            .filter(|target| target.path.starts_with(&f.paths.data))
            .count(),
        4
    );
}

#[test]
fn skips_a_missing_claude_bot_home_without_taking_down_codex_protection() {
    let f = fixture();
    fs::remove_dir_all(&f.claude.home_folder).unwrap();
    let plan = plan_targets(&f.paths, std::slice::from_ref(&f.claude), None);
    let report = finish(&plan, Ok(success()));
    assert_eq!(report.protection, Protection::Ready);
    assert_eq!(at(&plan, &f.claude.home_folder), None);
    let message = format!(
        "{}: Claude bot folder is missing",
        f.claude.home_folder.display()
    );
    assert!(
        report.info.iter().any(|line| line.contains(&message)),
        "{:?}",
        report.info
    );
}

#[test]
fn skips_a_missing_codex_bot_home_without_taking_down_other_codex_bots() {
    let f = fixture();
    let other = bot(&f.paths, "other-codex", Provider::Codex);
    fs::create_dir_all(other.home_folder.join("inbox")).unwrap();
    fs::write(other.home_folder.join("notes.md"), "fixture").unwrap();
    fs::remove_dir_all(&f.codex.home_folder).unwrap();
    let plan = plan_targets(&f.paths, &[f.codex.clone(), other.clone()], None);
    let report = finish(&plan, Ok(success()));
    assert_eq!(report.protection, Protection::Ready);
    assert!(
        !plan
            .targets
            .iter()
            .any(|target| target.path.starts_with(&f.codex.home_folder))
    );
    assert_eq!(
        at(&plan, &other.home_folder),
        Some((Mode::RemoveDeny, true))
    );
    let message = format!(
        "{}: Codex bot folder is missing",
        f.codex.home_folder.display()
    );
    assert!(
        report.info.iter().any(|line| line.contains(&message)),
        "{:?}",
        report.info
    );
}

#[test]
fn distinguishes_an_absent_group_and_setup_marker_from_a_failed_pass() {
    let f = fixture();
    let plan = plan_targets(&f.paths, &[], None);
    let missing = finish(
        &plan,
        Ok(PassResult {
            sid: None,
            group_missing: true,
            failures: vec![],
            applied: 0,
        }),
    );
    assert_eq!(missing.protection, Protection::NotConfigured);
    assert_eq!(missing.protection.reason(), Some("Set up Codex's sandbox"));
    assert!(!missing.info.is_empty());
    assert!(missing.errors.is_empty(), "{:?}", missing.errors);
    let broken = finish(&plan, Err("the ACL pass could not start".into()));
    assert_eq!(broken.protection, Protection::CheckFailed);
    assert!(
        broken
            .protection
            .reason()
            .unwrap()
            .contains("couldn't check Codex's sandbox")
    );
    assert!(
        broken
            .errors
            .iter()
            .any(|line| line.contains("could not start"))
    );
}

#[test]
fn skips_codex_main_until_the_setup_marker_exists() {
    let dir = TempDir::new().unwrap();
    let paths = data_paths(&dir);
    fs::create_dir(paths.accounts.join("claude-main")).unwrap();
    let plan = plan_targets(&paths, &[], None);
    let report = finish(&plan, Ok(success()));
    assert_eq!(report.protection, Protection::NotConfigured);
    assert_eq!(report.protection.reason(), Some("Set up Codex's sandbox"));
    assert!(
        !plan
            .targets
            .iter()
            .any(|target| target.path.to_string_lossy().contains("codex-main"))
    );
    assert!(!report.info.is_empty());
}

#[test]
fn rejects_a_non_directory_account_entry() {
    for name in ["claude-main", "codex-main"] {
        let dir = TempDir::new().unwrap();
        let paths = data_paths(&dir);
        fs::write(paths.accounts.join(name), "fixture").unwrap();
        let report = finish(&plan_targets(&paths, &[], None), Ok(success()));
        assert_eq!(report.protection, Protection::CheckFailed, "{name}");
        assert!(!report.errors.is_empty(), "{name}");
    }
}

#[test]
fn refuses_a_junction_in_place_of_an_account_folder() {
    let dir = TempDir::new().unwrap();
    let paths = data_paths(&dir);
    let outside = TempDir::new().unwrap();
    junction(&paths.accounts.join("claude-main"), outside.path());
    let plan = plan_targets(&paths, &[], None);
    let report = finish(&plan, Ok(success()));
    assert_ne!(report.protection, Protection::Ready);
    assert!(
        !plan
            .targets
            .iter()
            .any(|target| target.path == outside.path())
    );
    assert!(
        report
            .errors
            .iter()
            .any(|line| line.contains("unexpected path type")),
        "{:?}",
        report.errors
    );
}

#[test]
fn protects_every_additional_account_while_a_new_unconfigured_account_does_not_block_a_ready_sandbox()
 {
    let f = fixture();
    let accounts = [
        account("codex-main", Provider::Codex, &f.codex_home),
        account(
            "second-codex",
            Provider::Codex,
            &f.paths.accounts.join("second-codex"),
        ),
        account(
            "second-claude",
            Provider::Claude,
            &f.paths.accounts.join("second-claude"),
        ),
    ];
    fs::create_dir(&accounts[1].folder).unwrap();
    fs::create_dir(&accounts[2].folder).unwrap();
    let plan = plan_targets(&f.paths, &[], Some(&accounts));
    assert_eq!(finish(&plan, Ok(success())).protection, Protection::Ready);
    for account in &accounts {
        assert!(
            plan.targets
                .iter()
                .any(|t| t.path == account.folder && t.mode == Mode::Deny),
            "{}",
            account.id
        );
    }
    assert_eq!(
        at(&plan, &f.codex_home.join(".sandbox-bin")),
        Some((Mode::VerifyBin, true))
    );
    assert_eq!(at(&plan, &accounts[1].folder.join(".sandbox")), None);
}

#[test]
fn rejects_an_account_metadata_path_outside_the_account_id_folder() {
    let f = fixture();
    let accounts = [account("other", Provider::Codex, &f.paths.root)];
    let plan = plan_targets(&f.paths, &[], Some(&accounts));
    assert_eq!(
        finish(&plan, Ok(success())).protection,
        Protection::CheckFailed
    );
    assert_eq!(at(&plan, &f.paths.root), None);
}

#[test]
fn protects_an_explicitly_configured_external_account_root_in_startup_and_refresh_passes() {
    let mut f = fixture();
    let original_accounts = f.paths.accounts.clone();
    let external = TempDir::new().unwrap();
    f.paths.accounts = external.path().join("external-accounts");
    let claude_home = f.paths.accounts.join("claude-main");
    let codex_home = f.paths.accounts.join("codex-main");
    fs::create_dir_all(&claude_home).unwrap();
    fs::create_dir_all(codex_home.join(".sandbox")).unwrap();
    fs::create_dir(codex_home.join(".sandbox-bin")).unwrap();
    let accounts = [
        account("claude-main", Provider::Claude, &claude_home),
        account("codex-main", Provider::Codex, &codex_home),
    ];
    let startup = plan_targets(&f.paths, std::slice::from_ref(&f.claude), None);
    let refresh = plan_targets(&f.paths, std::slice::from_ref(&f.claude), Some(&accounts));
    assert_eq!(
        finish(&startup, Ok(success())).protection,
        Protection::Ready
    );
    assert_eq!(
        finish(&refresh, Ok(success())).protection,
        Protection::Ready
    );
    assert_eq!(startup.targets, refresh.targets);
    for path in [
        &claude_home,
        &codex_home,
        &f.paths.data,
        &f.paths.logs,
        &f.paths.shared_files,
        &f.claude.home_folder,
    ] {
        assert!(
            refresh
                .targets
                .iter()
                .any(|t| &t.path == path && t.mode == Mode::Deny),
            "{}",
            path.display()
        );
    }
    assert_eq!(
        at(&refresh, &codex_home.join(".sandbox")),
        Some((Mode::Allow, true))
    );
    assert_eq!(
        at(&refresh, &codex_home.join(".sandbox-bin")),
        Some((Mode::VerifyBin, true))
    );
    assert!(
        !refresh
            .targets
            .iter()
            .any(|t| t.path.starts_with(&original_accounts) || t.path == f.paths.accounts)
    );
}

#[test]
fn keeps_exact_account_identity_and_non_account_data_containment_with_an_external_account_root() {
    let mut f = fixture();
    let external = TempDir::new().unwrap();
    f.paths.accounts = external.path().to_path_buf();
    let wrong = f.paths.accounts.join("wrong-id");
    fs::create_dir(&wrong).unwrap();
    f.paths.logs = f.paths.accounts.join("logs");
    fs::create_dir(&f.paths.logs).unwrap();
    let accounts = [account("expected-id", Provider::Claude, &wrong)];
    let plan = plan_targets(&f.paths, &[], Some(&accounts));
    assert_eq!(
        finish(&plan, Ok(success())).protection,
        Protection::CheckFailed
    );
    assert!(
        !plan
            .targets
            .iter()
            .any(|t| t.path == wrong || t.path == f.paths.logs)
    );
}

#[test]
fn rejects_an_external_account_junction_before_applying_acls_through_it() {
    for kind in ["root", "account", "sandbox", "sandbox-bin"] {
        let dir = TempDir::new().unwrap();
        let mut paths = data_paths(&dir);
        let external = TempDir::new().unwrap();
        paths.accounts = external.path().join("accounts");
        let outside = TempDir::new().unwrap();
        let home = paths.accounts.join("codex-main");
        let unsafe_path = match kind {
            "root" => paths.accounts.clone(),
            "account" => home.clone(),
            other => home.join(format!(".{other}")),
        };
        if kind != "root" {
            fs::create_dir(&paths.accounts).unwrap();
        }
        if kind == "sandbox" || kind == "sandbox-bin" {
            fs::create_dir(&home).unwrap();
        }
        if kind == "sandbox-bin" {
            fs::create_dir(home.join(".sandbox")).unwrap();
        }
        junction(&unsafe_path, outside.path());
        let accounts = [account("codex-main", Provider::Codex, &home)];
        let plan = plan_targets(&paths, &[], Some(&accounts));
        assert_eq!(
            finish(&plan, Ok(success())).protection,
            Protection::CheckFailed,
            "{kind}"
        );
        assert!(
            !plan
                .targets
                .iter()
                .any(|t| t.path.starts_with(&unsafe_path) || t.path.starts_with(outside.path())),
            "{kind}: {:?}",
            plan.targets
        );
    }
}
