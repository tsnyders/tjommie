//! The 26 browser tools a bot drives its page with, over the DevTools protocol (Phase 3 plan T3.3; the TypeScript
//! original is `native-browser-automation.ts`). `args` decodes a call, `automation` runs it, and the other modules are
//! what the tools are made of.

mod actions;
pub mod args;
mod automation;
mod files;
mod observe;
mod page;
pub mod protocol;
mod reply;
mod run;
mod session;
mod snapshot;

pub use automation::Automation;
pub use reply::{ImageMime, ToolContent, ToolError, ToolReply};
