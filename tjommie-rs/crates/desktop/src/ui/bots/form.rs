//! The bot form (`bot-form/bot-form.tsx`, `identity.tsx`, `model-row.tsx`, `rows.tsx`; WF3, WF8): the avatar and
//! name, the colour and shape pickers, what the agent should do, and its rows (Account and model, Folder, Autonomy,
//! Browser, Can message, and the page's own Routines row). New agent and Agent settings share it.
//!
//! One difference from the TypeScript: Account and model opens in place, as the other rows do, instead of in a popover
//! (the same choices, with no overlay over the form).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Toggled, Window, div,
};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use protocol::model::{Bot, BotColor, BotShape, ModelInfo, Provider};
use protocol::wire::{ProviderParams, method};

use super::rows;
use crate::guard::{guarded, guarded_async};
use crate::model::accounts::{account_help, accounts_of, chosen_account, picker_state};
use crate::model::attachments::file_name;
use crate::model::bot_form::{BotForm, can_message_summary};
use crate::model::labels::{error_text, model_label};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Link, Look, RadioGroup, TextArea, TextField, label, problem};
use crate::ui::primitives::avatar::{BotAvatar, bot_color};
use crate::ui::primitives::button::pressable;
use crate::ui::primitives::focus_handle;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

pub const COLORS: [(BotColor, &str); 10] = [
    (BotColor::Brown, "brown"),
    (BotColor::Red, "red"),
    (BotColor::Orange, "orange"),
    (BotColor::Amber, "amber"),
    (BotColor::Green, "green"),
    (BotColor::Teal, "teal"),
    (BotColor::Blue, "blue"),
    (BotColor::Purple, "purple"),
    (BotColor::Pink, "pink"),
    (BotColor::Gray, "gray"),
];

pub const SHAPES: [(BotShape, &str, &str); 8] = [
    (BotShape::Circle, "circle", "circle"),
    (BotShape::Blob, "blob", "blob"),
    (BotShape::RoundedSquare, "rounded-square", "rounded square"),
    (BotShape::Pill, "pill", "pill"),
    (BotShape::Triangle, "triangle", "triangle"),
    (BotShape::Hexagon, "hexagon", "hexagon"),
    (BotShape::Cloud, "cloud", "cloud"),
    (BotShape::Droplet, "droplet", "droplet"),
];

/// The form's values, and the fields the text lives in while it is edited.
pub struct FormState {
    pub form: BotForm,
    pub name: Entity<InputState>,
    pub instructions: Entity<TextareaState>,
    /// Another PC's project folder, typed (its folders can't be browsed from here).
    pub folder: Entity<InputState>,
    /// The Account and model row's open tab, and each provider's models as read.
    tab: Provider,
    models: HashMap<Provider, Vec<ModelInfo>>,
    model_problems: HashMap<Provider, SharedString>,
    reading: HashSet<Provider>,
    _fields: Vec<Subscription>,
}

impl FormState {
    pub fn new(form: BotForm, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(form.name.clone()));
        let instructions =
            cx.new(|cx| TextareaState::new(window, cx).default_value(form.instructions.clone()));
        let folder = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(form.project_folder.clone().unwrap_or_default())
        });
        let fields = vec![
            cx.subscribe(&name, |this, field, event: &InputEvent, cx| {
                guarded("name an agent", || {
                    if matches!(event, InputEvent::Change) {
                        this.form.name = field.read(cx).value().to_string();
                        cx.notify();
                    }
                });
            }),
            cx.subscribe(&instructions, |this, field, event: &InputEvent, cx| {
                guarded("say what an agent does", || {
                    if matches!(event, InputEvent::Change) {
                        this.form.instructions = field.read(cx).value().to_string();
                        cx.notify();
                    }
                });
            }),
            cx.subscribe(&folder, |this, field, event: &InputEvent, cx| {
                guarded("type a project folder", || {
                    if matches!(event, InputEvent::Change) && this.form.project_folder.is_some() {
                        this.form.project_folder = Some(field.read(cx).value().to_string());
                        cx.notify();
                    }
                });
            }),
        ];
        FormState {
            tab: form.provider,
            form,
            name,
            instructions,
            folder,
            models: HashMap::new(),
            model_problems: HashMap::new(),
            reading: HashSet::new(),
            _fields: fields,
        }
    }

    /// Puts a whole form in (a suggestion): the fields show it too.
    pub fn replace(&mut self, form: BotForm, window: &mut Window, cx: &mut Context<Self>) {
        self.name.update(cx, |field, cx| {
            field.set_value(form.name.clone(), window, cx)
        });
        self.instructions.update(cx, |field, cx| {
            field.set_value(form.instructions.clone(), window, cx)
        });
        self.folder.update(cx, |field, cx| {
            field.set_value(form.project_folder.clone().unwrap_or_default(), window, cx)
        });
        self.tab = form.provider;
        self.form = form;
        cx.notify();
    }
}

/// What an existing agent has now, so Account and model can say a switch starts a fresh session.
#[derive(Clone)]
pub struct Saved {
    pub provider: Provider,
    pub account_id: Option<String>,
}

/// The form's column (`BotFormFields`): Identity, What should this agent do?, and the rows.
pub fn fields(
    state: &Entity<FormState>,
    self_id: Option<&str>,
    saved: Option<Saved>,
    name_error: Option<String>,
    routines_row: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let bots = store::state(cx).bots.clone();
    let identity = identity(state, name_error, window, cx);
    let model = model_row(state, saved, window, cx);
    let folder = folder_row(state, window, cx);
    let form = state.read(cx).form.clone();
    let autonomy = {
        let state = state.clone();
        rows::autonomy(
            "bot_form.autonomy",
            form.autonomy,
            form.provider,
            &form.name,
            move |autonomy, _, cx| {
                state.update(cx, |state, cx| {
                    state.form.autonomy = autonomy;
                    cx.notify();
                })
            },
            window,
            cx,
        )
    };
    let browser = {
        let state = state.clone();
        rows::browser(
            "bot_form.browser",
            form.browser,
            move |on, _, cx| {
                state.update(cx, |state, cx| {
                    state.form.browser = on;
                    cx.notify();
                })
            },
            window,
            cx,
        )
    };
    let others: Vec<Bot> = bots
        .iter()
        .filter(|bot| Some(bot.id.as_str()) != self_id)
        .cloned()
        .collect();
    let can_message = {
        let state = state.clone();
        rows::can_message(
            "bot_form.can_message",
            "Can message",
            can_message_summary(&form.can_message, &others),
            self_id,
            &form.can_message,
            &bots,
            false,
            move |ids, _, cx| {
                state.update(cx, |state, cx| {
                    state.form.can_message = ids;
                    cx.notify();
                })
            },
            "It can ask these agents for help. They can't ask it back unless you allow that in their own settings.",
            window,
            cx,
        )
    };
    let instructions = state.read(cx).instructions.clone();
    let theme = theme(cx);
    div()
        .flex()
        .flex_col()
        .child(identity)
        .child(
            div()
                .mt(r(theme.space.s24))
                .flex()
                .flex_col()
                .child(label("What should this agent do?", cx))
                .child(TextArea::new(
                    "bot_form.instructions",
                    "What should this agent do?",
                    &instructions,
                )),
        )
        .child(
            div()
                .mt(r(theme.space.s24))
                .flex()
                .flex_col()
                .gap(r(theme.space.s8))
                .child(model)
                .child(folder)
                .child(autonomy)
                .child(browser)
                .child(can_message)
                .child(routines_row),
        )
        .into_any_element()
}

/// The avatar, the name, and the colour and shape pickers (`Identity`): each picker one radio group, whose arrows
/// move the choice.
fn identity(
    state: &Entity<FormState>,
    name_error: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let _ = window;
    let (form, name) = {
        let state = state.read(cx);
        (state.form.clone(), state.name.clone())
    };
    let colors_group = {
        let state = state.clone();
        COLORS
            .iter()
            .fold(
                RadioGroup::new("bot_form.color", "Color", form.color).look(Look::Swatches),
                |group, (color, key)| {
                    let color = *color;
                    group.drawn(color, *key, *key, move |on, cx| swatch(color, on, cx))
                },
            )
            .on_choose(move |color, _, cx| {
                state.update(cx, |state, cx| {
                    state.form.color = color;
                    cx.notify();
                })
            })
    };
    let shapes_group = {
        let state = state.clone();
        let color = form.color;
        SHAPES
            .iter()
            .fold(
                RadioGroup::new("bot_form.shape", "Shape", form.shape).look(Look::Swatches),
                |group, (shape, key, label)| {
                    let shape = *shape;
                    group.drawn(shape, *key, *label, move |on, cx| {
                        shape_tile(color, shape, on, cx)
                    })
                },
            )
            .on_choose(move |shape, _, cx| {
                state.update(cx, |state, cx| {
                    state.form.shape = shape;
                    cx.notify();
                })
            })
    };
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .flex()
        .gap(r(theme.space.s24))
        .child(
            div()
                .size(r(156.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(r(theme.radius.card))
                .bg(hsla(colors.surface.raised))
                .child(BotAvatar::new(form.color, form.shape, 96.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(label("Name", cx))
                .child(TextField::new("bot_form.name", "Name", &name).bad(name_error.is_some()))
                .children(name_error.map(|error| problem("bot_form.name.problem", error, cx)))
                .child(div().mt(r(theme.space.s12)).child(colors_group))
                .child(div().mt(r(theme.space.s16)).child(shapes_group)),
        )
        .into_any_element()
}

fn swatch(color: BotColor, on: bool, cx: &App) -> AnyElement {
    let colors = palette(cx);
    div()
        .size(r(28.))
        .rounded_full()
        .bg(hsla(bot_color(color, cx)))
        .when(on, |swatch| {
            swatch
                .border_2()
                .border_color(hsla(colors.surface.raised))
                .shadow(vec![gpui::BoxShadow {
                    color: hsla(colors.text.primary),
                    offset: gpui::point(gpui::px(0.), gpui::px(0.)),
                    blur_radius: gpui::px(0.),
                    spread_radius: gpui::px(2.),
                    inset: false,
                }])
        })
        .into_any_element()
}

fn shape_tile(color: BotColor, shape: BotShape, on: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .size(r(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(r(theme.radius.field))
        .bg(hsla(if on {
            colors.state.hover
        } else {
            colors.surface.sunk
        }))
        .when(on, |tile| {
            tile.border_1().border_color(hsla(colors.accent.base))
        })
        .child(BotAvatar::new(color, shape, 24.))
        .into_any_element()
}

/// A choice in a list (`aria-pressed`): an account or a model in Account and model.
#[allow(clippy::too_many_arguments)]
fn choice(
    id: String,
    label: String,
    end: Option<String>,
    on: bool,
    enabled: bool,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let focus = focus_handle(&id, window, cx);
    let ring = focus_visible(&focus, window);
    let theme = theme(cx);
    let colors = palette(cx);
    let button = div()
        .semantic(id, Role::Button)
        .aria_label(label.clone())
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .track_focus(&focus)
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .gap(r(theme.space.s8))
        .h(r(36.))
        .px(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .cursor_pointer()
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(if on {
            colors.text.primary
        } else {
            colors.text.secondary
        }))
        .when(on, |button| button.bg(hsla(colors.state.hover)))
        .hover(|style| style.bg(hsla(colors.state.hover)))
        .when(!enabled, |button| button.opacity(0.5))
        .child(div().min_w_0().truncate().child(label))
        .children(end.map(|end| {
            div()
                .flex_none()
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.tertiary))
                .child(end)
        }))
        .when(ring, |button| {
            button.child(focus_ring(theme.radius.field, cx))
        });
    let press: Option<crate::ui::primitives::button::Handler> =
        enabled.then(|| Rc::new(on_press) as crate::ui::primitives::button::Handler);
    pressable(button, press).into_any_element()
}

/// Account and model (`ModelRow`, WF11): Claude or Codex, that provider's accounts and models, and a caption; under the
/// row, what the account in use means for this agent. Each provider's models are read once, when the row first shows
/// that provider.
fn model_row(
    state: &Entity<FormState>,
    saved: Option<Saved>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    const ID: &str = "bot_form.model";
    let open = rows::is_open(ID, false, window, cx);
    let accounts = store::state(cx).accounts.clone();
    let now = jiff::Zoned::now();
    let (form, tab) = {
        let state = state.read(cx);
        (state.form.clone(), state.tab)
    };
    if open {
        read_models(state, tab, cx);
    }
    let list = accounts.clone().unwrap_or_default();
    let current = chosen_account(&list, form.provider, form.account_id.as_deref()).cloned();
    let was = saved
        .as_ref()
        .and_then(|saved| chosen_account(&list, saved.provider, saved.account_id.as_deref()))
        .map(|account| account.id.clone());
    let changed = saved.is_some() && current.as_ref().map(|account| account.id.clone()) != was;
    let help = accounts
        .as_ref()
        .and_then(|_| account_help(current.as_ref(), &now, changed));
    let value = [
        current.as_ref().map(|account| account.label.clone()),
        Some(model_label(form.provider, &form.model)),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let state = state.clone();
    let row = rows::row(
        ID,
        "sparkles",
        "Account and model",
        Some(div().truncate().child(value).into_any_element()),
        false,
        move |window, cx| model_controls(&state, accounts.as_deref(), &now, window, cx),
        window,
        cx,
    );
    let theme = theme(cx);
    div()
        .flex()
        .flex_col()
        .child(row)
        .children(help.map(|help| {
            div()
                .semantic("bot_form.model.help", Role::Paragraph)
                .aria_label(help.clone())
                .px(r(theme.space.s4))
                .pt(r(theme.space.s4))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(palette(cx).text.secondary))
                .child(help)
        }))
        .into_any_element()
}

fn read_models(state: &Entity<FormState>, tab: Provider, cx: &mut App) {
    {
        let state = state.read(cx);
        if state.models.contains_key(&tab)
            || state.model_problems.contains_key(&tab)
            || state.reading.contains(&tab)
        {
            return;
        }
    }
    state.update(cx, |state, _| {
        state.reading.insert(tab);
    });
    let call = store::request::<method::ModelsList>(cx, ProviderParams { provider: tab });
    let state = state.clone();
    cx.spawn(async move |cx| {
        guarded_async("read a provider's models", async move {
            let read = call.await;
            cx.update(|cx| {
                state.update(cx, |state, cx| {
                    state.reading.remove(&tab);
                    match read {
                        Ok(models) => {
                            state.models.insert(tab, models);
                        }
                        Err(error) => {
                            state.model_problems.insert(tab, error_text(&error).into());
                        }
                    }
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

fn model_controls(
    state: &Entity<FormState>,
    accounts: Option<&[protocol::accounts::AccountInfo]>,
    now: &jiff::Zoned,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (form, tab, models, failed) = {
        let state = state.read(cx);
        (
            state.form.clone(),
            state.tab,
            state.models.get(&state.tab).cloned(),
            state.model_problems.get(&state.tab).cloned(),
        )
    };
    let list = accounts.unwrap_or(&[]);
    let here: Vec<protocol::accounts::AccountInfo> =
        accounts_of(list, tab).into_iter().cloned().collect();
    let in_tab = chosen_account(
        list,
        tab,
        if form.provider == tab {
            form.account_id.as_deref()
        } else {
            None
        },
    )
    .map(|account| account.id.clone());
    let default_model = models.as_ref().and_then(|models| {
        models
            .iter()
            .find(|model| model.is_default)
            .or_else(|| models.first())
            .map(|model| model.id.clone())
    });
    let tabs = {
        let state = state.clone();
        RadioGroup::new("bot_form.model.provider", "Provider", tab)
            .choice(Provider::Claude, "claude", "Claude")
            .choice(Provider::Codex, "codex", "Codex")
            .on_choose(move |next, _, cx| {
                state.update(cx, |state, cx| {
                    state.tab = next;
                    state.model_problems.clear();
                    cx.notify();
                })
            })
    };
    let account_rows: AnyElement = match accounts {
        None => note("Loading the accounts", cx),
        Some(_) if here.is_empty() => note(
            if tab == Provider::Codex {
                "No Codex accounts. Add one in Settings → Accounts."
            } else {
                "No Claude accounts. Add one in Settings → Accounts."
            },
            cx,
        ),
        Some(_) => div()
            .flex()
            .flex_col()
            .gap(r(theme(cx).space.s4))
            .children(here.iter().map(|account| {
                let selected = in_tab.as_deref() == Some(account.id.as_str());
                let enabled = !(tab != form.provider && default_model.is_none());
                let (state, id, model) = (state.clone(), account.id.clone(), default_model.clone());
                let provider = form.provider;
                let current_model = form.model.clone();
                choice(
                    format!("bot_form.model.account.{}", account.id),
                    account.label.clone(),
                    Some(picker_state(account, now)),
                    selected,
                    enabled,
                    move |_, cx| {
                        state.update(cx, |state, cx| {
                            state.form.model = if tab == provider {
                                current_model.clone()
                            } else {
                                model.clone().unwrap_or_else(|| current_model.clone())
                            };
                            state.form.provider = tab;
                            state.form.account_id = Some(id.clone());
                            cx.notify();
                        })
                    },
                    window,
                    cx,
                )
            }))
            .into_any_element(),
    };
    let model_rows: AnyElement = match (&failed, &models) {
        (Some(failed), _) => problem("bot_form.model.problem", failed.clone(), cx),
        (None, None) => note("Loading the models", cx),
        (None, Some(models)) => div()
            .flex()
            .flex_col()
            .gap(r(theme(cx).space.s4))
            .children(models.iter().map(|info| {
                let selected = form.provider == tab && form.model == info.id;
                let shown = if tab == Provider::Claude {
                    info.label.trim_start_matches("Claude ").to_owned()
                } else {
                    info.label.clone()
                };
                let (state, id, in_tab) = (state.clone(), info.id.clone(), in_tab.clone());
                let (provider, account) = (form.provider, form.account_id.clone());
                choice(
                    format!("bot_form.model.model.{}", info.id),
                    shown,
                    None,
                    selected,
                    true,
                    move |_, cx| {
                        state.update(cx, |state, cx| {
                            state.form.account_id = if provider == tab {
                                account.clone()
                            } else {
                                in_tab.clone()
                            };
                            state.form.provider = tab;
                            state.form.model = id.clone();
                            cx.notify();
                        })
                    },
                    window,
                    cx,
                )
            }))
            .into_any_element(),
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let heading = |text: &'static str| {
        div()
            .pt(r(theme.space.s12))
            .pb(r(theme.space.s4))
            .type_style(&theme.typography.desktop.label)
            .text_color(hsla(colors.text.tertiary))
            .child(text)
    };
    div()
        .flex()
        .flex_col()
        .child(tabs)
        .child(heading("Account"))
        .child(account_rows)
        .child(heading("Model"))
        .child(model_rows)
        .child(
            div()
                .pt(r(theme.space.s8))
                .type_style(&theme.typography.desktop.meta)
                .text_color(hsla(colors.text.tertiary))
                .child(if tab == Provider::Claude {
                    "Latest versions are picked automatically."
                } else {
                    "Models from Codex's own list."
                }),
        )
        .into_any_element()
}

fn note(text: &'static str, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .py(r(theme.space.s8))
        .type_style(&theme.typography.desktop.ui)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text)
        .into_any_element()
}

/// Folder (`FolderRow`): its own folder, or a project folder, chosen with the folder picker on this PC, typed for
/// another PC (whose folders can't be browsed from here).
fn folder_row(state: &Entity<FormState>, window: &mut Window, cx: &mut App) -> AnyElement {
    let project = state.read(cx).form.project_folder.clone();
    let remote = store::state(cx).host.is_some();
    let value = match &project {
        None => "Its own folder".to_owned(),
        Some(folder) => match file_name(folder) {
            "" => "A project folder".to_owned(),
            name => name.to_owned(),
        },
    };
    let state = state.clone();
    rows::row(
        "bot_form.folder",
        "folder",
        "Folder",
        Some(div().truncate().child(value).into_any_element()),
        false,
        move |_window, cx| {
            let pick = {
                let state = state.clone();
                RadioGroup::new("bot_form.folder.kind", "Folder", project.is_some())
                    .choice(false, "own", "Its own folder")
                    .choice(true, "project", "A project folder")
                    .on_choose(move |project, window, cx| {
                        if !project {
                            state.update(cx, |state, cx| {
                                state.form.project_folder = None;
                                cx.notify();
                            });
                        } else if remote {
                            state.update(cx, |state, cx| {
                                let typed = state.folder.read(cx).value().to_string();
                                state.form.project_folder = Some(typed);
                                cx.notify();
                            });
                        } else {
                            choose_folder(&state, window, cx);
                        }
                    })
            };
            let theme = theme(cx);
            let colors = palette(cx);
            let path: Option<AnyElement> = project.as_ref().map(|folder| {
                if remote {
                    let field = state.read(cx).folder.clone();
                    div()
                        .mt(r(theme.space.s12))
                        .child(TextField::new(
                            "bot_form.folder.path",
                            "Project folder path",
                            &field,
                        ))
                        .child(rows::help(
                            "A folder on the other PC. Type its full path.",
                            cx,
                        ))
                        .into_any_element()
                } else {
                    let state = state.clone();
                    div()
                        .mt(r(theme.space.s12))
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s8))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child(div().min_w_0().truncate().child(folder.clone()))
                        .child(
                            Link::new("bot_form.folder.change", "Change")
                                .on_press(move |window, cx| choose_folder(&state, window, cx)),
                        )
                        .into_any_element()
                }
            });
            div()
                .child(pick)
                .children(path)
                .child(rows::help(
                    if project.is_some() {
                        "It works in the project folder, and keeps its notes and inbox in its own folder."
                    } else {
                        "It works in its own folder, which Tjommie makes for it."
                    },
                    cx,
                ))
                .into_any_element()
        },
        window,
        cx,
    )
}

/// This PC's folder picker; a folder chosen becomes the project folder.
fn choose_folder(state: &Entity<FormState>, window: &mut Window, cx: &mut App) {
    let _ = window;
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        guarded_async("choose a project folder", async move {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(folder) = paths.first()
            {
                let folder = folder.to_string_lossy().into_owned();
                cx.update(|cx| {
                    state.update(cx, |state, cx| {
                        state.form.project_folder = Some(folder);
                        cx.notify();
                    })
                });
            }
        })
        .await;
    })
    .detach();
}

/// Routines (`RoutinesRow`): the summary, and the page's rows with Add routine.
pub fn routines_row(
    summary: String,
    controls: impl FnOnce(&mut Window, &mut App) -> AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    rows::row(
        "bot_form.routines",
        "calendar-clock",
        "Routines",
        Some(div().truncate().child(summary).into_any_element()),
        false,
        controls,
        window,
        cx,
    )
}
