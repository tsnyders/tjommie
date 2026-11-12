//! `refreshProtection` (`daemon.ts:138-146`): serialised passes, the 60 s throttle, the `override` home, and the status
//! kept. A fake pass stands in for the native one, so no ACL is touched.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{TempRoot, insert_account, make_default_account};
use daemon::bots::create_bot;
use daemon::context::Db;
use daemon::protect::{FolderProtection, ListAccounts, Pass};
use daemon::server::RpcError;
use daemon::win::acl::{BotHome, ProtectedAccount, ProtectedPaths, Protection, Provider};
use protocol::ErrorCode;
use protocol::model::BotInput;
use serde_json::json;

type Seen = Arc<Mutex<Vec<(Vec<(String, PathBuf, Provider)>, usize)>>>;

struct Fixture {
    _root: TempRoot,
    db: Db,
    bot_id: String,
    bot_home: PathBuf,
    paths: ProtectedPaths,
}

fn fixture() -> Fixture {
    let root = TempRoot::new();
    let db = daemon::db::open(&root.0.join("t.db")).unwrap();
    insert_account(&db, "claude-main", "claude", &root.0.join("accounts").join("claude-main"));
    make_default_account(&db, "claude-main");
    let input: BotInput = serde_json::from_value(json!({
        "name": "Researcher", "color": "teal", "shape": "droplet", "instructions": "Find sources.",
        "provider": "claude", "model": "sonnet",
    }))
    .unwrap();
    let bot = create_bot(&db, &root.0.join("bots"), &input).unwrap();
    let paths = ProtectedPaths::new(&root.0);
    Fixture {
        db: Arc::new(Mutex::new(db)),
        bot_id: bot.id,
        bot_home: PathBuf::from(bot.home_folder),
        paths,
        _root: root,
    }
}

fn accounts() -> ListAccounts {
    Arc::new(|_| {
        Ok(vec![ProtectedAccount {
            id: "claude-main".into(),
            provider: Provider::Claude,
            folder: PathBuf::from("accounts"),
        }])
    })
}

/// A pass that records the homes it saw and how many accounts, and answers `status`.
fn recording(seen: &Seen, status: Protection) -> Pass {
    let seen = seen.clone();
    Arc::new(move |_, homes, accounts| {
        let homes = homes
            .iter()
            .map(|home| (home.id.clone(), home.home_folder.clone(), home.provider))
            .collect();
        seen.lock().unwrap().push((homes, accounts.map_or(0, <[_]>::len)));
        status
    })
}

fn service(f: &Fixture, pass: Pass, initial: Protection) -> Arc<FolderProtection> {
    FolderProtection::with_pass(f.paths.clone(), f.db.clone(), accounts(), initial, pass)
}

#[tokio::test(start_paused = true)]
async fn an_unforced_pass_waits_out_the_throttle_and_a_forced_one_does_not() {
    let f = fixture();
    let seen: Seen = Arc::default();
    let protection = service(&f, recording(&seen, Protection::Ready), Protection::NotConfigured);
    assert_eq!(protection.status(), Protection::NotConfigured, "the pre-SQLite pass result is kept");
    protection.refresh(false, None).await.unwrap();
    assert_eq!((seen.lock().unwrap().len(), protection.status()), (1, Protection::Ready));
    // Within 60 s nothing runs unless it is forced.
    tokio::time::advance(Duration::from_secs(59)).await;
    protection.refresh(false, None).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    protection.refresh(true, None).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2);
    // A forced pass restarts the clock; after 60 s an unforced one runs again.
    tokio::time::advance(Duration::from_secs(59)).await;
    protection.refresh(false, None).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2);
    tokio::time::advance(Duration::from_secs(2)).await;
    protection.refresh(false, None).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn the_pass_gets_every_bot_home_and_the_account_folders() {
    let f = fixture();
    let seen: Seen = Arc::default();
    let protection = service(&f, recording(&seen, Protection::Ready), Protection::Ready);
    protection.refresh(true, None).await.unwrap();
    let passes = seen.lock().unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0].0, vec![(f.bot_id.clone(), f.bot_home.clone(), Provider::Claude)]);
    assert_eq!(passes[0].1, 1);
}

#[tokio::test]
async fn an_override_replaces_that_bots_home_and_provider_for_one_pass_only() {
    let f = fixture();
    let seen: Seen = Arc::default();
    let protection = service(&f, recording(&seen, Protection::Ready), Protection::Ready);
    let moved = BotHome {
        id: f.bot_id.clone(),
        home_folder: PathBuf::from("moved-home"),
        provider: Provider::Codex,
    };
    let stranger = BotHome {
        id: "not-a-bot".into(),
        home_folder: PathBuf::from("elsewhere"),
        provider: Provider::Codex,
    };
    protection.refresh(true, Some(moved)).await.unwrap();
    protection.refresh(true, Some(stranger)).await.unwrap();
    protection.refresh(true, None).await.unwrap();
    let passes = seen.lock().unwrap();
    assert_eq!(passes[0].0, vec![(f.bot_id.clone(), PathBuf::from("moved-home"), Provider::Codex)]);
    // An override only replaces a bot that exists; it never adds one, and it isn't remembered.
    assert_eq!(passes[1].0, vec![(f.bot_id.clone(), f.bot_home.clone(), Provider::Claude)]);
    assert_eq!(passes[2].0, passes[1].0);
}

#[tokio::test]
async fn passes_never_overlap_and_run_in_the_order_they_were_asked_for() {
    let f = fixture();
    let (running, overlapped) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let order: Arc<Mutex<Vec<String>>> = Arc::default();
    let pass: Pass = {
        let (running, overlapped, order) = (running.clone(), overlapped.clone(), order.clone());
        Arc::new(move |_, homes, _| {
            if running.fetch_add(1, Ordering::SeqCst) > 0 {
                overlapped.fetch_add(1, Ordering::SeqCst);
            }
            order.lock().unwrap().push(homes[0].home_folder.to_string_lossy().into_owned());
            std::thread::sleep(Duration::from_millis(40));
            running.fetch_sub(1, Ordering::SeqCst);
            Protection::Ready
        })
    };
    let protection = service(&f, pass, Protection::Ready);
    let home = |name: &str| BotHome {
        id: f.bot_id.clone(),
        home_folder: PathBuf::from(name),
        provider: Provider::Claude,
    };
    let (first, second, third) = tokio::join!(
        protection.refresh(true, Some(home("first"))),
        protection.refresh(true, Some(home("second"))),
        protection.refresh(true, Some(home("third"))),
    );
    first.and(second).and(third).unwrap();
    assert_eq!(overlapped.load(Ordering::SeqCst), 0, "two passes ran at once");
    assert_eq!(*order.lock().unwrap(), ["first", "second", "third"]);
}

#[tokio::test]
async fn a_failed_pass_reports_its_error_and_the_queue_keeps_working() {
    let f = fixture();
    let seen: Seen = Arc::default();
    let failing = Arc::new(AtomicUsize::new(1));
    let list: ListAccounts = {
        let failing = failing.clone();
        Arc::new(move |_| {
            if failing.swap(0, Ordering::SeqCst) == 1 {
                Err(RpcError::new(ErrorCode::Internal, "the accounts store is unavailable"))
            } else {
                Ok(Vec::new())
            }
        })
    };
    let protection = FolderProtection::with_pass(
        f.paths.clone(),
        f.db.clone(),
        list,
        Protection::Ready,
        recording(&seen, Protection::CheckFailed),
    );
    let error = protection.refresh(true, None).await.unwrap_err();
    assert_eq!(error.message, "the accounts store is unavailable");
    assert_eq!(seen.lock().unwrap().len(), 0, "no pass ran without its accounts");
    assert_eq!(protection.status(), Protection::Ready, "the old status stays");
    protection.refresh(true, None).await.unwrap();
    assert_eq!(protection.status(), Protection::CheckFailed);
    assert_eq!(
        protection.reason(),
        Some("Tjommie couldn't check Codex's sandbox. If Codex was updated, run its sandbox setup again.")
    );
}
