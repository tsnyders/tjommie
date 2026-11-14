//! The desktop updater (spec §3.10; port of `updater.ts`). The tray and the quit path read its state.

mod state;

pub use state::{SHUTDOWN_FAILURE, UpdateState};
