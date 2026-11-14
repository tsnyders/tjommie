//! Start at Windows sign-in (T6.3; port of `login-item.ts`). It writes what Electron's `setLoginItemSettings` writes:
//! the value named by the app's AppUserModelID in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, holding
//! `"<exe>" --hidden`, so an entry R1 already made keeps working for the Rust app in the same install folder. T8.1
//! records the real entry from a published R1 install; until it lands, the name and data below are Electron's documented
//! defaults, and `value_name` is the one thing to change if the recorded facts differ.
//!
//! Dev, e2e and update-test builds never write it (`allowed` is false), and neither does a run with its own
//! `TJOMMIE_USER_DATA`, which is a smoke run beside a real install.

use std::path::PathBuf;
use std::sync::Arc;

use super::registry::{Hive, Registry};

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager and Settings record whether the user turned a startup entry off.
pub const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const ARGS: &str = "--hidden";
/// The marker Electron's `syncLoginItem` leaves in its userData folder once it has made its one-time default.
const MARKER: &str = "login-item-set";

/// `startsHidden`: NSIS relaunches an updated app with `--updated`; that restart stays in the tray too.
pub fn starts_hidden(argv: &[String]) -> bool {
    argv.iter()
        .any(|arg| arg == "--hidden" || arg == "--updated")
}

/// Whether this run may touch the start-at-login entry at all.
pub fn allowed(identity_login_item: bool, own_user_data_dir: Option<&str>) -> bool {
    identity_login_item && own_user_data_dir.is_none_or(str::is_empty)
}

pub struct LoginItemConfig {
    pub allowed: bool,
    /// The Run value's name: the AppUserModelID (Electron's default).
    pub value_name: String,
    pub exe: PathBuf,
    /// Electron's userData folder (`%APPDATA%\Tjommie`), where the one-time marker lives.
    pub user_data: PathBuf,
}

pub struct LoginItem {
    registry: Arc<dyn Registry>,
    config: LoginItemConfig,
}

impl LoginItem {
    pub fn new(registry: Arc<dyn Registry>, config: LoginItemConfig) -> LoginItem {
        LoginItem { registry, config }
    }

    fn command(&self) -> String {
        format!("\"{}\" {ARGS}", self.config.exe.display())
    }

    /// Windows records "the user turned this off" as a 12-byte value whose first byte is odd (Task Manager writes 03).
    /// Without a record, the entry is on.
    fn approved(&self) -> bool {
        self.registry
            .read_binary(Hive::CurrentUser, APPROVED_KEY, &self.config.value_name)
            .and_then(|bytes| bytes.first().copied())
            .is_none_or(|first| first & 1 == 0)
    }

    /// Turns it on once, the first time a real install runs, and leaves a later choice alone (`syncLoginItem`).
    pub fn sync(&self) -> Result<(), String> {
        if !self.config.allowed {
            return Ok(());
        }
        let marker = self.config.user_data.join(MARKER);
        if marker.exists() {
            return Ok(());
        }
        self.set(true)?;
        std::fs::create_dir_all(&self.config.user_data)
            .and_then(|()| std::fs::write(&marker, b""))
            .map_err(|error| format!("The start-at-login marker could not be written: {error}"))
    }

    /// Whether Windows will start the app at sign-in: our entry is there, and the user hasn't turned it off.
    pub fn get(&self) -> bool {
        if !self.config.allowed {
            return false;
        }
        let registered =
            self.registry
                .read_string(Hive::CurrentUser, RUN_KEY, &self.config.value_name);
        registered.is_some_and(|value| value.eq_ignore_ascii_case(&self.command()))
            && self.approved()
    }

    pub fn set(&self, on: bool) -> Result<(), String> {
        if !self.config.allowed {
            return Ok(());
        }
        let name = &self.config.value_name;
        if on {
            self.registry.write_string(RUN_KEY, name, &self.command())?;
            // Turning it on again clears an earlier "off" from Task Manager, as Electron's default `enabled` does.
            self.registry.delete_value(APPROVED_KEY, name)
        } else {
            self.registry.delete_value(RUN_KEY, name)?;
            self.registry.delete_value(APPROVED_KEY, name)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::super::registry::Fake;
    use super::*;

    const EXE: &str = r"C:\Users\alice\AppData\Local\Programs\Tjommie\Tjommie.exe";
    const NAME: &str = "com.tsnyders.tjommie";

    fn item(registry: &Arc<Fake>, user_data: &Path, allowed: bool) -> LoginItem {
        LoginItem::new(
            Arc::clone(registry) as Arc<dyn Registry>,
            LoginItemConfig {
                allowed,
                value_name: NAME.into(),
                exe: PathBuf::from(EXE),
                user_data: user_data.to_path_buf(),
            },
        )
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    // `login-item.test.ts` "starts hidden only when requested".
    #[test]
    fn starts_hidden_only_when_requested() {
        assert!(!starts_hidden(&argv(&["Tjommie.exe"])));
        assert!(starts_hidden(&argv(&["Tjommie.exe", "--hidden"])));
        assert!(starts_hidden(&argv(&["Tjommie.exe", "--updated"])));
    }

    // "defaults on once for a packaged app and preserves a later user choice"
    #[test]
    fn defaults_on_once_and_preserves_a_later_user_choice() {
        let user_data = tempfile::tempdir().unwrap();
        let registry = Arc::new(Fake::default());
        let login = item(&registry, user_data.path(), true);
        login.sync().unwrap();
        assert_eq!(
            registry
                .read_string(Hive::CurrentUser, RUN_KEY, NAME)
                .as_deref(),
            Some(format!("\"{EXE}\" --hidden").as_str())
        );
        assert!(login.get());
        assert!(user_data.path().join("login-item-set").exists());
        login.set(false).unwrap();
        login.sync().unwrap();
        assert!(!login.get());
        assert_eq!(registry.read_string(Hive::CurrentUser, RUN_KEY, NAME), None);
    }

    // "does not register a login item in an unpackaged app", widened to every build that may not: dev, e2e and
    // update-test identities, and a smoke run with its own user-data folder.
    #[test]
    fn a_build_that_may_not_never_writes() {
        assert!(allowed(true, None));
        assert!(allowed(true, Some("")));
        assert!(!allowed(false, None));
        assert!(!allowed(true, Some(r"C:\tmp\smoke")));
        let user_data = tempfile::tempdir().unwrap();
        let registry = Arc::new(Fake::default());
        let login = item(&registry, user_data.path(), false);
        login.sync().unwrap();
        login.set(true).unwrap();
        login.set(false).unwrap();
        assert!(!login.get());
        assert_eq!(registry.writes(), 0);
        assert!(!user_data.path().join("login-item-set").exists());
    }

    // "reports Windows startup approval, not just the Run key"
    #[test]
    fn reports_startup_approval_not_just_the_run_key() {
        let user_data = tempfile::tempdir().unwrap();
        let registry = Arc::new(Fake::default());
        let login = item(&registry, user_data.path(), true);
        login.sync().unwrap();
        // Task Manager's "Disabled" is 03 00 00 00 and a timestamp.
        registry.set_binary(APPROVED_KEY, NAME, &[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(!login.get());
        // Its "Enabled" is 02 00 00 00 ...
        registry.set_binary(APPROVED_KEY, NAME, &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(login.get());
        // Turning it on in Tjommie's own settings clears an earlier "Disabled".
        registry.set_binary(APPROVED_KEY, NAME, &[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        login.set(true).unwrap();
        assert!(login.get());
    }

    // The point of the same value name and data: an entry R1's Electron app wrote keeps working.
    #[test]
    fn an_entry_r1_wrote_keeps_working_and_is_left_alone() {
        let user_data = tempfile::tempdir().unwrap();
        // R1 had already made its one-time default, so the marker is there.
        std::fs::write(user_data.path().join("login-item-set"), b"").unwrap();
        let registry = Arc::new(Fake::default());
        registry.set_string(
            RUN_KEY,
            NAME,
            &format!("\"{}\" --hidden", EXE.to_uppercase()),
        );
        let login = item(&registry, user_data.path(), true);
        assert!(login.get(), "the same exe and --hidden, whatever the case");
        login.sync().unwrap();
        assert_eq!(registry.writes(), 0);
        // Another exe, or another argument, is not ours.
        registry.set_string(RUN_KEY, NAME, r#""C:\other\Tjommie.exe" --hidden"#);
        assert!(!login.get());
        registry.set_string(RUN_KEY, NAME, &format!("\"{EXE}\""));
        assert!(!login.get());
    }
}
