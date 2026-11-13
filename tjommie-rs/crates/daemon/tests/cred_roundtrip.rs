//! T2.1 Credential Manager parity with `account-credentials.ts`, `vault-storage.ts`, `remote-identity.ts` and
//! `relay-identity.ts` as 0.4.2 shipped them: the target names, blob encoding and entry shape are pinned by
//! `fixtures/compat/credential-entries.json` (captured from the TypeScript modules), so no Node runs here. Tests that
//! touch Credential Manager use one random test namespace, refuse the production namespace, clean up by namespace, and
//! fail if any other `Tjommie/*` target changed or was left behind.
#![cfg(windows)]

use daemon::spawn;
use daemon::win::cred::{self, CredentialError, Namespace, validate_api_key};
use serde_json::Value;
use std::collections::BTreeMap;
use std::os::windows::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_TYPE_GENERIC, CREDENTIALW, CredEnumerateW, CredFree, CredReadW,
};
use windows::Win32::Storage::FileSystem::GetShortPathNameW;
use windows::core::{HSTRING, PWSTR};

// ---- Pure checks (no Credential Manager access; these run in CI too) ----

#[test]
fn references_are_opaque_and_isolated_by_installation_and_account() {
    let one = Namespace::new(Path::new("C:/Tjommie/one")).unwrap();
    let two = Namespace::new(Path::new("C:/Tjommie/two")).unwrap();
    let a = one.account_target("account-a").unwrap();
    assert_ne!(a, one.account_target("account-b").unwrap());
    assert_ne!(a, two.account_target("account-a").unwrap());
    assert!(!a.contains("account-a"));
}

/// The targets the TypeScript 0.4.2 modules asked for on `fixtures/compat/credential-entries.json`'s root.
#[test]
fn targets_match_the_captured_typescript_entries() {
    let captured = captured();
    let ns = Namespace::new(Path::new(captured["root"].as_str().unwrap())).unwrap();
    let account = &captured["account"];
    assert_eq!(
        ns.account_target(account["id"].as_str().unwrap()).unwrap(),
        account["target"].as_str().unwrap()
    );
    let login = &captured["login"];
    assert_eq!(
        ns.login_target(login["reference"].as_str().unwrap())
            .unwrap(),
        login["target"].as_str().unwrap()
    );
    assert_eq!(
        ns.remote_host_target(),
        captured["remoteHostTarget"].as_str().unwrap()
    );
    assert_eq!(
        ns.relay_login_target(),
        captured["relayLoginTarget"].as_str().unwrap()
    );
}

/// The same targets, spelled out, for two spellings of the root (values printed by the TypeScript 0.4.2 modules).
#[test]
fn targets_match_typescript_golden_values() {
    for root in [
        r"C:\Users\Fixture\AppData\Local\Tjommie-rs",
        "c:/users/fixture/appdata/local/TJOMMIE-RS/",
    ] {
        let ns = Namespace::new(Path::new(root)).unwrap();
        assert_eq!(ns.short(), "c7b7c5a986b63824c0803169");
        assert_eq!(ns.long(), "c7b7c5a986b63824c0803169405cdcd6");
        assert_eq!(
            ns.account_target("account-a").unwrap(),
            "Tjommie/c7b7c5a986b63824c0803169/account/fc164f8250803ea8d41834f1de85821035d27d3747e83610789e0f8e5313b9c3"
        );
        assert_eq!(
            ns.login_target("0f8fad5b-d9cb-469f-a165-70867728950e")
                .unwrap(),
            "Tjommie/c7b7c5a986b63824c0803169/login/0f8fad5b-d9cb-469f-a165-70867728950e"
        );
        assert_eq!(
            ns.remote_host_target(),
            "Tjommie/c7b7c5a986b63824c0803169405cdcd6/remote-host"
        );
        assert_eq!(
            ns.relay_login_target(),
            "Tjommie/c7b7c5a986b63824c0803169405cdcd6/relay-login"
        );
    }
}

/// Node's `path.resolve` keeps the separator after a bare share, as after a bare drive.
#[test]
fn unc_share_roots_match_typescript() {
    for root in [r"\\server\share", r"\\SERVER\Share\"] {
        let ns = Namespace::new(Path::new(root)).unwrap();
        assert_eq!(ns.long(), "af69ebc65f06522ece8ff99f0cc6dc33", "{root}");
    }
}

/// Windows drops a trailing dot or space from the last name when resolving and Node keeps it, so such a root would
/// get a different namespace than 0.4.2 gives it. Refusing it makes start-up fail loudly instead.
#[test]
fn roots_whose_last_name_ends_in_a_dot_or_space_are_refused() {
    for root in [r"D:\Tjommie ", r"D:\Tjommie.", r"D:\Tjommie \", "Tjommie."] {
        let error = Namespace::new(Path::new(root)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{root}");
    }
    // "." and ".." resolve the same way in both.
    let dotted = Namespace::new(Path::new(r"C:\Tjommie\one\.\..\two")).unwrap();
    assert_eq!(
        dotted.short(),
        Namespace::new(Path::new("C:/Tjommie/two")).unwrap().short()
    );
}

/// The harness guard judges what a root is on disk, not how it is spelled: `\\?\`, case, slashes, junctions and
/// 8.3 names all reach production. It only looks the production root up; it never opens anything inside it.
#[test]
fn production_roots_are_recognised_by_what_they_resolve_to() {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
    let production = local.join("Tjommie");
    let spelled = production.to_str().unwrap();
    for alias in [
        production.clone(),
        PathBuf::from(format!(r"\\?\{spelled}")),
        PathBuf::from(spelled.to_uppercase().replace('\\', "/")),
        production.join("data"),
        local.clone(),
    ] {
        assert!(cred::is_production_root(&alias), "{}", alias.display());
    }
    // The loopback admin share reaches the same folder (review MAJOR 1).
    let rest = &spelled[3..];
    let drive = &spelled[..1];
    for alias in [
        format!(r"\\localhost\{drive}$\{rest}"),
        format!(r"\\127.0.0.1\{drive}$\{rest}"),
        format!(r"\\?\UNC\localhost\{drive}$\{rest}"),
    ] {
        assert!(cred::is_production_root(Path::new(&alias)), "{alias}");
    }
    let dir = tempfile::TempDir::new().unwrap();
    assert!(!cred::is_production_root(dir.path()));
    assert!(!cred::is_production_root(
        &dir.path().join("not-yet-created")
    ));

    // A junction to %LOCALAPPDATA% makes <junction>\Tjommie the production root.
    let link = dir.path().join("link");
    let made = spawn::command("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(&link)
        .arg(&local)
        .output()
        .unwrap();
    assert!(made.status.success());
    let through_link = cred::is_production_root(&link.join("Tjommie"));
    std::fs::remove_dir(&link).unwrap(); // removes only the link
    assert!(through_link, "junction alias");

    // An 8.3 name, where the volume has them.
    if let Some(short) = short_name(&local) {
        assert!(
            cred::is_production_root(&short.join("Tjommie")),
            "{}",
            short.display()
        );
    }
}

fn short_name(path: &Path) -> Option<PathBuf> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: both buffers are local and sized.
    let length =
        unsafe { GetShortPathNameW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut buffer)) }
            as usize;
    let short = PathBuf::from(String::from_utf16_lossy(buffer.get(..length)?));
    (length > 0 && short != path).then_some(short)
}

#[test]
fn account_ids_are_bounded_in_utf16_units_like_javascript() {
    let ns = Namespace::new(Path::new("C:/Tjommie/one")).unwrap();
    assert!(ns.account_target(&"a".repeat(256)).is_ok());
    assert!(ns.account_target(&"😀".repeat(128)).is_ok()); // 256 UTF-16 units
    for bad in [String::new(), "a".repeat(257), "😀".repeat(129)] {
        assert_eq!(ns.account_target(&bad), Err(CredentialError::Unavailable));
    }
}

#[test]
fn login_references_must_look_like_uuids() {
    let ns = Namespace::new(Path::new("C:/Tjommie/one")).unwrap();
    assert!(
        ns.login_target("0f8fad5b-d9cb-469f-a165-70867728950e")
            .is_some()
    );
    for bad in [
        "",
        "0F8FAD5B-D9CB-469F-A165-70867728950E",
        "0f8fad5b-d9cb-469f-a165-70867728950",
        "../../x",
    ] {
        assert_eq!(ns.login_target(bad), None, "{bad}");
    }
}

#[test]
fn api_keys_follow_the_typescript_rules() {
    let too_long = "a".repeat(2561);
    let too_many_bytes = "é".repeat(1281);
    for bad in [
        "",
        " secret",
        "secret ",
        "a\nb",
        "a b",
        "a\u{7f}",
        "\u{feff}k",
        "k\u{3000}",
        "k\u{a0}",
        &too_long,
        &too_many_bytes,
    ] {
        assert_eq!(
            validate_api_key(bad),
            Err(CredentialError::InvalidKey),
            "{bad:?}"
        );
    }
    // JavaScript's trim() leaves U+0085 alone, and the control-character check stops at U+007F.
    for good in ["synthetic-key", "k\u{85}", &"é".repeat(1280)] {
        assert_eq!(validate_api_key(good), Ok(()), "{good:?}");
    }
}

#[test]
fn errors_carry_only_the_fixed_typescript_messages() {
    assert_eq!(
        CredentialError::Unavailable.to_string(),
        "Tjommie could not access Windows Credential Manager. The API key was not exposed or saved to a file."
    );
    assert_eq!(
        CredentialError::InvalidKey.to_string(),
        "Enter a single API key of at most 2560 UTF-8 bytes, without whitespace."
    );
}

// ---- Credential Manager (this PC; skipped on hosted CI like account-credentials.test.ts) ----

#[test]
fn credential_manager_entries_have_the_shape_typescript_0_4_2_wrote() {
    if std::env::var("TJOMMIE_CI_HOSTED").as_deref() == Ok("1") {
        eprintln!(
            "skipped: hosted Windows service logons cannot reliably reach Credential Manager"
        );
        return;
    }
    let root = std::env::temp_dir().join(format!("tjommie-rs-cred-test-{}", uuid::Uuid::new_v4()));
    // Refused by what the root resolves to on disk, and (below) by namespace, which is what targets are keyed by.
    assert!(
        !cred::is_production_root(&root),
        "refusing the production root"
    );
    let ns = Namespace::new(&root).unwrap();
    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    let production = Namespace::new(&PathBuf::from(local).join("Tjommie")).unwrap();
    assert_ne!(
        ns.short(),
        production.short(),
        "refusing the production namespace"
    );
    assert_ne!(
        ns.long(),
        production.long(),
        "refusing the production namespace"
    );
    let mine = |target: &str| {
        target.starts_with(&format!("Tjommie/{}/", ns.short()))
            || target.starts_with(&format!("Tjommie/{}/", ns.long()))
    };

    let before = snapshot();
    let result = catch_unwind(AssertUnwindSafe(|| cases(&ns)));
    let removed = cleanup(&ns);
    let after = snapshot();

    let outside = |map: &BTreeMap<String, u64>| -> BTreeMap<String, u64> {
        map.iter()
            .filter(|(target, _)| !mine(target))
            .map(|(t, w)| (t.clone(), *w))
            .collect()
    };
    eprintln!(
        "Credential Manager snapshot: before {} Tjommie/* targets ({} outside the test namespace), after {} ({} outside); \
         test namespace {} / {}: {} left behind and removed by cleanup",
        before.len(),
        outside(&before).len(),
        after.len(),
        outside(&after).len(),
        ns.short(),
        ns.long(),
        removed
    );
    if let Err(panic) = result {
        resume_unwind(panic);
    }
    assert!(
        after.keys().all(|target| !mine(target)),
        "test targets were left behind"
    );
    assert_eq!(removed, 0, "every case must clean up after itself");
    assert_eq!(
        outside(&before),
        outside(&after),
        "a Tjommie/* target outside the test namespace changed"
    );
}

/// `fixtures/compat/credential-entries.json`, as a value.
fn captured() -> Value {
    serde_json::from_str(include_str!("fixtures/compat/credential-entries.json")).unwrap()
}

/// The CREDENTIALW shape 0.4.2 gave every entry it wrote, as `entry` reads one back.
fn captured_metadata(captured: &Value) -> Metadata {
    let entry = &captured["entry"];
    let number = |field: &str| u32::try_from(entry[field].as_u64().unwrap()).unwrap();
    assert!(entry["comment"].is_null() && entry["targetAlias"].is_null());
    (
        number("flags"),
        number("credentialType"),
        number("persist"),
        entry["userName"].as_str().unwrap().to_owned(),
        None,
        None,
        number("attributeCount"),
    )
}

fn cases(ns: &Namespace) {
    let captured = captured();
    let metadata = captured_metadata(&captured);

    // Account keys: written as 0.4.2 wrote them, so a rollback reads what Rust wrote.
    let id = format!("synthetic-rs-{}", uuid::Uuid::new_v4());
    let key = captured["account"]["key"].as_str().unwrap();
    assert_eq!(captured["account"]["blobUtf8"].as_str(), Some(key));
    assert_eq!(ns.read_account_key(&id), Ok(None));
    ns.write_account_key(&id, key).unwrap();
    assert_eq!(ns.read_account_key(&id), Ok(Some(key.to_owned())));
    let written = entry(&ns.account_target(&id).unwrap()).unwrap();
    assert_eq!(written.blob, key.as_bytes());
    assert_eq!(written.metadata, metadata);

    // Deleting removes the entry, and deleting a missing one is not an error.
    ns.delete_account_key(&id).unwrap();
    assert_eq!(ns.read_account_key(&id), Ok(None));
    ns.delete_account_key(&id).unwrap();

    // Invalid keys are refused before Credential Manager is touched.
    assert_eq!(
        ns.write_account_key(&id, " spaced"),
        Err(CredentialError::InvalidKey)
    );
    assert_eq!(cred::read(&ns.account_target(&id).unwrap()).unwrap(), None);

    // Corrupt stored values are refused with the fixed message, and the raw layer still sees them.
    let corrupt = ns.account_target(&id).unwrap();
    cred::write(&corrupt, "has a space").unwrap();
    assert_eq!(ns.read_account_key(&id), Err(CredentialError::Unavailable));
    assert_eq!(
        cred::read(&corrupt).unwrap().as_deref(),
        Some("has a space")
    );
    cred::delete(&corrupt).unwrap();

    // The raw layer enforces the 2560-byte Credential Manager blob bound both ways.
    assert!(cred::write(&corrupt, "").is_err());
    assert!(cred::write(&corrupt, &"a".repeat(2561)).is_err());
    assert_eq!(cred::read(&corrupt).unwrap(), None);

    // Login secrets are raw JSON that the desktop's TypeScript vault writes (`tjommie-daemon import` copies them): the blob
    // 0.4.2 wrote reads back unchanged and keeps the shape of any other entry.
    let reference = uuid::Uuid::new_v4().to_string();
    let target = ns.login_target(&reference).unwrap();
    let blob = captured["login"]["blobUtf8"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(blob).unwrap(),
        captured["login"]["secret"]
    );
    cred::write(&target, blob).unwrap();
    assert_eq!(cred::read(&target).unwrap().as_deref(), Some(blob));
    let login = entry(&target).unwrap();
    assert_eq!(login.blob, blob.as_bytes());
    assert_eq!(login.metadata, metadata);
    cred::delete(&target).unwrap();
    assert_eq!(cred::read(&target).unwrap(), None);
}

/// Every `Tjommie/*` target and its LastWritten time. Blobs are never looked at.
fn snapshot() -> BTreeMap<String, u64> {
    enumerate("Tjommie/*").into_iter().collect()
}

fn enumerate(filter: &str) -> Vec<(String, u64)> {
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: CredEnumerateW fills `count` and `list`; the list is freed once with CredFree.
    unsafe {
        // Only "no matching targets" is empty; any other failure would make the before/after check pass vacuously.
        if let Err(error) = CredEnumerateW(&HSTRING::from(filter), None, &mut count, &mut list) {
            assert_eq!(
                error.code(),
                ERROR_NOT_FOUND.to_hresult(),
                "CredEnumerateW {filter}: {error}"
            );
            return Vec::new();
        }
        let found = std::slice::from_raw_parts(list, count as usize)
            .iter()
            .map(|&c| {
                let c = &*c;
                let written = (u64::from(c.LastWritten.dwHighDateTime) << 32)
                    | u64::from(c.LastWritten.dwLowDateTime);
                (c.TargetName.to_string().unwrap(), written)
            })
            .collect();
        CredFree(list as *const _);
        found
    }
}

/// Deletes whatever is left in the test namespace (only it) and says how many it removed.
fn cleanup(ns: &Namespace) -> usize {
    let mut removed = 0;
    for prefix in [ns.short(), ns.long()] {
        for (target, _) in enumerate(&format!("Tjommie/{prefix}/*")) {
            cred::delete(&target).unwrap();
            removed += 1;
        }
    }
    removed
}

type Metadata = (u32, u32, u32, String, Option<String>, Option<String>, u32);
struct Entry {
    metadata: Metadata, // flags, type, persist, user name, comment, target alias, attribute count
    blob: Vec<u8>,
}

fn entry(target: &str) -> Option<Entry> {
    let mut pointer: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: CredReadW fills `pointer` on success; it is freed with CredFree after copying.
    unsafe {
        CredReadW(
            &HSTRING::from(target),
            CRED_TYPE_GENERIC,
            None,
            &mut pointer,
        )
        .ok()?;
        let c = &*pointer;
        let text = |p: PWSTR| (!p.is_null()).then(|| p.to_string().unwrap());
        let found = Entry {
            metadata: (
                c.Flags.0,
                c.Type.0,
                c.Persist.0,
                text(c.UserName).unwrap_or_default(),
                text(c.Comment),
                text(c.TargetAlias),
                c.AttributeCount,
            ),
            // An empty blob may come with a null pointer, which from_raw_parts must never see.
            blob: match c.CredentialBlobSize {
                0 => Vec::new(),
                size => std::slice::from_raw_parts(c.CredentialBlob, size as usize).to_vec(),
            },
        };
        CredFree(pointer as *const _);
        Some(found)
    }
}
