//! The desktop app's parts without GPUI (spec §2): the WebView2 browser engine and its tools, broker clients, the
//! login vault, speech, the updater, tray, notifications, the login item, crash handling and the daemon process.
//! Its tests build without compiling GPUI.

pub mod app_dirs;
pub mod browser;
pub mod crash;
pub mod daemon_process;
pub mod device_keys;
pub mod device_settings;
pub mod diagnostics;
pub mod hosts;
pub mod identity;
pub mod links;
pub mod log;
pub mod media;
pub mod owner_safety;
pub mod paths;
pub mod pdf;
pub mod perf;
pub mod remote_hosts;
pub mod shell;
pub mod spawn;
pub mod speech;
pub mod updater;
pub mod vault;

/// SHA-256 for the desktop (it checks a saved download against the daemon's digest), the version this crate uses.
pub use sha2;
