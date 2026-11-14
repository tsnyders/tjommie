//! What the shell shows that the daemon decides (Phase 3 plan T2.1): the connection, the PC this window is on, the
//! paired PCs and the Recents. T1.4's store fills the `ShellData` global once it is wired, through an adapter from its
//! `AppState`; until then the app shows the defaults: connecting, on This PC, no Recents. The types mirror T1.4's
//! (`model::state::Connection`, `AppState.host`, `remoteHosts()`), so the adapter copies field for field.

use std::rc::Rc;

use gpui::{App, Global, SharedString, Window};
use protocol::model::{BotColor, BotShape, BotState};

/// How the store takes the shell's requests.
pub type OnCommand = Rc<dyn Fn(Command, &mut Window, &mut App)>;

#[derive(Clone)]
pub struct ShellData {
    pub connection: Connection,
    /// The saved PC this window is on, by id; `None` for This PC (`AppState.host`).
    pub host: Option<SharedString>,
    /// How this window reaches its PC ("LAN", "relay"), for the host button, when known.
    pub via: Option<SharedString>,
    /// The paired PCs (`remoteHosts()`), in their saved order.
    pub saved: Vec<SavedHost>,
    /// The first letter of the Windows account's name: the host button's face on This PC.
    pub initial: SharedString,
    pub recents: Vec<Recent>,
    /// Why forgetting the PC that removed this device failed, shown on that screen.
    pub problem: Option<SharedString>,
    /// Takes the shell's requests to the store (T1.2, T1.3); `None` until it is wired.
    pub on_command: Option<OnCommand>,
}

impl Global for ShellData {}

impl Default for ShellData {
    fn default() -> Self {
        ShellData {
            connection: Connection::Connecting,
            host: None,
            via: None,
            saved: Vec::new(),
            initial: initial(std::env::var("USERNAME").ok().as_deref()),
            recents: Vec::new(),
            problem: None,
            on_command: None,
        }
    }
}

impl ShellData {
    /// Sends `command` to the store; before the store is wired, it is only logged.
    pub fn command(command: Command, window: &mut Window, cx: &mut App) {
        match cx
            .try_global::<ShellData>()
            .and_then(|data| data.on_command.clone())
        {
            Some(on_command) => on_command(command, window, cx),
            None => log::info!("No store yet for {command:?}"),
        }
    }

    /// A bot's or a room's name from the Recents, or its id.
    pub fn name_of(&self, id: &str) -> SharedString {
        self.recents
            .iter()
            .find(|recent| recent.id == id)
            .map_or_else(
                || SharedString::from(id.to_owned()),
                |recent| recent.name.clone(),
            )
    }
}

/// The first letter of `name`, upper-cased (`rail.tsx`'s initial); empty without a name.
fn initial(name: Option<&str>) -> SharedString {
    name.and_then(|name| name.chars().next())
        .map(|first| first.to_uppercase().collect::<String>().into())
        .unwrap_or_default()
}

/// Where the connection stands: `model::state::Connection` (T1.4), variant for variant.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    Connecting,
    Ready,
    Reconnecting,
    Offline {
        reason: OfflineReason,
    },
    Removed,
    Update,
    Failed {
        reason: SharedString,
    },
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineReason {
    HostOffline,
    Unreachable,
}

impl Connection {
    /// Whether the connection screen takes the main column's place (`app-shell.tsx`). Reconnecting or offline, the
    /// cached screens stay on show under the status pill.
    pub fn blocks(&self) -> bool {
        !matches!(
            self,
            Connection::Ready | Connection::Reconnecting | Connection::Offline { .. }
        )
    }
}

/// A paired PC (`remoteHosts()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedHost {
    pub id: SharedString,
    pub name: SharedString,
}

/// A row of the sidebar's Recents: a bot or a room.
#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub id: SharedString,
    pub name: SharedString,
    pub kind: RecentKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecentKind {
    Bot {
        color: BotColor,
        shape: BotShape,
        state: BotState,
    },
    /// A room, and whether it has posts you haven't seen.
    Room { unread: bool },
}

/// What the shell asks of the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Try again on the failed connection screen (`restartDaemon`): this PC's daemon, or the other PC again.
    Retry,
    /// Back to this PC's own Tjommie (`connectLocal`).
    ConnectLocal,
    /// To a saved PC (`connectRemote(id)`).
    ConnectRemote(SharedString),
    /// "Connect to another computer…": the dialog that pairs with another PC (T5.5's connect screen).
    ConnectAnother,
    /// Pair again with the PC that removed this device: the same dialog.
    PairAgain,
    /// Forget the PC that removed this device (`forgetRemote(id)`).
    Forget(SharedString),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_is_the_first_letter_upper_cased() {
        assert_eq!(initial(Some("tjommie")), "T");
        assert_eq!(initial(Some("élise")), "É");
        assert_eq!(initial(None), "");
        assert_eq!(initial(Some("")), "");
    }

    /// `app-shell.test.tsx`: the cached screens stay under the pill while the PC can't be reached; the removed
    /// screen takes their place.
    #[test]
    fn blocks_only_without_a_usable_connection() {
        let offline = Connection::Offline {
            reason: OfflineReason::Unreachable,
        };
        assert!(!offline.blocks());
        assert!(!Connection::Reconnecting.blocks());
        assert!(!Connection::Ready.blocks());
        for blocking in [
            Connection::Removed,
            Connection::Connecting,
            Connection::Update,
            Connection::Outside,
            Connection::Failed { reason: "x".into() },
        ] {
            assert!(blocking.blocks(), "{blocking:?}");
        }
    }
}
