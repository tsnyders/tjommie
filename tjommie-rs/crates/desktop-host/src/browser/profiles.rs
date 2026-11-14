//! `native-browser-profiles.ts`: the bots' profile folders on disk. Every path is checked to be a physical folder
//! directly inside the owned root, and nothing is ever deleted through a link.
//!
//! WebView2 keeps a profile named `<hash>` in the folder `WV2Profile_<hash>` of its `EBWebView` folder (the plan
//! assumed the bare hash; the runtime decides). Everything here takes and returns the hash and adds the prefix itself.

use std::path::{Path, PathBuf};

/// Why a profile folder was refused or could not be handled.
#[derive(Debug)]
pub enum ProfileError {
    /// The name is not a 64-hex profile hash.
    InvalidName,
    /// The root, a profile or something inside it is a link or not a folder.
    NotPhysical(&'static str),
    /// The folder resolved outside the root, or to another name.
    Escaped,
    /// The folder changed between the check and the delete.
    Changed,
    Io(std::io::Error),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName => f.write_str("Invalid browser profile directory."),
            Self::NotPhysical(what) => f.write_str(what),
            Self::Escaped => f.write_str("Browser profile escaped its owned root."),
            Self::Changed => f.write_str("Browser profile changed during cleanup."),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl From<std::io::Error> for ProfileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// What WebView2 puts in front of a profile's name to make its folder name.
const FOLDER_PREFIX: &str = "WV2Profile_";

/// The folder WebView2 keeps the profile `hash` in.
pub fn folder_name(hash: &str) -> String {
    format!("{FOLDER_PREFIX}{hash}")
}

/// A profile is named by the bot's 64-hex hash.
pub fn is_profile_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Windows compares paths without regard to case.
fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
}

fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// The canonical root, or `None` when it does not exist. A link or a file is refused.
fn profile_root(root: &Path) -> Result<Option<PathBuf>, ProfileError> {
    match std::fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            Ok(Some(std::fs::canonicalize(root)?))
        }
        Ok(_) => Err(ProfileError::NotPhysical(
            "Browser profile root must be an owned physical directory.",
        )),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The hashes of the profile folders directly inside `root`. A missing root has none.
pub fn profile_directories(root: &Path) -> Result<Vec<String>, ProfileError> {
    let Some(base) = profile_root(root)? else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(base)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if let Ok(name) = entry.file_name().into_string()
            && let Some(hash) = name.strip_prefix(FOLDER_PREFIX)
            && is_profile_hash(hash)
            && (kind.is_dir() || kind.is_symlink())
        {
            found.push(hash.to_string());
        }
    }
    Ok(found)
}

/// The canonical path of the folder of profile `hash` inside `root`, or `None` when it does not exist.
pub fn checked_profile(root: &Path, hash: &str) -> Result<Option<PathBuf>, ProfileError> {
    if !is_profile_hash(hash) {
        return Err(ProfileError::InvalidName);
    }
    let Some(base) = profile_root(root)? else {
        return Ok(None);
    };
    let path = base.join(folder_name(hash));
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(ProfileError::NotPhysical(
            "Browser profile must be an owned physical directory.",
        ));
    }
    let canonical = std::fs::canonicalize(&path)?;
    check_contained(&base, &canonical, hash)?;
    Ok(Some(canonical))
}

/// Whether `child` is the folder of profile `hash` directly inside `base`, as Windows sees paths (case-insensitive).
pub fn check_contained(base: &Path, child: &Path, hash: &str) -> Result<(), ProfileError> {
    let in_base = child.parent().is_some_and(|parent| same_path(parent, base));
    let named = child
        .file_name()
        .is_some_and(|name| same_path(Path::new(name), Path::new(&folder_name(hash))));
    if in_base && named {
        Ok(())
    } else {
        Err(ProfileError::Escaped)
    }
}

fn reject_links(path: &Path) -> Result<(), ProfileError> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(ProfileError::NotPhysical(
                "Browser profile cleanup refuses symbolic links.",
            ));
        }
        if kind.is_dir() {
            reject_links(&entry.path())?;
        }
    }
    Ok(())
}

/// A Windows file lock or a refused delete: the files stay, to be removed at a later start.
fn is_locked(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION, ERROR_DIR_NOT_EMPTY
    matches!(error.raw_os_error(), Some(5 | 32 | 33 | 145))
        || matches!(
            error.kind(),
            ErrorKind::PermissionDenied | ErrorKind::DirectoryNotEmpty
        )
}

/// Removes the profile folder: never through a link, never retrying a lock. `Ok(false)` means Windows still holds
/// files in it; it is cleared and retried at the next start.
pub fn remove_profile(root: &Path, hash: &str) -> Result<bool, ProfileError> {
    remove_profile_with(root, hash, |path| std::fs::remove_dir_all(path))
}

/// [`remove_profile`] with the folder removal supplied, so a test can fail it.
pub fn remove_profile_with(
    root: &Path,
    hash: &str,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<bool, ProfileError> {
    let Some(path) = checked_profile(root, hash)? else {
        return Ok(true);
    };
    reject_links(&path)?;
    // Recheck the direct target immediately before the recursive delete.
    match checked_profile(root, hash)? {
        Some(current) if same_path(&current, &path) => {}
        _ => return Err(ProfileError::Changed),
    }
    match remove(&path) {
        Ok(()) => Ok(true),
        Err(error) if is_locked(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn hash(c: char) -> String {
        c.to_string().repeat(64)
    }

    #[test]
    fn only_enumerates_hashed_profiles_and_removes_the_checked_owned_child() {
        let root = tempfile::tempdir().unwrap();
        let name = hash('a');
        fs::create_dir(root.path().join(folder_name(&name))).unwrap();
        fs::create_dir(root.path().join("not-a-profile")).unwrap();
        // The bare hash is not what WebView2 names a profile's folder.
        fs::create_dir(root.path().join(hash('9'))).unwrap();
        fs::write(
            root.path().join(folder_name(&hash('8'))),
            "a file, not a folder",
        )
        .unwrap();
        fs::write(
            root.path().join(folder_name(&name)).join("cookie-test"),
            "synthetic",
        )
        .unwrap();
        assert_eq!(
            profile_directories(root.path()).unwrap(),
            vec![name.clone()]
        );
        assert!(remove_profile(root.path(), &name).unwrap());
        assert!(checked_profile(root.path(), &name).unwrap().is_none());
        assert!(matches!(
            checked_profile(root.path(), "../outside"),
            Err(ProfileError::InvalidName)
        ));
        assert!(
            !is_profile_hash(&"A".repeat(64)),
            "upper case is not a hash"
        );
        assert!(!is_profile_hash(&"a".repeat(63)));
        assert!(is_profile_hash(&name));
    }

    #[test]
    fn a_missing_root_has_no_profiles() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("nothing-here");
        assert!(profile_directories(&missing).unwrap().is_empty());
        assert!(checked_profile(&missing, &hash('a')).unwrap().is_none());
        assert!(remove_profile(&missing, &hash('a')).unwrap());
    }

    #[test]
    fn accepts_a_differently_cased_windows_root_and_canonical_child() {
        let root = tempfile::tempdir().unwrap();
        let name = hash('d');
        fs::create_dir(root.path().join(folder_name(&name))).unwrap();
        let supplied = PathBuf::from(root.path().to_string_lossy().to_uppercase());
        assert_eq!(profile_directories(&supplied).unwrap(), vec![name.clone()]);
        let canonical = fs::canonicalize(root.path().join(folder_name(&name))).unwrap();
        assert_eq!(
            checked_profile(&supplied, &name)
                .unwrap()
                .map(|p| p.to_string_lossy().to_lowercase()),
            Some(canonical.to_string_lossy().to_lowercase())
        );
        // Windows may spell the parent differently between the root and the child.
        let base = fs::canonicalize(root.path()).unwrap();
        let shouted = PathBuf::from(
            base.join(folder_name(&name))
                .to_string_lossy()
                .to_uppercase(),
        );
        assert!(check_contained(&base, &shouted, &name).is_ok());
        assert!(remove_profile(&supplied, &name).unwrap());
        assert!(checked_profile(root.path(), &name).unwrap().is_none());
    }

    #[test]
    fn rejects_children_outside_the_root_or_under_another_name() {
        let base = PathBuf::from(r"C:\owned");
        let name = hash('e');
        assert!(matches!(
            check_contained(&base, &base.join("outside").join(folder_name(&name)), &name),
            Err(ProfileError::Escaped)
        ));
        assert!(matches!(
            check_contained(&base, &base.join(folder_name(&hash('f'))), &name),
            Err(ProfileError::Escaped)
        ));
        assert!(check_contained(&base, &base.join(folder_name(&name)), &name).is_ok());
    }

    #[test]
    fn refuses_root_profile_and_nested_junctions_without_touching_their_destination() {
        let root = tempfile::tempdir().unwrap();
        let name = hash('b');
        let owned = root.path().join("owned");
        let outside = root.path().join("outside");
        fs::create_dir(&owned).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "outside").unwrap();
        // A junction needs no privilege on Windows, unlike a symlink: `mklink /J`.
        let junction = |link: &Path, target: &Path| {
            let made = crate::spawn::command("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(made.status.success(), "mklink failed");
        };
        let alias = root.path().join("alias");
        junction(&alias, &outside);
        assert!(matches!(
            profile_directories(&alias),
            Err(ProfileError::NotPhysical(_))
        ));
        junction(&owned.join(folder_name(&name)), &outside);
        assert!(matches!(
            remove_profile(&owned, &name),
            Err(ProfileError::NotPhysical(_))
        ));
        fs::remove_dir(owned.join(folder_name(&name))).unwrap();
        fs::create_dir(owned.join(folder_name(&name))).unwrap();
        junction(&owned.join(folder_name(&name)).join("nested"), &outside);
        assert!(matches!(
            remove_profile(&owned, &name),
            Err(ProfileError::NotPhysical(_))
        ));
        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "outside");
        fs::remove_dir(owned.join(folder_name(&name)).join("nested")).unwrap();
        fs::remove_dir(&alias).unwrap();
    }

    #[test]
    fn defers_a_windows_file_lock_without_retries_and_surfaces_other_errors() {
        use std::io::{Error, ErrorKind};
        let root = tempfile::tempdir().unwrap();
        let name = hash('c');
        fs::create_dir(root.path().join(folder_name(&name))).unwrap();
        let locked = remove_profile_with(root.path(), &name, |_| {
            // ERROR_SHARING_VIOLATION
            Err(Error::from_raw_os_error(32))
        });
        assert!(!locked.unwrap());
        let denied = remove_profile_with(root.path(), &name, |_| {
            Err(Error::from(ErrorKind::PermissionDenied))
        });
        assert!(!denied.unwrap());
        let broken = remove_profile_with(root.path(), &name, |_| Err(Error::other("I/O failure")));
        assert!(matches!(broken, Err(ProfileError::Io(_))));
        assert!(
            root.path().join(folder_name(&name)).exists(),
            "nothing was removed"
        );
    }
}
