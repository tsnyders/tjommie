//! Host-wiring review Minor 5, the other half: the browser host asks the daemon who its bots are before every command
//! (`resolveNativeBot` with `requireBrowser`, `main.ts:445-452`), so a bot deleted or switched off while the watcher was away
//! is refused whatever the engine has been told. The jobs run through the real queue and `serve` on a real engine.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use common::Rig;
use desktop_host::browser::BotIdentity;
use desktop_host::browser::engine::Reply;
use desktop_host::browser::host::{JobQueue, serve};
use desktop_host::browser::host_bridge::{Executor as _, Job};
use desktop_host::hosts::{BotDirectory, BotInfo, DirectoryError};
use futures::future::BoxFuture;
use serde_json::{Value, json};

/// The daemon's list of bots, as a test says it is.
struct Listed {
    /// Where the bots' folders are (the e2e identity refuses any folder outside the test's own).
    root: std::path::PathBuf,
    bots: Mutex<Vec<(String, bool)>>,
    down: AtomicBool,
}

impl Listed {
    fn set(&self, bots: &[(&str, bool)]) {
        *self.bots.lock().unwrap() = bots
            .iter()
            .map(|(id, on)| ((*id).to_owned(), *on))
            .collect();
    }
}

impl BotDirectory for Listed {
    fn find(&self, id: &str) -> BoxFuture<'_, Result<Option<BotInfo>, DirectoryError>> {
        let found = if self.down.load(Ordering::SeqCst) {
            Err(DirectoryError)
        } else {
            Ok(self
                .bots
                .lock()
                .unwrap()
                .iter()
                .find(|(listed, _)| listed == id)
                .map(|(listed, browser)| BotInfo {
                    identity: serde_json::from_value::<BotIdentity>(json!({
                        "id": listed,
                        "name": "As the daemon lists it",
                        "homeFolder": self.root.join("bots").join(listed),
                    }))
                    .unwrap(),
                    browser: *browser,
                }))
        };
        Box::pin(async move { found })
    }
}

fn job(rig: &Rig, id: &str, command: Value) -> Job {
    serde_json::from_value(json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "bot": { "id": id, "name": id, "homeFolder": rig.bot(id).home_folder },
        "command": command,
    }))
    .expect("a job the host protocol accepts")
}

#[test]
fn a_command_runs_only_for_a_bot_the_daemon_lists_with_its_browser_on() {
    let mut rig = Rig::new();
    let listed = Arc::new(Listed {
        root: rig.root.path().to_owned(),
        bots: Mutex::default(),
        down: AtomicBool::new(false),
    });
    let (queue, jobs) = JobQueue::new();
    let spawner = rig.ui.spawner();
    let bots: Arc<dyn BotDirectory> = listed.clone();
    rig.ui
        .spawn(serve(rig.engine.clone(), jobs, bots, move |task| {
            use futures::task::LocalSpawnExt as _;
            let _ = spawner.spawn_local(task);
        }));
    let run = |rig: &mut Rig, id: &str, command: Value| {
        let queued = queue.execute(job(rig, id, command));
        rig.ui.run(queued)
    };

    // Listed with its browser on: the command runs.
    listed.set(&[("alpha", true)]);
    let opened = run(&mut rig, "alpha", json!({ "kind": "open" }));
    assert!(matches!(opened, Ok(Reply::State(_))), "{opened:?}");
    assert!(rig.engine.snapshot("alpha").is_some());

    // Switched off in the daemon while the watcher was away: the engine was never told, and the command is refused.
    listed.set(&[("alpha", false)]);
    let refused = run(&mut rig, "alpha", json!({ "kind": "state" }));
    assert_eq!(
        refused.unwrap_err(),
        "This agent's browser is off or the agent no longer exists."
    );

    // Deleted in the daemon: the same.
    listed.set(&[]);
    let refused = run(&mut rig, "alpha", json!({ "kind": "open" }));
    assert_eq!(
        refused.unwrap_err(),
        "This agent's browser is off or the agent no longer exists."
    );

    // Closing a browser, deleting its profile and cancelling its tools are for a bot the daemon is losing: no list is asked.
    for kind in ["close", "cancel_tools", "delete_profile"] {
        let done = run(&mut rig, "alpha", json!({ "kind": kind }));
        assert!(done.is_ok(), "{kind}: {done:?}");
    }
    assert!(
        rig.engine.snapshot("alpha").is_none(),
        "the browser stayed open after its close"
    );

    // A daemon the host cannot reach is said so, not taken for a bot that is gone.
    listed.set(&[("alpha", true)]);
    listed.down.store(true, Ordering::SeqCst);
    let unreachable = run(&mut rig, "alpha", json!({ "kind": "open" }));
    assert_eq!(
        unreachable.unwrap_err(),
        "The native browser needs the desktop background service."
    );
}
