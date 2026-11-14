//! The desktop app's parts without GPUI (spec §2): the WebView2 browser engine and its tools, broker clients, the
//! login vault, speech, the updater, tray, notifications, the login item, crash handling and the daemon process.
//! Its tests build without compiling GPUI.

pub mod crash;
pub mod log;
pub mod spawn;
