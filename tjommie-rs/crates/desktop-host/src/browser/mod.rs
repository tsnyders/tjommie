//! The native browser (spec §3.9; Phase 3 plan M3): one WebView2 environment, a profile per bot, and the bridge
//! the daemon's `/browser-host` jobs arrive through. Ports `packages/desktop/src/native-browser*.ts`.
//!
//! Threading: the engine and everything that touches WebView2 lives on the UI thread, the one that owns the parent
//! window and pumps its messages. Nothing here is `Send`.

use serde::Deserialize;

pub mod com;
pub mod engine;
pub mod flags;
pub mod lifecycle;
pub mod policy;
pub mod profiles;

/// The bot a browser belongs to, as the daemon names it in a `/browser-host` job (`native-browser-host.ts:6`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotIdentity {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub home_folder: String,
    #[serde(default)]
    pub project_folder: Option<String>,
}
