//! `remote.test.ts`

use super::{params, rejects, round_trip, with, without};
use protocol::model::Settings;
use protocol::parse;
use protocol::remote::{
    PairingCode, PushKind, RemotePreface, RemoteReady, RemoteRefused, encode_pairing_code,
    parse_pairing_code, remote_prologue,
};
use serde_json::{Value, json};

const NOW: i64 = 1_792_670_400_000; // Date.parse("2026-10-22T12:00:00.000Z")
const HOST_ID: &str = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
const PAIRING_ID: &str = "04364399-a4af-4ba6-a1e0-58ee51a40822";
// encodePairingCode(sample()) from the TypeScript protocol: the Rust encoding must match it byte for byte.
const SAMPLE_CODE: &str = "eyJ2IjoyLCJyZWxheSI6bnVsbCwicGFpcmluZ0lkIjoiMDQzNjQzOTktYTRhZi00YmE2LWExZTAtNThlZTUxYTQwODIyIiwiaG9zdElkIjoiOTljZjk4YmEtMjYzOC00YjE2LThhNmUtMTY5MWMxNjczYjQ1IiwiaG9zdE5hbWUiOiJNeSBQQyIsImhvc3RLZXkiOiJBUUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBIiwic2VjcmV0IjoiQWdBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQSIsImxhbiI6W3siaG9zdCI6IjE5Mi4xNjguMS4yIiwicG9ydCI6NDU3ODJ9XSwiZXhwaXJlcyI6IjIwMjYtMTAtMjJUMTI6MTA6MDAuMDAwWiJ9";
const INVALID: &str = "This pairing code is invalid or expired.";

fn sample() -> Value {
    json!({ "v": 2, "relay": null, "pairingId": PAIRING_ID, "hostId": HOST_ID, "hostName": "My PC", "hostKey": format!("{:A<43}", "AQ"),
        "secret": format!("{:A<43}", "Ag"), "lan": [{ "host": "192.168.1.2", "port": 45782 }], "expires": "2026-10-22T12:10:00.000Z" })
}

#[test]
fn round_trips_a_bounded_one_use_pairing_code_and_its_app_uri() {
    let input: PairingCode = parse(sample()).unwrap();
    let code = encode_pairing_code(&input).unwrap();
    assert_eq!(code, SAMPLE_CODE);
    assert_eq!(parse_pairing_code(&code, NOW).unwrap(), input);
    assert_eq!(
        parse_pairing_code(&format!("tjommie://pair#{code}"), NOW).unwrap(),
        input
    );
    assert!(!code.contains(input.secret.as_str()));
}

#[test]
fn accepts_v2_pairing_codes_with_an_https_relay_or_no_relay() {
    assert!(parse::<PairingCode>(sample()).is_ok());
    let input: PairingCode = parse(with(
        sample(),
        json!({ "relay": "https://r.example.workers.dev" }),
    ))
    .unwrap();
    assert_eq!(
        parse_pairing_code(&encode_pairing_code(&input).unwrap(), NOW).unwrap(),
        input
    );
    for bad in [
        with(sample(), json!({ "v": 1 })),
        with(sample(), json!({ "relay": "http://r.example.workers.dev" })),
        with(sample(), json!({ "relay": "wss://r.example.workers.dev" })),
    ] {
        assert!(parse::<PairingCode>(bad).is_err());
    }
}

#[test]
fn requires_a_relay_on_ready_and_accepts_only_named_refusal_reasons() {
    let ready = json!({ "type": "ready", "deviceId": PAIRING_ID, "hostId": HOST_ID, "hostName": "My PC", "relay": null });
    assert!(parse::<RemoteReady>(ready.clone()).is_ok());
    assert!(
        parse::<RemoteReady>(with(
            ready.clone(),
            json!({ "relay": "https://r.example.workers.dev" })
        ))
        .is_ok()
    );
    assert!(parse::<RemoteReady>(without(ready, "relay")).is_err());
    for reason in ["revoked", "unknown"] {
        assert!(parse::<RemoteRefused>(json!({ "type": "refused", "reason": reason })).is_ok());
    }
    assert!(parse::<RemoteRefused>(json!({ "type": "refused", "reason": "expired" })).is_err());
}

#[test]
fn accepts_a_null_relay_url_in_settings() {
    let _: Settings = round_trip("settings.get", json!({ "chainLimit": 5, "relayUrl": null }));
    assert_eq!(
        params("settings.set", json!({ "chainLimit": 5, "relayUrl": null })),
        json!({ "chainLimit": 5, "relayUrl": null })
    );
    rejects(
        "settings.set",
        json!({ "chainLimit": 5, "relayUrl": "http://r.example" }),
    );
    assert!(
        serde_json::from_value::<Settings>(
            json!({ "chainLimit": 5, "relayUrl": "http://r.example" })
        )
        .is_err()
    );
}

// Not in remote.test.ts. Probed against zod 4.6.5: its URL check trims with String.prototype.trim (so NBSP too), stores
// the trimmed text, and checks .max(512) after trimming.
#[test]
fn trims_a_relay_url_like_zod_before_checking_its_length() {
    let set = |url: String| json!({ "relayUrl": url });
    let url = "https://r.example.workers.dev";
    assert_eq!(
        params("settings.set", set(format!("\u{a0}{url}\u{a0}")))["relayUrl"],
        url
    );
    let long = format!("https://r.example/{}", "a".repeat(512 - 18));
    assert_eq!(
        params("settings.set", set(format!("\u{a0}{long}\u{a0}")))["relayUrl"],
        long
    );
    rejects("settings.set", set(format!("{long}a")));
}

#[test]
fn accepts_optional_push_kinds_when_registering_a_device() {
    for registration in [
        json!({ "token": "synthetic-token" }),
        json!({ "token": "synthetic-token", "kinds": [] }),
        json!({ "token": "synthetic-token", "kinds": ["done"] }),
    ] {
        assert_eq!(
            params("device.registerPush", registration.clone()),
            registration
        );
    }
    let all = json!(["approval", "limit", "routine_failed", "done", "failed"]);
    assert_eq!(serde_json::to_value(PushKind::ALL).unwrap(), all);
    assert_eq!(
        params(
            "device.registerPush",
            json!({ "token": "synthetic-token", "kinds": all })
        )["kinds"],
        all
    );
    assert_eq!(
        params(
            "device.registerPush",
            json!({ "token": "synthetic-token", "kinds": ["done", "mention"] })
        )["kinds"],
        json!(["done", "mention"])
    );
    rejects(
        "device.registerPush",
        json!({ "token": "synthetic-token", "kinds": ["x".repeat(33)] }),
    );
}

#[test]
fn rejects_expired_overlong_malformed_and_unsupported_pairing_inputs_without_echoing_them() {
    let code = encode_pairing_code(&parse(sample()).unwrap()).unwrap();
    for value in [
        "secret-token".to_owned(),
        "x".repeat(9000),
        "https://example.com/#abc".to_owned(),
        format!("{code}="),
    ] {
        assert_eq!(
            parse_pairing_code(&value, NOW).unwrap_err().to_string(),
            INVALID
        );
    }
    assert!(parse_pairing_code(&code, NOW + 600_000).is_err());
    assert!(parse_pairing_code(&code, NOW - 120_000).is_err());
    assert!(
        parse::<PairingCode>(with(
            sample(),
            json!({ "lan": [{ "host": "example.com", "port": 1234 }] })
        ))
        .is_err()
    );
    assert!(parse::<PairingCode>(with(sample(), json!({ "secret": "A".repeat(42) }))).is_err());
    assert!(
        parse::<PairingCode>(with(
            sample(),
            json!({ "relay": "http://relay.example.com" })
        ))
        .is_err()
    );
}

#[test]
fn writes_the_noise_prologue_byte_for_byte_like_typescript() {
    // remotePrologue output from the TypeScript protocol. Keys follow the schema's order, not the caller's.
    let pair = remote_prologue(&RemotePreface::pair(HOST_ID, PAIRING_ID)).unwrap();
    assert_eq!(
        String::from_utf8(pair).unwrap(),
        format!(
            "Tjommie remote v1\n{{\"v\":1,\"hostId\":\"{HOST_ID}\",\"mode\":\"pair\",\"pairingId\":\"{PAIRING_ID}\"}}"
        )
    );
    let resume = remote_prologue(&RemotePreface::resume(HOST_ID)).unwrap();
    assert_eq!(
        String::from_utf8(resume).unwrap(),
        format!("Tjommie remote v1\n{{\"v\":1,\"hostId\":\"{HOST_ID}\",\"mode\":\"resume\"}}")
    );
    assert!(
        parse::<RemotePreface>(
            json!({ "v": 1, "hostId": HOST_ID, "mode": "resume", "pairingId": PAIRING_ID })
        )
        .is_err()
    );
    assert!(parse::<RemotePreface>(json!({ "v": 1, "hostId": HOST_ID, "mode": "pair" })).is_err());
}
