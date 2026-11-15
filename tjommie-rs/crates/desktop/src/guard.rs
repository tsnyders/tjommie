//! The UI boundary rule (gate condition 4): a panic in an event handler, action, spawned task body or timer callback
//! never takes the app down. Each runs inside `guarded` (a spawned task's body inside `guarded_async`), which catches
//! the panic (the crash log keeps its backtrace), logs it, raises "Something went wrong" and returns `None`.
//! `tools/check-guarded.ps1` enforces it across this crate.

use std::task::Poll;

use crate::notices::{self, Notice};

pub fn guarded<R>(what: &'static str, f: impl FnOnce() -> R) -> Option<R> {
    desktop_host::crash::catching(f)
        .inspect_err(|_| caught(what))
        .ok()
}

/// `guarded` for a spawned task's body: every poll of `task` runs inside `catching`, so a panic ends the task, not
/// the app.
pub async fn guarded_async<R>(what: &'static str, task: impl Future<Output = R>) -> Option<R> {
    let mut task = std::pin::pin!(task);
    std::future::poll_fn(
        |cx| match desktop_host::crash::catching(|| task.as_mut().poll(cx)) {
            Ok(poll) => poll.map(Some),
            Err(_) => {
                caught(what);
                Poll::Ready(None)
            }
        },
    )
    .await
}

fn caught(what: &str) {
    log::error!("{what} panicked; its backtrace is in desktop-crash.log");
    notices::raise(Notice::SomethingWentWrong);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panicking_handler_returns_none_and_notices() {
        let mut inbox = notices::listen();
        assert_eq!(guarded("a calm handler", || 7), Some(7));
        assert!(inbox.try_recv().is_err(), "a calm handler raised a notice");
        assert_eq!(
            guarded("a planted panic", || -> u8 { panic!("planted") }),
            None
        );
        assert_eq!(inbox.try_recv().ok(), Some(Notice::SomethingWentWrong));
    }

    /// Yields once and wakes itself, so what follows runs in a later poll.
    async fn yield_once() {
        let mut yielded = false;
        std::future::poll_fn(|cx| {
            if std::mem::replace(&mut yielded, true) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }

    async fn planted() -> u8 {
        yield_once().await;
        panic!("planted")
    }

    #[test]
    fn panicking_task_returns_none_and_notices() {
        let mut inbox = notices::listen();
        let calm = guarded_async("a calm task", async {
            yield_once().await;
            7
        });
        assert_eq!(futures::executor::block_on(calm), Some(7));
        assert!(inbox.try_recv().is_err(), "a calm task raised a notice");
        // The panic comes in the second poll: every poll is guarded, not only the first.
        let planted = guarded_async("a planted panic", planted());
        assert_eq!(futures::executor::block_on(planted), None);
        assert_eq!(inbox.try_recv().ok(), Some(Notice::SomethingWentWrong));
    }
}
