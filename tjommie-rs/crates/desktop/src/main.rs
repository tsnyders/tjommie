//! Tjommie's desktop app: the GPUI shell in the Aegis design (Phase 3 plan).
// No console window, ever (spec §1).
#![windows_subsystem = "windows"]

use std::path::Path;
use std::process::ExitCode;

use desktop_host::identity::{self, DataRoot, Identity};
use desktop_host::owner_safety;
use desktop_host::shell::single_instance::{self, Launch};

fn main() -> ExitCode {
    let identity = identity::CURRENT;
    // The data root (spec §4), refused when it is the owner's and this isn't the production build. There is no window
    // or log yet to say so; the exit code does.
    let root = match identity.data_root() {
        Ok(root) if owner_safety::assert_root_allowed(&root, &identity).is_ok() => root,
        _ => return ExitCode::from(2),
    };
    desktop_host::log::init(&root);
    desktop_host::crash::install(&root);
    let instance = match single_instance::acquire(&lock_name(&identity, &root)) {
        Ok(Launch::Second) => return ExitCode::SUCCESS,
        Ok(Launch::First(instance)) => Some(instance),
        Err(error) => {
            log::warn!("Running without the single-instance lock: {error}");
            None
        }
    };
    // Windows passes --after-crash when it restarts the app after a crash. Its --hidden (start in the tray) waits for
    // the tray (T6.1): until then the window shows, so a restarted app is never invisible.
    let after_crash = std::env::args().any(|arg| arg == "--after-crash");
    desktop::app::run(identity, after_crash, instance);
    ExitCode::SUCCESS
}

/// One app per identity, and per root for the disposable identities (Electron locks per profile), so parallel e2e runs
/// don't take each other's lock.
fn lock_name(identity: &Identity, root: &Path) -> String {
    match identity.data_root {
        DataRoot::Disposable => {
            let root = root.display().to_string().replace(['\\', '/', ':'], "_");
            format!("{}.{root}", identity.app_id)
        }
        DataRoot::LocalAppData(_) => identity.app_id.to_owned(),
    }
}
