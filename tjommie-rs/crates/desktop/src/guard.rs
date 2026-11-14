//! The UI boundary rule (gate condition 4): a panic in an event handler, action, spawned task body or timer callback
//! never takes the app down. Each runs inside `guarded`, which catches the panic (the crash log keeps its
//! backtrace), logs it, raises "Something went wrong" and returns `None`. `tools/check-guarded.ps1` enforces it in
//! `src/ui`.

use crate::notices::{self, Notice};

pub fn guarded<R>(what: &'static str, f: impl FnOnce() -> R) -> Option<R> {
    match desktop_host::crash::catching(f) {
        Ok(value) => Some(value),
        Err(_) => {
            log::error!("{what} panicked; its backtrace is in desktop-crash.log");
            notices::raise(Notice::SomethingWentWrong);
            None
        }
    }
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
}
