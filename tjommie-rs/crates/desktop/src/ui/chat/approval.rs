//! The approval card (Phase 3 plan T4.2; chat-cards.html `.approval`, `.answered`; `approval-card.tsx`). The one
//! raised card in a thread: what the agent wants, the exact command, its autonomy, and the answers. Approve is the
//! near-black pill and Deny an underlined text action, so the safe answer is never the loud one by accident. When the
//! request names a rule, Always allow saves it for this agent; a routine proposal is added or dismissed. The answers
//! lock after one press.
//!
//! The Approve motion (aegis/README.md, signature moment 2): the card stamps down on the playful spring while its edge
//! flashes ochre and settles to ink; after a 260 ms beat it closes into its one-line record, whose check draws on.
//! Deny lets go quietly, with no stamp. A key (Ctrl+Enter in the card, or Enter on Approve) snaps: the record shows at
//! once, as under reduced motion. An answer the daemon refuses undoes the motion from wherever it is.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, FocusHandle, Global, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px, svg,
};
use protocol::model::{ApprovalData, ApprovalDecision, ApprovalStatus, Autonomy, Bot, Message};
use protocol::wire::{ApprovalsRespond, method};

use super::rows::tool_icon;
use crate::guard::{guarded, guarded_async};
use crate::keymap::{self, Approve};
use crate::model::approval::{approval_detail, approval_rule};
use crate::model::labels::error_text;
use crate::motion::check::{DrawOn, check};
use crate::motion::keyboard::Trigger;
use crate::motion::spring::Spring;
use crate::motion::{clock, reduced};
use crate::semantic::SemanticExt as _;
use crate::theme;
use crate::ui::primitives::button::{Pill, PillVariant, TextLink};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, focus_ring, focus_visible, hsla, palette, r, shadows, theme};

/// The beat between the stamp and the close.
const BEAT: f32 = 0.26;
/// The stamp's kick, in px per second: on the playful spring (380, 0.7) the card dips about 3 px.
const STAMP_KICK: f32 = 128.;

/// Answers given in this window, so an answered card can say "Always allowed".
#[derive(Default)]
struct Decided(HashMap<String, ApprovalDecision>);

impl Global for Decided {}

/// The Approve motion, as plain state: the time comes in, so it is tested without a window.
pub struct Approving {
    started: f32,
    stamp: Spring,
    snapped: bool,
    undone: bool,
    check: DrawOn,
    /// The record's arrival where the card was, in percent on the calm spring (the spring rests within a quarter of a
    /// unit, so a 0-1 range would stop short).
    arrival: Spring,
}

impl Approving {
    /// The motion from `now`. `snapped`: a key or reduced motion, so it is already closed.
    pub fn start(now: f32, snapped: bool, motion: &theme::Motion) -> Self {
        let mut stamp = Spring::new(0., &motion.spring.playful);
        let mut check = DrawOn::new(&motion.spring.calm);
        let arrival = Spring::new(if snapped { 100. } else { 0. }, &motion.spring.calm);
        if snapped {
            check.draw(true);
        } else {
            stamp.velocity = STAMP_KICK;
        }
        Approving {
            started: now,
            stamp,
            snapped,
            undone: false,
            check,
            arrival,
        }
    }

    /// Advances by `dt` seconds at `now`. Returns whether anything still moves.
    pub fn step(&mut self, now: f32, dt: f32) -> bool {
        let stamping = self.stamp.step(dt);
        if self.closed(now) {
            self.check.draw(false);
            self.arrival.set_target(100.);
        }
        let drawing = self.check.step(dt);
        let arriving = self.arrival.step(dt);
        stamping
            || drawing
            || arriving
            || (!self.undone && !self.snapped && now - self.started < BEAT)
    }

    /// How far down the card is, in px.
    pub fn offset(&self) -> f32 {
        self.stamp.value
    }

    /// The ochre edge's strength: 1 at the press, settled to ink by the end of the beat, an undo included.
    pub fn flash(&self, now: f32) -> f32 {
        if self.snapped {
            return 0.;
        }
        (1. - (now - self.started) / BEAT).clamp(0., 1.)
    }

    /// Whether the card has closed into its record.
    pub fn closed(&self, now: f32) -> bool {
        !self.undone && (self.snapped || now - self.started >= BEAT)
    }

    /// The check's drawn fraction.
    pub fn check_progress(&self) -> f32 {
        self.check.progress()
    }

    /// How far the record has arrived, 0 to 1: its opacity where the card was.
    pub fn arrival(&self) -> f32 {
        (self.arrival.value / 100.).clamp(0., 1.)
    }

    /// The daemon refused the answer: the card comes back from wherever it is (the stamp keeps its velocity).
    pub fn undo(&mut self) {
        self.undone = true;
    }
}

/// What a card's answer is doing: locked while it is sent, what went wrong with it, and its motion.
#[derive(Default)]
struct Answer {
    busy: bool,
    problem: Option<SharedString>,
    decision: Option<ApprovalDecision>,
    approving: Option<Approving>,
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
    record_of(
        summary,
        card.routine,
        card.status == ApprovalStatus::Allowed,
        decision,
    )
}

fn record_of(
    summary: &str,
    routine: bool,
    allowed: bool,
    decision: Option<ApprovalDecision>,
) -> String {
    let label = match (routine, allowed, decision) {
        (true, true, _) => "Routine added",
        (true, false, _) => "Dismissed",
        (false, false, _) => "Denied",
        (false, true, Some(ApprovalDecision::Always)) => "Always allowed",
        (false, true, _) => "Approved",
    };
    // A routine card's summary is the daemon's proposal ("Add a routine: Morning digest, ..."); once answered, the
    // label says what happened to it.
    let subject = if routine {
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
        let text = record(&message.body, data, decision);
        return answered(
            &message.id,
            &text,
            fields.status == ApprovalStatus::Allowed,
            None,
            cx,
        );
    }
    pending(message, &fields, bot, window, cx)
}

/// The one-line record. `check` is the drawn fraction of an approval's check while it draws on.
fn answered(
    id: &str,
    text: &str,
    allowed: bool,
    check_progress: Option<f32>,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    let text = SharedString::from(text.to_owned());
    let mark = match (allowed, check_progress) {
        (true, Some(progress)) => {
            check(progress, 14., hsla(colors.text.primary)).into_any_element()
        }
        _ => svg()
            .path(format!(
                "icons/i/200/{}.svg",
                if allowed { "check" } else { "x" }
            ))
            .size(r(14.))
            .flex_none()
            .text_color(hsla(colors.text.primary))
            .into_any_element(),
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
        .child(mark)
        .child(div().min_w_0().truncate().child(text))
        .into_any_element()
}

/// The ochre edge, `strength` of the way from ink.
fn edge(strength: f32, cx: &App) -> Hsla {
    let colors = palette(cx);
    let (ochre, ink) = (hsla(colors.accent.base), hsla(colors.ink.base));
    let mix = |a: f32, b: f32| a * strength + b * (1. - strength);
    Hsla {
        h: if strength > 0.5 { ochre.h } else { ink.h },
        s: mix(ochre.s, ink.s),
        l: mix(ochre.l, ink.l),
        a: strength,
    }
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
    let focus: Entity<FocusHandle> = window.use_keyed_state(
        SharedString::from(format!("approval.{id}.focus")),
        cx,
        |_, cx| cx.focus_handle(),
    );
    let focus = focus.read(cx).clone();

    // The motion's frame.
    let now = clock::now(cx);
    let dt = clock::dt(cx);
    let moving = answer.update(cx, |answer, _| {
        answer
            .approving
            .as_mut()
            .is_some_and(|approving| approving.step(now, dt))
    });
    if moving {
        clock::keep_moving(window, cx);
    }
    let (busy, problem, decision, offset, flash, closed, progress, arrival) = {
        let answer = answer.read(cx);
        let approving = answer.approving.as_ref();
        (
            answer.busy,
            answer.problem.clone(),
            answer.decision,
            approving.map_or(0., Approving::offset),
            approving.map_or(0., |approving| approving.flash(now)),
            approving.is_some_and(|approving| approving.closed(now)),
            approving.map(Approving::check_progress),
            approving.map_or(1., Approving::arrival),
        )
    };
    // Closed before the daemon has recorded the answer: the record says what was answered, arriving on the calm spring.
    if closed && let Some(decision) = decision {
        let allowed = decision != ApprovalDecision::Deny;
        let text = record_of(&message.body, card.routine, allowed, Some(decision));
        return div()
            .opacity(arrival)
            .child(answered(&id, &text, allowed, progress, cx))
            .into_any_element();
    }

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
        move |window: &mut Window, cx: &mut App| {
            if !answer.read(cx).busy {
                respond(&answer, &approval_id, decision, Trigger::of(window), cx);
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
    let ring = focus_visible(&focus, window);
    let on_key = answer_with(ApprovalDecision::Once);
    div()
        .semantic(format!("approval.{id}"), Role::Group)
        .aria_label("Needs you")
        .key_context(keymap::APPROVAL)
        .track_focus(&focus)
        .on_action(move |_: &Approve, window, cx| {
            guarded("approve by key", || on_key(window, cx));
        })
        .relative()
        .top(px(offset))
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
        .when(flash > 0., |card| {
            card.border_1().border_color(edge(flash, cx))
        })
        .when(ring, |card| card.child(focus_ring(theme.radius.slot, cx)))
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

/// Sends the answer and starts its motion: Approve stamps (or snaps after a key); Deny lets go without one. The buttons
/// lock until the daemon has it; a refusal undoes the motion, unlocks them and says why.
fn respond(
    answer: &Entity<Answer>,
    approval_id: &str,
    decision: ApprovalDecision,
    trigger: Trigger,
    cx: &mut App,
) {
    let snapped = trigger.snaps(cx) || reduced::is_on(cx) || decision == ApprovalDecision::Deny;
    let now = clock::now(cx);
    let motion = theme(cx).motion.clone();
    answer.update(cx, |answer, cx| {
        answer.busy = true;
        answer.problem = None;
        answer.decision = Some(decision);
        answer.approving = Some(Approving::start(now, snapped, &motion));
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
                        if let Some(approving) = &mut answer.approving {
                            approving.undo();
                        }
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

    fn motion() -> theme::Motion {
        theme::Theme::aegis().unwrap().motion
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

    #[test]
    fn the_stamp_dips_about_three_px_flashes_and_closes_after_the_beat() {
        let motion = motion();
        let mut approving = Approving::start(0., false, &motion);
        let (mut deepest, mut now) = (0f32, 0f32);
        while now < 1. {
            now += 1. / 60.;
            approving.step(now, 1. / 60.);
            deepest = deepest.max(approving.offset());
            assert_eq!(
                approving.closed(now),
                now >= BEAT,
                "closed only after the beat, at {now}"
            );
        }
        assert!((2.5..=3.5).contains(&deepest), "dipped {deepest} px");
        assert!(approving.offset().abs() < 0.3, "and settled");
        assert_eq!(
            approving.flash(BEAT),
            0.,
            "the edge is ink by the end of the beat"
        );
        assert!(
            (approving.check_progress() - 1.).abs() < 0.01,
            "the check drew on"
        );
        assert!(
            (approving.arrival() - 1.).abs() < 0.01,
            "and the record has arrived"
        );
    }

    #[test]
    fn the_record_arrives_on_the_calm_spring_after_the_beat() {
        let motion = motion();
        let mut approving = Approving::start(0., false, &motion);
        let frame = 1. / 60.;
        let mut now = 0.;
        let mut seen = Vec::new();
        while now < BEAT + 0.2 {
            now += frame;
            approving.step(now, frame);
            seen.push((now, approving.arrival()));
        }
        assert!(
            seen.iter()
                .filter(|(at, _)| *at < BEAT)
                .all(|(_, arrival)| *arrival == 0.),
            "nothing arrives before the beat"
        );
        let after: Vec<f32> = seen
            .iter()
            .filter(|(at, _)| *at >= BEAT)
            .map(|(_, a)| *a)
            .collect();
        assert!(
            after.windows(2).all(|pair| pair[1] >= pair[0]),
            "it only rises"
        );
        assert!(
            after.last().is_some_and(|last| (0.2..1.).contains(last)),
            "still arriving 200 ms on: {after:?}"
        );
    }

    #[test]
    fn interrupt_undo_halfway() {
        let motion = motion();
        let mut approving = Approving::start(0., false, &motion);
        let frame = 1. / 60.;
        let (mut now, mut before, mut largest) = (0f32, 0f32, 0f32);
        while now < BEAT / 2. {
            now += frame;
            approving.step(now, frame);
            largest = largest.max((approving.offset() - before).abs());
            before = approving.offset();
        }
        let flash = approving.flash(now);
        // Halfway through the beat the daemon refuses the answer.
        approving.undo();
        now += frame;
        approving.step(now, frame);
        assert!(
            (approving.offset() - before).abs() <= largest + 0.5,
            "the card turns from where it was: {before} -> {}",
            approving.offset()
        );
        assert!(
            approving.flash(now) <= flash,
            "the edge keeps settling, with no jump back to ochre"
        );
        while now < 2. {
            now += frame;
            approving.step(now, frame);
            assert!(!approving.closed(now), "an undone card never closes");
        }
        assert!(approving.offset().abs() < 0.3, "it came back to rest");
    }

    #[test]
    fn a_key_snaps_with_no_frames_in_between() {
        let approving = Approving::start(0., true, &motion());
        assert!(approving.closed(0.), "closed at once");
        assert_eq!(approving.offset(), 0.);
        assert_eq!(approving.flash(0.), 0.);
        assert_eq!(approving.check_progress(), 1., "the check is whole");
        assert_eq!(approving.arrival(), 1., "and the record is there");
    }

    mod views {
        use gpui::TestAppContext;
        use serde_json::json;

        use crate::model::testing::FakeRpc;
        use crate::ui::chat::tests::{calls_to, chat, message, with};
        use crate::ui::shell::route::Route;
        use crate::ui::testing::{app, click, draw, node};
        use protocol::model::{ApprovalDecision, MessageKind};

        fn pending_card() -> protocol::model::Message {
            with(
                MessageKind::Approval,
                json!({ "approvalId": "a1", "tool": "Bash", "detail": "{\"command\":\"git push\"}", "status": "pending", "rule": null }),
                message("m1", "b1", "Forge wants to run a command", 1),
            )
        }

        #[gpui::test]
        fn keyboard_snaps(cx: &mut TestAppContext) {
            // The fake has no answer for approvals.respond, so the daemon refuses the answer.
            let rpc = FakeRpc::new();
            let (cx, calls) = app(
                cx,
                Route::Chat("b1".into()),
                chat(vec![pending_card()], false),
                rpc,
            );
            assert!(node("approval.m1.approve").is_some());
            // Focus the card, then Ctrl+Enter: the answer goes, and the record shows on the very next frame.
            click(cx, "approval.m1.title");
            cx.update(|window, cx| {
                window.dispatch_keystroke(gpui::Keystroke::parse("ctrl-enter").unwrap(), cx);
                window.draw(cx).clear(cx);
            });
            assert_eq!(
                node("approval.m1").map(|node| node.text),
                Some("Approved: Forge wants to run a command".into()),
                "snapped: no stamp, no beat"
            );
            draw(cx);
            let answers = calls_to(&calls, "approvals.respond");
            assert_eq!(answers.len(), 1);
            assert_eq!(answers[0]["decision"], json!(ApprovalDecision::Once));
            // The daemon refused it: the card is back, saying why.
            assert!(node("approval.m1.problem").is_some());
            assert!(node("approval.m1.approve").is_some());
        }
    }
}
