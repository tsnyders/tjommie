//! Navigation (Phase 3 plan T2.1): the route the main column shows, and `navigate`, which the sidebar, the screens and
//! notifications (T6.2) all go through.

use gpui::{App, Global};

/// A bot's id (`protocol::model::Bot::id`).
pub type BotId = String;
/// A room's id (`protocol::model::Room::id`).
pub type RoomId = String;

/// A section of Settings (`settings/page.tsx`'s `SECTIONS`); General when none is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    General,
    Appearance,
    Notifications,
    Files,
    Voice,
    Logins,
    Bots,
    Accounts,
    Usage,
    Devices,
    Diagnostics,
    About,
}

/// What a link into an account asks of Settings (`?action=` in `/settings/?section=accounts&account=…`): start its
/// sign-in, set its spending cap, or check its limit again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountAction {
    SignIn,
    Cap,
    Retry,
}

/// What the main column shows. The app opens on Agents.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Agents,
    Chat(BotId),
    /// The rooms list, the sidebar's Rooms (`/room/` without an id).
    Rooms,
    Room(RoomId),
    Projects,
    Routines,
    Settings(Section),
    /// Settings, Accounts, on one account: the deep link (`/settings/?section=accounts&account=<id>&action=<…>`) that
    /// the chat's setup banner and limit notices follow. The Accounts section focuses the control `action` names on that
    /// account's card (Sign in, Raise or Edit cap, Retry queued work) once it's there, once; a link never starts a
    /// sign-in or a retry by itself. The sidebar's Settings and the Accounts link show as selected.
    /// Nothing produces this route yet: its keyboard path (and `e2e_keyboard_navigation`'s step through it) arrives with
    /// the Chat lane's link producers, the setup banner and the limit notices (M5 review M8).
    Account {
        account: String,
        action: Option<AccountAction>,
    },
    NewBot,
    BotSettings(BotId),
}

impl Route {
    /// Where a notification's link goes (`notificationRoute`, which the daemon's notices carry as the toast's launch
    /// text): the room, else the agent's chat. Anything else, or an id that is empty or isn't escaped properly, is `None`.
    pub fn from_notification_link(link: &str) -> Option<Route> {
        let (room, id) = if let Some(id) = link.strip_prefix("/room/?id=") {
            (true, id)
        } else {
            (false, link.strip_prefix("/chat/?bot=")?)
        };
        let id = percent_decode(id).filter(|id| !id.is_empty() && !id.contains('&'))?;
        Some(if room {
            Route::Room(id)
        } else {
            Route::Chat(id)
        })
    }
}

/// `decodeURIComponent` for what `encodeURIComponent` made: `%XX` bytes, read as UTF-8. `None` for a bad escape.
fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.bytes();
    while let Some(byte) = rest.next() {
        if byte == b'%' {
            let high = rest.next().and_then(hex)?;
            let low = rest.next().and_then(hex)?;
            bytes.push(high << 4 | low);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

#[derive(Default)]
struct Current(Route);

impl Global for Current {}

pub trait Navigate {
    /// Shows `route` in the main column.
    fn navigate(&mut self, route: Route);
}

impl Navigate for App {
    fn navigate(&mut self, route: Route) {
        let current = &mut self.default_global::<Current>().0;
        if *current != route {
            *current = route;
            self.refresh_windows();
        }
    }
}

/// The route the main column shows.
pub fn current(cx: &App) -> Route {
    cx.try_global::<Current>()
        .map(|current| current.0.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notifications_link_goes_to_its_room_or_its_agents_chat() {
        // `notification_route`'s two shapes (`/room/?id=` and `/chat/?bot=`), percent-encoded as `encodeURIComponent` does.
        assert_eq!(
            Route::from_notification_link("/chat/?bot=0d6ff97e-a44e-42f4-86a0-27ace431918d"),
            Some(Route::Chat("0d6ff97e-a44e-42f4-86a0-27ace431918d".into()))
        );
        assert_eq!(
            Route::from_notification_link("/room/?id=team%20room%2F1"),
            Some(Route::Room("team room/1".into()))
        );
        assert_eq!(
            Route::from_notification_link("/chat/?bot=caf%C3%A9"),
            Some(Route::Chat("caf\u{e9}".into()))
        );
        // Anything else goes nowhere: an empty id, another path, a stray query, a bad escape.
        for link in [
            "",
            "/chat/?bot=",
            "/room/?id=",
            "/settings/?section=accounts",
            "/chat/?other=b1",
            "/chat/?bot=%zz",
            "https://example.com/chat/?bot=b1",
        ] {
            assert_eq!(Route::from_notification_link(link), None, "{link:?}");
        }
    }

    #[gpui::test]
    fn starts_on_agents_and_follows_navigate(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(current(cx), Route::Agents);
            cx.navigate(Route::Chat("bot-1".into()));
            assert_eq!(current(cx), Route::Chat("bot-1".into()));
            cx.navigate(Route::Settings(Section::Devices));
            assert_eq!(current(cx), Route::Settings(Section::Devices));
            let signin = Route::Account {
                account: "acct-1".into(),
                action: Some(AccountAction::SignIn),
            };
            cx.navigate(signin.clone());
            assert_eq!(current(cx), signin);
        });
    }
}
