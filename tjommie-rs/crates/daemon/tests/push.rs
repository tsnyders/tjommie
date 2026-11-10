//! Port of `packages/daemon/src/push.test.ts`, and the end of the last `devices-store.test.ts` case. The relay is a fake
//! sender: nothing here reaches a relay, FCM or a device.

mod remote_support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::devices_store::{PushTarget, add_device, push_targets, set_push_token};
use daemon::push::{Push, PushHost, PushRelay};
use daemon::relay_client::RelayError;
use daemon::server::RpcError;
use futures_util::future::BoxFuture;
use protocol::relay::PushResult;
use protocol::remote::PushKind;
use protocol::wire::Notify;
use remote_support::{key_of, remote_db};
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    tokens: Vec<String>,
    kind: PushKind,
    bot_id: String,
    room_id: Option<String>,
}

#[derive(Default)]
struct FakeHost {
    targets: Mutex<Vec<PushTarget>>,
    cleared: Mutex<Vec<(String, String)>>,
}

impl PushHost for FakeHost {
    fn push_targets(&self) -> Vec<PushTarget> {
        self.targets.lock().unwrap().clone()
    }
    fn clear_push_token(&self, device_id: &str, token: &str) -> Result<(), RpcError> {
        self.cleared
            .lock()
            .unwrap()
            .push((device_id.into(), token.into()));
        Ok(())
    }
}

struct FakeRelay {
    calls: Mutex<Vec<Call>>,
    kinds: Mutex<Vec<PushKind>>,
    /// Answers the first call only, as the TypeScript fake does.
    results: Mutex<Option<Vec<PushResult>>>,
    reject: AtomicBool,
}

impl PushRelay for FakeRelay {
    fn kinds(&self) -> Vec<PushKind> {
        self.kinds.lock().unwrap().clone()
    }
    fn push(
        &self,
        tokens: Vec<String>,
        kind: PushKind,
        bot_id: String,
        room_id: Option<String>,
    ) -> BoxFuture<'static, Result<Vec<PushResult>, RelayError>> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(Call {
            tokens: tokens.clone(),
            kind,
            bot_id,
            room_id,
        });
        let reply = if self.reject.load(Ordering::Relaxed) {
            Err(RelayError::Unavailable)
        } else {
            match self.results.lock().unwrap().clone() {
                Some(results) if calls.len() == 1 => Ok(results),
                _ => Ok(tokens
                    .iter()
                    .map(|_| PushResult {
                        ok: true,
                        unregistered: false,
                    })
                    .collect()),
            }
        };
        Box::pin(async move { reply })
    }
}

fn setup(count: usize) -> (Push, Arc<FakeHost>, Arc<FakeRelay>) {
    let host = Arc::new(FakeHost::default());
    *host.targets.lock().unwrap() = (0..count)
        .map(|index| PushTarget {
            device_id: format!("device-{index}"),
            token: format!("synthetic-token-{index}"),
            kinds: None,
        })
        .collect();
    let relay = Arc::new(FakeRelay {
        calls: Mutex::default(),
        kinds: Mutex::new(PushKind::ALL.to_vec()),
        results: Mutex::default(),
        reject: AtomicBool::new(false),
    });
    (Push::new(host.clone(), relay.clone()), host, relay)
}

fn notice() -> Notify {
    Notify::Approval {
        bot_id: "bot-1".into(),
        title: "private content".into(),
        body: "private detail".into(),
        approval_id: None,
    }
}

fn done(bot_id: &str) -> Notify {
    Notify::Done {
        bot_id: bot_id.into(),
        title: "private content".into(),
        body: "private detail".into(),
        room_id: Some("team".into()),
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn coalesces_by_device_kind_and_bot_then_enforces_30_non_approval_pushes_per_device_per_hour()
{
    let (sender, _, relay) = setup(1);
    sender.send(&done("bot-1"));
    sender.send(&done("bot-1"));
    assert_eq!(relay.calls.lock().unwrap().len(), 1);
    tokio::time::advance(Duration::from_secs(30)).await;
    for index in 0..29 {
        sender.send(&done(&format!("bot-{index}")));
    }
    assert_eq!(relay.calls.lock().unwrap().len(), 30);
    sender.send(&done("overflow"));
    assert_eq!(relay.calls.lock().unwrap().len(), 30);
    sender.send(&notice());
    assert_eq!(relay.calls.lock().unwrap().len(), 31);
    tokio::time::advance(Duration::from_millis(3_600_001 - 30_000)).await;
    sender.send(&done("bot-1"));
    assert_eq!(relay.calls.lock().unwrap().len(), 32);
}

#[tokio::test]
async fn filters_per_device_and_relay_capability_and_forwards_only_room_routing_metadata() {
    let (sender, host, relay) = setup(2);
    host.targets.lock().unwrap()[0].kinds = Some(vec![PushKind::Approval]);
    host.targets.lock().unwrap()[1].kinds = Some(vec![PushKind::Done]);
    *relay.kinds.lock().unwrap() =
        vec![PushKind::Approval, PushKind::Limit, PushKind::RoutineFailed];
    sender.send(&done("bot-1"));
    assert_eq!(*relay.calls.lock().unwrap(), []);
    *relay.kinds.lock().unwrap() = vec![PushKind::Approval, PushKind::Done];
    sender.send(&done("bot-1"));
    assert_eq!(
        *relay.calls.lock().unwrap(),
        [Call {
            tokens: vec!["synthetic-token-1".into()],
            kind: PushKind::Done,
            bot_id: "bot-1".into(),
            room_id: Some("team".into()),
        }]
    );
    assert!(!format!("{:?}", relay.calls.lock().unwrap()).contains("private"));
}

#[tokio::test]
async fn batches_at_16_clears_unregistered_tokens_and_counts_failures() {
    let (sender, host, relay) = setup(17);
    *relay.results.lock().unwrap() = Some(
        (0..16)
            .map(|index| PushResult {
                ok: index != 0,
                unregistered: index == 0,
            })
            .collect(),
    );
    sender.send(&notice());
    settle().await;
    let calls = relay.calls.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.tokens.len())
            .collect::<Vec<_>>(),
        [16, 1]
    );
    assert_eq!(
        (calls[0].kind, calls[0].bot_id.as_str()),
        (PushKind::Approval, "bot-1")
    );
    assert!(!format!("{calls:?}").contains("private detail"));
    assert_eq!(
        *host.cleared.lock().unwrap(),
        [("device-0".to_owned(), "synthetic-token-0".to_owned())]
    );
    assert_eq!(sender.failures(), 1);
    relay.reject.store(true, Ordering::Relaxed);
    sender.send(&Notify::Limit {
        bot_id: "bot-1".into(),
        title: "private content".into(),
        body: "private detail".into(),
    });
    settle().await;
    assert_eq!(sender.failures(), 18);
}

#[tokio::test(start_paused = true)]
async fn restores_rate_budget_when_an_offline_push_is_rejected() {
    let (sender, _, relay) = setup(1);
    relay.reject.store(true, Ordering::Relaxed);
    sender.send(&done("bot-1"));
    settle().await;
    assert_eq!(sender.failures(), 1);
    relay.reject.store(false, Ordering::Relaxed);
    tokio::time::advance(Duration::from_secs(10)).await;
    sender.send(&done("bot-1"));
    assert_eq!(
        relay.calls.lock().unwrap().len(),
        2,
        "a rejected push doesn't hold the 30-second coalescing slot"
    );
    for index in 0..30 {
        relay.reject.store(true, Ordering::Relaxed);
        sender.send(&done(&format!("failed-{index}")));
        settle().await;
        assert_eq!(sender.failures(), index + 2);
    }
    relay.reject.store(false, Ordering::Relaxed);
    sender.send(&done("after-outage"));
    assert_eq!(relay.calls.lock().unwrap().len(), 33);
}

/// The push host over a database, as the remote host is.
struct StoreHost(Mutex<Connection>);

impl PushHost for StoreHost {
    fn push_targets(&self) -> Vec<PushTarget> {
        push_targets(&self.0.lock().unwrap()).unwrap()
    }
    fn clear_push_token(&self, _: &str, _: &str) -> Result<(), RpcError> {
        Ok(())
    }
}

#[tokio::test]
async fn keeps_every_push_target_available_when_stored_kinds_are_malformed_or_newer() {
    let db = remote_db();
    let names = ["Malformed", "Unknown", "Mixed", "Valid"];
    let mut ids = Vec::new();
    for (index, name) in names.into_iter().enumerate() {
        let device = add_device(&db, &key_of(index as u8 + 1), name).unwrap();
        set_push_token(&db, &device.id, Some(&format!("token-{name}")), None).unwrap();
        ids.push(device.id);
    }
    for (kinds, id) in [
        ("not json", &ids[0]),
        (r#"["mention"]"#, &ids[1]),
        (r#"["done","mention"]"#, &ids[2]),
    ] {
        db.execute(
            "UPDATE devices SET push_kinds = ? WHERE id = ?",
            [kinds, id],
        )
        .unwrap();
    }
    let relay = Arc::new(FakeRelay {
        calls: Mutex::default(),
        kinds: Mutex::new(vec![PushKind::Done]),
        results: Mutex::default(),
        reject: AtomicBool::new(false),
    });
    let sender = Push::new(Arc::new(StoreHost(Mutex::new(db))), relay.clone());
    sender.send(&Notify::Done {
        bot_id: "bot".into(),
        title: "Done".into(),
        body: String::new(),
        room_id: None,
    });
    let mut sent: Vec<String> = relay
        .calls
        .lock()
        .unwrap()
        .iter()
        .flat_map(|call| call.tokens.clone())
        .collect();
    sent.sort();
    assert_eq!(sent, ["token-Malformed", "token-Mixed", "token-Valid"]);
}
