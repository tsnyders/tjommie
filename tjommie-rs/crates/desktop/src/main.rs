//! Tjommie's desktop app: the GPUI shell in the Aegis design (Phase 3 plan).
// No console window, ever (spec §1).
#![windows_subsystem = "windows"]

use std::path::Path;
use std::process::ExitCode;

use desktop_host::identity::{self, Identity};
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
    // Windows restarts a crashed app with `--hidden --after-crash`: it comes back in the tray, and Windows says what
    // happened (`app::run`), as the in-app notice has no window to show it.
    let after_crash = std::env::args_os().any(|arg| arg == "--after-crash");
    desktop::app::run(identity, root, after_crash, instance);
    // A motion check that failed (T7.2) says so to the script that ran it.
    #[cfg(feature = "test-driver")]
    if desktop::scenario::failed() {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// One app per data root (Electron locks per profile), so parallel e2e runs and dev runs on other TJOMMIE_HOME roots
/// don't take each other's lock (review F9). Object names are case-sensitive and paths aren't, hence the lowercase.
/// Production keeps one lock whatever its root (review note N1: M8 hands it over from Electron's).
fn lock_name(identity: &Identity, root: &Path) -> String {
    if identity.name == "production" {
        return identity.app_id.to_owned();
    }
    let root = root.display().to_string().to_lowercase();
    format!(
        "{}.{}",
        identity.app_id,
        root.replace(['\\', '/', ':'], "_")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_host::identity::{DEV, E2E, PRODUCTION};

    #[test]
    fn one_lock_per_root_but_production() {
        let (a, b) = (Path::new("C:/Temp/A"), Path::new("C:/Temp/B"));
        assert_ne!(lock_name(&DEV, a), lock_name(&DEV, b));
        assert_ne!(lock_name(&E2E, a), lock_name(&E2E, b));
        // The same folder, spelled in another case.
        assert_eq!(lock_name(&DEV, a), lock_name(&DEV, Path::new("c:/temp/a")));
        // Production keeps one lock whatever its root (review note N1: M8 hands it over from Electron's).
        assert_eq!(lock_name(&PRODUCTION, a), lock_name(&PRODUCTION, b));
    }
}
