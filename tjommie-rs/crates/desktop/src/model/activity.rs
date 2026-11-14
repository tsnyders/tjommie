//! Port of `packages/ui/lib/activity.ts`: the bot panel's Recent activity, the newest steps from the chat's activity rows.
//! Each step shows the time its turn's row started.

use protocol::model::{ActivityStatus, Message, MessageData, message_data};

/// One step in the bot panel's Recent activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentStep {
    /// `"<message id>:<item id>"`.
    pub key: String,
    pub summary: String,
    pub status: ActivityStatus,
    /// The activity row's `createdAt`.
    pub at: String,
}

/// The newest steps from the chat's activity rows, newest first, up to `limit` (the TypeScript default is 8). A row whose
/// payload doesn't match the activity shape is skipped.
pub fn recent_activity(messages: &[Message], limit: usize) -> Vec<RecentStep> {
    let mut steps = Vec::new();
    for message in messages.iter().rev() {
        if steps.len() >= limit {
            break;
        }
        // `message_data` checks the payload against the message's kind, so only activity rows match here.
        let Ok(MessageData::Activity(data)) = message_data(message) else {
            continue;
        };
        for item in data.items.iter().rev() {
            if steps.len() >= limit {
                break;
            }
            steps.push(RecentStep {
                key: format!("{}:{}", message.id, item.id),
                summary: item.summary.clone(),
                status: item.status,
                at: message.created_at.clone(),
            });
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn message(value: Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    /// `steps` are `(id, summary, status)`, as the TypeScript fixture builds them.
    fn activity(id: &str, created_at: &str, steps: &[(&str, &str, &str)]) -> Message {
        let items: Vec<Value> = steps
            .iter()
            .map(|(item, summary, status)| {
                json!({"id": item, "name": "Tool", "summary": summary, "status": status})
            })
            .collect();
        message(json!({
            "id": id, "conversationId": "b1", "author": "b1", "kind": "activity",
            "body": format!("{} steps", steps.len()), "data": {"items": items},
            "status": "done", "createdAt": created_at
        }))
    }

    fn text(id: &str) -> Message {
        message(json!({
            "id": id, "conversationId": "b1", "author": "b1", "kind": "text", "body": "Hi",
            "data": null, "status": "done", "createdAt": "2026-09-27T09:30:00.000Z"
        }))
    }

    #[test]
    fn lists_the_newest_steps_first_each_with_its_turns_time_up_to_the_limit() {
        let messages = [
            activity(
                "a1",
                "2026-09-27T09:15:00.000Z",
                &[("s1", "Searched pnet.co.za for Full Stack roles", "done")],
            ),
            text("t1"),
            activity(
                "a2",
                "2026-09-27T09:31:00.000Z",
                &[
                    ("s2", "Read listing PN-48213", "done"),
                    ("s3", "Filling the Apply form on pnet.co.za", "running"),
                ],
            ),
        ];
        let steps = recent_activity(&messages, 8);
        assert_eq!(steps.len(), 3);
        assert_eq!(
            (
                steps[0].summary.as_str(),
                steps[0].status,
                steps[0].at.as_str()
            ),
            (
                "Filling the Apply form on pnet.co.za",
                ActivityStatus::Running,
                "2026-09-27T09:31:00.000Z"
            )
        );
        assert_eq!(
            (
                steps[1].summary.as_str(),
                steps[1].status,
                steps[1].at.as_str()
            ),
            (
                "Read listing PN-48213",
                ActivityStatus::Done,
                "2026-09-27T09:31:00.000Z"
            )
        );
        assert_eq!(
            (
                steps[2].summary.as_str(),
                steps[2].status,
                steps[2].at.as_str()
            ),
            (
                "Searched pnet.co.za for Full Stack roles",
                ActivityStatus::Done,
                "2026-09-27T09:15:00.000Z"
            )
        );
        assert_eq!(steps[0].key, "a2:s3");
        let limited = recent_activity(&messages, 2);
        assert_eq!(
            limited
                .iter()
                .map(|step| step.summary.as_str())
                .collect::<Vec<_>>(),
            [
                "Filling the Apply form on pnet.co.za",
                "Read listing PN-48213"
            ]
        );
    }

    #[test]
    fn skips_activity_rows_whose_payload_doesnt_match() {
        let row = message(json!({
            "id": "x", "conversationId": "b1", "author": "b1", "kind": "activity", "body": "Hi",
            "data": {"steps": 3}, "status": "done", "createdAt": "2026-09-27T09:30:00.000Z"
        }));
        assert!(recent_activity(&[row], 8).is_empty());
    }
}
