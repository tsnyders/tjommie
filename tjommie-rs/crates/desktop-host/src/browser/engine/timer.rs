//! The engine's default way to wait (`EngineConfig::sleep`): every wait shares one timer thread, started by the first.
//! A thread per wait would be hundreds of threads in a busy settle loop of the tools.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use futures::future::LocalBoxFuture;

/// A wait can be no longer than this (a year: no overflow when it is added to the clock, and nothing waits longer).
const LONGEST: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// A wait, due at `at`.
struct Due {
    at: Instant,
    done: oneshot::Sender<()>,
}

// The earliest deadline comes first out of the heap (a max-heap, so the order is reversed).
impl Ord for Due {
    fn cmp(&self, other: &Self) -> Ordering {
        other.at.cmp(&self.at)
    }
}

impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at
    }
}

impl Eq for Due {}

type Queue = Arc<(Mutex<BinaryHeap<Due>>, Condvar)>;

/// Waits `duration`, on the timer thread all waits share.
pub(super) fn shared_sleep(duration: Duration) -> LocalBoxFuture<'static, ()> {
    // ponytail: one timer thread for the life of the process; a wait is a heap entry, not a thread.
    static TIMER: OnceLock<Option<Queue>> = OnceLock::new();
    let (done, wait) = oneshot::channel();
    match TIMER.get_or_init(start) {
        Some(queue) => {
            let at = Instant::now() + duration.min(LONGEST);
            let (heap, wake) = &**queue;
            heap.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Due { at, done });
            // The new wait may be due before the one the timer sleeps until.
            wake.notify_one();
        }
        // No timer thread could start: the wait ends at once rather than never.
        None => {
            let _ = done.send(());
        }
    }
    Box::pin(async move {
        let _ = wait.await;
    })
}

fn start() -> Option<Queue> {
    let queue: Queue = Arc::new((Mutex::new(BinaryHeap::new()), Condvar::new()));
    let worker = queue.clone();
    match std::thread::Builder::new()
        .name("engine-timer".into())
        .spawn(move || run(&worker))
    {
        Ok(_) => Some(queue),
        Err(error) => {
            log::error!("the engine's timer thread could not start: {error}");
            None
        }
    }
}

fn run(queue: &Queue) {
    let (heap, wake) = &**queue;
    let mut waiting = heap.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let now = Instant::now();
        while waiting.peek().is_some_and(|next| next.at <= now) {
            if let Some(due) = waiting.pop() {
                let _ = due.done.send(());
            }
        }
        waiting = match waiting.peek().map(|next| next.at) {
            Some(at) => {
                wake.wait_timeout(waiting, at.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => wake.wait(waiting).unwrap_or_else(PoisonError::into_inner),
        };
    }
}
