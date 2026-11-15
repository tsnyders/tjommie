//! The approval card (Phase 3 plan T4.2; chat-cards.html `.approval`, `.answered`; `approval-card.tsx`). The one
//! raised card in a thread: what the agent wants, the exact command, its autonomy, and the answers. Approve is the
//! near-black pill and Deny an underlined text action, so the safe answer is never the loud one by accident. When the
//! request names a rule, Always allow saves it for this agent; a routine proposal is added or dismissed. The answers
//! lock after one press; the card becomes its one-line record when the daemon records the answer.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, Global, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use protocol::model::{ApprovalData, ApprovalDecision, ApprovalStatus, Autonomy, Bot, Message};
use protocol::wire::{ApprovalsRespond, method};

use super::rows::tool_icon;
use crate::guard::guarded_async;
use crate::model::approval::{approval_detail, approval_rule};
use crate::model::labels::error_text;
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::{Pill, PillVariant, TextLink};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// Answers given in this window, so an answered card can say "Always allowed".
#[derive(Default)]
struct Decided(HashMap<String, ApprovalDecision>);

impl Global for Decided {}

/// What a card's answer is doing: locked while it is sent, and what went wrong with it.
#[derive(Default)]
struct Answer {
    busy: bool,
    problem: Option<SharedString>,
}

/// The fields both kinds of card share.
struct Card<'a> {
    approval_id: &'a str,
    tool: &'a str,
    detail: &'a str,
    status: ApprovalStatus,
    rule: Option<&'a str>,
    routine: bool,
}

impl<'a> Card<'a> {
    fn of(data: &'a ApprovalData) -> Card<'a> {
        match data {
            ApprovalData::Tool(tool) => Card {
                approval_id: &tool.approval_id,
                tool: &tool.tool,
                detail: &tool.detail,
                status: tool.status,
                // A tool request can come without one (Codex asks to change files before it says which): then the
                // card offers only Approve and Deny.
                rule: tool.rule.as_deref().filter(|rule| !rule.is_empty()),
                routine: false,
            },
            ApprovalData::Routine(routine) => Card {
                approval_id: &routine.approval_id,
                tool: &routine.tool,
                detail: &routine.detail,
                status: routine.status,
                rule: None,
                routine: true,
            },
        }
    }
}

/// The autonomy chip's word.
pub fn autonomy_word(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Ask => "Ask",
        Autonomy::Folder => "Folder",
        Autonomy::Full => "Full",
    }
}

/// The answered card's record: what happened, then what it was about.
pub fn record(summary: &str, data: &ApprovalData, decision: Option<ApprovalDecision>) -> String {
    let card = Card::of(data);
    let allowed = card.status == ApprovalStatus::Allowed;
    let label = match (card.routine, allowed, decision) {
        (true, true, _) => "Routine added",
        (true, false, _) => "Dismissed",
        (false, false, _) => "Denied",
        (false, true, Some(ApprovalDecision::Always)) => "Always allowed",
        (false, true, _) => "Approved",
    };
    // A routine card's summary is the daemon's proposal ("Add a routine: Morning digest, ..."); once answered, the
    // label says what happened to it.
    let subject = if card.routine {
        summary.strip_prefix("Add a routine: ").unwrap_or(summary)
    } else {
        summary
    };
    format!("{label}: {subject}")
}

/// An approval message: the card while it waits, its record once answered.
pub fn card(
    message: &Message,
    data: &ApprovalData,
    bot: Option<&Bot>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let fields = Card::of(data);
    if fields.status != ApprovalStatus::Pending {
        let decision = cx
            .try_global::<Decided>()
            .and_then(|decided| decided.0.get(fields.approval_id).copied());
        return answered(
            &message.id,
            &record(&message.body, data, decision),
            fields.status,
            cx,
        );
    }
    pending(message, &fields, bot, window, cx)
}

fn answered(id: &str, text: &str, status: ApprovalStatus, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let text = SharedString::from(text.to_owned());
    let icon = if status == ApprovalStatus::Allowed {
        "check"
    } else {
        "x"
    };
    div()
        .semantic(format!("approval.{id}"), Role::Status)
        .aria_label(text.clone())
        .max_w_full()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .px(r(theme.space.s12))
        .py(r(theme.space.s6))
        .rounded_full()
        .border_1()
        .border_color(hsla(colors.line.base))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.secondary))
        .child(
            svg()
                .path(format!("icons/i/200/{icon}.svg"))
                .size(r(14.))
                .flex_none()
                .text_color(hsla(colors.text.primary)),
        )
        .child(div().min_w_0().truncate().child(text))
        .into_any_element()
}

fn pending(
    message: &Message,
    card: &Card,
    bot: Option<&Bot>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = message.id.clone();
    let answer: Entity<Answer> = window.use_keyed_state(
        SharedString::from(format!("approval.{id}.answer")),
        cx,
        |_, _| Answer::default(),
    );
    let (busy, problem) = {
        let answer = answer.read(cx);
        (answer.busy, answer.problem.clone())
    };
    let theme = theme(cx);
    let colors = palette(cx);
    let detail = if card.routine {
        format!("It will: {}", card.detail)
    } else {
        approval_detail(card.detail)
    };
    let answer_with = |decision: ApprovalDecision| {
        let answer = answer.clone();
        let approval_id = card.approval_id.to_owned();
        move |_: &mut Window, cx: &mut App| {
            if !answer.read(cx).busy {
                respond(&answer, &approval_id, decision, cx);
            }
        }
    };
    let (yes, no) = if card.routine {
        ("Add routine", "Dismiss")
    } else {
        ("Approve", "Deny")
    };
    let rule_line = match (card.routine, card.rule, bot) {
        (false, Some(rule), Some(bot)) => Some(format!(
            "Always allow saves: {}, for {} only",
            approval_rule(rule),
            bot.name
        )),
        (false, None, _) => Some("Always allow isn't available for this request yet.".to_owned()),
        _ => None,
    };
    let title = SharedString::from(message.body.clone());
    div()
        .semantic(format!("approval.{id}"), Role::Group)
        .aria_label("Needs you")
        .w_full()
        .max_w(r(600.))
        .flex()
        .flex_col()
        .gap(r(theme.space.s10))
        .px(r(theme.space.s16))
        .py(r(theme.space.s14))
        .bg(hsla(colors.surface.raised))
        .rounded(r(theme.radius.slot))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .shadow(shadows(&theme.shadow.light.raised))
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .child(
                    div()
                        .semantic(format!("approval.{id}.title"), Role::Heading)
                        .aria_label(title.clone())
                        .flex_1()
                        .min_w_0()
                        .type_style(&theme.typography.desktop.body_strong)
                        .text_color(hsla(colors.text.primary))
                        .child(title),
                )
                .when_some(bot, |row, bot| {
                    row.child(
                        div()
                            .flex_none()
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(colors.text.tertiary))
                            .child(format!("Autonomy: {}", autonomy_word(bot.autonomy))),
                    )
                })
                .child(needs_you(cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(r(theme.space.s16))
                .child(
                    div()
                        .semantic(format!("approval.{id}.detail"), Role::Label)
                        .aria_label(SharedString::from(detail.clone()))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s10))
                        .px(r(theme.space.s12))
                        .py(r(theme.space.s10))
                        .bg(hsla(colors.surface.panel))
                        .rounded(r(theme.radius.field))
                        .type_style(&theme.typography.desktop.code)
                        .text_color(hsla(colors.text.primary))
                        .child(
                            svg()
                                .path(format!(
                                    "icons/i/175/{}.svg",
                                    if card.routine {
                                        "calendar-clock"
                                    } else {
                                        tool_icon(card.tool)
                                    }
                                ))
                                .size(r(15.))
                                .flex_none()
                                .text_color(hsla(colors.icon.muted)),
                        )
                        .child(div().min_w_0().truncate().child(SharedString::from(detail))),
                )
                .when(!busy, |row| {
                    row.child(
                        TextLink::new(format!("approval.{id}.deny"), no)
                            .on_press(answer_with(ApprovalDecision::Deny)),
                    )
                    .when(card.rule.is_some(), |row| {
                        row.child(
                            TextLink::new(format!("approval.{id}.always"), "Always allow")
                                .on_press(answer_with(ApprovalDecision::Always)),
                        )
                    })
                    .child(
                        Pill::new(format!("approval.{id}.approve"), yes)
                            .variant(PillVariant::Primary)
                            .on_press(answer_with(ApprovalDecision::Once)),
                    )
                }),
        )
        .when_some(rule_line, |card, line| {
            card.child(
                div()
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(colors.text.tertiary))
                    .child(line),
            )
        })
        .when_some(problem, |card, problem| {
            card.child(
                div()
                    .semantic(format!("approval.{id}.problem"), Role::Alert)
                    .aria_label(problem.clone())
                    .type_style(&theme.typography.desktop.small)
                    .text_color(hsla(theme.color.bot.red))
                    .child(problem),
            )
        })
        .into_any_element()
}

/// The ochre "Needs you" pill: something is waiting for you.
fn needs_you(cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .flex_none()
        .h(r(24.))
        .px(r(theme.space.s10))
        .flex()
        .items_center()
        .gap(r(theme.space.s6))
        .rounded_full()
        .bg(hsla(colors.accent.tint))
        .type_style(&theme.typography.desktop.label)
        .text_color(hsla(colors.accent.text))
        .child(
            div()
                .size(r(7.))
                .rounded_full()
                .bg(hsla(colors.accent.marker)),
        )
        .child("Needs you")
        .into_any_element()
}

/// Sends the answer. The buttons lock until the daemon has it; a failure unlocks them and says why.
fn respond(answer: &Entity<Answer>, approval_id: &str, decision: ApprovalDecision, cx: &mut App) {
    answer.update(cx, |answer, cx| {
        answer.busy = true;
        answer.problem = None;
        cx.notify();
    });
    cx.default_global::<Decided>()
        .0
        .insert(approval_id.to_owned(), decision);
    let call = store::request::<method::ApprovalsRespond>(
        cx,
        ApprovalsRespond {
            approval_id: approval_id.to_owned(),
            decision,
        },
    );
    let answer = answer.clone();
    let approval_id = approval_id.to_owned();
    cx.spawn(async move |cx| {
        guarded_async("answer an approval", async move {
            if let Err(error) = call.await {
                cx.update(|cx| {
                    cx.default_global::<Decided>().0.remove(&approval_id);
                    answer.update(cx, |answer, cx| {
                        answer.busy = false;
                        answer.problem = Some(error_text(&error).into());
                        cx.notify();
                    });
                });
            }
        })
        .await;
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::model::{RoutineApprovalData, RoutineKind, ToolApprovalData};

    fn tool(status: ApprovalStatus) -> ApprovalData {
        ApprovalData::Tool(ToolApprovalData {
            approval_id: "a1".into(),
            tool: "Bash".into(),
            detail: "{\"command\":\"git push\"}".into(),
            status,
            kind: None,
            rule: Some("Bash(git push:*)".into()),
        })
    }

    fn routine(status: ApprovalStatus) -> ApprovalData {
        ApprovalData::Routine(RoutineApprovalData {
            approval_id: "a2".into(),
            tool: "add_routine".into(),
            detail: "Every weekday at 9:00".into(),
            status,
            kind: RoutineKind::Routine,
        })
    }

    #[test]
    fn an_answered_card_says_what_happened() {
        let summary = "Forge wants to run a command";
        assert_eq!(
            record(
                summary,
                &tool(ApprovalStatus::Allowed),
                Some(ApprovalDecision::Always)
            ),
            "Always allowed: Forge wants to run a command"
        );
        assert_eq!(
            record(
                summary,
                &tool(ApprovalStatus::Allowed),
                Some(ApprovalDecision::Once)
            ),
            "Approved: Forge wants to run a command"
        );
        // Answered elsewhere: this window doesn't know how.
        assert_eq!(
            record(summary, &tool(ApprovalStatus::Allowed), None),
            "Approved: Forge wants to run a command"
        );
        assert_eq!(
            record(
                summary,
                &tool(ApprovalStatus::Denied),
                Some(ApprovalDecision::Deny)
            ),
            "Denied: Forge wants to run a command"
        );
        let proposal = "Add a routine: Morning digest, every weekday";
        assert_eq!(
            record(proposal, &routine(ApprovalStatus::Allowed), None),
            "Routine added: Morning digest, every weekday"
        );
        assert_eq!(
            record(proposal, &routine(ApprovalStatus::Denied), None),
            "Dismissed: Morning digest, every weekday"
        );
    }
}
