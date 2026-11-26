//! The client against the real daemon, in this process, on 127.0.0.1:0 with a temporary data root. Nothing here reaches a
//! real CLI, account or ACL (the daemon's own test hooks switch those checks off). The client checks every result and
//! event it receives against the protocol manifest, so a daemon answer that breaks the contract fails these calls.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use client::{Client, ConnectError, Endpoint, Rpc, RpcError};
use daemon::startup::{Auth, Daemon, Launch, Mode, start_in};
use protocol::model::SettingsPatch;
use protocol::wire::{Empty, method};
use protocol::{ErrorCode, Event};

/// 43 base64url characters, the least the daemon accepts as a token.
const TOKEN: &str = "T1-1_client_test_token_0123456789_abcdefghij";

async fn start() -> (Daemon, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let launch = Launch {
        mode: Mode::Production,
        port: 0,
        lan_port: 0,
        lan_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        skip_account_check: true,
        skip_folder_protection: true,
        accounts_dir: None,
    };
    let auth = Auth::Production {
        token: TOKEN.to_owned(),
        desktop_host_token: None,
    };
    let daemon = start_in(&launch, &auth, root.path().to_owned())
        .await
        .expect("the daemon starts on a temporary data root");
    (daemon, root)
}

async fn connect(daemon: &Daemon) -> Client {
    Client::connect(
        Endpoint::Local {
            port: daemon.port(),
        },
        TOKEN,
    )
    .await
    .expect("the client connects with the right token")
}

/// Code written against [`Rpc`] runs on a real connection.
async fn bot_count(rpc: &impl Rpc) -> usize {
    rpc.call::<method::BotsList>(Empty {}).await.unwrap().len()
}

#[tokio::test]
async fn calls_bots_list() {
    let (daemon, _root) = start().await;
    let client = connect(&daemon).await;
    let bots = client.call::<method::BotsList>(Empty {}).await.unwrap();
    assert!(bots.is_empty(), "a fresh data root has no bots");
    assert_eq!(bot_count(&client).await, 0);
    client.close().await;
    daemon.close().await;
}

#[tokio::test]
async fn a_refused_call_carries_the_daemons_error_code() {
    let (daemon, _root) = start().await;
    let client = connect(&daemon).await;
    let patch = SettingsPatch {
        chain_limit: Some(0),
        ..SettingsPatch::default()
    };
    // The chain limit is at least 1, so the daemon refuses 0 and says why.
    let refused = client.call::<method::SettingsSet>(patch).await.unwrap_err();
    assert!(
        matches!(
            refused,
            RpcError::Daemon {
                code: ErrorCode::BadParams,
                ..
            }
        ),
        "{refused}"
    );
    // The connection is still good.
    client.call::<method::SettingsGet>(Empty {}).await.unwrap();
    client.close().await;
    daemon.close().await;
}

#[tokio::test]
async fn a_bad_token_is_unauthorized() {
    let (daemon, _root) = start().await;
    let refused = Client::connect(
        Endpoint::Local {
            port: daemon.port(),
        },
        "not-the-token",
    )
    .await;
    assert!(
        matches!(refused, Err(ConnectError::Unauthorized)),
        "{:?}",
        refused.err()
    );
    daemon.close().await;
}

#[tokio::test]
async fn delivers_events_to_every_receiver() {
    let (daemon, _root) = start().await;
    let client = connect(&daemon).await;
    let mut first = client.events();
    let mut second = client.events();
    let patch = SettingsPatch {
        chain_limit: Some(7),
        ..SettingsPatch::default()
    };
    let settings = client.call::<method::SettingsSet>(patch).await.unwrap();
    assert_eq!(settings.chain_limit, 7);
    for receiver in [&mut first, &mut second] {
        let event = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .expect("the event arrives")
            .unwrap();
        match event {
            Event::SettingsChanged(changed) => assert_eq!(changed.chain_limit, 7),
            other => panic!("expected settings.changed, got {other:?}"),
        }
    }
    client.close().await;
    daemon.close().await;
}

#[tokio::test]
async fn closed_resolves_when_the_daemon_goes_away_and_later_calls_fail() {
    let (daemon, _root) = start().await;
    let client = connect(&daemon).await;
    let closed = client.closed();
    daemon.close().await;
    let reason = tokio::time::timeout(Duration::from_secs(10), closed)
        .await
        .expect("closed resolves");
    assert!(reason.opened, "the connection had opened: {reason:?}");
    let failed = client.call::<method::BotsList>(Empty {}).await.unwrap_err();
    assert!(matches!(failed, RpcError::Closed(_)), "{failed}");
}
