//! Settings (Phase 3 plan T5.5; `app/settings/page.tsx`, `settings/section.tsx`): the sections at the left and the
//! chosen one beside them. Each section is its own module with `section(window, cx)`; they share the frame (a title
//! and a line saying what's in it), the card and its rows (`SettingsRow`), and the problem line.

mod about;
mod account_dialogs;
mod accounts;
mod appearance;
mod bots;
mod devices;
mod diagnostics;
mod files;
mod general;
mod logins;
mod notifications;
mod usage;
mod voice;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, svg,
};

use crate::semantic::SemanticExt as _;
use crate::ui::fields::arrows;
use crate::ui::primitives::button::pressable;
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::scroll::{follow_focus, scroll_handle};
use crate::ui::shell::route::{Navigate as _, Route, Section};
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, shadows, theme};

/// The sections in the order they show (`SECTIONS`): the key in ids, the label and the icon.
pub const SECTIONS: [(Section, &str, &str, &str); 12] = [
    (Section::General, "general", "General", "sliders-horizontal"),
    (Section::Appearance, "appearance", "Appearance", "palette"),
    (
        Section::Notifications,
        "notifications",
        "Notifications",
        "bell",
    ),
    (Section::Files, "files", "Files", "file"),
    (Section::Voice, "voice", "Voice", "mic"),
    (Section::Logins, "logins", "Logins", "key-round"),
    (Section::Bots, "bots", "Agents", "bot"),
    (Section::Accounts, "accounts", "Accounts", "key-round"),
    (Section::Usage, "usage", "Usage", "chart-column"),
    (Section::Devices, "devices", "Devices", "smartphone"),
    (
        Section::Diagnostics,
        "diagnostics",
        "Diagnostics",
        "stethoscope",
    ),
    (Section::About, "about", "About", "info"),
];

/// Settings on `section`.
pub fn screen(section: Section, window: &mut Window, cx: &mut App) -> AnyElement {
    let nav = nav(section, window, cx);
    let content = match section {
        Section::General => general::section(window, cx),
        Section::Appearance => appearance::section(window, cx),
        Section::Notifications => notifications::section(window, cx),
        Section::Files => files::section(window, cx),
        Section::Voice => voice::section(window, cx),
        Section::Logins => logins::section(window, cx),
        Section::Bots => bots::section(window, cx),
        Section::Accounts => accounts::section(window, cx),
        Section::Usage => usage::section(window, cx),
        Section::Devices => devices::section(window, cx),
        Section::Diagnostics => diagnostics::section(window, cx),
        Section::About => about::section(window, cx),
    };
    let scroll = scroll_handle("settings.scroll", window, cx);
    let theme = theme(cx);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .child(nav)
        .child(follow_focus(
            &scroll,
            div()
                .id("settings.scroll")
                .track_scroll(&scroll)
                .flex_1()
                .min_w_0()
                .overflow_y_scroll()
                .pt(r(theme.space.s32))
                .px(r(theme.space.s32))
                .pb(r(theme.space.s40))
                .child(content),
        ))
        .into_any_element()
}

/// The sections' list (`NavLink`s): a link each, the chosen one on a raised card; the arrow keys step through them.
fn nav(chosen: Section, window: &mut Window, cx: &mut App) -> AnyElement {
    let links: Vec<AnyElement> = SECTIONS
        .iter()
        .map(|(section, key, label, icon)| {
            nav_link(*section, key, label, icon, *section == chosen, window, cx)
        })
        .collect();
    let list = window
        .use_keyed_state("settings.nav.list", cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .w(r(220.))
        .flex_none()
        .flex()
        .flex_col()
        .pt(r(theme.space.s32))
        .px(r(theme.space.s12))
        .pb(r(theme.space.s16))
        .border_r_1()
        .border_color(hsla(colors.line.base))
        .child(
            div()
                .semantic("settings.title", Role::Heading)
                .aria_label("Settings")
                .px(r(theme.space.s12))
                .mb(r(theme.space.s16))
                .type_style(&theme.typography.desktop.display_s)
                .text_color(hsla(colors.text.primary))
                .child("Settings"),
        )
        .child(arrows(
            div()
                .id("settings.nav")
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(links),
            &list,
        ))
        .into_any_element()
}

fn nav_link(
    section: Section,
    key: &str,
    label: &'static str,
    icon: &str,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: SharedString = format!("settings.nav.{key}").into();
    let focus = focus_handle(&id, window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    let link = div()
        .semantic(id, Role::Link)
        .aria_label(label)
        .aria_selected(selected)
        .track_focus(&focus)
        .relative()
        .flex()
        .items_center()
        .gap(r(theme.space.s10))
        .h(r(36.))
        .px(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .cursor_pointer()
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(colors.text.primary))
        .map(|link| {
            if selected {
                link.bg(hsla(colors.surface.raised))
                    .shadow(shadows(&theme.shadow.light.card))
                    .font_weight(FontWeight::MEDIUM)
            } else {
                link.hover(|style| style.bg(hsla(colors.state.hover)))
            }
        })
        .child(
            svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(16.))
                .flex_none()
                .text_color(hsla(if selected {
                    colors.text.primary
                } else {
                    colors.text.secondary
                })),
        )
        .child(label)
        .when(ring, |link| link.child(focus_ring(theme.radius.field, cx)));
    pressable(
        link,
        Some(std::rc::Rc::new(move |_, cx: &mut App| {
            cx.navigate(Route::Settings(section))
        })),
    )
    .into_any_element()
}

/// A section's frame (`SettingsSection`): its title and the line saying what's in it (none for `""`, as Usage's
/// bare heading has), then its parts.
pub(crate) fn frame(key: &str, title: &'static str, note: &'static str, cx: &App) -> Div {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .max_w(r(700.))
        .flex()
        .flex_col()
        .child(
            div()
                .semantic(format!("settings.{key}.title"), Role::Heading)
                .aria_label(title)
                .type_style(&theme.typography.desktop.display_m)
                .text_color(hsla(colors.text.primary))
                .child(title),
        )
        .when(!note.is_empty(), |frame| {
            frame.child(
                div()
                    .mt(r(theme.space.s8))
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.text.secondary))
                    .child(note),
            )
        })
}

/// A settings card (`SettingsCard`): raised, its rows ruled between (`SettingsRow::ruled`).
pub(crate) fn card(cx: &App) -> Div {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .mt(r(theme.space.s16))
        .flex()
        .flex_col()
        .min_w_0()
        .px(r(theme.space.s16))
        .rounded(r(theme.radius.card))
        .bg(hsla(colors.surface.raised))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.card))
}

/// A small heading over a card (`h3`, `.section-label`).
pub(crate) fn heading(text: &'static str, cx: &App) -> Div {
    let theme = theme(cx);
    div()
        .mt(r(theme.space.s32))
        .type_style(&theme.typography.desktop.label)
        .text_color(hsla(palette(cx).text.tertiary))
        .child(text)
}

#[cfg(test)]
mod tests {
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{self, Route, Section};
    use crate::ui::testing::{app, click, draw, ids, next_frame, node, state};

    /// The chat's setup banner links to one account (`?section=accounts&account=…&action=signin`).
    #[gpui::test]
    fn an_account_link_opens_accounts(cx: &mut gpui::TestAppContext) {
        let link = Route::Account {
            account: "acct-1".into(),
            action: Some(crate::ui::shell::route::AccountAction::SignIn),
        };
        let (_, _) = app(cx, link, state(vec![], vec![], vec![]), FakeRpc::new());
        assert!(node("settings.accounts.title").is_some());
        assert!(node("settings.nav.accounts").unwrap().selected);
        assert!(node("sidebar.nav.settings").unwrap().selected);
    }

    #[gpui::test]
    fn every_section_is_listed_and_a_link_opens_it(cx: &mut gpui::TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::General),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        assert_eq!(ids("settings.nav.").len(), 12);
        assert!(node("settings.nav.general").unwrap().selected);
        assert!(node("settings.general.title").is_some());
        click(cx, "settings.nav.devices");
        assert_eq!(
            cx.update(|_, cx| route::current(cx)),
            Route::Settings(Section::Devices)
        );
        assert!(node("settings.nav.devices").unwrap().selected);
        assert!(node("settings.devices.title").is_some());
    }

    /// T5.6: Tab moving to a control below the fold scrolls the section so that control shows.
    #[gpui::test]
    fn tab_brings_a_control_below_the_fold_into_view(cx: &mut gpui::TestAppContext) {
        // Appearance: its choices are Tab stops all the way down.
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Appearance),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let height = 240.;
        cx.simulate_resize(gpui::size(gpui::px(960.), gpui::px(height)));
        draw(cx);
        let content = |id: &str| id.starts_with("settings.") && !id.starts_with("settings.nav.");
        // The controls whose middle is below the fold before anything is focused: one shown later was scrolled to.
        let below: Vec<String> = crate::test_driver::nodes()
            .into_iter()
            .filter(|node| {
                node.interactive
                    && content(&node.id)
                    && node.bounds.y + node.bounds.height / 2. > height
            })
            .map(|node| node.id)
            .collect();
        assert!(
            !below.is_empty(),
            "Appearance reaches below the fold at this height"
        );
        let mut reached = Vec::new();
        for _ in 0..80 {
            // One draw for the key, then only the frame the app asks for (M5 review M1): no other event prompts it.
            cx.simulate_keystrokes("tab");
            draw(cx);
            next_frame(cx);
            let Some(now) = crate::test_driver::nodes()
                .into_iter()
                .find(|node| node.focused)
            else {
                continue;
            };
            if content(&now.id) {
                let middle = now.bounds.y + now.bounds.height / 2.;
                assert!(
                    (0. ..=height).contains(&middle),
                    "{} has the focus out of view at {:?}",
                    now.id,
                    now.bounds
                );
                reached.push(now.id);
            }
        }
        assert!(
            below.iter().any(|id| reached.contains(id)),
            "Tab reached one that started below: {below:?}, {reached:?}"
        );
    }

    /// M5 review M1: a control is brought into view once per focus. A wheel that then scrolls away from it (a wheel
    /// leaves the keyboard modality, so the ring stays) isn't undone on the frames after.
    #[gpui::test]
    fn a_wheel_after_tab_is_not_undone(cx: &mut gpui::TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Appearance),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let height = 240.;
        cx.simulate_resize(gpui::size(gpui::px(960.), gpui::px(height)));
        draw(cx);
        let focused = || {
            crate::test_driver::nodes()
                .into_iter()
                .find(|node| node.focused)
        };
        // Tab until the focus has scrolled the page: the focused control is then near the bottom of the view.
        let mut revealed = None;
        for _ in 0..80 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            if next_frame(cx) {
                revealed = focused();
                break;
            }
        }
        let revealed = revealed.expect("a Tab scrolled the page");
        // The wheel scrolls back up: the content, and the control with it, move down.
        cx.simulate_event(gpui::ScrollEvent {
            position: gpui::point(gpui::px(700.), gpui::px(120.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(120.))),
            ..Default::default()
        });
        draw(cx);
        let wheeled = focused().expect("the focus stays");
        assert_eq!(wheeled.id, revealed.id);
        assert!(
            wheeled.bounds.y > revealed.bounds.y,
            "the wheel scrolled: {:?} then {:?}",
            revealed.bounds,
            wheeled.bounds
        );
        // Nothing asks for a frame to undo it, and a later frame leaves it where the wheel put it.
        assert!(!next_frame(cx), "no reveal after the wheel");
        draw(cx);
        assert_eq!(focused().map(|node| node.bounds), Some(wheeled.bounds));
    }
}
