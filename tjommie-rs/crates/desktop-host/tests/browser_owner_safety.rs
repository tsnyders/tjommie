//! F7: the engine never works in the owner's production folders unless the build is production. Nothing here creates
//! a folder or starts a browser: the refusal comes first.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::path::PathBuf;
use std::rc::Rc;

use common::Rig;
use desktop_host::browser::engine::{Engine, EngineConfig, EngineError};
use desktop_host::identity::{DEV, E2E, Identity, PRODUCTION};
use windows::Win32::Foundation::HWND;

fn owner_folder(variable: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(variable).expect("a Windows profile variable")).join("Tjommie")
}

fn engine_for(root: PathBuf, identity: Identity) -> Result<Rc<Engine>, EngineError> {
    let (events, _) = async_channel::unbounded();
    let (notify, _) = async_channel::unbounded();
    Engine::new(
        EngineConfig::new(HWND::default(), root, identity),
        events,
        notify,
    )
}

#[test]
fn a_dev_engine_refuses_the_owners_browser_profile_folders() {
    for variable in ["LOCALAPPDATA", "APPDATA"] {
        let root = owner_folder(variable).join("data").join("webview2");
        let error = engine_for(root.clone(), DEV).err().expect("a refusal");
        assert!(
            error.to_string().contains("must not use the owner's"),
            "{root:?}: {error}"
        );
    }
}

#[test]
fn a_disposable_identity_stays_under_temp_and_production_may_use_its_own_root() {
    // The e2e identity keeps its data under %TEMP% and nowhere else, even outside the owner's folders.
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("Tjommie-rs")
        .join("x");
    assert!(engine_for(local, E2E).is_err());
    let temp = desktop_host::owner_safety::test_root().expect("a temp root");
    assert!(engine_for(temp.path().join("data").join("webview2"), E2E).is_ok());
    // Production is the one identity that may use the owner's folders.
    assert!(
        engine_for(
            owner_folder("LOCALAPPDATA").join("data").join("webview2"),
            PRODUCTION
        )
        .is_ok()
    );
}

#[test]
fn a_bot_whose_home_is_in_the_owners_folders_is_refused_before_anything_starts() {
    let mut rig = Rig::new();
    let mut bot = rig.bot("alpha");
    bot.home_folder = owner_folder("LOCALAPPDATA")
        .join("bots")
        .join("alpha")
        .to_string_lossy()
        .into_owned();
    let engine = rig.engine.clone();
    let error = rig
        .ui
        .run(async move { engine.open(&bot).await })
        .expect_err("a refusal");
    assert!(
        error.to_string().contains("must not use the owner's"),
        "{error}"
    );
    assert!(rig.engine.snapshot("alpha").is_none());
    assert!(
        common::webview_processes(rig.root.path()).is_empty(),
        "no browser was started"
    );
}
