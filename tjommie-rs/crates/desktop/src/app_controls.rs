//! This device's own app controls (`DesktopApp` in the TypeScript, `desktop().loginItem` and `desktop().updates`): start at
//! Windows sign-in, and the updater. Settings, General reads them through [`AppControls`], so a view test gives it a fake;
//! `services` gives it the real ones. They belong to this PC's app, whichever PC the window is on.

use std::sync::Arc;

use desktop_host::updater::UpdateState;
use gpui::{App, Global};

/// What General asks of the desktop app.
pub trait DesktopApp: Send + Sync {
    /// Whether Windows will start the app at sign-in (`loginItem.get`).
    fn login_item(&self) -> bool;
    /// Turns starting at sign-in on or off (`loginItem.set`). `Err` says why it couldn't, in words for the person.
    fn set_login_item(&self, on: bool) -> Result<(), String>;
    /// The updater's state now (`updates.state`).
    fn update_state(&self) -> UpdateState;
    /// Checks for an update now (`updates.check`). The state says how it goes; this returns at once.
    fn check_for_updates(&self);
    /// Restarts to install the ready update (`updates.restart`).
    fn restart_to_update(&self);
}

/// The app's controls, once `services` has started them. Without them (a view test that gives none) General says they
/// aren't available.
#[derive(Clone)]
pub struct AppControls(pub Arc<dyn DesktopApp>);

impl Global for AppControls {}

/// The controls the app has, if any.
pub fn controls(cx: &App) -> Option<Arc<dyn DesktopApp>> {
    cx.try_global::<AppControls>()
        .map(|controls| Arc::clone(&controls.0))
}
