//! A project review in fixtures mode is refused, before anything is asked of an account, a credential or the host. A real
//! review launches the Claude CLI for a version probe and a stream-json turn, with an API-key account's key, so it is the
//! one place where fixtures mode could still reach a real provider. Here the daemon's own pieces are wired as startup wires
//! them (`Fixtures::review_options_hook` over the real account resolver and the fixture host), with an API-key account
//! configured in this test's own temporary data root and a fake key held in memory. No real credential, no Credential
//! Manager, no CLI and no network is touched.
//!
//! **The reason string is the oracle.** The review's `reason` must be exactly the refusal's text. The host's launch counter
//! is NOT: the reviewer's CLI is the placeholder, which is no path, so without the refusal the review would still stop at
//! that placeholder before it asked the host for anything, and the counter would stay 0. The second test below shows
//! exactly that, so nobody "simplifies" the first down to the counter.
#![cfg(feature = "provider-fixtures")]

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use daemon::account_quota::now_ms;
use daemon::account_runtime::{AccountRuntime, AccountRuntimeOptions, CredentialStore};
use daemon::accounts_store::{AccountInput, create_account};
use daemon::bots::{create_bot, update_bot};
use daemon::cli::{BoxFuture, CliHost};
use daemon::context::Db;
use daemon::project_checks::{CheckError, CheckRuntime, HostCheckRuntime};
use daemon::project_review::{
    ReviewContext, ReviewInstructions, ReviewResult, ReviewRuntime, ReviewSnapshot,
};
use daemon::project_review_account::review_account_resolver;
use daemon::project_review_runtime::{ProjectReviewOptions, create_project_review_runtime};
use daemon::provider_fixtures::{Fixtures, REVIEW_REFUSED};
use daemon::server::{Handlers, RpcError, Server, ServerOptions};
use protocol::accounts::AccountKind;
use protocol::model::{BotColor, BotInput, BotPatch, BotShape, Provider};
use protocol::projects::{EvaluationState, ProjectCheck};
use tokio_util::sync::CancellationToken;

/// Never a real key: it only has to be recognisable if it ever leaks.
const FAKE_KEY: &str = "sk-fixture-not-real";

/// Credentials in memory, counting reads, standing in for Credential Manager.
#[derive(Default)]
struct Keys {
    keys: Mutex<HashMap<String, String>>,
    reads: AtomicUsize,
}

impl CredentialStore for Keys {
    fn reference(&self, account_id: &str) -> Result<String, RpcError> {
        Ok(format!("fixture/{account_id}"))
    }

    fn read(&self, account_id: &str) -> BoxFuture<'_, Result<Option<String>, RpcError>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let key = self.keys.lock().unwrap().get(account_id).cloned();
        Box::pin(async move { Ok(key) })
    }

    fn write(&self, account_id: &str, key: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.keys
            .lock()
            .unwrap()
            .insert(account_id.into(), key.into());
        Box::pin(async { Ok(()) })
    }

    fn delete(&self, account_id: &str) -> BoxFuture<'_, Result<(), RpcError>> {
        self.keys.lock().unwrap().remove(account_id);
        Box::pin(async { Ok(()) })
    }
}

fn snapshot() -> ReviewSnapshot {
    ReviewSnapshot::new(
        "a".repeat(40),
        vec!["source.ts".into()],
        "+return 1;".into(),
        BTreeMap::from([(
            "source.ts".to_string(),
            "export const source = 1;\n".to_string(),
        )]),
    )
}

/// An API-key account, which is what a real review needs, its key in memory, a bot pinned to it, and fixtures.
struct Scene {
    root: tempfile::TempDir,
    fixtures: Fixtures,
    keys: Arc<Keys>,
    db: Db,
    accounts: AccountRuntime,
    server: Server,
    context: ReviewContext,
}

async fn scene() -> Scene {
    let root = tempfile::tempdir().unwrap();
    let (accounts_dir, bots_dir) = (root.path().join("accounts"), root.path().join("bots"));
    std::fs::create_dir_all(&bots_dir).unwrap();
    let fixtures_dir = root.path().join("fixtures");
    std::fs::create_dir_all(&fixtures_dir).unwrap();
    let fixtures = Fixtures::open(&fixtures_dir).unwrap();

    let connection = common::test_db(root.path());
    common::no_cli_setup()
        .ensure_accounts(&connection, &accounts_dir, root.path())
        .unwrap();
    create_account(
        &connection,
        AccountInput {
            id: Some("review-api".into()),
            provider: Provider::Claude,
            label: "Review fixture".into(),
            kind: AccountKind::ApiKey,
            folder: accounts_dir
                .join("review-api")
                .to_string_lossy()
                .into_owned(),
            credential_ref: None,
            is_default: false,
            is_fallback: false,
            spending_cap_usd: Some(1.0),
        },
    )
    .unwrap();
    let keys = Arc::new(Keys::default());
    let db: Db = Arc::new(Mutex::new(connection));
    let accounts = AccountRuntime::new(AccountRuntimeOptions {
        db: db.clone(),
        root: root.path().to_path_buf(),
        accounts: accounts_dir,
        credentials: keys.clone(),
        ops: Arc::new(common::HealthyOps),
        on_changed: None,
        on_sign_in_ended: None,
        on_cap: None,
        now: Arc::new(now_ms),
    });
    accounts.set_api_key("review-api", FAKE_KEY).await.unwrap();
    accounts.check_all().await.unwrap();
    let bot = {
        let db = db.lock().unwrap();
        let bot = create_bot(
            &db,
            &bots_dir,
            &BotInput {
                name: "Reviewer".into(),
                color: BotColor::Blue,
                shape: BotShape::Circle,
                instructions: "Review".into(),
                provider: Provider::Claude,
                model: "claude-sonnet-4-6".into(),
                account_id: None,
                autonomy: None,
                project_folder: None,
                browser: None,
                can_message: None,
            },
        )
        .unwrap();
        update_bot(
            &db,
            &bot.id,
            &BotPatch {
                account_id: Some(Some("review-api".into())),
                ..BotPatch::default()
            },
        )
        .unwrap();
        bot
    };
    let server = Server::start(ServerOptions {
        port: 0,
        token: "t".into(),
        handlers: Handlers::default(),
        allowed_origins: vec![],
        defer_requests: true,
    })
    .await
    .unwrap();
    // The setup above wrote and checked the key. From here on, any read is the review's.
    keys.reads.store(0, Ordering::SeqCst);
    let context = ReviewContext {
        bot_id: bot.id,
        project_id: "project".into(),
        task_id: "task".into(),
        attempt_id: "attempt".into(),
        evaluation_id: "evaluation".into(),
        revision: 1,
        project_revision: 1,
        task_revision: 1,
        instructions: ReviewInstructions {
            reviewer: "Review".into(),
            role: "Review source".into(),
            title: "Task".into(),
            task: "Check the change".into(),
            acceptance: Vec::new(),
        },
    };
    Scene {
        root,
        fixtures,
        keys,
        db,
        accounts,
        server,
        context,
    }
}

impl Scene {
    /// The review options before any hook: the real resolver over these accounts and the fixture host.
    fn options(&self) -> ProjectReviewOptions {
        ProjectReviewOptions {
            root: self.root.path().join("data").join("reviewer"),
            resolve: review_account_resolver(
                self.db.clone(),
                self.accounts.clone(),
                self.server.broadcaster(),
            ),
            host: self.fixtures.host(),
            worker: None,
            cli: None,
            machine_policy_hash: None,
        }
    }

    async fn review(&self, options: ProjectReviewOptions) -> ReviewResult {
        create_project_review_runtime(options)
            .unwrap()
            .review(&snapshot(), &CancellationToken::new(), &self.context)
            .await
            .expect("a review that can't run is a result, not a failed call")
    }
}

#[tokio::test]
async fn a_review_in_fixtures_mode_is_refused_before_any_account_credential_or_launch() {
    let scene = scene().await;

    // The review runtime, as `m5::install` makes it in a fixtures daemon: the options adjusted by the fixtures' hook.
    let options = (scene.fixtures.review_options_hook())(scene.options());
    assert!(options.worker.is_none(), "no worker can launch a reviewer");
    let command = options.cli.as_ref().and_then(|find| find());
    assert!(
        command.is_some_and(|command| !Path::new(&command.file).exists()),
        "the reviewer's CLI is the placeholder, no path"
    );
    let result = scene.review(options).await;

    // THE ORACLE: the exact reason. Take the refusal out and the review reads the key and fails for another reason.
    assert_eq!(result.state, EvaluationState::Unavailable);
    assert_eq!(
        result.reason.as_deref(),
        Some(format!("review_isolation_unavailable: {REVIEW_REFUSED}").as_str())
    );
    assert!(result.findings.is_empty() && result.coverage.is_none());
    // The rest says it was refused first: no credential read and no lease, and nothing left behind. The launch counter
    // is only a backstop (see the module docs): it is 0 with or without the refusal.
    assert_eq!(
        scene.keys.reads.load(Ordering::SeqCst),
        0,
        "the key was never read"
    );
    assert!(
        !scene.accounts.in_use("review-api"),
        "no account was leased"
    );
    assert_eq!(scene.fixtures.host().launch_attempts(), 0);
    assert!(
        !scene.root.path().join("data/reviewer/receipts").exists()
            && !scene.root.path().join("data/reviewer/evaluations").exists()
    );
    assert!(
        !format!("{result:?}").contains(FAKE_KEY),
        "the key is nowhere in the result"
    );
    scene.server.close().await;
}

#[tokio::test]
async fn without_the_refusal_the_review_reads_the_key_and_the_launch_counter_still_reads_zero() {
    // The control for the oracle above. Everything but the refusing resolver: the real resolver, the fixture host, no
    // worker and the placeholder CLI (never a lookup on PATH). The review goes on to the account and its key, then stops
    // at the placeholder, which can't be opened as a file, before it asks the host to launch anything.
    let scene = scene().await;
    let host = scene.fixtures.host();
    let placeholder = host.clone();
    let options = ProjectReviewOptions {
        host: host.clone(),
        worker: None,
        cli: Some(Arc::new(move || placeholder.find_cli(Provider::Claude))),
        ..scene.options()
    };
    let result = scene.review(options).await;

    assert_eq!(result.state, EvaluationState::Unavailable);
    let reason = result.reason.unwrap();
    assert_ne!(
        reason,
        format!("review_isolation_unavailable: {REVIEW_REFUSED}"),
        "this review was not refused"
    );
    assert!(
        scene.keys.reads.load(Ordering::SeqCst) > 0,
        "it read the key"
    );
    assert_eq!(
        host.launch_attempts(),
        0,
        "and still nothing was launched, so the counter can't tell a refused review from one that was not"
    );
    scene.server.close().await;
}

#[tokio::test]
async fn a_configured_project_check_goes_through_the_refusing_host_and_is_counted() {
    // Checks run the project's own configured command through the host, as `m5::install` wires them: in fixtures mode
    // the host is the refusing one, so the command is never started.
    let root = tempfile::tempdir().unwrap();
    let fixtures = Fixtures::open(root.path()).unwrap();
    let check = ProjectCheck {
        id: "check".into(),
        label: "Tests".into(),
        command: "cargo".into(),
        args: vec!["test".into()],
        timeout_ms: 1_000,
    };
    let runtime = HostCheckRuntime::new(fixtures.host());
    let outcome = runtime
        .run(
            &check,
            &root.path().to_string_lossy(),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(outcome, Err(CheckError::Failed(_))),
        "the check could not start"
    );
    assert_eq!(fixtures.host().launch_attempts(), 1);
}

#[test]
fn a_refused_review_prints_nothing_and_never_shows_the_key() {
    // The scenario above, in its own process, so that what it prints can be read. A refused launch would log a line.
    let output = daemon::spawn::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_review_in_fixtures_mode_is_refused_before_any_account_credential_or_launch",
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .unwrap();
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{shown}");
    assert!(
        shown.contains("1 passed"),
        "the scenario really ran: {shown}"
    );
    assert!(!shown.contains(FAKE_KEY), "{shown}");
    assert!(!shown.contains("launch was refused"), "{shown}");
}
