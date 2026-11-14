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
    NewBot,
    BotSettings(BotId),
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

    #[gpui::test]
    fn starts_on_agents_and_follows_navigate(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(current(cx), Route::Agents);
            cx.navigate(Route::Chat("bot-1".into()));
            assert_eq!(current(cx), Route::Chat("bot-1".into()));
            cx.navigate(Route::Settings(Section::Devices));
            assert_eq!(current(cx), Route::Settings(Section::Devices));
        });
    }
}
