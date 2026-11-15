//! The chat's header (desktop-chat.html `.chat-head`; `chat-header.tsx`): the agent's avatar, name and state over its
//! provider, then Pause or Resume, Agent settings, More (Open browser, Open home folder) and the panel button. A
//! failed action says why under the header.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use jiff::tz::TimeZone;
use protocol::model::{Bot, BotState};
use protocol::wire::{BotsPause, method};

use crate::guard::guarded_async;
use crate::model::labels::{error_text, model_label, state_label};
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::primitives::button::{Handler, IconButton, IconVariant};
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// What the header's buttons do that only the chat knows: the panel, and the browser pane in it.
#[derive(Clone)]
pub struct HeaderActions {
    pub panel_open: bool,
    pub on_toggle_panel: Handler,
    pub on_open_browser: Handler,
}

/// A header action that failed, said under it until the next one.
#[derive(Default)]
struct Problem(Option<SharedString>);

/// The state pill's look and words: Needs you in ochre, Working on stone, the rest quiet; idle shows none.
fn state_pill(bot: &Bot, cx: &App) -> Option<AnyElement> {
    let theme = theme(cx);
    let colors = palette(cx);
    let words = match bot.state {
        BotState::Idle => return None,
        BotState::Waiting => "Needs you".to_owned(),
        BotState::NeedsSetup => "Needs setup".to_owned(),
        _ => state_label(bot.state, bot.limited_until.as_deref(), &TimeZone::system()),
    };
    let pill = div()
        .semantic(format!("chat.{}.state", bot.id), Role::Status)
        .aria_label(SharedString::from(words.clone()))
        .flex_none()
        .h(r(24.))
        .flex()
        .items_center()
        .gap(r(theme.space.s6))
        .rounded_full()
        .type_style(&theme.typography.desktop.label);
    let pill = match bot.state {
        BotState::Waiting | BotState::NeedsSetup => pill
            .px(r(theme.space.s10))
            .bg(hsla(colors.accent.tint))
            .text_color(hsla(colors.accent.text))
            .child(
                div()
                    .size(r(7.))
                    .rounded_full()
                    .bg(hsla(colors.accent.marker)),
            ),
        BotState::Working => pill
            .px(r(theme.space.s10))
            .bg(hsla(colors.surface.sunk))
            .text_color(hsla(colors.text.primary))
            .child(
                div()
                    .size(r(7.))
                    .rounded_full()
                    .bg(hsla(colors.text.primary)),
            ),
        _ => {
            let icon = if bot.state == BotState::Limited {
                "hourglass"
            } else {
                "pause"
            };
            pill.px(r(theme.space.s10))
                .border_1()
                .border_color(hsla(colors.line.soft))
                .text_color(hsla(colors.text.secondary))
                .child(
                    svg()
                        .path(format!("icons/i/200/{icon}.svg"))
                        .size(r(13.))
                        .flex_none()
                        .text_color(hsla(colors.text.secondary)),
                )
        }
    };
    Some(pill.child(words).into_any_element())
}

pub fn header(bot: &Bot, actions: &HeaderActions, window: &mut Window, cx: &mut App) -> AnyElement {
    let problem: Entity<Problem> = window.use_keyed_state(
        SharedString::from(format!("chat.{}.problem", bot.id)),
        cx,
        |_, _| Problem::default(),
    );
    let shown_problem = problem.read(cx).0.clone();
    let local = store::state(cx).host.is_none();
    let theme = theme(cx);
    let colors = palette(cx);
    let id = bot.id.clone();
    let name = SharedString::from(bot.name.clone());
    let pill = state_pill(bot, cx);

    let pause = {
        let (problem, id, paused) = (problem.clone(), id.clone(), bot.paused);
        let (icon, label) = if paused {
            ("play", format!("Resume {}", bot.name))
        } else {
            ("pause", format!("Pause {}", bot.name))
        };
        IconButton::new(format!("chat.{id}.pause"), icon, label).on_press(move |_, cx| {
            let call = store::request::<method::BotsPause>(
                cx,
                BotsPause {
                    id: id.clone(),
                    paused: !paused,
                },
            );
            run(&problem, call, cx);
        })
    };
    let settings = {
        let id = id.clone();
        IconButton::new(
            format!("chat.{id}.settings"),
            "sliders-horizontal",
            "Agent settings",
        )
        .on_press(move |_, cx| cx.navigate(Route::BotSettings(id.clone())))
    };
    let trigger = format!("chat.{id}.more.open");
    let mut more = Menu::new(
        SharedString::from(format!("chat.{id}.more")),
        move |open, _, _| {
            IconButton::new(trigger, "ellipsis", "More")
                .variant(if open {
                    IconVariant::Grey
                } else {
                    IconVariant::Plain
                })
                .into_any_element()
        },
    )
    .anchor(gpui::Anchor::TopRight)
    .offset(gpui::px(6.));
    if bot.browser {
        let open = actions.on_open_browser.clone();
        more = more.item(
            MenuItem::new(
                format!("chat.{id}.more.browser"),
                "Open browser",
                move |window, cx| open(window, cx),
            )
            .icon("globe"),
        );
    }
    // Only this PC's own folders open here; another PC's are its own to open.
    if local {
        let (problem, folder) = (problem.clone(), bot.home_folder.clone());
        more = more.item(
            MenuItem::new(
                format!("chat.{id}.more.folder"),
                "Open home folder",
                move |_, cx| {
                    let opened = desktop_host::links::open_folder(&folder);
                    problem.update(cx, |problem, cx| {
                        problem.0 = opened.err().map(SharedString::from);
                        cx.notify();
                    });
                },
            )
            .icon("folder-git-2"),
        );
    }
    let toggle = actions.on_toggle_panel.clone();
    let panel = IconButton::new(
        format!("chat.{id}.panel"),
        "panel-right",
        if actions.panel_open {
            "Hide the panel"
        } else {
            "Show the panel"
        },
    )
    .variant(if actions.panel_open {
        IconVariant::Grey
    } else {
        IconVariant::Plain
    })
    .on_press(move |window, cx| toggle(window, cx));

    div()
        .relative()
        .flex_none()
        .h(r(52.))
        .flex()
        .items_center()
        .gap(r(theme.space.s12))
        .pl(r(theme.space.s24))
        .pr(r(theme.space.s16))
        .child(BotAvatar::new(bot.color, bot.shape, theme.size.avatar40).pose(Pose::of(bot.state)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s10))
                        .child(
                            div()
                                .semantic(format!("chat.{id}.name"), Role::Heading)
                                .aria_label(name.clone())
                                .min_w_0()
                                .truncate()
                                .type_style(&theme.typography.desktop.display_s)
                                .text_color(hsla(colors.text.primary))
                                .child(name),
                        )
                        .children(pill),
                )
                .child(
                    div()
                        .mt(gpui::px(3.))
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.tertiary))
                        .child(SharedString::from(model_label(bot.provider, &bot.model))),
                ),
        )
        .child(pause)
        .child(settings)
        .child(more)
        .child(panel)
        .when_some(shown_problem, |header, problem| {
            header.child(
                div()
                    .semantic(format!("chat.{id}.problem"), Role::Alert)
                    .aria_label(problem.clone())
                    .absolute()
                    .top_full()
                    .left(r(theme.space.s24))
                    .px(r(theme.space.s12))
                    .py(r(theme.space.s6))
                    .rounded_full()
                    .bg(hsla(colors.surface.raised))
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(theme.color.bot.red))
                    .child(problem),
            )
        })
        .into_any_element()
}

/// Runs a header action and says why if it failed.
fn run<T: 'static>(
    problem: &Entity<Problem>,
    call: futures::future::BoxFuture<'static, Result<T, crate::model::connection::RequestError>>,
    cx: &mut App,
) {
    problem.update(cx, |problem, cx| {
        problem.0 = None;
        cx.notify();
    });
    let problem = problem.clone();
    cx.spawn(async move |cx| {
        guarded_async("run a header action", async move {
            if let Err(error) = call.await {
                problem.update(cx, |problem, cx| {
                    problem.0 = Some(error_text(&error).into());
                    cx.notify();
                });
            }
        })
        .await;
    })
    .detach();
}
