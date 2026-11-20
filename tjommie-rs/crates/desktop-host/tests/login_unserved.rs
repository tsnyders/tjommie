//! Host-wiring review Minor 11: a Logins step that is sent to the UI thread and never served (no engine, a UI thread busy
//! or gone) must not hold the vault's one operation slot for good. The step ends when its operation's check says the
//! operation is over (a lock, a cancel, a change of boundary), whether or not the UI thread ever answers.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use desktop_host::browser::BotIdentity;
use desktop_host::browser::login::{BotLookup, EngineLogin};
use desktop_host::vault::login::{Check, LoginBrowser, LoginError};
use desktop_host::vault::storage::VaultSecret;
use futures::future::BoxFuture;
use protocol::vault::FinishMode;

/// Every bot is listed.
struct Everyone;

impl BotLookup for Everyone {
    fn bot(&self, id: &str) -> BoxFuture<'_, Result<BotIdentity, LoginError>> {
        let bot = serde_json::from_value::<BotIdentity>(
            serde_json::json!({ "id": id, "name": id, "homeFolder": "C:/bots/alpha" }),
        )
        .map_err(|_| LoginError::Failed);
        Box::pin(async move { bot })
    }
}

/// A check that passes until `over` is set, as an operation's does until a lock or a cancel ends it.
fn until(over: &Arc<AtomicBool>) -> Check {
    let over = over.clone();
    Arc::new(move || {
        if over.load(Ordering::SeqCst) {
            Err(LoginError::Changed)
        } else {
            Ok(())
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_that_nothing_serves_ends_when_its_operation_does() {
    // The requests' receiver is kept, so the step is sent, and nothing ever reads or answers it.
    let (login, _requests) = EngineLogin::new(Arc::new(Everyone));
    let over = Arc::new(AtomicBool::new(false));
    let secret = || VaultSecret::new("user", "password");

    let steps: [(&str, BoxFuture<'_, Result<(), LoginError>>); 2] = [
        (
            "fill",
            login.fill("alpha".into(), 1, secret(), until(&over)),
        ),
        (
            "finish",
            login.finish(
                "alpha".into(),
                "generation".into(),
                1,
                FinishMode::Clear,
                until(&over),
            ),
        ),
    ];
    for (name, step) in steps {
        over.store(false, Ordering::SeqCst);
        let ending = {
            let over = over.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                over.store(true, Ordering::SeqCst);
            }
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(step, ending)
        })
        .await
        .unwrap_or_else(|_| panic!("the {name} step held its operation after the operation ended"));
        assert_eq!(result, Err(LoginError::Changed), "{name}");
    }
}
