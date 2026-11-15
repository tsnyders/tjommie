//! Port of `packages/ui/lib/document-ranges.ts` (T1.4): the byte ranges an attachment's preview URL serves, checked against
//! what was asked, and the PDF reader's queue of those ranges. `fetch` is [`RangeFetch`]; the TypeScript `AbortSignal` is
//! [`Signal`]; a `RangeError` and an `Error` are both [`DocumentError`].

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use protocol::attachments::{ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES};
use tokio::sync::Notify;

/// Why a document read failed. The messages are the TypeScript's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentError {
    InvalidRange,
    Unavailable,
    Incomplete,
    /// The request itself failed; the text is why.
    Transport(String),
    /// The signal was aborted (the TypeScript's `AbortError`).
    Cancelled,
    InvalidPdfRequest,
    InvalidSection,
    InvalidSearch,
    /// `TextDecoder` with `fatal` refused the bytes.
    NotUtf8,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidRange => "Invalid document byte range.",
            Self::Unavailable => "The shared document is no longer available.",
            Self::Incomplete => "The document content is incomplete.",
            Self::Transport(why) => why.as_str(),
            Self::Cancelled => "Cancelled",
            Self::InvalidPdfRequest => {
                "The local PDF reader requested an invalid or overlapping range."
            }
            Self::InvalidSection => "Invalid Markdown section.",
            Self::InvalidSearch => "Invalid Markdown search.",
            Self::NotUtf8 => "The encoded data was not valid for encoding utf-8",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for DocumentError {}

/// The TypeScript `AbortSignal`: a cancel flag that wakes whoever waits on it. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct Signal(Arc<SignalState>);

#[derive(Debug, Default)]
struct SignalState {
    aborted: AtomicBool,
    woken: Notify,
}

impl Signal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.0.aborted.store(true, Ordering::SeqCst);
        self.0.woken.notify_waiters();
    }

    pub fn aborted(&self) -> bool {
        self.0.aborted.load(Ordering::SeqCst)
    }

    /// `throwIfAborted()`.
    pub fn throw_if_aborted(&self) -> Result<(), DocumentError> {
        if self.aborted() {
            Err(DocumentError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Resolves once the signal is aborted (at once, if it already is).
    pub async fn cancelled(&self) {
        loop {
            // Created before the flag is read, so an abort between the two can't be missed.
            let woken = self.0.woken.notified();
            if self.aborted() {
                return;
            }
            woken.await;
        }
    }
}

/// Races `future` against the signal: an aborted signal wins, and the future is dropped.
pub async fn until_aborted<T>(
    signal: &Signal,
    future: impl Future<Output = T>,
) -> Result<T, DocumentError> {
    signal.throw_if_aborted()?;
    tokio::select! {
        biased;
        () = signal.cancelled() => Err(DocumentError::Cancelled),
        value = future => Ok(value),
    }
}

/// One range reply: its status, the two headers the checks read (`None` when absent), and the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeReply {
    pub status: u16,
    pub content_range: Option<String>,
    pub content_length: Option<String>,
    pub body: Vec<u8>,
}

/// The preview URL's `Range` request, for the bytes `begin..=last`. The TypeScript `fetch`, with the transport left out.
pub trait RangeFetch: Sync {
    fn fetch_range(
        &self,
        url: &str,
        begin: u64,
        last: u64,
    ) -> impl Future<Output = Result<RangeReply, String>> + Send;
}

/// `documentRange`: the bytes `begin..end` of a `size`-byte document, as one exact range request. A reply that doesn't say
/// exactly that range (status, Content-Range, Content-Length) or doesn't carry exactly its bytes is refused.
pub async fn document_range<F: RangeFetch>(
    fetch: &F,
    url: &str,
    size: u64,
    begin: u64,
    end: u64,
    signal: &Signal,
) -> Result<Vec<u8>, DocumentError> {
    signal.throw_if_aborted()?;
    // The TypeScript checks Number.isSafeInteger first; u64 inputs make that unnecessary apart from the size bound.
    if size > MAX_ATTACHMENT_BYTES
        || end <= begin
        || end > size
        || end - begin > u64::from(ATTACHMENT_CHUNK_BYTES)
    {
        return Err(DocumentError::InvalidRange);
    }
    let last = end - 1;
    let reply = until_aborted(signal, fetch.fetch_range(url, begin, last))
        .await?
        .map_err(DocumentError::Transport)?;
    let length = end - begin;
    let expected_range = format!("bytes {begin}-{last}/{size}");
    let said_length = reply
        .content_length
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok());
    if reply.status != 206
        || reply.content_range.as_deref() != Some(expected_range.as_str())
        || said_length != Some(length)
    {
        return Err(DocumentError::Unavailable);
    }
    signal.throw_if_aborted()?;
    if u64::try_from(reply.body.len()) != Ok(length) {
        return Err(DocumentError::Incomplete);
    }
    Ok(reply.body)
}

/// `PdfRangeQueue`: the PDF reader's requests for whole, aligned chunks, fetched one at a time in request order. A bad or
/// overlapping request fails the queue, and nothing after it is fetched. Requests queued before an abort are dropped.
pub struct PdfRangeQueue<'a, F> {
    fetch: &'a F,
    url: &'a str,
    size: u64,
    signal: Signal,
    /// Queued and in-flight requests, in order.
    pending: Vec<(u64, u64)>,
    failed: bool,
}

impl<'a, F: RangeFetch> PdfRangeQueue<'a, F> {
    pub fn new(fetch: &'a F, url: &'a str, size: u64, signal: Signal) -> Self {
        Self {
            fetch,
            url,
            size,
            signal,
            pending: Vec::new(),
            failed: false,
        }
    }

    /// `request`: queues `begin..end`. The error is what fails the queue (the TypeScript's `fail` callback), returned once.
    pub fn request(&mut self, begin: u64, end: u64) -> Result<(), DocumentError> {
        if self.signal.aborted() || self.failed {
            return Ok(());
        }
        let chunk = u64::from(ATTACHMENT_CHUNK_BYTES);
        let overlaps = self
            .pending
            .iter()
            .any(|&(first, last)| begin < last && end > first);
        let slots = self.size.div_ceil(chunk);
        let queued = u64::try_from(self.pending.len()).unwrap_or(u64::MAX);
        if !begin.is_multiple_of(chunk)
            || end != self.size.min(begin.saturating_add(chunk))
            || end <= begin
            || overlaps
            || queued >= slots
        {
            self.failed = true;
            return Err(DocumentError::InvalidPdfRequest);
        }
        self.pending.push((begin, end));
        Ok(())
    }

    /// `settled`: fetches the queued ranges in order and hands each to `deliver`. A failed fetch fails the queue and is
    /// returned; after an abort or a failure, what is queued is dropped without a fetch.
    pub async fn settle(
        &mut self,
        mut deliver: impl FnMut(u64, Vec<u8>),
    ) -> Result<(), DocumentError> {
        while let Some(&(begin, end)) = self.pending.first() {
            if self.signal.aborted() || self.failed {
                self.pending.clear();
                return Ok(());
            }
            let result =
                document_range(self.fetch, self.url, self.size, begin, end, &self.signal).await;
            self.pending.remove(0);
            match result {
                Ok(bytes) => {
                    if !self.signal.aborted() && !self.failed {
                        deliver(begin, bytes);
                    }
                }
                Err(error) => {
                    if !self.signal.aborted() && !self.failed {
                        self.failed = true;
                        return Err(error);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use protocol::attachments::{ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES};

    use super::*;

    /// The server's honest answer for `begin..=last` of a `size`-byte document.
    fn honest(size: u64, begin: u64, last: u64, body: Vec<u8>) -> RangeReply {
        RangeReply {
            status: 206,
            content_range: Some(format!("bytes {begin}-{last}/{size}")),
            content_length: Some((last - begin + 1).to_string()),
            body,
        }
    }

    /// Answers every range with `reply(begin, last)`, and remembers the ranges asked for.
    struct Server<F> {
        requests: Mutex<Vec<(u64, u64)>>,
        reply: F,
    }

    impl<F: Fn(u64, u64) -> RangeReply + Sync> Server<F> {
        fn new(reply: F) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                reply,
            }
        }

        fn asked(&self) -> Vec<(u64, u64)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl<F: Fn(u64, u64) -> RangeReply + Sync> RangeFetch for Server<F> {
        fn fetch_range(
            &self,
            _url: &str,
            begin: u64,
            last: u64,
        ) -> impl Future<Output = Result<RangeReply, String>> + Send {
            self.requests.lock().unwrap().push((begin, last));
            let reply = (self.reply)(begin, last);
            std::future::ready(Ok(reply))
        }
    }

    fn chunk() -> u64 {
        u64::from(ATTACHMENT_CHUNK_BYTES)
    }

    #[tokio::test]
    async fn delivers_all_800_full_size_pdf_callbacks_serially_one_exact_chunk_each_without_the_provisional_queue_range_cap()
     {
        let size = MAX_ATTACHMENT_BYTES;
        let server = Server::new(move |begin, last| {
            let fill = (begin / chunk() % 255) as u8;
            honest(size, begin, last, vec![fill; (last - begin + 1) as usize])
        });
        let mut queue = PdfRangeQueue::new(&server, "local", size, Signal::new());
        for begin in (0..size).step_by(chunk() as usize) {
            queue.request(begin, begin + chunk()).unwrap();
        }
        let mut delivered = 0u64;
        queue
            .settle(|begin, bytes| {
                assert_eq!(begin, delivered * chunk());
                assert_eq!(bytes.len(), chunk() as usize);
                delivered += 1;
            })
            .await
            .unwrap();
        assert_eq!(delivered, 800);
        let asked = server.asked();
        assert_eq!(asked.len(), 800);
        assert!(
            asked
                .iter()
                .all(|(begin, last)| last - begin + 1 == chunk())
        );
    }

    #[tokio::test]
    async fn fails_duplicate_overlapping_pdf_requests_and_drops_all_queued_callbacks_after_abort() {
        let size = chunk() * 2;
        let server = Server::new(move |begin, last| {
            honest(size, begin, last, vec![0; (last - begin + 1) as usize])
        });
        let abort = Signal::new();
        let mut queue = PdfRangeQueue::new(&server, "local", size, abort.clone());
        queue.request(0, chunk()).unwrap();
        assert_eq!(
            queue.request(0, chunk()),
            Err(DocumentError::InvalidPdfRequest)
        );
        let mut delivered = 0;
        queue.settle(|_, _| delivered += 1).await.unwrap();
        assert!(server.asked().is_empty());
        assert_eq!(delivered, 0);

        let mut other = PdfRangeQueue::new(&server, "local", size, abort.clone());
        other.request(0, chunk()).unwrap();
        other.request(chunk(), chunk() * 2).unwrap();
        abort.abort();
        other.settle(|_, _| delivered += 1).await.unwrap();
        assert_eq!(delivered, 0);
        assert!(server.asked().is_empty());
    }

    #[tokio::test]
    async fn refuses_a_reply_that_does_not_say_exactly_the_range_asked_for() {
        let signal = Signal::new();
        let replies: [fn(u64, u64) -> RangeReply; 3] = [
            |begin, last| RangeReply {
                status: 200,
                ..honest(4, begin, last, vec![0; 2])
            },
            |begin, last| RangeReply {
                content_length: Some("2,2".into()),
                ..honest(4, begin, last, vec![0; 2])
            },
            |begin, last| RangeReply {
                content_range: Some("bytes 0-1/5".into()),
                ..honest(4, begin, last, vec![0; 2])
            },
        ];
        for reply in replies {
            let server = Server::new(reply);
            assert_eq!(
                document_range(&server, "local", 4, 0, 2, &signal).await,
                Err(DocumentError::Unavailable)
            );
        }
        let short = Server::new(|begin, last| honest(4, begin, last, vec![0; 1]));
        assert_eq!(
            document_range(&short, "local", 4, 0, 2, &signal).await,
            Err(DocumentError::Incomplete)
        );
    }

    #[tokio::test]
    async fn refuses_ranges_outside_the_document_or_over_a_chunk_before_fetching() {
        let server = Server::new(|begin, last| {
            honest(
                MAX_ATTACHMENT_BYTES,
                begin,
                last,
                vec![0; (last - begin + 1) as usize],
            )
        });
        let signal = Signal::new();
        let bad = [
            (MAX_ATTACHMENT_BYTES + 1, 0, 1),
            (4, 2, 2),
            (4, 0, 5),
            (MAX_ATTACHMENT_BYTES, 0, chunk() + 1),
        ];
        for (size, begin, end) in bad {
            assert_eq!(
                document_range(&server, "local", size, begin, end, &signal).await,
                Err(DocumentError::InvalidRange)
            );
        }
        assert!(server.asked().is_empty());
    }

    #[test]
    fn an_aborted_signal_is_a_cancellation_not_a_failure() {
        let signal = Signal::new();
        assert!(signal.throw_if_aborted().is_ok());
        signal.abort();
        assert_eq!(signal.throw_if_aborted(), Err(DocumentError::Cancelled));
    }
}
