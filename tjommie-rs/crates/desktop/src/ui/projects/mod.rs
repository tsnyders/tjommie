//! Projects (Phase 3 plan T5.3; `app/projects/page.tsx`, `projects/*`): developer teams. The teams at the left; at the
//! right a team's refs, room and actions, its tasks, and the chosen task's detail (`detail.rs`). The team and task
//! editors are dialogs (`editors.rs`). Saving a team or a task starts no provider work; Start does.

mod detail;
mod editors;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use client::{ClientMethod, Rpc as _, RpcError};
use futures::StreamExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Task, Window, div, svg,
};
use protocol::projects::{
    AttemptId, ExpectedRevision, Project, ProjectId, ProjectRepository, ProjectState, ProjectTask,
    TaskCleanupPreview, TaskDetail, TaskDiff,
};
use protocol::wire::{Empty, Id, method};
use serde::Serialize;

use crate::guard::{guarded, guarded_async};
use crate::model::labels::error_text;
use crate::model::state::Connection;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Link, problem};
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{Pill, PillVariant, pressable};
use crate::ui::primitives::dialog::open_confirm;
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::scroll::{follow_focus, scroll_handle};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, theme};

/// The events the screen reads everything again on (`project.updated`, `task.*`), as the TypeScript's did.
fn project_event(event: &protocol::Event) -> bool {
    use protocol::Event;
    matches!(
        event,
        Event::ProjectUpdated(_)
            | Event::TaskUpdated(_)
            | Event::TaskAttemptUpdated(_)
            | Event::TaskEvaluationUpdated(_)
            | Event::TaskIntegrationUpdated(_)
    )
}

/// A new id for a request the daemon must not run twice (`crypto.randomUUID()`): unique in this run, and the time
/// keeps it apart from another run's.
fn request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let at = jiff::Timestamp::now().as_nanosecond();
    format!("desktop-{at}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A value as the daemon names it, capitalised as the TypeScript's `capitalize` shows it: "todo" is "Todo".
fn word(value: &impl Serialize) -> String {
    let named = serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut letters = named.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => named,
    }
}

/// The Projects screen, kept while the route shows it, and started afresh on another PC (the TypeScript's
/// `key={host}`): the last PC's team and task mean nothing there (M5 review M4).
pub fn screen(window: &mut Window, cx: &mut App) -> AnyElement {
    let key = match &store::state(cx).host {
        Some(host) => SharedString::from(format!("screen.projects.view.{host}")),
        None => SharedString::from("screen.projects.view"),
    };
    window
        .use_keyed_state(key, cx, ProjectsScreen::new)
        .into_any_element()
}

/// What one read brings back.
struct Read {
    projects: Vec<Project>,
    repositories: Vec<ProjectRepository>,
    tasks: Vec<ProjectTask>,
    detail: Option<TaskDetail>,
    diff: Option<TaskDiff>,
    target_sha: Option<String>,
}

/// Everything the screen shows, read in one go: the teams, the registered repositories, the chosen team's tasks and
/// heads, and the chosen task with its current attempt's diff.
async fn read(
    rpc: &store::StoreRpc,
    project: Option<String>,
    task: Option<String>,
) -> Result<Read, RpcError> {
    let projects = rpc.call::<method::ProjectsList>(Empty {}).await?;
    let repositories = rpc.call::<method::ProjectsRepositories>(Empty {}).await?;
    let (tasks, heads) = match &project {
        Some(id) => (
            rpc.call::<method::TasksList>(ProjectId {
                project_id: id.clone(),
            })
            .await?,
            Some(rpc.call::<method::ProjectsHeads>(Id { id: id.clone() }).await?),
        ),
        None => (Vec::new(), None),
    };
    let detail = match task {
        Some(id) => Some(rpc.call::<method::TasksGet>(Id { id }).await?),
        None => None,
    };
    // A missing or provisioning worktree has its own attempt state: no diff is not a problem.
    let diff = match detail
        .as_ref()
        .and_then(|detail| detail.task.current_attempt_id.clone())
    {
        Some(attempt_id) => rpc
            .call::<method::TasksDiff>(AttemptId { attempt_id })
            .await
            .ok(),
        None => None,
    };
    Ok(Read {
        projects,
        repositories,
        tasks,
        detail,
        diff,
        target_sha: heads.map(|heads| heads.target_sha),
    })
}

pub struct ProjectsScreen {
    projects: Vec<Project>,
    repositories: Vec<ProjectRepository>,
    tasks: Vec<ProjectTask>,
    detail: Option<TaskDetail>,
    diff: Option<TaskDiff>,
    target_sha: Option<String>,
    project_id: Option<String>,
    task_id: Option<String>,
    /// Actions on their way: while one runs the others wait (Stop and Cancel go at once).
    pending: usize,
    loading: bool,
    problem: Option<SharedString>,
    cleanup: Option<TaskCleanupPreview>,
    /// Which collapsible parts are open: a review's coverage, a check's output.
    open: HashSet<String>,
    /// Each read's number: a newer one drops what an older one brings back.
    reads: u64,
    /// The task whose detail comes into view, and takes the focus, once it is read.
    bring: Option<String>,
    was_ready: bool,
    detail_focus: FocusHandle,
    scroll: ScrollHandle,
    _refresh: Task<()>,
}

impl ProjectsScreen {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        let mut feed = store::events(cx, project_event);
        let refresh = cx.spawn(async move |this, cx| {
            guarded_async("follow the projects' events", async move {
                while feed.arrivals.next().await.is_some() {
                    // A burst of task events is one read.
                    while let Ok(()) = feed.arrivals.try_recv() {}
                    if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                        break;
                    }
                }
            })
            .await;
        });
        let was_ready = Self::ready(cx);
        let mut screen = ProjectsScreen {
            projects: Vec::new(),
            repositories: Vec::new(),
            tasks: Vec::new(),
            detail: None,
            diff: None,
            target_sha: None,
            project_id: None,
            task_id: None,
            pending: 0,
            loading: false,
            problem: None,
            cleanup: None,
            open: HashSet::new(),
            reads: 0,
            bring: None,
            was_ready,
            detail_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _refresh: refresh,
        };
        screen.reload(cx);
        screen
    }

    fn ready(cx: &App) -> bool {
        store::state(cx).connection == Connection::Ready
    }

    /// Reads everything again (`reload`).
    fn reload(&mut self, cx: &mut Context<Self>) {
        if !Self::ready(cx) {
            return;
        }
        self.reads += 1;
        let reads = self.reads;
        self.loading = true;
        cx.notify();
        let rpc = store::rpc(cx);
        let (project, task) = (self.project_id.clone(), self.task_id.clone());
        cx.spawn(async move |this, cx| {
            guarded_async("read the projects", async move {
                let read = read(&rpc, project, task).await;
                let _ = this.update(cx, |this, cx| this.take(reads, read, cx));
            })
            .await;
        })
        .detach();
    }

    fn take(&mut self, reads: u64, read: Result<Read, RpcError>, cx: &mut Context<Self>) {
        if reads != self.reads {
            return;
        }
        self.loading = false;
        match read {
            Ok(read) => {
                self.projects = read.projects;
                self.repositories = read.repositories;
                self.tasks = read.tasks;
                self.detail = read.detail;
                self.diff = read.diff;
                self.target_sha = read.target_sha;
                self.problem = None;
            }
            Err(error) => self.problem = Some(error_text(&error).into()),
        }
        cx.notify();
    }

    fn select_project(&mut self, id: String, cx: &mut Context<Self>) {
        self.bring = None;
        self.project_id = Some(id);
        self.task_id = None;
        self.detail = None;
        self.diff = None;
        self.cleanup = None;
        self.reload(cx);
    }

    /// Shows a task's detail, brought into view with the focus once it is read; choosing the one shown brings it again.
    fn select_task(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.task_id.as_deref() == Some(id.as_str()) {
            self.bring_detail(window, cx);
            return;
        }
        self.bring = Some(id.clone());
        self.task_id = Some(id);
        self.detail = None;
        self.diff = None;
        self.cleanup = None;
        self.reload(cx);
    }

    fn bring_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bring = None;
        // The detail is the scroll's last child.
        if let Some(last) = self.scroll.children_count().checked_sub(1) {
            self.scroll.scroll_to_top_of_item(last);
        }
        self.detail_focus.focus(window, cx);
    }

    /// One of the screen's actions (`action`): then everything is read again; a failure says why at the top. While one
    /// runs the others wait, except `concurrent` ones (Stop, Cancel task). `then` gets what the daemon answered.
    fn act<M: ClientMethod>(
        &mut self,
        params: M::Params,
        concurrent: bool,
        then: impl FnOnce(&mut Self, M::Result) + 'static,
        cx: &mut Context<Self>,
    ) {
        if (self.pending > 0 && !concurrent) || !Self::ready(cx) {
            return;
        }
        self.pending += 1;
        self.problem = None;
        cx.notify();
        let call = store::request::<M>(cx, params);
        cx.spawn(async move |this, cx| {
            guarded_async("run a project action", async move {
                let result = call.await;
                let _ = this.update(cx, |this, cx| {
                    this.pending = this.pending.saturating_sub(1);
                    match result {
                        Ok(value) => {
                            then(this, value);
                            this.reload(cx);
                        }
                        Err(error) => this.problem = Some(error_text(&error).into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }

    fn busy(&self) -> bool {
        self.pending > 0
    }

    fn toggle_open(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        cx.notify();
    }
}

/// A pill that is unavailable while `enabled` is false.
fn pill(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let pill = Pill::new(id, label).small().disabled(!enabled).on_press(on_press);
    let pill = if primary {
        pill.variant(PillVariant::Primary)
    } else {
        pill
    };
    pill.into_any_element()
}

/// A short line of secondary text, read by tests as `id`.
pub(crate) fn line(id: impl Into<SharedString>, role: Role, text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let text = text.into();
    div()
        .semantic(id, role)
        .aria_label(text.clone())
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text)
        .into_any_element()
}

impl Render for ProjectsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = Self::ready(cx);
        if ready && !self.was_ready {
            self.reload(cx);
        }
        self.was_ready = ready;
        if self
            .bring
            .as_ref()
            .is_some_and(|id| self.detail.as_ref().is_some_and(|detail| &detail.task.id == id))
        {
            cx.defer_in(window, |this, window, cx| {
                guarded("bring the task into view", || this.bring_detail(window, cx));
            });
        }
        let bots = store::state(cx).bots.clone();
        let screen = cx.entity();
        let busy = self.busy();
        let project = self
            .project_id
            .as_ref()
            .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            .cloned();

        let teams: Vec<AnyElement> = self
            .projects
            .iter()
            .map(|item| {
                let id: SharedString = format!("projects.team.{}", item.id).into();
                let focus = focus_handle(&id, window, cx);
                let ring = focus_visible(&focus, window);
                let theme = theme(cx);
                let colors = palette(cx);
                let chosen = self.project_id.as_deref() == Some(item.id.as_str());
                let row = div()
                    .semantic(id, Role::Button)
                    .aria_label(item.input.name.clone())
                    .aria_selected(chosen)
                    .track_focus(&focus)
                    .relative()
                    .min_h(r(56.))
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s10))
                    .px(r(theme.space.s14))
                    .py(r(theme.space.s12))
                    .rounded(r(theme.radius.card))
                    .bg(hsla(if chosen {
                        colors.surface.raised
                    } else {
                        colors.surface.sunk
                    }))
                    .when(chosen, |row| row.shadow(crate::ui::style::shadows(&theme.shadow.light.card)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/i/175/folder-git-2.svg")
                            .size(r(theme.size.icon))
                            .flex_none()
                            .text_color(hsla(colors.icon.base)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .type_style(&theme.typography.desktop.ui_strong)
                            .text_color(hsla(colors.text.primary))
                            .child(item.input.name.clone()),
                    )
                    .when(item.state == ProjectState::Archived, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .type_style(&theme.typography.desktop.meta)
                                .text_color(hsla(colors.text.tertiary))
                                .child("Archived"),
                        )
                    })
                    .when(ring, |row| row.child(focus_ring(theme.radius.card, cx)));
                let (screen, id) = (screen.clone(), item.id.clone());
                pressable(
                    row,
                    Some(std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                        screen.update(cx, |this, cx| this.select_project(id.clone(), cx))
                    })),
                )
                .into_any_element()
            })
            .collect();

        let right = project.as_ref().map(|project| self.team(project, &bots, window, cx));
        let teams_scroll = scroll_handle("projects.teams", window, cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let new_project = {
            let screen = screen.clone();
            pill("projects.new", "New project", true, ready && !busy, move |window, cx| {
                editors::open_project_editor(screen.clone(), None, window, cx)
            })
        };
        let refresh = screen.clone();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(r(theme.space.s20))
            .pt(r(theme.space.s32))
            .px(r(theme.space.s32))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(r(theme.space.s16))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .semantic("screen.projects", Role::Heading)
                                    .aria_label("Developer teams")
                                    .type_style(&theme.typography.desktop.display_l)
                                    .text_color(hsla(colors.text.primary))
                                    .child("Developer teams"),
                            )
                            .child(
                                div()
                                    .mt(r(theme.space.s10))
                                    .type_style(&theme.typography.desktop.ui)
                                    .text_color(hsla(colors.text.secondary))
                                    .child("Project tasks, isolated workspaces and exact-commit integration."),
                            ),
                    )
                    .child(new_project),
            )
            .when(!ready, |screen| {
                screen.child(line(
                    "projects.offline",
                    Role::Status,
                    "Connect to the host PC to view and run project work.",
                    cx,
                ))
            })
            .children(self.problem.clone().map(|text| {
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(r(theme.space.s8))
                    .child(problem("projects.problem", text, cx))
                    .child(Pill::new("projects.refresh", "Refresh").small().on_press(
                        move |_, cx| refresh.update(cx, |this, cx| this.reload(cx)),
                    ))
            }))
            .when(self.loading, |screen| {
                screen.child(line(
                    "projects.loading",
                    Role::Status,
                    "Refreshing project state…",
                    cx,
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(r(theme.space.s16))
                    .child(follow_focus(
                        &teams_scroll,
                        div()
                            .id("projects.teams")
                            .track_scroll(&teams_scroll)
                            .w(r(240.))
                            .flex_none()
                            .overflow_y_scroll()
                            .pb(r(theme.space.s24))
                            .flex()
                            .flex_col()
                            .gap(r(theme.space.s8))
                            .children(teams)
                            .when(ready && self.projects.is_empty() && !self.loading, |list| {
                                list.child(line(
                                    "projects.none",
                                    Role::Paragraph,
                                    "Create a project team to assign existing agents and prepare task drafts.",
                                    cx,
                                ))
                            }),
                    ))
                    .child(follow_focus(
                        &self.scroll,
                        div()
                            .id("projects.scroll")
                            .track_scroll(&self.scroll)
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .pb(r(theme.space.s24))
                            .flex()
                            .flex_col()
                            .gap(r(theme.space.s16))
                            .children(right.into_iter().flatten()),
                    )),
            )
    }
}

impl ProjectsScreen {
    /// The chosen team: its header, its tasks, and the chosen task's detail (the scroll's children, the detail last).
    fn team(
        &self,
        project: &Project,
        bots: &[protocol::model::Bot],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let screen = cx.entity();
        let active = project.state == ProjectState::Active;
        let can = !self.busy() && active;
        let tasks: Vec<AnyElement> = self
            .tasks
            .iter()
            .map(|task| {
                let id: SharedString = format!("projects.task.{}", task.id).into();
                let focus = focus_handle(&id, window, cx);
                let ring = focus_visible(&focus, window);
                let theme = theme(cx);
                let colors = palette(cx);
                let chosen = self.task_id.as_deref() == Some(task.id.as_str());
                let developer = bots.iter().find(|bot| bot.id == task.input.assignee_id);
                let card = div()
                    .semantic(id, Role::Button)
                    .aria_label(task.input.title.clone())
                    .aria_selected(chosen)
                    .track_focus(&focus)
                    .relative()
                    .min_h(r(56.))
                    .p(r(theme.space.s16))
                    .rounded(r(theme.radius.card))
                    .bg(hsla(if chosen {
                        colors.surface.raised
                    } else {
                        colors.surface.sunk
                    }))
                    .when(chosen, |card| card.shadow(crate::ui::style::shadows(&theme.shadow.light.card)))
                    .cursor_pointer()
                    .child(
                        div()
                            .type_style(&theme.typography.desktop.ui_strong)
                            .text_color(hsla(colors.text.primary))
                            .child(task.input.title.clone()),
                    )
                    .child(
                        div()
                            .mt(r(theme.space.s8))
                            .flex()
                            .items_center()
                            .gap(r(theme.space.s8))
                            .children(developer.map(|bot| BotAvatar::new(bot.color, bot.shape, 18.)))
                            .child(
                                div()
                                    .type_style(&theme.typography.desktop.meta)
                                    .text_color(hsla(colors.text.secondary))
                                    .child(word(&task.state)),
                            ),
                    )
                    .when(ring, |card| card.child(focus_ring(theme.radius.card, cx)));
                let (screen, id) = (screen.clone(), task.id.clone());
                pressable(
                    card,
                    Some(std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                        screen.update(cx, |this, cx| this.select_task(id.clone(), window, cx))
                    })),
                )
                .into_any_element()
            })
            .collect();
        let theme = theme(cx);
        let colors = palette(cx);
        let edit = {
            let (screen, project) = (screen.clone(), project.clone());
            pill("projects.edit", "Edit team", false, can, move |window, cx| {
                editors::open_project_editor(screen.clone(), Some(project.clone()), window, cx)
            })
        };
        let archive = {
            let (screen, project) = (screen.clone(), project.clone());
            pill("projects.archive", "Archive team", false, can, move |window, cx| {
                let (screen, id, revision) = (screen.clone(), project.id.clone(), project.revision);
                open_confirm(
                    window,
                    cx,
                    format!("Archive {}?", project.input.name),
                    "You will no longer be able to edit this team or start new tasks. Existing branches and files \
                     are kept. This cannot be undone in Tjommie.",
                    "Archive team",
                    false,
                    move |_, cx| {
                        let params = ExpectedRevision {
                            id: id.clone(),
                            expected_revision: revision,
                        };
                        screen.update(cx, |this, cx| {
                            this.act::<method::ProjectsArchive>(params, false, |_, _| {}, cx)
                        })
                    },
                );
            })
        };
        let new_task = {
            let (screen, project) = (screen.clone(), project.clone());
            pill("projects.new_task", "New task", true, can, move |window, cx| {
                editors::open_task_editor(screen.clone(), project.clone(), None, window, cx)
            })
        };
        let room = project.input.room_id.clone().map(|room| {
            div().mt(r(theme.space.s8)).child(
                Link::new("projects.room", "Open team room")
                    .on_press(move |_, cx| cx.navigate(Route::Room(room.clone()))),
            )
        });
        let header = div()
            .flex()
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap(r(theme.space.s16))
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .semantic("projects.name", Role::Heading)
                            .aria_label(project.input.name.clone())
                            .type_style(&theme.typography.desktop.display_s)
                            .text_color(hsla(colors.text.primary))
                            .child(project.input.name.clone()),
                    )
                    .child(line(
                        "projects.refs",
                        Role::Paragraph,
                        format!("{} → {}", project.input.base_ref, project.input.target_ref),
                        cx,
                    ))
                    .children(room),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(r(theme.space.s8))
                    .child(edit)
                    .child(archive)
                    .child(new_task),
            )
            .into_any_element();
        let mut children = vec![
            header,
            div()
                .semantic("projects.tasks", Role::Group)
                .aria_label("Project tasks")
                .grid()
                .grid_cols(2)
                .gap(r(theme.space.s16))
                .children(tasks)
                .into_any_element(),
        ];
        if self.tasks.is_empty() {
            children.push(line(
                "projects.tasks.none",
                Role::Paragraph,
                "No tasks yet. Saving a task does not start provider work.",
                cx,
            ));
        }
        if let Some(detail) = self
            .detail
            .as_ref()
            .filter(|detail| detail.task.input.project_id == project.id)
        {
            children.push(self.detail_view(project, detail, window, cx));
        }
        children
    }
}

#[cfg(test)]
mod tests;
