//! Port of `packages/ui/lib/bot-form.ts` (T1.4): the bot form's values, what the person changed, and the summaries its rows
//! show. The TypeScript `Pick<Bot, ...>` parameters take whole bots here: the callers hold the daemon's list.

use protocol::model::{Autonomy, Bot, BotColor, BotInput, BotPatch, BotShape, Provider};

use super::format::{days_label, times_label};

/// The bot form's values (`BotForm`). `account_id` is `None` for the provider's default account.
#[derive(Debug, Clone, PartialEq)]
pub struct BotForm {
    pub name: String,
    pub color: BotColor,
    pub shape: BotShape,
    pub instructions: String,
    pub provider: Provider,
    pub model: String,
    pub account_id: Option<String>,
    pub autonomy: Autonomy,
    pub project_folder: Option<String>,
    pub browser: bool,
    pub can_message: Vec<String>,
}

/// A routine on a form: one the New bot screen creates with the bot, or the one the routine editor holds.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutineDraft {
    pub name: String,
    pub prompt: String,
    pub days: Vec<u8>,
    pub times: Vec<String>,
    pub time_limit_minutes: Option<u32>,
    pub enabled: bool,
}

/// The form a new agent starts from (`NEW_BOT`). A function, because its strings are allocated.
pub fn new_bot() -> BotForm {
    BotForm {
        name: String::new(),
        color: BotColor::Blue,
        shape: BotShape::Circle,
        instructions: String::new(),
        provider: Provider::Claude,
        model: "sonnet".to_owned(),
        account_id: None,
        autonomy: Autonomy::Folder,
        project_folder: None,
        browser: false,
        can_message: Vec::new(),
    }
}

/// The form a bot opens with (`formFromBot`).
pub fn form_from_bot(bot: &Bot) -> BotForm {
    BotForm {
        name: bot.name.clone(),
        color: bot.color,
        shape: bot.shape,
        instructions: bot.instructions.clone(),
        provider: bot.provider,
        model: bot.model.clone(),
        account_id: bot.account_id.clone().flatten(),
        autonomy: bot.autonomy,
        project_folder: bot.project_folder.clone(),
        browser: bot.browser,
        can_message: bot.can_message.clone(),
    }
}

/// The ids that are still bots there (`bots.some(...)` in the TypeScript).
fn still_there(ids: &[String], bots: &[Bot]) -> Vec<String> {
    ids.iter()
        .filter(|id| bots.iter().any(|bot| bot.id == **id))
        .cloned()
        .collect()
}

/// Why the name can't be saved yet, or `None`. The daemon checks again (name_taken). The length is counted in
/// JavaScript's UTF-16 units, as `String.length` counts them.
pub fn name_problem(name: &str, bots: &[Bot], self_id: Option<&str>) -> Option<String> {
    // `trim` matches JavaScript's except for U+0085 and U+FEFF, which a name doesn't carry.
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Some("Give the agent a name.".to_owned());
    }
    if trimmed.encode_utf16().count() > 40 {
        return Some("Keep the name to 40 characters or fewer.".to_owned());
    }
    let lower = trimmed.to_lowercase();
    let taken = bots
        .iter()
        .any(|bot| Some(bot.id.as_str()) != self_id && bot.name.to_lowercase() == lower);
    taken.then(|| format!("There's already an agent called {trimmed}."))
}

/// The name as the daemon takes it, trimmed.
pub fn create_input(form: &BotForm) -> BotInput {
    BotInput {
        name: form.name.trim().to_owned(),
        color: form.color,
        shape: form.shape,
        instructions: form.instructions.clone(),
        provider: form.provider,
        model: form.model.clone(),
        account_id: Some(form.account_id.clone()),
        autonomy: Some(form.autonomy),
        project_folder: Some(form.project_folder.clone()),
        browser: Some(form.browser),
        can_message: Some(form.can_message.clone()),
    }
}

/// Two id lists with the same members, in any order (and the same length).
pub fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|id| b.contains(id))
}

/// Only what the person changed, measured from the bot as the form opened (`bot`), so a save never puts back a value that
/// changed elsewhere since. Provider and model travel together. Can message counts only the bots there still are: one
/// deleted since the form opened left with its bot, so it isn't a change, and it's never sent.
pub fn patch_for(bot: &Bot, form: &BotForm, bots: &[Bot]) -> BotPatch {
    let mut patch = BotPatch::default();
    let name = form.name.trim();
    if name != bot.name {
        patch.name = Some(name.to_owned());
    }
    if form.color != bot.color {
        patch.color = Some(form.color);
    }
    if form.shape != bot.shape {
        patch.shape = Some(form.shape);
    }
    if form.instructions != bot.instructions {
        patch.instructions = Some(form.instructions.clone());
    }
    if form.provider != bot.provider || form.model != bot.model {
        patch.provider = Some(form.provider);
        patch.model = Some(form.model.clone());
    }
    if form.account_id != bot.account_id.clone().flatten() {
        patch.account_id = Some(form.account_id.clone());
    }
    if form.autonomy != bot.autonomy {
        patch.autonomy = Some(form.autonomy);
    }
    if form.project_folder != bot.project_folder {
        patch.project_folder = Some(form.project_folder.clone());
    }
    if form.browser != bot.browser {
        patch.browser = Some(form.browser);
    }
    let can_message = still_there(&form.can_message, bots);
    if !same_set(&can_message, &still_there(&bot.can_message, bots)) {
        patch.can_message = Some(can_message);
    }
    patch
}

/// The Routines row's value: "None yet", "Weekdays 8:48 AM, 12:47 PM", "2 routines" or "1 paused".
pub fn routines_summary(routines: &[RoutineDraft]) -> String {
    if routines.is_empty() {
        return "None yet".to_owned();
    }
    let on: Vec<&RoutineDraft> = routines.iter().filter(|routine| routine.enabled).collect();
    let Some(first) = on.first() else {
        return format!("{} paused", routines.len());
    };
    let days = days_label(&first.days);
    if on.iter().all(|routine| days_label(&routine.days) == days) {
        let times: Vec<&str> = on
            .iter()
            .flat_map(|routine| routine.times.iter().map(String::as_str))
            .collect();
        return format!("{days} {}", times_label(&times));
    }
    format!("{} routines", on.len())
}

/// The value of a row of bots to choose (Can message, a room's bots). Deleted bots don't count.
pub fn can_message_summary(ids: &[String], bots: &[Bot]) -> String {
    let count = still_there(ids, bots).len();
    match count {
        0 => "Nobody yet".to_owned(),
        1 => "1 agent".to_owned(),
        _ => format!("{count} agents"),
    }
}

/// The autonomy choices and their labels, as the form offers them (`AUTONOMY_LEVELS`).
pub const AUTONOMY_LEVELS: [(Autonomy, &str); 3] = [
    (Autonomy::Ask, "Always ask"),
    (Autonomy::Folder, "Own folder"),
    (Autonomy::Full, "Full auto"),
];

/// The line under Autonomy. What "Own folder" doesn't protect is said plainly (spec 5.4).
pub fn autonomy_help(autonomy: Autonomy, provider: Provider) -> &'static str {
    match autonomy {
        Autonomy::Ask => "Always ask: asks before every change it makes.",
        Autonomy::Full => {
            "Full auto: never asks. It can run any command and change any file your Windows account can."
        }
        Autonomy::Folder if provider == Provider::Claude => {
            "Own folder: works freely inside its own folder and asks before anything else. Claude agents also ask before each shell command."
        }
        Autonomy::Folder => {
            "Own folder: works freely inside its own folder and asks before anything else. It can still read the rest of your user profile, and Codex agents can read each other's folders."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The TypeScript fixture: "Pnet apply", a cloud on Claude's Sonnet, that can message b2 and b3.
    fn bot() -> Bot {
        serde_json::from_value(json!({
            "id": "b1", "name": "Pnet apply", "color": "orange", "shape": "cloud",
            "instructions": "Apply to Full Stack roles.", "provider": "claude", "model": "sonnet",
            "autonomy": "folder", "homeFolder": "C:\\bots\\b1", "projectFolder": null, "browser": true,
            "paused": false, "canMessage": ["b2", "b3"], "state": "idle", "setupReason": null,
            "limitedUntil": null, "createdAt": "2026-09-21T08:00:00.000Z"
        }))
        .unwrap()
    }

    fn with_id(id: &str, name: &str) -> Bot {
        Bot {
            id: id.to_owned(),
            name: name.to_owned(),
            ..bot()
        }
    }

    /// The bots there are: b1, b2 and b3.
    fn bots() -> Vec<Bot> {
        vec![
            bot(),
            with_id("b2", "Pnet apply"),
            with_id("b3", "Pnet apply"),
        ]
    }

    /// The TypeScript "Scout": a bot pinned to account a1.
    fn scout() -> Bot {
        Bot {
            id: "b1".to_owned(),
            name: "Scout".to_owned(),
            color: BotColor::Blue,
            shape: BotShape::Circle,
            instructions: String::new(),
            account_id: Some(Some("a1".to_owned())),
            project_folder: None,
            browser: true,
            can_message: Vec::new(),
            ..bot()
        }
    }

    fn routine(days: &[u8], times: &[&str], enabled: bool) -> RoutineDraft {
        RoutineDraft {
            name: "Routine".to_owned(),
            prompt: String::new(),
            days: days.to_vec(),
            times: times.iter().map(|time| (*time).to_owned()).collect(),
            time_limit_minutes: None,
            enabled,
        }
    }

    #[test]
    fn sends_only_what_the_person_changed() {
        let existing = bot();
        let form = form_from_bot(&existing);
        assert_eq!(patch_for(&existing, &form, &bots()), BotPatch::default());
        let renamed = BotForm {
            name: " Job scout ".to_owned(),
            can_message: vec!["b3".to_owned(), "b2".to_owned()],
            ..form.clone()
        };
        assert_eq!(
            patch_for(&existing, &renamed, &bots()),
            BotPatch {
                name: Some("Job scout".to_owned()),
                ..BotPatch::default()
            }
        );
        let haiku = BotForm {
            model: "haiku".to_owned(),
            ..form.clone()
        };
        assert_eq!(
            patch_for(&existing, &haiku, &bots()),
            BotPatch {
                provider: Some(Provider::Claude),
                model: Some("haiku".to_owned()),
                ..BotPatch::default()
            }
        );
        let codex = BotForm {
            provider: Provider::Codex,
            model: "gpt-6-astra".to_owned(),
            can_message: vec!["b2".to_owned()],
            ..form.clone()
        };
        assert_eq!(
            patch_for(&existing, &codex, &bots()),
            BotPatch {
                provider: Some(Provider::Codex),
                model: Some("gpt-6-astra".to_owned()),
                can_message: Some(vec!["b2".to_owned()]),
                ..BotPatch::default()
            }
        );
        let folder = BotForm {
            project_folder: Some("C:\\work".to_owned()),
            browser: false,
            ..form
        };
        assert_eq!(
            patch_for(&existing, &folder, &bots()),
            BotPatch {
                project_folder: Some(Some("C:\\work".to_owned())),
                browser: Some(false),
                ..BotPatch::default()
            }
        );
    }

    #[test]
    fn checks_the_name_and_trims_it_for_the_daemon() {
        let all = vec![bot(), with_id("b2", "CV Writer")];
        assert_eq!(
            name_problem("  ", &all, None),
            Some("Give the agent a name.".to_owned())
        );
        assert_eq!(
            name_problem(&"x".repeat(41), &all, None),
            Some("Keep the name to 40 characters or fewer.".to_owned())
        );
        assert_eq!(
            name_problem("cv writer", &all, None),
            Some("There's already an agent called cv writer.".to_owned())
        );
        assert_eq!(name_problem("Pnet apply", &all, Some("b1")), None);
        let night = BotForm {
            name: " Night Shift ".to_owned(),
            ..new_bot()
        };
        assert_eq!(create_input(&night).name, "Night Shift");
    }

    #[test]
    fn sums_up_routines_and_can_message_for_their_rows() {
        let weekdays: [u8; 5] = [1, 2, 3, 4, 5];
        assert_eq!(routines_summary(&[]), "None yet");
        assert_eq!(
            routines_summary(&[
                routine(&weekdays, &["16:45"], true),
                routine(&weekdays, &["08:48", "12:47"], true)
            ]),
            "Weekdays 8:48 AM, 12:47 PM, 4:45 PM"
        );
        assert_eq!(
            routines_summary(&[
                routine(&weekdays, &["08:48"], true),
                routine(&[0], &["09:00"], true)
            ]),
            "2 routines"
        );
        assert_eq!(
            routines_summary(&[routine(&weekdays, &["08:48"], false)]),
            "1 paused"
        );
        assert_eq!(
            can_message_summary(
                &["b2".to_owned(), "gone".to_owned()],
                &[bot(), with_id("b2", "Pnet apply")]
            ),
            "1 agent"
        );
        assert_eq!(can_message_summary(&[], &[bot()]), "Nobody yet");
    }

    #[test]
    fn says_what_own_folder_protects_for_each_cli() {
        assert!(
            autonomy_help(Autonomy::Folder, Provider::Claude)
                .contains("Claude agents also ask before each shell command.")
        );
        assert!(
            autonomy_help(Autonomy::Folder, Provider::Codex)
                .contains("It can still read the rest of your user profile")
        );
        assert!(
            autonomy_help(Autonomy::Full, Provider::Codex).starts_with("Full auto: never asks.")
        );
    }

    #[test]
    fn sends_only_what_was_changed_in_the_form_so_a_change_made_elsewhere_meanwhile_isnt_put_back()
    {
        let existing = bot();
        let edited = BotForm {
            instructions: "Apply to React roles.".to_owned(),
            ..form_from_bot(&existing)
        };
        assert_eq!(
            patch_for(&existing, &edited, &bots()),
            BotPatch {
                instructions: Some("Apply to React roles.".to_owned()),
                ..BotPatch::default()
            }
        );
    }

    #[test]
    fn never_sends_a_can_message_bot_deleted_since_the_form_opened_and_doesnt_count_it_as_a_change()
    {
        let existing = bot();
        let left: Vec<Bot> = bots().into_iter().filter(|each| each.id != "b2").collect(); // b2 deleted elsewhere
        assert_eq!(
            patch_for(&existing, &form_from_bot(&existing), &left),
            BotPatch::default()
        ); // Save stays off
        let with_b4 = [left.clone(), vec![with_id("b4", "Pnet apply")]].concat();
        let added = BotForm {
            can_message: vec!["b2".to_owned(), "b3".to_owned(), "b4".to_owned()],
            ..form_from_bot(&existing)
        };
        assert_eq!(
            patch_for(&existing, &added, &with_b4),
            BotPatch {
                can_message: Some(vec!["b3".to_owned(), "b4".to_owned()]),
                ..BotPatch::default()
            }
        );
        let emptied = BotForm {
            can_message: vec!["b2".to_owned()],
            ..form_from_bot(&existing)
        };
        assert_eq!(
            patch_for(&existing, &emptied, &left),
            BotPatch {
                can_message: Some(Vec::new()),
                ..BotPatch::default()
            }
        ); // b3 taken out here
    }

    #[test]
    fn starts_a_new_bot_on_the_providers_default_account_and_sends_the_pin_when_creating() {
        assert_eq!(new_bot().account_id, None);
        let pinned = BotForm {
            name: " Scout ".to_owned(),
            account_id: Some("a2".to_owned()),
            ..new_bot()
        };
        assert_eq!(
            create_input(&pinned).account_id,
            Some(Some("a2".to_owned()))
        );
    }

    #[test]
    fn reads_a_bots_pin_and_sends_it_only_when_it_changed_apart_from_the_provider_and_model() {
        let pinned = scout();
        let form = form_from_bot(&pinned);
        assert_eq!(form.account_id, Some("a1".to_owned()));
        assert_eq!(patch_for(&pinned, &form, &bots()), BotPatch::default());
        let moved = BotForm {
            account_id: Some("a2".to_owned()),
            ..form.clone()
        };
        assert_eq!(
            patch_for(&pinned, &moved, &bots()),
            BotPatch {
                account_id: Some(Some("a2".to_owned())),
                ..BotPatch::default()
            }
        );
        let unpinned = BotForm {
            account_id: None,
            ..form.clone()
        };
        assert_eq!(
            patch_for(&pinned, &unpinned, &bots()),
            BotPatch {
                account_id: Some(None),
                ..BotPatch::default()
            }
        );
        let codex = BotForm {
            provider: Provider::Codex,
            model: "gpt".to_owned(),
            account_id: Some("c1".to_owned()),
            ..form
        };
        assert_eq!(
            patch_for(&pinned, &codex, &bots()),
            BotPatch {
                provider: Some(Provider::Codex),
                model: Some("gpt".to_owned()),
                account_id: Some(Some("c1".to_owned())),
                ..BotPatch::default()
            }
        );
        let no_pin = Bot {
            account_id: None,
            ..pinned.clone()
        };
        assert_eq!(
            patch_for(&no_pin, &form_from_bot(&no_pin), &bots()),
            BotPatch::default()
        );
    }

    #[test]
    fn a_name_is_measured_in_javascript_characters_so_an_emoji_counts_twice() {
        // 20 emoji are 40 UTF-16 units, the most a name may have; 21 are 42.
        assert_eq!(name_problem(&"😀".repeat(20), &[], None), None);
        assert_eq!(
            name_problem(&"😀".repeat(21), &[], None),
            Some("Keep the name to 40 characters or fewer.".to_owned())
        );
    }
}
