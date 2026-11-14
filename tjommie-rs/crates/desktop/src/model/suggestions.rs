//! Port of `packages/ui/lib/suggestions.ts` (T1.4): the New bot screen's templates. Picking one fills in the form; nothing is
//! created until Create bot, which then creates the routines too.

use protocol::model::{BotColor, BotShape};

use super::bot_form::RoutineDraft;

/// The part of a bot form a template sets (`Pick<BotForm, ...>`).
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestionForm {
    pub name: &'static str,
    pub color: BotColor,
    pub shape: BotShape,
    pub instructions: &'static str,
    pub browser: bool,
}

/// A template: a bot to fill in and the routines to create with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub title: &'static str,
    pub description: &'static str,
    pub form: SuggestionForm,
    pub routines: Vec<RoutineDraft>,
}

/// The templates (`SUGGESTIONS`, spec section 7, WF3).
pub fn suggestions() -> Vec<Suggestion> {
    let weekdays = vec![1, 2, 3, 4, 5];
    vec![
        Suggestion {
            title: "Night Shift",
            description: "Works overnight and preps your morning digest",
            form: SuggestionForm {
                name: "Night Shift",
                color: BotColor::Amber,
                shape: BotShape::Hexagon,
                instructions: "Each weekday morning, write a short digest in digest.md in your home folder: what's new in your inbox folder, what you finished yesterday, and anything I asked you to keep an eye on. Keep it to one screen, and tell me when it's ready.",
                browser: false,
            },
            routines: vec![RoutineDraft {
                name: "Morning digest".to_owned(),
                prompt: "Write today's morning digest in digest.md, then tell me it's ready in one line.".to_owned(),
                days: weekdays.clone(),
                times: vec!["06:30".to_owned()],
                time_limit_minutes: Some(60),
                enabled: true,
            }],
        },
        Suggestion {
            title: "Inbox Triage",
            description: "Sorts your email and drafts replies in your voice",
            form: SuggestionForm {
                name: "Inbox Triage",
                color: BotColor::Pink,
                shape: BotShape::Cloud,
                instructions: "Use your browser to check my email; I sign in to it there once. Sort new mail into what needs me, what can wait and what's noise. Draft replies in my voice, but never send anything without asking me first.",
                browser: true,
            },
            routines: vec![RoutineDraft {
                name: "Inbox sweep".to_owned(),
                prompt: "Check my inbox for new mail, sort it, and draft replies for anything that needs one. Don't send anything.".to_owned(),
                days: weekdays.clone(),
                times: vec!["08:00".to_owned(), "12:00".to_owned(), "16:00".to_owned()],
                time_limit_minutes: Some(60),
                enabled: true,
            }],
        },
        Suggestion {
            title: "Job hunter",
            description: "Applies to roles that match your CV every weekday",
            form: SuggestionForm {
                name: "Job hunter",
                color: BotColor::Orange,
                shape: BotShape::Cloud,
                instructions: "Find jobs that match my CV (I'll put it in your inbox folder) on the job sites I sign in to in your browser. Apply only to roles that fit, skip senior roles unless I say otherwise, and keep a list of what you applied to in applications.md.",
                browser: true,
            },
            routines: vec![RoutineDraft {
                name: "Job search".to_owned(),
                prompt: "Search for new roles that match my CV and apply to the ones that fit. Add each application to applications.md.".to_owned(),
                days: weekdays,
                times: vec!["09:00".to_owned()],
                time_limit_minutes: Some(180),
                enabled: true,
            }],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::super::bot_form::{BotForm, create_input, new_bot};
    use super::*;
    use protocol::wire::Request;
    use serde_json::{Value, json};

    /// Whether the daemon's manifest accepts `params` for `method`, as the TypeScript `parse` checks them.
    fn accepted(method: &str, params: Value) -> Result<(), String> {
        Request::decode(method, params)
            .map(|_| ())
            .map_err(|error| format!("{method}: {error:?}"))
    }

    #[test]
    fn every_suggestion_is_a_valid_bot_with_valid_routines() {
        let suggestions = suggestions();
        let titles: Vec<&str> = suggestions
            .iter()
            .map(|suggestion| suggestion.title)
            .collect();
        assert_eq!(titles, ["Night Shift", "Inbox Triage", "Job hunter"]);
        for suggestion in &suggestions {
            assert_eq!(suggestion.form.name, suggestion.title);
            // `BotInput.parse({ ...NEW_BOT, ...suggestion.form })`
            let form = BotForm {
                name: suggestion.form.name.to_owned(),
                color: suggestion.form.color,
                shape: suggestion.form.shape,
                instructions: suggestion.form.instructions.to_owned(),
                browser: suggestion.form.browser,
                ..new_bot()
            };
            let bot = serde_json::to_value(create_input(&form)).unwrap();
            assert_eq!(accepted("bots.create", bot), Ok(()));
            assert!(!suggestion.routines.is_empty());
            // `RoutineInput.parse({ botId: "b1", ...routine })`
            for routine in &suggestion.routines {
                let input = json!({
                    "botId": "b1",
                    "name": routine.name,
                    "prompt": routine.prompt,
                    "days": routine.days,
                    "times": routine.times,
                    "timeLimitMinutes": routine.time_limit_minutes,
                    "enabled": routine.enabled,
                });
                assert_eq!(accepted("routines.create", input), Ok(()));
            }
        }
    }
}
