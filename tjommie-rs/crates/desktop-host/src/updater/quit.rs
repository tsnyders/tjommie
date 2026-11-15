//! Quitting, and the update that may install as the app goes (port of `main.ts:119-189`, the `will-quit` handler).
//!
//! The order matters: the logins vault locks and the browser closes, then the daemon stops, and only then does an
//! installer start, because the installer replaces the daemon's exe. If the daemon would not stop on its own and had to
//! be forced, or its stop could not be confirmed, the update is **not** installed: the window comes back, saying
//! "Update not installed", and the daemon is restarted or reconnected. A plain quit with no update pending goes ahead
//! whatever the daemon did.

use super::Updater;

/// How the daemon stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStop {
    /// It exited when asked.
    Graceful,
    /// It didn't within the grace period (20 s) and was killed.
    Forced,
}

/// The app's side of quitting, as the app implements it.
#[allow(async_fn_in_trait)] // used with a concrete type, on the UI thread
pub trait QuitHost {
    /// Lock the vault, close the vault host and the browser pages. Best effort: it must not fail the quit.
    async fn close_native(&self);
    /// Stops the daemon (a stop request, then a forced stop after 20 s). `Err` when its stop couldn't be confirmed.
    async fn stop_daemon(&self) -> Result<DaemonStop, String>;
    /// Closes the browser host's own connections. Best effort.
    async fn close_native_host(&self);
    /// The update will not install: show the window again, say "Update not installed", and restart or reconnect the
    /// daemon. `daemon_stopped` says which of the two messages applies (the service did not stop in time and is being
    /// restarted, or its stop could not be confirmed and Tjommie must be restarted).
    fn update_not_installed(&self, daemon_stopped: bool);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// Go ahead and quit. `installer_started` says whether an update installer was started.
    Quit { installer_started: bool },
    /// Don't quit: an update was pending and the daemon didn't stop cleanly.
    Aborted,
}

pub async fn run_quit<H: QuitHost>(host: &H, updates: &Updater) -> QuitOutcome {
    host.close_native().await;
    let stop = host.stop_daemon().await;
    // Whether the daemon is known to be gone, and whether it took a forced stop to get it there.
    let (daemon_stopped, forced) = match stop {
        Ok(stop) => (true, stop == DaemonStop::Forced),
        Err(error) => {
            log::warn!("The background service's stop could not be confirmed: {error}");
            (false, false)
        }
    };
    host.close_native_host().await;
    if forced {
        log::warn!("Shutdown failed: The background service required a forced stop.");
    }
    let clean = daemon_stopped && !forced;
    if !clean && updates.installation_pending() {
        updates.shutdown_failed();
        host.update_not_installed(daemon_stopped);
        return QuitOutcome::Aborted;
    }
    match updates.install_on_quit() {
        Ok(installer_started) => QuitOutcome::Quit { installer_started },
        Err(error) => {
            // The downloaded file no longer matched its checksum: quit without installing it.
            log::warn!("The update was not installed: {error}");
            QuitOutcome::Quit {
                installer_started: false,
            }
        }
    }
}
