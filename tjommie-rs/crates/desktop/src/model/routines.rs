//! Port of `packages/ui/lib/routines.ts` (T1.4): saving and removing a bot's routines through the daemon. The TypeScript
//! `request` (connection.ts) is the `Rpc` the caller passes in; the connection manager that makes one is not ported yet.
//! The TypeScript comment says new routines get this PC's zone, but the code sends no zone, so the daemon's default
//! applies. This port follows the code.

use client::{Rpc, RpcError};
use protocol::model::{Routine, RoutineInput, RoutinePatch};
use protocol::wire::{Id, RoutinesUpdate, method};

use super::bot_form::RoutineDraft;

/// The draft with its name and prompt trimmed (`cleaned`).
fn cleaned(draft: &RoutineDraft) -> RoutineDraft {
    RoutineDraft {
        name: draft.name.trim().to_owned(),
        prompt: draft.prompt.trim().to_owned(),
        days: draft.days.clone(),
        times: draft.times.clone(),
        time_limit_minutes: draft.time_limit_minutes,
        enabled: draft.enabled,
    }
}

/// Creates a routine for the bot, or saves the changes to one it already has (`saveRoutine`). Both send the whole draft.
pub async fn save_routine(
    rpc: &impl Rpc,
    bot_id: &str,
    routine: Option<&Routine>,
    draft: &RoutineDraft,
) -> Result<(), RpcError> {
    let draft = cleaned(draft);
    match routine {
        Some(routine) => rpc
            .call::<method::RoutinesUpdate>(RoutinesUpdate {
                id: routine.id.clone(),
                patch: RoutinePatch {
                    name: Some(draft.name),
                    prompt: Some(draft.prompt),
                    days: Some(draft.days),
                    times: Some(draft.times),
                    time_zone: None,
                    time_limit_minutes: Some(draft.time_limit_minutes),
                    enabled: Some(draft.enabled),
                },
            })
            .await
            .map(|_| ()),
        None => rpc
            .call::<method::RoutinesCreate>(RoutineInput {
                bot_id: bot_id.to_owned(),
                name: draft.name,
                prompt: draft.prompt,
                days: draft.days,
                times: draft.times,
                time_zone: None,
                time_limit_minutes: Some(draft.time_limit_minutes),
                enabled: Some(draft.enabled),
            })
            .await
            .map(|_| ()),
    }
}

/// Deletes a routine (`removeRoutine`).
pub async fn remove_routine(rpc: &impl Rpc, routine: &Routine) -> Result<(), RpcError> {
    rpc.call::<method::RoutinesDelete>(Id {
        id: routine.id.clone(),
    })
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::FakeRpc;
    use protocol::wire::Empty;
    use serde_json::json;

    fn routine() -> Routine {
        serde_json::from_value(json!({
            "id": "r1", "botId": "b1", "name": "Morning digest", "prompt": "Write it.",
            "days": [1, 2, 3, 4, 5], "times": ["06:30"], "timeZone": "UTC", "timeLimitMinutes": 60,
            "enabled": true, "nextRunAt": null, "createdAt": "2026-09-21T08:00:00.000Z"
        }))
        .unwrap()
    }

    fn draft() -> RoutineDraft {
        RoutineDraft {
            name: " Check ".to_owned(),
            prompt: " Look\n ".to_owned(),
            days: vec![1],
            times: vec!["09:00".to_owned()],
            time_limit_minutes: Some(60),
            enabled: true,
        }
    }

    // No TypeScript test exists for this module; these check the three daemon calls it makes.

    #[tokio::test]
    async fn a_new_routine_is_created_for_the_bot_with_its_text_trimmed() {
        let rpc = FakeRpc::new().on::<method::RoutinesCreate>(|_| Ok(routine()));
        save_routine(&rpc, "b1", None, &draft()).await.unwrap();
        assert_eq!(
            rpc.calls(),
            vec![(
                "routines.create",
                json!({
                    "botId": "b1", "name": "Check", "prompt": "Look", "days": [1],
                    "times": ["09:00"], "timeLimitMinutes": 60, "enabled": true
                })
            )]
        );
    }

    #[tokio::test]
    async fn an_existing_routine_is_updated_with_its_whole_draft_and_no_limit_is_sent_as_null() {
        let rpc = FakeRpc::new().on::<method::RoutinesUpdate>(|_| Ok(routine()));
        let unlimited = RoutineDraft {
            time_limit_minutes: None,
            ..draft()
        };
        save_routine(&rpc, "b1", Some(&routine()), &unlimited)
            .await
            .unwrap();
        assert_eq!(
            rpc.calls(),
            vec![(
                "routines.update",
                json!({
                    "id": "r1",
                    "patch": {
                        "name": "Check", "prompt": "Look", "days": [1], "times": ["09:00"],
                        "timeLimitMinutes": null, "enabled": true
                    }
                })
            )]
        );
    }

    #[tokio::test]
    async fn removing_a_routine_deletes_it_by_its_id() {
        let rpc = FakeRpc::new().on::<method::RoutinesDelete>(|_| Ok(Empty {}));
        remove_routine(&rpc, &routine()).await.unwrap();
        assert_eq!(rpc.calls(), vec![("routines.delete", json!({"id": "r1"}))]);
    }
}
