//! `relay.test.ts`

use super::with;
use protocol::parse;
use protocol::relay::{
    DEFAULT_RELAY_URL, HostToRelay, LEGACY_PUSH_KINDS, PushData, RELAY, RELAY_CLOSE, RelayToHost,
    login_message,
};
use protocol::remote::{PushKind, RelayUrl};
use serde_json::json;

const HOST_ID: &str = "99cf98ba-2638-4b16-8a6e-1691c1673b45";

fn key() -> String {
    format!("{:A<43}", "AQ")
}

#[test]
fn freezes_relay_limits_and_control_messages() {
    assert_eq!(
        DEFAULT_RELAY_URL,
        Some("https://tjommie-relay.tyrellsnyders30.workers.dev")
    );
    let url = DEFAULT_RELAY_URL.expect("The public relay URL must be set");
    assert_eq!(RelayUrl::new(url).unwrap().as_str(), url);
    assert_eq!(
        (
            RELAY.max_frame,
            RELAY.host_ping_ms,
            RELAY.host_dead_ms,
            RELAY.accept_timeout_ms
        ),
        (65_535, 30_000, 75_000, 10_000)
    );
    assert_eq!(
        (
            RELAY.max_channels,
            RELAY.max_pending_channels,
            RELAY.connects_per_ip_per_minute,
            RELAY.max_push_tokens
        ),
        (32, 8, 20, 16)
    );
    assert_eq!(
        (
            RELAY_CLOSE.host_offline,
            RELAY_CLOSE.bad_login,
            RELAY_CLOSE.replaced,
            RELAY_CLOSE.host_dead,
            RELAY_CLOSE.too_large,
            RELAY_CLOSE.limited
        ),
        (4404, 4401, 4409, 4408, 1009, 4429)
    );
    assert!(parse::<RelayToHost>(json!({ "t": "challenge", "nonce": key() })).is_ok());
    for ready in [
        json!({ "t": "ready", "push": true }),
        json!({ "t": "ready", "push": true, "kinds": ["done"] }),
        json!({ "t": "ready", "push": true, "kinds": ["done", "mention"] }),
    ] {
        assert_eq!(
            serde_json::to_value(parse::<RelayToHost>(ready.clone()).unwrap()).unwrap(),
            ready
        );
    }
    assert!(
        parse::<HostToRelay>(
            json!({ "t": "login", "publicKey": key(), "signature": "A".repeat(86) })
        )
        .is_ok()
    );
}

#[test]
fn bounds_push_tokens_and_kinds_and_excludes_content_from_push_data() {
    assert_eq!(
        serde_json::to_value(PushKind::ALL).unwrap(),
        json!(["approval", "limit", "routine_failed", "done", "failed"])
    );
    assert_eq!(
        serde_json::to_value(LEGACY_PUSH_KINDS).unwrap(),
        json!(["approval", "limit", "routine_failed"])
    );
    let push =
        json!({ "t": "push", "id": 1, "tokens": ["token"], "kind": "approval", "botId": "bot" });
    assert!(parse::<HostToRelay>(push.clone()).is_ok());
    let room = with(push.clone(), json!({ "kind": "done", "roomId": "room" }));
    assert_eq!(
        serde_json::to_value(parse::<HostToRelay>(room.clone()).unwrap()).unwrap(),
        room
    );
    assert!(
        parse::<HostToRelay>(with(push.clone(), json!({ "tokens": vec!["token"; 17] }))).is_err()
    );
    assert!(parse::<HostToRelay>(with(push, json!({ "kind": "chat" }))).is_err());
    let data = json!({ "v": "1", "hostId": HOST_ID, "kind": "approval", "botId": "bot" });
    assert!(parse::<PushData>(data.clone()).is_ok());
    let failed = with(data.clone(), json!({ "kind": "failed", "roomId": "room" }));
    assert_eq!(
        serde_json::to_value(parse::<PushData>(failed.clone()).unwrap()).unwrap(),
        failed
    );
    assert!(parse::<PushData>(with(data.clone(), json!({ "roomId": "x".repeat(65) }))).is_err());
    assert!(parse::<PushData>(with(data, json!({ "title": "secret" }))).is_err());
}

#[test]
fn signs_the_domain_separated_login_message_bytes() {
    assert_eq!(
        login_message("h", "n", "r.example"),
        b"tjommie-relay-login/v1\nh\nn\nr.example".to_vec()
    );
}
