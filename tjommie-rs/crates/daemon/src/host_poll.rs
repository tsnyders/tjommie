//! The desktop host's long poll, shared by `/browser-host` and `/vault-host`. The desktop main process keeps one GET
//! waiting per lane (normal, and `?control=1` for urgent work) with its private host token; the daemon answers a GET
//! with the next job, or 204 after 15 s, and the desktop POSTs each result back.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use http_body::{Frame, SizeHint};
use serde::Serialize;
use tokio::sync::oneshot;

const POLL_WAIT: Duration = Duration::from_secs(15);

/// A waiting GET: send it a response to answer it.
pub type Waiter = oneshot::Sender<Response>;

/// One waiting GET per lane. A newer GET replaces an older one, which gets 204.
#[derive(Default)]
pub struct Polls {
    normal: Option<Waiter>,
    control: Option<Waiter>,
}

impl Polls {
    fn slot(&mut self, control: bool) -> &mut Option<Waiter> {
        if control {
            &mut self.control
        } else {
            &mut self.normal
        }
    }

    pub fn wait(&mut self, control: bool) -> oneshot::Receiver<Response> {
        let (waiter, answer) = oneshot::channel();
        if let Some(older) = self.slot(control).replace(waiter) {
            let _ = older.send(StatusCode::NO_CONTENT.into_response());
        }
        answer
    }

    /// The lane's waiting GET, if its caller is still there.
    pub fn take(&mut self, control: bool) -> Option<Waiter> {
        self.slot(control)
            .take()
            .filter(|waiter| !waiter.is_closed())
    }

    pub fn waiting(&self, control: bool) -> bool {
        let slot = if control { &self.control } else { &self.normal };
        slot.as_ref().is_some_and(|waiter| !waiter.is_closed())
    }

    /// Answers both lanes with 503.
    pub fn close(&mut self) {
        for waiter in [self.normal.take(), self.control.take()]
            .into_iter()
            .flatten()
        {
            let _ = waiter.send(StatusCode::SERVICE_UNAVAILABLE.into_response());
        }
    }
}

/// The waiting GET's answer, or 204 once it has waited 15 s.
pub async fn answer(waiting: oneshot::Receiver<Response>) -> Response {
    match tokio::time::timeout(POLL_WAIT, waiting).await {
        Ok(Ok(response)) => response,
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

/// A job's response body. It carries a callback for the one way a job can be lost between the broker and the desktop: the
/// connection dies and the body is dropped before the server ever took the job to write it (`response.close` with
/// `!writableFinished` in `native-browser-broker.ts`). Then the broker still owns the job and can hand it to the next poll.
/// A body the server has begun writing is the desktop's: a write that fails after that cannot be seen from here, and its
/// job ends with the broker's usual timeout, "its outcome is unknown".
struct Handoff {
    bytes: Option<Bytes>,
    size: u64,
    lost: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl http_body::Body for Handoff {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        Poll::Ready(self.bytes.take().map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.bytes.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.size)
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        if self.bytes.is_some()
            && let Some(lost) = self.lost.take()
        {
            lost();
        }
    }
}

/// A job for the desktop, like [`job`], that calls `lost` if the connection drops it before it was written. `lost` runs
/// wherever the response is dropped, possibly under the caller's lock, so it must not wait for anything.
pub fn job_or(value: &impl Serialize, lost: Box<dyn FnOnce() + Send + Sync>) -> Response {
    let bytes = Bytes::from(serde_json::to_vec(value).expect("jobs serialize"));
    let size = bytes.len() as u64;
    let mut response = Response::new(Body::new(Handoff {
        bytes: Some(bytes),
        size,
        lost: Some(lost),
    }));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// A job for the desktop: JSON, never cached.
pub fn job(value: &impl Serialize) -> Response {
    let body = serde_json::to_string(value).expect("jobs serialize");
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// `Authorization: Bearer <secret>`, compared in constant time. An empty secret never matches.
pub fn bearer(headers: &HeaderMap, secret: &str) -> bool {
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    !secret.is_empty()
        && given.len() == secret.len()
        && given
            .bytes()
            .zip(secret.bytes())
            .fold(0, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// A GET on the urgent lane (`?control=1`).
pub fn is_control(uri: &Uri) -> bool {
    uri.query() == Some("control=1")
}

/// The request body, or 413 past `limit` bytes and 400 if it can't be read.
pub async fn body(body: Body, limit: usize) -> Result<Vec<u8>, Response> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
        if bytes.len() + chunk.len() > limit {
            return Err(StatusCode::PAYLOAD_TOO_LARGE.into_response());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
