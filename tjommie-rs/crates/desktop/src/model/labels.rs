//! Port of `packages/ui/lib/labels.ts`: names and one-line descriptions of bots, models, states and errors. The
//! TypeScript `STATE_TEXT` (Tailwind class names) is a styling choice and is left to the screens (M5).

use jiff::tz::TimeZone;
use protocol::model::{Autonomy, Bot, BotState, Provider};

use super::format::clock;

pub const DELETED_BOT: &str = "A deleted agent";

/// The bot's name, or [`DELETED_BOT`] when `id` is no longer in `bots`.
pub fn bot_name<'a>(bots: &'a [Bot], id: &str) -> &'a str {
    bots.iter()
        .find(|bot| bot.id == id)
        .map_or(DELETED_BOT, |bot| bot.name.as_str())
}

/// The bot summary's autonomy line.
pub fn autonomy_line(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Ask => "Always ask: asks before every change",
        Autonomy::Folder => "Own folder: works freely in its folder, asks before anything else",
        Autonomy::Full => "Full auto: never asks",
    }
}

/// The empty chat's second sentence.
pub fn autonomy_hint(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Ask => "It asks before every change.",
        Autonomy::Folder => "It works in its own folder and asks before touching anything else.",
        Autonomy::Full => "It works without asking first.",
    }
}

/// "Claude · Sonnet (latest)", "Codex · gpt-6-astra".
pub fn model_label(provider: Provider, model: &str) -> String {
    match provider {
        Provider::Claude => {
            let name = match model {
                "sonnet" => "Sonnet (latest)",
                "opus" => "Opus (latest)",
                "haiku" => "Haiku (latest)",
                other => other,
            };
            format!("Claude · {name}")
        }
        Provider::Codex => format!("Codex · {model}"),
    }
}

/// "Working", "Waiting for you", "Limited until 3:45 PM"...
pub fn state_label(state: BotState, limited_until: Option<&str>, zone: &TimeZone) -> String {
    match state {
        BotState::Working => "Working".to_owned(),
        BotState::Waiting => "Waiting for you".to_owned(),
        BotState::Paused => "Paused".to_owned(),
        BotState::Limited => match limited_until {
            Some(until) => format!("Limited until {}", clock(until, zone)),
            None => "Limited".to_owned(),
        },
        BotState::NeedsSetup => "Needs setup".to_owned(),
        BotState::Idle => "Idle".to_owned(),
    }
}

/// The short state, for tight rows (the mention popover, the Agents overview's chips).
pub fn short_state(state: BotState) -> &'static str {
    match state {
        BotState::Idle => "Idle",
        BotState::Working => "Working",
        BotState::Waiting => "Waiting",
        BotState::Paused => "Paused",
        BotState::Limited => "Limited",
        BotState::NeedsSetup => "Needs setup",
    }
}

/// A daemon error as a sentence. The client's errors start with their code ("bad_params: ...").
pub fn error_text(error: &impl std::fmt::Display) -> String {
    let text = error.to_string();
    match text.split_once(": ") {
        Some((code, rest))
            if !code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            rest.to_owned()
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, name: &str) -> Bot {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "color": "orange", "shape": "circle", "instructions": "",
            "provider": "claude", "model": "sonnet", "autonomy": "ask", "homeFolder": "C:/b",
            "projectFolder": null, "browser": false, "paused": false, "canMessage": [],
            "state": "idle", "setupReason": null, "limitedUntil": null, "createdAt": "2026-09-27T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn names_bots_models_and_states_and_says_when_a_bot_is_gone() {
        let bots = [bot("b1", "Pnet apply")];
        assert_eq!(bot_name(&bots, "b1"), "Pnet apply");
        assert_eq!(bot_name(&bots, "gone"), "A deleted agent");
        assert_eq!(
            model_label(Provider::Claude, "sonnet"),
            "Claude · Sonnet (latest)"
        );
        assert_eq!(
            model_label(Provider::Codex, "gpt-6-astra"),
            "Codex · gpt-6-astra"
        );
        assert_eq!(
            state_label(
                BotState::Limited,
                Some("2026-09-27T15:45:00Z"),
                &TimeZone::UTC
            ),
            "Limited until 3:45 PM"
        );
        assert_eq!(
            state_label(BotState::Waiting, None, &TimeZone::UTC),
            "Waiting for you"
        );
        let refused = "bad_params: This agent's browser is off";
        assert_eq!(error_text(&refused), "This agent's browser is off");
        assert_eq!(
            error_text(&"busy: Scout is still working"),
            "Scout is still working"
        );
        assert_eq!(error_text(&"No code here"), "No code here");
    }
}
