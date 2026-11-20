//! PDF pages (Phase 3 plan T4.4), drawn by pdfium through `pdfium-render`.
//!
//! `pdfium.dll` is a pinned build from bblanchon/pdfium-binaries ([`PDFIUM_RELEASE`]). [`load_library`] hashes the file
//! and refuses anything but [`PDFIUM_SHA256`] before Windows loads it; once loaded, pdfium stays loaded for the life of
//! the process. A PDF is read from a [`RangeSource`] (the app's goes to `attachments.read`) through
//! `FPDF_LoadCustomDocument`, in whole [`CHUNK`]s and only where pdfium asks: opening it and drawing a page read its
//! header, its cross-reference table and that page's objects, so a 200 MiB PDF is never read whole. A damaged file is
//! the exception: pdfium rebuilds a broken cross-reference table by scanning all of it.
//!
//! pdfium isn't thread-safe, so `pdfium-render` holds one lock around every call into it: PDFs open and pages draw on
//! whatever thread asks, one at a time across the process. The app asks off the UI thread.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use pdfium_render::prelude::{PdfRenderConfig, Pdfium, PdfiumError, PdfiumInternalError};
use protocol::attachments::ATTACHMENT_CHUNK_BYTES;

/// The bblanchon/pdfium-binaries release `pdfium.dll` comes from. Its build, 8125, is the pdfium API that
/// `pdfium-render` 0.9.5's `pdfium_latest` feature binds, so the two move together: a DLL from another build lacks
/// functions the bindings look up, and doesn't load.
pub const PDFIUM_RELEASE: &str = "chromium/8125";

/// The SHA-256 of `bin/pdfium.dll` in that release's `pdfium-win-x64.tgz`, as hex. No other file is loaded.
/// `tools/fetch-pdfium.ps1` pins the same value.
pub const PDFIUM_SHA256: &str = "4b0dec92555d69437bef0df37be630181ab3e83a6823ca264ede355a4f2ef994";

/// 256 KiB: the most one `attachments.read` returns. A [`RangeSource`] is asked only for whole chunks, each starting
/// at a multiple of `CHUNK`; a document's last chunk is shorter when its length isn't a multiple.
pub const CHUNK: usize = ATTACHMENT_CHUNK_BYTES as usize;

const CHUNK_U64: u64 = CHUNK as u64;

/// The chunks an open document keeps (1 MiB), the least recently used dropped first, so the header's and the current
/// page's stay while pdfium jumps between them.
const CACHED_CHUNKS: usize = 4;

/// The longest side of a drawn page, in pixels: a very tall page would otherwise ask for gigabytes.
const MAX_SIDE: u16 = 8192;

/// A document's bytes, read a range at a time. The app's reads go to the daemon's `attachments.read`.
#[allow(clippy::len_without_is_empty)] // A byte count; nothing asks whether it is empty.
pub trait RangeSource: Send + Sync {
    /// The document's length in bytes.
    fn len(&self) -> u64;

    /// Exactly `len` bytes from `offset`; anything shorter counts as a failure. `offset` is always a multiple of
    /// [`CHUNK`], `len` is at most [`CHUNK`], and the range never passes the end. This runs inside pdfium's
    /// process-wide lock: it must not use this module, and every other PDF waits while it runs.
    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, String>;
}

/// `Read + Seek` over a [`RangeSource`], for `pdfium-render`: the source is asked only for whole chunks.
struct ChunkReader<S> {
    len: u64,
    source: S,
    position: u64,
    /// `(chunk number, its bytes)`, least recently used first.
    cache: VecDeque<(u64, Vec<u8>)>,
    /// How many of the source's reads have failed. pdfium sees a failed read only as missing data, so the document
    /// checks this after every call that can read.
    failed_reads: Arc<AtomicU64>,
}

impl<S: RangeSource> ChunkReader<S> {
    /// Chunk `number`, from the cache or read whole.
    fn chunk(&mut self, number: u64) -> io::Result<&[u8]> {
        if let Some(at) = self.cache.iter().position(|(n, _)| *n == number) {
            if let Some(hit) = self.cache.remove(at) {
                self.cache.push_back(hit);
            }
        } else {
            let offset = number * CHUNK_U64;
            // At most CHUNK, so the cast is exact.
            let length = self.len.saturating_sub(offset).min(CHUNK_U64) as usize;
            match self.source.read(offset, length) {
                Ok(bytes) if bytes.len() == length => {
                    if self.cache.len() == CACHED_CHUNKS {
                        self.cache.pop_front();
                    }
                    self.cache.push_back((number, bytes));
                }
                _ => {
                    self.failed_reads.fetch_add(1, Ordering::Relaxed);
                    return Err(io::Error::other("Part of the PDF couldn't be read."));
                }
            }
        }
        let newest = self.cache.back().map(|(_, bytes)| bytes.as_slice());
        Ok(newest.unwrap_or_default())
    }
}

impl<S: RangeSource> Read for ChunkReader<S> {
    // `pdfium-render` hands pdfium whatever one `read` returns as the whole block pdfium asked for, so a read fills
    // `buf` or fails; it never stops short. (pdfium never asks past the end.)
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let wanted = buf.len();
        let mut rest = buf;
        while !rest.is_empty() && self.position < self.len {
            let number = self.position / CHUNK_U64;
            let start = (self.position % CHUNK_U64) as usize;
            let chunk = self.chunk(number)?;
            let copied = rest.write(chunk.get(start..).unwrap_or_default())?;
            if copied == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.position += copied as u64;
        }
        Ok(wanted - rest.len())
    }
}

impl<S> Seek for ChunkReader<S> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let position = match to {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        };
        self.position = position.ok_or(io::ErrorKind::InvalidInput)?;
        Ok(self.position)
    }
}

/// pdfium, loaded and pinned. Every copy is the same library, which loads once per process and stays loaded.
#[derive(Debug, Clone, Copy)]
pub struct PdfLibrary(&'static Pdfium);

static LIBRARY: OnceLock<Pdfium> = OnceLock::new();
/// Held through the first load, so two threads asking at once load pdfium once.
static LOADING: Mutex<()> = Mutex::new(());

/// Loads pdfium from `dll` if the file's SHA-256 is `expected_sha256` (the app passes [`PDFIUM_SHA256`]), in either
/// case. The file is hashed first, and a mismatch is refused before Windows loads anything. Once pdfium is loaded, a
/// later call with a matching file gets the library already loaded.
pub fn load_library(dll: &Path, expected_sha256: &str) -> Result<PdfLibrary, PdfError> {
    // Absolute, so Windows loads the file that was hashed rather than searching its DLL path for the name.
    let dll = std::path::absolute(dll).map_err(|_| PdfError::LibraryMissing)?;
    let sha256 = crate::speech::sha256_file(&dll).map_err(|_| PdfError::LibraryMissing)?;
    if !sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(PdfError::LibraryNotPinned);
    }
    let _loading = LOADING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(pdfium) = LIBRARY.get() {
        return Ok(PdfLibrary(pdfium));
    }
    let bindings = Pdfium::bind_to_library(dll).map_err(|_| PdfError::LibraryLoad)?;
    Ok(PdfLibrary(LIBRARY.get_or_init(|| Pdfium::new(bindings))))
}

impl PdfLibrary {
    /// Opens the PDF in `source`, reading only what pdfium asks for, in whole [`CHUNK`]s. A read that fails while it
    /// opens makes it [`PdfError::Read`]: pdfium can carry on with half a document (no pages, say) instead of failing.
    pub fn open(self, source: impl RangeSource + 'static) -> Result<PdfDocument, PdfError> {
        let failed_reads = Arc::new(AtomicU64::new(0));
        let reader = ChunkReader {
            len: source.len(),
            source,
            position: 0,
            cache: VecDeque::new(),
            failed_reads: Arc::clone(&failed_reads),
        };
        let opened = self.0.load_pdf_from_reader(reader, None);
        if failed_reads.load(Ordering::Relaxed) != 0 {
            return Err(PdfError::Read);
        }
        match opened {
            Ok(document) => Ok(PdfDocument {
                document,
                failed_reads,
            }),
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                Err(PdfError::Locked)
            }
            Err(_) => Err(PdfError::Open),
        }
    }
}

/// An open PDF, holding its source and the last few chunks read from it.
pub struct PdfDocument {
    document: pdfium_render::prelude::PdfDocument<'static>,
    failed_reads: Arc<AtomicU64>,
}

/// A drawn page: `width` × `height` pixels of four bytes each in pdfium's order (blue, green, red, alpha), row by row
/// from the top with no padding, so `bgra.len()` is `width * height * 4`.
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl PdfDocument {
    /// How many pages the PDF has.
    pub fn page_count(&self) -> u32 {
        u32::try_from(self.document.pages().len()).unwrap_or(0)
    }

    /// Page `index`'s size in points (1/72 inch), as it is shown: the viewer draws a page at its own size times the
    /// zoom, as the TypeScript's pdf.js did. Reading it loads the page, so a failed read is [`PdfError::Read`].
    pub fn page_size(&self, index: u32) -> Result<(f32, f32), PdfError> {
        if index >= self.page_count() {
            return Err(PdfError::NoPage);
        }
        let failed_before = self.failed_reads.load(Ordering::Relaxed);
        let index = i32::try_from(index).map_err(|_| PdfError::NoPage)?;
        let Ok(page) = self.document.pages().get(index) else {
            return Err(PdfError::Render);
        };
        let size = (page.width().value, page.height().value);
        if self.failed_reads.load(Ordering::Relaxed) != failed_before {
            return Err(PdfError::Read);
        }
        Ok(size)
    }

    /// Draws page `index` (from 0) on white, `width_px` pixels wide and as tall as the page's shape makes it. Neither
    /// side passes 8192 pixels: a page that would is drawn smaller, keeping its shape. A read that fails while drawing
    /// makes it [`PdfError::Read`], rather than a page with parts missing.
    pub fn render_page(&self, index: u32, width_px: u32) -> Result<RenderedPage, PdfError> {
        if index >= self.page_count() {
            return Err(PdfError::NoPage);
        }
        // A draw on another thread can only add to the count: it may fail this one needlessly, never hide a failure.
        let failed_before = self.failed_reads.load(Ordering::Relaxed);
        let page = self.draw(index, width_px);
        if self.failed_reads.load(Ordering::Relaxed) != failed_before {
            return Err(PdfError::Read);
        }
        page
    }

    fn draw(&self, index: u32, width_px: u32) -> Result<RenderedPage, PdfError> {
        let index = i32::try_from(index).map_err(|_| PdfError::NoPage)?;
        let Ok(page) = self.document.pages().get(index) else {
            return Err(PdfError::Render);
        };
        let side = i32::from(MAX_SIDE);
        let width = i32::try_from(width_px).unwrap_or(side).min(side);
        // `pdfium-render` asks pdfium for RGBA unless told otherwise; this keeps pdfium's own BGRA.
        let config = PdfRenderConfig::new()
            .set_target_width(width)
            .set_maximum_height(side)
            .set_reverse_byte_order(false);
        let Ok(bitmap) = page.render_with_config(&config) else {
            return Err(PdfError::Render);
        };
        let width = u32::try_from(bitmap.width()).map_err(|_| PdfError::Render)?;
        let height = u32::try_from(bitmap.height()).map_err(|_| PdfError::Render)?;
        let bgra = bitmap.as_raw_bytes();
        // Four bytes a pixel, so rows are never padded; checked anyway, as callers rely on it.
        if bgra.len() != width as usize * height as usize * 4 {
            return Err(PdfError::Render);
        }
        Ok(RenderedPage {
            width,
            height,
            bgra,
        })
    }
}

/// What went wrong, in words fit to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfError {
    /// `pdfium.dll` isn't there or can't be read.
    LibraryMissing,
    /// `pdfium.dll` isn't the pinned build, so it wasn't loaded.
    LibraryNotPinned,
    /// Windows couldn't load `pdfium.dll`, or it lacks a function the bindings look up.
    LibraryLoad,
    /// The file is damaged or isn't a PDF.
    Open,
    /// The PDF needs a password.
    Locked,
    /// Reading part of the PDF failed; trying again may work.
    Read,
    /// The PDF has no page with that number.
    NoPage,
    /// pdfium couldn't draw the page.
    Render,
}

impl fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::LibraryMissing => "pdfium.dll is missing or can't be read.",
            Self::LibraryNotPinned => "pdfium.dll doesn't match the pinned build.",
            Self::LibraryLoad => "pdfium.dll couldn't be loaded.",
            Self::Open => "This PDF couldn't be opened.",
            Self::Locked => "This PDF is protected by a password.",
            Self::Read => "Part of this PDF couldn't be read. Try again.",
            Self::NoPage => "This PDF has no such page.",
            Self::Render => "This page of the PDF couldn't be drawn.",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for PdfError {}

#[cfg(test)]
mod tests {
    use pdfium_render::prelude::{PdfPagePaperSize, PdfPoints};

    use super::*;

    /// The fixture's attachment that no page uses: 8 MiB.
    const PAYLOAD: usize = 8 * 1024 * 1024;

    /// A PDF in memory that notes every read it serves.
    struct Counting {
        bytes: Vec<u8>,
        reads: Arc<Mutex<Vec<(u64, usize)>>>,
    }

    impl RangeSource for Counting {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
            self.reads.lock().unwrap().push((offset, len));
            let start = usize::try_from(offset).unwrap();
            // Not indexing: a panic here would be inside pdfium's callback, which aborts every test.
            match self.bytes.get(start..start + len) {
                Some(bytes) => Ok(bytes.to_vec()),
                None => Err("past the end".into()),
            }
        }
    }

    /// Every read fails, as when the daemon has gone.
    struct Offline;

    impl RangeSource for Offline {
        fn len(&self) -> u64 {
            1024 * 1024
        }

        fn read(&self, _: u64, _: usize) -> Result<Vec<u8>, String> {
            Err("offline".into())
        }
    }

    /// The real pdfium from `TJOMMIE_PDFIUM_DLL`, which CI sets; without it, `None` after saying so.
    fn real_pdfium() -> Option<PdfLibrary> {
        let Some(dll) = std::env::var_os("TJOMMIE_PDFIUM_DLL") else {
            // On CI a missing DLL is a broken step, not a reason to pass.
            assert!(
                std::env::var_os("TJOMMIE_CI_HOSTED").is_none(),
                "CI sets TJOMMIE_PDFIUM_DLL (ci.yml fetches the pinned pdfium.dll)"
            );
            eprintln!("skipped: TJOMMIE_PDFIUM_DLL is not set (CI sets it)");
            return None;
        };
        let pdfium = load_library(Path::new(&dll), PDFIUM_SHA256);
        Some(pdfium.expect("TJOMMIE_PDFIUM_DLL is the pinned pdfium.dll"))
    }

    /// Two 600 × 800 pt pages and an 8 MiB attachment no page uses, random so pdfium can't compress it away.
    fn fixture(pdfium: PdfLibrary) -> Vec<u8> {
        let mut document = pdfium.0.create_new_pdf().unwrap();
        let size = PdfPagePaperSize::new_custom(PdfPoints::new(600.0), PdfPoints::new(800.0));
        for _ in 0..2 {
            document.pages_mut().create_page_at_end(size).unwrap();
        }
        // xorshift64
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let payload: Vec<u8> = (0..PAYLOAD / 8)
            .flat_map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()
            })
            .collect();
        document
            .attachments_mut()
            .create_attachment_from_bytes("payload.bin", &payload)
            .unwrap();
        document.save_to_bytes().unwrap()
    }

    #[test]
    fn range_reads_only() {
        let Some(pdfium) = real_pdfium() else { return };
        let bytes = fixture(pdfium);
        let length = bytes.len() as u64;
        assert!(length > PAYLOAD as u64, "{length} bytes");
        let reads = Arc::new(Mutex::new(Vec::new()));
        let source = Counting {
            bytes,
            reads: Arc::clone(&reads),
        };
        let document = pdfium.open(source).unwrap();
        assert_eq!(document.page_count(), 2);
        assert_eq!(document.page_size(0).unwrap(), (600.0, 800.0));
        assert_eq!(document.page_size(2).err(), Some(PdfError::NoPage));
        let page = document.render_page(0, 300).unwrap();
        assert_eq!((page.width, page.height), (300, 400));
        assert_eq!(page.bgra.len(), 300 * 400 * 4);
        // A blank page is the white it was cleared to: opaque, in BGRA.
        assert_eq!(page.bgra[..4], [255, 255, 255, 255]);
        let past_the_end = document.render_page(2, 300).err();
        assert_eq!(past_the_end, Some(PdfError::NoPage));

        let reads = reads.lock().unwrap();
        for &(offset, len) in reads.iter() {
            assert_eq!(offset % CHUNK_U64, 0, "read at {offset}");
            assert!((1..=CHUNK).contains(&len), "read of {len} bytes");
        }
        // Opening and drawing page 1 need the header, the cross-reference table and trailer at the end, and a few small
        // objects (catalog, page tree, page) that pdfium writes ahead of the attachment: the first chunk and the last
        // one or two, about 768 KiB at most. The attachment is never read, so a quarter of the file is a safe ceiling.
        let total: u64 = reads.iter().map(|&(_, len)| len as u64).sum();
        assert!(
            total < length / 4,
            "read {total} of {length} bytes in {} reads",
            reads.len()
        );
    }

    #[test]
    fn unreadable_and_damaged_pdfs_are_errors() {
        let Some(pdfium) = real_pdfium() else { return };
        assert_eq!(pdfium.open(Offline).err(), Some(PdfError::Read));
        let junk = Counting {
            bytes: b"not a pdf ".repeat(100_000),
            reads: Arc::default(),
        };
        assert_eq!(pdfium.open(junk).err(), Some(PdfError::Open));
    }

    #[test]
    fn a_dll_that_is_not_the_pinned_build_is_refused_before_it_loads() {
        let folder = tempfile::tempdir().unwrap();
        let dll = folder.path().join("pdfium.dll");
        let missing = load_library(&dll, PDFIUM_SHA256).err();
        assert_eq!(missing, Some(PdfError::LibraryMissing));
        // Not a DLL at all: had it reached LoadLibrary, the error would be LibraryLoad.
        std::fs::write(&dll, b"not pdfium").unwrap();
        let zeros = "0".repeat(64);
        for pin in [PDFIUM_SHA256, "", zeros.as_str()] {
            let refused = load_library(&dll, pin).err();
            assert_eq!(refused, Some(PdfError::LibraryNotPinned), "{pin}");
        }
    }
}
