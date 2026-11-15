//! `latest.yml`, the release feed (spec §3.10). electron-builder writes it and R1's electron-updater reads it, so the
//! Rust updater reads the same file:
//!
//! ```yaml
//! version: 0.6.0
//! files:
//!   - url: Tjommie-Setup-0.6.0.exe
//!     sha512: <base64 of the SHA-512 of the installer>
//!     size: 54321234
//! path: Tjommie-Setup-0.6.0.exe
//! sha512: <the same>
//! releaseDate: '2026-11-20T10:00:00.000Z'
//! ```
//!
//! Only that shape is read, not YAML in general, and everything in it is checked: the version is semver, the file
//! name is a plain `.exe` name (no folder, no scheme), the checksum decodes to exactly 64 bytes, and the top-level
//! `path` and `sha512` agree with the `files` entry they name.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use semver::Version;

/// A feed is a few hundred bytes. Anything near this is not a feed.
pub const MAX_FEED_BYTES: usize = 64 * 1024;
/// The largest installer the updater will download when the feed doesn't say (it always does).
pub const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub file_name: String,
    pub sha512: [u8; 64],
    pub size: Option<u64>,
}

#[derive(Default)]
struct Entry {
    url: Option<String>,
    sha512: Option<String>,
    size: Option<String>,
}

/// Removes the quotes YAML allows around a plain value.
fn unquote(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return inner.to_string();
    }
    value.to_string()
}

/// `electron-builder` names installers with letters, digits, dots, dashes and underscores. Nothing here can name a folder,
/// a drive, a stream or another host.
fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && name.to_ascii_lowercase().ends_with(".exe")
}

fn checksum(text: &str) -> Result<[u8; 64], String> {
    STANDARD
        .decode(text.trim())
        .ok()
        .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
        .ok_or_else(|| "The release feed has a checksum that is not a SHA-512.".to_string())
}

pub fn parse(text: &str) -> Result<Release, String> {
    if text.len() > MAX_FEED_BYTES {
        return Err("The release feed is too large.".into());
    }
    let mut version = None;
    let mut path = None;
    let mut top_sha512 = None;
    let mut files: Vec<Entry> = Vec::new();
    let mut in_files = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if line.len() == body.len() {
            in_files = false;
            let Some((key, value)) = body.split_once(':') else {
                continue;
            };
            match key.trim() {
                "version" => version = Some(unquote(value)),
                "path" => path = Some(unquote(value)),
                "sha512" => top_sha512 = Some(unquote(value)),
                "files" => in_files = true,
                _ => {}
            }
        } else if in_files {
            let item = match body.strip_prefix("- ") {
                Some(rest) => {
                    files.push(Entry::default());
                    rest
                }
                None => body,
            };
            let Some((key, value)) = item.split_once(':') else {
                continue;
            };
            let Some(entry) = files.last_mut() else {
                return Err("The release feed is malformed.".into());
            };
            match key.trim() {
                "url" => entry.url = Some(unquote(value)),
                "sha512" => entry.sha512 = Some(unquote(value)),
                "size" => entry.size = Some(unquote(value)),
                _ => {}
            }
        }
    }

    let version = version.ok_or("The release feed has no version.")?;
    let version = Version::parse(&version)
        .map_err(|_| "The release feed's version is not a version number.".to_string())?;
    // The installer: the entry the top-level `path` names, or the only .exe.
    let mut installers = files.iter().filter(|entry| {
        entry
            .url
            .as_deref()
            .is_some_and(|url| url.to_ascii_lowercase().ends_with(".exe"))
    });
    let entry = match &path {
        Some(path) => installers
            .find(|entry| entry.url.as_deref() == Some(path.as_str()))
            .ok_or("The release feed's path is not among its files.")?,
        None => {
            let only = installers
                .next()
                .ok_or("The release feed lists no installer.")?;
            if installers.next().is_some() {
                return Err("The release feed lists several installers and no path.".into());
            }
            only
        }
    };
    let file_name = entry.url.clone().unwrap_or_default();
    if !valid_file_name(&file_name) {
        return Err(
            "The release feed names an installer file that isn't a plain file name.".into(),
        );
    }
    let sha512 = checksum(
        entry
            .sha512
            .as_deref()
            .ok_or("The release feed has no checksum for the installer.")?,
    )?;
    if let Some(top) = top_sha512
        && checksum(&top)? != sha512
    {
        return Err("The release feed's checksums disagree.".into());
    }
    let size = match &entry.size {
        Some(size) => Some(
            size.trim()
                .parse::<u64>()
                .ok()
                .filter(|size| (1..=MAX_INSTALLER_BYTES).contains(size))
                .ok_or("The release feed's installer size is not believable.")?,
        ),
        None => None,
    };
    Ok(Release {
        version,
        file_name,
        sha512,
        size,
    })
}

#[cfg(test)]
pub(crate) fn feed_text(version: &str, file_name: &str, sha512: &[u8; 64], size: u64) -> String {
    let sha = STANDARD.encode(sha512);
    format!(
        "version: {version}\nfiles:\n  - url: {file_name}\n    sha512: {sha}\n    size: {size}\npath: {file_name}\nsha512: {sha}\nreleaseDate: '2026-11-20T10:00:00.000Z'\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(byte: u8) -> [u8; 64] {
        [byte; 64]
    }

    #[test]
    fn reads_what_electron_builder_writes() {
        let text = feed_text("0.6.0", "Tjommie-Setup-0.6.0.exe", &sha(7), 54_321_234);
        assert_eq!(
            parse(&text).unwrap(),
            Release {
                version: Version::new(0, 6, 0),
                file_name: "Tjommie-Setup-0.6.0.exe".into(),
                sha512: sha(7),
                size: Some(54_321_234),
            }
        );
        // A pre-release version, quoted values, CRLF line ends, a comment, an unknown key with a block value.
        let quoted = format!(
            "# generated\r\nversion: '0.3.1-test'\r\nfiles:\r\n  - url: \"Tjommie-Setup.exe\"\r\n    sha512: {}\r\n    blockMapSize: 12\r\nreleaseNotes: |\r\n  - not a file\r\n  - url: nope.exe\r\n",
            STANDARD.encode(sha(1))
        );
        let release = parse(&quoted).unwrap();
        assert_eq!(release.version.to_string(), "0.3.1-test");
        assert_eq!(release.file_name, "Tjommie-Setup.exe");
        assert_eq!(release.size, None);
    }

    #[test]
    fn refuses_a_feed_that_could_point_the_updater_anywhere() {
        let good = |name: &str| feed_text("0.6.0", name, &sha(7), 100);
        // The installer must be a plain file name: no folder, drive, stream, host or parent folder.
        for name in [
            "..\\Tjommie.exe",
            "sub/Tjommie.exe",
            "sub\\Tjommie.exe",
            "C:Tjommie.exe",
            "https://evil.example/Tjommie.exe",
            "Tjommie.exe:stream.exe",
            "..Tjommie.exe",
            "Tjommie.msi",
            ".exe.txt",
            "",
        ] {
            assert!(parse(&good(name)).is_err(), "{name:?} must be refused");
        }
        assert!(parse(&good("Tjommie-Setup-0.6.0.exe")).is_ok());
    }

    #[test]
    fn refuses_a_feed_that_is_incomplete_or_inconsistent() {
        let sha7 = STANDARD.encode(sha(7));
        let sha8 = STANDARD.encode(sha(8));
        let cases = [
            // no version, a version that isn't one
            format!("files:\n  - url: a.exe\n    sha512: {sha7}\n"),
            format!("version: latest\nfiles:\n  - url: a.exe\n    sha512: {sha7}\n"),
            // no files, no checksum, a checksum that is not a SHA-512
            "version: 1.0.0\n".to_string(),
            "version: 1.0.0\nfiles:\n  - url: a.exe\n".to_string(),
            "version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: AAAA\n".to_string(),
            "version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: not base64!!\n".to_string(),
            // the top-level checksum names another installer's bytes
            format!(
                "version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: {sha7}\npath: a.exe\nsha512: {sha8}\n"
            ),
            // a path that isn't among the files, two installers and no path
            format!("version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: {sha7}\npath: b.exe\n"),
            format!(
                "version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: {sha7}\n  - url: b.exe\n    sha512: {sha7}\n"
            ),
            // sizes that are zero, huge or not numbers
            format!("version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: {sha7}\n    size: 0\n"),
            format!(
                "version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: {sha7}\n    size: 99999999999\n"
            ),
            format!("version: 1.0.0\nfiles:\n  - url: a.exe\n    sha512: {sha7}\n    size: many\n"),
            // a field before any `- ` item
            format!("version: 1.0.0\nfiles:\n    sha512: {sha7}\n"),
        ];
        for text in cases {
            assert!(parse(&text).is_err(), "must be refused:\n{text}");
        }
        assert!(parse(&"x".repeat(MAX_FEED_BYTES + 1)).is_err());
    }
}
