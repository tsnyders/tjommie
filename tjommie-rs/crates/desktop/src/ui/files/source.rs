//! A shared file read straight from the daemon (`attachments.open`, `.read`, `.close`): the bytes of one attachment,
//! a 256 KiB chunk at a time, checked against the transfer as the model checks them (`attachment_bytes`). The calls are
//! the store's requests, which run on any executor; nothing here waits on a Tokio timer. Once cancelled or closed, the
//! transfer is read no more: a read fails at once, and one waiting on the daemon is let go.

use std::sync::Arc;

use client::Rpc as _;
use protocol::attachments::{
    ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentInfo, AttachmentOpen, AttachmentRead,
    AttachmentRef, TransferId,
};
use protocol::wire::method;

use crate::model::attachment_transfer::{TransferError, attachment_bytes, attachment_error};
use crate::model::document_ranges::{RangeFetch, RangeReply, Signal, until_aborted};
use crate::ui::store::StoreRpc;

/// One open transfer on the daemon. Close it when done; the daemon also expires it.
pub struct Opened {
    rpc: StoreRpc,
    open: AttachmentOpen,
    /// Aborted when the reads are given up. pdfium reads through the transfer under its process-wide lock, which every
    /// other PDF waits on, so a closed viewer's reads must stop at once rather than run on to the daemon (M4 review B4).
    cancelled: Signal,
}

/// A failed call in the app's words (`attachmentError`), or the call's own when it has none.
fn said(error: client::RpcError) -> String {
    let error = TransferError::Rpc(error);
    match attachment_error(&error) {
        "" => error.to_string(),
        words => words.to_owned(),
    }
}

impl Opened {
    /// Opens `reference` for reading.
    pub async fn open(rpc: StoreRpc, reference: AttachmentRef) -> Result<Arc<Opened>, String> {
        let open = rpc
            .call::<method::AttachmentsOpen>(reference)
            .await
            .map_err(said)?;
        Ok(Arc::new(Opened {
            rpc,
            open,
            cancelled: Signal::new(),
        }))
    }

    pub fn info(&self) -> &AttachmentInfo {
        &self.open.attachment
    }

    /// `length` bytes from `offset` (at most a chunk; fewer at the end). Once the reads are cancelled, an error at once,
    /// without asking the daemon.
    pub async fn read(&self, offset: u64, length: u32) -> Result<Vec<u8>, String> {
        let length = length.min(ATTACHMENT_CHUNK_BYTES);
        let call = self.rpc.call::<method::AttachmentsRead>(AttachmentRead {
            transfer_id: self.open.transfer_id.clone(),
            offset,
            length,
        });
        let chunk: AttachmentChunk = until_aborted(&self.cancelled, call)
            .await
            .map_err(|_| "The file's preview has closed.".to_owned())?
            .map_err(said)?;
        attachment_bytes(&chunk, &self.open, offset, length).map_err(|error| {
            let code = attachment_error(&error);
            if code.is_empty() {
                error.to_string()
            } else {
                code.to_owned()
            }
        })
    }

    /// `begin..end`, in as many chunks as it takes.
    pub async fn read_span(&self, begin: u64, end: u64) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::with_capacity(usize::try_from(end.saturating_sub(begin)).unwrap_or(0));
        let mut at = begin;
        while at < end {
            let want = u32::try_from((end - at).min(u64::from(ATTACHMENT_CHUNK_BYTES)))
                .unwrap_or(ATTACHMENT_CHUNK_BYTES);
            let chunk = self.read(at, want).await?;
            if chunk.is_empty() {
                break;
            }
            at += u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    /// Gives up the reads: one waiting on the daemon is let go, and any after it fails at once. pdfium, reading under
    /// its lock, unwinds and lets the next PDF in (M4 review B4).
    pub fn cancel(&self) {
        self.cancelled.abort();
    }

    /// Closes the transfer; nothing is read through it after.
    pub async fn close(&self) {
        self.cancel();
        let _ = self
            .rpc
            .call::<method::AttachmentsClose>(TransferId {
                transfer_id: self.open.transfer_id.clone(),
            })
            .await;
    }
}

/// The model's range reads (`markdown_section`, `find_markdown_text`) over the transfer: the URL is the transfer's own.
impl RangeFetch for Opened {
    async fn fetch_range(&self, _url: &str, begin: u64, last: u64) -> Result<RangeReply, String> {
        let size = self.info().size;
        let bytes = self
            .read_span(begin, last.saturating_add(1).min(size))
            .await?;
        // The range the bytes cover, which is shorter than asked at the file's end (M4 review L5).
        let end = begin.saturating_add(bytes.len() as u64).saturating_sub(1);
        Ok(RangeReply {
            status: 206,
            content_range: Some(format!("bytes {begin}-{end}/{size}")),
            content_length: Some(bytes.len().to_string()),
            body: bytes,
        })
    }
}

/// The daemon's side of one shared file, for the file views' tests: it opens, serves each chunk asked for, closes, and
/// remembers every call.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use base64::Engine as _;
    use protocol::attachments::{AttachmentChunk, AttachmentInfo, AttachmentOpen, AttachmentRef};
    use protocol::wire::{Empty, method};

    use crate::model::state::AppState;
    use crate::model::testing::FakeRpc;
    use crate::ui::store::{self, StoreRpc};

    /// `bytes` as a shared file of `content_type`, in the store of `cx`: its calls, and what records them.
    pub fn serving(
        cx: &mut gpui::TestAppContext,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> (StoreRpc, Arc<FakeRpc>) {
        let size = bytes.len() as u64;
        let content_type = content_type.to_owned();
        let rpc = FakeRpc::new()
            .on::<method::AttachmentsOpen>(move |_| {
                Ok(AttachmentOpen {
                    transfer_id: "t1".into(),
                    attachment: AttachmentInfo {
                        id: "7d3f5b1e-2c4a-4e6b-9f80-1a2b3c4d5e6f".into(),
                        host_id: "h1".into(),
                        name: "shared".into(),
                        size,
                        content_type: content_type.clone(),
                        sha256: "rev".into(),
                    },
                    chunk_bytes: 262_144,
                    expires_at: "2026-11-15T09:00:00.000Z".into(),
                })
            })
            .on::<method::AttachmentsRead>(move |params| {
                let offset = params["offset"].as_u64().unwrap();
                let end = (offset + params["length"].as_u64().unwrap()).min(size);
                Ok(AttachmentChunk {
                    transfer_id: "t1".into(),
                    revision: "rev".into(),
                    offset,
                    bytes: base64::engine::general_purpose::STANDARD
                        .encode(&bytes[offset as usize..end as usize]),
                    eof: end == size,
                })
            })
            .on::<method::AttachmentsClose>(|_| Ok(Empty {}));
        let calls = cx.update(|cx| store::fake(cx, AppState::default(), rpc));
        (cx.update(|cx| store::rpc(cx)), calls)
    }

    /// How many times `method` was called.
    pub fn asked(calls: &FakeRpc, method: &str) -> usize {
        calls
            .calls()
            .iter()
            .filter(|(name, _)| *name == method)
            .count()
    }

    pub fn reference() -> AttachmentRef {
        AttachmentRef {
            message_id: "m1".into(),
            attachment_index: 0,
            attachment_id: None,
        }
    }

    /// A PNG's header for a `width` × `height` picture, padded to `length` bytes: all `raster_dimensions` reads of it.
    pub fn png(width: u32, height: u32, length: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; length];
        bytes[..8].copy_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&width.to_be_bytes());
        bytes[20..24].copy_from_slice(&height.to_be_bytes());
        bytes
    }
}
