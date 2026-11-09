//! `wire.test.ts`

use super::{invalid, params, rejects, round_trip, with, without};
use protocol::model::{
    Approval, ApprovalKind, Bot, ProviderSetup, Room, SETUP_CODEX_SANDBOX_REASON, Settings,
    TEAM_ROOM_ID,
};
use protocol::wire::{Empty, ExchangeView, Id, MessagePage, Notify, PendingNotification};
use protocol::{ClientMessage, ErrorCode, PROTOCOL_VERSION, parse};
use serde_json::{Value, json};

fn bot() -> Value {
    json!({ "name": "Writer", "color": "blue", "shape": "circle", "instructions": "", "provider": "claude", "model": "sonnet" })
}

#[test]
fn requires_a_typed_codex_sandbox_status_and_a_setup_reason_on_bots() {
    let base = json!({ "installed": true, "version": "0.155.1", "signedIn": true, "account": null, "plan": null, "error": null });
    let codex = |sandbox: &str| {
        with(
            base.clone(),
            json!({ "provider": "codex", "sandbox": sandbox }),
        )
    };
    for sandbox in ["ready", "notConfigured", "updateRequired", "unavailable"] {
        let setup: Vec<ProviderSetup> = round_trip("setup.status", json!([codex(sandbox)]));
        assert!(matches!(setup[0], ProviderSetup::Codex(_)));
    }
    invalid(
        "setup.status",
        json!([with(base.clone(), json!({ "provider": "codex" }))]),
    );
    invalid("setup.status", json!([codex("unknown")]));
    let minimal_bot = with(
        bot(),
        json!({ "id": "b", "autonomy": "folder", "homeFolder": "C:\\b", "projectFolder": null, "browser": false, "paused": false,
            "canMessage": [], "state": "needs_setup", "setupReason": "Set up Codex's sandbox", "limitedUntil": null, "createdAt": "t" }),
    );
    let parsed: Bot = round_trip("bot.updated", minimal_bot.clone());
    assert_eq!(
        parsed.setup_reason.as_deref(),
        Some(SETUP_CODEX_SANDBOX_REASON)
    );
    invalid("bot.updated", without(minimal_bot, "setupReason"));
    let changed: Vec<ProviderSetup> = round_trip("setup.changed", json!([codex("ready")]));
    assert_eq!(changed.len(), 1);
}

#[test]
fn types_the_one_time_codex_sandbox_setup_call() {
    assert_eq!(params("setup.prepareCodexSandbox", json!({})), json!({}));
    let status = json!({ "provider": "codex", "installed": true, "version": "0.155.1", "signedIn": true, "account": null, "plan": null,
        "error": null, "sandbox": "ready" });
    let _: ProviderSetup = round_trip("setup.prepareCodexSandbox", status);
}

#[test]
fn is_version_2_for_the_project_attachment_and_browser_control_contracts() {
    assert_eq!(PROTOCOL_VERSION, 2);
}

#[test]
fn lists_every_daemon_error_code_and_rejects_unknown_codes() {
    let codes = [
        "bad_params",
        "not_found",
        "busy",
        "name_taken",
        "already_answered",
        "bad_state",
        "not_installed",
        "no_account",
        "unknown_method",
        "internal",
        "forbidden",
        "unavailable",
        "expired",
        "corrupt",
    ];
    assert_eq!(serde_json::to_value(ErrorCode::ALL).unwrap(), json!(codes));
    for code in codes {
        let parsed: ErrorCode = serde_json::from_value(json!(code)).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(code));
    }
    assert!(serde_json::from_value::<ErrorCode>(json!("unexpected")).is_err());
}

#[test]
fn parses_hello_and_request_messages_and_rejects_anything_else() {
    assert!(matches!(
        parse::<ClientMessage>(json!({ "type": "hello", "protocol": 1, "token": "t" })),
        Ok(ClientMessage::Hello { .. })
    ));
    assert!(matches!(
        parse::<ClientMessage>(
            json!({ "type": "req", "id": 1, "method": "bots.list", "params": {} })
        ),
        Ok(ClientMessage::Req { .. })
    ));
    assert!(parse::<ClientMessage>(json!({ "type": "nope" })).is_err());
    // Not in wire.test.ts. Probed against zod 4.6.5: a request without a params key fails ClientMessage
    // ("expected nonoptional"), so the server treats it as a malformed frame.
    assert!(
        parse::<ClientMessage>(json!({ "type": "req", "id": 1, "method": "bots.list" })).is_err()
    );
}

#[test]
fn trims_bot_names_and_rejects_blank_ones() {
    assert_eq!(
        params("bots.create", with(bot(), json!({ "name": "  Writer " })))["name"],
        "Writer"
    );
    rejects("bots.create", with(bot(), json!({ "name": "   " })));
}

#[test]
fn leaves_unspecified_fields_out_of_a_patch_so_they_stay_unchanged() {
    assert_eq!(
        params(
            "bots.update",
            json!({ "id": "b", "patch": { "autonomy": "full" } })
        )["patch"],
        json!({ "autonomy": "full" })
    );
}

#[test]
fn applies_chat_send_defaults_and_rejects_blank_text() {
    assert_eq!(
        params("chat.send", json!({ "botId": "b", "text": " hi " })),
        json!({ "botId": "b", "text": "hi", "files": [] })
    );
    rejects("chat.send", json!({ "botId": "b", "text": "  " }));
}

#[test]
fn defaults_room_calls_to_the_team_room_for_older_callers() {
    assert_eq!(
        params("room.history", json!({})),
        json!({ "limit": 50, "roomId": TEAM_ROOM_ID })
    );
    assert_eq!(
        params("room.post", json!({ "text": " hello " })),
        json!({ "roomId": TEAM_ROOM_ID, "text": "hello" })
    );
}

#[test]
fn preserves_an_optional_bot_id_when_selecting_a_failed_room_turn_to_retry() {
    assert_eq!(
        params("chat.retry", json!({ "messageId": "m" })),
        json!({ "messageId": "m" })
    );
    assert_eq!(
        params("chat.retry", json!({ "messageId": "m", "botId": "b" })),
        json!({ "messageId": "m", "botId": "b" })
    );
}

#[test]
fn requires_an_explicit_has_more_flag_on_history_responses() {
    for method in ["chat.history", "room.history"] {
        let _: MessagePage = round_trip(method, json!({ "messages": [], "hasMore": false }));
        invalid(method, json!([]));
        invalid(method, json!({ "messages": [] }));
    }
    let exchange = json!({ "id": "e", "fromBotId": "a", "toBotId": "b", "status": "waiting", "request": "Help", "reply": null,
        "createdAt": "t", "repliedAt": null });
    let view: ExchangeView = round_trip(
        "exchanges.get",
        json!({ "exchange": exchange, "messages": [], "hasMore": false }),
    );
    assert!(!view.has_more);
    invalid(
        "exchanges.get",
        json!({ "exchange": exchange, "messages": [] }),
    );
}

#[test]
fn types_durable_routine_notifications_and_their_explicit_acknowledgement() {
    let notification = json!({ "kind": "routine_failed", "botId": "b", "routineId": "r", "runId": "run", "title": "Failed", "body": "Restarted" });
    assert_eq!(params("notifications.pending", json!({})), json!({}));
    let _: Vec<PendingNotification> = round_trip("notifications.pending", json!([notification]));
    invalid(
        "notifications.pending",
        json!([without(notification, "runId")]),
    );
    assert_eq!(
        params("notifications.ack", json!({ "runId": "run" })),
        json!({ "runId": "run" })
    );
    rejects("notifications.ack", json!({ "runId": "" }));
    let _: Empty = round_trip("notifications.ack", json!({}));
}

#[test]
fn names_the_team_room() {
    assert_eq!(TEAM_ROOM_ID, "team");
}

#[test]
fn validates_room_inputs_and_permits_an_empty_patch() {
    assert_eq!(
        params(
            "rooms.create",
            json!({ "name": "  Research  ", "botIds": ["bot"] })
        ),
        json!({ "name": "Research", "botIds": ["bot"] })
    );
    rejects("rooms.create", json!({ "name": " ", "botIds": ["bot"] }));
    rejects(
        "rooms.create",
        json!({ "name": "x".repeat(61), "botIds": ["bot"] }),
    );
    rejects("rooms.create", json!({ "name": "Research", "botIds": [] }));
    assert_eq!(
        params("rooms.update", json!({ "id": "r", "patch": {} }))["patch"],
        json!({})
    );
}

#[test]
fn describes_room_methods_and_events() {
    let room = json!({ "id": "research", "name": "Research", "botIds": ["writer"], "everyone": false, "createdAt": "created" });
    let input = json!({ "name": "Research", "botIds": ["writer"] });
    assert_eq!(params("rooms.list", json!({})), json!({}));
    let _: Vec<Room> = round_trip("rooms.list", json!([room]));
    assert_eq!(params("rooms.create", input.clone()), input);
    let _: Room = round_trip("rooms.create", room.clone());
    assert_eq!(
        params("rooms.update", json!({ "id": "research", "patch": {} })),
        json!({ "id": "research", "patch": {} })
    );
    let _: Room = round_trip("rooms.update", room.clone());
    assert_eq!(
        params("rooms.delete", json!({ "id": "research" })),
        json!({ "id": "research" })
    );
    let _: Empty = round_trip("rooms.delete", json!({}));
    let _: Room = round_trip("room.updated", room);
    let _: Id = round_trip("room.deleted", json!({ "id": "research" }));
}

#[test]
fn lets_callers_list_all_routines_or_one_bots_routines() {
    assert_eq!(params("routines.list", json!({})), json!({}));
    assert_eq!(
        params("routines.list", json!({ "botId": "b" })),
        json!({ "botId": "b" })
    );
}

#[test]
fn describes_notification_kinds_and_their_optional_targets() {
    let notification = json!({ "botId": "b", "title": "Needs attention", "body": "Open this bot" });
    invalid("notify", notification.clone());
    for target in [
        json!({ "kind": "approval", "approvalId": "a" }),
        json!({ "kind": "limit" }),
        json!({ "kind": "routine_failed", "routineId": "r", "runId": "run" }),
    ] {
        let _: Notify = round_trip("notify", with(notification.clone(), target));
    }
    for kind in ["approval", "limit", "routine_failed"] {
        let _: Notify = round_trip(
            "notify",
            with(notification.clone(), json!({ "kind": kind })),
        );
    }
    for kind in ["done", "failed"] {
        let _: Notify = round_trip(
            "notify",
            with(notification.clone(), json!({ "kind": kind })),
        );
        let _: Notify = round_trip(
            "notify",
            with(
                notification.clone(),
                json!({ "kind": kind, "roomId": "room" }),
            ),
        );
        invalid(
            "notify",
            with(
                notification.clone(),
                json!({ "kind": kind, "roomId": "x".repeat(65) }),
            ),
        );
    }
    invalid(
        "notify",
        with(notification.clone(), json!({ "kind": "other" })),
    );
    invalid(
        "notify",
        with(
            notification,
            json!({ "kind": "approval", "approvalId": 42 }),
        ),
    );
}

#[test]
fn accepts_a_weekday_routine_and_rejects_bad_days_and_times() {
    let routine = json!({ "botId": "b", "name": "Morning check", "prompt": "Check pnet.co.za", "days": [1, 2, 3, 4, 5], "times": ["08:48", "12:47"] });
    assert_eq!(params("routines.create", routine.clone()), routine);
    rejects(
        "routines.create",
        with(routine.clone(), json!({ "days": [7] })),
    );
    rejects(
        "routines.create",
        with(routine.clone(), json!({ "times": ["8:48"] })),
    );
    rejects(
        "routines.create",
        with(routine, json!({ "times": ["24:00"] })),
    );
}

#[test]
fn accepts_real_iana_time_zones_and_rejects_unknown_ones() {
    let routine = json!({ "botId": "b", "name": "Morning check", "prompt": "Check pnet.co.za", "days": [1], "times": ["08:48"] });
    assert_eq!(
        params(
            "routines.create",
            with(
                routine.clone(),
                json!({ "timeZone": "Africa/Johannesburg" })
            )
        )["timeZone"],
        "Africa/Johannesburg"
    );
    rejects(
        "routines.create",
        with(routine, json!({ "timeZone": "Mars/Olympus" })),
    );
    let patch = |time_zone: &str| json!({ "id": "r", "patch": { "timeZone": time_zone } });
    assert_eq!(
        params("routines.update", patch("Africa/Johannesburg"))["patch"],
        json!({ "timeZone": "Africa/Johannesburg" })
    );
    rejects("routines.update", patch("Mars/Olympus"));
}

#[test]
fn leaves_unspecified_routine_fields_out_of_a_patch() {
    assert_eq!(
        params(
            "routines.update",
            json!({ "id": "r", "patch": { "enabled": false } })
        )["patch"],
        json!({ "enabled": false })
    );
}

#[test]
fn describes_routine_proposals_as_well_as_tool_approvals() {
    let approval = json!({ "id": "a", "botId": "b", "kind": "routine", "tool": "Routine", "summary": "s", "detail": "d", "rule": null,
        "payload": { "name": "x" }, "status": "pending", "messageId": "m", "createdAt": "t" });
    let parsed: Approval = round_trip("approval.added", approval);
    assert_eq!(parsed.kind, ApprovalKind::Routine);
}

#[test]
fn bounds_the_chain_limit_between_1_and_20() {
    let settings: Settings =
        round_trip("settings.get", json!({ "chainLimit": 5, "relayUrl": null }));
    assert_eq!(settings.chain_limit, 5);
    invalid("settings.get", json!({ "chainLimit": 0, "relayUrl": null }));
    invalid(
        "settings.get",
        json!({ "chainLimit": 21, "relayUrl": null }),
    );
}
