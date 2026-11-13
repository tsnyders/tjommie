//! `model-data.test.ts`

use super::{invalid, round_trip, with, without};
use protocol::Invalid;
use protocol::model::{
    ExchangeHandoffData, Message, MessageData, RoomHandoffData, RoutineRun, TextData, message_data,
};
use serde_json::{Value, json};

fn message(kind: &str, data: Value) -> Message {
    serde_json::from_value(json!({ "id": "message", "conversationId": "chat", "author": "bot", "kind": kind, "body": "Content",
        "data": data, "status": "done", "createdAt": "2026-09-26T12:00:00.000Z" }))
    .unwrap()
}

fn data(kind: &str, data: Value) -> Result<MessageData, Invalid> {
    message_data(&message(kind, data))
}

/// The payload `messageData` returns, as JSON.
fn payload(kind: &str, value: Value) -> Value {
    let parsed = data(kind, value).unwrap_or_else(|error| panic!("{kind}: {error}"));
    serde_json::to_value(parsed).unwrap()["data"].take()
}

#[test]
fn validates_exchange_handoffs_and_room_answer_links() {
    let exchange = json!({ "exchangeId": "exchange", "fromBotId": "writer", "toBotId": "reader", "messages": 2, "status": "replied" });
    let room = json!({ "roomMessageId": "room-reply", "roomId": "research" });
    assert_eq!(payload("handoff", exchange.clone()), exchange);
    assert_eq!(
        payload("handoff", json!({ "roomMessageId": "room-reply" })),
        json!({ "roomMessageId": "room-reply" })
    );
    assert_eq!(payload("handoff", room.clone()), room);
    assert!(serde_json::from_value::<ExchangeHandoffData>(room).is_err());
    assert!(serde_json::from_value::<RoomHandoffData>(exchange.clone()).is_err());
    assert!(
        data(
            "handoff",
            with(exchange.clone(), json!({ "status": "done" }))
        )
        .is_err()
    );
    assert!(data("handoff", with(exchange, json!({ "messages": "2" }))).is_err());
    assert!(data("handoff", json!({ "roomMessageId": 1 })).is_err());
}

#[test]
fn validates_routine_label_run_status_and_note() {
    for status in [
        "queued", "running", "done", "stopped", "failed", "missed", "skipped",
    ] {
        let note = if status == "missed" {
            json!("The PC was asleep.")
        } else {
            Value::Null
        };
        let label =
            json!({ "routineId": "routine", "runId": "run", "status": status, "note": note });
        assert_eq!(payload("routine", label.clone()), label);
    }
    assert!(
        data(
            "routine",
            json!({ "routineId": "routine", "runId": "run", "status": "replied", "note": null })
        )
        .is_err()
    );
}

#[test]
fn validates_shared_file_cards() {
    let file = json!({ "path": "C:\\Bot\\inbox\\report.txt", "name": "report.txt", "size": 12 });
    assert_eq!(payload("file", file.clone()), file);
    assert!(data("file", with(file.clone(), json!({ "size": -1 }))).is_err());
    assert!(data("file", with(file, json!({ "path": null }))).is_err());
}

#[test]
fn validates_tool_and_routine_approval_cards_in_every_persisted_state() {
    for status in ["pending", "allowed", "denied"] {
        let tool = json!({ "approvalId": "approval", "tool": "Write", "detail": "Write a file", "rule": null, "status": status });
        let routine = json!({ "approvalId": "proposal", "kind": "routine", "tool": "Routine", "detail": "Check each morning", "status": status });
        assert_eq!(payload("approval", tool.clone()), tool);
        assert_eq!(
            payload("approval", with(tool.clone(), json!({ "kind": "tool" }))),
            with(tool, json!({ "kind": "tool" }))
        );
        assert_eq!(payload("approval", routine.clone()), routine);
    }
    assert!(data("approval", json!({ "approvalId": "approval", "tool": "Write", "detail": "Write", "rule": null, "status": "done" })).is_err());
}

#[test]
fn validates_activity_items_and_their_progress() {
    let activity = json!({ "items": [
        { "id": "a", "name": "Read", "summary": "Read a file", "status": "running" },
        { "id": "b", "name": "Write", "summary": "Write a file", "status": "done" },
        { "id": "c", "name": "Bash", "summary": "Run a command", "status": "failed" },
    ] });
    assert_eq!(payload("activity", activity.clone()), activity);
    assert!(
        data(
            "activity",
            json!({ "items": [with(activity["items"][0].clone(), json!({ "status": "pending" }))] })
        )
        .is_err()
    );
    assert!(data("activity", json!({ "items": "Read a file" })).is_err());
}

#[test]
fn validates_retry_links_and_exchange_failure_notice_links() {
    assert_eq!(
        payload("error", json!({ "retryMessageId": "failed-message" })),
        json!({ "retryMessageId": "failed-message" })
    );
    let room_retry = json!({ "retryMessageId": "room-post", "retryBotId": "failed-bot" });
    assert_eq!(payload("error", room_retry.clone()), room_retry);
    assert_eq!(
        payload("notice", json!({ "exchangeId": "failed-exchange" })),
        json!({ "exchangeId": "failed-exchange" })
    );
    assert!(data("error", json!({ "retryMessageId": false })).is_err());
    assert!(
        data(
            "error",
            json!({ "retryMessageId": "room-post", "retryBotId": false })
        )
        .is_err()
    );
    assert!(data("notice", json!({ "exchangeId": 1 })).is_err());
}

#[test]
fn distinguishes_copied_attachment_names_from_exchange_file_paths() {
    let attachments = json!({ "files": ["report.txt", "notes.txt"] });
    let exchange = json!({ "exchangeId": "exchange", "toBotId": "reader", "files": [{ "name": "report.txt", "path": "C:\\Bot\\inbox\\report.txt" }] });
    assert_eq!(payload("text", attachments.clone()), attachments);
    assert_eq!(payload("text", exchange.clone()), exchange);
    assert_eq!(
        payload("text", without(exchange.clone(), "files")),
        json!({ "exchangeId": "exchange", "toBotId": "reader" })
    );
    assert!(data("text", json!({ "files": exchange["files"] })).is_err());
    assert!(
        data(
            "text",
            with(exchange, json!({ "files": attachments["files"] }))
        )
        .is_err()
    );
}

#[test]
fn keeps_the_raw_wire_data_compatible_while_validating_data_at_the_typed_reader() {
    let legacy = message("text", json!({ "futureField": true }));
    assert_eq!(
        serde_json::to_value(&legacy).unwrap()["data"],
        json!({ "futureField": true })
    );
    assert!(message_data(&legacy).is_err());
    assert_eq!(message("text", Value::Null).data, None);
}

#[test]
fn returns_data_narrowed_by_the_message_kind() {
    let Ok(MessageData::Routine(label)) = data(
        "routine",
        json!({ "routineId": "routine", "runId": "run", "status": "done", "note": null }),
    ) else {
        panic!("Expected a routine label");
    };
    assert_eq!(label.run_id, "run");
    let Ok(MessageData::Text(Some(TextData::ExchangePost(post)))) = data(
        "text",
        json!({ "exchangeId": "exchange", "toBotId": "reader" }),
    ) else {
        panic!("Expected an exchange post");
    };
    assert_eq!(post.to_bot_id, "reader");
}

#[test]
fn validates_each_message_kind_instead_of_accepting_another_kinds_payload() {
    let cases = [
        ("text", json!({ "files": ["report.txt"] })),
        ("activity", json!({ "items": [] })),
        (
            "approval",
            json!({ "approvalId": "a", "tool": "Write", "detail": "Write", "rule": null, "status": "pending" }),
        ),
        ("notice", json!({ "exchangeId": "exchange" })),
        ("error", json!({ "retryMessageId": "retry" })),
        ("handoff", json!({ "roomMessageId": "reply" })),
        (
            "routine",
            json!({ "routineId": "routine", "runId": "run", "status": "done", "note": null }),
        ),
        (
            "file",
            json!({ "path": "C:\\Bot\\inbox\\report.txt", "name": "report.txt", "size": 12 }),
        ),
    ];
    for (kind, value) in cases {
        assert_eq!(
            serde_json::to_value(data(kind, value.clone()).unwrap()).unwrap(),
            json!({ "kind": kind, "data": value })
        );
        assert!(
            data(kind, json!({ "invalidField": true })).is_err(),
            "{kind}"
        );
    }
    assert!(data("file", json!({ "retryMessageId": "retry" })).is_err());
}

#[test]
fn permits_absent_data_only_on_plain_text_notices_and_errors() {
    for kind in ["text", "notice", "error"] {
        assert_eq!(
            serde_json::to_value(data(kind, Value::Null).unwrap()).unwrap(),
            json!({ "kind": kind, "data": null })
        );
    }
    for kind in ["activity", "approval", "handoff", "routine", "file"] {
        assert!(data(kind, Value::Null).is_err(), "{kind}");
    }
}

#[test]
fn routine_run_carries_the_owning_bot_id() {
    let run = json!({ "id": "run", "botId": "bot", "routineId": "routine", "dueAt": "due", "startedAt": null, "endedAt": null, "status": "queued", "note": null });
    let _: RoutineRun = round_trip("routine.run", run.clone());
    invalid("routine.run", without(run, "botId"));
}
