//! The Rust protocol against `tjommie-rs/protocol/manifest.json`, which `scripts/protocol-manifest.ts` writes from the zod schemas.

use protocol::diagnostics::DiagnosticReport;
use protocol::model::MessageData;
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserMount, NativeBrowserUiCommand};
use protocol::relay::{HostToRelay, PushData, RelayToHost};
use protocol::remote::{PairingCode, RemoteConfirm, RemotePreface, RemoteReady, RemoteRefused};
use protocol::speech::{SpeechEvent, SpeechStart, SpeechStatus};
use protocol::vault::VaultHostCommand;
use protocol::vault_host::{VaultHostJob, VaultHostReply};
use protocol::wire::Id;
use protocol::{
    ClientMessage, DecodeError, EVENTS, ErrorCode, Event, METHODS, PROTOCOL_VERSION, Request,
    Response, ServerMessage, Wire, assert_valid, validate,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    protocol_version: u32,
    error_codes: Vec<String>,
    methods: Vec<Named>,
    events: Vec<Named>,
    types: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

fn manifest() -> Manifest {
    serde_json::from_str(include_str!("../../../protocol/manifest.json")).unwrap()
}

fn sorted<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut names: Vec<&str> = names.into_iter().collect();
    names.sort_unstable();
    names
}

#[test]
fn method_names_equal_the_manifest() {
    let manifest = manifest();
    assert_eq!(
        sorted(METHODS.iter().copied()),
        sorted(manifest.methods.iter().map(|method| method.name.as_str()))
    );
    assert_eq!(METHODS.len(), 101);
}

#[test]
fn event_names_equal_the_manifest() {
    let manifest = manifest();
    assert_eq!(
        sorted(EVENTS.iter().copied()),
        sorted(manifest.events.iter().map(|event| event.name.as_str()))
    );
    assert_eq!(EVENTS.len(), 26);
}

macro_rules! wire_types {
    ($($wire:ty),* $(,)?) => {
        /// Each standalone type's manifest name, with a call that builds its validator.
        fn wire_types() -> Vec<(&'static str, fn())> {
            vec![$((<$wire as Wire>::NAME, || drop(protocol::parse::<$wire>(serde_json::Value::Null)))),*]
        }
    };
}

wire_types!(
    ClientMessage,
    MessageData,
    PairingCode,
    RemotePreface,
    RemoteConfirm,
    RemoteReady,
    RemoteRefused,
    RelayToHost,
    HostToRelay,
    PushData,
    NativeBrowserCommand,
    NativeBrowserUiCommand,
    NativeBrowserMount,
    VaultHostCommand,
    VaultHostJob,
    VaultHostReply,
    DiagnosticReport,
    SpeechStatus,
    SpeechEvent,
    SpeechStart,
);

#[test]
fn every_manifest_type_has_a_rust_type() {
    assert_eq!(
        sorted(wire_types().into_iter().map(|(name, _)| name)),
        sorted(manifest().types.iter().map(|entry| entry.name.as_str()))
    );
}

#[test]
fn every_manifest_schema_compiles() {
    // Validators are built on first use, behind an expect. Building them all here makes a bad pattern fail CI instead
    // of panicking in the daemon on the first request that needs it.
    for name in METHODS.iter().chain(EVENTS) {
        let _ = validate(name, &serde_json::Value::Null);
    }
    for method in METHODS {
        let _ = Request::decode(method, serde_json::Value::Null);
    }
    for (_, compile) in wire_types() {
        compile();
    }
}

#[test]
fn protocol_version_and_error_codes_equal_the_manifest() {
    let manifest = manifest();
    assert_eq!(PROTOCOL_VERSION, manifest.protocol_version);
    assert_eq!(
        serde_json::to_value(ErrorCode::ALL).unwrap(),
        json!(manifest.error_codes)
    );
}

#[test]
fn method_and_event_names_do_not_overlap() {
    // assert_valid and validate take either kind of name.
    assert!(METHODS.iter().all(|method| !EVENTS.contains(method)));
}

#[test]
fn assert_valid_checks_results_and_event_data() {
    assert_valid(
        "settings.get",
        &json!({ "chainLimit": 5, "relayUrl": null }),
    );
    assert_valid("room.deleted", &json!({ "id": "r" }));
    assert!(
        validate(
            "settings.get",
            &json!({ "chainLimit": 0, "relayUrl": null })
        )
        .is_err()
    );
    assert!(validate("room.deleted", &json!({ "id": "r", "extra": true })).is_err());
}

#[test]
#[should_panic(expected = "settings.get")]
fn assert_valid_panics_on_an_invalid_result() {
    assert_valid(
        "settings.get",
        &json!({ "chainLimit": 21, "relayUrl": null }),
    );
}

#[test]
#[should_panic(expected = "no.such")]
fn assert_valid_panics_on_an_unknown_name() {
    assert_valid("no.such", &json!({}));
}

#[test]
fn decode_tells_unknown_methods_from_bad_params() {
    assert_eq!(
        Request::decode("no.such", json!({})),
        Err(DecodeError::UnknownMethod)
    );
    assert_eq!(DecodeError::UnknownMethod.code(), ErrorCode::UnknownMethod);
    let bad = Request::decode("bots.delete", json!({}));
    assert!(matches!(&bad, Err(DecodeError::BadParams(_))));
    assert_eq!(bad.unwrap_err().code(), ErrorCode::BadParams);
    assert_eq!(
        Request::decode("bots.delete", json!({ "id": "b" })),
        Ok(Request::BotsDelete(Id { id: "b".into() }))
    );
}

#[test]
fn bad_params_messages_never_echo_what_was_sent() {
    let secret = "SECRETKEY".repeat(10);
    for (method, params) in [
        (
            "accounts.create",
            json!({ "provider": "codex", "label": "API", "kind": "api_key", "apiKey": secret, "spendingCapUsd": -1 }),
        ),
        (
            "google.configure",
            json!({ "clientId": "https://evil.test", "clientSecret": secret }),
        ),
        ("device.registerPush", json!({ "token": secret.repeat(50) })),
    ] {
        let Err(DecodeError::BadParams(message)) = Request::decode(method, params) else {
            panic!("{method} accepted bad params");
        };
        assert!(!message.contains("SECRETKEY"), "{method}: {message}");
    }
}

#[test]
fn string_lengths_count_code_points_like_zod() {
    // zod 4.6.5 measures .min/.max in code points (checks.js:271-322, util.codePointLength), and so does JSON Schema.
    // Probed against zod: rooms.create accepts a 60-emoji name (120 UTF-16 units) and rejects 61 emoji.
    let face = "\u{1F600}";
    let room = |name: String| json!({ "name": name, "botIds": ["b"] });
    assert!(Request::decode("rooms.create", room(face.repeat(60))).is_ok());
    assert!(Request::decode("rooms.create", room(face.repeat(31))).is_ok());
    assert!(matches!(
        Request::decode("rooms.create", room(face.repeat(61))),
        Err(DecodeError::BadParams(_))
    ));
}

#[test]
fn method_markers_pair_each_name_with_its_params() {
    use protocol::wire::{Method, method};
    assert_eq!(method::ChatSend::NAME, "chat.send");
    let request = Request::decode("bots.delete", json!({ "id": "b" })).unwrap();
    assert_eq!(
        method::BotsDelete::take(request.clone()),
        Some(Id { id: "b".into() })
    );
    assert_eq!(method::BotsList::take(request), None);
}

#[test]
fn remote_frames_serialize_with_their_type_tag() {
    let host = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
    for frame in [
        json!({ "type": "confirm", "protocol": 1, "name": "My phone" }),
        json!({ "type": "ready", "deviceId": host, "hostId": host, "hostName": "My PC", "relay": null }),
        json!({ "type": "refused", "reason": "revoked" }),
    ] {
        let back = match frame["type"].as_str() {
            Some("confirm") => {
                serde_json::to_value(protocol::parse::<RemoteConfirm>(frame.clone()).unwrap())
            }
            Some("ready") => {
                serde_json::to_value(protocol::parse::<RemoteReady>(frame.clone()).unwrap())
            }
            _ => serde_json::to_value(protocol::parse::<RemoteRefused>(frame.clone()).unwrap()),
        };
        assert_eq!(back.unwrap(), frame);
    }
}

#[test]
fn events_and_responses_serialize_as_server_frames() {
    let event = Event::RoomDeleted(Id { id: "r".into() });
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "event": "room.deleted", "data": { "id": "r" } })
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::Event(event)).unwrap(),
        json!({ "type": "event", "event": "room.deleted", "data": { "id": "r" } })
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::Welcome {
            protocol: PROTOCOL_VERSION
        })
        .unwrap(),
        json!({ "type": "welcome", "protocol": 2 })
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::Res(Response::ok(1, json!({})))).unwrap(),
        json!({ "type": "res", "id": 1, "ok": true, "result": {} })
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::Res(Response::error(
            2,
            ErrorCode::NotFound,
            "No bot"
        )))
        .unwrap(),
        json!({ "type": "res", "id": 2, "ok": false, "error": { "code": "not_found", "message": "No bot" } })
    );
}
