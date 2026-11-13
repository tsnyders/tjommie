//! `Daemon::close` lets go of everything it built, so the database closes and a data root can be deleted afterwards.
//! The runner, the task service, the server's handlers and the accounts' feed held one another in loops, which kept the
//! SQLite connection (and its `-wal` and `-shm` files) open until the process exited. On Windows an open database file
//! can't be deleted, and TypeScript closes the database explicitly (`db.close()`, `daemon.ts:686`).
//!
//! The daemon runs in-process on a temporary data root, on a free loopback port, with no account check and no folder
//! protection pass, so nothing here reaches a real CLI, account or ACL.

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use common::{TempRoot, connect};
use daemon::paths::data_paths;
use daemon::startup::{Auth, Launch, Mode, start_in};
use serde_json::json;

#[tokio::test]
async fn closing_the_daemon_closes_its_database_so_the_data_root_can_be_deleted() {
    let root = TempRoot::new();
    let launch = Launch {
        mode: Mode::Development,
        port: 0,
        lan_port: 0,
        lan_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        skip_account_check: true,
        skip_folder_protection: true,
        accounts_dir: None,
    };
    let daemon = start_in(&launch, &Auth::Development, root.0.clone())
        .await
        .expect("the daemon starts on a temporary data root");

    // A request first, so the handlers (which hold the runner and the context) have really been used.
    let paths = data_paths(&root.0).unwrap();
    let token = std::fs::read_to_string(&paths.token).unwrap();
    let mut client = connect(daemon.port()).await;
    client.hello(token.trim()).await;
    let listed = client.call(1, "bots.list", json!({})).await;
    assert_eq!(listed["ok"], true, "{listed}");
    client.close().await;

    daemon.close().await;
    // Aborted and finished tasks give back what they hold the next time the runtime runs them.
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        paths.db.exists(),
        "the database file is where the daemon put it"
    );
    std::fs::remove_dir_all(&root.0).expect(
        "the data root can be deleted: SQLite let go of its files when Daemon::close returned",
    );
}
