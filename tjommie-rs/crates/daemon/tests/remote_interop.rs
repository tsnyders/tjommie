//! Interop with the TypeScript client (`packages/client`). `tools/remote-probe.ts` runs `authenticateRemote` and
//! `connectTransport` as a device against this Rust host: over LAN TCP on 127.0.0.1, and through the local relay
//! (`packages/relay/src/local-relay.test-helper.ts`). Needs `node` on PATH and `npm ci` at the repository root.

mod remote_support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use daemon::devices_store::list_devices;
use daemon::server::{Handlers, RpcError};
use protocol::ErrorCode;
use protocol::attachments::AttachmentChunk;
use protocol::wire::{Empty, Event, method};
use remote_support::harness::{Harness, Setup};
use remote_support::probe::Probe;
use serde_json::{Value, json};

/// Over 64 KiB once base64-encoded, so the response spans several Noise records.
const LARGE: usize = 96 * 1024;

/// `attachments.read` answering with a large chunk, so a response crosses Noise record boundaries.
fn large_reads(handlers: Handlers) -> Handlers {
    handlers.on::<method::AttachmentsRead, _>(|params, _| async move {
        Ok::<_, RpcError>(AttachmentChunk {
            transfer_id: params.transfer_id,
            revision: "0".repeat(64),
            offset: params.offset,
            bytes: STANDARD.encode(vec![7u8; LARGE]),
            eof: true,
        })
    })
}

#[tokio::test]
async fn a_typescript_device_pairs_resumes_and_is_refused_over_lan() {
    let h = Harness::start(Setup {
        extra: large_reads,
        ..Setup::default()
    })
    .await;
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let mut probe = Probe::start();

    let paired = probe
        .send(json!({ "op": "pair", "via": "lan", "code": invite["code"] }))
        .await;
    assert_eq!(paired["ok"], true, "{paired}");
    let device_id = paired["deviceId"].as_str().unwrap().to_owned();
    assert_eq!(paired["relay"], Value::Null);
    let devices = list_devices(&h.db.lock().unwrap()).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, device_id);
    assert_eq!(devices[0].name.as_str(), "Synthetic TypeScript device");

    // Requests the host answers itself, requests it dispatches to the daemon, and an event.
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
    let registered = probe
        .send(json!({ "op": "call", "method": "device.registerPush", "params": { "token": "synthetic-fcm-token", "kinds": ["done", "mention"] } }))
        .await;
    assert_eq!(registered, json!({ "ok": true, "result": {} }));
    let targets = h.remote.host().push_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].token, "synthetic-fcm-token");
    h.remote.publish(&Event::DevicesChanged(Empty {}));
    let event = probe
        .send(json!({ "op": "event", "name": "devices.changed" }))
        .await;
    assert_eq!(
        event,
        json!({ "ok": true, "type": "event", "event": "devices.changed", "data": {} })
    );

    // A response and a request that each span several Noise records.
    let transfer = "13653af3-8ca6-4e95-bc95-0949edc28715";
    let read = probe
        .send(json!({ "op": "call", "method": "attachments.read", "params": { "transferId": transfer, "offset": 0, "length": LARGE } }))
        .await;
    assert_eq!(read["ok"], true, "{}", &read.to_string()[..200]);
    assert_eq!(
        STANDARD
            .decode(read["result"]["bytes"].as_str().unwrap())
            .unwrap(),
        vec![7u8; LARGE]
    );
    let long_name = "n".repeat(70_000);
    let refused = probe
        .send(json!({ "op": "call", "method": "devices.rename", "params": { "id": device_id, "name": long_name } }))
        .await;
    assert_eq!(
        refused["code"],
        "bad_params",
        "{}",
        &refused.to_string()[..200]
    );
    assert_eq!(
        refused["message"],
        "bad_params: This request could not be completed on the PC."
    );

    // Resume with the same key, then revoke: the live client drops and a new resume is refused.
    assert_eq!(probe.send(json!({ "op": "close" })).await["ok"], true);
    let resumed = probe.send(json!({ "op": "resume", "via": "lan" })).await;
    assert_eq!(resumed["deviceId"], device_id.as_str(), "{resumed}");
    let status = probe
        .send(json!({ "op": "call", "method": "remote.status", "params": {} }))
        .await;
    assert_eq!(status["result"]["connectedDeviceIds"], json!([device_id]));
    h.call("devices.revoke", json!({ "id": device_id }))
        .await
        .unwrap();
    let after = probe
        .send(json!({ "op": "call", "method": "remote.ping", "params": {} }))
        .await;
    assert_eq!(after["ok"], false, "{after}");
    let refusal = probe.send(json!({ "op": "resume", "via": "lan" })).await;
    assert_eq!(refusal, json!({ "ok": false, "refused": "revoked" }));
    probe.stop().await;
    h.remote.close().await;
}

#[tokio::test]
async fn a_typescript_device_with_the_wrong_secret_or_an_unknown_key_gets_nothing() {
    let h = Harness::start(Setup::default()).await;
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let mut probe = Probe::start();
    let wrong = probe
        .send(json!({ "op": "pair", "via": "lan", "code": invite["code"], "psk": "wrong" }))
        .await;
    assert_eq!(wrong["ok"], false, "{wrong}");
    assert!(list_devices(&h.db.lock().unwrap()).unwrap().is_empty());
    let unknown = probe.send(json!({ "op": "resume", "via": "lan" })).await;
    assert_eq!(unknown, json!({ "ok": false, "refused": "unknown" }));
    // The code is still unused, so the right secret still pairs.
    let paired = probe
        .send(json!({ "op": "pair", "via": "lan", "code": invite["code"] }))
        .await;
    assert_eq!(paired["ok"], true, "{paired}");
    assert_eq!(
        h.call(
            "devices.revoke",
            json!({ "id": "13653af3-8ca6-4e95-bc95-0949edc28715" })
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    probe.stop().await;
    h.remote.close().await;
}

/// Through `packages/relay/src/local-relay.test-helper.ts` (wrangler on 127.0.0.1, no FCM credentials, so push stays
/// off): the Rust host's relay client logs in, and the TypeScript device pairs, talks, resumes and is refused, all over
/// relay channels. Like `relay-client.real.test.ts`, the relay URL is https and the sockets swap back to plain `ws:`.
#[tokio::test]
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
