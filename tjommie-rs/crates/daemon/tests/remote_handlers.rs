//! The `pairing.*`, `devices.*`, `device.*` and `remote.*` methods (`daemon.ts:334-404`), called through the server as
//! a local client and, for a paired device, through the remote host. Every result is checked against the manifest.

mod remote_support;

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daemon::devices_store::add_device;
use protocol::ErrorCode;
use protocol::remote::{RemotePreface, parse_pairing_code};
use remote_support::device::{Pairing, authenticate};
use remote_support::harness::{Harness, Setup};
use remote_support::{MemoryVault, key_of, shared_db};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use transport::generate_keypair;

async fn harness_with(db: Arc<Mutex<rusqlite::Connection>>, vault: Arc<MemoryVault>) -> Harness {
    Harness::start(Setup {
        db,
        vault,
        ..Setup::default()
    })
    .await
}

async fn harness() -> Harness {
    Harness::start(Setup::default()).await
}

#[tokio::test]
async fn answers_ping_and_refuses_push_registration_from_this_pc() {
    let h = harness().await;
    assert_eq!(h.call("remote.ping", json!({})).await, Ok(json!({})));
    let refused = h
        .call("device.registerPush", json!({ "token": "synthetic-token" }))
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::BadState);
    assert_eq!(
        refused.message,
        "Push registration is available only to paired devices."
    );
    h.remote.close().await;
}

#[tokio::test]
async fn reports_status_before_and_after_pairing_starts_the_listener() {
    let h = harness().await;
    let before = h.call("remote.status", json!({})).await.unwrap();
    assert_eq!(before["listening"], false);
    assert_eq!(before["hostId"], Value::Null);
    assert_eq!(
        before["relay"],
        json!({ "url": null, "state": "off", "push": false })
    );
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    assert_eq!(
        invite["uri"],
        format!("tjommie://pair#{}", invite["code"].as_str().unwrap())
    );
    assert_eq!(code.relay, None);
    let after = h.call("remote.status", json!({})).await.unwrap();
    assert_eq!(after["listening"], true);
    assert_eq!(after["hostId"], code.host_id.as_str());
    assert_eq!(after["port"], code.lan[0].port);
    assert_eq!(code.lan[0].host, "127.0.0.1");
    h.remote.close().await;
}

#[tokio::test]
async fn cancels_a_pairing_code() {
    let h = harness().await;
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let id = invite["id"].as_str().unwrap();
    assert_eq!(
        h.call("pairing.cancel", json!({ "id": id })).await,
        Ok(json!({}))
    );
    let rows: i64 =
        h.db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM pairing_codes", [], |row| row.get(0))
            .unwrap();
    assert_eq!(rows, 0);
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    let wire = daemon::peer_wire::PeerWire::tcp(
        TcpStream::connect(("127.0.0.1", code.lan[0].port))
            .await
            .unwrap(),
    );
    let keys = generate_keypair().unwrap();
    let paired = authenticate(
        wire,
        Pairing {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key: &keys.private_key[..],
            host_key: &URL_SAFE_NO_PAD.decode(code.host_key.as_str()).unwrap(),
            psk: Some(&URL_SAFE_NO_PAD.decode(code.secret.as_str()).unwrap()),
            name: "Synthetic Android",
        },
    )
    .await;
    assert!(paired.is_err());
    h.remote.close().await;
}

#[tokio::test]
async fn lists_renames_and_revokes_devices_and_announces_changes() {
    let h = harness().await;
    let device = add_device(&h.db.lock().unwrap(), &key_of(9), "Synthetic phone").unwrap();
    let listed = h.call("devices.list", json!({})).await.unwrap();
    assert_eq!(listed[0]["id"], device.id.as_str());
    let renamed = h
        .call(
            "devices.rename",
            json!({ "id": device.id, "name": "  Renamed phone " }),
        )
        .await
        .unwrap();
    assert_eq!(renamed["name"], "Renamed phone");
    assert_eq!(h.changes(), 1);
    let revoked = h
        .call("devices.revoke", json!({ "id": device.id }))
        .await
        .unwrap();
    assert!(revoked["revokedAt"].is_string());
    assert_eq!(h.changes(), 2);
    let missing = "13653af3-8ca6-4e95-bc95-0949edc28715";
    assert_eq!(
        h.call("devices.rename", json!({ "id": missing, "name": "x" }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let local = h
        .call("devices.rename", json!({ "id": device.id, "name": " " }))
        .await
        .unwrap_err();
    assert_ne!(
        local.message, "This request could not be completed on the PC.",
        "local callers keep the detail"
    );
    assert_eq!(
        h.call("devices.rename", json!({ "id": device.id, "name": " " }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::BadParams
    );
    h.remote.close().await;
}

#[tokio::test]
async fn a_paired_device_calls_the_daemons_methods_with_its_own_principal() {
    let h = harness().await;
    let invite = h.call("pairing.create", json!({})).await.unwrap();
    let code = parse_pairing_code(
        invite["code"].as_str().unwrap(),
        jiff::Timestamp::now().as_millisecond(),
    )
    .unwrap();
    let wire = daemon::peer_wire::PeerWire::tcp(
        TcpStream::connect(("127.0.0.1", code.lan[0].port))
            .await
            .unwrap(),
    );
    let keys = generate_keypair().unwrap();
    let paired = authenticate(
        wire,
        Pairing {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key: &keys.private_key[..],
            host_key: &URL_SAFE_NO_PAD.decode(code.host_key.as_str()).unwrap(),
            psk: Some(&URL_SAFE_NO_PAD.decode(code.secret.as_str()).unwrap()),
            name: "Synthetic Android",
        },
    )
    .await
    .map_err(|(error, _)| error)
    .unwrap();
    let device_id = paired.device_id.clone();
    let phone = paired.connect().await;
    let listed = phone.call("devices.list", json!({})).await.unwrap();
    assert_eq!(listed[0]["id"], device_id.as_str());
    assert_eq!(listed[0]["name"], "Synthetic Android");
    let status = phone.call("remote.status", json!({})).await.unwrap();
    assert_eq!(status["connectedDeviceIds"], json!([device_id]));
    assert_eq!(
        phone
            .call(
                "device.registerPush",
                json!({ "token": "synthetic-fcm-token" })
            )
            .await,
        Ok(json!({}))
    );
    assert_eq!(
        phone.call("bots.list", json!({})).await.unwrap_err().0,
        "unknown_method",
        "methods without a handler answer unknown_method"
    );
    assert!(h.changes() >= 1, "pairing announces devices.changed");
    phone.close();
    h.remote.close().await;
}

#[tokio::test]
async fn listens_at_start_only_when_devices_are_paired() {
    let (db, vault) = (shared_db(), Arc::new(MemoryVault::default()));
    let first = harness_with(db.clone(), vault.clone()).await;
    first.remote.start().await;
    assert_eq!(
        first.call("remote.status", json!({})).await.unwrap()["listening"],
        false
    );
    // A pairing code creates this PC's identity; then a device pairs and the daemon restarts.
    first.call("pairing.create", json!({})).await.unwrap();
    add_device(&db.lock().unwrap(), &key_of(4), "Synthetic phone").unwrap();
    first.remote.close().await;
    let second = harness_with(db, vault).await;
    second.remote.start().await;
    let status = second.call("remote.status", json!({})).await.unwrap();
    assert_eq!(status["listening"], true, "{status}");
    second.remote.close().await;
}
