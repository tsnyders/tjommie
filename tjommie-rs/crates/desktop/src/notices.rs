//! Notices the shell shows as toasts (T2.2 draws them). Code with no `App` at hand, such as `guard`, raises them on
//! the UI thread through a channel; `init` moves them into the `Notices` global and redraws.

use std::cell::RefCell;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{App, Global};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// A UI handler panicked and `guard` caught it.
    SomethingWentWrong,
    /// Windows restarted the app after a crash (`--after-crash`). Its Save report action comes with T6.6.
    ClosedUnexpectedly,
}

impl Notice {
    pub fn text(self) -> &'static str {
        match self {
            Notice::SomethingWentWrong => "Something went wrong.",
            Notice::ClosedUnexpectedly => {
                "Tjommie closed unexpectedly and restarted. Your agents were restarted too."
            }
        }
    }
}

/// The notices waiting to be shown, oldest first.
#[derive(Default)]
pub struct Notices(pub Vec<Notice>);

impl Global for Notices {}

impl Notices {
    /// Adds `notice` unless the same one is already waiting, so a panic storm shows one toast (review F7).
    fn add(&mut self, notice: Notice) -> bool {
        let new = !self.0.contains(&notice);
        if new {
            self.0.push(notice);
        }
        new
    }
}

thread_local! {
    static OUTBOX: RefCell<Option<UnboundedSender<Notice>>> = const { RefCell::new(None) };
}

/// Raises a notice. Before anything listens on this thread, it is only logged.
pub fn raise(notice: Notice) {
    let sent = OUTBOX.with(|outbox| {
        outbox
            .borrow()
            .as_ref()
            .is_some_and(|sender| sender.unbounded_send(notice).is_ok())
    });
    if !sent {
        log::warn!("A notice nothing shows: {}", notice.text());
    }
}

/// Routes the notices raised on this thread to the returned receiver.
pub fn listen() -> UnboundedReceiver<Notice> {
    let (sender, receiver) = unbounded();
    OUTBOX.with(|outbox| *outbox.borrow_mut() = Some(sender));
    receiver
}

/// Sets up the `Notices` global and moves raised notices into it. Call on the UI thread.
pub fn init(cx: &mut App) {
    cx.set_global(Notices::default());
    let mut inbox = listen();
    // A panic while showing a notice is only logged: guarded would raise it as another notice into this same loop,
    // which would show it and panic again (review F8).
    // unguarded: the notice sink
    cx.spawn(async move |cx| {
        while let Some(notice) = inbox.next().await {
            cx.update(|cx| {
                let shown = desktop_host::crash::catching(|| {
                    if cx.global_mut::<Notices>().add(notice) {
                        log::info!("Notice: {}", notice.text());
                        cx.refresh_windows();
                    }
                });
                if shown.is_err() {
                    log::error!("Showing a notice panicked; its backtrace is in desktop-crash.log");
                }
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_notice_is_not_added_twice() {
        let mut notices = Notices::default();
        assert!(notices.add(Notice::SomethingWentWrong));
        assert!(!notices.add(Notice::SomethingWentWrong));
        assert!(notices.add(Notice::ClosedUnexpectedly));
        assert_eq!(
            notices.0,
            [Notice::SomethingWentWrong, Notice::ClosedUnexpectedly]
        );
    }
}
