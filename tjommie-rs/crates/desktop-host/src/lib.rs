//! The desktop app's parts without GPUI (spec §2): the WebView2 browser engine and its tools, broker clients, the
//! login vault, speech, the updater, tray, notifications, the login item, crash handling and the daemon process.
//! Its tests build without compiling GPUI.

pub mod app_dirs;
pub mod crash;
pub mod diagnostics;
pub mod identity;
pub mod links;
pub mod log;
pub mod owner_safety;
pub mod paths;
pub mod shell;
pub mod spawn;
pub mod speech;
pub mod updater;
