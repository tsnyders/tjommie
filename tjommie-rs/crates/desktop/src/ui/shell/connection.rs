//! The connection screen, the status pill and the failed-load row (Phase 3 plan T2.1; `connection-screen.tsx`,
//! `app-shell.tsx`'s `StatusPill`, `failed-load.tsx`).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Transformation, Window, div, percentage, svg,
};

use super::data::{Command, Connection, OfflineReason, SavedHost, ShellData};
use crate::guard::guarded;
use crate::motion::{clock, reduced};
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// What the connection screen says and offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Words {
    pub title: SharedString,
    pub body: Option<SharedString>,
    pub spinner: bool,
    pub offers: Vec<Offer>,
}

/// A button on the connection screen and what it asks of the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Offer {
    pub id: &'static str,
    pub label: &'static str,
    pub command: Command,
    pub primary: bool,
}

/// The connection screen's words (`ConnectionScreen`, `RemovedScreen`), or `None` while the connection doesn't block.
/// `host` is the saved PC this window is on, `None` for This PC; another PC is named, since it isn't this PC's own
/// Tjommie that is starting or failing.
pub(super) fn words(
    connection: &Connection,
    host: Option<&str>,
    saved: &[SavedHost],
) -> Option<Words> {
    let pc = host.map(|host| {
        saved
            .iter()
            .find(|each| each.id == host)
            .map_or_else(|| "the other PC".into(), |each| each.name.clone())
    });
    let words = |title: SharedString, body: Option<&'static str>| Words {
        title,
        body: body.map(SharedString::from),
        spinner: false,
        offers: Vec::new(),
    };
    Some(match connection {
        Connection::Ready | Connection::Reconnecting | Connection::Offline { .. } => return None,
        Connection::Connecting => Words {
            spinner: true,
            ..words(
                pc.map_or_else(
                    || "Starting Tjommie".into(),
                    |pc| format!("Connecting to {pc}").into(),
                ),
                None,
            )
        },
        Connection::Failed { reason } => Words {
            body: Some(reason.clone()),
            offers: vec![Offer {
                id: "connection.retry",
                label: "Try again",
                command: Command::Retry,
                primary: true,
            }],
            ..words(
                pc.map_or_else(
                    || "Tjommie's background service didn't start".into(),
                    |pc| format!("Can't connect to {pc}").into(),
                ),
                None,
            )
        },
        Connection::Update => words(
            "Update Tjommie on this device".into(),
            Some(
                "This window and Tjommie's background service are different versions. Install the latest \
                 Tjommie, then open it again.",
            ),
        ),
        Connection::Outside => words(
            "Open Tjommie from the desktop app".into(),
            Some(
                "This page talks to Tjommie's background service, which only the desktop app can reach.",
            ),
        ),
        Connection::Removed => Words {
            offers: std::iter::once(Offer {
                id: "connection.pair",
                label: "Pair again",
                command: Command::PairAgain,
                primary: true,
            })
            .chain(host.map(|host| Offer {
                id: "connection.forget",
                label: "Forget",
                command: Command::Forget(host.to_owned().into()),
                primary: false,
            }))
            .collect(),
            ..words(
                "This PC removed this device".into(),
                Some("To use it again, pair again with a new code from that PC."),
            )
        },
    })
}

/// The connection screen in the main column (`ConnectionScreen`). The removed screen takes the focus to its heading
/// when it appears, so the removal is read out and the keyboard starts there.
pub(super) fn screen(
    data: &ShellData,
    words: Words,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let heading = window
        .use_keyed_state("connection.heading", cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    if data.connection == Connection::Removed {
        let moved = window.use_keyed_state("connection.removed.focused", cx, |_, _| false);
        if !*moved.read(cx) {
            moved.update(cx, |moved, _| *moved = true);
            let heading = heading.clone();
            window.defer(cx, move |window, cx| {
                guarded("focus the removed screen's heading", || {
                    heading.focus(window, cx)
                });
            });
        }
    }
    let spinner = words.spinner.then(|| spinner(20., window, cx));
    let theme = theme(cx);
    let colors = palette(cx);
    let problem = (data.connection == Connection::Removed)
        .then(|| data.problem.clone())
        .flatten();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(r(theme.space.s12))
        .px(r(theme.space.s32))
        .pb(r(theme.size.titlebar_height))
        .children(spinner)
        .child(
            div()
                .semantic("connection.title", Role::Heading)
                .aria_label(words.title.clone())
                .track_focus(&heading)
                .type_style(&theme.typography.desktop.display_s)
                .text_color(hsla(colors.text.primary))
                .text_center()
                .child(words.title),
        )
        .when_some(words.body, |screen, body| {
            screen.child(
                div()
                    .semantic("connection.body", Role::Paragraph)
                    .aria_label(body.clone())
                    .max_w(r(440.))
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.text.secondary))
                    .text_center()
                    .child(body),
            )
        })
        .when_some(problem, |screen, problem| {
            screen.child(
                div()
                    .semantic("connection.problem", Role::Alert)
                    .aria_label(problem.clone())
                    .max_w(r(440.))
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.danger.text))
                    .text_center()
                    .child(problem),
            )
        })
        .when(!words.offers.is_empty(), |screen| {
            screen.child(div().flex().gap(r(theme.space.s8)).children(
                words.offers.into_iter().map(|offer| {
                    let command = offer.command;
                    Pill::new(offer.id, offer.label)
                        .variant(if offer.primary {
                            PillVariant::Primary
                        } else {
                            PillVariant::Grey
                        })
                        .on_press(move |window, cx| ShellData::command(command.clone(), window, cx))
                }),
            ))
        })
        .into_any_element()
}

/// What the status pill says over the cached screens while the link is down (`StatusPill`), and whether it spins.
pub(super) fn status(connection: &Connection) -> Option<(&'static str, bool)> {
    match connection {
        Connection::Reconnecting => Some(("Reconnecting\u{2026}", true)),
        Connection::Offline {
            reason: OfflineReason::HostOffline,
        } => Some(("Your PC is offline", false)),
        Connection::Offline {
            reason: OfflineReason::Unreachable,
        } => Some(("Can't reach your PC", false)),
        _ => None,
    }
}

/// The status pill, top centre over the main column. Nothing in it listens for the mouse, so it takes no clicks and
/// the screen under it still works.
pub(super) fn status_pill(
    connection: &Connection,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let (text, spins) = status(connection)?;
    let icon = if spins {
        spinner(14., window, cx)
    } else {
        svg()
            .path("icons/i/175/wifi-off.svg")
            .size(r(14.))
            .text_color(hsla(palette(cx).icon.base))
            .into_any_element()
    };
    let theme = theme(cx);
    let colors = palette(cx);
    Some(
        div()
            .absolute()
            .top(r(theme.space.s12))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .semantic("shell.status", Role::Status)
                    .aria_label(text)
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s8))
                    .h(r(28.))
                    .px(r(theme.space.s12))
                    .rounded_full()
                    .bg(hsla(colors.surface.raised))
                    .shadow(shadows(&theme.shadow.light.card))
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(colors.text.secondary))
                    .child(icon)
                    .child(text),
            )
            .into_any_element(),
    )
}

/// A turning loader, for connections (spinners remain only for tools and connections). It turns on the motion clock,
/// so it keeps to the inactive window's 30 fps, and stands still under reduced motion.
pub(crate) fn spinner(size: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let loader = svg()
        .path("icons/i/175/loader-circle.svg")
        .size(r(size))
        .flex_none()
        .text_color(hsla(palette(cx).icon.base));
    if reduced::is_on(cx) {
        return loader.into_any_element();
    }
    let turned = (clock::now(cx) / (theme(cx).motion.duration.spin / 1000.)).fract();
    clock::keep_moving(window, cx);
    loader
        .with_transformation(Transformation::rotate(percentage(turned)))
        .into_any_element()
}

/// A chat, a room or an exchange whose history didn't load (`failed-load.tsx`). A new connection also loads it again.
pub fn failed_load(
    id: &str,
    text: impl Into<SharedString>,
    on_retry: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let text = text.into();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(r(theme.space.s12))
        .px(r(theme.space.s32))
        .child(
            div()
                .semantic(format!("{id}.text"), Role::Paragraph)
                .aria_label(text.clone())
                .type_style(&theme.typography.desktop.body)
                .text_color(hsla(palette(cx).text.primary))
                .text_center()
                .child(text),
        )
        .child(
            Pill::new(format!("{id}.retry"), "Retry")
                .variant(PillVariant::Grey)
                .small()
                .icon("rotate-cw")
                .on_press(on_retry),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::primitives::test_window;

    const HOME: &str = "5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11";

    fn saved() -> Vec<SavedHost> {
        vec![SavedHost {
            id: HOME.into(),
            name: "Home PC".into(),
        }]
    }

    fn on_home(connection: Connection) -> Words {
        words(&connection, Some(HOME), &saved()).expect("a blocking connection")
    }

    #[test]
    fn says_the_pc_removed_this_device_and_offers_pair_again_and_forget() {
        let removed = on_home(Connection::Removed);
        assert_eq!(removed.title, "This PC removed this device");
        assert_eq!(
            removed.body.as_deref(),
            Some("To use it again, pair again with a new code from that PC.")
        );
        let labels: Vec<_> = removed.offers.iter().map(|offer| offer.label).collect();
        assert_eq!(labels, ["Pair again", "Forget"]);
    }

    #[test]
    fn forgets_that_pc_on_forget() {
        let removed = on_home(Connection::Removed);
        assert_eq!(removed.offers[1].command, Command::Forget(HOME.into()));
        assert_eq!(removed.offers[0].command, Command::PairAgain);
    }

    #[test]
    fn names_the_other_pc_the_desktop_is_connecting_to() {
        let connecting = on_home(Connection::Connecting);
        assert_eq!(connecting.title, "Connecting to Home PC");
        assert!(connecting.spinner);
    }

    #[test]
    fn says_it_cant_connect_to_the_other_pc_why_and_tries_that_pc_again() {
        let failed = on_home(Connection::Failed {
            reason: "Can't reach your PC. Check that it's awake with Tjommie open.".into(),
        });
        assert_eq!(failed.title, "Can't connect to Home PC");
        assert_eq!(
            failed.body.as_deref(),
            Some("Can't reach your PC. Check that it's awake with Tjommie open.")
        );
        assert!(!failed.title.contains("background service"));
        assert_eq!(failed.offers[0].label, "Try again");
        assert_eq!(failed.offers[0].command, Command::Retry);
    }

    #[test]
    fn keeps_the_desktops_own_words_for_this_pc() {
        let this_pc = |connection| words(&connection, None, &saved()).unwrap().title;
        assert_eq!(this_pc(Connection::Connecting), "Starting Tjommie");
        assert_eq!(
            this_pc(Connection::Failed { reason: "x".into() }),
            "Tjommie's background service didn't start"
        );
    }

    #[test]
    fn says_nothing_while_the_connection_is_usable() {
        assert_eq!(words(&Connection::Ready, None, &[]), None);
        assert_eq!(words(&Connection::Reconnecting, None, &[]), None);
    }

    #[test]
    fn status_says_reconnecting_with_a_spinner() {
        assert_eq!(
            status(&Connection::Reconnecting),
            Some(("Reconnecting\u{2026}", true))
        );
    }

    #[test]
    fn status_says_offline_or_unreachable_without_a_spinner() {
        let offline = |reason| status(&Connection::Offline { reason });
        assert_eq!(
            offline(OfflineReason::HostOffline),
            Some(("Your PC is offline", false))
        );
        assert_eq!(
            offline(OfflineReason::Unreachable),
            Some(("Can't reach your PC", false))
        );
    }

    #[test]
    fn status_shows_nothing_while_the_connection_is_up() {
        assert_eq!(status(&Connection::Ready), None);
        assert_eq!(status(&Connection::Connecting), None);
    }

    #[gpui::test]
    fn takes_the_focus_to_its_heading_when_it_replaces_the_screen(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(ShellData {
                connection: Connection::Connecting,
                host: Some(HOME.into()),
                saved: saved(),
                ..ShellData::default()
            })
        });
        let cx = test_window(cx, |window, cx| {
            let data = cx.global::<ShellData>().clone();
            let shown = words(&data.connection, data.host.as_deref(), &data.saved).unwrap();
            screen(&data, shown, window, cx)
        });
        cx.run_until_parked();
        assert!(
            cx.update(|window, cx| window.focused(cx)).is_none(),
            "connecting: the focus stays put"
        );

        cx.update(|_, cx| cx.global_mut::<ShellData>().connection = Connection::Removed);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        assert!(
            cx.update(|window, cx| window.focused(cx)).is_some(),
            "removed: the heading takes the focus"
        );
    }

    #[gpui::test]
    fn stays_and_says_why_when_the_pc_cant_be_forgotten(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(ShellData {
                connection: Connection::Removed,
                host: Some(HOME.into()),
                saved: saved(),
                problem: Some("Windows couldn't delete this PC's key.".into()),
                ..ShellData::default()
            })
        });
        let cx = test_window(cx, |window, cx| {
            let data = cx.global::<ShellData>().clone();
            let shown = words(&data.connection, data.host.as_deref(), &data.saved).unwrap();
            screen(&data, shown, window, cx)
        });
        assert!(
            cx.debug_bounds("connection.problem").is_some(),
            "the reason shows"
        );
        assert!(
            cx.debug_bounds("connection.title").is_some(),
            "the screen stays"
        );
    }
}
