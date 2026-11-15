//! Port of `packages/ui/lib/overview.ts`: the Agents overview. Its filters, its summary and lede, and the line each bot and
//! room shows. The TypeScript's `Pick<Bot, ...>` arguments are the whole protocol `Bot` here.

use jiff::{Timestamp, Zoned};
use protocol::model::{
    ActivityStatus, ApprovalData, ApprovalStatus, Bot, BotState, HandoffData, Message, MessageData,
    MessageKind, Routine, TextData, message_data,
};

use super::format::{clock, day_name, time_ago};
use super::labels::{bot_name, state_label};

/// The filter chips in the order they show, with their labels.
const FILTERS: [(OverviewFilter, &str); 4] = [
    (OverviewFilter::All, "All"),
    (OverviewFilter::Working, "Working"),
    (OverviewFilter::Waiting, "Waiting"),
    (OverviewFilter::Paused, "Paused"),
];

/// The overview's filter. `All` shows every bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewFilter {
    All,
    Working,
    Waiting,
    Paused,
}

/// A phone filter chip: its filter, its label, and how many bots it would show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneChip {
    pub filter: OverviewFilter,
    pub label: &'static str,
    pub count: usize,
}

/// The phone's filter chips. A state no bot is in has no chip (unless it's the picked one, so it can be left), and
/// when no chip would narrow the list there are none at all.
/// Whether a bot in `state` is shown under `filter`.
fn in_filter(filter: OverviewFilter, state: BotState) -> bool {
    match filter {
        OverviewFilter::All => true,
        OverviewFilter::Working => state == BotState::Working,
        OverviewFilter::Waiting => state == BotState::Waiting,
        OverviewFilter::Paused => state == BotState::Paused,
    }
}

/// The phone's filter chips. A state no bot is in has no chip (unless it's the picked one, so it can be left), and
/// when no chip would narrow the list there are none at all.
pub fn phone_filters(bots: &[Bot], current: OverviewFilter) -> Vec<PhoneChip> {
    let chips: Vec<PhoneChip> = FILTERS
        .iter()
        .map(|&(filter, label)| PhoneChip {
            filter,
            label,
            count: bots
                .iter()
                .filter(|bot| in_filter(filter, bot.state))
                .count(),
        })
        .collect();
    let narrowing = chips.iter().any(|chip| {
        chip.filter != OverviewFilter::All && chip.count > 0 && chip.count < bots.len()
    });
    if !narrowing && current == OverviewFilter::All {
        return Vec::new();
    }
    chips
        .into_iter()
        .filter(|chip| {
            chip.filter == OverviewFilter::All || chip.count > 0 || chip.filter == current
        })
        .collect()
}

/// Folio's one-sentence lede under the Agents title: "2 agents are working; Nova is waiting for you."
pub fn overview_lede(bots: &[Bot]) -> String {
    if bots.is_empty() {
        return "No agents yet.".to_owned();
    }
    let working: Vec<&Bot> = bots
        .iter()
        .filter(|bot| bot.state == BotState::Working)
        .collect();
    let waiting: Vec<&Bot> = bots
        .iter()
        .filter(|bot| bot.state == BotState::Waiting)
        .collect();
    let mut parts = Vec::new();
    if let Some(first) = working.first() {
        parts.push(match working.len() {
            1 => format!("{} is working", first.name),
            count => format!("{count} agents are working"),
        });
    }
    if let Some(first) = waiting.first() {
        parts.push(match waiting.len() {
            1 => format!("{} is waiting for you", first.name),
            count => format!("{count} agents are waiting for you"),
        });
    }
    if parts.is_empty() {
        return "All quiet.".to_owned();
    }
    // Capitalise the first letter; the rest is as written.
    let sentence = parts.join("; ");
    let mut chars = sentence.chars();
    let head: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    format!("{head}{}.", chars.as_str())
}

/// "6 agents, 2 working, 1 waiting for you".
pub fn overview_summary(bots: &[Bot]) -> String {
    let count = |state: BotState| bots.iter().filter(|bot| bot.state == state).count();
    let mut parts = vec![format!(
        "{} {}",
        bots.len(),
        if bots.len() == 1 { "agent" } else { "agents" }
    )];
    let working = count(BotState::Working);
    if working > 0 {
        parts.push(format!("{working} working"));
    }
    let waiting = count(BotState::Waiting);
    if waiting > 0 {
        parts.push(format!("{waiting} waiting for you"));
    }
    parts.join(", ")
}

/// Whether a bot is shown under `filter` and for `query` (its name contains the query, any case, after trimming).
pub fn matches(bot: &Bot, filter: OverviewFilter, query: &str) -> bool {
    let typed = query.trim().to_lowercase();
    if !typed.is_empty() && !bot.name.to_lowercase().contains(typed.as_str()) {
        return false;
    }
    in_filter(filter, bot.state)
}

/// The first line with words in it, trimmed.
fn first_line(text: &str) -> &str {
    text.split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// The line for a pending approval: its body when that has a space in it, otherwise the tool and the body. Claude
/// describes a Write by its file name alone, so "hello.txt" reads as "Write: hello.txt".
fn pending_line(card: &ApprovalData, body: &str) -> Option<String> {
    let (tool, status) = match card {
        ApprovalData::Tool(card) => (card.tool.as_str(), card.status),
        ApprovalData::Routine(card) => (card.tool.as_str(), card.status),
    };
    if status != ApprovalStatus::Pending {
        return None;
    }
    Some(if body.trim().contains(char::is_whitespace) {
        body.to_owned()
    } else {
        format!("{tool}: {body}")
    })
}

/// What the bot is doing, in one line, from its newest messages (WF2's task line). `now` gives the zone a limited bot's
/// time is read in.
pub fn task_line(bot: &Bot, messages: &[Message], now: &Zoned) -> String {
    match bot.state {
        BotState::Paused => "Paused by you".to_owned(),
        BotState::Limited => state_label(bot.state, bot.limited_until.as_deref(), now.time_zone()),
        BotState::NeedsSetup => bot
            .setup_reason
            .clone()
            .unwrap_or_else(|| "Needs setup".to_owned()),
        BotState::Waiting => messages
            .iter()
            .rev()
            .find_map(|message| match message_data(message) {
                Ok(MessageData::Approval(card)) => pending_line(&card, &message.body),
                _ => None,
            })
            .unwrap_or_else(|| "Waiting for you".to_owned()),
        BotState::Working => {
            // The newest activity row's last step, while that step is running.
            let row = messages
                .iter()
                .rev()
                .find(|message| message.kind == MessageKind::Activity);
            let step = row.and_then(|row| match message_data(row) {
                Ok(MessageData::Activity(data)) => data.items.into_iter().last(),
                _ => None,
            });
            match step {
                Some(step) if step.status == ActivityStatus::Running => step.summary,
                _ => "Working".to_owned(),
            }
        }
        BotState::Idle => messages
            .iter()
            .rev()
            .find(|message| {
                message.author == bot.id
                    && message.kind == MessageKind::Text
                    && !message.body.trim().is_empty()
            })
            .map_or_else(
                || "No messages yet".to_owned(),
                |reply| first_line(&reply.body).to_owned(),
            ),
    }
}

/// "Next: Morning check, 8:48 AM", "Next: Morning check, Tomorrow 8:48 AM", "No routine" or "Paused".
pub fn next_routine_line(bot: &Bot, routines: &[Routine], now: &Zoned) -> String {
    if bot.paused {
        return "Paused".to_owned();
    }
    // The soonest enabled routine. ISO times sort as text, and the first of equal times wins, as in a stable sort.
    let next = routines
        .iter()
        .filter(|routine| routine.bot_id == bot.id && routine.enabled)
        .filter_map(|routine| routine.next_run_at.as_deref().map(|at| (routine, at)))
        .min_by(|a, b| a.1.cmp(b.1));
    let Some((routine, at)) = next else {
        return "No routine".to_owned();
    };
    let day = day_name(at, now, false);
    let when = if day == "Today" {
        String::new()
    } else {
        format!("{day} ")
    };
    format!(
        "Next: {}, {when}{}",
        routine.name,
        clock(at, now.time_zone())
    )
}

/// The bots this bot had exchanges with today, from the handoff rows among the messages, in the order they're first seen.
pub fn talking_with(bot_id: &str, messages: &[Message], now: &Zoned) -> Vec<String> {
    let zone = now.time_zone().clone();
    let today = now.date();
    let mut partners: Vec<String> = Vec::new();
    for message in messages {
        if message.kind != MessageKind::Handoff {
            continue;
        }
        let Ok(at) = message.created_at.parse::<Timestamp>() else {
            continue;
        };
        if at.to_zoned(zone.clone()).date() != today {
            continue;
        }
        if let Ok(MessageData::Handoff(HandoffData::Exchange(row))) = message_data(message) {
            let partner = if row.from_bot_id == bot_id {
                row.to_bot_id
            } else {
                row.from_bot_id
            };
            if !partners.contains(&partner) {
                partners.push(partner);
            }
        }
    }
    partners
}

/// A room's line (its card on Agents, its row in the rooms list), from its newest post of any kind: yours, a bot's
/// answer, or a bot-to-bot exchange, which names both bots. Notices, activity and a reply still on its way aren't posts.
/// `None` messages means the posts are still being read: a blank line that keeps its height, not "No posts yet".
pub fn room_line(messages: Option<&[Message]>, bots: &[Bot], now: &Zoned) -> String {
    let Some(messages) = messages else {
        return "\u{a0}".to_owned();
    };
    let Some(post) = messages
        .iter()
        .rev()
        .find(|message| message.kind == MessageKind::Text && !message.body.trim().is_empty())
    else {
        return "No posts yet".to_owned();
    };
    let ago = time_ago(&post.created_at, now);
    if let Ok(MessageData::Text(Some(TextData::ExchangePost(exchange)))) = message_data(post) {
        return format!(
            "{} and {} talked {ago}",
            bot_name(bots, &post.author),
            bot_name(bots, &exchange.to_bot_id)
        );
    }
    if post.author == "you" {
        format!("You posted {ago}")
    } else {
        format!("{} answered {ago}", bot_name(bots, &post.author))
    }
}

#[cfg(test)]
mod tests {
    use jiff::Zoned;
    use protocol::model::{Bot, Message, MessageStatus, Routine};
    use serde_json::{Value, json};

    use super::*;

    const NINE_AM: &str = "2026-09-27T09:00:00Z";

    fn now() -> Zoned {
        "2026-09-27T14:00:00[UTC]".parse().unwrap()
    }

    /// A bot as the wire sends it, with the fields the overview reads.
    fn named_bot(
        id: &str,
        name: &str,
        state: &str,
        paused: bool,
        setup_reason: Option<&str>,
    ) -> Bot {
        serde_json::from_value(json!({
            "id": id, "name": name, "color": "orange", "shape": "circle", "instructions": "",
            "provider": "claude", "model": "sonnet", "autonomy": "ask", "homeFolder": "C:/b",
            "projectFolder": null, "browser": false, "paused": paused, "canMessage": [],
            "state": state, "setupReason": setup_reason, "limitedUntil": null,
            "createdAt": "2026-09-27T00:00:00Z"
        }))
        .unwrap()
    }

    /// "Pnet apply" (b1) in `state`, paused exactly when the state is paused.
    fn bot(state: &str) -> Bot {
        named_bot("b1", "Pnet apply", state, state == "paused", None)
    }

    fn message(
        id: &str,
        kind: &str,
        author: &str,
        body: &str,
        data: Value,
        created_at: &str,
    ) -> Message {
        serde_json::from_value(json!({
            "id": id, "conversationId": "b1", "author": author, "kind": kind, "body": body,
            "data": data, "status": "done", "createdAt": created_at
        }))
        .unwrap()
    }

    fn routine(name: &str, next_run_at: Option<&str>, enabled: bool) -> Routine {
        serde_json::from_value(json!({
            "id": name, "botId": "b1", "name": name, "prompt": "Check", "days": [1],
            "times": ["08:48"], "timeZone": "Africa/Johannesburg", "timeLimitMinutes": null,
            "enabled": enabled, "nextRunAt": next_run_at, "createdAt": "2026-09-27T14:00:00.000Z"
        }))
        .unwrap()
    }

    #[test]
    fn sums_up_the_bots_and_filters_them() {
        let bots = [
            bot("working"),
            bot("working"),
            bot("waiting"),
            bot("idle"),
            bot("idle"),
            bot("paused"),
        ];
        assert_eq!(
            overview_summary(&bots),
            "6 agents, 2 working, 1 waiting for you"
        );
        assert_eq!(overview_summary(&[bot("idle")]), "1 agent");
        let cv_paused = named_bot("b1", "CV Writer", "paused", true, None);
        assert!(matches(&cv_paused, OverviewFilter::Paused, ""));
        assert!(!matches(&cv_paused, OverviewFilter::Working, ""));
        let cv_idle = named_bot("b1", "CV Writer", "idle", false, None);
        assert!(matches(&cv_idle, OverviewFilter::All, "writ"));
        assert!(!matches(&cv_idle, OverviewFilter::All, "pnet"));
    }

    #[test]
    fn says_what_each_bot_is_doing() {
        let messages = [
            message(
                "m1",
                "text",
                "b1",
                "Morning briefing sent at 7:00 AM.\nDetails below.",
                Value::Null,
                NINE_AM,
            ),
            // Claude's description of a Write is just the file name.
            message(
                "m2",
                "approval",
                "b1",
                "hello.txt",
                json!({"approvalId": "a1", "tool": "Write", "detail": "{}", "status": "pending", "rule": "Write"}),
                NINE_AM,
            ),
            message(
                "m3",
                "activity",
                "b1",
                "2 steps",
                json!({"items": [
                    {"id": "s1", "name": "Read", "summary": "Read listing PN-48213", "status": "done"},
                    {"id": "s2", "name": "Fill", "summary": "Filling the Apply form", "status": "running"}
                ]}),
                NINE_AM,
            ),
        ];
        assert_eq!(
            task_line(&bot("idle"), &messages, &now()),
            "Morning briefing sent at 7:00 AM."
        );
        assert_eq!(
            task_line(&bot("waiting"), &messages, &now()),
            "Write: hello.txt"
        );
        let sentence = [message(
            "m4",
            "approval",
            "b1",
            "Run npm test",
            json!({"approvalId": "a2", "tool": "Shell", "detail": "npm test", "status": "pending", "rule": null}),
            NINE_AM,
        )];
        assert_eq!(
            task_line(&bot("waiting"), &sentence, &now()),
            "Run npm test"
        );
        assert_eq!(
            task_line(&bot("working"), &messages, &now()),
            "Filling the Apply form"
        );
        assert_eq!(
            task_line(&bot("paused"), &messages, &now()),
            "Paused by you"
        );
        assert_eq!(
            task_line(
                &named_bot(
                    "b1",
                    "Pnet apply",
                    "needs_setup",
                    false,
                    Some("Sign in to Claude")
                ),
                &messages,
                &now()
            ),
            "Sign in to Claude"
        );
        assert_eq!(task_line(&bot("idle"), &[], &now()), "No messages yet");
    }

    #[test]
    fn shows_the_next_routine_run() {
        let routines = [
            routine("Later", Some("2026-09-28T08:48:00Z"), true),
            routine("Soon", Some("2026-09-27T17:00:00Z"), true),
            routine("Off", Some("2026-09-27T15:00:00Z"), false),
        ];
        let idle = named_bot("b1", "Pnet apply", "idle", false, None);
        assert_eq!(
            next_routine_line(&idle, &routines, &now()),
            "Next: Soon, 5:00 PM"
        );
        assert_eq!(
            next_routine_line(&idle, &routines[..1], &now()),
            "Next: Later, Tomorrow 8:48 AM"
        );
        let paused = named_bot("b1", "Pnet apply", "paused", true, None);
        assert_eq!(next_routine_line(&paused, &routines, &now()), "Paused");
        let other = named_bot("b2", "CV Writer", "idle", false, None);
        assert_eq!(next_routine_line(&other, &routines, &now()), "No routine");
    }

    #[test]
    fn finds_todays_partners_and_the_rooms_last_exchange() {
        let handoff = |id: &str, from: &str, to: &str, at: &str| {
            message(
                id,
                "handoff",
                "b1",
                "",
                json!({"exchangeId": format!("x-{id}"), "fromBotId": from, "toBotId": to, "messages": 2, "status": "replied"}),
                at,
            )
        };
        let handoffs = [
            handoff("h1", "b1", "b2", "2026-09-27T09:31:00Z"),
            handoff("h2", "b3", "b1", "2026-09-27T09:31:00Z"),
            handoff("h3", "b1", "b4", "2026-09-26T09:00:00Z"),
        ];
        assert_eq!(talking_with("b1", &handoffs, &now()), ["b2", "b3"]);

        let bots = [
            named_bot("b1", "Pnet apply", "idle", false, None),
            named_bot("b2", "CV Writer", "idle", false, None),
        ];
        let post = message(
            "p1",
            "text",
            "b2",
            "Done.",
            json!({"exchangeId": "x1", "toBotId": "b1"}),
            "2026-09-27T13:56:00Z",
        );
        assert_eq!(
            room_line(Some(&[post][..]), &bots, &now()),
            "CV Writer and Pnet apply talked 4 minutes ago"
        );
        assert_eq!(room_line(Some(&[][..]), &bots, &now()), "No posts yet");
    }

    #[test]
    fn gives_each_room_the_line_from_its_own_posts() {
        let bots = [
            named_bot("b1", "Sales", "idle", false, None),
            named_bot("b2", "Marketing", "idle", false, None),
            named_bot("b3", "Chief", "idle", false, None),
        ];
        let post = |id: &str, from: &str, to: &str, at: &str| {
            message(
                id,
                "text",
                from,
                "Done.",
                json!({"exchangeId": format!("x-{id}"), "toBotId": to}),
                at,
            )
        };
        // Sales and Marketing are both in Growth, so their exchange is mirrored there; Chief's isn't.
        let everyone = [
            post("p1", "b1", "b2", "2026-09-27T13:00:00Z"),
            post("p2", "b3", "b1", "2026-09-27T13:50:00Z"),
        ];
        let growth = [post("p3", "b1", "b2", "2026-09-27T13:00:00Z")];
        assert_eq!(
            room_line(Some(&everyone[..]), &bots, &now()),
            "Chief and Sales talked 10 minutes ago"
        );
        assert_eq!(
            room_line(Some(&growth[..]), &bots, &now()),
            "Sales and Marketing talked 1 hour ago"
        );
    }

    #[test]
    fn gives_a_room_the_line_from_its_newest_post_of_any_kind_and_says_so_when_it_has_none() {
        let bots = [named_bot("b1", "Sales", "idle", false, None)];
        let at = |minute: u32| format!("2026-09-27T13:{minute:02}:00Z");
        let yours = message("p1", "text", "you", "@Sales hi", Value::Null, &at(50));
        let answer = message("p2", "text", "b1", "Hi.", Value::Null, &at(56));
        let exchange = message(
            "p3",
            "text",
            "b1",
            "Done.",
            json!({"exchangeId": "x1", "toBotId": "b2"}),
            &at(30),
        );
        assert_eq!(
            room_line(Some(&[exchange.clone(), yours.clone()][..]), &bots, &now()),
            "You posted 10 minutes ago"
        );
        assert_eq!(
            room_line(
                Some(&[exchange.clone(), yours.clone(), answer][..]),
                &bots,
                &now()
            ),
            "Sales answered 4 minutes ago"
        );
        assert_eq!(
            room_line(Some(&[exchange][..]), &bots, &now()),
            "Sales and A deleted agent talked 30 minutes ago"
        );
        // Notices, activity and a reply still on its way aren't posts.
        let notice = message(
            "p4",
            "notice",
            "system",
            "Sales couldn't answer.",
            Value::Null,
            NINE_AM,
        );
        let mut streaming = message("p5", "text", "b1", "", Value::Null, NINE_AM);
        streaming.status = MessageStatus::Streaming;
        assert_eq!(
            room_line(
                Some(&[notice.clone(), streaming.clone()][..]),
                &bots,
                &now()
            ),
            "No posts yet"
        );
        assert_eq!(
            room_line(Some(&[yours, notice, streaming][..]), &bots, &now()),
            "You posted 10 minutes ago"
        );
    }

    #[test]
    fn leaves_a_rooms_line_blank_while_its_posts_are_being_read_rather_than_saying_it_has_none() {
        // A blank that keeps the line's height.
        assert_eq!(room_line(None, &[], &now()), "\u{a0}");
    }

    /// Bots in the given states, named by their state.
    fn of(states: &[&str]) -> Vec<Bot> {
        states.iter().map(|state| bot(state)).collect()
    }

    fn shown(bots: &[Bot], current: OverviewFilter) -> Vec<String> {
        phone_filters(bots, current)
            .iter()
            .map(|chip| format!("{} {}", chip.label, chip.count))
            .collect()
    }

    #[test]
    fn offers_only_the_states_some_bot_is_in_with_counts() {
        assert_eq!(
            shown(&of(&["working", "waiting", "idle"]), OverviewFilter::All),
            ["All 3", "Working 1", "Waiting 1"]
        );
    }

    #[test]
    fn shows_nothing_when_no_chip_would_narrow_the_list() {
        assert!(shown(&of(&["working", "working"]), OverviewFilter::All).is_empty());
        assert!(shown(&of(&["idle"]), OverviewFilter::All).is_empty());
        assert!(shown(&[], OverviewFilter::All).is_empty());
    }

    #[test]
    fn keeps_the_picked_chip_so_a_filter_that_now_matches_nobody_can_be_left() {
        assert_eq!(
            shown(&of(&["idle", "idle"]), OverviewFilter::Paused),
            ["All 2", "Paused 0"]
        );
    }

    #[test]
    fn names_a_lone_bot_and_counts_several() {
        let lede = [
            named_bot("b1", "Nova", "waiting", false, None),
            named_bot("b2", "Ada", "working", false, None),
            named_bot("b3", "Bo", "working", false, None),
        ];
        assert_eq!(
            overview_lede(&lede),
            "2 agents are working; Nova is waiting for you."
        );
        assert_eq!(
            overview_lede(&[named_bot("b1", "Ada", "idle", false, None)]),
            "All quiet."
        );
        assert_eq!(overview_lede(&[]), "No agents yet.");
    }
}
