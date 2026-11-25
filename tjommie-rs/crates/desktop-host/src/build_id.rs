//! The build id of an exe: the PE header's timestamp and size of image, and the CodeView record that names its PDB (the
//! PDB's GUID and age). A crash log carries them (`crash`), so a stack in it can be matched to the PDB of the release that
//! made it (`Tjommie-<version>-symbols.zip`): the PDB's own GUID and age must be the same.
//!
//! The headers are read from the file with plain reads (no loading, no `unsafe`), a few kilobytes of it, never more than
//! the first 16 KiB plus the debug directory and its CodeView record. A file that is not a PE, or has no CodeView record, has
//! no id, and a caller says so.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// More than any real PE puts before its section table (DOS stub, headers, 16 data directories, a few dozen sections).
const HEADER_BYTES: usize = 16 * 1024;
/// The debug directory has a handful of entries; this is generous.
const MAX_DEBUG_ENTRIES: usize = 32;
/// `IMAGE_DEBUG_DIRECTORY` is 28 bytes.
const DEBUG_ENTRY: usize = 28;
/// A CodeView record is the signature, a GUID, an age and a path.
const MAX_CODEVIEW: usize = 1024;
const IMAGE_DEBUG_TYPE_CODEVIEW: u32 = 2;
const DEBUG_DIRECTORY: usize = 6;

/// Where a PE's symbols are: the PDB's file name (never its folder, which is the build machine's), GUID and age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pdb {
    pub name: String,
    /// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`, upper case: the GUID a PDB's own header carries.
    pub guid: String,
    pub age: u32,
}

/// What names one build of an exe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildId {
    pub timestamp: u32,
    pub size_of_image: u32,
    pub pdb: Option<Pdb>,
}

impl BuildId {
    /// `timestamp=0x… size=0x… pdb=… guid=… age=…`: one line's worth, the way a crash log writes it.
    pub fn describe(&self) -> String {
        let mut text = format!(
            "size=0x{:x} timestamp=0x{:08x}",
            self.size_of_image, self.timestamp
        );
        if let Some(pdb) = &self.pdb {
            text.push_str(&format!(
                " pdb={} guid={} age={}",
                pdb.name, pdb.guid, pdb.age
            ));
        }
        text
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    Some(u16::from_le_bytes(bytes.get(at..end)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    Some(u32::from_le_bytes(bytes.get(at..end)?.try_into().ok()?))
}

fn usize_at(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32_at(bytes, at)?).ok()
}

/// The build id of the PE at `path`, or `None` when it is no PE or says nothing about its symbols.
pub fn read(path: &Path) -> Option<BuildId> {
    let mut file = File::open(path).ok()?;
    let mut head = Vec::with_capacity(HEADER_BYTES);
    (&mut file)
        .take(HEADER_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    parse(&head, |offset, wanted| {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut bytes = Vec::with_capacity(wanted);
        (&mut file)
            .take(wanted as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    })
}

/// The build id from a PE's first bytes (`head`), asking `at(offset, length)` for anything beyond them: the debug directory
/// and its CodeView record, which a linker puts in a section.
pub fn parse(head: &[u8], mut at: impl FnMut(u64, usize) -> Option<Vec<u8>>) -> Option<BuildId> {
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let nt = usize_at(head, 0x3c)?;
    if head.get(nt..nt.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let coff = nt.checked_add(4)?;
    let sections = usize::from(u16_at(head, coff.checked_add(2)?)?);
    let timestamp = u32_at(head, coff.checked_add(4)?)?;
    let optional_size = usize::from(u16_at(head, coff.checked_add(16)?)?);
    let optional = coff.checked_add(20)?;
    // PE32+ (0x20b) and PE32 (0x10b) differ only in where the data directories start.
    let directories = match u16_at(head, optional)? {
        0x20b => optional.checked_add(112)?,
        0x10b => optional.checked_add(96)?,
        _ => return None,
    };
    let size_of_image = u32_at(head, optional.checked_add(56)?)?;
    let build = |pdb| BuildId {
        timestamp,
        size_of_image,
        pdb,
    };
    let entry = directories.checked_add(DEBUG_DIRECTORY.checked_mul(8)?)?;
    let (debug_rva, debug_size) = (u32_at(head, entry)?, usize_at(head, entry.checked_add(4)?)?);
    if debug_rva == 0 || debug_size == 0 {
        return Some(build(None));
    }
    // The file offset of an RVA: through the section that holds it.
    let table = optional.checked_add(optional_size)?;
    let file_offset = |rva: u32| -> Option<u64> {
        (0..sections).find_map(|index| {
            let section = table.checked_add(index.checked_mul(40)?)?;
            let (virtual_size, address, raw_size, raw) = (
                u32_at(head, section.checked_add(8)?)?,
                u32_at(head, section.checked_add(12)?)?,
                u32_at(head, section.checked_add(16)?)?,
                u32_at(head, section.checked_add(20)?)?,
            );
            let inside = rva >= address && rva - address < virtual_size.max(raw_size);
            inside.then(|| u64::from(raw) + u64::from(rva - address))
        })
    };
    // The timestamp is known by now, so a directory that can't be read costs only the PDB's name.
    let Some(directory) = file_offset(debug_rva)
        .and_then(|offset| at(offset, debug_size.min(MAX_DEBUG_ENTRIES * DEBUG_ENTRY)))
    else {
        return Some(build(None));
    };
    let pdb = directory
        .chunks_exact(DEBUG_ENTRY)
        .find(|entry| u32_at(entry, 12) == Some(IMAGE_DEBUG_TYPE_CODEVIEW))
        .and_then(|entry| {
            let size = usize_at(entry, 16)?;
            let raw = u32_at(entry, 24)?;
            codeview(&at(u64::from(raw), size.min(MAX_CODEVIEW))?)
        });
    Some(build(pdb))
}

/// The `RSDS` record: signature, GUID, age, then the PDB's path as the linker wrote it.
fn codeview(record: &[u8]) -> Option<Pdb> {
    if record.get(..4)? != b"RSDS" {
        return None;
    }
    let guid = record.get(4..20)?;
    let age = u32_at(record, 20)?;
    let path = record.get(24..)?.split(|byte| *byte == 0).next()?;
    // Only the file name: the folder is the build machine's, and a log is read, shared and attached to reports.
    let name = String::from_utf8_lossy(path)
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_owned();
    let d4 = guid.get(8..16)?;
    let guid = format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{}",
        u32_at(guid, 0)?,
        u16_at(guid, 4)?,
        u16_at(guid, 6)?,
        d4.first()?,
        d4.get(1)?,
        d4.get(2..)?
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>(),
    );
    Some(Pdb { name, guid, age })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PE32+ of one section (`.rdata`, RVA 0x1000, raw 0x400) that holds a debug directory with a CodeView entry.
    fn synthetic(pdb_path: &str) -> Vec<u8> {
        let mut file = vec![0u8; 0x400];
        file[..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        file[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        file[coff + 4..coff + 8].copy_from_slice(&0x6740_1234u32.to_le_bytes());
        file[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
        let optional = coff + 20;
        file[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        file[optional + 56..optional + 60].copy_from_slice(&0x5000u32.to_le_bytes());
        let debug_entry = optional + 112 + 6 * 8;
        file[debug_entry..debug_entry + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        file[debug_entry + 4..debug_entry + 8].copy_from_slice(&28u32.to_le_bytes());
        // The section table follows the optional header.
        let section = optional + 240;
        file[section + 8..section + 12].copy_from_slice(&0x400u32.to_le_bytes());
        file[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 16..section + 20].copy_from_slice(&0x400u32.to_le_bytes());
        file[section + 20..section + 24].copy_from_slice(&0x400u32.to_le_bytes());
        // The section's raw data: the directory at 0x400, its CodeView record at 0x420.
        let mut tail = vec![0u8; 0x20];
        tail[12..16].copy_from_slice(&IMAGE_DEBUG_TYPE_CODEVIEW.to_le_bytes());
        let mut record = b"RSDS".to_vec();
        record.extend_from_slice(&0x0123_4567u32.to_le_bytes());
        record.extend_from_slice(&0x89abu16.to_le_bytes());
        record.extend_from_slice(&0xcdefu16.to_le_bytes());
        record.extend_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        record.extend_from_slice(&7u32.to_le_bytes());
        record.extend_from_slice(pdb_path.as_bytes());
        record.push(0);
        tail[16..20].copy_from_slice(&(record.len() as u32).to_le_bytes());
        tail[24..28].copy_from_slice(&0x420u32.to_le_bytes());
        tail.truncate(28);
        file.extend_from_slice(&tail);
        file.resize(0x420, 0);
        file.extend_from_slice(&record);
        file
    }

    fn read_from(file: &[u8]) -> Option<BuildId> {
        let head = &file[..file.len().min(0x400)];
        parse(head, |offset, wanted| {
            let start = usize::try_from(offset).ok()?;
            Some(file.get(start..)?.iter().take(wanted).copied().collect())
        })
    }

    #[test]
    fn reads_the_timestamp_the_image_size_and_the_pdb() {
        let id = read_from(&synthetic(r"D:\a\build\target\release\deps\app.pdb")).unwrap();
        assert_eq!(id.timestamp, 0x6740_1234);
        assert_eq!(id.size_of_image, 0x5000);
        assert_eq!(
            id.pdb,
            Some(Pdb {
                // The build machine's folder is dropped.
                name: "app.pdb".into(),
                guid: "01234567-89AB-CDEF-0123-456789ABCDEF".into(),
                age: 7,
            })
        );
        assert_eq!(
            id.describe(),
            "size=0x5000 timestamp=0x67401234 pdb=app.pdb guid=01234567-89AB-CDEF-0123-456789ABCDEF age=7"
        );
    }

    #[test]
    fn a_file_that_is_no_pe_has_no_id_and_nothing_panics() {
        assert_eq!(read_from(b""), None);
        assert_eq!(read_from(b"MZ"), None);
        assert_eq!(read_from(b"not an exe at all"), None);
        // Headers that point past the end of the file.
        let mut broken = synthetic("app.pdb");
        broken[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert_eq!(read_from(&broken), None);
        // A debug directory whose section is not there: the timestamp is still known, only the PDB is not.
        let mut cut = synthetic("app.pdb");
        cut[0x84 + 2..0x84 + 4].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            read_from(&cut).map(|id| (id.timestamp, id.pdb)),
            Some((0x6740_1234, None))
        );
    }

    #[test]
    fn an_image_with_no_debug_directory_has_the_timestamp_and_no_pdb() {
        let mut bare = synthetic("app.pdb");
        let entry = 0x84 + 20 + 112 + 6 * 8;
        bare[entry..entry + 8].fill(0);
        let id = read_from(&bare).unwrap();
        assert_eq!(id.timestamp, 0x6740_1234);
        assert_eq!(id.pdb, None);
    }

    #[test]
    fn this_test_binary_has_a_build_id_with_a_pdb() {
        let exe = std::env::current_exe().unwrap();
        let id = read(&exe).expect("the test binary is a PE");
        let pdb = id.pdb.expect("a dev build keeps a PDB");
        assert!(pdb.name.ends_with(".pdb"), "{pdb:?}");
        assert!(!pdb.name.contains(['\\', '/']), "{pdb:?}");
        assert_eq!(pdb.guid.len(), 36, "{pdb:?}");
        assert!(pdb.age >= 1, "{pdb:?}");
        assert!(id.size_of_image > 0);
    }
}
