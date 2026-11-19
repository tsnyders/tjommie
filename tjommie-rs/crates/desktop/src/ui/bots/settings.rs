//! Agent settings (`app/bots/settings/page.tsx`; WF8): the New agent form filled from the agent, About this agent, and
//! the action bar (Delete agent, Pause or Resume, Cancel, Save changes). Save sends only what changed since the form
//! opened. An unknown or deleted agent has no settings: the screen goes home.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use protocol::model::{Bot, BotPatch};
use protocol::wire::{BotsPause, BotsUpdate, Id, method};

use super::form::{self, FormState, Saved};
use super::{chip, footer, header, page};
use crate::guard::{guarded, guarded_async};
use crate::model::bot_form::{form_from_bot, name_problem, patch_for};
use crate::model::format::long_date;
use crate::model::labels::{error_text, state_label};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Link, problem};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{IconButton, Pill, PillVariant};
use crate::ui::primitives::dialog::open_confirm;
use crate::ui::primitives::scroll::scroll_handle;
use crate::ui::routines::rows::routine_rows;
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// Agent settings for `bot_id`, kept while the route shows it; each agent's settings start from that agent.
pub fn screen(bot_id: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(bot) = store::state(cx)
        .bots
        .iter()
        .find(|bot| bot.id == bot_id)
        .cloned()
    else {
        // An unknown or deleted agent: home, after this frame.
        window.defer(cx, |_, cx| {
            guarded("leave a gone agent's settings", || {
                cx.navigate(Route::Agents)
            });
        });
        return div().into_any_element();
    };
    window
        .use_keyed_state(
            SharedString::from(format!("screen.bot_settings.{bot_id}.view")),
            cx,
            move |window, cx| BotSettings::new(bot, window, cx),
        )
        .into_any_element()
}

/// About this agent's notes: being read, unreadable, or their text.
enum Notes {
    Loading,
    Failed,
    Read(String),
}

pub struct BotSettings {
    /// The agent as the form opened; the edit is measured from it.
    opened: Bot,
    form: Entity<FormState>,
    busy: bool,
    problem: Option<SharedString>,
    notes: Notes,
}

impl BotSettings {
    fn new(bot: Bot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = cx.new(|cx| FormState::new(form_from_bot(&bot), window, cx));
        cx.observe(&form, |_, _, cx| {
            guarded("follow the form", || cx.notify());
        })
        .detach();
        let call = store::request::<method::BotsNotes>(cx, Id { id: bot.id.clone() });
        cx.spawn(async move |this, cx| {
            guarded_async("read an agent's notes", async move {
                let read = call.await;
                let _ = this.update(cx, |this, cx| {
                    this.notes = match read {
                        Ok(notes) => Notes::Read(notes.text),
                        Err(_) => Notes::Failed,
                    };
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
        BotSettings {
            opened: bot,
            form,
            busy: false,
            problem: None,
            notes: Notes::Loading,
        }
    }
}

/// Runs one of the bar's actions (`run`): busy while it runs, its problem said, `then` after it worked.
fn run<T: 'static>(
    screen: &Entity<BotSettings>,
    call: futures::future::BoxFuture<'static, Result<T, crate::model::connection::RequestError>>,
    then: impl FnOnce(&mut App) + 'static,
    cx: &mut App,
) {
    screen.update(cx, |screen, cx| {
        screen.busy = true;
        screen.problem = None;
        cx.notify();
    });
    let screen = screen.clone();
    cx.spawn(async move |cx| {
        guarded_async("run an agent's action", async move {
            let done = call.await;
            cx.update(|cx| {
                screen.update(cx, |screen, cx| {
                    screen.busy = false;
                    if let Err(error) = &done {
                        screen.problem = Some(error_text(error).into());
                    }
                    cx.notify();
                });
                if done.is_ok() {
                    then(cx);
                }
            });
        })
        .await;
    })
    .detach();
}

impl Render for BotSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = store::state(cx).clone();
        // The agent as it is now: its state, name and routines follow changes made elsewhere.
        let bot = state
            .bots
            .iter()
            .find(|bot| bot.id == self.opened.id)
            .cloned()
            .unwrap_or_else(|| self.opened.clone());
        let screen = cx.entity();
        let form = self.form.read(cx).form.clone();
        let name_error = name_problem(&form.name, &state.bots, Some(&bot.id));
        let patch = patch_for(&self.opened, &form, &state.bots);
        let dirty = patch != BotPatch::default();
        let mine: Vec<protocol::model::Routine> = state
            .routines
            .iter()
            .filter(|routine| routine.bot_id == bot.id)
            .cloned()
            .collect();
        let routines = {
            let rows: Vec<_> = mine.iter().map(crate::ui::routines::row_of).collect();
            let bot_id = bot.id.clone();
            let summary = match mine.len() {
                0 => "None".to_owned(),
                1 => "1 routine".to_owned(),
                count => format!("{count} routines"),
            };
            form::routines_row(
                summary,
                move |window, cx| {
                    let open = Rc::new(move |key: &str, window: &mut Window, cx: &mut App| {
                        if let Some(routine) = mine.iter().find(|routine| routine.id == key) {
                            crate::ui::routines::edit(routine.clone(), window, cx);
                        }
                    });
                    div()
                        .child(routine_rows(
                            "bot_settings.routines",
                            &rows,
                            open,
                            window,
                            cx,
                        ))
                        .child(div().mt(r(8.)).child(
                            Link::new("bot_settings.routines.add", "Add routine").on_press(
                                move |window, cx| {
                                    crate::ui::routines::add(bot_id.clone(), window, cx)
                                },
                            ),
                        ))
                        .into_any_element()
                },
                window,
                cx,
            )
        };
        let saved = Saved {
            provider: bot.provider,
            account_id: bot.account_id.clone().flatten(),
        };
        let fields = form::fields(
            &self.form,
            Some(&bot.id),
            Some(saved),
            name_error.clone(),
            routines,
            window,
            cx,
        );
        let about = about(&bot, &self.notes, state.host.is_none(), cx);
        let chat = Route::Chat(bot.id.clone());
        let delete = {
            let (screen, id, name) = (screen.clone(), bot.id.clone(), bot.name.clone());
            IconButton::new("bot_settings.delete", "trash-2", "Delete agent").on_press(move |window, cx| {
                let (screen, id) = (screen.clone(), id.clone());
                open_confirm(
                    window,
                    cx,
                    format!("Delete {name}?"),
                    "Its chat, routines and home folder (notes, inbox and browser profile) are deleted. Attached \
                     project folders aren't touched.",
                    "Delete agent",
                    true,
                    move |_, cx| {
                        let call = store::request::<method::BotsDelete>(cx, Id { id: id.clone() });
                        run(&screen, call, |cx| cx.navigate(Route::Agents), cx);
                    },
                );
            })
        };
        let pause = {
            let (screen, id, paused) = (screen.clone(), bot.id.clone(), bot.paused);
            Pill::new(
                "bot_settings.pause",
                if paused {
                    "Resume agent"
                } else {
                    "Pause agent"
                },
            )
            .icon(if paused { "play" } else { "pause" })
            .small()
            .on_press(move |_, cx| {
                let call = store::request::<method::BotsPause>(
                    cx,
                    BotsPause {
                        id: id.clone(),
                        paused: !paused,
                    },
                );
                run(&screen, call, |_| {}, cx);
            })
        };
        let can_save = !self.busy && dirty && name_error.is_none();
        let save = {
            let (screen, id, patch, chat) =
                (screen.clone(), bot.id.clone(), patch.clone(), chat.clone());
            Pill::new("bot_settings.save", "Save changes")
                .variant(PillVariant::Primary)
                .disabled(!can_save)
                .on_press(move |_, cx| {
                    let call = store::request::<method::BotsUpdate>(
                        cx,
                        BotsUpdate {
                            id: id.clone(),
                            patch: patch.clone(),
                        },
                    );
                    let chat = chat.clone();
                    run(&screen, call, move |cx| cx.navigate(chat), cx);
                })
        };
        let failed = self
            .problem
            .clone()
            .map(|text| problem("bot_settings.problem", text, cx));
        let actions = vec![
            delete.into_any_element(),
            pause.into_any_element(),
            div().flex_1().into_any_element(),
            Pill::new("bot_settings.cancel", "Cancel")
                .on_press(move |_, cx| cx.navigate(chat.clone()))
                .into_any_element(),
            save.into_any_element(),
        ];
        let zone = jiff::tz::TimeZone::system();
        let status = state_label(bot.state, bot.limited_until.as_deref(), &zone);
        let scroll = scroll_handle("bot_form.scroll", window, cx);
        page(
            &scroll,
            header(
                "bot_settings.back",
                chip(
                    BotAvatar::new(bot.color, bot.shape, 24.).into_any_element(),
                    div()
                        .semantic("screen.bot_settings", Role::Heading)
                        .aria_label(bot.name.clone())
                        .min_w_0()
                        .truncate()
                        .child(bot.name.clone())
                        .into_any_element(),
                    Some(status),
                    cx,
                ),
                window,
                cx,
            ),
            fields,
            about,
            footer(failed, actions, cx),
            cx,
        )
    }
}

/// About this agent (WF8): when it was made, its home folder, and the start of its notes.md. A missing notes.md comes
/// back empty, so a failed read means the notes couldn't be read. Open folder and Open notes.md are this PC's only.
fn about(bot: &Bot, notes: &Notes, local: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let zone = jiff::tz::TimeZone::system();
    let caption = |text: &'static str| {
        div()
            .mt(r(theme.space.s8))
            .type_style(&theme.typography.desktop.meta)
            .text_color(hsla(colors.text.secondary))
            .child(text)
    };
    let shown = match notes {
        Notes::Loading => "Loading".to_owned(),
        Notes::Failed => "Couldn't read the notes.".to_owned(),
        Notes::Read(text) if text.trim().is_empty() => "No notes yet.".to_owned(),
        Notes::Read(text) => text.trim().to_owned(),
    };
    let home = bot.home_folder.clone();
    let notes_home = bot.home_folder.clone();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .type_style(&theme.typography.desktop.label)
                .text_color(hsla(colors.text.tertiary))
                .child("About this agent"),
        )
        .child(
            div()
                .mt(r(theme.space.s6))
                .p(r(theme.space.s16))
                .rounded(r(theme.radius.card))
                .bg(hsla(colors.surface.raised))
                .border_t_1()
                .border_color(hsla(colors.overlay.highlight))
                .shadow(shadows(&theme.shadow.light.card))
                .flex()
                .flex_col()
                .child(caption("Created").mt(r(0.)))
                .child(
                    div()
                        .type_style(&theme.typography.desktop.ui)
                        .text_color(hsla(colors.text.primary))
                        .child(long_date(&bot.created_at, &zone)),
                )
                .child(caption("Home folder"))
                .children(local.then(|| {
                    Link::new("bot_settings.open_folder", "Open folder")
                        .icon("folder")
                        .on_press(move |_, _| {
                            if let Err(why) = desktop_host::links::open_folder(&home) {
                                log::warn!("Open folder: {why}");
                            }
                        })
                }))
                .child(caption("Notes"))
                .child(
                    div()
                        .semantic("bot_settings.notes", Role::Paragraph)
                        .aria_label(shown.clone())
                        .mt(r(theme.space.s6))
                        .max_h(r(160.))
                        .overflow_hidden()
                        .p(r(theme.space.s12))
                        .rounded(r(theme.radius.field))
                        .bg(hsla(colors.surface.panel))
                        .type_style(&theme.typography.desktop.meta)
                        .text_color(hsla(colors.text.secondary))
                        .child(shown),
                )
                .children(local.then(|| {
                    div().mt(r(theme.space.s10)).child(
                        Link::new("bot_settings.open_notes", "Open notes.md").on_press(
                            move |_, cx| {
                                let Some(root) = cx.try_global::<crate::live::DataRoot>() else {
                                    return;
                                };
                                let agents = root.0.join("bots");
                                if let Err(why) = desktop_host::links::open_notes(
                                    &agents,
                                    std::path::Path::new(&notes_home),
                                ) {
                                    log::warn!("Open notes.md: {why}");
                                }
                            },
                        ),
                    )
                })),
        )
        .into_any_element()
}
