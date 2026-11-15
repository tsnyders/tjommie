//! What the update machinery reports (`updater.ts:6-11`). The tray, the settings screen and the quit path all read it.

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    /// Checked, and this is the newest version.
    None,
    Downloading {
        percent: f64,
    },
    Ready {
        version: String,
    },
    Error {
        message: String,
    },
    /// An unpackaged, dev or disabled build: there is nothing to check.
    Unsupported,
}

/// Shown when the daemon wouldn't stop and the update was therefore not installed (`updater.ts:4`).
pub const SHUTDOWN_FAILURE: &str =
    "Couldn't stop the background service. The update was not installed.";
