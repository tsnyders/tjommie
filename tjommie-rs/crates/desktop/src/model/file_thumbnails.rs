//! Port of `packages/ui/lib/file-thumbnails.ts` (T1.4): which files get a thumbnail, the raster header checks that run before a
//! decoder touches untrusted bytes, the byte budget of a PDF's range reads, the one-at-a-time queue of background thumbnails
//! (paused while the user works in a file, invalidated when the list changes), and the thumbnail itself. Not ported: the
//! canvas drawing and the decoders (the WebView's; the renderer seam is [`ThumbnailRenderer`]), pdf.js (PDF thumbnails are
//! `Unsupported` in the Rust port until it has a PDF renderer), and the wait on native cleanup of an invalidated generation
//! (the caller awaits its own cleanup before it queues again).

use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use client::Rpc;
use protocol::attachments::{ATTACHMENT_CHUNK_BYTES, AttachmentInfo, AttachmentRef};
use tokio::sync::Mutex as AsyncMutex;

use super::attachment_transfer::{
    AttachmentNative, AttachmentPreview, HOST_REPLY, Retirements, TransferError, preview_attachment,
};
use super::document_ranges::{DocumentError, RangeFetch, Signal, document_range};

pub const THUMBNAIL_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
pub const THUMBNAIL_VIDEO_BYTES: u64 = 8 * 1024 * 1024;
pub const THUMBNAIL_PIXELS: u64 = 16 * 1024 * 1024;
pub const THUMBNAIL_PDF_BYTES: u64 = 2 * 1024 * 1024;
/// The pinned PDF worker reserves a document-length buffer, even when ranges are used.
pub const THUMBNAIL_PDF_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const THUMBNAIL_TEXT_BYTES: u64 = 4096;
const TEXT_CHARS: usize = 600;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const TEXT_LIMIT_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailKind {
    Image,
    Pdf,
    Video,
    Text,
    Audio,
    File,
}

pub fn thumbnail_kind(content_type: &str) -> ThumbnailKind {
    match content_type {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => ThumbnailKind::Image,
        "application/pdf" => ThumbnailKind::Pdf,
        "video/mp4" | "video/webm" => ThumbnailKind::Video,
        "audio/wav" | "audio/mpeg" | "audio/mp4" | "audio/ogg" => ThumbnailKind::Audio,
        "text/markdown" | "text/plain" => ThumbnailKind::Text,
        _ => ThumbnailKind::File,
    }
}

/// Whether a file of this type and size gets a thumbnail without being asked for.
pub fn thumbnail_eligible(content_type: &str, size: u64) -> bool {
    match thumbnail_kind(content_type) {
        ThumbnailKind::Image => size > 0 && size <= THUMBNAIL_IMAGE_BYTES,
        ThumbnailKind::Video => size > 0 && size <= THUMBNAIL_VIDEO_BYTES,
        ThumbnailKind::Pdf => size > 0 && size <= THUMBNAIL_PDF_FILE_BYTES,
        ThumbnailKind::Text => size <= TEXT_LIMIT_BYTES,
        _ => false,
    }
}

/// Why a thumbnail failed or was stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum ThumbnailError {
    /// Cancelled, or retired by the list changing (the TypeScript's `AbortError`).
    Cancelled,
    /// Thumbnails are paused while the user works in a file.
    Paused,
    /// The file or its pixels are over a limit, with the TypeScript's message.
    Limit(&'static str),
    Unsupported(&'static str),
    /// Text with control characters: not a text thumbnail.
    NotText,
    /// The renderer could not decode the file.
    Decode(String),
    Document(DocumentError),
    Transfer(TransferError),
    /// The preview's lease or native handle did not close, so the thumbnail is refused.
    PreviewRetirement,
}

impl std::fmt::Display for ThumbnailError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("Cancelled"),
            Self::Paused => formatter.write_str("Preview paused"),
            Self::Limit(message) | Self::Unsupported(message) => formatter.write_str(message),
            Self::NotText => formatter.write_str("Not a text thumbnail."),
            Self::Decode(message) => formatter.write_str(message),
            Self::Document(error) => error.fmt(formatter),
            Self::Transfer(error) => error.fmt(formatter),
            Self::PreviewRetirement => formatter
                .write_str("The previous file preview could not close. Reconnect and try again."),
        }
    }
}

impl std::error::Error for ThumbnailError {}

fn document<T>(result: Result<T, DocumentError>) -> Result<T, ThumbnailError> {
    result.map_err(|error| match error {
        DocumentError::Cancelled => ThumbnailError::Cancelled,
        other => ThumbnailError::Document(other),
    })
}

fn transfer<T>(result: Result<T, TransferError>) -> Result<T, ThumbnailError> {
    result.map_err(|error| match error {
        TransferError::Cancelled => ThumbnailError::Cancelled,
        other => ThumbnailError::Transfer(other),
    })
}

/// `untilAbort` for the renderer's work: an abort wins.
async fn race<T>(signal: &Signal, future: impl Future<Output = T>) -> Result<T, ThumbnailError> {
    if signal.aborted() {
        return Err(ThumbnailError::Cancelled);
    }
    tokio::select! {
        biased;
        () = signal.cancelled() => Err(ThumbnailError::Cancelled),
        value = future => Ok(value),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

/// A header's size as it says it: no limit here, each reader applies its own (`thumbnail_dimensions`, the viewer's).
/// A side of 0 is no picture at all.
fn dimensions(width: u32, height: u32) -> Result<Dimensions, ThumbnailError> {
    if width == 0 || height == 0 {
        return Err(unsupported_raster());
    }
    Ok(Dimensions { width, height })
}

fn unsupported_raster() -> ThumbnailError {
    ThumbnailError::Unsupported("Unsupported raster header.")
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    let pair: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(pair))
}

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    let pair: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(pair))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let quad: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(quad))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    let quad: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(quad))
}

fn le24(bytes: &[u8], at: usize) -> Option<u32> {
    let [low, middle, high]: [u8; 3] = bytes.get(at..at + 3)?.try_into().ok()?;
    Some(u32::from(low) | u32::from(middle) << 8 | u32::from(high) << 16)
}

/// `rasterDimensions`: the pixel size from the header of PNG, GIF, WebP or JPEG bytes, read before a decoder is asked to
/// decode untrusted data, whatever the size: the thumbnail and the viewer each apply their own limit. Anything else (or a
/// header it can't read) is unsupported. It reads no further than the header, so what it says of a file's first bytes it
/// says of the whole file, unless that is unsupported (a JPEG's frame header can lie further on): a picture too large is
/// refused from its first chunk (M4 re-review R8).
pub fn raster_dimensions(bytes: &[u8], content_type: &str) -> Result<Dimensions, ThumbnailError> {
    const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if content_type == "image/png"
        && bytes.len() >= 24
        && bytes.get(..8) == Some(&PNG_SIGNATURE[..])
        && bytes.get(12..16) == Some(&b"IHDR"[..])
    {
        return dimensions(
            be32(bytes, 16).ok_or_else(unsupported_raster)?,
            be32(bytes, 20).ok_or_else(unsupported_raster)?,
        );
    }
    if content_type == "image/gif"
        && bytes.len() >= 10
        && (bytes.get(..6) == Some(&b"GIF87a"[..]) || bytes.get(..6) == Some(&b"GIF89a"[..]))
    {
        return dimensions(
            u32::from(le16(bytes, 6).ok_or_else(unsupported_raster)?),
            u32::from(le16(bytes, 8).ok_or_else(unsupported_raster)?),
        );
    }
    if content_type == "image/webp"
        && bytes.len() >= 30
        && bytes.get(..4) == Some(&b"RIFF"[..])
        && bytes.get(8..12) == Some(&b"WEBP"[..])
    {
        match bytes.get(12..16) {
            Some(b"VP8X") => {
                return dimensions(
                    le24(bytes, 24).ok_or_else(unsupported_raster)? + 1,
                    le24(bytes, 27).ok_or_else(unsupported_raster)? + 1,
                );
            }
            Some(b"VP8 ") if bytes.get(23..26) == Some(&[157, 1, 42][..]) => {
                let width = le16(bytes, 26).ok_or_else(unsupported_raster)? & 16383;
                let height = le16(bytes, 28).ok_or_else(unsupported_raster)? & 16383;
                return dimensions(u32::from(width), u32::from(height));
            }
            Some(b"VP8L") if bytes.get(20) == Some(&47) => {
                let bits = le32(bytes, 21).ok_or_else(unsupported_raster)?;
                return dimensions((bits & 16383) + 1, ((bits >> 14) & 16383) + 1);
            }
            _ => {}
        }
    }
    if content_type == "image/jpeg" && bytes.len() >= 4 && bytes.get(..2) == Some(&[255, 216][..]) {
        return jpeg_dimensions(bytes);
    }
    Err(unsupported_raster())
}

/// The size of a picture a thumbnail may be drawn from: its header's, within `THUMBNAIL_PIXELS`.
pub fn thumbnail_dimensions(
    bytes: &[u8],
    content_type: &str,
) -> Result<Dimensions, ThumbnailError> {
    let size = raster_dimensions(bytes, content_type)?;
    if u64::from(size.width) * u64::from(size.height) > THUMBNAIL_PIXELS {
        return Err(ThumbnailError::Limit("Image exceeds thumbnail dimensions."));
    }
    Ok(size)
}

/// Whether a GIF or WebP holds more than one frame. GPUI decodes every frame of one to a buffer the size of the whole
/// picture, so a small file with many frames is gigabytes once decoded (M4 review C2): an animation is never handed to
/// the decoder, and a GIF whose blocks don't read is treated as one. A GIF is animated with a second image descriptor or
/// a NETSCAPE2.0 (loop) extension; a WebP when its VP8X flags say so.
pub fn animated(bytes: &[u8], content_type: &str) -> bool {
    match content_type {
        "image/gif" => gif_animated(bytes),
        "image/webp" => {
            bytes.get(12..16) == Some(&b"VP8X"[..])
                && bytes.get(20).is_some_and(|flags| flags & 0x02 != 0)
        }
        _ => false,
    }
}

fn gif_animated(bytes: &[u8]) -> bool {
    /// The size of a colour table the packed byte announces, if any.
    fn table(packed: u8) -> usize {
        if packed & 0x80 == 0 {
            0
        } else {
            3 << ((packed & 7) + 1)
        }
    }
    /// The offset after the data sub-blocks starting at `at`, if they end.
    fn skip_blocks(bytes: &[u8], mut at: usize) -> Option<usize> {
        loop {
            let size = usize::from(*bytes.get(at)?);
            at += 1;
            if size == 0 {
                return Some(at);
            }
            at += size;
        }
    }
    let Some(&packed) = bytes.get(10) else {
        return true;
    };
    let mut at = 13 + table(packed);
    let mut images = 0;
    loop {
        match bytes.get(at) {
            // The trailer, or the end of the bytes: done, one frame at most.
            Some(0x3B) | None => return images > 1,
            Some(0x2C) => {
                images += 1;
                if images > 1 {
                    return true;
                }
                let Some(&packed) = bytes.get(at + 9) else {
                    return true;
                };
                // The descriptor, its colour table, and the LZW code size before the data.
                let Some(next) = skip_blocks(bytes, at + 10 + table(packed) + 1) else {
                    return true;
                };
                at = next;
            }
            Some(0x21) => {
                let label = bytes.get(at + 1).copied();
                if label == Some(0xFF) && bytes.get(at + 3..at + 14) == Some(&b"NETSCAPE2.0"[..]) {
                    return true;
                }
                let Some(next) = skip_blocks(bytes, at + 2) else {
                    return true;
                };
                at = next;
            }
            Some(_) => return true,
        }
    }
}

/// The frame sizes: the markers that start a frame header (SOF0 to SOF15, without DHT, JPG and DAC).
const FRAME_MARKERS: [u8; 13] = [
    192, 193, 194, 195, 197, 198, 199, 201, 202, 203, 205, 206, 207,
];

fn jpeg_dimensions(bytes: &[u8]) -> Result<Dimensions, ThumbnailError> {
    let mut at = 2usize;
    while at + 3 < bytes.len() {
        if bytes.get(at) != Some(&255) {
            break;
        }
        at += 1;
        while bytes.get(at) == Some(&255) {
            at += 1;
        }
        let Some(&marker) = bytes.get(at) else {
            break;
        };
        at += 1;
        if marker == 217 || marker == 218 {
            break;
        }
        if marker == 1 || (208..=215).contains(&marker) {
            continue;
        }
        if at + 2 > bytes.len() {
            break;
        }
        let length = usize::from(be16(bytes, at).unwrap_or(0));
        if length < 2 || at + length > bytes.len() {
            break;
        }
        if FRAME_MARKERS.contains(&marker) && length >= 7 {
            let height = be16(bytes, at + 3).unwrap_or(0);
            let width = be16(bytes, at + 5).unwrap_or(0);
            return dimensions(u32::from(width), u32::from(height));
        }
        at += length;
    }
    Err(unsupported_raster())
}

/// The renderer: the WebView's decoder and video frames, behind the seam the Rust side calls. It releases whatever it
/// decoded before it answers.
pub trait ThumbnailRenderer: Sync {
    type Picture: Send;
    /// Decodes `bytes` (a `mime` raster) to a picture of `width` by `height`.
    fn decode(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        width: u32,
        height: u32,
    ) -> impl Future<Output = Result<Self::Picture, String>> + Send;
    /// A frame from a video, read from the preview's URL.
    fn video_frame(&self, url: &str) -> impl Future<Output = Result<Self::Picture, String>> + Send;
}

/// Where a thumbnail's parts come from: the daemon (the preview lease), the native side, the preview's bytes, and the renderer.
pub struct Thumbnails<'a, R, N, F, G> {
    pub rpc: &'a R,
    pub native: &'a N,
    pub fetch: &'a F,
    pub renderer: &'a G,
}

/// A thumbnail: a picture the renderer made, or the first characters of a text file.
#[derive(Debug, Clone, PartialEq)]
pub enum FileThumbnail<P> {
    Picture(P),
    Text(String),
}

/// `readFileThumbnail`: opens a preview lease for the file, reads what the thumbnail needs, and closes the lease (and waits for
/// any retired work) on every way out. A lease that does not close refuses the thumbnail.
pub async fn read_file_thumbnail<
    'a,
    R: Rpc,
    N: AttachmentNative,
    F: RangeFetch,
    G: ThumbnailRenderer,
>(
    source: Thumbnails<'a, R, N, F, G>,
    reference: &AttachmentRef,
    signal: &Signal,
) -> Result<FileThumbnail<G::Picture>, ThumbnailError> {
    let mut retire: Retirements<'a> = Vec::new();
    let opened = preview_attachment(
        source.rpc,
        reference,
        source.native,
        signal,
        HOST_REPLY,
        &mut retire,
    )
    .await;
    match opened {
        Ok(preview) => {
            let made = thumbnail_of(&source, &preview, signal).await;
            let closed = preview.close().await;
            drain(&mut retire).await;
            match closed {
                Ok(()) => made,
                Err(_) => Err(ThumbnailError::PreviewRetirement),
            }
        }
        Err(error) => {
            drain(&mut retire).await;
            transfer(Err(error))
        }
    }
}

async fn drain(retire: &mut Retirements<'_>) {
    for cleanup in retire.drain(..) {
        cleanup.await;
    }
}

async fn thumbnail_of<'a, R: Rpc, N: AttachmentNative, F: RangeFetch, G: ThumbnailRenderer>(
    source: &Thumbnails<'a, R, N, F, G>,
    preview: &AttachmentPreview<'a, R, N>,
    signal: &Signal,
) -> Result<FileThumbnail<G::Picture>, ThumbnailError> {
    let info: AttachmentInfo = preview.attachment().clone();
    if !thumbnail_eligible(&info.content_type, info.size) {
        return Err(ThumbnailError::Limit("File exceeds thumbnail limits."));
    }
    let url = preview.url();
    match thumbnail_kind(&info.content_type) {
        ThumbnailKind::Image => {
            let bytes = read_whole(source.fetch, url, info.size, signal).await?;
            let size = thumbnail_dimensions(&bytes, &info.content_type)?;
            let scale = 1f64
                .min(f64::from(WIDTH) / f64::from(size.width))
                .min(f64::from(HEIGHT) / f64::from(size.height));
            let width = fitted(size.width, scale);
            let height = fitted(size.height, scale);
            let decoding = source
                .renderer
                .decode(bytes, &info.content_type, width, height);
            let mut pending = Box::pin(decoding);
            tokio::select! {
                biased;
                () = signal.cancelled() => {
                    // A decode that finishes after the abort is released before the cancellation is reported.
                    let _late = pending.await;
                    Err(ThumbnailError::Cancelled)
                }
                decoded = &mut pending => decoded.map(FileThumbnail::Picture).map_err(ThumbnailError::Decode),
            }
        }
        ThumbnailKind::Video => {
            let frame = race(signal, source.renderer.video_frame(url)).await?;
            Ok(FileThumbnail::Picture(
                frame.map_err(ThumbnailError::Decode)?,
            ))
        }
        ThumbnailKind::Text => {
            let size = info.size;
            let bytes = if size > 0 {
                document(
                    document_range(
                        source.fetch,
                        url,
                        size,
                        0,
                        size.min(THUMBNAIL_TEXT_BYTES),
                        signal,
                    )
                    .await,
                )?
            } else {
                Vec::new()
            };
            let complete = u64::try_from(bytes.len()).unwrap_or(u64::MAX) >= size;
            text_thumbnail(&bytes, complete).map(FileThumbnail::Text)
        }
        ThumbnailKind::Pdf => Err(ThumbnailError::Unsupported(
            "PDF thumbnails are rendered by the WebView.",
        )),
        _ => Err(ThumbnailError::Unsupported(
            "No thumbnail for this file type.",
        )),
    }
}

/// A side of the picture, scaled, at least one pixel.
fn fitted(side: u32, scale: f64) -> u32 {
    // Math.round: halves go up, which is what round does for positive values.
    let scaled = (f64::from(side) * scale).round();
    if scaled >= 1.0 { scaled as u32 } else { 1 }
}

/// The whole file, read in exact chunks (never more than one chunk in a request).
async fn read_whole<F: RangeFetch>(
    fetch: &F,
    url: &str,
    size: u64,
    signal: &Signal,
) -> Result<Vec<u8>, ThumbnailError> {
    let mut bytes = Vec::new();
    for offset in (0..size).step_by(ATTACHMENT_CHUNK_BYTES as usize) {
        let end = size.min(offset + u64::from(ATTACHMENT_CHUNK_BYTES));
        bytes.extend(document(
            document_range(fetch, url, size, offset, end, signal).await,
        )?);
    }
    Ok(bytes)
}

/// The first characters of a text file. A character cut off at the end of a partial read waits for the rest, as a streaming
/// decoder does; control characters mean it is not text.
fn text_thumbnail(bytes: &[u8], complete: bool) -> Result<String, ThumbnailError> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if !complete && error.error_len().is_none() => bytes
            .get(..error.valid_up_to())
            .and_then(|head| std::str::from_utf8(head).ok())
            .ok_or(ThumbnailError::Document(DocumentError::NotUtf8))?,
        Err(_) => return Err(ThumbnailError::Document(DocumentError::NotUtf8)),
    };
    if text
        .chars()
        .any(|character| matches!(u32::from(character), 0..=8 | 11 | 12 | 14..=31))
    {
        return Err(ThumbnailError::NotText);
    }
    Ok(text.chars().take(TEXT_CHARS).collect())
}

/// `PdfRangeQueue`'s budget for one thumbnail: at most [`THUMBNAIL_PDF_BYTES`] of ranges (the TypeScript's `requestDataRange`).
/// Past that the load stops, which the TypeScript reports as an abort.
#[derive(Debug, Default)]
pub struct PdfBudget {
    requested: u64,
}

impl PdfBudget {
    pub fn request(&mut self, begin: u64, end: u64) -> Result<(), ThumbnailError> {
        let length = end
            .checked_sub(begin)
            .filter(|length| *length > 0)
            .ok_or(ThumbnailError::Cancelled)?;
        if self.requested.saturating_add(length) > THUMBNAIL_PDF_BYTES {
            return Err(ThumbnailError::Cancelled);
        }
        self.requested += length;
        Ok(())
    }
}

/// The background thumbnails: one at a time, in order. Paused while the user works in a file, invalidated when the list
/// changes. (The TypeScript keeps this in module state; here the app owns it.)
pub struct ThumbnailQueue {
    turn: AsyncMutex<()>,
    state: Mutex<QueueState>,
    work_timeout: Duration,
    settle_timeout: Duration,
}

#[derive(Default)]
struct QueueState {
    pauses: usize,
    generation: u64,
    running: Option<Signal>,
}

impl ThumbnailQueue {
    /// `work_timeout`: how long one thumbnail may run before it is stopped. `settle_timeout`: how long a pause may wait.
    pub fn new(work_timeout: Duration, settle_timeout: Duration) -> Self {
        Self {
            turn: AsyncMutex::new(()),
            state: Mutex::new(QueueState::default()),
            work_timeout,
            settle_timeout,
        }
    }

    fn state(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn paused(&self) -> bool {
        self.state().pauses > 0
    }

    /// `queueThumbnail`: runs `work` once the thumbnails before it have finished, unless the queue is paused, or the list
    /// changed while it waited. The work gets a signal of its own, which is aborted when `signal` is, or after the timeout.
    pub async fn queue<T, F, Fut>(&self, signal: &Signal, work: F) -> Result<T, ThumbnailError>
    where
        F: FnOnce(Signal) -> Fut,
        Fut: Future<Output = Result<T, ThumbnailError>>,
    {
        if self.paused() {
            return Err(ThumbnailError::Paused);
        }
        let generation = self.state().generation;
        let _turn = self.turn.lock().await;
        if signal.aborted() {
            return Err(ThumbnailError::Cancelled);
        }
        if self.paused() {
            return Err(ThumbnailError::Paused);
        }
        if self.state().generation != generation {
            return Err(ThumbnailError::Cancelled);
        }
        let running = Signal::new();
        self.state().running = Some(running.clone());
        let mut work = Box::pin(work(running.clone()));
        let timer = tokio::time::sleep(self.work_timeout);
        tokio::pin!(timer);
        let mut stopping = false;
        let result = loop {
            tokio::select! {
                biased;
                result = &mut work => break result,
                () = signal.cancelled(), if !stopping => {
                    stopping = true;
                    running.abort();
                }
                () = &mut timer, if !stopping => {
                    stopping = true;
                    running.abort();
                }
            }
        };
        self.state().running = None;
        result
    }

    /// `pauseThumbnailWork`: admits no more thumbnails, and aborts the one running.
    pub fn pause(&self) {
        let mut state = self.state();
        state.pauses += 1;
        if let Some(running) = &state.running {
            running.abort();
        }
    }

    /// `release`: the foreground work is done; thumbnails may be queued again once no pause is left.
    pub fn release(&self) {
        let mut state = self.state();
        state.pauses = state.pauses.saturating_sub(1);
    }

    /// `ready`: resolves once the thumbnail that was running, and any queued before this call, have settled. Taking the turn
    /// waits for them; the timeout is a retirement that did not finish.
    pub async fn ready(&self, signal: &Signal) -> Result<(), ThumbnailError> {
        if signal.aborted() {
            return Err(ThumbnailError::Cancelled);
        }
        tokio::select! {
            biased;
            () = signal.cancelled() => Err(ThumbnailError::Cancelled),
            () = tokio::time::sleep(self.settle_timeout) => Err(ThumbnailError::PreviewRetirement),
            _turn = self.turn.lock() => Ok(()),
        }
    }

    /// `invalidateThumbnailWork`: the list changed. Whatever runs is aborted, and what waits is cancelled when its turn comes.
    pub fn invalidate(&self) {
        let mut state = self.state();
        state.generation += 1;
        if let Some(running) = &state.running {
            running.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use protocol::attachments::{
        ATTACHMENT_CHUNK_BYTES, AttachmentInfo, AttachmentOpen, AttachmentRef,
    };
    use protocol::wire::{Empty, method};
    use tokio::sync::oneshot;

    use crate::model::testing::FakeRpc;

    use super::super::attachment_transfer::{
        AttachmentNative, NativePreview, SaveStatus, TransferError,
    };
    use super::super::document_ranges::{PdfRangeQueue, RangeFetch, RangeReply, Signal};
    use super::*;

    const PREVIEW: &str = "624d43d7-1a49-4d79-8d1d-85d846cfbd8e";
    const MEGABYTE: u64 = 1024 * 1024;

    fn reference() -> AttachmentRef {
        AttachmentRef {
            message_id: "message".to_owned(),
            attachment_index: 0,
            attachment_id: None,
        }
    }

    fn info(content_type: &str, size: u64) -> AttachmentInfo {
        AttachmentInfo {
            id: PREVIEW.to_owned(),
            host_id: "storage-host".to_owned(),
            name: "fixture".to_owned(),
            size,
            content_type: content_type.to_owned(),
            sha256: "a".repeat(64),
        }
    }

    /// The daemon hands out a preview lease for `info`, and closes it.
    fn daemon(info: AttachmentInfo) -> FakeRpc {
        FakeRpc::new()
            .on::<method::AttachmentsOpen>(move |_| {
                Ok(AttachmentOpen {
                    transfer_id: "41f8bac7-358c-48cf-9f48-a3a358804da6".to_owned(),
                    attachment: info.clone(),
                    chunk_bytes: ATTACHMENT_CHUNK_BYTES,
                    expires_at: "2026-11-03T12:00:00Z".to_owned(),
                })
            })
            .on::<method::AttachmentsClose>(|_| Ok(Empty {}))
    }

    /// Serves a `size`-byte document from `body`, in honest range replies, and remembers each range asked for.
    struct Serve<B> {
        size: u64,
        requests: Mutex<Vec<(u64, u64)>>,
        body: B,
    }

    impl<B: Fn(u64, u64) -> Vec<u8> + Sync> Serve<B> {
        fn new(size: u64, body: B) -> Self {
            Self {
                size,
                requests: Mutex::new(Vec::new()),
                body,
            }
        }

        fn asked(&self) -> Vec<(u64, u64)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl<B: Fn(u64, u64) -> Vec<u8> + Sync> RangeFetch for Serve<B> {
        fn fetch_range(
            &self,
            _url: &str,
            begin: u64,
            last: u64,
        ) -> impl Future<Output = Result<RangeReply, String>> + Send {
            self.requests.lock().unwrap().push((begin, last));
            let reply = RangeReply {
                status: 206,
                content_range: Some(format!("bytes {begin}-{last}/{}", self.size)),
                content_length: Some((last - begin + 1).to_string()),
                body: (self.body)(begin, last),
            };
            std::future::ready(Ok(reply))
        }
    }

    /// The native side for thumbnails: it only serves the preview lease, and counts how many leases it closed.
    struct Previews {
        url: String,
        closed: Mutex<usize>,
    }

    impl Previews {
        fn new(url: &str) -> Self {
            Self {
                url: url.to_owned(),
                closed: Mutex::new(0),
            }
        }

        fn closed(&self) -> usize {
            *self.closed.lock().unwrap()
        }
    }

    fn unused<T: Send>() -> impl Future<Output = Result<T, TransferError>> + Send {
        std::future::ready(Err(TransferError::Failed(
            "not used by thumbnails".to_owned(),
        )))
    }

    impl AttachmentNative for Previews {
        fn start_save(
            &self,
            _info: AttachmentInfo,
        ) -> impl Future<Output = Result<Option<String>, TransferError>> + Send {
            unused()
        }

        fn append_save(
            &self,
            _save_id: &str,
            _offset: u64,
            _bytes: Vec<u8>,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            unused()
        }

        fn finish_save(
            &self,
            _save_id: &str,
        ) -> impl Future<Output = Result<SaveStatus, TransferError>> + Send {
            unused()
        }

        fn cancel_save(
            &self,
            _save_id: &str,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            unused()
        }

        fn preview(
            &self,
            _info: AttachmentInfo,
        ) -> impl Future<Output = Result<NativePreview, TransferError>> + Send {
            std::future::ready(Ok(NativePreview {
                preview_id: PREVIEW.to_owned(),
                url: self.url.clone(),
            }))
        }

        fn close_preview(
            &self,
            _preview_id: &str,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            *self.closed.lock().unwrap() += 1;
            std::future::ready(Ok(()))
        }

        fn respond(
            &self,
            _preview_id: &str,
            _request_id: &str,
            _bytes: Option<Vec<u8>>,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            std::future::ready(Ok(()))
        }
    }

    /// Counts the bitmaps a decode releases.
    struct Bitmap(Arc<AtomicUsize>);

    impl Drop for Bitmap {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Debug, PartialEq)]
    struct Picture;

    /// The WebView's decoder: it records each decode, and releases its bitmap before answering. `gate` holds a decode back.
    #[derive(Default)]
    struct Renderer {
        decoded: Mutex<Vec<(usize, u32, u32)>>,
        closes: Arc<AtomicUsize>,
        gate: Mutex<Option<oneshot::Receiver<Bitmap>>>,
    }

    impl ThumbnailRenderer for Renderer {
        type Picture = Picture;

        fn decode(
            &self,
            bytes: Vec<u8>,
            _mime: &str,
            width: u32,
            height: u32,
        ) -> impl Future<Output = Result<Picture, String>> + Send {
            self.decoded
                .lock()
                .unwrap()
                .push((bytes.len(), width, height));
            let gate = self.gate.lock().unwrap().take();
            let closes = self.closes.clone();
            async move {
                let bitmap = match gate {
                    Some(gate) => gate.await.map_err(|error| error.to_string())?,
                    None => Bitmap(closes),
                };
                drop(bitmap);
                Ok(Picture)
            }
        }

        fn video_frame(&self, _url: &str) -> impl Future<Output = Result<Picture, String>> + Send {
            std::future::ready(Err("not used by these tests".to_owned()))
        }
    }

    fn png(width: u32, height: u32, length: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; length];
        bytes[..8].copy_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&width.to_be_bytes());
        bytes[20..24].copy_from_slice(&height.to_be_bytes());
        bytes
    }

    async fn thumbnail(
        rpc: &FakeRpc,
        previews: &Previews,
        fetch: &impl RangeFetch,
        renderer: &Renderer,
    ) -> Result<FileThumbnail<Picture>, ThumbnailError> {
        let source = Thumbnails {
            rpc,
            native: previews,
            fetch,
            renderer,
        };
        read_file_thumbnail(source, &reference(), &Signal::new()).await
    }

    #[test]
    fn admits_ordinary_phone_photos_and_rejects_large_automatic_resources() {
        assert!(thumbnail_eligible("image/jpeg", 12 * MEGABYTE));
        assert!(!thumbnail_eligible("image/jpeg", THUMBNAIL_IMAGE_BYTES + 1));
        assert!(!thumbnail_eligible("video/mp4", THUMBNAIL_VIDEO_BYTES + 1));
        assert!(thumbnail_eligible(
            "application/pdf",
            THUMBNAIL_PDF_FILE_BYTES
        ));
        assert!(!thumbnail_eligible("application/pdf", 200 * MEGABYTE));
        assert!(thumbnail_eligible("text/markdown", 200 * MEGABYTE));
        assert!(!thumbnail_eligible("audio/wav", 1024));
    }

    #[tokio::test]
    async fn reads_only_the_actual_first_4kib_of_a_200mib_document_and_keeps_markup_literal() {
        let mut content =
            "# Actual report\n<img src='https://invalid.example/private'>\n".to_owned();
        content.push_str(&"x".repeat(4096 - content.len()));
        let bytes = content.into_bytes();
        let fetch = Serve::new(200 * MEGABYTE, move |begin, last| {
            bytes[begin as usize..=last as usize].to_vec()
        });
        let (rpc, previews, renderer) = (
            daemon(info("text/markdown", 200 * MEGABYTE)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
            Renderer::default(),
        );
        let result = thumbnail(&rpc, &previews, &fetch, &renderer).await;
        let Ok(FileThumbnail::Text(text)) = result else {
            panic!("{result:?}")
        };
        assert!(text.contains("# Actual report\n<img"));
        assert_eq!(fetch.asked(), vec![(0, 4095)]);
        assert_eq!(previews.closed(), 1);
    }

    #[tokio::test]
    async fn rejects_duplicate_range_lengths_and_closes_the_secure_preview() {
        struct Mangled;
        impl RangeFetch for Mangled {
            fn fetch_range(
                &self,
                _url: &str,
                _begin: u64,
                _last: u64,
            ) -> impl Future<Output = Result<RangeReply, String>> + Send {
                std::future::ready(Ok(RangeReply {
                    status: 206,
                    content_range: Some("bytes 0-1/2".to_owned()),
                    content_length: Some("2,2".to_owned()),
                    body: b"hi".to_vec(),
                }))
            }
        }
        let (rpc, previews, renderer) = (
            daemon(info("text/plain", 2)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
            Renderer::default(),
        );
        assert!(
            thumbnail(&rpc, &previews, &Mangled, &renderer)
                .await
                .is_err()
        );
        assert_eq!(previews.closed(), 1);
    }

    #[tokio::test]
    async fn decodes_bounded_12mp_images_after_exact_chunk_reads_and_releases_the_bitmap_and_native_handle()
     {
        let bytes = png(4000, 3000, 600_000);
        let size = bytes.len() as u64;
        let fetch = Serve::new(size, move |begin, last| {
            bytes[begin as usize..=last as usize].to_vec()
        });
        let (rpc, previews, renderer) = (
            daemon(info("image/png", 600_000)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
            Renderer::default(),
        );
        let result = thumbnail(&rpc, &previews, &fetch, &renderer).await;
        assert_eq!(result, Ok(FileThumbnail::Picture(Picture)));
        assert_eq!(
            fetch.asked(),
            vec![(0, 262_143), (262_144, 524_287), (524_288, 599_999)]
        );
        assert_eq!(*renderer.decoded.lock().unwrap(), vec![(600_000, 240, 180)]);
        assert_eq!(renderer.closes.load(Ordering::SeqCst), 1);
        assert_eq!(previews.closed(), 1);
    }

    /// The header says the size, whatever it is; the thumbnail applies its own cap and the viewer its own (the viewer
    /// once inherited the thumbnail's 16 megapixels and called a 20-megapixel photo corrupt). A header with no width or
    /// height is unsupported, not too large.
    #[test]
    fn the_header_says_the_size_and_each_reader_applies_its_own_limit() {
        let photo = png(5472, 3648, 24);
        assert_eq!(
            raster_dimensions(&photo, "image/png"),
            Ok(Dimensions {
                width: 5472,
                height: 3648
            })
        );
        assert_eq!(
            thumbnail_dimensions(&photo, "image/png"),
            Err(ThumbnailError::Limit("Image exceeds thumbnail dimensions."))
        );
        assert!(matches!(
            raster_dimensions(&png(0, 3648, 24), "image/png"),
            Err(ThumbnailError::Unsupported(_))
        ));
    }

    #[tokio::test]
    async fn checks_raster_pixel_limits_before_decode_including_jpeg_gif_webp_dimensions() {
        assert_eq!(
            raster_dimensions(&png(4000, 3000, 24), "image/png"),
            Ok(Dimensions {
                width: 4000,
                height: 3000
            })
        );
        let gif = [71, 73, 70, 56, 57, 97, 128, 2, 104, 1];
        assert_eq!(
            raster_dimensions(&gif, "image/gif"),
            Ok(Dimensions {
                width: 640,
                height: 360
            })
        );
        let jpeg = [255, 216, 255, 192, 0, 7, 8, 1, 104, 2, 128];
        assert_eq!(
            raster_dimensions(&jpeg, "image/jpeg"),
            Ok(Dimensions {
                width: 640,
                height: 360
            })
        );
        let mut webp = vec![0u8; 30];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..16].copy_from_slice(b"WEBPVP8X");
        webp[24..30].copy_from_slice(&[127, 2, 0, 103, 1, 0]);
        assert_eq!(
            raster_dimensions(&webp, "image/webp"),
            Ok(Dimensions {
                width: 640,
                height: 360
            })
        );

        let bytes = png(100_000, 100_000, 24);
        let fetch = Serve::new(24, move |begin, last| {
            bytes[begin as usize..=last as usize].to_vec()
        });
        let (rpc, previews, renderer) = (
            daemon(info("image/png", 24)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
            Renderer::default(),
        );
        let result = thumbnail(&rpc, &previews, &fetch, &renderer).await;
        assert!(
            matches!(&result, Err(ThumbnailError::Limit(message)) if message.contains("dimensions")),
            "{result:?}"
        );
        assert!(renderer.decoded.lock().unwrap().is_empty());
        assert_eq!(previews.closed(), 1);
    }

    #[tokio::test]
    async fn closes_a_bitmap_that_finishes_decoding_after_cancellation_without_drawing_it() {
        let bytes = png(640, 360, 24);
        let fetch = Serve::new(24, move |begin, last| {
            bytes[begin as usize..=last as usize].to_vec()
        });
        let (rpc, previews) = (
            daemon(info("image/png", 24)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
        );
        let renderer = Renderer::default();
        let (late, held) = oneshot::channel();
        *renderer.gate.lock().unwrap() = Some(held);
        let signal = Signal::new();
        let source = Thumbnails {
            rpc: &rpc,
            native: &previews,
            fetch: &fetch,
            renderer: &renderer,
        };
        let asked = AttachmentRef {
            message_id: "message".to_owned(),
            attachment_index: 0,
            attachment_id: None,
        };
        let (result, ()) = tokio::join!(read_file_thumbnail(source, &asked, &signal), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            signal.abort();
            tokio::time::sleep(Duration::from_millis(10)).await;
            late.send(Bitmap(renderer.closes.clone())).ok();
        });
        assert_eq!(result, Err(ThumbnailError::Cancelled));
        assert_eq!(renderer.closes.load(Ordering::SeqCst), 1);
        assert_eq!(previews.closed(), 1);
    }

    #[tokio::test]
    async fn never_allocates_the_pdf_worker_for_a_200mib_automatic_preview() {
        let fetch = Serve::new(200 * MEGABYTE, |begin, last| {
            vec![0; (last - begin + 1) as usize]
        });
        let (rpc, previews, renderer) = (
            daemon(info("application/pdf", 200 * MEGABYTE)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
            Renderer::default(),
        );
        let result = thumbnail(&rpc, &previews, &fetch, &renderer).await;
        assert!(
            matches!(&result, Err(ThumbnailError::Limit(message)) if message.contains("limits")),
            "{result:?}"
        );
        assert!(fetch.asked().is_empty());
        assert_eq!(previews.closed(), 1);
    }

    #[tokio::test]
    async fn stops_pdf_loading_before_range_requests_exceed_2mib_and_retires_its_worker() {
        let chunk = u64::from(ATTACHMENT_CHUNK_BYTES);
        let fetch = Serve::new(4 * MEGABYTE, |begin, last| {
            vec![0; (last - begin + 1) as usize]
        });
        let mut budget = PdfBudget::default();
        let mut queue = PdfRangeQueue::new(&fetch, "local", 4 * MEGABYTE, Signal::new());
        let mut stopped = None;
        for index in 0..9 {
            let (begin, end) = (index * chunk, index * chunk + chunk);
            match budget.request(begin, end) {
                Ok(()) => queue.request(begin, end).unwrap(),
                Err(error) => {
                    stopped = Some(error);
                    break;
                }
            }
        }
        assert_eq!(stopped, Some(ThumbnailError::Cancelled));
        queue.settle(|_, _| {}).await.unwrap();
        assert!(fetch.asked().len() <= 8);
    }

    #[tokio::test]
    async fn retires_an_active_thumbnail_before_foreground_work_and_does_not_admit_queued_cards_while_paused()
     {
        let queue = ThumbnailQueue::new(Duration::from_secs(10), Duration::from_secs(30));
        let signal = Signal::new();
        let (started, began) = oneshot::channel::<()>();
        let (cleaned, cleanup_done) = oneshot::channel::<()>();
        let next = Arc::new(AtomicUsize::new(0));
        let next_work = next.clone();
        let first = queue.queue(&signal, |running| async move {
            let _ = started.send(());
            running.cancelled().await;
            let _ = cleanup_done.await;
            Ok::<(), ThumbnailError>(())
        });
        let second = queue.queue(&signal, move |_| async move {
            next_work.fetch_add(1, Ordering::SeqCst);
            Ok::<(), ThumbnailError>(())
        });
        let (first, second, ()) = tokio::join!(first, second, async {
            began.await.unwrap();
            queue.pause();
            let early =
                tokio::time::timeout(Duration::from_millis(20), queue.ready(&Signal::new())).await;
            assert!(early.is_err(), "ready before the active thumbnail retired");
            cleaned.send(()).unwrap();
            queue.ready(&Signal::new()).await.unwrap();
        });
        assert_eq!(first, Ok(()));
        assert_eq!(second, Err(ThumbnailError::Paused));
        assert_eq!(next.load(Ordering::SeqCst), 0);
        queue.release();
        queue
            .queue(&Signal::new(), |_| async { Ok::<(), ThumbnailError>(()) })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn refuses_text_with_control_characters_as_not_a_text_thumbnail() {
        let bytes = b"ok\x01 text".to_vec();
        let size = bytes.len() as u64;
        let fetch = Serve::new(size, move |begin, last| {
            bytes[begin as usize..=last as usize].to_vec()
        });
        let (rpc, previews, renderer) = (
            daemon(info("text/plain", size)),
            Previews::new("app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"),
            Renderer::default(),
        );
        assert_eq!(
            thumbnail(&rpc, &previews, &fetch, &renderer).await.err(),
            Some(ThumbnailError::NotText)
        );
        assert_eq!(previews.closed(), 1);
    }

    /// A 64 x 64 GIF of `frames` frames (two colours, one tiny LZW block each), with the NETSCAPE2.0 loop extension or
    /// not. The pixels don't matter: what's checked is the structure.
    pub(crate) fn gif(frames: usize, looping: bool) -> Vec<u8> {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[64, 0, 64, 0, 0x80, 0, 0]);
        gif.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        if looping {
            gif.extend_from_slice(&[0x21, 0xFF, 11]);
            gif.extend_from_slice(b"NETSCAPE2.0");
            gif.extend_from_slice(&[3, 1, 0, 0, 0]);
        }
        for _ in 0..frames {
            gif.extend_from_slice(&[0x21, 0xF9, 4, 0, 10, 0, 0, 0]);
            gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 64, 0, 64, 0, 0]);
            gif.extend_from_slice(&[2, 2, 0x4C, 0x01, 0]);
        }
        gif.push(0x3B);
        gif
    }

    /// M4 review C2: an animated GIF or WebP is found from its structure before any decoder sees it.
    #[test]
    fn an_animation_is_found_before_it_is_decoded() {
        assert!(animated(&gif(3, false), "image/gif"), "three frames");
        assert!(animated(&gif(1, true), "image/gif"), "one frame that loops");
        assert!(
            animated(&gif(1000, false), "image/gif"),
            "a thousand frames"
        );
        assert!(!animated(&gif(1, false), "image/gif"), "a still GIF");
        assert!(
            animated(b"GIF89a", "image/gif"),
            "a header with nothing after it reads as unsafe"
        );
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
        webp.extend_from_slice(&[0x02, 0, 0, 0, 63, 0, 0, 63, 0, 0]);
        assert!(
            animated(&webp, "image/webp"),
            "a VP8X with the animation flag"
        );
        webp[20] = 0x10;
        assert!(!animated(&webp, "image/webp"), "a VP8X without it");
        assert!(
            !animated(&gif(3, false), "image/png"),
            "only GIF and WebP animate"
        );
    }
}
