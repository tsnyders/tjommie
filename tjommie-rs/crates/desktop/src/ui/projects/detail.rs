//! A task's detail (`task-detail.tsx`, `task-diff.tsx`): its state and attempt, its instructions and checklist, the work
//! actions (Start or Return to developer, Stop, Cancel task, Run review, Run checks), the attempt's workspace and its
//! cleanup, the committed diff, the review and check history, and local integration. Every action is the screen's
//! `act`, so the screen reads everything again after it.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use protocol::projects::{
    AttemptId, AttemptState, CleanupConfirm, EvaluationKind, EvaluationState, ExpectedRevision,
    IntegrationState, Project, ProjectState, TaskDetail, TaskDiff, TaskEvaluation, TaskRequest,
    TaskState, TasksCleanup, TasksEvaluate, TasksIntegrate, TasksPromote,
};
use protocol::wire::method;

use super::{ProjectsScreen, line, pill, request_id, word};
use crate::model::projects::{can_integrate, short_sha};
use crate::semantic::SemanticExt as _;
use crate::ui::fields::Link;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// The note under the work actions.
const ALLOWANCE: &str = "Start, Return to developer and Run review use the assigned agent's provider allowance. The first \
                         review verifies this host's tool restrictions before reviewing a frozen source snapshot. \
                         Unsupported accounts show an unavailable result. Stop and Cancel retain all branches and files.";

/// A part's label (`text-label` headings in the detail).
fn label(text: &'static str, cx: &App) -> Div {
    let theme = theme(cx);
    div()
        .type_style(&theme.typography.desktop.label)
        .text_color(hsla(palette(cx).text.tertiary))
        .child(text)
}

/// Body text.
fn body(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = theme(cx);
    div()
        .type_style(&theme.typography.desktop.body)
        .text_color(hsla(palette(cx).text.primary))
        .child(text.into())
}

/// A bordered box (`rounded-[12px] border border-divider p-3`).
fn boxed(cx: &App) -> Div {
    let theme = theme(cx);
    div()
        .p(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .border_1()
        .border_color(hsla(palette(cx).line.base))
        .flex()
        .flex_col()
        .gap(r(theme.space.s4))
}

/// Monospaced text: a diff or a check's output, named `label`; the text itself reads as `<id>.body`.
fn mono(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    text: String,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let id: SharedString = id.into();
    let shown = div()
        .semantic(format!("{id}.body"), Role::Paragraph)
        .aria_label(text.clone())
        .child(text);
    div()
        .semantic(id, Role::Code)
        .aria_label(label)
        .max_h(r(384.))
        .overflow_y_scroll()
        .p(r(theme.space.s12))
        .rounded(r(theme.radius.field))
        .border_1()
        .border_color(hsla(palette(cx).line.base))
        .type_style(&theme.typography.desktop.meta)
        .font_family(crate::assets::MONO_FAMILY)
        .text_color(hsla(palette(cx).text.primary))
        .child(shown)
        .into_any_element()
}

/// The committed diff (`TaskDiffView`): its text, read as text, and what limits a review of it.
pub(super) fn diff_view(diff: Option<&TaskDiff>, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let shown: Vec<AnyElement> = match diff {
        Some(diff) => {
            let mut shown = Vec::new();
            if diff.dirty {
                shown.push(line(
                    "projects.diff.dirty",
                    Role::Status,
                    "The worktree has changes outside this commit. Review and integration are blocked until they are \
                     committed.",
                    cx,
                ));
            }
            let text = if diff.text.is_empty() {
                "No committed changes from the attempt base.".to_owned()
            } else {
                diff.text.clone()
            };
            shown.push(mono("projects.diff.text", "Committed diff", text, cx));
            if diff.truncated {
                shown.push(line(
                    "projects.diff.truncated",
                    Role::Status,
                    "This diff exceeds the preview limit. Automated review cannot claim complete coverage.",
                    cx,
                ));
            }
            shown
        }
        None => vec![line(
            "projects.diff.none",
            Role::Paragraph,
            "The diff is available after the attempt has a managed worktree.",
            cx,
        )],
    };
    div()
        .semantic("projects.diff", Role::Group)
        .aria_label("Task diff")
        .flex()
        .flex_col()
        .gap(r(theme.space.s8))
        .child(label("Committed changes", cx))
        .children(shown)
        .into_any_element()
}

impl ProjectsScreen {
    /// A collapsible part: a link that opens it, and what it holds while open.
    fn collapsible(
        &self,
        key: String,
        summary: String,
        held: impl FnOnce(&App) -> Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open.contains(&key);
        let screen = cx.entity();
        let id = format!("projects.open.{key}");
        let toggle = Link::new(id, summary)
            .quiet()
            .icon(if open {
                "chevron-down"
            } else {
                "chevron-right"
            })
            .on_press(move |_, cx| {
                let key = key.clone();
                screen.update(cx, |this, cx| this.toggle_open(key, cx))
            });
        let theme = theme(cx);
        div()
            .flex()
            .flex_col()
            .gap(r(theme.space.s6))
            .child(toggle)
            .when(open, |part| part.children(held(cx)))
            .into_any_element()
    }

    fn evaluation(&self, evaluation: &TaskEvaluation, cx: &mut Context<Self>) -> AnyElement {
        let id = &evaluation.id;
        let mut parts = vec![
            body(
                format!(
                    "{} · {} · {}",
                    word(&evaluation.kind),
                    word(&evaluation.state),
                    short_sha(Some(&evaluation.source_sha))
                ),
                cx,
            )
            .into_any_element(),
        ];
        if let Some(reason) = &evaluation.reason {
            parts.push(line(
                format!("projects.evaluation.{id}.reason"),
                Role::Paragraph,
                reason.clone(),
                cx,
            ));
        }
        if evaluation.kind == EvaluationKind::Review {
            match evaluation.review_coverage.as_ref().and_then(Option::as_ref) {
                Some(coverage) => {
                    let coverage = coverage.clone();
                    parts.push(self.collapsible(
                        format!("coverage.{id}"),
                        format!(
                            "Complete snapshot supplied · {} files",
                            coverage.files.len()
                        ),
                        move |cx| {
                            let mut held = vec![line(
                                "projects.coverage.snapshot",
                                Role::Paragraph,
                                format!("Snapshot SHA-256: {}", coverage.snapshot_sha256),
                                cx,
                            )];
                            held.extend(coverage.files.iter().enumerate().map(|(at, file)| {
                                line(
                                    format!("projects.coverage.file.{at}"),
                                    Role::ListItem,
                                    format!(
                                        "{} · {} bytes · {}",
                                        file.path, file.bytes, file.sha256
                                    ),
                                    cx,
                                )
                            }));
                            held
                        },
                        cx,
                    ));
                }
                None => parts.push(line(
                    format!("projects.evaluation.{id}.coverage"),
                    Role::Paragraph,
                    "Complete source coverage is unavailable for this result.",
                    cx,
                )),
            }
        }
        for (at, finding) in evaluation.findings.iter().enumerate() {
            let place = match finding.line {
                Some(line) => format!("{}:{line}", finding.path),
                None => finding.path.clone(),
            };
            parts.push(line(
                format!("projects.evaluation.{id}.finding.{at}"),
                Role::ListItem,
                format!("{} · {place}: {}", word(&finding.severity), finding.message),
                cx,
            ));
        }
        for (at, check) in evaluation.checks.iter().enumerate() {
            let exit = check
                .exit_code
                .map_or_else(|| "unavailable".to_owned(), |code| code.to_string());
            let summary = format!("{} {} · exit {exit}", check.command, check.args.join(" "));
            let (output, truncated) = (check.output.clone(), check.truncated);
            let key = format!("check.{id}.{at}");
            let output_id = format!("projects.{key}.output");
            parts.push(self.collapsible(
                key,
                summary,
                move |cx| {
                    let mut held = vec![mono(output_id, "Check output", output, cx)];
                    if truncated {
                        held.push(line(
                            "projects.check.truncated",
                            Role::Paragraph,
                            "Output was truncated.",
                            cx,
                        ));
                    }
                    held
                },
                cx,
            ));
        }
        boxed(cx).children(parts).into_any_element()
    }

    /// The task's detail. `window` gives the parts their focus handles.
    pub(super) fn detail_view(
        &self,
        project: &Project,
        detail: &TaskDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = window;
        let screen = cx.entity();
        let task = &detail.task;
        let attempt = detail
            .attempts
            .iter()
            .find(|attempt| Some(attempt.id.as_str()) == task.current_attempt_id.as_deref())
            .cloned();
        let running = attempt.as_ref().is_some_and(|attempt| {
            matches!(
                attempt.state,
                AttemptState::Provisioning | AttemptState::Queued | AttemptState::Running
            )
        });
        let evaluating = detail
            .evaluations
            .iter()
            .any(|evaluation| evaluation.state == EvaluationState::Running)
            || detail.integrations.iter().any(|integration| {
                matches!(
                    integration.state,
                    IntegrationState::Preparing | IntegrationState::Checking
                )
            });
        let connected = Self::ready(cx);
        let busy = self.busy();
        let active = project.state == ProjectState::Active;
        let disabled = busy || !connected || !active;
        let dirty = self.diff.as_ref().is_some_and(|diff| diff.dirty);
        let returning = attempt
            .as_ref()
            .is_some_and(|attempt| attempt.state != AttemptState::Cleaned);
        let status = format!(
            "{} · {}",
            word(&task.state),
            attempt.as_ref().map_or_else(
                || "Not started".to_owned(),
                |attempt| format!("Attempt {}: {}", attempt.ordinal, word(&attempt.state))
            )
        );

        let edit = {
            let (screen, project, task) = (screen.clone(), project.clone(), task.clone());
            pill(
                "projects.detail.edit",
                "Edit task",
                false,
                !(disabled || running || evaluating),
                move |window, cx| {
                    super::editors::open_task_editor(
                        screen.clone(),
                        project.clone(),
                        Some(task.clone()),
                        window,
                        cx,
                    )
                },
            )
        };
        let start = {
            let screen = screen.clone();
            let params = TaskRequest {
                id: task.id.clone(),
                expected_revision: task.revision,
                request_id: String::new(),
            };
            pill(
                "projects.detail.start",
                if returning {
                    "Return to developer"
                } else {
                    "Start task"
                },
                true,
                !(disabled || running || evaluating),
                move |_, cx| {
                    let params = TaskRequest {
                        request_id: request_id(),
                        ..params.clone()
                    };
                    screen.update(cx, |this, cx| {
                        if returning {
                            this.act::<method::TasksResume>(params, false, |_, _| {}, cx)
                        } else {
                            this.act::<method::TasksStart>(params, false, |_, _| {}, cx)
                        }
                    })
                },
            )
        };
        let expected = ExpectedRevision {
            id: task.id.clone(),
            expected_revision: task.revision,
        };
        let stop = {
            let (screen, expected) = (screen.clone(), expected.clone());
            pill(
                "projects.detail.stop",
                "Stop",
                false,
                connected && active && (running || evaluating),
                move |_, cx| {
                    let expected = expected.clone();
                    screen.update(cx, |this, cx| {
                        this.act::<method::TasksStop>(expected, true, |_, _| {}, cx)
                    })
                },
            )
        };
        let cancel = {
            let screen = screen.clone();
            pill(
                "projects.detail.cancel",
                "Cancel task",
                false,
                connected && active && task.state != TaskState::Cancelled,
                move |_, cx| {
                    let expected = expected.clone();
                    screen.update(cx, |this, cx| {
                        this.act::<method::TasksCancel>(expected, true, |_, _| {}, cx)
                    })
                },
            )
        };
        let completed = attempt
            .as_ref()
            .is_some_and(|attempt| attempt.state == AttemptState::Completed);
        let evaluate = |kind: EvaluationKind, id: &'static str, text: &'static str| {
            let (screen, attempt) = (screen.clone(), attempt.clone());
            pill(
                id,
                text,
                false,
                !(disabled || evaluating || !completed || dirty),
                move |_, cx| {
                    let Some(attempt) = &attempt else {
                        return;
                    };
                    let Some(head) = attempt.head_sha.clone() else {
                        return;
                    };
                    let params = TasksEvaluate {
                        attempt_id: attempt.id.clone(),
                        expected_revision: attempt.revision,
                        source_sha: head,
                        kind,
                    };
                    screen.update(cx, |this, cx| {
                        this.act::<method::TasksEvaluate>(params, false, |_, _| {}, cx)
                    })
                },
            )
        };
        let review = evaluate(
            EvaluationKind::Review,
            "projects.detail.review",
            "Run review",
        );
        let checks = evaluate(EvaluationKind::Test, "projects.detail.checks", "Run checks");

        let workspace = attempt.as_ref().map(|attempt| {
            let theme = theme(cx);
            div()
                .semantic("projects.detail.workspace", Role::Group)
                .aria_label("Attempt workspace")
                .flex()
                .flex_col()
                .gap(r(theme.space.s4))
                .child(line(
                    "projects.detail.branch",
                    Role::Paragraph,
                    format!("Branch: {}", attempt.branch),
                    cx,
                ))
                .child(line(
                    "projects.detail.worktree",
                    Role::Paragraph,
                    format!("Worktree on host: {}", attempt.worktree_path),
                    cx,
                ))
                .child(line(
                    "projects.detail.commit",
                    Role::Paragraph,
                    format!(
                        "Commit: {}",
                        attempt.head_sha.as_deref().unwrap_or("Not recorded")
                    ),
                    cx,
                ))
                .children(
                    attempt
                        .reason
                        .clone()
                        .map(|reason| line("projects.detail.reason", Role::Status, reason, cx)),
                )
        });
        let cleanup = attempt
            .as_ref()
            .filter(|attempt| attempt.state != AttemptState::Cleaned)
            .map(|attempt| self.cleanup_part(attempt, busy, connected, running || evaluating, cx));

        let evaluations: Vec<AnyElement> = detail
            .evaluations
            .iter()
            .map(|evaluation| self.evaluation(evaluation, cx))
            .collect();
        let integrate = {
            let screen = screen.clone();
            let ready = !disabled
                && self.target_sha.is_some()
                && can_integrate(
                    project,
                    detail,
                    self.diff.as_ref().is_none_or(|diff| diff.dirty),
                );
            let (attempt, target) = (attempt.clone(), self.target_sha.clone());
            pill(
                "projects.detail.integrate",
                "Integrate accepted commit",
                true,
                ready,
                move |_, cx| {
                    let (Some(attempt), Some(target)) = (&attempt, &target) else {
                        return;
                    };
                    let Some(head) = attempt.head_sha.clone() else {
                        return;
                    };
                    let params = TasksIntegrate {
                        attempt_id: attempt.id.clone(),
                        expected_revision: attempt.revision,
                        source_sha: head,
                        expected_target_sha: target.clone(),
                        request_id: request_id(),
                    };
                    screen.update(cx, |this, cx| {
                        this.act::<method::TasksIntegrate>(params, false, |_, _| {}, cx)
                    })
                },
            )
        };
        let integrations: Vec<AnyElement> = detail
            .integrations
            .iter()
            .map(|integration| {
                let id = &integration.id;
                let blocked_here = integration.reason.as_deref() == Some("target_checked_out");
                let retry = (integration.candidate_sha.is_some()
                    && integration.state == IntegrationState::Blocked
                    && blocked_here)
                    .then(|| {
                        let screen = screen.clone();
                        let params = TasksPromote {
                            id: integration.id.clone(),
                            expected_target_sha: integration.expected_target_sha.clone(),
                        };
                        pill(
                            format!("projects.integration.{id}.retry"),
                            "Retry target promotion",
                            false,
                            !disabled,
                            move |_, cx| {
                                let params = params.clone();
                                screen.update(cx, |this, cx| {
                                    this.act::<method::TasksPromote>(params, false, |_, _| {}, cx)
                                })
                            },
                        )
                    });
                boxed(cx)
                    .child(body(
                        format!(
                            "{} · {}",
                            word(&integration.state),
                            short_sha(integration.candidate_sha.as_deref())
                        ),
                        cx,
                    ))
                    .children(integration.reason.clone().map(|reason| {
                        line(format!("projects.integration.{id}.reason"), Role::Paragraph, reason, cx)
                    }))
                    .child(line(
                        format!("projects.integration.{id}.candidate"),
                        Role::Paragraph,
                        format!("Retained candidate: {}", integration.worktree_path),
                        cx,
                    ))
                    .when(blocked_here, |part| {
                        part.child(line(
                            format!("projects.integration.{id}.help"),
                            Role::Paragraph,
                            "Save the target checkout's work and leave that branch, then retry promotion. Its index \
                             and files are preserved.",
                            cx,
                        ))
                    })
                    .children(retry)
                    .into_any_element()
            })
            .collect();

        let theme = theme(cx);
        let colors = palette(cx);
        div()
            .semantic("projects.detail", Role::Article)
            .aria_label(format!("Task: {}", task.input.title))
            .track_focus(&self.detail_focus)
            .p(r(theme.space.s20))
            .rounded(r(theme.radius.card))
            .bg(hsla(colors.surface.raised))
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(crate::ui::style::shadows(&theme.shadow.light.card))
            .flex()
            .flex_col()
            .gap(r(theme.space.s20))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .justify_between()
                    .gap(r(theme.space.s12))
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .semantic("projects.detail.title", Role::Heading)
                                    .aria_label(task.input.title.clone())
                                    .type_style(&theme.typography.desktop.display_s)
                                    .text_color(hsla(colors.text.primary))
                                    .child(task.input.title.clone()),
                            )
                            .child(
                                div()
                                    .semantic("projects.detail.status", Role::Status)
                                    .aria_label(status.clone())
                                    .mt(r(theme.space.s4))
                                    .type_style(&theme.typography.desktop.body)
                                    .text_color(hsla(colors.text.secondary))
                                    .child(status),
                            ),
                    )
                    .child(edit),
            )
            .child(body(task.input.instructions.clone(), cx))
            .when(!task.input.acceptance.is_empty(), |part| {
                part.child(
                    div()
                        .semantic("projects.detail.acceptance", Role::List)
                        .aria_label("Acceptance checklist")
                        .flex()
                        .flex_col()
                        .gap(r(theme.space.s4))
                        .children(task.input.acceptance.iter().enumerate().map(|(at, item)| {
                            line(format!("projects.detail.acceptance.{at}"), Role::ListItem, format!("• {item}"), cx)
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(r(theme.space.s8))
                    .child(start)
                    .child(stop)
                    .child(cancel)
                    .child(review)
                    .child(checks),
            )
            .child(line("projects.detail.allowance", Role::Paragraph, ALLOWANCE, cx))
            .children(workspace)
            .children(cleanup)
            .child(diff_view(self.diff.as_ref(), cx))
            .child(
                div()
                    .semantic("projects.history", Role::Group)
                    .aria_label("Review and check history")
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s12))
                    .child(label("Review and checks", cx))
                    .when(detail.evaluations.is_empty(), |part| {
                        part.child(line(
                            "projects.history.none",
                            Role::Paragraph,
                            "No independent results yet. A developer's reported checks do not count as verified \
                             results.",
                            cx,
                        ))
                    })
                    .children(evaluations),
            )
            .child(
                div()
                    .semantic("projects.integration", Role::Group)
                    .aria_label("Local integration")
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s12))
                    .child(label("Integrate locally", cx))
                    .child(line(
                        "projects.integration.source",
                        Role::Paragraph,
                        format!(
                            "Accepted source: {} · current target: {}",
                            short_sha(attempt.as_ref().and_then(|attempt| attempt.head_sha.as_deref())),
                            short_sha(self.target_sha.as_deref())
                        ),
                        cx,
                    ))
                    .child(div().child(integrate))
                    .child(line(
                        "projects.integration.note",
                        Role::Paragraph,
                        "Requires passing review and checks for this exact commit. Combined checks run again in a \
                         separate candidate. A target checked out by the user stays untouched. Pushing and PRs are \
                         separate actions.",
                        cx,
                    ))
                    .children(integrations),
            )
            .into_any_element()
    }

    /// Worktree cleanup: a preview first, then removal of a clean worktree.
    fn cleanup_part(
        &self,
        attempt: &protocol::projects::TaskAttempt,
        busy: bool,
        connected: bool,
        working: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let screen = cx.entity();
        let preview = {
            let (screen, attempt_id) = (screen.clone(), attempt.id.clone());
            pill(
                "projects.cleanup.preview",
                "Preview worktree cleanup",
                false,
                !busy && connected && !working,
                move |_, cx| {
                    let params = AttemptId {
                        attempt_id: attempt_id.clone(),
                    };
                    screen.update(cx, |this, cx| {
                        this.act::<method::TasksCleanupPreview>(
                            params,
                            false,
                            |this, preview| this.cleanup = Some(preview),
                            cx,
                        )
                    })
                },
            )
        };
        let shown = self
            .cleanup
            .as_ref()
            .filter(|preview| preview.attempt_id == attempt.id)
            .map(|preview| {
                let remove = (preview.can_remove)
                    .then_some(preview.head_sha.clone())
                    .flatten()
                    .map(|head| {
                        let (screen, attempt_id) = (screen.clone(), preview.attempt_id.clone());
                        pill(
                            "projects.cleanup.remove",
                            "Remove this clean worktree",
                            false,
                            !busy && connected,
                            move |_, cx| {
                                let params = TasksCleanup {
                                    attempt_id: attempt_id.clone(),
                                    expected_head_sha: head.clone(),
                                    confirm: CleanupConfirm::RemoveCleanWorktree,
                                };
                                screen.update(cx, |this, cx| {
                                    this.act::<method::TasksCleanup>(
                                        params,
                                        false,
                                        |this, _| this.cleanup = None,
                                        cx,
                                    )
                                })
                            },
                        )
                    });
                boxed(cx)
                    .child(line(
                        "projects.cleanup.reason",
                        Role::Paragraph,
                        preview.reason.clone(),
                        cx,
                    ))
                    .children(preview.files.iter().enumerate().map(|(at, file)| {
                        line(
                            format!("projects.cleanup.file.{at}"),
                            Role::ListItem,
                            file.clone(),
                            cx,
                        )
                    }))
                    .when(preview.truncated, |part| {
                        part.child(line(
                            "projects.cleanup.truncated",
                            Role::Paragraph,
                            "More changed files are retained.",
                            cx,
                        ))
                    })
                    .children(remove)
            });
        let theme = theme(cx);
        div()
            .semantic("projects.cleanup", Role::Group)
            .aria_label("Worktree cleanup")
            .flex()
            .flex_col()
            .gap(r(theme.space.s12))
            .child(div().child(preview))
            .children(shown)
            .into_any_element()
    }
}
