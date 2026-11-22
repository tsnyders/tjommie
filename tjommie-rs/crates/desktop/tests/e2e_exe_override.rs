//! `TJOMMIE_E2E_EXE` (test only): the e2e tests launch the exe it names in place of the one cargo built, so an acceptance run
//! can install a packaged test-identity build and run the e2e set against it. This is the resolution logic alone, with no
//! app started: unset leaves the built exe as it was, a good value wins, and a relative path, a missing file, a folder and an
//! empty value are each refused with a message that says why, never answered with the built exe instead (a run that meant to
//! test the installed build must not test another one).
#![cfg(feature = "test-driver")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::ffi::OsStr;

use common::{E2E_EXE_VAR, resolve_desktop_exe};
use desktop_host::owner_safety;

const BUILT: &str = r"C:\built\target\debug\tjommie-desktop.exe";

fn resolve(over: Option<&OsStr>) -> Result<String, String> {
    resolve_desktop_exe(over, BUILT, None)
}

#[test]
fn unset_is_the_exe_cargo_built() {
    assert_eq!(resolve(None).unwrap(), BUILT);
}

#[test]
fn an_absolute_path_to_an_existing_file_is_launched_instead() {
    let root = owner_safety::test_root().unwrap();
    let packaged = root.path().join("Tjommie E2E.exe");
    std::fs::write(&packaged, b"not really an exe").unwrap();
    let wanted = packaged.to_str().unwrap();
    assert_eq!(resolve(Some(OsStr::new(wanted))).unwrap(), wanted);
}

#[test]
fn a_relative_path_is_refused_and_says_absolute() {
    // It exists relative to the test's folder, so only the rule about absolute paths can refuse it.
    let relative = "Cargo.toml";
    assert!(std::path::Path::new(relative).is_file());
    let error = resolve(Some(OsStr::new(relative))).unwrap_err();
    assert!(
        error.contains(E2E_EXE_VAR) && error.contains("absolute") && error.contains(relative),
        "{error}"
    );
}

#[test]
fn a_missing_file_is_refused_and_says_it_does_not_exist() {
    let root = owner_safety::test_root().unwrap();
    let missing = root.path().join("not-installed.exe");
    let error = resolve(Some(missing.as_os_str())).unwrap_err();
    assert!(
        error.contains(E2E_EXE_VAR) && error.contains("does not exist"),
        "{error}"
    );
    assert!(error.contains(missing.to_str().unwrap()), "{error}");
}

#[test]
fn a_folder_is_refused_and_says_it_is_not_a_file() {
    let root = owner_safety::test_root().unwrap();
    let error = resolve(Some(root.path().as_os_str())).unwrap_err();
    assert!(
        error.contains(E2E_EXE_VAR) && error.contains("not a file"),
        "{error}"
    );
}

#[test]
fn an_empty_value_is_refused_not_ignored() {
    let error = resolve(Some(OsStr::new(""))).unwrap_err();
    assert!(
        error.contains(E2E_EXE_VAR) && error.contains("empty"),
        "{error}"
    );
}

#[test]
fn the_installed_production_app_is_refused_and_a_test_identity_build_is_not() {
    // A stand-in for %LOCALAPPDATA%, with an exe in the production app's install folder and one in the update test app's.
    let root = owner_safety::test_root().unwrap();
    let local = root.path();
    let production = local.join("Programs").join("Tjommie");
    let update_test = local.join("Programs").join("tjommie-updatetest");
    std::fs::create_dir_all(&production).unwrap();
    std::fs::create_dir_all(&update_test).unwrap();
    let production_exe = production.join("Tjommie.exe");
    let test_exe = update_test.join("Tjommie Update Test.exe");
    std::fs::write(&production_exe, b"x").unwrap();
    std::fs::write(&test_exe, b"x").unwrap();
    let local = Some(local.as_os_str());
    let error = resolve_desktop_exe(Some(production_exe.as_os_str()), BUILT, local).unwrap_err();
    assert!(
        error.contains(E2E_EXE_VAR) && error.contains("installed Tjommie"),
        "{error}"
    );
    // However the folder is spelled: other case, forward slashes.
    let spelled = production_exe
        .to_string_lossy()
        .replace('\\', "/")
        .to_uppercase();
    assert!(resolve_desktop_exe(Some(OsStr::new(&spelled)), BUILT, local).is_err());
    // The update test app is another folder, and is what the override is for.
    assert_eq!(
        resolve_desktop_exe(Some(test_exe.as_os_str()), BUILT, local).unwrap(),
        test_exe.to_str().unwrap()
    );
}

#[test]
fn the_daemon_is_looked_for_beside_the_exe_that_is_launched() {
    // Without the variable, beside the built exe: what every e2e test did before the override existed.
    if std::env::var_os(E2E_EXE_VAR).is_none() {
        let beside = common::daemon_exe();
        assert_eq!(beside.file_name().unwrap(), "tjommie-daemon.exe");
        assert_eq!(
            beside.parent(),
            std::path::Path::new(env!("CARGO_BIN_EXE_tjommie-desktop")).parent()
        );
    }
}
