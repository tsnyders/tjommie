//! Port of `packages/ui/lib/attachment-transfer.ts` (T1.4): a bounded client for one shared file. The daemon hands out chunks
//! of at most 256 KiB; the native side (Android's file and preview APIs) picks the save destination and writes, or serves a
//! preview that asks for bytes as it needs them. TypeScript's detached promises (a daemon or native reply that arrives after
//! the user gave up, which must still be closed) are [`Retirements`]: futures the caller drives. The `AbortSignal` is
//! [`Signal`]; the native bridge is [`AttachmentNative`].

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use client::{Rpc, RpcError};
use protocol::ErrorCode;
use protocol::attachments::{
    ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentInfo, AttachmentOpen, AttachmentRead,
    AttachmentRef, TransferId,
};
use protocol::wire::{Empty, method};
use tokio::sync::OnceCell;

use super::document_ranges::Signal;

/// How long the daemon may take to answer a call before the transfer gives up on it.
pub const HOST_REPLY: Duration = Duration::from_secs(30);
const HOST_SILENT: &str = "The file host did not answer.";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The detached work of abandoned calls (TypeScript's un-awaited promises). The caller must drive it, so that a late
/// transfer or preview is still closed.
pub type Retirements<'a> = Vec<BoxFuture<'a, ()>>;

/// Why a transfer step failed.
#[derive(Debug, Clone, PartialEq)]
pub enum TransferError {
    /// The signal was aborted (the TypeScript's `AbortError`).
    Cancelled,
    /// A daemon call failed: a daemon error, or the connection closed.
    Rpc(RpcError),
    /// The native bridge failed, with the code it reported, if any.
    Native {
        code: Option<String>,
        message: String,
    },
    Failed(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("Cancelled"),
            Self::Rpc(error) => error.fmt(formatter),
            Self::Native { message, .. } | Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for TransferError {}

/// The words the app shows for a failed open or save, by the error's code.
pub fn attachment_error(error: &TransferError) -> &'static str {
    if matches!(error, TransferError::Rpc(RpcError::Closed(_))) {
        return "The file host is offline. Reconnect and try again.";
    }
    let (code, from_daemon) = match error {
        TransferError::Rpc(RpcError::Daemon { code, .. }) => (daemon_code_name(*code), true),
        TransferError::Native {
            code: Some(code), ..
        } => (code_name(code), false),
        _ => ("", false),
    };
    match code {
        "forbidden" => "This device no longer has permission to read the file.",
        "unavailable" if from_daemon => {
            "This shared file is unavailable; it may have been removed."
        }
        "unavailable" => "The file or save destination is unavailable.",
        "corrupt" => "The file failed its content check. Open it again or request a fresh share.",
        "expired" => "This file transfer expired. Open the file again.",
        "busy" => "Two file transfers are already active. Close one and try again.",
        "cleanup_failed" => {
            "The save provider kept an incomplete file. Remove it from the selected destination."
        }
        _ => "The file could not be opened or saved. Check the connection and try again.",
    }
}

fn daemon_code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Forbidden => "forbidden",
        ErrorCode::Unavailable => "unavailable",
        ErrorCode::Corrupt => "corrupt",
        ErrorCode::Expired => "expired",
        ErrorCode::Busy => "busy",
        _ => "",
    }
}

fn code_name(text: &str) -> &'static str {
    match text {
        "forbidden" => "forbidden",
        "unavailable" => "unavailable",
        "corrupt" => "corrupt",
        "expired" => "expired",
        "busy" => "busy",
        "cleanup_failed" => "cleanup_failed",
        _ => "",
    }
}

/// `attachmentBytes`: the bytes of one chunk, checked against the transfer and the offset and length asked for.
pub fn attachment_bytes(
    chunk: &AttachmentChunk,
    transfer: &AttachmentOpen,
    offset: u64,
    length: u32,
) -> Result<Vec<u8>, TransferError> {
    let invalid = || TransferError::Failed("The file host returned an invalid chunk.".to_owned());
    let size = transfer.attachment.size;
    if offset > size {
        return Err(invalid());
    }
    let expected = u64::from(length).min(size - offset);
    if chunk.transfer_id != transfer.transfer_id
        || chunk.revision != transfer.attachment.sha256
        || chunk.offset != offset
        || chunk.eof != (offset + expected == size)
    {
        return Err(invalid());
    }
    let invalid_bytes =
        || TransferError::Failed("The file host returned invalid bytes.".to_owned());
    // Strict base64, round-tripped: atob would accept what the host should not send.
    let bytes = STANDARD.decode(&chunk.bytes).map_err(|_| invalid_bytes())?;
    if u64::try_from(bytes.len()) != Ok(expected)
        || bytes.len() > ATTACHMENT_CHUNK_BYTES as usize
        || STANDARD.encode(&bytes) != chunk.bytes
    {
        return Err(invalid_bytes());
    }
    Ok(bytes)
}

/// `untilAbort` for a daemon call that is simply dropped if it loses: the signal, or the host's silence past `deadline`.
async fn host_call<T>(
    signal: &Signal,
    deadline: Duration,
    call: impl Future<Output = Result<T, RpcError>>,
) -> Result<T, TransferError> {
    if signal.aborted() {
        return Err(TransferError::Cancelled);
    }
    tokio::select! {
        biased;
        () = signal.cancelled() => Err(TransferError::Cancelled),
        () = tokio::time::sleep(deadline) => Err(TransferError::Failed(HOST_SILENT.to_owned())),
        reply = call => reply.map_err(TransferError::Rpc),
    }
}

/// `untilAbort` without a deadline: a native picker or write may take as long as the user needs.
async fn abortable<T>(
    signal: &Signal,
    future: impl Future<Output = T>,
) -> Result<T, TransferError> {
    if signal.aborted() {
        return Err(TransferError::Cancelled);
    }
    tokio::select! {
        biased;
        () = signal.cancelled() => Err(TransferError::Cancelled),
        value = future => Ok(value),
    }
}

async fn close_transfer<R: Rpc>(rpc: &R, transfer_id: String) -> Result<(), TransferError> {
    rpc.call::<method::AttachmentsClose>(TransferId { transfer_id })
        .await
        .map(|_: Empty| ())
        .map_err(TransferError::Rpc)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStatus {
    Saved,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePreview {
    pub preview_id: String,
    pub url: String,
}

/// One request from the native preview for bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRead {
    pub preview_id: String,
    pub request_id: String,
    pub offset: u64,
    pub length: u32,
}

/// The native side, as the TypeScript `AttachmentNativeApi`. Each method is one bridge request. Bytes are decoded, not base64.
pub trait AttachmentNative: Sync {
    /// `startSave`: the picker's save id, or `None` when the user cancels it.
    fn start_save(
        &self,
        info: AttachmentInfo,
    ) -> impl Future<Output = Result<Option<String>, TransferError>> + Send;
    /// `activateSave`: the destination is ready for bytes.
    fn activate_save(
        &self,
        _save_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send {
        std::future::ready(Ok(()))
    }
    fn append_save(
        &self,
        save_id: &str,
        offset: u64,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<(), TransferError>> + Send;
    fn finish_save(
        &self,
        save_id: &str,
    ) -> impl Future<Output = Result<SaveStatus, TransferError>> + Send;
    fn cancel_save(&self, save_id: &str) -> impl Future<Output = Result<(), TransferError>> + Send;
    fn preview(
        &self,
        info: AttachmentInfo,
    ) -> impl Future<Output = Result<NativePreview, TransferError>> + Send;
    fn close_preview(
        &self,
        preview_id: &str,
    ) -> impl Future<Output = Result<(), TransferError>> + Send;
    /// `respond`: the bytes for a read request, or `None` when it failed.
    fn respond(
        &self,
        preview_id: &str,
        request_id: &str,
        bytes: Option<Vec<u8>>,
    ) -> impl Future<Output = Result<(), TransferError>> + Send;
}

/// One open transfer on the daemon. Reads are one at a time; closing happens once.
pub struct AttachmentTransfer<'a, R> {
    rpc: &'a R,
    open: AttachmentOpen,
    signal: Signal,
    host_reply: Duration,
    closed: AtomicBool,
    reading: AtomicBool,
}

impl<'a, R: Rpc> AttachmentTransfer<'a, R> {
    /// `AttachmentTransfer.open`. A reply that comes after the signal gave up is closed by the retirement pushed to `retire`.
    pub async fn open(
        rpc: &'a R,
        reference: &AttachmentRef,
        signal: &Signal,
        host_reply: Duration,
        retire: &mut Retirements<'a>,
    ) -> Result<Self, TransferError> {
        if signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        let mut pending: BoxFuture<'a, Result<AttachmentOpen, RpcError>> =
            Box::pin(rpc.call::<method::AttachmentsOpen>(reference.clone()));
        let settled = tokio::select! {
            biased;
            () = signal.cancelled() => Err(TransferError::Cancelled),
            () = tokio::time::sleep(host_reply) => Err(TransferError::Failed(HOST_SILENT.to_owned())),
            reply = &mut pending => Ok(reply),
        };
        let open = match settled {
            Ok(reply) => reply.map_err(TransferError::Rpc)?,
            Err(error) => {
                retire.push(Box::pin(async move {
                    if let Ok(late) = pending.await {
                        let _ = close_transfer(rpc, late.transfer_id).await;
                    }
                }));
                return Err(error);
            }
        };
        if signal.aborted() {
            let _ = close_transfer(rpc, open.transfer_id).await;
            return Err(TransferError::Cancelled);
        }
        Ok(Self {
            rpc,
            open,
            signal: signal.clone(),
            host_reply,
            closed: AtomicBool::new(false),
            reading: AtomicBool::new(false),
        })
    }

    pub fn info(&self) -> &AttachmentInfo {
        &self.open.attachment
    }

    /// `read`: the decoded bytes of `length` at `offset`. A second read while one is in flight, or past the end, is refused.
    pub async fn read(&self, offset: u64, length: u32) -> Result<Vec<u8>, TransferError> {
        if self.closed.load(Ordering::SeqCst) || self.signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        if self.reading.load(Ordering::SeqCst)
            || offset > self.open.attachment.size
            || !(1..=ATTACHMENT_CHUNK_BYTES).contains(&length)
        {
            return Err(TransferError::Failed(
                "Invalid or overlapping file read.".to_owned(),
            ));
        }
        self.reading.store(true, Ordering::SeqCst);
        let request = AttachmentRead {
            transfer_id: self.open.transfer_id.clone(),
            offset,
            length,
        };
        let reply = host_call(
            &self.signal,
            self.host_reply,
            self.rpc.call::<method::AttachmentsRead>(request),
        )
        .await;
        self.reading.store(false, Ordering::SeqCst);
        let chunk = reply?;
        if self.closed.load(Ordering::SeqCst) || self.signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        attachment_bytes(&chunk, &self.open, offset, length)
    }

    /// `close`: once. Later calls succeed without a second close on the daemon.
    pub async fn close(&self) -> Result<(), TransferError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        close_transfer(self.rpc, self.open.transfer_id.clone()).await
    }
}

/// The connection a download reacquires once the picker has answered.
pub type Reacquire<'a, R> =
    Box<dyn FnOnce() -> BoxFuture<'a, Result<&'a R, TransferError>> + Send + 'a>;

/// What a download needs beyond its inputs.
pub struct DownloadOptions<'a, R> {
    /// How long the daemon may take to answer a call.
    pub host_reply: Duration,
    /// Once the picker has answered, the connection the transfer binds to, when the one it had is gone (TypeScript's `reacquire`).
    pub reacquire: Option<Reacquire<'a, R>>,
    /// Told before the transfer opens whether it is on a reacquired connection (TypeScript's `transferring`).
    pub transferring: Option<Box<dyn FnMut(bool) + Send + 'a>>,
}

/// `downloadAttachment`: saves the file to the destination the user picks, a chunk at a time. The picker may take as long as
/// it likes; the daemon's silence is timed only while the transfer is open. Whatever was started is cancelled on every way
/// out, and a cleanup failure is reported unless the save was published.
pub async fn download_attachment<'a, R: Rpc, N: AttachmentNative>(
    rpc: &'a R,
    reference: &AttachmentRef,
    native: &'a N,
    signal: &Signal,
    mut options: DownloadOptions<'a, R>,
    retire: &mut Retirements<'a>,
    mut progress: impl FnMut(u64, u64),
) -> Result<SaveStatus, TransferError> {
    let info = host_call(
        signal,
        options.host_reply,
        rpc.call::<method::AttachmentsMetadata>(reference.clone()),
    )
    .await?;
    let mut save_id: Option<String> = None;
    let mut transfer: Option<AttachmentTransfer<'a, R>> = None;
    let outcome = async {
        let mut starting: BoxFuture<'a, Result<Option<String>, TransferError>> =
            Box::pin(native.start_save(info.clone()));
        let picked = tokio::select! {
            biased;
            () = signal.cancelled() => None,
            reply = &mut starting => Some(reply),
        };
        let Some(picked) = picked else {
            retire.push(Box::pin(async move {
                if let Ok(Some(late)) = starting.await {
                    let _ = native.cancel_save(&late).await;
                }
            }));
            return Err(TransferError::Cancelled);
        };
        let Some(id) = picked? else {
            return Ok(SaveStatus::Cancelled);
        };
        save_id = Some(id.clone());
        if signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        let fresh: Option<&'a R> = match options.reacquire.take() {
            Some(reacquire) => abortable(signal, reacquire()).await??.into(),
            None => None,
        };
        if signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        let current: &'a R = fresh.unwrap_or(rpc);
        if let Some(bind) = options.transferring.as_mut() {
            bind(fresh.is_some());
        }
        if signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        let opened =
            AttachmentTransfer::open(current, reference, signal, options.host_reply, retire)
                .await?;
        let ready = transfer.insert(opened);
        let size = ready.info().size;
        if ready.info() != &info {
            return Err(TransferError::Failed(
                "The shared file changed while choosing a destination.".to_owned(),
            ));
        }
        if signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        abortable(signal, native.activate_save(&id)).await??;
        let mut offset = 0u64;
        while offset < size {
            let Some(open) = transfer.as_ref() else {
                return Err(TransferError::Cancelled);
            };
            let bytes = open.read(offset, ATTACHMENT_CHUNK_BYTES).await?;
            if signal.aborted() {
                return Err(TransferError::Cancelled);
            }
            let written = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            abortable(signal, native.append_save(&id, offset, bytes)).await??;
            offset += written;
            progress(offset, size);
        }
        if signal.aborted() {
            return Err(TransferError::Cancelled);
        }
        abortable(signal, native.finish_save(&id)).await?
    }
    .await;

    let cleanup: Option<TransferError> = match save_id.as_deref() {
        Some(id) => native.cancel_save(id).await.err(),
        None => None,
    };
    if let Some(open) = transfer.as_ref() {
        let _ = open.close().await;
    }
    let published = outcome == Ok(SaveStatus::Saved);
    match cleanup {
        Some(error) if !published => Err(error),
        _ => outcome,
    }
}

/// `untilAbort` for a native preview open: an abandoned open is closed by the retirement pushed to `retire`.
async fn open_preview<'a, N: AttachmentNative>(
    native: &'a N,
    info: AttachmentInfo,
    signal: &Signal,
    host_reply: Duration,
    retire: &mut Retirements<'a>,
) -> Result<NativePreview, TransferError> {
    if signal.aborted() {
        return Err(TransferError::Cancelled);
    }
    let mut opening: BoxFuture<'a, Result<NativePreview, TransferError>> =
        Box::pin(native.preview(info));
    let settled = tokio::select! {
        biased;
        () = signal.cancelled() => Err(TransferError::Cancelled),
        () = tokio::time::sleep(host_reply) => Err(TransferError::Failed(HOST_SILENT.to_owned())),
        reply = &mut opening => Ok(reply),
    };
    match settled {
        Ok(reply) => {
            let preview = reply?;
            if signal.aborted() {
                let _ = native.close_preview(&preview.preview_id).await;
                return Err(TransferError::Cancelled);
            }
            Ok(preview)
        }
        Err(error) => {
            retire.push(Box::pin(async move {
                if let Ok(late) = opening.await {
                    let _ = native.close_preview(&late.preview_id).await;
                }
            }));
            Err(error)
        }
    }
}

/// The preview's URL must be the app's own handle for this preview and nothing else.
fn is_preview_handle(url: &str, preview_id: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let own_origin = (parsed.scheme() == "app" && parsed.host_str() == Some("tjommie"))
        || (parsed.scheme() == "https" && parsed.host_str() == Some("localhost"));
    own_origin
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && is_uuid_text(preview_id)
        && parsed.path() == format!("/_attachments/{preview_id}")
}

fn is_uuid_text(text: &str) -> bool {
    text.len() == 36
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// `previewAttachment`: opens the file for the native preview, which asks for its bytes through [`AttachmentPreview::serve`].
pub async fn preview_attachment<'a, R: Rpc, N: AttachmentNative>(
    rpc: &'a R,
    reference: &AttachmentRef,
    native: &'a N,
    signal: &Signal,
    host_reply: Duration,
    retire: &mut Retirements<'a>,
) -> Result<AttachmentPreview<'a, R, N>, TransferError> {
    let transfer = AttachmentTransfer::open(rpc, reference, signal, host_reply, retire).await?;
    let preview =
        match open_preview(native, transfer.info().clone(), signal, host_reply, retire).await {
            Ok(preview) => preview,
            Err(error) => {
                let _ = transfer.close().await;
                return Err(error);
            }
        };
    if !is_preview_handle(&preview.url, &preview.preview_id) {
        let _ = native.close_preview(&preview.preview_id).await;
        let _ = transfer.close().await;
        return Err(TransferError::Failed(
            "The device returned an invalid preview handle.".to_owned(),
        ));
    }
    Ok(AttachmentPreview {
        native,
        transfer,
        preview_id: preview.preview_id,
        url: preview.url,
        closing: OnceCell::new(),
    })
}

/// An open preview. Closing is shared: every caller gets the first close's outcome, and the native handle is closed once.
pub struct AttachmentPreview<'a, R: Rpc, N: AttachmentNative> {
    native: &'a N,
    transfer: AttachmentTransfer<'a, R>,
    preview_id: String,
    url: String,
    closing: OnceCell<Result<(), TransferError>>,
}

impl<'a, R: Rpc, N: AttachmentNative> AttachmentPreview<'a, R, N> {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn attachment(&self) -> &AttachmentInfo {
        self.transfer.info()
    }

    /// One native read request. Another preview's requests are ignored, and nothing is served once closing has begun. A read
    /// that fails goes to `on_error` (not when the signal was the cause) and is answered with no bytes.
    pub async fn serve(
        &self,
        request: &NativeRead,
        on_error: &mut dyn FnMut(&TransferError),
    ) -> Result<(), TransferError> {
        if request.preview_id != self.preview_id || self.closing.initialized() {
            return Ok(());
        }
        let bytes = match self.transfer.read(request.offset, request.length).await {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                if error != TransferError::Cancelled {
                    on_error(&error);
                }
                None
            }
        };
        self.native
            .respond(&self.preview_id, &request.request_id, bytes)
            .await
    }

    pub async fn close(&self) -> Result<(), TransferError> {
        self.closing
            .get_or_init(|| async {
                let failure = self.native.close_preview(&self.preview_id).await.err();
                failure
                    .or(self.transfer.close().await.err())
                    .map_or(Ok(()), Err)
            })
            .await
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use client::{ClientMethod, Rpc, RpcError};
    use protocol::ErrorCode;
    use protocol::attachments::{
        ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentInfo, AttachmentOpen, AttachmentRead,
        AttachmentRef,
    };
    use protocol::wire::{Empty, method};
    use serde_json::json;
    use tokio::sync::oneshot;

    use crate::model::testing::FakeRpc;

    use super::*;

    const TRANSFER: &str = "41f8bac7-358c-48cf-9f48-a3a358804da6";
    const CHUNK: u64 = ATTACHMENT_CHUNK_BYTES as u64;

    fn reference() -> &'static AttachmentRef {
        static REFERENCE: std::sync::LazyLock<AttachmentRef> =
            std::sync::LazyLock::new(|| AttachmentRef {
                message_id: "message".to_owned(),
                attachment_index: 0,
                attachment_id: None,
            });
        &REFERENCE
    }

    fn info() -> AttachmentInfo {
        AttachmentInfo {
            id: "83c70656-e163-478e-a934-6a5801f5ef18".to_owned(),
            host_id: "host".to_owned(),
            name: "file.bin".to_owned(),
            size: CHUNK + 7,
            content_type: "application/octet-stream".to_owned(),
            sha256: "a".repeat(64),
        }
    }

    fn opened(attachment: AttachmentInfo) -> AttachmentOpen {
        AttachmentOpen {
            transfer_id: TRANSFER.to_owned(),
            attachment,
            chunk_bytes: ATTACHMENT_CHUNK_BYTES,
            expires_at: "2026-11-03T12:00:00Z".to_owned(),
        }
    }

    /// The daemon: metadata is the file, `open` answers `open_reply`, and reads return 'a' bytes.
    fn daemon(open_reply: AttachmentOpen) -> FakeRpc {
        FakeRpc::new()
            .on::<method::AttachmentsMetadata>(|_| Ok(info()))
            .on::<method::AttachmentsOpen>(move |_| Ok(open_reply.clone()))
            .on::<method::AttachmentsRead>(|params| {
                let read: AttachmentRead = serde_json::from_value(params)
                    .map_err(|error| RpcError::BadParams(error.to_string()))?;
                let size = (CHUNK + 7)
                    .saturating_sub(read.offset)
                    .min(u64::from(read.length));
                let bytes = vec![97u8; size as usize];
                Ok(AttachmentChunk {
                    transfer_id: TRANSFER.to_owned(),
                    revision: "a".repeat(64),
                    offset: read.offset,
                    bytes: base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        bytes,
                    ),
                    eof: read.offset + size == CHUNK + 7,
                })
            })
            .on::<method::AttachmentsClose>(|_| Ok(Empty {}))
    }

    fn healthy() -> FakeRpc {
        daemon(opened(info()))
    }

    /// A daemon whose `attachments.open` answers only when the test sends the reply (a late reply); the rest is `inner`.
    struct LateOpen {
        reply: Mutex<Option<oneshot::Receiver<AttachmentOpen>>>,
        inner: FakeRpc,
    }

    impl Rpc for LateOpen {
        fn call<M: ClientMethod>(
            &self,
            params: M::Params,
        ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
            let late = if M::NAME == "attachments.open" {
                self.reply.lock().unwrap().take()
            } else {
                None
            };
            let inner = if late.is_none() {
                Some(self.inner.call::<M>(params))
            } else {
                None
            };
            async move {
                match (late, inner) {
                    (Some(receiver), _) => {
                        let open = receiver
                            .await
                            .map_err(|_| RpcError::BadResult("the reply never came".to_owned()))?;
                        let value = serde_json::to_value(open)
                            .map_err(|error| RpcError::BadResult(error.to_string()))?;
                        serde_json::from_value(value)
                            .map_err(|error| RpcError::BadResult(error.to_string()))
                    }
                    (None, Some(inner)) => inner.await,
                    (None, None) => Err(RpcError::BadResult("no reply".to_owned())),
                }
            }
        }
    }

    /// The native side: a picker that answers when the test says so, and records every call as (name, argument).
    struct Native {
        picker: Mutex<Option<oneshot::Receiver<Option<String>>>>,
        save: Option<String>,
        finish_gate: Mutex<Option<oneshot::Receiver<SaveStatus>>>,
        finish: SaveStatus,
        activation_error: Option<String>,
        cancel_error: Option<TransferError>,
        abort_on_start: Option<Signal>,
        close_gate: Mutex<Option<oneshot::Receiver<()>>>,
        close_error: Option<TransferError>,
        preview: NativePreview,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl Native {
        fn new() -> Self {
            Self {
                picker: Mutex::new(None),
                save: Some("native-save".to_owned()),
                finish_gate: Mutex::new(None),
                finish: SaveStatus::Saved,
                activation_error: None,
                cancel_error: None,
                abort_on_start: None,
                close_gate: Mutex::new(None),
                close_error: None,
                preview: NativePreview {
                    preview_id: "624d43d7-1a49-4d79-8d1d-85d846cfbd8e".to_owned(),
                    url: "app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e"
                        .to_owned(),
                },
                calls: Mutex::new(Vec::new()),
            }
        }

        fn note(&self, name: &str, argument: impl Into<String>) {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_owned(), argument.into()));
        }

        fn count(&self, name: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(called, _)| called == name)
                .count()
        }

        fn saw(&self, name: &str, argument: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .any(|(called, value)| called == name && value == argument)
        }
    }

    impl AttachmentNative for Native {
        fn start_save(
            &self,
            _info: AttachmentInfo,
        ) -> impl Future<Output = Result<Option<String>, TransferError>> + Send {
            self.note("start", "");
            let picker = self.picker.lock().unwrap().take();
            let answer = self.save.clone();
            let abort = self.abort_on_start.clone();
            async move {
                if let Some(picker) = picker {
                    return Ok(picker.await.unwrap_or(None));
                }
                if let Some(signal) = abort {
                    signal.abort();
                }
                Ok(answer)
            }
        }

        fn activate_save(
            &self,
            save_id: &str,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            self.note("activate", save_id);
            let failure = self.activation_error.clone();
            async move { failure.map_or(Ok(()), |message| Err(TransferError::Failed(message))) }
        }

        fn append_save(
            &self,
            _save_id: &str,
            offset: u64,
            _bytes: Vec<u8>,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            self.note("append", offset.to_string());
            std::future::ready(Ok(()))
        }

        fn finish_save(
            &self,
            save_id: &str,
        ) -> impl Future<Output = Result<SaveStatus, TransferError>> + Send {
            self.note("finish", save_id);
            let gate = self.finish_gate.lock().unwrap().take();
            let answer = self.finish;
            async move {
                match gate {
                    Some(gate) => Ok(gate.await.unwrap_or(SaveStatus::Cancelled)),
                    None => Ok(answer),
                }
            }
        }

        fn cancel_save(
            &self,
            save_id: &str,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            self.note("cancel", save_id);
            let failure = self.cancel_error.clone();
            std::future::ready(failure.map_or(Ok(()), Err))
        }

        fn preview(
            &self,
            _info: AttachmentInfo,
        ) -> impl Future<Output = Result<NativePreview, TransferError>> + Send {
            self.note("preview", "");
            std::future::ready(Ok(self.preview.clone()))
        }

        fn close_preview(
            &self,
            preview_id: &str,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            self.note("close_preview", preview_id);
            let gate = self.close_gate.lock().unwrap().take();
            let failure = self.close_error.clone();
            async move {
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                failure.map_or(Ok(()), Err)
            }
        }

        fn respond(
            &self,
            preview_id: &str,
            request_id: &str,
            bytes: Option<Vec<u8>>,
        ) -> impl Future<Output = Result<(), TransferError>> + Send {
            let shown = bytes.map_or("null".to_owned(), |bytes| {
                String::from_utf8_lossy(&bytes).into_owned()
            });
            self.note("respond", format!("{preview_id}|{request_id}|{shown}"));
            std::future::ready(Ok(()))
        }
    }

    fn options<'a, R>() -> DownloadOptions<'a, R> {
        DownloadOptions {
            host_reply: HOST_REPLY,
            reacquire: None,
            transferring: None,
        }
    }

    /// `reacquire` answering with `client` (the reconnected daemon).
    fn reacquire_with<'a>(
        client: &'a FakeRpc,
    ) -> Box<dyn FnOnce() -> BoxFuture<'a, Result<&'a FakeRpc, TransferError>> + Send + 'a> {
        Box::new(
            move || -> BoxFuture<'a, Result<&'a FakeRpc, TransferError>> {
                Box::pin(async move { Ok(client) })
            },
        )
    }

    /// `reacquire` that is refused, as when the authorization is gone.
    fn reacquire_refused<'a>()
    -> Box<dyn FnOnce() -> BoxFuture<'a, Result<&'a FakeRpc, TransferError>> + Send + 'a> {
        Box::new(|| -> BoxFuture<'a, Result<&'a FakeRpc, TransferError>> {
            Box::pin(async { Err(TransferError::Failed("Authorization refused".to_owned())) })
        })
    }

    #[tokio::test]
    async fn reports_failed_uri_cleanup_after_reauth_revision_or_activation_fails_and_still_closes_its_transfer()
     {
        for phase in ["reauth", "revision", "activation"] {
            let rpc = if phase == "revision" {
                daemon(opened(AttachmentInfo {
                    sha256: "b".repeat(64),
                    ..info()
                }))
            } else {
                healthy()
            };
            let mut native = Native::new();
            native.cancel_error = Some(TransferError::Native {
                code: Some("cleanup_failed".to_owned()),
                message: "Synthetic provider retained document".to_owned(),
            });
            if phase == "activation" {
                native.activation_error = Some("Activation refused".to_owned());
            }
            let mut options = options();
            if phase == "reauth" {
                options.reacquire = Some(reacquire_refused());
            }
            let mut retire = Vec::new();
            let result = download_attachment(
                &rpc,
                reference(),
                &native,
                &Signal::new(),
                options,
                &mut retire,
                |_, _| {},
            )
            .await;
            assert!(
                matches!(&result, Err(TransferError::Native { code: Some(code), .. }) if code == "cleanup_failed"),
                "{phase}: {result:?}"
            );
            assert_eq!(native.count("append"), 0, "{phase}");
            if phase != "reauth" {
                assert_eq!(
                    rpc.calls().last().map(|(name, _)| *name),
                    Some("attachments.close"),
                    "{phase}"
                );
            }
        }
    }

    #[tokio::test]
    async fn holds_no_transfer_while_a_ten_minute_picker_waits_then_opens_on_the_reacquired_client()
    {
        let first = healthy();
        let fresh = healthy();
        let native = Native::new();
        let (choose, picked) = oneshot::channel();
        *native.picker.lock().unwrap() = Some(picked);
        let phases = Arc::new(Mutex::new(Vec::new()));
        let sink = phases.clone();
        let mut options = options();
        options.reacquire = Some(reacquire_with(&fresh));
        options.transferring = Some(Box::new(move |reacquired| {
            sink.lock().unwrap().push(reacquired)
        }));
        let signal = Signal::new();
        let mut retire = Vec::new();
        let (result, ()) = tokio::join!(
            download_attachment(
                &first,
                reference(),
                &native,
                &signal,
                options,
                &mut retire,
                |_, _| {}
            ),
            async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                assert_eq!(
                    first
                        .calls()
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>(),
                    vec!["attachments.metadata"]
                );
                assert_eq!(native.count("append"), 0);
                assert_eq!(native.count("activate"), 0);
                choose.send(Some("chosen".to_owned())).unwrap();
            }
        );
        assert_eq!(result, Ok(SaveStatus::Saved));
        assert_eq!(*phases.lock().unwrap(), vec![true]);
        assert!(
            fresh
                .calls()
                .iter()
                .any(|(name, _)| *name == "attachments.open")
        );
        assert!(native.saw("activate", "chosen"));
    }

    #[tokio::test]
    async fn refuses_changed_fields_after_the_picker_before_activating_or_reading() {
        let fields = ["id", "hostId", "name", "size", "contentType", "sha256"];
        for field in fields {
            let mut changed = info();
            match field {
                "id" => changed.id = "changed-id".to_owned(),
                "hostId" => changed.host_id = "changed".to_owned(),
                "name" => changed.name = "changed".to_owned(),
                "size" => changed.size = 12,
                "contentType" => changed.content_type = "changed".to_owned(),
                _ => changed.sha256 = "b".repeat(64),
            }
            let first = healthy();
            let fresh = daemon(opened(changed));
            let mut native = Native::new();
            native.activation_error = None;
            let mut options = options();
            options.reacquire = Some(reacquire_with(&fresh));
            let mut retire = Vec::new();
            let result = download_attachment(
                &first,
                reference(),
                &native,
                &Signal::new(),
                options,
                &mut retire,
                |_, _| {},
            )
            .await;
            assert!(
                matches!(&result, Err(TransferError::Failed(message)) if message.contains("changed")),
                "{field}: {result:?}"
            );
            assert_eq!(native.count("activate"), 0, "{field}");
            assert_eq!(native.count("append"), 0, "{field}");
            assert_eq!(
                fresh.calls().last().map(|(name, _)| *name),
                Some("attachments.close"),
                "{field}"
            );
            assert!(native.saw("cancel", "native-save"), "{field}");
        }
    }

    #[tokio::test]
    async fn enters_transfer_cancellation_before_open_and_closes_a_late_open_after_genuine_loss() {
        let (reply, late) = oneshot::channel();
        let rpc = LateOpen {
            reply: Mutex::new(Some(late)),
            inner: healthy(),
        };
        let native = Native::new();
        let signal = Signal::new();
        let phases = Arc::new(Mutex::new(Vec::new()));
        let sink = phases.clone();
        let mut options = options();
        options.transferring = Some(Box::new(move |_| sink.lock().unwrap().push("transferring")));
        let mut retire = Vec::new();
        let (result, ()) = tokio::join!(
            download_attachment(
                &rpc,
                reference(),
                &native,
                &signal,
                options,
                &mut retire,
                |_, _| {}
            ),
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                signal.abort();
                tokio::time::sleep(Duration::from_millis(10)).await;
                reply.send(opened(info())).unwrap();
            }
        );
        assert_eq!(result, Err(TransferError::Cancelled));
        assert_eq!(*phases.lock().unwrap(), vec!["transferring"]);
        assert_eq!(native.count("append"), 0);
        assert!(native.saw("cancel", "native-save"));
        for cleanup in retire {
            cleanup.await;
        }
        assert!(
            rpc.inner
                .calls()
                .iter()
                .any(|(name, params)| *name == "attachments.close"
                    && params["transferId"] == TRANSFER)
        );
    }

    #[tokio::test]
    async fn lets_a_native_start_or_finish_picker_wait_beyond_the_host_rpc_deadline() {
        for phase in ["start", "finish"] {
            let rpc = healthy();
            let native = Native::new();
            let (send, gate) = oneshot::channel::<Option<String>>();
            let (finish_send, finish_gate) = oneshot::channel::<SaveStatus>();
            if phase == "start" {
                *native.picker.lock().unwrap() = Some(gate);
            } else {
                *native.finish_gate.lock().unwrap() = Some(finish_gate);
            }
            let mut options = options();
            options.host_reply = Duration::from_millis(40);
            let mut retire = Vec::new();
            let signal = Signal::new();
            let (result, ()) = tokio::join!(
                download_attachment(
                    &rpc,
                    reference(),
                    &native,
                    &signal,
                    options,
                    &mut retire,
                    |_, _| {}
                ),
                async {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    if phase == "start" {
                        let _ = send.send(Some("chosen".to_owned()));
                    } else {
                        let _ = finish_send.send(SaveStatus::Saved);
                    }
                }
            );
            assert_eq!(result, Ok(SaveStatus::Saved), "{phase}");
        }
    }

    #[tokio::test]
    async fn streams_downloads_into_the_native_destination_without_assembling_the_file() {
        let rpc = healthy();
        let native = Native::new();
        let progress = Arc::new(Mutex::new(Vec::new()));
        let sink = progress.clone();
        let mut retire = Vec::new();
        let result = download_attachment(
            &rpc,
            reference(),
            &native,
            &Signal::new(),
            options(),
            &mut retire,
            move |written, total| sink.lock().unwrap().push((written, total)),
        )
        .await;
        assert_eq!(result, Ok(SaveStatus::Saved));
        assert_eq!(native.count("append"), 2);
        assert_eq!(
            progress.lock().unwrap().last(),
            Some(&(CHUNK + 7, CHUNK + 7))
        );
        let lengths: Vec<u64> = rpc
            .calls()
            .iter()
            .filter(|(name, _)| *name == "attachments.read")
            .filter_map(|(_, params)| params["length"].as_u64())
            .collect();
        assert_eq!(lengths, vec![CHUNK, CHUNK]);
        assert_eq!(
            rpc.calls().last().map(|(name, _)| *name),
            Some("attachments.close")
        );
    }

    #[tokio::test]
    async fn does_not_write_bytes_after_a_cancelled_picker_or_report_a_cancelled_finish_as_saved() {
        let rpc = healthy();
        {
            let mut native = Native::new();
            native.save = None;
            let mut retire = Vec::new();
            assert_eq!(
                download_attachment(
                    &rpc,
                    reference(),
                    &native,
                    &Signal::new(),
                    options(),
                    &mut retire,
                    |_, _| {}
                )
                .await,
                Ok(SaveStatus::Cancelled)
            );
            assert_eq!(native.count("append"), 0);
        }
        {
            let mut native = Native::new();
            native.finish = SaveStatus::Cancelled;
            let mut retire = Vec::new();
            assert_eq!(
                download_attachment(
                    &rpc,
                    reference(),
                    &native,
                    &Signal::new(),
                    options(),
                    &mut retire,
                    |_, _| {}
                )
                .await,
                Ok(SaveStatus::Cancelled)
            );
        }
    }

    #[test]
    fn rejects_revision_count_offset_transfer_and_base64_corruption() {
        let transfer = opened(info());
        let chunk = AttachmentChunk {
            transfer_id: TRANSFER.to_owned(),
            revision: "a".repeat(64),
            offset: 0,
            bytes: "YQ==".to_owned(),
            eof: false,
        };
        assert_eq!(attachment_bytes(&chunk, &transfer, 0, 1), Ok(vec![97]));
        let patches = [
            json!({ "revision": "b".repeat(64) }),
            json!({ "offset": 1 }),
            json!({ "transferId": info().id }),
            json!({ "bytes": "YQ==\n" }),
            json!({ "bytes": "Yg==", "eof": true }),
            json!({ "bytes": "Zh==" }),
            json!({ "bytes": "" }),
        ];
        for patch in patches {
            let mut value = serde_json::to_value(&chunk).unwrap();
            for (key, replacement) in patch.as_object().unwrap() {
                value[key.as_str()] = replacement.clone();
            }
            let corrupt: AttachmentChunk = serde_json::from_value(value).unwrap();
            assert!(
                attachment_bytes(&corrupt, &transfer, 0, 1).is_err(),
                "{patch}"
            );
        }
    }

    #[tokio::test]
    async fn refuses_a_network_preview_url_and_closes_its_lease_and_native_handle() {
        let rpc = healthy();
        let mut native = Native::new();
        native.preview = NativePreview {
            preview_id: "preview".to_owned(),
            url: "https://example.com/file".to_owned(),
        };
        let mut retire = Vec::new();
        let result = preview_attachment(
            &rpc,
            reference(),
            &native,
            &Signal::new(),
            HOST_REPLY,
            &mut retire,
        )
        .await;
        assert!(
            matches!(&result, Err(TransferError::Failed(message)) if message.contains("invalid preview")),
            "{:?}",
            result.as_ref().err()
        );
        assert!(native.saw("close_preview", "preview"));
        assert_eq!(
            rpc.calls().last().map(|(name, _)| *name),
            Some("attachments.close")
        );
    }

    #[tokio::test]
    async fn serves_only_the_matching_native_preview_and_disposes_its_listener_once() {
        let rpc = healthy();
        let native = Native::new();
        let mut retire = Vec::new();
        let preview = preview_attachment(
            &rpc,
            reference(),
            &native,
            &Signal::new(),
            HOST_REPLY,
            &mut retire,
        )
        .await
        .unwrap();
        let request_id = "8172071e-bd79-4d10-8b8a-a9db09353cba";
        let mut ignored = |_: &TransferError| {};
        let other = NativeRead {
            preview_id: "another".to_owned(),
            request_id: request_id.to_owned(),
            offset: 0,
            length: 3,
        };
        preview.serve(&other, &mut ignored).await.unwrap();
        assert_eq!(native.count("respond"), 0);
        let mine = NativeRead {
            preview_id: native.preview.preview_id.clone(),
            request_id: request_id.to_owned(),
            offset: 0,
            length: 3,
        };
        preview.serve(&mine, &mut ignored).await.unwrap();
        assert!(native.saw(
            "respond",
            &format!("{}|{request_id}|aaa", native.preview.preview_id)
        ));
        preview.close().await.unwrap();
        preview.close().await.unwrap();
        preview.serve(&mine, &mut ignored).await.unwrap();
        assert_eq!(native.count("respond"), 1);
        assert_eq!(native.count("close_preview"), 1);
        assert_eq!(
            rpc.calls()
                .iter()
                .filter(|(name, _)| *name == "attachments.close")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn waits_for_the_first_in_flight_abort_cleanup_on_every_subsequent_preview_close() {
        let rpc = healthy();
        let native = Native::new();
        let (release, gate) = oneshot::channel();
        *native.close_gate.lock().unwrap() = Some(gate);
        let mut retire = Vec::new();
        let preview = preview_attachment(
            &rpc,
            reference(),
            &native,
            &Signal::new(),
            HOST_REPLY,
            &mut retire,
        )
        .await
        .unwrap();
        let (first, second, ()) = tokio::join!(preview.close(), preview.close(), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(
                rpc.calls()
                    .iter()
                    .all(|(name, _)| *name != "attachments.close")
            );
            release.send(()).unwrap();
        });
        assert_eq!((first, second), (Ok(()), Ok(())));
        assert_eq!(native.count("close_preview"), 1);
        assert_eq!(
            rpc.calls().last().map(|(name, _)| *name),
            Some("attachments.close")
        );
    }

    #[tokio::test]
    async fn retains_the_first_preview_cleanup_failure_for_later_close_callers_while_closing_the_host_lease()
     {
        let rpc = healthy();
        let mut native = Native::new();
        native.close_error = Some(TransferError::Failed(
            "Synthetic cleanup failure".to_owned(),
        ));
        let mut retire = Vec::new();
        let preview = preview_attachment(
            &rpc,
            reference(),
            &native,
            &Signal::new(),
            HOST_REPLY,
            &mut retire,
        )
        .await
        .unwrap();
        let first = preview.close().await;
        assert_eq!(
            first,
            Err(TransferError::Failed(
                "Synthetic cleanup failure".to_owned()
            ))
        );
        assert_eq!(preview.close().await, first);
        assert_eq!(native.count("close_preview"), 1);
        assert_eq!(
            rpc.calls().last().map(|(name, _)| *name),
            Some("attachments.close")
        );
    }

    #[tokio::test]
    async fn reports_a_host_read_failure_separately_from_an_unsupported_media_codec() {
        let rpc = healthy().on::<method::AttachmentsRead>(|_| {
            Err(RpcError::Daemon {
                code: ErrorCode::Corrupt,
                message: "Synthetic corrupt file".to_owned(),
            })
        });
        let native = Native::new();
        let mut retire = Vec::new();
        let preview = preview_attachment(
            &rpc,
            reference(),
            &native,
            &Signal::new(),
            HOST_REPLY,
            &mut retire,
        )
        .await
        .unwrap();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        let mut record = move |error: &TransferError| sink.lock().unwrap().push(error.clone());
        let request = NativeRead {
            preview_id: native.preview.preview_id.clone(),
            request_id: "request".to_owned(),
            offset: 0,
            length: 3,
        };
        preview.serve(&request, &mut record).await.unwrap();
        assert_eq!(
            *errors.lock().unwrap(),
            vec![TransferError::Rpc(RpcError::Daemon {
                code: ErrorCode::Corrupt,
                message: "Synthetic corrupt file".to_owned()
            })]
        );
        assert!(native.saw(
            "respond",
            &format!("{}|request|null", native.preview.preview_id)
        ));
        preview.close().await.unwrap();
    }

    #[tokio::test]
    async fn closes_an_open_reply_that_arrives_after_cancellation() {
        let (reply, late) = oneshot::channel();
        let rpc = LateOpen {
            reply: Mutex::new(Some(late)),
            inner: healthy(),
        };
        let signal = Signal::new();
        let mut retire = Vec::new();
        let (opening, ()) = tokio::join!(
            AttachmentTransfer::open(&rpc, reference(), &signal, HOST_REPLY, &mut retire),
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                signal.abort();
                tokio::time::sleep(Duration::from_millis(10)).await;
                reply.send(opened(info())).unwrap();
            }
        );
        assert!(matches!(opening, Err(TransferError::Cancelled)));
        for cleanup in retire {
            cleanup.await;
        }
        assert!(
            rpc.inner
                .calls()
                .iter()
                .any(|(name, params)| *name == "attachments.close"
                    && params["transferId"] == TRANSFER)
        );
    }

    #[tokio::test]
    async fn never_finalizes_a_native_save_when_cancellation_wins_just_before_its_start_reply_resumes()
     {
        let rpc = healthy();
        let signal = Signal::new();
        let mut native = Native::new();
        native.save = Some("late-save".to_owned());
        native.abort_on_start = Some(signal.clone());
        let mut retire = Vec::new();
        let result = download_attachment(
            &rpc,
            reference(),
            &native,
            &signal,
            options(),
            &mut retire,
            |_, _| {},
        )
        .await;
        assert_eq!(result, Err(TransferError::Cancelled));
        assert_eq!(native.count("finish"), 0);
        assert_eq!(native.count("append"), 0);
        assert!(native.saw("cancel", "late-save"));
    }

    #[test]
    fn says_why_an_attachment_failed_in_the_words_the_app_shows() {
        let daemon = |code| {
            TransferError::Rpc(RpcError::Daemon {
                code,
                message: String::new(),
            })
        };
        let closed = RpcError::Closed(client::CloseReason {
            code: 1006,
            reason: String::new(),
            opened: true,
        });
        assert_eq!(
            attachment_error(&TransferError::Rpc(closed)),
            "The file host is offline. Reconnect and try again."
        );
        assert_eq!(
            attachment_error(&daemon(ErrorCode::Unavailable)),
            "This shared file is unavailable; it may have been removed."
        );
        assert_eq!(
            attachment_error(&TransferError::Native {
                code: Some("unavailable".to_owned()),
                message: String::new()
            }),
            "The file or save destination is unavailable."
        );
        assert_eq!(
            attachment_error(&daemon(ErrorCode::Busy)),
            "Two file transfers are already active. Close one and try again."
        );
        assert_eq!(
            attachment_error(&TransferError::Failed("odd".to_owned())),
            "The file could not be opened or saved. Check the connection and try again."
        );
    }
}
