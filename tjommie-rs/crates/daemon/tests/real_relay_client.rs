//! Port of `packages/daemon/src/relay-client.real.test.ts`, a real test and `#[ignore]` by default
//! (`tjommie-rs/test-map.md`): the Rust host and relay client with the TypeScript device, through the local relay.
//! Needs `node` on PATH and `npm ci` at the repository root.

mod remote_support;

use protocol::wire::{Empty, Event};
use remote_support::harness::{Harness, Setup};
use remote_support::probe::Probe;
use serde_json::json;

/// Through `packages/relay/src/local-relay.test-helper.ts` (wrangler on 127.0.0.1, no FCM credentials, so push stays
/// off): the Rust host's relay client logs in, and the TypeScript device pairs, talks, resumes and is refused, all over
/// relay channels. Like `relay-client.real.test.ts`, the relay URL is https and the sockets swap back to plain `ws:`.
#[tokio::test]
#[ignore = "real test: runs wrangler's local relay; cargo test --test real_relay_client -- --ignored"]
async fn a_typescript_device_pairs_resumes_and_is_refused_through_the_local_relay() {
    let mut probe = Probe::start();
    let started = probe.send(json!({ "op": "relay-start" })).await;
    assert_eq!(started["ok"], true, "{started}");
    let base = started["base"]
        .as_str()
        .unwrap()
        .replacen("http:", "https:", 1);
    assert!(base.starts_with("https://127.0.0.1:"), "{base}");
    let setting = base.clone();
    let h = Harness::start(Setup {
        relay_url: std::sync::Arc::new(move || Some(setting.clone())),
        relay_connect: std::sync::Arc::new(|url: String| {
            Box::pin(async move {
                daemon::peer_wire::Socket::connect(&url.replacen("wss:", "ws:", 1)).await
            })
        }),
        ..Setup::default()
    })
    .await;

    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let status = h.call("remote.status", json!({})).await.unwrap();
    assert_eq!(
        status["relay"],
        json!({ "url": base, "state": "online", "push": false }),
        "{status}"
    );
    let paired = probe
        .send(json!({ "op": "pair", "via": "relay", "code": invite["code"] }))
        .await;
    assert_eq!(paired["ok"], true, "{paired}");
    assert_eq!(paired["relay"], base.as_str());
    let device_id = paired["deviceId"].as_str().unwrap().to_owned();
    assert_eq!(
        probe
            .send(json!({ "op": "call", "method": "remote.ping", "params": {} }))
            .await,
        json!({ "ok": true, "result": {} })
    );
    let listed = probe
        .send(json!({ "op": "call", "method": "devices.list", "params": {} }))
        .await;
    assert_eq!(listed["result"][0]["id"], device_id.as_str(), "{listed}");
    h.remote.publish(&Event::DevicesChanged(Empty {}));
    let event = probe
        .send(json!({ "op": "event", "name": "devices.changed" }))
        .await;
    assert_eq!(event["ok"], true, "{event}");

    assert_eq!(probe.send(json!({ "op": "close" })).await["ok"], true);
    let resumed = probe.send(json!({ "op": "resume", "via": "relay" })).await;
    assert_eq!(resumed["deviceId"], device_id.as_str(), "{resumed}");
    assert_eq!(
        probe
            .send(json!({ "op": "call", "method": "remote.ping", "params": {} }))
            .await,
        json!({ "ok": true, "result": {} })
    );
    h.call("devices.revoke", json!({ "id": device_id }))
        .await
        .unwrap();
    let refusal = probe.send(json!({ "op": "resume", "via": "relay" })).await;
    assert_eq!(refusal, json!({ "ok": false, "refused": "revoked" }));

    h.remote.close().await;
    assert_eq!(probe.send(json!({ "op": "relay-stop" })).await["ok"], true);
    probe.stop().await;
}
