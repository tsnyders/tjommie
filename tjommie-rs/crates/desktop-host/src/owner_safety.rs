//! Keeps every build but production off the owner's data (spec §4): %LOCALAPPDATA%\Tjommie (the data folder) and
//! %APPDATA%\Tjommie (Electron's profile), and anything inside them, however the path is spelled. They also keep to
//! local drives and refuse to start when LOCALAPPDATA or APPDATA is unset. Disposable identities (e2e, the update test)
//! must also stay under %TEMP%.
//!
//! Electron's `owner-safety.ts` is a different guard: a test-only snapshot of the owner's folders and registry before
//! and after a run. That one is ported when a Phase 3 test runs an installer or a real app (M8).

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};

use crate::identity::{DataRoot, Identity};

#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Refuses `root` unless `identity` may use it: production may use any root; every other identity is refused the
/// owner's folders, and a disposable identity anything outside %TEMP%.
pub fn assert_root_allowed(root: &Path, identity: &Identity) -> Result<(), Refused> {
    let folder = |name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let (local, roaming) = (folder("LOCALAPPDATA"), folder("APPDATA"));
    check(root, identity, local.as_deref(), roaming.as_deref())?;
    if identity.data_root == DataRoot::Disposable && !within(root, &std::env::temp_dir()) {
        return Err(Refused(format!(
            "{} keeps its data under %TEMP%, not in {}",
            identity.product,
            root.display()
        )));
    }
    Ok(())
}

/// A fresh folder under %TEMP% for one test, removed when dropped.
pub fn test_root() -> std::io::Result<tempfile::TempDir> {
    let root = tempfile::Builder::new().prefix("tjommie-test-").tempdir()?;
    if within(root.path(), &std::env::temp_dir()) {
        Ok(root)
    } else {
        Err(std::io::Error::other(format!(
            "{} is not under %TEMP%",
            root.path().display()
        )))
    }
}

fn check(
    root: &Path,
    identity: &Identity,
    local: Option<&Path>,
    roaming: Option<&Path>,
) -> Result<(), Refused> {
    if identity.name == "production" {
        return Ok(());
    }
    // Unknown owner folders rule nothing out (review F3).
    let (Some(local), Some(roaming)) = (local, roaming) else {
        return Err(Refused(format!(
            "{} needs LOCALAPPDATA and APPDATA set to keep off the owner's data",
            identity.product
        )));
    };
    // A share reaches the owner's folder under another name (\\localhost\C$\...), so local drives only (review F2).
    let local_drive = matches!(
        resolve(root).components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    );
    if !local_drive {
        return Err(Refused(format!(
            "{} keeps its data on a local drive, not {}",
            identity.product,
            root.display()
        )));
    }
    for owner in [local, roaming].map(|base| base.join("Tjommie")) {
        if within(root, &owner) {
            return Err(Refused(format!(
                "{} must not use the owner's {} (resolved from {})",
                identity.product,
                owner.display(),
                root.display()
            )));
        }
    }
    Ok(())
}

/// Whether `path` is `base` or inside it, compared as Windows resolves them: absolute, `..` folded, links and short
/// names followed where the path exists, and case-insensitive.
fn within(path: &Path, base: &Path) -> bool {
    let path = components(&resolve(path));
    let base = components(&resolve(base));
    path.starts_with(&base)
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// The path as Windows would open it: the deepest part that exists, canonicalized, with the rest appended.
fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest: Vec<OsString> = Vec::new();
    let mut existing = absolute.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            return rest
                .iter()
                .rev()
                .fold(real, |resolved, part| resolved.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return absolute,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{DEV, E2E, PRODUCTION};

    #[test]
    fn refuses_prod_root_for_dev() {
        let base = tempfile::tempdir().unwrap();
        let local = base.path().join("Local");
        let roaming = base.path().join("Roaming");
        std::fs::create_dir_all(local.join("Tjommie").join("data")).unwrap();
        std::fs::create_dir_all(local.join("Tjommie-rs")).unwrap();
        let refused = |root: &Path| check(root, &DEV, Some(&local), Some(&roaming)).is_err();
        // The owner's data folder and Electron's profile, however the path is spelled.
        assert!(refused(&local.join("Tjommie")));
        assert!(refused(&local.join("TJOMMIE")));
        assert!(refused(&local.join("Tjommie").join("data")));
        assert!(refused(
            &local.join("Tjommie-rs").join("..").join("Tjommie")
        ));
        assert!(refused(&local.join("Tjommie").join("not-made-yet")));
        assert!(refused(&roaming.join("Tjommie")));
        // A folder whose name only starts with Tjommie is not the owner's.
        assert!(!refused(&local.join("Tjommie-rs")));
        // Production may use its own root.
        assert!(
            check(
                &local.join("Tjommie"),
                &PRODUCTION,
                Some(&local),
                Some(&roaming)
            )
            .is_ok()
        );
    }

    #[test]
    fn test_root_is_fresh_and_under_temp() {
        let first = test_root().unwrap();
        let second = test_root().unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir());
        assert!(within(first.path(), &std::env::temp_dir()));
        assert!(assert_root_allowed(first.path(), &E2E).is_ok());
        // A disposable identity outside %TEMP%.
        assert!(assert_root_allowed(Path::new("C:\\Tjommie-e2e"), &E2E).is_err());
    }

    /// Review F3: without LOCALAPPDATA or APPDATA the owner's folders are unknown, so a non-production root is refused
    /// (Electron's owner-safety.ts threw).
    #[test]
    fn refuses_without_the_owner_folders_known() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("Tjommie-rs");
        assert!(check(&root, &DEV, None, Some(base.path())).is_err());
        assert!(check(&root, &DEV, Some(base.path()), None).is_err());
        assert!(check(&root, &PRODUCTION, None, None).is_ok());
    }

    /// Review F2: the owner's folder through the loopback admin share is still the owner's folder. Nothing is written.
    #[test]
    fn refuses_unc_alias() {
        let base = tempfile::tempdir().unwrap();
        let local = base.path().join("Local");
        let roaming = base.path().join("Roaming");
        std::fs::create_dir_all(local.join("Tjommie")).unwrap();
        let owner = local.join("Tjommie").display().to_string();
        let (drive, rest) = owner.split_once(":\\").unwrap();
        for alias in [
            format!(r"\\localhost\{drive}$\{rest}"),
            format!(r"\\127.0.0.1\{drive}$\{rest}"),
            format!(r"\\?\UNC\localhost\{drive}$\{rest}"),
        ] {
            assert!(
                check(Path::new(&alias), &DEV, Some(&local), Some(&roaming)).is_err(),
                "{alias}"
            );
        }
        assert!(
            check(
                &local.join("Tjommie-rs"),
                &DEV,
                Some(&local),
                Some(&roaming)
            )
            .is_ok()
        );
    }
}
