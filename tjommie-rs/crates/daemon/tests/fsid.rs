//! `daemon::fsid`: where a path really is, compared by file identity (volume serial and 128-bit file id) of its
//! nearest existing ancestor plus the missing rest. Spellings, `\\?\`, UNC loopback, junctions, `subst` drives and
//! volume-GUID paths all reach the same folder. Only temp folders made here are used.
#![cfg(windows)]

use daemon::fsid::{is_within, same_location};
use daemon::spawn;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use windows::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
use windows::core::HSTRING;

fn junction(link: &Path, target: &Path) {
    let output = spawn::command("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mklink /J: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `C:\x` as `<prefix><letter>$\x`, for the loopback admin share spellings.
fn share(path: &Path, prefix: &str) -> PathBuf {
    let text = path.to_str().unwrap();
    PathBuf::from(format!("{prefix}{}$\\{}", &text[..1], &text[3..]))
}

fn loopback(path: &Path) -> [PathBuf; 3] {
    [
        share(path, r"\\localhost\"),
        share(path, r"\\127.0.0.1\"),
        share(path, r"\\?\UNC\localhost\"),
    ]
}

fn tree() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("Root");
    fs::create_dir_all(root.join("child")).unwrap();
    fs::create_dir(dir.path().join("Root2")).unwrap();
    (dir, root)
}

#[test]
fn same_location_sees_through_spelling_prefixes_loopback_shares_and_junctions() {
    let (dir, root) = tree();
    let spelled = root.to_str().unwrap();
    let mut aliases = vec![
        root.clone(),
        PathBuf::from(spelled.to_uppercase()),
        PathBuf::from(spelled.replace('\\', "/")),
        PathBuf::from(format!(r"\\?\{spelled}")),
    ];
    aliases.extend(loopback(&root));
    let link = dir.path().join("link");
    junction(&link, &root);
    aliases.push(link.clone());
    for alias in &aliases {
        assert!(same_location(alias, &root).unwrap(), "{}", alias.display());
    }
    // Missing paths compare by their nearest existing ancestor plus the rest, without case.
    assert!(
        same_location(
            &root.join("missing").join("Deep"),
            &link.join("MISSING").join("deep")
        )
        .unwrap()
    );
    assert!(!same_location(&root.join("missing"), &root.join("other")).unwrap());
    assert!(!same_location(&root, &root.join("child")).unwrap());
    assert!(!same_location(&root, &dir.path().join("Root2")).unwrap());
}

#[test]
fn is_within_resolves_both_sides_and_takes_missing_children() {
    let (dir, root) = tree();
    let link = dir.path().join("link");
    junction(&link, &root);
    for (path, inside) in [
        (root.join("child"), true),
        (root.clone(), true),
        (root.join("a").join("b").join("c"), true),
        (link.join("child"), true),
        (link.join("x").join("y"), true),
        (dir.path().join("Root2"), false),
        (dir.path().join("Root2").join("x"), false),
        (dir.path().to_path_buf(), false),
    ] {
        assert_eq!(
            is_within(&path, &root).unwrap(),
            inside,
            "{}",
            path.display()
        );
    }
    for alias in loopback(&root.join("deep").join("missing")) {
        assert!(is_within(&alias, &root).unwrap(), "{}", alias.display());
    }
    assert!(is_within(&root.join("child"), &link).unwrap());
    assert!(!is_within(&root, &root.join("child")).unwrap());
    // A missing root: only its own missing descendants are inside it.
    let gone = root.join("gone");
    assert!(is_within(&link.join("GONE").join("x"), &gone).unwrap());
    assert!(!is_within(&root.join("child"), &gone).unwrap());
}

#[test]
fn subst_drives_and_volume_guid_paths_reach_the_same_folder() {
    let (_dir, root) = tree();
    // Volume GUID spelling of the same folder.
    let text = root.to_str().unwrap();
    let mut volume = [0u16; 64];
    // SAFETY: the buffer is local and sized.
    unsafe { GetVolumeNameForVolumeMountPointW(&HSTRING::from(&text[..3]), &mut volume) }.unwrap();
    let volume = String::from_utf16_lossy(&volume[..volume.iter().position(|c| *c == 0).unwrap()]);
    let by_guid = PathBuf::from(format!("{volume}{}", &text[3..]));
    assert!(
        same_location(&by_guid, &root).unwrap(),
        "{}",
        by_guid.display()
    );
    assert!(is_within(&by_guid.join("child"), &root).unwrap());

    // A `subst` drive letter for the folder, on a free letter, removed again afterwards.
    let letter = ('J'..='Y')
        .rev()
        .find(|l| !Path::new(&format!("{l}:\\")).exists())
        .expect("a free drive letter");
    let drive = format!("{letter}:");
    let made = spawn::command("subst")
        .arg(&drive)
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "subst: {}",
        String::from_utf8_lossy(&made.stdout)
    );
    let checks = std::panic::catch_unwind(|| {
        let mapped = PathBuf::from(format!("{drive}\\"));
        assert!(same_location(&mapped, &root).unwrap());
        assert!(is_within(&mapped.join("child"), &root).unwrap());
        assert!(is_within(&mapped.join("missing").join("x"), &root).unwrap());
    });
    let removed = spawn::command("subst")
        .arg(&drive)
        .arg("/d")
        .output()
        .unwrap();
    assert!(removed.status.success());
    if let Err(panic) = checks {
        std::panic::resume_unwind(panic);
    }
}
