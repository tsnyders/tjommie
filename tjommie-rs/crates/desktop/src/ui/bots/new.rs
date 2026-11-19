//! New agent (`app/bots/new/page.tsx`; WF3): the form, Suggestions beside it, and Create agent. Routines added here
//! are drafts until the agent is made, then created with it; any that fail are named, and the agent, made already,
//! opens from Open <name> instead of being made again.

use std::rc::Rc;

use futures::FutureExt as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div,
};
use protocol::model::Bot;
use protocol::wire::method;

use super::form::{self, FormState};
use super::{chip, footer, header, page};
use crate::guard::{guarded, guarded_async};
use crate::model::bot_form::{RoutineDraft, create_input, name_problem, new_bot, routines_summary};
use crate::model::labels::error_text;
use crate::model::routines::save_routine;
use crate::model::suggestions::{Suggestion, suggestions};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Link, problem};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{Pill, PillVariant, pressable};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::scroll::scroll_handle;
use crate::ui::routines::editor::{RoutineEditor, local_zone, open_routine_editor};
use crate::ui::routines::rows::{RoutineRow, routine_rows};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// New agent, kept while the route shows it.
pub fn screen(window: &mut Window, cx: &mut App) -> AnyElement {
    window
        .use_keyed_state("screen.new_bot.view", cx, NewBot::new)
        .into_any_element()
}

pub struct NewBot {
    form: Entity<FormState>,
    routines: Vec<RoutineDraft>,
    touched: bool,
    busy: bool,
    problem: Option<SharedString>,
    /// The agent, once made: this screen mustn't offer to make it again.
    created: Option<Bot>,
}

impl NewBot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = cx.new(|cx| FormState::new(new_bot(), window, cx));
        cx.observe(&form, |_, _, cx| {
            guarded("follow the form", || cx.notify());
        })
        .detach();
        NewBot {
            form,
            routines: Vec::new(),
            touched: false,
            busy: false,
            problem: None,
            created: None,
        }
    }

    fn apply(&mut self, suggestion: &Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        let mut next = new_bot();
        next.name = suggestion.form.name.to_owned();
        next.color = suggestion.form.color;
        next.shape = suggestion.form.shape;
        next.instructions = suggestion.form.instructions.to_owned();
        next.browser = suggestion.form.browser;
        self.form
            .update(cx, |state, cx| state.replace(next, window, cx));
        self.routines = suggestion.routines.clone();
        self.touched = false;
        self.problem = None;
        cx.notify();
    }
}

/// Create agent (`create`): the agent, then each routine; the agent's chat opens when all of it worked.
fn create(screen: &Entity<NewBot>, cx: &mut App) {
    let bots = store::state(cx).bots.clone();
    let (busy, form, routines) = {
        let screen = screen.read(cx);
        (
            screen.busy,
            screen.form.read(cx).form.clone(),
            screen.routines.clone(),
        )
    };
    screen.update(cx, |screen, cx| {
        screen.touched = true;
        cx.notify();
    });
    if busy || name_problem(&form.name, &bots, None).is_some() {
        return;
    }
    screen.update(cx, |screen, cx| {
        screen.busy = true;
        screen.problem = None;
        cx.notify();
    });
    let call = store::request::<method::BotsCreate>(cx, create_input(&form));
    let rpc = store::rpc(cx);
    let screen = screen.clone();
    cx.spawn(async move |cx| {
        guarded_async("create an agent", async move {
            let bot = match call.await {
                Ok(bot) => bot,
                Err(error) => {
                    cx.update(|cx| {
                        screen.update(cx, |screen, cx| {
                            screen.problem = Some(error_text(&error).into());
                            screen.busy = false;
                            cx.notify();
                        })
                    });
                    return;
                }
            };
            // The agent goes into the store at once: the daemon's `bot.updated` may reach it after this answer, and
            // the agent's chat opens next.
            cx.update(|cx| {
                store::update(cx, |state| {
                    crate::model::state::apply_event(
                        state,
                        &protocol::wire::Event::BotUpdated(bot.clone()),
                    )
                })
            });
            let mut failed = Vec::new();
            for routine in &routines {
                if let Err(error) = save_routine(&rpc, &bot.id, None, routine).await {
                    failed.push(format!("{} ({})", routine.name, error_text(&error)));
                }
            }
            cx.update(|cx| {
                if failed.is_empty() {
                    cx.navigate(Route::Chat(bot.id.clone()));
                    return;
                }
                screen.update(cx, |screen, cx| {
                    screen.problem = Some(
                        format!(
                            "{} was created, but not these routines: {}.",
                            bot.name,
                            failed.join(", ")
                        )
                        .into(),
                    );
                    screen.created = Some(bot);
                    screen.busy = false;
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

/// The routine editor on a draft: Save keeps it in this screen's list (`at` is its place, `None` for a new one).
fn edit_draft(screen: &Entity<NewBot>, at: Option<usize>, window: &mut Window, cx: &mut App) {
    let initial = at
        .and_then(|at| screen.read(cx).routines.get(at).cloned())
        .unwrap_or_else(crate::model::routine_form::empty_routine);
    let (saving, deleting) = (screen.clone(), screen.clone());
    open_routine_editor(
        RoutineEditor {
            title: if at.is_some() {
                "Edit routine"
            } else {
                "New routine"
            },
            initial,
            time_zone: local_zone(),
            runs_for: None,
            on_save: Rc::new(move |draft, cx| {
                saving.update(cx, |screen, cx| {
                    match at.and_then(|at| screen.routines.get_mut(at)) {
                        Some(slot) => *slot = draft,
                        None => screen.routines.push(draft),
                    }
                    cx.notify();
                });
                futures::future::ready(Ok(())).boxed_local()
            }),
            on_delete: at.map(|at| -> crate::ui::routines::editor::Delete {
                let deleting = deleting.clone();
                Rc::new(move |cx| {
                    deleting.update(cx, |screen, cx| {
                        if at < screen.routines.len() {
                            screen.routines.remove(at);
                        }
                        cx.notify();
                    });
                    futures::future::ready(Ok(())).boxed_local()
                })
            }),
        },
        window,
        cx,
    );
}

impl Render for NewBot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bots = store::state(cx).bots.clone();
        let screen = cx.entity();
        let form = self.form.read(cx).form.clone();
        let name_error = (self.touched && !self.busy && self.created.is_none())
            .then(|| name_problem(&form.name, &bots, None))
            .flatten();
        let routines =
            {
                let rows: Vec<RoutineRow> = self
                    .routines
                    .iter()
                    .enumerate()
                    .map(|(at, draft)| RoutineRow {
                        key: at.to_string(),
                        name: draft.name.clone(),
                        time_limit_minutes: draft.time_limit_minutes,
                        days: draft.days.clone(),
                        times: draft.times.clone(),
                        enabled: draft.enabled,
                    })
                    .collect();
                let (opening, adding) = (screen.clone(), screen.clone());
                form::routines_row(
                    routines_summary(&self.routines),
                    move |window, cx| {
                        let open = Rc::new(move |key: &str, window: &mut Window, cx: &mut App| {
                            edit_draft(&opening, key.parse().ok(), window, cx)
                        });
                        div()
                            .child(routine_rows("new_bot.routines", &rows, open, window, cx))
                            .child(div().mt(r(8.)).child(
                                Link::new("new_bot.routines.add", "Add routine").on_press(
                                    move |window, cx| edit_draft(&adding, None, window, cx),
                                ),
                            ))
                            .into_any_element()
                    },
                    window,
                    cx,
                )
            };
        let fields = form::fields(&self.form, None, None, name_error, routines, window, cx);
        let suggestions = suggestions_aside(&screen, window, cx);
        let actions: Vec<AnyElement> = match &self.created {
            Some(bot) => {
                let chat = Route::Chat(bot.id.clone());
                vec![
                    Pill::new("new_bot.open", format!("Open {}", bot.name))
                        .variant(PillVariant::Primary)
                        .on_press(move |_, cx| cx.navigate(chat.clone()))
                        .into_any_element(),
                ]
            }
            None => {
                let creating = screen.clone();
                vec![
                    Pill::new("new_bot.cancel", "Cancel")
                        .on_press(|_, cx| cx.navigate(Route::Agents))
                        .into_any_element(),
                    div()
                        .opacity(if self.busy { 0.38 } else { 1.0 })
                        .child(
                            Pill::new("new_bot.create", "Create agent")
                                .variant(PillVariant::Primary)
                                .on_press(move |_, cx| create(&creating, cx)),
                        )
                        .into_any_element(),
                ]
            }
        };
        let failed = self
            .problem
            .clone()
            .map(|text| problem("new_bot.problem", text, cx));
        let scroll = scroll_handle("bot_form.scroll", window, cx);
        page(
            &scroll,
            header(
                "new_bot.back",
                chip(
                    BotAvatar::new(form.color, form.shape, 24.).into_any_element(),
                    div()
                        .semantic("screen.new", Role::Heading)
                        .aria_label("New agent")
                        .child("New agent")
                        .into_any_element(),
                    None,
                    cx,
                ),
                window,
                cx,
            ),
            fields,
            suggestions,
            footer(failed, actions, cx),
            cx,
        )
    }
}

/// Suggestions (WF3): each fills the form from a starting point.
fn suggestions_aside(screen: &Entity<NewBot>, window: &mut Window, cx: &mut App) -> AnyElement {
    let cards: Vec<AnyElement> = suggestions()
        .into_iter()
        .enumerate()
        .map(|(at, suggestion)| {
            let id: SharedString = format!("new_bot.suggestion.{at}").into();
            let focus = focus_handle(&id, window, cx);
            let ring = focus_visible(&focus, window);
            let theme = theme(cx);
            let colors = palette(cx);
            let card = div()
                .semantic(id, Role::Button)
                .aria_label(format!("{}: {}", suggestion.title, suggestion.description))
                .track_focus(&focus)
                .relative()
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .p(r(theme.space.s16))
                .rounded(r(theme.radius.card))
                .bg(hsla(colors.surface.raised))
                .border_t_1()
                .border_color(hsla(colors.overlay.highlight))
                .shadow(crate::ui::style::shadows(&theme.shadow.light.card))
                .cursor_pointer()
                .child(BotAvatar::new(
                    suggestion.form.color,
                    suggestion.form.shape,
                    40.,
                ))
                .child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .type_style(&theme.typography.desktop.ui_strong)
                                .text_color(hsla(colors.text.primary))
                                .child(suggestion.title),
                        )
                        .child(
                            div()
                                .type_style(&theme.typography.desktop.small)
                                .text_color(hsla(colors.text.secondary))
                                .child(suggestion.description),
                        ),
                );
            let card = if ring {
                card.child(focus_ring(theme.radius.card, cx))
            } else {
                card
            };
            let screen = screen.clone();
            pressable(
                card,
                Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                    screen.update(cx, |screen, cx| screen.apply(&suggestion, window, cx))
                })),
            )
            .into_any_element()
        })
        .collect();
    let theme = theme(cx);
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .type_style(&theme.typography.desktop.label)
                .text_color(hsla(palette(cx).text.tertiary))
                .child("Suggestions"),
        )
        .child(
            div()
                .mt(r(theme.space.s6))
                .flex()
                .flex_col()
                .gap(r(theme.space.s12))
                .children(cards),
        )
        .into_any_element()
}
