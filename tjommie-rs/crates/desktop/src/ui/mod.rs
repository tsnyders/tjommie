//! The UI. `tools/check-guarded.ps1` holds every handler here to the guard rule (Phase 3 plan T0.4).

pub mod agents;
pub mod bots;
pub mod browser;
pub mod chat;
pub mod fields;
pub mod files;
pub mod logins;
pub mod primitives;
pub mod projects;
pub mod rooms;
pub mod routines;
pub mod settings;
pub mod setup;
pub mod shell;
pub mod store;
pub mod style;
#[cfg(test)]
pub mod testing;
pub mod vault_entry;

/// This PC's file or folder picker. In a test-driver build, the test's answer when it gave one (`AnswerPathPrompt`):
/// keys can't drive the system's dialog (M5 review M8).
pub(crate) fn prompt_for_paths(
    options: gpui::PathPromptOptions,
    cx: &gpui::App,
) -> futures::channel::oneshot::Receiver<gpui::Result<Option<Vec<std::path::PathBuf>>>> {
    #[cfg(feature = "test-driver")]
    if let Some(path) = crate::test_driver::take_path_answer() {
        let (answer, picked) = futures::channel::oneshot::channel();
        let _ = answer.send(Ok(Some(vec![path])));
        return picked;
    }
    cx.prompt_for_paths(options)
}

/// This PC's save dialog, opening in `directory` with `name` suggested. In a test-driver build, the test's answer when it
/// gave one, as [`prompt_for_paths`] (M4 review C8: an e2e can then drive Download and Save report).
pub(crate) fn prompt_for_new_path(
    directory: &std::path::Path,
    name: Option<&str>,
    cx: &gpui::App,
) -> futures::channel::oneshot::Receiver<gpui::Result<Option<std::path::PathBuf>>> {
    #[cfg(feature = "test-driver")]
    if let Some(path) = crate::test_driver::take_path_answer() {
        let (answer, picked) = futures::channel::oneshot::channel();
        let _ = answer.send(Ok(Some(path)));
        return picked;
    }
    cx.prompt_for_new_path(directory, name)
}
