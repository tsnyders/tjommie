//! A shared file read straight from the daemon (`attachments.open`, `.read`, `.close`): the bytes of one attachment,
//! a 256 KiB chunk at a time, checked against the transfer as the model checks them (`attachment_bytes`). The calls are
//! the store's requests, which run on any executor; nothing here waits on a Tokio timer.

use std::sync::Arc;

use client::Rpc as _;
use protocol::attachments::{
    ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentInfo, AttachmentOpen, AttachmentRead,
    AttachmentRef, TransferId,
};
use protocol::wire::method;

use crate::model::attachment_transfer::{TransferError, attachment_bytes, attachment_error};
use crate::model::document_ranges::{RangeFetch, RangeReply};
use crate::ui::store::StoreRpc;

/// One open transfer on the daemon. Close it when done; the daemon also expires it.
pub struct Opened {
    rpc: StoreRpc,
    open: AttachmentOpen,
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
        Ok(Arc::new(Opened { rpc, open }))
    }

    pub fn info(&self) -> &AttachmentInfo {
        &self.open.attachment
    }

    /// `length` bytes from `offset` (at most a chunk; fewer at the end).
    pub async fn read(&self, offset: u64, length: u32) -> Result<Vec<u8>, String> {
        let length = length.min(ATTACHMENT_CHUNK_BYTES);
        let chunk: AttachmentChunk = self
            .rpc
            .call::<method::AttachmentsRead>(AttachmentRead {
                transfer_id: self.open.transfer_id.clone(),
                offset,
                length,
            })
            .await
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

    /// The whole file, refused past `limit` bytes.
    pub async fn read_all(&self, limit: u64) -> Result<Vec<u8>, String> {
        let size = self.info().size;
        if size > limit {
            return Err("This file is too large to preview. Download is still available.".into());
        }
        self.read_span(0, size).await
    }

    pub async fn close(&self) {
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
        Ok(RangeReply {
            status: 206,
            content_range: Some(format!("bytes {begin}-{last}/{size}")),
            content_length: Some(bytes.len().to_string()),
            body: bytes,
        })
    }
}
