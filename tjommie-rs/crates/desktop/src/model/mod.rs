//! The view models (Phase 3 plan T1.4): the logic of `packages/ui/lib/*` as plain Rust, with no GPUI and no entities.
//! One module per TypeScript file, each with that file's tests ported one for one. What isn't ported, and why, is in
//! `desktop-test-map.md`.

pub mod accounts;
pub mod activity;
pub mod approval;
pub mod attachment_transfer;
pub mod attachments;
pub mod bot_form;
pub mod browser_geometry;
pub mod browser_preview;
pub mod browser_session;
pub mod connection;
pub mod dictation;
pub mod document_ranges;
pub mod exchange;
pub mod file_thumbnails;
pub mod format;
pub mod google_connection;
pub mod labels;
pub mod logins;
pub mod markdown_document;
pub mod mentions;
pub mod native_browser_session;
pub mod overview;
pub mod panel_layout;
pub mod projects;
pub mod routine_form;
pub mod routines;
pub mod setup;
pub mod state;
pub mod store;
pub mod suggestions;
pub mod timeline;
pub mod usage;

#[cfg(test)]
pub mod testing;
