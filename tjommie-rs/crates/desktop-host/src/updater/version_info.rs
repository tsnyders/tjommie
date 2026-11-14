//! Reads a string from an executable's version resource (`VS_VERSION_INFO`), which is where the installer carries its
//! `AppId` (M8's NSIS `VIAddVersionKey`). The updater refuses an installer whose `AppId` isn't its own: a feed that
//! points at some other app's installer, or at a dev build's, installs nothing.

use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::PCWSTR;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `name` from the string table of `path`'s version resource, in the first language the resource lists.
pub fn read_string(path: &Path, name: &str) -> Result<String, String> {
    let file = wide(&path.to_string_lossy());
    let unreadable =
        |step: &str| format!("{} has no version information ({step}).", path.display());
    // SAFETY: the buffers outlive the calls; the pointers VerQueryValueW returns point into `data`, which isn't
    // touched or dropped until they have been copied out.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(file.as_ptr()), None);
        if size == 0 {
            return Err(unreadable("size"));
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(
            PCWSTR(file.as_ptr()),
            None,
            size,
            data.as_mut_ptr().cast::<c_void>(),
        )
        .map_err(|error| unreadable(&format!("read: {error}")))?;

        // The languages the resource lists, then the ones installers use when they list none (NSIS writes 0409/04b0).
        let mut blocks: Vec<(u16, u16)> = Vec::new();
        let mut languages: *mut c_void = std::ptr::null_mut();
        let mut length = 0u32;
        let found = VerQueryValueW(
            data.as_ptr().cast::<c_void>(),
            PCWSTR(wide(r"\VarFileInformation\Translation").as_ptr()),
            &mut languages,
            &mut length,
        );
        if found.as_bool() && !languages.is_null() {
            // Each entry is a language and a code page, two 16-bit numbers.
            let pairs = std::slice::from_raw_parts(languages.cast::<u16>(), (length / 2) as usize);
            blocks.extend(
                pairs
                    .chunks_exact(2)
                    .filter_map(|pair| Some((*pair.first()?, *pair.get(1)?))),
            );
        }
        blocks.extend([
            (0x0409, 0x04b0),
            (0x0409, 0x04e4),
            (0x0000, 0x04b0),
            (0x0409, 0x0000),
        ]);
        for (language, code_page) in blocks {
            let query = wide(&format!(
                r"\StringFileInfo\{language:04x}{code_page:04x}\{name}"
            ));
            let mut value: *mut c_void = std::ptr::null_mut();
            let mut chars = 0u32;
            let found = VerQueryValueW(
                data.as_ptr().cast::<c_void>(),
                PCWSTR(query.as_ptr()),
                &mut value,
                &mut chars,
            );
            if !found.as_bool() || value.is_null() || chars == 0 {
                continue;
            }
            let units = std::slice::from_raw_parts(value.cast::<u16>(), chars as usize);
            let end = units
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(units.len());
            return String::from_utf16(units.get(..end).unwrap_or_default())
                .map_err(|_| format!("{name} in {} isn't text.", path.display()));
        }
        Err(format!(
            "{} has no {name} in its version information.",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_string_from_a_real_executables_version_resource() {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let dll = Path::new(&windir).join("System32").join("kernel32.dll");
        assert_eq!(
            read_string(&dll, "OriginalFilename")
                .unwrap()
                .to_lowercase(),
            "kernel32"
        );
        assert!(
            read_string(&dll, "AppId").is_err(),
            "a string that isn't there is an error"
        );
    }

    /// One `VS_VERSIONINFO` node: length, value length, type (1 text, 0 binary), key, value, children, 4-aligned.
    fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
        fn pad(out: &mut Vec<u8>) {
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
        }
        let mut out = vec![0, 0];
        out.extend(value_len.to_le_bytes());
        out.extend(u16::from(text).to_le_bytes());
        for unit in key.encode_utf16().chain([0]) {
            out.extend(unit.to_le_bytes());
        }
        pad(&mut out);
        out.extend(value);
        pad(&mut out);
        for child in children {
            out.extend(child);
            pad(&mut out);
        }
        let length = u16::try_from(out.len()).unwrap();
        out.splice(0..2, length.to_le_bytes());
        out
    }

    /// What NSIS's `VIAddVersionKey /LANG=1033 "AppId" ...` writes: US English, Unicode.
    fn version_resource(app_id: &str) -> Vec<u8> {
        let text: Vec<u8> = app_id
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let string = node("AppId", &text, (text.len() / 2) as u16, true, &[]);
        let table = node("040904b0", &[], 0, true, &[string]);
        let strings = node("StringFileInfo", &[], 0, true, &[table]);
        let translation = node("Translation", &[0x09, 0x04, 0xb0, 0x04], 4, false, &[]);
        let vars = node("VarFileInfo", &[], 0, true, &[translation]);
        let mut fixed = vec![0u8; 52];
        fixed[..4].copy_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        fixed[4..8].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        node("VS_VERSION_INFO", &fixed, 52, false, &[strings, vars])
    }

    /// Writes `resource` as the version resource of a copy of a small system executable, which stands in for an
    /// installer the way NSIS would build it.
    fn installer_carrying(dir: &Path, resource: &[u8]) -> std::path::PathBuf {
        use windows::Win32::System::LibraryLoader::{
            BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
        };
        use windows::Win32::UI::WindowsAndMessaging::RT_VERSION;
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let installer = dir.join("Setup.exe");
        std::fs::copy(
            Path::new(&windir).join("System32").join("find.exe"),
            &installer,
        )
        .unwrap();
        let file = wide(&installer.to_string_lossy());
        // SAFETY: updates the resources of the temporary copy made above, and ends the update.
        unsafe {
            let update = BeginUpdateResourceW(PCWSTR(file.as_ptr()), false).unwrap();
            UpdateResourceW(
                update,
                RT_VERSION,
                // MAKEINTRESOURCE(1): a resource id is passed in the pointer.
                PCWSTR(std::ptr::without_provenance::<u16>(1)),
                0x0409,
                Some(resource.as_ptr().cast::<c_void>()),
                resource.len() as u32,
            )
            .unwrap();
            EndUpdateResourceW(update, false).unwrap();
        }
        installer
    }

    // The link the updater's trust rests on: the id an installer carries is the id read back from it.
    #[test]
    fn reads_the_app_id_an_nsis_style_installer_carries() {
        let dir = tempfile::tempdir().unwrap();
        let installer = installer_carrying(dir.path(), &version_resource("com.tsnyders.tjommie"));
        assert_eq!(
            read_string(&installer, "AppId").unwrap(),
            "com.tsnyders.tjommie"
        );
        // Another key it doesn't have is an error, not the app id.
        assert!(read_string(&installer, "ProductName").is_err());
        // And a different id reads as different, so the updater's comparison has something to refuse.
        let other = tempfile::tempdir().unwrap();
        let foreign =
            installer_carrying(other.path(), &version_resource("com.tsnyders.tjommie-rs"));
        assert_eq!(
            read_string(&foreign, "AppId").unwrap(),
            "com.tsnyders.tjommie-rs"
        );
        assert_eq!(
            crate::updater::Inspector::app_id(&crate::updater::VersionResource, &foreign).unwrap(),
            "com.tsnyders.tjommie-rs"
        );
    }

    #[test]
    fn a_file_without_version_information_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.exe");
        std::fs::write(&file, b"MZ not really an executable").unwrap();
        assert!(read_string(&file, "AppId").is_err());
        assert!(read_string(&dir.path().join("missing.exe"), "AppId").is_err());
    }
}
