//! The team and task editors (`project-editor.tsx`, `task-editor.tsx`): dialogs over the Projects screen. Saving creates,
//! or updates at the revision the editor opened with, and starts no work; the screen then shows what was saved.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, svg,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputState, TextareaState};
use protocol::projects::{
    Project, ProjectCheck, ProjectInput, ProjectMember, ProjectPatch, ProjectRepository,
    ProjectRole, ProjectTask, ProjectsUpdate, RepositoryPath, TaskInput, TaskPatch, TasksUpdate,
};
use protocol::wire::method;

use super::ProjectsScreen;
use crate::guard::{guarded, guarded_async};
use crate::model::connection::RequestError;
use crate::model::labels::error_text;
use crate::semantic::{SemanticDisabled as _, SemanticExt as _};
use crate::ui::fields::{TextArea, TextField, label, problem};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{Pill, PillVariant};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::primitives::scroll::{follow_focus, scroll_handle};
use crate::ui::primitives::toggle::Toggle;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// The roles, as `ProjectRole.options` lists them, with their names in ids and labels.
const ROLES: [(ProjectRole, &str, &str); 4] = [
    (ProjectRole::Lead, "lead", "Lead"),
    (ProjectRole::Developer, "developer", "Developer"),
    (ProjectRole::Reviewer, "reviewer", "Reviewer"),
    (ProjectRole::Tester, "tester", "Tester"),
];

/// A line of secondary text.
fn note(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    div()
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text.into())
        .into_any_element()
}

/// A picker's button (`RowHeader` in a `Menu.Trigger`): its title, the choice and a chevron; dimmed when it can't
/// open. The choice reads as `<id>.value`.
#[allow(clippy::too_many_arguments)]
fn trigger(
    id: &str,
    heading: &'static str,
    value: String,
    open: bool,
    enabled: bool,
    focus: &FocusHandle,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    // Unavailable as a Pill is (M5 review M2): not a Tab stop, and said so.
    let focus = focus.clone().tab_stop(enabled);
    let ring = focus_visible(&focus, window);
    div()
        .semantic(SharedString::from(id.to_owned()), Role::Button)
        .aria_label(if enabled {
            SharedString::from(heading)
        } else {
            SharedString::from(format!("{heading}, unavailable"))
        })
        .semantic_disabled(!enabled)
        .aria_expanded(open)
        .track_focus(&focus)
        .relative()
        .h(r(44.))
        .flex()
        .items_center()
        .gap(r(theme.space.s10))
        .px(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .border_1()
        .border_color(hsla(colors.line.field))
        .bg(hsla(colors.surface.raised))
        .opacity(if enabled { 1.0 } else { 0.38 })
        .when(enabled, |button| button.cursor_pointer())
        .child(
            div()
                .flex_none()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.secondary))
                .child(heading),
        )
        .child(
            div()
                .semantic(format!("{id}.value"), Role::Paragraph)
                .aria_label(value.clone())
                .flex_1()
                .min_w_0()
                .truncate()
                .type_style(&theme.typography.desktop.ui_strong)
                .text_color(hsla(colors.text.primary))
                .child(value),
        )
        .child(
            svg()
                .path("icons/i/175/chevron-down.svg")
                .size(r(theme.size.icon))
                .flex_none()
                .text_color(hsla(colors.icon.base)),
        )
        .when(ring, |button| {
            button.child(focus_ring(theme.radius.field, cx))
        })
        .into_any_element()
}

/// A picker: the button, and its menu of choices while it can open. `choices` are (value, label).
#[allow(clippy::too_many_arguments)]
fn picker(
    id: &'static str,
    heading: &'static str,
    value: String,
    enabled: bool,
    current: String,
    choices: Vec<(String, String)>,
    on_choose: impl Fn(String, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let focus = focus_handle(id, window, cx);
    if !enabled {
        return trigger(id, heading, value, false, false, &focus, window, cx);
    }
    let shown = value.clone();
    choices
        .into_iter()
        .fold(
            Menu::new(
                SharedString::from(format!("{id}.menu")),
                move |open, window, cx| trigger(id, heading, shown, open, true, &focus, window, cx),
            )
            .anchor(gpui::Anchor::TopLeft)
            .offset(px(6.)),
            |menu, (choice, text)| {
                let (on_choose, picked) = (on_choose.clone(), choice.clone());
                let key = if choice.is_empty() {
                    "none".to_owned()
                } else {
                    choice.clone()
                };
                menu.item(
                    MenuItem::new(format!("{id}.{key}"), text, move |_, cx| {
                        on_choose(picked.clone(), cx)
                    })
                    .checked(choice == current),
                )
            },
        )
        .into_any_element()
}

/// The dialog's footer: a problem, then Cancel and the save button (off until it can save).
fn footer(
    prefix: &'static str,
    saving: &'static str,
    busy: bool,
    ready: bool,
    failed: Option<SharedString>,
    on_save: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let save = Pill::new(
        format!("{prefix}.save"),
        if busy { "Saving…" } else { saving },
    )
    .variant(PillVariant::Primary)
    .disabled(!ready)
    .on_press(on_save);
    div()
        .flex()
        .flex_col()
        .gap(r(theme.space.s12))
        .children(failed.map(|text| problem(format!("{prefix}.problem"), text, cx)))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(r(theme.space.s8))
                .child(
                    Pill::new(format!("{prefix}.cancel"), "Cancel")
                        .on_press(|window, cx| window.close_dialog(cx)),
                )
                .child(save),
        )
        .into_any_element()
}

/// A labelled field.
fn field(heading: &'static str, control: impl IntoElement, cx: &App) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(label(heading, cx))
        .child(control)
        .into_any_element()
}

// The team editor.

struct ProjectForm {
    opened: Option<Project>,
    name: Entity<InputState>,
    base: Entity<InputState>,
    target: Entity<InputState>,
    checks: Entity<TextareaState>,
    repositories: Vec<ProjectRepository>,
    repository_id: String,
    members: Vec<ProjectMember>,
    /// Each member's role instructions.
    notes: HashMap<String, Entity<TextareaState>>,
    busy: bool,
    problem: Option<SharedString>,
    window: AnyWindowHandle,
    /// The dialog in front when Save was pressed (`dialog::opened`): a late answer closes only that one.
    dialog: u64,
}

impl ProjectForm {
    /// A role switch: a member gains or loses `role`; one with no roles left is no member (`role`).
    fn role(
        &mut self,
        bot_id: &str,
        role: ProjectRole,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut member = self
            .members
            .iter()
            .find(|member| member.bot_id == bot_id)
            .cloned()
            .unwrap_or_else(|| ProjectMember {
                bot_id: bot_id.to_owned(),
                roles: Vec::new(),
                instructions: String::new(),
            });
        if on {
            if !member.roles.contains(&role) {
                member.roles.push(role);
            }
        } else {
            member.roles.retain(|held| *held != role);
        }
        self.members.retain(|other| other.bot_id != bot_id);
        if member.roles.is_empty() {
            self.notes.remove(bot_id);
        } else {
            self.notes
                .entry(bot_id.to_owned())
                .or_insert_with(|| cx.new(|cx| TextareaState::new(window, cx)));
            self.members.push(member);
        }
        cx.notify();
    }

    fn ready(&self, cx: &App) -> bool {
        !self.busy
            && !self.name.read(cx).value().trim().is_empty()
            && !self.repository_id.is_empty()
            && !self.members.is_empty()
    }
}

pub(super) fn open_project_editor(
    screen: Entity<ProjectsScreen>,
    project: Option<Project>,
    window: &mut Window,
    cx: &mut App,
) {
    let repositories = screen.read(cx).repositories.clone();
    let input = project.as_ref().map(|project| project.input.clone());
    let text = |value: Option<&str>, fallback: &str| value.unwrap_or(fallback).to_owned();
    let field = |value: String, window: &mut Window, cx: &mut App| {
        cx.new(|cx| InputState::new(window, cx).default_value(value))
    };
    let name = field(
        text(input.as_ref().map(|input| input.name.as_str()), ""),
        window,
        cx,
    );
    let base = field(
        text(
            input.as_ref().map(|input| input.base_ref.as_str()),
            "refs/heads/main",
        ),
        window,
        cx,
    );
    let target = field(
        text(
            input.as_ref().map(|input| input.target_ref.as_str()),
            "refs/heads/main",
        ),
        window,
        cx,
    );
    let checks_text = serde_json::to_string_pretty(
        &input
            .as_ref()
            .map(|input| input.checks.clone())
            .unwrap_or_default(),
    )
    .unwrap_or_else(|_| "[]".to_owned());
    let checks = cx.new(|cx| TextareaState::new(window, cx).default_value(checks_text));
    let members = input
        .as_ref()
        .map(|input| input.members.clone())
        .unwrap_or_default();
    let notes = members
        .iter()
        .map(|member| {
            let said = member.instructions.clone();
            (
                member.bot_id.clone(),
                cx.new(|cx| TextareaState::new(window, cx).default_value(said)),
            )
        })
        .collect();
    let repository_id = input
        .as_ref()
        .map(|input| input.repository_id.clone())
        .or_else(|| repositories.first().map(|repository| repository.id.clone()))
        .unwrap_or_default();
    let form = cx.new(|_| ProjectForm {
        opened: project,
        name: name.clone(),
        base,
        target,
        checks,
        repositories,
        repository_id,
        members,
        notes,
        busy: false,
        problem: None,
        window: window.window_handle(),
        dialog: 0,
    });
    open_dialog(window, cx, move |dialog, window, cx| {
        let heading = if form.read(cx).opened.is_some() {
            "Edit project team"
        } else {
            "New project team"
        };
        let body = project_body(&form, &screen, window, cx);
        // project-editor.tsx's 600 px, as rems.
        dialog
            .w(window.rem_size() * 37.5)
            .title(title(heading, cx))
            // Enter in a field never closes the form through the dialog's Confirm.
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(body)
    });
    window.defer(cx, move |window, cx| {
        guarded("focus the team's name", || {
            name.update(cx, |name, cx| name.focus(window, cx))
        });
    });
}

fn project_body(
    form: &Entity<ProjectForm>,
    screen: &Entity<ProjectsScreen>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let bots = store::state(cx).bots.clone();
    let state = form.read(cx);
    let editing = state.opened.is_some();
    let ready = state.ready(cx);
    let busy = state.busy;
    let failed = state.problem.clone();
    let (name, base, target, checks) = (
        state.name.clone(),
        state.base.clone(),
        state.target.clone(),
        state.checks.clone(),
    );
    let repository = state
        .repositories
        .iter()
        .find(|repository| repository.id == state.repository_id)
        .map_or_else(
            || "Choose a registered repository".to_owned(),
            |repository| repository.name.clone(),
        );
    let can_pick = !editing && !busy && !state.repositories.is_empty();
    let choices: Vec<(String, String)> = state
        .repositories
        .iter()
        .map(|repository| (repository.id.clone(), repository.name.clone()))
        .collect();
    let current = state.repository_id.clone();
    let members = state.members.clone();
    let notes = state.notes.clone();

    let picking = form.clone();
    let repositories = picker(
        "project_form.repository",
        "Repository",
        repository,
        can_pick,
        current,
        choices,
        move |chosen, cx| {
            picking.update(cx, |form, cx| {
                form.repository_id = chosen;
                cx.notify();
            })
        },
        window,
        cx,
    );
    let register: AnyElement = if editing {
        note(
            "Repositories are registered on the host PC. Saved downloads and folders on this device are separate.",
            cx,
        )
    } else {
        let registering = form.clone();
        Pill::new("project_form.register", "Choose a repository folder")
            .disabled(busy)
            .on_press(move |_, cx| register_repository(&registering, cx))
            .into_any_element()
    };
    let agents: Vec<AnyElement> = bots
        .iter()
        .map(|bot| {
            let member = members.iter().find(|member| member.bot_id == bot.id);
            let switches: Vec<AnyElement> = ROLES
                .iter()
                .map(|(role, key, shown)| {
                    let on = member.is_some_and(|member| member.roles.contains(role));
                    let (switching, bot_id, role) = (form.clone(), bot.id.clone(), *role);
                    let theme = theme(cx);
                    div()
                        .min_h(r(44.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(r(theme.space.s12))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(palette(cx).text.secondary))
                        .child(*shown)
                        .child(
                            Toggle::new(
                                format!("project_form.role.{}.{key}", bot.id),
                                format!("{}: {key}", bot.name),
                                on,
                            )
                            .on_toggle(move |on, window, cx| {
                                switching
                                    .update(cx, |form, cx| form.role(&bot_id, role, on, window, cx))
                            }),
                        )
                        .into_any_element()
                })
                .collect();
            let instructions = member
                .and_then(|member| notes.get(&member.bot_id))
                .map(|state| {
                    TextArea::new(
                        format!("project_form.instructions.{}", bot.id),
                        format!("{} role instructions", bot.name),
                        state,
                    )
                });
            let theme = theme(cx);
            let colors = palette(cx);
            div()
                .p(r(theme.space.s12))
                .rounded(r(theme.radius.field))
                .border_1()
                .border_color(hsla(colors.line.base))
                .flex()
                .flex_col()
                .gap(r(theme.space.s8))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s12))
                        .child(BotAvatar::new(bot.color, bot.shape, 24.))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .type_style(&theme.typography.desktop.ui)
                                .text_color(hsla(colors.text.primary))
                                .child(bot.name.clone()),
                        ),
                )
                .child(
                    div()
                        .grid()
                        .grid_cols(2)
                        .gap_x(r(theme.space.s16))
                        .children(switches),
                )
                .children(instructions)
                .into_any_element()
        })
        .collect();
    let (saving, screen) = (form.clone(), screen.clone());
    let footer = footer(
        "project_form",
        "Save team",
        busy,
        ready,
        failed,
        move |_, cx| save_project(&saving, &screen, cx),
        cx,
    );
    let scroll = scroll_handle("project_form.scroll", window, cx);
    let theme = theme(cx);
    // The fields scroll; Cancel and Save stay in view under them.
    let fields = div()
        .id("project_form.scroll")
        .track_scroll(&scroll)
        .max_h(window.viewport_size().height * 0.6)
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(r(theme.space.s16))
        .child(field("Project name", TextField::new("project_form.name", "Project name", &name), cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(r(theme.space.s8))
                .child(field("Repository on this host", repositories, cx))
                .child(register),
        )
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap(r(theme.space.s12))
                .child(field("Base branch ref", TextField::new("project_form.base", "Base branch ref", &base), cx))
                .child(field(
                    "Integration target ref",
                    TextField::new("project_form.target", "Integration target ref", &target),
                    cx,
                )),
        )
        .child(
            div()
                .semantic("project_form.agents", Role::Group)
                .aria_label("Agents and roles")
                .flex()
                .flex_col()
                .gap(r(theme.space.s12))
                .child(label("Agents and roles", cx))
                .child(note(
                    "One active project per agent. Existing messaging, account and autonomy choices still apply.",
                    cx,
                ))
                .children(agents),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(r(theme.space.s8))
                .child(field(
                    "Accepted checks (JSON array)",
                    TextArea::new("project_form.checks", "Accepted checks (JSON array)", &checks),
                    cx,
                ))
                .child(note(
                    "Each check needs id, label, command and args. Checks run project code only after Evaluate or \
                     Integrate. Full auto permits host execution; other autonomy levels need an available isolated \
                     executor. Saving does not run them.",
                    cx,
                ))
                .child(note(
                    r#"Example: [{"id":"test","label":"Tests","command":"node","args":["--test"],"timeoutMs":120000}]"#,
                    cx,
                )),
        );
    div()
        .mt(r(theme.space.s16))
        .flex()
        .flex_col()
        .gap(r(theme.space.s16))
        .child(follow_focus(&scroll, fields))
        .child(footer)
        .into_any_element()
}

/// Choose a repository folder: this PC's folder picker, then the host registers it, and it is chosen.
fn register_repository(form: &Entity<ProjectForm>, cx: &mut App) {
    let picked = crate::ui::prompt_for_paths(
        PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        },
        cx,
    );
    form.update(cx, |form, cx| {
        form.busy = true;
        form.problem = None;
        cx.notify();
    });
    let form = form.clone();
    cx.spawn(async move |cx| {
        guarded_async("register a repository", async move {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths
                    .first()
                    .map(|path| path.to_string_lossy().into_owned()),
                _ => None,
            };
            let Some(path) = path else {
                cx.update(|cx| {
                    form.update(cx, |form, cx| {
                        form.busy = false;
                        cx.notify();
                    })
                });
                return;
            };
            let call = cx.update(|cx| {
                store::request::<method::ProjectsRegisterRepository>(cx, RepositoryPath { path })
            });
            let saved = call.await;
            cx.update(|cx| {
                form.update(cx, |form, cx| {
                    form.busy = false;
                    match saved {
                        Ok(saved) => {
                            form.repositories
                                .retain(|repository| repository.id != saved.id);
                            form.repository_id = saved.id.clone();
                            form.repositories.push(saved);
                        }
                        Err(error) => form.problem = Some(error_text(&error).into()),
                    }
                    cx.notify();
                })
            });
        })
        .await;
    })
    .detach();
}

fn save_project(form: &Entity<ProjectForm>, screen: &Entity<ProjectsScreen>, cx: &mut App) {
    let state = form.read(cx);
    if !state.ready(cx) {
        return;
    }
    let name = state.name.read(cx).value().trim().to_owned();
    let base_ref = state.base.read(cx).value().trim().to_owned();
    let target_ref = state.target.read(cx).value().trim().to_owned();
    let members: Vec<ProjectMember> = state
        .members
        .iter()
        .map(|member| ProjectMember {
            instructions: state.notes.get(&member.bot_id).map_or_else(
                || member.instructions.clone(),
                |note| note.read(cx).value().to_string(),
            ),
            ..member.clone()
        })
        .collect();
    let checks = match serde_json::from_str::<Vec<ProjectCheck>>(&state.checks.read(cx).value()) {
        Ok(checks) if checks.len() <= 20 => Ok(checks),
        Ok(_) => Err("Accepted checks can hold at most 20 checks.".to_owned()),
        Err(error) => Err(format!(
            "Accepted checks must be a JSON array of checks: {error}"
        )),
    };
    let (opened, repository_id) = (state.opened.clone(), state.repository_id.clone());
    let checks = match checks {
        Ok(checks) => checks,
        Err(said) => {
            form.update(cx, |form, cx| {
                form.problem = Some(said.into());
                cx.notify();
            });
            return;
        }
    };
    form.update(cx, |form, cx| {
        form.busy = true;
        form.problem = None;
        form.dialog = crate::ui::primitives::dialog::opened(cx);
        cx.notify();
    });
    let call = match &opened {
        Some(project) => store::request::<method::ProjectsUpdate>(
            cx,
            ProjectsUpdate {
                id: project.id.clone(),
                expected_revision: project.revision,
                patch: ProjectPatch {
                    name: Some(name),
                    base_ref: Some(base_ref),
                    target_ref: Some(target_ref),
                    room_id: None,
                    members: Some(members),
                    checks: Some(checks),
                },
            },
        ),
        None => store::request::<method::ProjectsCreate>(
            cx,
            ProjectInput {
                name,
                repository_id,
                base_ref,
                target_ref,
                room_id: None,
                members,
                checks,
            },
        ),
    };
    let (form, screen) = (form.clone(), screen.clone());
    cx.spawn(async move |cx| {
        guarded_async("save a project team", async move {
            let saved = call.await;
            cx.update(|cx| {
                let opened = (form.read(cx).window, form.read(cx).dialog);
                finish(
                    opened,
                    saved,
                    cx,
                    move |saved: Project, _, cx| {
                        screen.update(cx, |this, cx| this.select_project(saved.id, cx))
                    },
                    move |said, cx| {
                        form.update(cx, |form, cx| {
                            form.busy = false;
                            form.problem = Some(said);
                            cx.notify();
                        })
                    },
                )
            });
        })
        .await;
    })
    .detach();
}

/// After a save: close the dialog and show what was saved, or say why it failed. A save that ends after the dialog
/// was closed does neither.
fn finish<T>(
    (window, dialog): (AnyWindowHandle, u64),
    saved: Result<T, RequestError>,
    cx: &mut App,
    on_saved: impl FnOnce(T, &mut Window, &mut App),
    on_failed: impl FnOnce(SharedString, &mut App),
) {
    let _ = window.update(cx, |_, window, cx| {
        // Closed meanwhile, or another dialog opened since (M5 review Minor 4).
        if !window.has_active_dialog(cx) || crate::ui::primitives::dialog::opened(cx) != dialog {
            return;
        }
        match saved {
            Ok(saved) => {
                window.close_dialog(cx);
                on_saved(saved, window, cx);
            }
            Err(error) => on_failed(error_text(&error).into(), cx),
        }
    });
}

// The task editor.

struct TaskForm {
    project: Project,
    opened: Option<ProjectTask>,
    title: Entity<InputState>,
    instructions: Entity<TextareaState>,
    acceptance: Entity<TextareaState>,
    developer: String,
    reviewer: String,
    tester: String,
    busy: bool,
    problem: Option<SharedString>,
    window: AnyWindowHandle,
    /// The dialog in front when Save was pressed (`dialog::opened`): a late answer closes only that one.
    dialog: u64,
}

impl TaskForm {
    fn chosen(&self, role: ProjectRole) -> &str {
        match role {
            ProjectRole::Developer => &self.developer,
            ProjectRole::Reviewer => &self.reviewer,
            ProjectRole::Tester | ProjectRole::Lead => &self.tester,
        }
    }

    fn choose(&mut self, role: ProjectRole, id: String) {
        match role {
            ProjectRole::Developer => self.developer = id,
            ProjectRole::Reviewer => self.reviewer = id,
            ProjectRole::Tester | ProjectRole::Lead => self.tester = id,
        }
    }

    fn ready(&self, cx: &App) -> bool {
        !self.busy
            && !self.title.read(cx).value().trim().is_empty()
            && !self.instructions.read(cx).value().trim().is_empty()
            && !self.developer.is_empty()
    }
}

/// The first member with `role`: the picker's choice for a new task.
fn first_with(project: &Project, role: ProjectRole) -> String {
    project
        .input
        .members
        .iter()
        .find(|member| member.roles.contains(&role))
        .map(|member| member.bot_id.clone())
        .unwrap_or_default()
}

pub(super) fn open_task_editor(
    screen: Entity<ProjectsScreen>,
    project: Project,
    task: Option<ProjectTask>,
    window: &mut Window,
    cx: &mut App,
) {
    let input = task.as_ref().map(|task| task.input.clone());
    let title_text = input
        .as_ref()
        .map(|input| input.title.clone())
        .unwrap_or_default();
    let instructions_text = input
        .as_ref()
        .map(|input| input.instructions.clone())
        .unwrap_or_default();
    let acceptance_text = input
        .as_ref()
        .map(|input| input.acceptance.join("\n"))
        .unwrap_or_default();
    let title_field = cx.new(|cx| InputState::new(window, cx).default_value(title_text));
    let instructions = cx.new(|cx| TextareaState::new(window, cx).default_value(instructions_text));
    let acceptance = cx.new(|cx| TextareaState::new(window, cx).default_value(acceptance_text));
    let developer = input
        .as_ref()
        .map(|input| input.assignee_id.clone())
        .unwrap_or_else(|| first_with(&project, ProjectRole::Developer));
    let reviewer = match &input {
        Some(input) => input.reviewer_id.clone().unwrap_or_default(),
        None => first_with(&project, ProjectRole::Reviewer),
    };
    let tester = match &input {
        Some(input) => input.tester_id.clone().unwrap_or_default(),
        None => first_with(&project, ProjectRole::Tester),
    };
    let form = cx.new(|_| TaskForm {
        project,
        opened: task,
        title: title_field.clone(),
        instructions,
        acceptance,
        developer,
        reviewer,
        tester,
        busy: false,
        problem: None,
        window: window.window_handle(),
        dialog: 0,
    });
    open_dialog(window, cx, move |dialog, window, cx| {
        let heading = if form.read(cx).opened.is_some() {
            "Edit task"
        } else {
            "New task"
        };
        let body = task_body(&form, &screen, window, cx);
        // task-editor.tsx's 520 px, as rems.
        dialog
            .w(window.rem_size() * 32.5)
            .title(title(heading, cx))
            // unguarded: returns a constant, nothing in it can panic
            .on_ok(|_, _, _| false)
            .child(body)
    });
    window.defer(cx, move |window, cx| {
        guarded("focus the task's title", || {
            title_field.update(cx, |field, cx| field.focus(window, cx))
        });
    });
}

fn task_body(
    form: &Entity<TaskForm>,
    screen: &Entity<ProjectsScreen>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let bots = store::state(cx).bots.clone();
    let state = form.read(cx);
    let (title_field, instructions, acceptance) = (
        state.title.clone(),
        state.instructions.clone(),
        state.acceptance.clone(),
    );
    let ready = state.ready(cx);
    let busy = state.busy;
    let failed = state.problem.clone();
    let members = state.project.input.members.clone();
    let chosen: Vec<String> = [
        ProjectRole::Developer,
        ProjectRole::Reviewer,
        ProjectRole::Tester,
    ]
    .into_iter()
    .map(|role| state.chosen(role).to_owned())
    .collect();
    let pickers: Vec<AnyElement> = [
        (
            ProjectRole::Developer,
            "task_form.developer",
            "Developer",
            "Choose a developer",
        ),
        (
            ProjectRole::Reviewer,
            "task_form.reviewer",
            "Reviewer",
            "Unassigned",
        ),
        (
            ProjectRole::Tester,
            "task_form.tester",
            "Tester",
            "Unassigned",
        ),
    ]
    .into_iter()
    .zip(chosen)
    .map(|((role, id, heading, nobody), current)| {
        let name_of = |id: &str| {
            bots.iter()
                .find(|bot| bot.id == id)
                .map(|bot| bot.name.clone())
        };
        let value = name_of(&current).unwrap_or_else(|| nobody.to_owned());
        let mut choices = vec![(String::new(), nobody.to_owned())];
        choices.extend(
            members
                .iter()
                .filter(|member| member.roles.contains(&role))
                .map(|member| {
                    (
                        member.bot_id.clone(),
                        name_of(&member.bot_id).unwrap_or_else(|| "Unavailable agent".to_owned()),
                    )
                }),
        );
        let choosing = form.clone();
        let control = picker(
            id,
            heading,
            value,
            !busy,
            current,
            choices,
            move |chosen, cx| {
                choosing.update(cx, |form, cx| {
                    form.choose(role, chosen);
                    cx.notify();
                })
            },
            window,
            cx,
        );
        field(heading, control, cx)
    })
    .collect();
    let (saving, screen) = (form.clone(), screen.clone());
    let footer = footer(
        "task_form",
        "Save task",
        busy,
        ready,
        failed,
        move |_, cx| save_task(&saving, &screen, cx),
        cx,
    );
    let theme = theme(cx);
    div()
        .mt(r(theme.space.s16))
        .flex()
        .flex_col()
        .gap(r(theme.space.s16))
        .child(field("Title", TextField::new("task_form.title", "Title", &title_field), cx))
        .child(field(
            "Instructions",
            TextArea::new("task_form.instructions", "Instructions", &instructions),
            cx,
        ))
        .child(field(
            "Acceptance checklist (one item per line)",
            TextArea::new(
                "task_form.acceptance",
                "Acceptance checklist (one item per line)",
                &acceptance,
            ),
            cx,
        ))
        .children(pickers)
        .child(note(
            "Save creates a task draft. Start explicitly begins developer work and may use the agent's provider \
             allowance. Integration needs a reviewer, tester and accepted checks.",
            cx,
        ))
        .child(footer)
        .into_any_element()
}

fn save_task(form: &Entity<TaskForm>, screen: &Entity<ProjectsScreen>, cx: &mut App) {
    let state = form.read(cx);
    if !state.ready(cx) {
        return;
    }
    let title = state.title.read(cx).value().trim().to_owned();
    let instructions = state.instructions.read(cx).value().trim().to_owned();
    let acceptance: Vec<String> = state
        .acceptance
        .read(cx)
        .value()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    let someone = |id: &str| (!id.is_empty()).then(|| id.to_owned());
    let (assignee_id, reviewer_id, tester_id) = (
        state.developer.clone(),
        someone(&state.reviewer),
        someone(&state.tester),
    );
    let (opened, project_id) = (state.opened.clone(), state.project.id.clone());
    form.update(cx, |form, cx| {
        form.busy = true;
        form.problem = None;
        form.dialog = crate::ui::primitives::dialog::opened(cx);
        cx.notify();
    });
    let call = match &opened {
        Some(task) => store::request::<method::TasksUpdate>(
            cx,
            TasksUpdate {
                id: task.id.clone(),
                expected_revision: task.revision,
                patch: TaskPatch {
                    title: Some(title),
                    instructions: Some(instructions),
                    acceptance: Some(acceptance),
                    assignee_id: Some(assignee_id),
                    reviewer_id: Some(reviewer_id),
                    tester_id: Some(tester_id),
                },
            },
        ),
        None => store::request::<method::TasksCreate>(
            cx,
            TaskInput {
                project_id,
                title,
                instructions,
                acceptance,
                assignee_id,
                reviewer_id,
                tester_id,
            },
        ),
    };
    let (form, screen) = (form.clone(), screen.clone());
    cx.spawn(async move |cx| {
        guarded_async("save a task", async move {
            let saved = call.await;
            cx.update(|cx| {
                let opened = (form.read(cx).window, form.read(cx).dialog);
                finish(
                    opened,
                    saved,
                    cx,
                    move |saved: ProjectTask, window, cx| {
                        screen.update(cx, |this, cx| this.select_task(saved.id, window, cx))
                    },
                    move |said, cx| {
                        form.update(cx, |form, cx| {
                            form.busy = false;
                            form.problem = Some(said);
                            cx.notify();
                        })
                    },
                )
            });
        })
        .await;
    })
    .detach();
}
