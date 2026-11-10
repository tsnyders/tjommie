//! Port of `packages/daemon/src/remote-host.test.ts`. LAN cases use real TCP on 127.0.0.1; relay cases use an in-memory
//! wire like the TypeScript test's `relayWire`. The two timer cases run on paused tokio time over in-memory wires of
//! both kinds (the sweep doesn't look at the wire type).

mod remote_support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daemon::devices_store::{PushTarget, add_device, list_devices};
use daemon::peer_wire::{PeerWire, WireKind};
use daemon::remote_host::{Dispatch, RemoteHost, RemoteHostOptions, StartError};
use daemon::server::{Principal, RequestContext, RpcError};
use protocol::ErrorCode;
use protocol::remote::{PairingCode, PushKind, RefusedReason, RemotePreface, parse_pairing_code};
use protocol::wire::{Empty, Event};
use remote_support::device::{AuthError, Authenticated, Pairing, authenticate};
use remote_support::{MemoryVault, shared_db};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use transport::generate_keypair;

type Db = Arc<Mutex<Connection>>;

const WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug)]
enum Kind {
    Lan,
    Relay,
}

struct Fixture {
    db: Db,
    host: RemoteHost,
    code: PairingCode,
    mutations: Arc<Mutex<u32>>,
    contexts: Arc<Mutex<Vec<RequestContext>>>,
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn options(
    db: &Db,
    contexts: Arc<Mutex<Vec<RequestContext>>>,
    mutations: Arc<Mutex<u32>>,
) -> RemoteHostOptions {
    let dispatch: Dispatch = Arc::new(
        move |method: String, _params: Value, context: RequestContext| {
            contexts.lock().unwrap().push(context);
            let mutations = mutations.clone();
            Box::pin(async move {
                match method.as_str() {
                    "chat.stop" => {
                        *mutations.lock().unwrap() += 1;
                        Ok(json!({}))
                    }
                    "bots.list" => Ok(json!([])),
                    _ => Err(RpcError::new(
                        ErrorCode::Internal,
                        "synthetic failure with private detail",
                    )),
                }
            })
        },
    );
    let mut options =
        RemoteHostOptions::new(db.clone(), PathBuf::from("synthetic-remote-root"), dispatch);
    options.port = 0;
    options.bind = [127, 0, 0, 1].into();
    options.addresses = Some(Arc::new(|| vec!["127.0.0.1".into()]));
    options.vault = Arc::new(MemoryVault::default());
    options
}

async fn fixture_with(relay_url: Option<Arc<Mutex<String>>>) -> Fixture {
    let db = shared_db();
    let (contexts, mutations) = (Arc::default(), Arc::default());
    let mut options = options(&db, Arc::clone(&contexts), Arc::clone(&mutations));
    if let Some(url) = relay_url {
        options.relay_url = Arc::new(move |_| Some(url.lock().unwrap().clone()));
    }
    let host = RemoteHost::new(options);
    let invite = host.create_pairing().await.unwrap();
    let code = parse_pairing_code(&invite.code, now_ms()).unwrap();
    Fixture {
        db,
        host,
        code,
        mutations,
        contexts,
    }
}

async fn fixture() -> Fixture {
    fixture_with(None).await
}

/// The device's side of an in-memory wire the host accepted, like the TypeScript test's `relayWire`.
fn memory_wire(host: &RemoteHost, address: &str, kind: WireKind) -> PeerWire {
    let (host_side, device_side) = PeerWire::memory(kind, address);
    host.accept_wire(host_side);
    device_side
}

fn relay_wire(host: &RemoteHost) -> PeerWire {
    memory_wire(host, "relay", WireKind::Relay)
}

async fn tcp_wire(port: u16) -> PeerWire {
    PeerWire::tcp(TcpStream::connect(("127.0.0.1", port)).await.unwrap())
}

async fn open(kind: Kind, host: &RemoteHost, code: &PairingCode) -> PeerWire {
    match kind {
        Kind::Lan => tcp_wire(code.lan[0].port).await,
        Kind::Relay => relay_wire(host),
    }
}

fn bytes(key: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(key).unwrap()
}

async fn pair(
    code: &PairingCode,
    private_key: &[u8],
    psk: &[u8],
    wire: PeerWire,
) -> Result<Authenticated, AuthError> {
    authenticate(
        wire,
        Pairing {
            preface: RemotePreface::pair(&code.host_id, &code.pairing_id),
            private_key,
            host_key: &bytes(code.host_key.as_str()),
            psk: Some(psk),
            name: "Synthetic Android",
        },
    )
    .await
    .map_err(|(error, _)| error)
}

async fn resume(
    code: &PairingCode,
    private_key: &[u8],
    wire: PeerWire,
) -> Result<Authenticated, (AuthError, PeerWire)> {
    authenticate(
        wire,
        Pairing {
            preface: RemotePreface::resume(&code.host_id),
            private_key,
            host_key: &bytes(code.host_key.as_str()),
            psk: None,
            name: "Ignored on reconnect",
        },
    )
    .await
}

fn secret(code: &PairingCode) -> Vec<u8> {
    bytes(code.secret.as_str())
}

fn devices_changed() -> Event {
    Event::DevicesChanged(Empty {})
}

fn device_tokens(db: &Db, id: &str) -> (Option<String>, Option<String>) {
    db.lock()
        .unwrap()
        .query_row(
            "SELECT push_token, push_kinds FROM devices WHERE id = ?",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn used_at(db: &Db) -> Option<String> {
    db.lock()
        .unwrap()
        .query_row("SELECT used_at FROM pairing_codes", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn pairs_and_resumes_before_being_refused_after_revoke() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture {
            db,
            host,
            code,
            mutations,
            contexts,
        } = fixture().await;
        let keys = generate_keypair().unwrap();
        let paired = pair(
            &code,
            &keys.private_key[..],
            &secret(&code),
            open(kind, &host, &code).await,
        )
        .await
        .unwrap();
        assert_eq!(list_devices(&db.lock().unwrap()).unwrap().len(), 1);
        // No RPC or event data is sent before the channel's encrypted hello/welcome completes.
        host.broadcast(&devices_changed());
        let device_id = paired.device_id.clone();
        let mut client = paired.connect().await;
        assert_eq!(client.call("bots.list", json!({})).await, Ok(json!([])));
        client
            .call("chat.stop", json!({ "botId": "synthetic" }))
            .await
            .unwrap();
        assert_eq!(*mutations.lock().unwrap(), 1);
        let first = contexts.lock().unwrap()[0].clone();
        assert_eq!(
            first.principal,
            Principal::Device {
                device_id: device_id.clone()
            }
        );
        assert_eq!(
            contexts.lock().unwrap()[1].connection_id,
            first.connection_id
        );
        host.broadcast(&devices_changed());
        assert_eq!(client.next_event().await["event"], "devices.changed");
        client.close();
        timeout(WAIT, first.signal.cancelled()).await.unwrap();
        let resumed = resume(&code, &keys.private_key[..], open(kind, &host, &code).await)
            .await
            .map_err(|(error, _)| error)
            .unwrap();
        assert_eq!(resumed.device_id, device_id);
        let second = resumed.connect().await;
        assert_eq!(second.call("bots.list", json!({})).await, Ok(json!([])));
        let third = contexts.lock().unwrap()[2].clone();
        assert_ne!(third.connection_id, first.connection_id);
        assert_eq!(
            third.principal,
            Principal::Device {
                device_id: device_id.clone()
            }
        );
        host.revoke(&device_id).unwrap();
        assert!(third.signal.is_cancelled(), "{kind:?}");
        second.disconnected().await;
        assert!(
            list_devices(&db.lock().unwrap()).unwrap()[0]
                .revoked_at
                .is_some()
        );
        let refused = resume(&code, &keys.private_key[..], open(kind, &host, &code).await).await;
        assert_eq!(
            refused.err().map(|(error, _)| error),
            Some(AuthError::Refused(RefusedReason::Revoked))
        );
        host.close().await;
    }
}

#[tokio::test]
async fn authentically_refuses_an_unknown_resume_key() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture { host, code, .. } = fixture().await;
        let refused = resume(
            &code,
            &generate_keypair().unwrap().private_key[..],
            open(kind, &host, &code).await,
        )
        .await;
        assert_eq!(
            refused.err().map(|(error, _)| error),
            Some(AuthError::Refused(RefusedReason::Unknown)),
            "{kind:?}"
        );
        host.close().await;
    }
}

#[tokio::test]
async fn closes_a_refused_peer_even_when_the_client_leaves_its_wire_open() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture { host, code, .. } = fixture().await;
        let refusal = resume(
            &code,
            &generate_keypair().unwrap().private_key[..],
            open(kind, &host, &code).await,
        )
        .await;
        let Err((AuthError::Refused(RefusedReason::Unknown), mut wire)) = refusal else {
            panic!("expected an unknown-key refusal over {kind:?}");
        };
        // The device keeps reading and never closes; the host must close after the refusal leaves.
        assert_eq!(
            timeout(Duration::from_secs(10), wire.next_frame())
                .await
                .unwrap(),
            None
        );
        host.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn sweeps_relay_peers_and_starts_only_one_sweep_when_lan_listening_fails() {
    let blocker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let db = shared_db();
    let mut options = options(&db, Arc::default(), Arc::default());
    options.port = blocker.local_addr().unwrap().port();
    let host = RemoteHost::new(options);
    let failure = host.ensure_listening().await.unwrap_err();
    assert!(
        matches!(&failure, StartError::Listen(error) if error.kind() == std::io::ErrorKind::AddrInUse),
        "{failure:?}"
    );
    let host_id = host
        .status()
        .host_id
        .expect("the identity loads before listening");
    let host_key: String = db
        .lock()
        .unwrap()
        .query_row("SELECT public_key FROM remote_host", [], |row| row.get(0))
        .unwrap();
    let keys = generate_keypair().unwrap();
    let device = add_device(
        &db.lock().unwrap(),
        &URL_SAFE_NO_PAD.encode(keys.public_key),
        "Relay phone",
    )
    .unwrap();
    let paired = authenticate(
        relay_wire(&host),
        Pairing {
            preface: RemotePreface::resume(&host_id),
            private_key: &keys.private_key[..],
            host_key: &bytes(&host_key),
            psk: None,
            name: "Relay phone",
        },
    )
    .await
    .map_err(|(error, _)| error)
    .unwrap();
    let client = paired.connect().await;
    assert_eq!(host.status().connected_device_ids, [device.id]);
    assert!(matches!(
        host.ensure_listening().await,
        Err(StartError::Listen(_))
    ));
    tokio::time::advance(Duration::from_secs(150)).await;
    client.disconnected().await;
    tokio::task::yield_now().await;
    assert_eq!(host.status().connected_device_ids, Vec::<String>::new());
    host.close().await;
}

#[tokio::test]
async fn includes_the_current_relay_url_in_pairing_and_ready_and_handles_ping_locally() {
    let url = Arc::new(Mutex::new("https://relay.example.workers.dev".to_owned()));
    let Fixture {
        host,
        code,
        contexts,
        ..
    } = fixture_with(Some(url.clone())).await;
    assert_eq!(code.v, 2);
    assert_eq!(
        code.relay.as_ref().map(|relay| relay.as_str()),
        Some("https://relay.example.workers.dev")
    );
    assert_eq!(
        host.status()
            .relay
            .url
            .map(|relay| relay.as_str().to_owned())
            .as_deref(),
        Some("https://relay.example.workers.dev")
    );
    *url.lock().unwrap() = "https://next.example.workers.dev".into();
    let paired = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        relay_wire(&host),
    )
    .await
    .unwrap();
    assert_eq!(
        paired.relay.as_deref(),
        Some("https://next.example.workers.dev")
    );
    let client = paired.connect().await;
    assert_eq!(client.call("remote.ping", json!({})).await, Ok(json!({})));
    assert!(
        contexts.lock().unwrap().is_empty(),
        "remote.ping is answered by the host"
    );
    client.close();
    host.close().await;
}

#[tokio::test]
async fn registers_and_clears_only_the_authenticated_peers_push_token() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture { db, host, code, .. } = fixture().await;
        let paired = pair(
            &code,
            &generate_keypair().unwrap().private_key[..],
            &secret(&code),
            open(kind, &host, &code).await,
        )
        .await
        .unwrap();
        let paired_id = paired.device_id.clone();
        let client = paired.connect().await;
        let next =
            parse_pairing_code(&host.create_pairing().await.unwrap().code, now_ms()).unwrap();
        let other = pair(
            &next,
            &generate_keypair().unwrap().private_key[..],
            &secret(&next),
            open(kind, &host, &next).await,
        )
        .await
        .unwrap();
        let other_id = other.device_id.clone();
        let other_client = other.connect().await;
        let target = |id: &str, token: &str, kinds| PushTarget {
            device_id: id.into(),
            token: token.into(),
            kinds,
        };
        assert_eq!(
            other_client
                .call(
                    "device.registerPush",
                    json!({ "token": "other-device-token" })
                )
                .await,
            Ok(json!({}))
        );
        assert_eq!(
            client
                .call(
                    "device.registerPush",
                    json!({ "token": "synthetic-fcm-token", "kinds": ["done", "mention"] })
                )
                .await,
            Ok(json!({}))
        );
        assert_eq!(
            device_tokens(&db, &paired_id),
            (
                Some("synthetic-fcm-token".into()),
                Some(r#"["done"]"#.into())
            )
        );
        assert_eq!(
            device_tokens(&db, &other_id).0.as_deref(),
            Some("other-device-token")
        );
        let targets = host.push_targets();
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&target(
            &paired_id,
            "synthetic-fcm-token",
            Some(vec![PushKind::Done])
        )));
        host.clear_push_token(&paired_id, "synthetic-fcm-token")
            .unwrap();
        assert_eq!(
            host.push_targets(),
            [target(&other_id, "other-device-token", None)]
        );
        assert_eq!(
            client
                .call(
                    "device.registerPush",
                    json!({ "token": "synthetic-fcm-token" })
                )
                .await,
            Ok(json!({}))
        );
        assert_eq!(
            client
                .call("device.registerPush", json!({ "token": null }))
                .await,
            Ok(json!({}))
        );
        assert_eq!(
            host.push_targets(),
            [target(&other_id, "other-device-token", None)]
        );
        assert_eq!(
            client
                .call(
                    "device.registerPush",
                    json!({ "token": "synthetic-fcm-token" })
                )
                .await,
            Ok(json!({}))
        );
        host.revoke(&paired_id).unwrap();
        assert_eq!(
            host.push_targets(),
            [target(&other_id, "other-device-token", None)]
        );
        assert_eq!(device_tokens(&db, &paired_id), (None, None));
        other_client.close();
        host.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn keeps_a_peer_alive_on_ping_then_closes_it_after_150_seconds_without_inbound_frames() {
    for kind in [WireKind::Lan, WireKind::Relay] {
        let Fixture { host, code, .. } = fixture().await;
        let wire = memory_wire(&host, "192.0.2.10", kind);
        let paired = pair(
            &code,
            &generate_keypair().unwrap().private_key[..],
            &secret(&code),
            wire,
        )
        .await
        .unwrap();
        let device_id = paired.device_id.clone();
        let client = paired.connect().await;
        tokio::time::advance(Duration::from_secs(120)).await;
        assert_eq!(client.call("remote.ping", json!({})).await, Ok(json!({})));
        tokio::time::advance(Duration::from_secs(120)).await;
        assert_eq!(
            host.status().connected_device_ids,
            std::slice::from_ref(&device_id),
            "{kind:?}"
        );
        assert!(client.is_connected());
        tokio::time::advance(Duration::from_secs(30)).await;
        client.disconnected().await;
        tokio::task::yield_now().await;
        assert_eq!(host.status().connected_device_ids, Vec::<String>::new());
        host.close().await;
    }
}

#[tokio::test]
async fn applies_relay_pending_and_active_limits_without_a_per_address_relay_cap() {
    let Fixture { host, code, .. } = fixture().await;
    let mut pending: Vec<PeerWire> = (0..8).map(|_| relay_wire(&host)).collect();
    let paired = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        relay_wire(&host),
    )
    .await
    .unwrap();
    assert_eq!(
        timeout(WAIT, pending[0].next_frame()).await.unwrap(),
        None,
        "the oldest pending relay peer rotates out"
    );
    for wire in &pending {
        wire.writer().destroy();
    }
    let mut active = vec![paired.connect().await];
    for _ in 1..32 {
        let next =
            parse_pairing_code(&host.create_pairing().await.unwrap().code, now_ms()).unwrap();
        let paired = pair(
            &next,
            &generate_keypair().unwrap().private_key[..],
            &secret(&next),
            relay_wire(&host),
        )
        .await
        .unwrap();
        active.push(paired.connect().await);
    }
    assert_eq!(host.status().connected_device_ids.len(), 32);
    let mut excess = relay_wire(&host);
    assert_eq!(timeout(WAIT, excess.next_frame()).await.unwrap(), None);
    assert_eq!(host.status().connected_device_ids.len(), 32);
    for client in &active {
        client.close();
    }
    host.close().await;
}

#[tokio::test]
async fn keeps_lan_pending_handshakes_when_relay_handshakes_fill_and_rotate_their_own_budget() {
    let Fixture { host, .. } = fixture().await;
    let lan: Vec<PeerWire> = (1..=8)
        .map(|index| memory_wire(&host, &format!("192.0.2.{index}"), WireKind::Lan))
        .collect();
    let relay: Vec<PeerWire> = (0..8).map(|_| relay_wire(&host)).collect();
    let _next = relay_wire(&host);
    assert!(relay[0].writer().is_closed());
    assert!(lan.iter().all(|wire| !wire.writer().is_closed()));
    host.close().await;
}

#[tokio::test]
async fn keeps_relay_pending_handshakes_when_lan_handshakes_fill_and_rotate_their_own_budget() {
    let Fixture { host, .. } = fixture().await;
    let relay: Vec<PeerWire> = (0..8).map(|_| relay_wire(&host)).collect();
    let lan: Vec<PeerWire> = (1..=8)
        .map(|index| memory_wire(&host, &format!("192.0.2.{index}"), WireKind::Lan))
        .collect();
    let _next = memory_wire(&host, "192.0.2.9", WireKind::Lan);
    assert!(lan[0].writer().is_closed());
    assert!(lan[1..].iter().all(|wire| !wire.writer().is_closed()));
    assert!(relay.iter().all(|wire| !wire.writer().is_closed()));
    host.close().await;
}

#[tokio::test]
async fn never_consumes_or_registers_on_a_wrong_psk_and_rejects_reuse() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture { db, host, code, .. } = fixture().await;
        let wrong = pair(
            &code,
            &generate_keypair().unwrap().private_key[..],
            &[99; 32],
            open(kind, &host, &code).await,
        )
        .await;
        assert_eq!(wrong.err(), Some(AuthError::Failed));
        assert_eq!(list_devices(&db.lock().unwrap()).unwrap(), []);
        assert_eq!(used_at(&db), None);
        let valid = pair(
            &code,
            &generate_keypair().unwrap().private_key[..],
            &secret(&code),
            open(kind, &host, &code).await,
        )
        .await
        .unwrap();
        valid.close();
        assert!(used_at(&db).is_some());
        let reused = pair(
            &code,
            &generate_keypair().unwrap().private_key[..],
            &secret(&code),
            open(kind, &host, &code).await,
        )
        .await;
        assert!(reused.is_err());
        assert_eq!(
            list_devices(&db.lock().unwrap()).unwrap().len(),
            1,
            "{kind:?}"
        );
        host.close().await;
    }
}

#[tokio::test]
async fn rejects_cancelled_invites_and_a_mismatched_pc_key() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture { db, host, code, .. } = fixture().await;
        let mut bad = code.clone();
        bad.host_key = protocol::remote::RemoteKey::new(
            URL_SAFE_NO_PAD.encode(generate_keypair().unwrap().public_key),
        )
        .unwrap();
        assert!(
            pair(
                &bad,
                &generate_keypair().unwrap().private_key[..],
                &secret(&bad),
                open(kind, &host, &code).await
            )
            .await
            .is_err()
        );
        host.cancel_pairing(&code.pairing_id);
        assert!(
            pair(
                &code,
                &generate_keypair().unwrap().private_key[..],
                &secret(&code),
                open(kind, &host, &code).await
            )
            .await
            .is_err()
        );
        assert_eq!(list_devices(&db.lock().unwrap()).unwrap(), [], "{kind:?}");
        host.close().await;
    }
}

#[tokio::test]
async fn keeps_a_pairing_code_single_use_across_relay_then_lan() {
    let Fixture { db, host, code, .. } = fixture().await;
    let first = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        relay_wire(&host),
    )
    .await
    .unwrap();
    first.close();
    let again = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        tcp_wire(code.lan[0].port).await,
    )
    .await;
    assert!(again.is_err());
    assert_eq!(list_devices(&db.lock().unwrap()).unwrap().len(), 1);
    host.close().await;
}

#[tokio::test]
async fn keeps_authenticated_rpc_available_while_refusing_excess_pending_connections() {
    let Fixture { host, code, .. } = fixture().await;
    let port = code.lan[0].port;
    let peer = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        tcp_wire(port).await,
    )
    .await
    .unwrap();
    let device_id = peer.device_id.clone();
    let client = peer.connect().await;
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(tcp_wire(port).await);
    }
    let mut excess = tcp_wire(port).await;
    assert_eq!(
        timeout(Duration::from_secs(2), excess.next_frame())
            .await
            .unwrap(),
        None
    );
    assert_eq!(client.call("bots.list", json!({})).await, Ok(json!([])));
    assert_eq!(host.status().connected_device_ids, [device_id]);
    client.close();
    host.close().await;
}

#[tokio::test]
async fn does_not_register_after_a_persisted_invitation_expires() {
    for kind in [Kind::Lan, Kind::Relay] {
        let Fixture { db, host, code, .. } = fixture().await;
        db.lock()
            .unwrap()
            .execute(
                "UPDATE pairing_codes SET expires_at = ? WHERE id = ?",
                ["2020-01-01T00:00:00.000Z", &code.pairing_id],
            )
            .unwrap();
        let result = pair(
            &code,
            &generate_keypair().unwrap().private_key[..],
            &secret(&code),
            open(kind, &host, &code).await,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(list_devices(&db.lock().unwrap()).unwrap(), [], "{kind:?}");
        host.close().await;
    }
}

#[tokio::test]
async fn hides_internal_error_detail_from_devices() {
    let Fixture { host, code, .. } = fixture().await;
    let client = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        relay_wire(&host),
    )
    .await
    .unwrap()
    .connect()
    .await;
    assert_eq!(
        client.call("settings.get", json!({})).await,
        Err((
            "internal".into(),
            "This request could not be completed on the PC.".into()
        ))
    );
    client.close();
    host.close().await;
}

/// TypeScript hides zod's detail from devices: a schema failure is `bad_params` with the generic text.
#[tokio::test]
async fn hides_schema_failure_detail_from_devices() {
    let Fixture {
        host,
        code,
        mutations,
        ..
    } = fixture().await;
    let client = pair(
        &code,
        &generate_keypair().unwrap().private_key[..],
        &secret(&code),
        relay_wire(&host),
    )
    .await
    .unwrap()
    .connect()
    .await;
    let generic = Err((
        "bad_params".to_owned(),
        "This request could not be completed on the PC.".to_owned(),
    ));
    assert_eq!(client.call("chat.stop", json!({})).await, generic);
    assert_eq!(
        client
            .call("remote.ping", json!({ "extra": 1 }))
            .await
            .map(|_| ())
            .map_err(|e| e.0),
        Ok(())
    );
    assert_eq!(
        client
            .call("device.registerPush", json!({ "token": "" }))
            .await,
        generic
    );
    assert_eq!(
        *mutations.lock().unwrap(),
        0,
        "a request that fails its schema never reaches a handler"
    );
    client.close();
    host.close().await;
}

#[tokio::test]
async fn refuses_local_only_work_after_close() {
    let Fixture { host, .. } = fixture().await;
    host.close().await;
    assert!(matches!(
        host.ensure_listening().await,
        Err(StartError::Closed)
    ));
    assert_eq!(
        host.create_pairing().await.unwrap_err().code,
        ErrorCode::BadState
    );
    assert!(!host.status().listening);
}

#[test]
fn lists_this_pcs_ipv4_addresses_without_loopback() {
    for address in daemon::remote_host::lan_addresses() {
        let ip: std::net::Ipv4Addr = address.parse().unwrap();
        assert!(!ip.is_loopback(), "{address}");
    }
}
