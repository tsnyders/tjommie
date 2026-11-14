//! Where this run keeps what M6 writes besides the daemon's own data root: Electron's user-data folder (the
//! `tray-hint-shown` and `login-item-set` markers), the speech folder (the model and the temporary recordings) and the
//! update download folder. They come from the identity, never from a caller's guess, and each one goes through
//! `owner_safety::assert_root_allowed`, so no dev, e2e or update-test build can be handed the owner's
//! `%LOCALAPPDATA%\Tjommie` or `%APPDATA%\Tjommie`, however the path is spelled. Production may use them: they are its own.
//!
//! The rules are Electron's (`daemon-process.ts` `userDataFor`) and spec §4:
//! - **user data:** `TJOMMIE_USER_DATA` if set; else production uses Electron's default, `%APPDATA%\<product>`, and every
//!   other identity `<data root>\electron`.
//! - **speech:** production keeps Electron's `<user data>\speech`, so there is no second model download and a rollback to R1
//!   still finds it; every other identity uses `<data root>\speech`.
//! - **update downloads:** `%TEMP%`.

use std::path::{Path, PathBuf};

use crate::identity::{Identity, Kind};
use crate::owner_safety::{Refused, assert_root_allowed};

/// The environment the folders depend on, passed in so a test doesn't need the real one.
#[derive(Debug, Default, Clone, Copy)]
pub struct DirsEnv<'a> {
    /// `TJOMMIE_USER_DATA`.
    pub user_data: Option<&'a Path>,
    /// `%APPDATA%`.
    pub roaming: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    pub data_root: PathBuf,
    pub user_data: PathBuf,
    /// Holds `models\` and `sessions\`.
    pub speech: PathBuf,
    pub download_root: PathBuf,
}

impl AppDirs {
    /// Derives the folders from the identity and refuses any this identity may not use.
    pub fn resolve(
        identity: &Identity,
        data_root: &Path,
        env: &DirsEnv<'_>,
    ) -> Result<AppDirs, Refused> {
        let own_user_data = env.user_data.filter(|path| !path.as_os_str().is_empty());
        let production = identity.kind == Kind::Production;
        let user_data = match (own_user_data, production) {
            (Some(path), _) => path.to_path_buf(),
            (None, true) => env
                .roaming
                .ok_or_else(|| Refused("APPDATA is not set".into()))?
                .join(identity.product),
            (None, false) => data_root.join("electron"),
        };
        let speech = if production {
            user_data.join("speech")
        } else {
            data_root.join("speech")
        };
        let dirs = AppDirs {
            data_root: data_root.to_path_buf(),
            user_data,
            speech,
            download_root: std::env::temp_dir(),
        };
        for path in [
            &dirs.data_root,
            &dirs.user_data,
            &dirs.speech,
            &dirs.download_root,
        ] {
            assert_root_allowed(path, identity)?;
        }
        Ok(dirs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{DEV, E2E, PRODUCTION, UPDATE_TEST};

    fn within(path: &Path, base: &Path) -> bool {
        path.starts_with(base)
    }

    #[test]
    fn production_uses_electrons_folders_so_a_rollback_finds_them() {
        let roaming = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let env = DirsEnv {
            user_data: None,
            roaming: Some(roaming.path()),
        };
        let dirs = AppDirs::resolve(&PRODUCTION, data.path(), &env).unwrap();
        assert_eq!(dirs.user_data, roaming.path().join("Tjommie"));
        assert_eq!(dirs.speech, roaming.path().join("Tjommie").join("speech"));
        // A smoke run's own user-data folder is honoured, and its speech folder follows it.
        let own = tempfile::tempdir().unwrap();
        let env = DirsEnv {
            user_data: Some(own.path()),
            roaming: Some(roaming.path()),
        };
        let dirs = AppDirs::resolve(&PRODUCTION, data.path(), &env).unwrap();
        assert_eq!(dirs.user_data, own.path());
        assert_eq!(dirs.speech, own.path().join("speech"));
        // Without %APPDATA% production can't find Electron's folder and says so.
        assert!(AppDirs::resolve(&PRODUCTION, data.path(), &DirsEnv::default()).is_err());
    }

    // The test the review asked for: no M6 path of a non-production build can be the owner's folder.
    #[test]
    fn a_non_production_build_keeps_every_path_under_its_own_root() {
        let roaming = tempfile::tempdir().unwrap();
        for identity in [DEV, E2E, UPDATE_TEST] {
            let data = tempfile::tempdir().unwrap();
            // Even with %APPDATA% in view, it is never where a non-production build puts anything.
            let env = DirsEnv {
                user_data: None,
                roaming: Some(roaming.path()),
            };
            let dirs = AppDirs::resolve(&identity, data.path(), &env).unwrap();
            assert_eq!(
                dirs.user_data,
                data.path().join("electron"),
                "{}",
                identity.name
            );
            assert_eq!(dirs.speech, data.path().join("speech"), "{}", identity.name);
            for path in [&dirs.user_data, &dirs.speech] {
                assert!(
                    within(path, data.path()),
                    "{}: {}",
                    identity.name,
                    path.display()
                );
                assert!(!within(path, roaming.path()), "{}", identity.name);
            }
            assert_eq!(dirs.download_root, std::env::temp_dir());
        }
    }

    #[test]
    fn a_non_production_build_is_refused_the_owners_folders_however_they_are_reached() {
        let (Some(local), Some(roaming)) = (
            std::env::var_os("LOCALAPPDATA"),
            std::env::var_os("APPDATA"),
        ) else {
            return; // a machine with no profile folders has no owner folders to protect
        };
        let (local, roaming) = (PathBuf::from(local), PathBuf::from(roaming));
        let scratch = tempfile::tempdir().unwrap();
        let spellings = [
            local.join("Tjommie"),
            local.join("Tjommie").join("logs"),
            roaming.join("Tjommie"),
            roaming.join("Tjommie").join("speech"),
            // `..` and another case reach the same folder.
            roaming.join("Tjommie-rs").join("..").join("Tjommie"),
            PathBuf::from(roaming.join("TJOMMIE").to_string_lossy().to_uppercase()),
        ];
        for identity in [DEV, E2E, UPDATE_TEST] {
            for owner in &spellings {
                // As the data root.
                assert!(
                    AppDirs::resolve(&identity, owner, &DirsEnv::default()).is_err(),
                    "{} accepted {} as its data root",
                    identity.name,
                    owner.display()
                );
                // As TJOMMIE_USER_DATA, with an innocent data root.
                let env = DirsEnv {
                    user_data: Some(owner),
                    roaming: Some(&roaming),
                };
                assert!(
                    AppDirs::resolve(&identity, scratch.path(), &env).is_err(),
                    "{} accepted {} as its user data",
                    identity.name,
                    owner.display()
                );
            }
        }
        // Production may, because they are its own.
        let env = DirsEnv {
            user_data: None,
            roaming: Some(&roaming),
        };
        assert!(AppDirs::resolve(&PRODUCTION, &local.join("Tjommie"), &env).is_ok());
    }

    #[test]
    fn a_disposable_identity_must_stay_under_temp() {
        // The update test and e2e keep their data under %TEMP%; a root elsewhere (not the owner's, just not disposable)
        // is refused too. `C:\` stands in for "not under %TEMP%".
        for identity in [E2E, UPDATE_TEST] {
            assert!(
                AppDirs::resolve(
                    &identity,
                    Path::new(r"C:\tjommie-elsewhere"),
                    &DirsEnv::default()
                )
                .is_err()
            );
        }
    }
}
