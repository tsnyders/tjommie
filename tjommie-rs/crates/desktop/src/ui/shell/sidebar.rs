//! The Aegis sidebar (Phase 3 plan T2.1; navigation-shell.html `.sidebar`, navigation-host-switcher.html; the port of
//! `rail.tsx` and `host-switcher.tsx`): the wordmark, the five sections, Recents with living avatars and state marks,
//! View all, and at the foot the host button, which opens the host list, beside the New agent pill.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    svg,
};
use protocol::model::BotState;

use super::data::{Command, Connection, Recent, RecentKind, SavedHost, ShellData};
use super::route::{Navigate as _, Route, Section};
use crate::assets::DISPLAY_FAMILY;
use crate::semantic::{Semantic, SemanticExt as _};
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::primitives::button::{Pill, PillVariant, pressable};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, shadows, theme};

/// This PC's value in the host list; a saved PC's is its id (`THIS_PC`).
pub(super) const THIS_PC: &str = "this-pc";

pub(super) fn sidebar(
    route: &Route,
    data: &ShellData,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let sections = [
        ("agents", "bot", "Agents", Route::Agents),
        ("rooms", "messages-square", "Rooms", Route::Rooms),
        ("projects", "folder-git-2", "Projects", Route::Projects),
        ("routines", "calendar-clock", "Routines", Route::Routines),
        (
            "settings",
            "settings",
            "Settings",
            Route::Settings(Section::General),
        ),
    ];
    let nav: Vec<AnyElement> = sections
        .into_iter()
        .map(|(key, icon, label, to)| {
            let selected = section_selected(&to, route);
            nav_row(key, icon, label, selected, to, window, cx)
        })
        .collect();
    let recents: Vec<AnyElement> = data
        .recents
        .iter()
        .map(|recent| recent_row(recent, route, window, cx))
        .collect();
    let view_all = view_all(window, cx);
    let host = host_menu(data);
    let theme = theme(cx);
    let colors = palette(cx);
    let wordmark = &theme.typography.desktop.wordmark;
    div()
        .w(r(theme.size.sidebar))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .pt(r(theme.space.s24))
        .px(r(theme.space.s16))
        .pb(r(theme.space.s16))
        .child(
            div().pl(r(theme.space.s12)).pr(r(theme.space.s8)).child(
                div()
                    .semantic("sidebar.wordmark", Role::Heading)
                    .aria_label("Tjommie")
                    .font_family(DISPLAY_FAMILY)
                    .font_weight(FontWeight(wordmark.weight))
                    .text_size(r(wordmark.size))
                    .line_height(r(wordmark.line_height))
                    .text_color(hsla(colors.text.primary))
                    .child("Tjommie"),
            ),
        )
        .child(
            div()
                .mt(r(theme.space.s28))
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(nav),
        )
        .child(
            div()
                .mx(r(theme.space.s12))
                .my(r(theme.space.s16))
                .h(px(1.))
                .flex_none()
                .bg(hsla(colors.line.base)),
        )
        .child(
            div()
                .px(r(theme.space.s12))
                .pb(r(theme.space.s6))
                .type_style(&theme.typography.desktop.label)
                .text_color(hsla(colors.text.tertiary))
                .child("Recents"),
        )
        .child(
            div()
                .id("sidebar.recents")
                .flex()
                .flex_col()
                .gap(px(1.))
                .min_h_0()
                .overflow_y_scroll()
                .children(recents),
        )
        .child(view_all)
        .child(
            div()
                .mt_auto()
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .pt(r(theme.space.s8))
                .pl(r(theme.space.s4))
                // The host button gives way to the pill (`.host-btn`'s min-width: 0): a long PC name runs into the gap.
                .child(div().flex_1().min_w_0().child(host))
                .child(
                    div().flex_none().child(
                        Pill::new("sidebar.new", "New agent")
                            .variant(PillVariant::Primary)
                            .tight()
                            .icon("plus")
                            .on_press(|_, cx| cx.navigate(Route::NewBot)),
                    ),
                ),
        )
        .into_any_element()
}

/// Whether the section that goes `to` shows as selected on `route`: Settings in any section, and Rooms in a room
/// too, as `rail.tsx` kept it for `/room/?id=` (M2 review F7).
fn section_selected(to: &Route, route: &Route) -> bool {
    match (to, route) {
        (Route::Settings(_), Route::Settings(_)) | (Route::Rooms, Route::Room(_)) => true,
        (to, route) => to == route,
    }
}

/// A sidebar row that goes somewhere: a link and a tab stop, followed by a click, Enter or Space. The selected row sits
/// on a raised card.
fn link_row(
    id: SharedString,
    label: SharedString,
    selected: bool,
    height: f32,
    to: Route,
    window: &mut Window,
    cx: &mut App,
) -> Semantic<Stateful<Div>> {
    let focus = focus_handle(&id, window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    let row = div()
        .semantic(id, Role::Link)
        .aria_label(label)
        .aria_selected(selected)
        .track_focus(&focus)
        .relative()
        .flex()
        .items_center()
        .gap(r(theme.space.s12))
        .h(r(height))
        .flex_none()
        .px(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .cursor_pointer()
        .text_color(hsla(colors.text.primary))
        .map(|row| {
            if selected {
                row.bg(hsla(colors.surface.raised))
                    .shadow(shadows(&theme.shadow.light.card))
                    .font_weight(FontWeight::SEMIBOLD)
            } else {
                row.hover(|style| style.bg(hsla(colors.state.hover)))
            }
        })
        .when(ring, |row| row.child(focus_ring(theme.radius.field, cx)));
    pressable(row, Some(Rc::new(move |_, cx| cx.navigate(to.clone()))))
}

fn nav_row(
    key: &str,
    icon: &str,
    label: &'static str,
    selected: bool,
    to: Route,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let row = link_row(
        format!("sidebar.nav.{key}").into(),
        label.into(),
        selected,
        40.,
        to,
        window,
        cx,
    );
    let theme = theme(cx);
    row.type_style(&theme.typography.desktop.nav)
        .child(
            svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(theme.size.icon))
                .flex_none()
                .text_color(hsla(palette(cx).icon.base)),
        )
        .child(label)
        .into_any_element()
}

/// A Recents row's state mark: a dot (needs you, needs setup, unread) or an icon (limited, paused).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mark {
    Dot,
    Icon(&'static str),
}

/// A Recents row's mark, and the state a screen reader hears after the name (navigation-shell.html's rows).
pub(super) fn mark(kind: &RecentKind) -> (Option<Mark>, Option<&'static str>) {
    match kind {
        RecentKind::Bot { state, .. } => match state {
            BotState::Waiting => (Some(Mark::Dot), Some("Needs you")),
            BotState::NeedsSetup => (Some(Mark::Dot), Some("Needs setup")),
            BotState::Limited => (Some(Mark::Icon("hourglass")), Some("Limited")),
            BotState::Paused => (Some(Mark::Icon("pause")), Some("Paused")),
            BotState::Working => (None, Some("Working")),
            BotState::Idle => (None, None),
        },
        RecentKind::Room { unread: true } => (Some(Mark::Dot), Some("Unread")),
        RecentKind::Room { unread: false } => (None, None),
    }
}

fn recent_row(recent: &Recent, route: &Route, window: &mut Window, cx: &mut App) -> AnyElement {
    let (to, selected) = match recent.kind {
        RecentKind::Bot { .. } => (
            Route::Chat(recent.id.to_string()),
            matches!(route, Route::Chat(id) | Route::BotSettings(id) if *id == recent.id),
        ),
        RecentKind::Room { .. } => (
            Route::Room(recent.id.to_string()),
            matches!(route, Route::Room(id) if *id == recent.id),
        ),
    };
    let (mark, said) = mark(&recent.kind);
    let label: SharedString = match said {
        Some(said) => format!("{}, {said}", recent.name).into(),
        None => recent.name.clone(),
    };
    let row = link_row(
        format!("sidebar.recent.{}", recent.id).into(),
        label,
        selected,
        36.,
        to,
        window,
        cx,
    );
    let theme = theme(cx);
    let colors = palette(cx);
    let face = match recent.kind {
        RecentKind::Bot {
            color,
            shape,
            state,
        } => BotAvatar::new(color, shape, theme.size.avatar20)
            .pose(Pose::of(state))
            .into_any_element(),
        RecentKind::Room { .. } => div()
            .size(r(theme.size.avatar20))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/i/175/messages-square.svg")
                    .size(r(theme.size.icon_sm))
                    .text_color(hsla(colors.icon.base)),
            )
            .into_any_element(),
    };
    row.type_style(&theme.typography.desktop.ui)
        .child(face)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(recent.name.clone()),
        )
        .when_some(mark, |row, mark| {
            row.child(match mark {
                Mark::Dot => div()
                    .size(px(8.))
                    .flex_none()
                    .rounded_full()
                    .bg(hsla(colors.accent.marker))
                    .into_any_element(),
                Mark::Icon(icon) => svg()
                    .path(format!("icons/i/175/{icon}.svg"))
                    .size(r(15.))
                    .flex_none()
                    .text_color(hsla(colors.text.tertiary))
                    .into_any_element(),
            })
        })
        .into_any_element()
}

fn view_all(window: &mut Window, cx: &mut App) -> AnyElement {
    let focus = focus_handle("sidebar.view_all", window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    let link = div()
        .semantic("sidebar.view_all", Role::Link)
        .aria_label("View all")
        .track_focus(&focus)
        .relative()
        .self_start()
        .flex()
        .flex_none()
        .items_center()
        .gap(r(theme.space.s4))
        .h(r(32.))
        .px(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .cursor_pointer()
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(colors.text.secondary))
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .child("View all")
        .child(
            svg()
                .path("icons/i/175/chevron-right.svg")
                .size(r(15.))
                .text_color(hsla(colors.text.secondary)),
        )
        .when(ring, |link| link.child(focus_ring(theme.radius.field, cx)));
    pressable(link, Some(Rc::new(|_, cx| cx.navigate(Route::Agents)))).into_any_element()
}

/// A row of the host list (`hostRows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HostRow {
    pub value: SharedString,
    pub label: SharedString,
    pub status: Option<&'static str>,
}

/// The host list: This PC, then the saved PCs. Only the PC in use has a known state; the others aren't connected.
pub(super) fn host_rows(
    saved: &[SavedHost],
    current: Option<&str>,
    connection: &Connection,
) -> Vec<HostRow> {
    let status = |id: &str| -> &'static str {
        if current != Some(id) {
            return "Not connected";
        }
        match connection {
            Connection::Ready => "Connected",
            Connection::Connecting | Connection::Reconnecting => "Connecting",
            Connection::Offline { .. } => "Offline",
            _ => "Not connected",
        }
    };
    std::iter::once(HostRow {
        value: THIS_PC.into(),
        label: "This PC".into(),
        status: None,
    })
    .chain(saved.iter().map(|host| HostRow {
        value: host.id.clone(),
        label: host.name.clone(),
        status: Some(status(&host.id)),
    }))
    .collect()
}

/// What picking `value` in the host list asks for (`chooseHost`). The PC in use is left alone, so its link isn't
/// dropped.
pub(super) fn choose_host(value: &str, current: Option<&str>) -> Option<Command> {
    if value == current.unwrap_or(THIS_PC) {
        None
    } else if value == THIS_PC {
        Some(Command::ConnectLocal)
    } else {
        Some(Command::ConnectRemote(value.to_owned().into()))
    }
}

/// The host button (`HostSwitcher`): its accessible name, the PC's name, and its face: the account's initial on This
/// PC, a monitor on another PC (`None`), since the window is somewhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HostButton {
    pub label: SharedString,
    pub name: SharedString,
    pub initial: Option<SharedString>,
}

pub(super) fn host_button(data: &ShellData) -> HostButton {
    let name: SharedString = match &data.host {
        None => "This PC".into(),
        Some(id) => data
            .saved
            .iter()
            .find(|host| host.id == *id)
            .map_or_else(|| "Another PC".into(), |host| host.name.clone()),
    };
    HostButton {
        label: format!("Switch PC, now on {name}").into(),
        initial: data.host.is_none().then(|| data.initial.clone()),
        name,
    }
}

fn host_menu(data: &ShellData) -> Menu {
    let button = host_button(data);
    let via = data.via.clone();
    let current: Option<SharedString> = data.host.clone();
    let on = current.clone().unwrap_or_else(|| THIS_PC.into());
    let menu = Menu::new("sidebar.host.menu", move |open, _, cx| {
        host_trigger(button, via, open, cx)
    })
    .anchor(gpui::Anchor::BottomLeft)
    .offset(px(8.));
    let menu = host_rows(&data.saved, data.host.as_deref(), &data.connection)
        .into_iter()
        .fold(menu, |menu, row| {
            let picked = row.value.clone();
            let current = current.clone();
            let item = MenuItem::new(
                format!("host.{}", row.value),
                row.label,
                move |window, cx| {
                    if let Some(command) = choose_host(&picked, current.as_deref()) {
                        ShellData::command(command, window, cx);
                    }
                },
            )
            .icon("monitor")
            .checked(row.value == on);
            menu.item(match row.status {
                Some(status) => item.detail(status),
                None => item,
            })
        });
    menu.rule().item(
        MenuItem::new(
            "host.connect",
            "Connect to another computer\u{2026}",
            |window, cx| ShellData::command(Command::ConnectAnother, window, cx),
        )
        .icon("plus"),
    )
}

fn host_trigger(button: HostButton, via: Option<SharedString>, open: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let face = div()
        .size(r(36.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(hsla(colors.surface.raised))
        .shadow(shadows(&theme.shadow.light.card))
        .type_style(&theme.typography.desktop.ui_strong)
        .text_color(hsla(colors.text.primary))
        .map(|face| match button.initial {
            Some(initial) => face.child(initial),
            None => face.child(
                svg()
                    .path("icons/i/175/monitor.svg")
                    .size(r(theme.size.icon_sm))
                    .text_color(hsla(colors.icon.base)),
            ),
        });
    div()
        .semantic("sidebar.host", Role::Button)
        .aria_label(button.label)
        .aria_expanded(open)
        .flex()
        .items_center()
        .gap(r(theme.space.s10))
        .min_w_0()
        .p(r(theme.space.s4))
        .pr(r(theme.space.s8))
        .rounded_full()
        .cursor_pointer()
        .when(open, |host| host.bg(hsla(colors.state.hover)))
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .child(face)
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .type_style(&theme.typography.desktop.ui_strong)
                        .text_color(hsla(colors.text.primary))
                        .whitespace_nowrap()
                        .child(button.name),
                )
                .when_some(via, |lines, via| {
                    lines.child(
                        div()
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(colors.text.tertiary))
                            .child(via),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::model::{BotColor, BotShape};

    const HOME: &str = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";
    const WORK: &str = "9c4f6b1d-2a7e-4e3b-8d55-0f6a1c2b3d4e";

    fn hosts() -> Vec<SavedHost> {
        vec![
            SavedHost {
                id: HOME.into(),
                name: "Home PC".into(),
            },
            SavedHost {
                id: WORK.into(),
                name: "Work PC".into(),
            },
        ]
    }

    fn row(value: &str, label: &str, status: Option<&'static str>) -> HostRow {
        HostRow {
            value: value.to_owned().into(),
            label: label.to_owned().into(),
            status,
        }
    }

    #[test]
    fn lists_this_pc_first_then_each_saved_pc_with_how_its_doing() {
        assert_eq!(
            host_rows(&hosts(), None, &Connection::Ready),
            [
                row(THIS_PC, "This PC", None),
                row(HOME, "Home PC", Some("Not connected")),
                row(WORK, "Work PC", Some("Not connected")),
            ]
        );
    }

    #[test]
    fn gives_the_pc_in_use_the_connections_state_and_the_others_not_connected() {
        let status =
            |connection: Connection| host_rows(&hosts(), Some(HOME), &connection)[1].status;
        assert_eq!(status(Connection::Ready), Some("Connected"));
        assert_eq!(status(Connection::Connecting), Some("Connecting"));
        assert_eq!(status(Connection::Reconnecting), Some("Connecting"));
        for reason in [
            super::super::data::OfflineReason::HostOffline,
            super::super::data::OfflineReason::Unreachable,
        ] {
            assert_eq!(status(Connection::Offline { reason }), Some("Offline"));
        }
        assert_eq!(status(Connection::Removed), Some("Not connected"));
        assert_eq!(
            host_rows(&hosts(), Some(HOME), &Connection::Ready)[2].status,
            Some("Not connected")
        );
    }

    #[test]
    fn connects_to_the_saved_pc_picked() {
        assert_eq!(
            choose_host(WORK, Some(HOME)),
            Some(Command::ConnectRemote(WORK.into()))
        );
    }

    #[test]
    fn goes_back_to_this_pcs_own_tjommie_on_this_pc() {
        assert_eq!(
            choose_host(THIS_PC, Some(HOME)),
            Some(Command::ConnectLocal)
        );
    }

    #[test]
    fn leaves_the_pc_in_use_alone_when_its_picked_again() {
        assert_eq!(choose_host(HOME, Some(HOME)), None);
        assert_eq!(choose_host(THIS_PC, None), None);
    }

    #[test]
    fn is_the_avatar_on_this_pc_with_the_accounts_initial() {
        let data = ShellData {
            initial: "T".into(),
            saved: hosts(),
            ..ShellData::default()
        };
        assert_eq!(
            host_button(&data),
            HostButton {
                label: "Switch PC, now on This PC".into(),
                name: "This PC".into(),
                initial: Some("T".into()),
            }
        );
    }

    #[test]
    fn shows_the_pcs_name_in_the_button_while_on_another_pc() {
        let data = ShellData {
            initial: "T".into(),
            host: Some(HOME.into()),
            saved: hosts(),
            ..ShellData::default()
        };
        assert_eq!(
            host_button(&data),
            HostButton {
                label: "Switch PC, now on Home PC".into(),
                name: "Home PC".into(),
                initial: None,
            }
        );
    }

    #[test]
    fn rooms_stays_selected_in_a_room() {
        assert!(section_selected(&Route::Rooms, &Route::Rooms));
        assert!(section_selected(
            &Route::Rooms,
            &Route::Room("launch-crew".into())
        ));
        assert!(section_selected(
            &Route::Settings(Section::General),
            &Route::Settings(Section::Devices)
        ));
        assert!(!section_selected(
            &Route::Agents,
            &Route::Chat("forge".into())
        ));
        assert!(!section_selected(&Route::Rooms, &Route::Agents));
    }

    #[test]
    fn marks_follow_the_design() {
        let bot = |state| RecentKind::Bot {
            color: BotColor::Blue,
            shape: BotShape::Hexagon,
            state,
        };
        assert_eq!(
            mark(&bot(BotState::Waiting)),
            (Some(Mark::Dot), Some("Needs you"))
        );
        assert_eq!(
            mark(&bot(BotState::NeedsSetup)),
            (Some(Mark::Dot), Some("Needs setup"))
        );
        assert_eq!(
            mark(&bot(BotState::Limited)),
            (Some(Mark::Icon("hourglass")), Some("Limited"))
        );
        assert_eq!(
            mark(&bot(BotState::Paused)),
            (Some(Mark::Icon("pause")), Some("Paused"))
        );
        assert_eq!(mark(&bot(BotState::Working)), (None, Some("Working")));
        assert_eq!(mark(&bot(BotState::Idle)), (None, None));
        assert_eq!(
            mark(&RecentKind::Room { unread: true }),
            (Some(Mark::Dot), Some("Unread"))
        );
        assert_eq!(mark(&RecentKind::Room { unread: false }), (None, None));
    }
}
