//! The desktop host's long poll, shared by `/browser-host` and `/vault-host`. The desktop main process keeps one GET
//! waiting per lane (normal, and `?control=1` for urgent work) with its private host token; the daemon answers a GET
//! with the next job, or 204 after 15 s, and the desktop POSTs each result back.

use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
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
