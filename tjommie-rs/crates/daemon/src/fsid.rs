//! Where a path really is. Paths are compared by file identity: the volume serial and 128-bit file id of the nearest
//! existing ancestor, then the missing rest compared without case. Spelling, case, `\\?\`, UNC loopback shares
//! (`\\localhost\c$`), junctions, symlinks, `subst` drives and volume-GUID paths all reach the same identity, which
//! string comparisons after `canonicalize` don't. Use it for every "is this the same or inside that folder" check.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::mem::ManuallyDrop;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, OPEN_EXISTING,
};
use windows::core::HSTRING;

/// `realpathSync.native`: `path` resolved through links and short names, without the `\\?\` (or `\\?\UNC\`) prefix
/// that `canonicalize` adds on Windows. The path must exist.
pub fn real_path(path: &Path) -> io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    let text = real.to_string_lossy();
    Ok(if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{share}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        real
    })
}

/// A volume serial and file id.
type Identity = (u64, [u8; 16]);

/// A path as its nearest existing ancestor (with that folder's own ancestors, all by identity) and the missing rest.
struct Location {
    /// The nearest existing ancestor first, then its parents up to the volume root, after links are resolved.
    chain: Vec<Identity>,
    /// The components below the nearest existing ancestor, lowercased.
    rest: Vec<String>,
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owned and closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Opens a file or folder for identity only, following links (`FILE_FLAG_BACKUP_SEMANTICS` opens folders).
fn open(path: &Path) -> io::Result<Handle> {
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    // SAFETY: no access rights are asked for; the handle is owned by the guard.
    let handle = unsafe {
        CreateFileW(
            &HSTRING::from(path),
            0,
            share,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(os_error)?;
    Ok(Handle(handle))
}

/// A Win32 failure as the OS error it wraps, so a missing file reads as `NotFound`.
fn os_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(error)
    }
}

fn identity(handle: &Handle) -> io::Result<Identity> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is a live, correctly sized struct.
    unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileIdInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    }?;
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}

fn final_path(handle: &Handle) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: the buffer is local and sized.
    let length =
        unsafe { GetFinalPathNameByHandleW(handle.0, &mut buffer, FILE_NAME_NORMALIZED) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

fn locate(path: &Path) -> io::Result<Location> {
    let absolute = std::path::absolute(path)?;
    let (mut existing, mut rest) = (absolute.as_path(), Vec::<OsString>::new());
    let anchor = loop {
        match open(existing) {
            Ok(handle) => break handle,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or(error)?;
                rest.push(name.to_owned());
                existing = existing
                    .parent()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            }
            Err(error) => return Err(error),
        }
    };
    let mut chain = vec![identity(&anchor)?];
    let resolved = final_path(&anchor)?;
    let mut parent = resolved.parent();
    while let Some(folder) = parent {
        chain.push(identity(&open(folder)?)?);
        parent = folder.parent();
    }
    let rest = rest
        .iter()
        .rev()
        .map(|part| part.to_string_lossy().to_lowercase())
        .collect();
    Ok(Location { chain, rest })
}

/// Whether `a` and `b` name the same file or folder (or, when missing, the same place).
pub fn same_location(a: &Path, b: &Path) -> io::Result<bool> {
    let (a, b) = (locate(a)?, locate(b)?);
    Ok(a.chain[0] == b.chain[0] && a.rest == b.rest)
}

/// Whether `path` names the very file `file` holds open: the same volume serial and file id. A different file copied
/// into its place, even with the same size and times, is not. Like the rest of this module, `Err` means refuse.
pub fn is_open_file(file: &File, path: &Path) -> io::Result<bool> {
    // Borrowed from `file`, so it must not be closed here.
    let held = ManuallyDrop::new(Handle(HANDLE(file.as_raw_handle())));
    let at = locate(path)?;
    Ok(at.rest.is_empty() && at.chain[0] == identity(&held)?)
}

/// Whether `path` is `root` or lies anywhere below it.
pub fn is_within(path: &Path, root: &Path) -> io::Result<bool> {
    let (path, root) = (locate(path)?, locate(root)?);
    Ok(if root.rest.is_empty() {
        // The root exists: it is the path's nearest existing ancestor or one of that folder's ancestors.
        path.chain.contains(&root.chain[0])
    } else {
        // A missing root holds only its own missing descendants.
        path.chain[0] == root.chain[0] && path.rest.starts_with(&root.rest)
    })
}
