//! Port of `packages/daemon/src/project-review-runtime.test.ts`. The TypeScript synthetic Claude script becomes an
//! in-process stub (`StubHost`) that answers the same protocol: `--version`, the argument checks, the acceptance
//! reply, the review reply, and the `mutate`, `incomplete`, `tools` and `wait` modes. The account and its resolver are
//! fakes. The Windows worker cases run the real `WorkerBoundary` over a fake native layer (`#[cfg(windows)]`).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::cli::{BoxFuture, ChildExit, CliChild, CliCommand, CliExit, CliHost, KillTree, LaunchOptions};
use daemon::project_checks::CheckError;
use daemon::project_review::{
    ReviewContext, ReviewInstructions, ReviewRuntime, ReviewSnapshot, review_coverage,
    review_instructions_hash,
};
use daemon::project_review_runtime::{
    CliSeam, MachinePolicySeam, ProjectReviewOptions, ProjectReviewRuntime, ReviewAccount,
    ReviewAccountResolver, ReviewAccountSelection, create_project_review_runtime,
};
use daemon::providers::{ApiKey, ProviderEvent, Usage};
use protocol::accounts::AccountKind;
use protocol::model::Provider;
use protocol::projects::EvaluationState;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// The TypeScript `context` (project-review-runtime.test.ts:16).
fn instructions() -> ReviewInstructions {
    ReviewInstructions {
        reviewer: "Review correctness".into(),
        role: "Review the assigned task".into(),
        title: "Synthetic task".into(),
        task: "Change the source".into(),
        acceptance: vec!["Export one constant".into()],
    }
}

fn context() -> ReviewContext {
    ReviewContext {
        bot_id: "reviewer".into(),
        project_id: "project".into(),
        task_id: "task".into(),
        attempt_id: "attempt".into(),
        evaluation_id: "evaluation".into(),
        revision: 1,
        project_revision: 1,
        task_revision: 1,
        instructions: instructions(),
    }
}

/// The TypeScript `snapshot` (project-review-runtime.test.ts:18).
fn snapshot() -> ReviewSnapshot {
    // TODO(M5-integration): the contract hides `ReviewSnapshot`'s contents. Assumed constructor, same data as the TS.
    ReviewSnapshot::new(
        "a".repeat(40),
        vec!["source.ts".into()],
        "+return 1;".into(),
        BTreeMap::from([("source.ts".to_string(), "export const source = 1;\n".to_string())]),
    )
}

/// The turn log the stub keeps (`turns.jsonl`), and the mode the stub answers in.
struct StubState {
    mode: &'static str,
    log: Mutex<Vec<Value>>,
}

/// Starts a stub child whose stdio and exit are the runner's, as a Node child's would be.
fn stub_child(state: Arc<StubState>, args: Vec<String>, cwd: PathBuf) -> CliChild {
    let (stdin_runner, stdin_stub) = duplex(1 << 20);
    let (stdout_stub, stdout_runner) = duplex(1 << 20);
    let (stderr_stub, stderr_runner) = duplex(1 << 20);
    let exit = ChildExit::new();
    let kill = Arc::new(Notify::new());
    let kill_tree: KillTree = {
        let (exit, kill) = (exit.clone(), kill.clone());
        Arc::new(move || {
            let (exit, kill) = (exit.clone(), kill.clone());
            Box::pin(async move {
                kill.notify_one();
                exit.fire(CliExit::Exited {
                    code: None,
                    signal: Some("SIGTERM".into()),
                });
                Ok(())
            })
        })
    };
    let task_exit = exit.clone();
    tokio::spawn(async move {
        // The stub never writes stderr; keeping its end open makes the runner see an idle stream, not EOF.
        let _stderr = stderr_stub;
        run_stub(state, args, cwd, stdin_stub, stdout_stub, task_exit, kill).await;
    });
    CliChild {
        pid: Some(4242),
        stdin: Box::new(stdin_runner),
        stdout: Box::new(stdout_runner),
        stderr: Box::new(stderr_runner),
        exit,
        kill_tree,
    }
}

async fn say(out: &mut DuplexStream, line: &Value) {
    out.write_all(format!("{line}\n").as_bytes()).await.expect("the runner reads stdout");
}

/// The TypeScript synthetic script's body (project-review-runtime.test.ts:21-33).
async fn run_stub(
    state: Arc<StubState>,
    args: Vec<String>,
    cwd: PathBuf,
    mut stdin: DuplexStream,
    mut stdout: DuplexStream,
    exit: ChildExit,
    kill: Arc<Notify>,
) {
    let exited = |code: i32| CliExit::Exited { code: Some(code), signal: None };
    if args.iter().any(|arg| arg == "--version") {
        stdout.write_all(b"2.1.314 (Claude Code)\n").await.ok();
        exit.fire(exited(0));
        return;
    }
    let tools_empty = args
        .iter()
        .position(|arg| arg == "--tools")
        .and_then(|index| args.get(index + 1))
        .is_some_and(String::is_empty);
    let bare = args.iter().any(|arg| arg == "--bare") && args.iter().any(|arg| arg == "--safe-mode");
    if !bare || !tools_empty {
        exit.fire(exited(2));
        return;
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).await.expect("the runner writes the prompt");
    let input: Value = serde_json::from_str(&text).expect("the prompt is JSON");
    let budget = args
        .iter()
        .position(|arg| arg == "--max-budget-usd")
        .and_then(|index| args.get(index + 1))
        .cloned();
    state.log.lock().unwrap().push(json!({
        "acceptance": input.get("requests").is_some(),
        "budget": budget,
        "cwd": cwd.to_string_lossy(),
        "criteria": input.get("criteria"),
        "instructionsSha256": input.get("instructionsSha256"),
    }));
    let tools = if state.mode == "tools" { json!(["Bash"]) } else { json!([]) };
    say(&mut stdout, &json!({ "type": "system", "subtype": "init", "tools": tools, "mcp_servers": [], "plugins": [] })).await;
    if matches!(state.mode, "tools" | "wait") {
        // The runner must stop this child: its kill fires the exit.
        kill.notified().await;
        return;
    }
    if state.mode == "mutate" {
        let marker = cwd.parent().unwrap_or(&cwd).join("forbidden-mutation");
        fs::write(marker, "mutated").expect("the fixture folder exists");
    }
    let result = if input.get("requests").is_some() {
        let mut attempts = input["output"]["attempts"].as_array().cloned().unwrap_or_default();
        if state.mode == "incomplete" {
            let first = attempts.first().cloned().unwrap_or(Value::Null);
            attempts.iter_mut().for_each(|attempt| *attempt = first.clone());
        }
        json!({ "nonce": input["nonce"], "attempts": attempts })
    } else {
        let reviewed: Vec<Value> = input["snapshot"]["files"]
            .as_array()
            .map(|files| files.iter().map(|file| file["path"].clone()).collect())
            .unwrap_or_default();
        json!({
            "sourceSha": input["coverage"]["sourceSha"],
            "snapshotSha256": input["coverage"]["snapshotSha256"],
            "instructionsSha256": input["instructionsSha256"],
            "complete": true,
            "reviewedFiles": reviewed,
            "state": "passed",
            "reason": null,
            "findings": [],
        })
    };
    say(
        &mut stdout,
        &json!({
            "type": "result", "subtype": "success", "is_error": false, "result": result.to_string(),
            "uuid": "result:1", "total_cost_usd": 0.01, "usage": { "input_tokens": 10, "output_tokens": 5 },
            "modelUsage": { "test": {} },
        }),
    )
    .await;
    exit.fire(exited(0));
}

/// Launches stub children for the runtime's CLI host.
struct StubHost(Arc<StubState>);

impl CliHost for StubHost {
    fn launch<'a>(
        &'a self,
        _cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        let state = self.0.clone();
        Box::pin(async move { Ok(stub_child(state, args, PathBuf::from(options.cwd))) })
    }
}

/// The synthetic account (`ReviewAccount`). Budgets are recorded as the TypeScript test records them.
struct FakeAccount {
    provider: Provider,
    kind: AccountKind,
    key: Option<ApiKey>,
    /// The TypeScript override `getMaxBudgetUsd = () => events.length ? 0 : 0.01`.
    stop_when_charged: bool,
    spent: Mutex<f64>,
    budgets: Mutex<Vec<f64>>,
    events: Mutex<Vec<ProviderEvent>>,
    released: AtomicUsize,
    /// Runs after each usage event (the TypeScript tests wrap `onEvent` to change the policy here).
    on_usage: Option<Box<dyn Fn() + Send + Sync>>,
}

impl FakeAccount {
    fn api() -> Self {
        FakeAccount {
            provider: Provider::Claude,
            kind: AccountKind::ApiKey,
            key: Some(ApiKey("synthetic-api-key-not-real".into())),
            stop_when_charged: false,
            spent: Mutex::new(0.0),
            budgets: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            released: AtomicUsize::new(0),
            on_usage: None,
        }
    }

    fn usages(&self) -> Vec<Usage> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::Usage(usage) => Some(usage.clone()),
                _ => None,
            })
            .collect()
    }

    fn released(&self) -> usize {
        self.released.load(Ordering::SeqCst)
    }
}

impl ReviewAccount for FakeAccount {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn model(&self) -> &str {
        "test"
    }

    fn kind(&self) -> AccountKind {
        self.kind
    }

    fn api_key(&self) -> Option<&ApiKey> {
        self.key.as_ref()
    }

    fn max_budget_usd(&self) -> Result<Option<f64>, String> {
        let remaining = if self.stop_when_charged {
            if self.events.lock().unwrap().is_empty() { 0.01 } else { 0.0 }
        } else {
            0.1 - *self.spent.lock().unwrap()
        };
        self.budgets.lock().unwrap().push(remaining);
        Ok(Some(remaining))
    }

    fn assert_current(&self) -> Result<(), String> {
        Ok(())
    }

    fn on_event(&self, event: ProviderEvent) -> Result<(), String> {
        if let ProviderEvent::Usage(usage) = &event {
            *self.spent.lock().unwrap() += usage.cost_usd.unwrap_or(0.0);
        }
        let is_usage = matches!(event, ProviderEvent::Usage(_));
        self.events.lock().unwrap().push(event);
        if is_usage && let Some(hook) = &self.on_usage {
            hook();
        }
        Ok(())
    }

    fn release(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

/// Hands out one account, as the TypeScript `resolve: async () => account`.
struct FakeResolver(Arc<FakeAccount>);

impl ReviewAccountResolver for FakeResolver {
    fn resolve<'a>(
        &'a self,
        _context: &'a ReviewContext,
        _signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<ReviewAccountSelection, String>> {
        let account = self.0.clone();
        Box::pin(async move { Ok(ReviewAccountSelection::Ready(account)) })
    }
}

/// A test folder with a synthetic CLI executable, and the stub that answers for it.
struct Harness {
    temp: TempDir,
    stub: Arc<StubState>,
    account: Arc<FakeAccount>,
    cli: PathBuf,
}

impl Harness {
    fn new(mode: &'static str) -> Self {
        Self::with_account(mode, FakeAccount::api())
    }

    fn with_account(mode: &'static str, account: FakeAccount) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("tjommie test ñ-")
            .tempdir()
            .unwrap();
        let cli = temp.path().join(format!("synthetic-claude-{mode}.exe"));
        fs::write(&cli, b"").unwrap();
        Harness {
            temp,
            stub: Arc::new(StubState { mode, log: Mutex::new(Vec::new()) }),
            account: Arc::new(account),
            cli,
        }
    }

    fn reviews(&self) -> PathBuf {
        self.temp.path().join("reviews")
    }

    fn log(&self) -> Vec<Value> {
        self.stub.log.lock().unwrap().clone()
    }

    fn cli_seam(&self) -> CliSeam {
        let command = CliCommand { file: self.cli.to_string_lossy().into_owned(), args: vec![] };
        Arc::new(move || Some(command.clone()))
    }

    fn policy(hash: String) -> MachinePolicySeam {
        Arc::new(move || hash.clone())
    }

    /// The TypeScript `createProjectReviewRuntime(options)` for this harness, with the synthetic seams.
    fn options(&self) -> ProjectReviewOptions {
        ProjectReviewOptions {
            root: self.reviews(),
            resolve: Arc::new(FakeResolver(self.account.clone())),
            host: Arc::new(StubHost(self.stub.clone())),
            worker: None,
            cli: Some(self.cli_seam()),
            machine_policy_hash: Some(Self::policy("0".repeat(64))),
        }
    }

    fn runtime(&self) -> ProjectReviewRuntime {
        create_project_review_runtime(self.options()).unwrap()
    }

    fn receipts(&self) -> Vec<PathBuf> {
        read_names(&self.reviews().join("receipts"))
    }

    fn evaluations(&self) -> Vec<PathBuf> {
        read_names(&self.reviews().join("evaluations"))
    }

    fn review_folders(&self) -> usize {
        read_names(&self.reviews())
            .iter()
            .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".review-")))
            .count()
    }
}

/// The entries of a folder, or none when it doesn't exist.
fn read_names(folder: &Path) -> Vec<PathBuf> {
    match fs::read_dir(folder) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn token() -> CancellationToken {
    CancellationToken::new()
}

#[tokio::test]
async fn copies_the_server_criteria_into_the_frozen_prompt_before_resolving_an_account() {
    let h = Harness::new("good");
    let acceptance = vec!["Require the exported constant".to_string()];
    let frozen = ReviewInstructions {
        task: "Original trusted task".into(),
        acceptance: acceptance.clone(),
        ..instructions()
    };
    let supplied = ReviewContext { instructions: frozen.clone(), ..context() };
    let digest = review_instructions_hash(&supplied);
    // Rust takes the context by reference, so a late edit by the resolver cannot reach the frozen copy. The resolver
    // checks the criteria it is handed are the supplied ones, before it returns the account.
    struct Checking(Arc<FakeAccount>, ReviewInstructions, Mutex<bool>);
    impl ReviewAccountResolver for Checking {
        fn resolve<'a>(
            &'a self,
            context: &'a ReviewContext,
            _signal: &'a CancellationToken,
        ) -> BoxFuture<'a, Result<ReviewAccountSelection, String>> {
            *self.2.lock().unwrap() = context.instructions.task == self.1.task;
            let account = self.0.clone();
            Box::pin(async move { Ok(ReviewAccountSelection::Ready(account)) })
        }
    }
    let resolver = Arc::new(Checking(h.account.clone(), frozen.clone(), Mutex::new(false)));
    let runtime = create_project_review_runtime(ProjectReviewOptions {
        resolve: resolver.clone(),
        ..h.options()
    })
    .unwrap();
    let result = runtime.review(&snapshot(), &token(), &supplied).await.unwrap();
    assert_eq!(result.state, EvaluationState::Passed);
    assert!(*resolver.2.lock().unwrap());
    let turns = h.log();
    assert_eq!(turns[1]["criteria"], serde_json::to_value(&frozen).unwrap());
    assert_eq!(turns[1]["criteria"]["acceptance"], json!(acceptance));
    assert_eq!(turns[1]["instructionsSha256"], json!(digest));
}

#[tokio::test]
async fn derives_its_own_acceptance_charges_both_turns_refreshes_the_budget_and_retains_exact_coverage() {
    let h = Harness::new("good");
    let runtime = h.runtime();
    let result = runtime.review(&snapshot(), &token(), &context()).await.unwrap();
    assert_eq!(result.state, EvaluationState::Passed);
    assert_eq!(result.coverage, Some(review_coverage(&snapshot()).unwrap()));
    assert_eq!(h.account.usages().len(), 2);
    let budgets = h.account.budgets.lock().unwrap().clone();
    assert_eq!(budgets.len(), 2);
    assert_eq!(budgets[0], 0.1);
    assert!((budgets[1] - 0.09).abs() < 1e-9);
    assert_eq!(h.account.released(), 1);
    let ids: std::collections::BTreeSet<_> = h.account.usages().into_iter().filter_map(|usage| usage.event_id).collect();
    assert_eq!(ids.len(), 2);
    let receipts = h.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = read_json(&receipts[0]);
    assert_eq!(receipt["identity"]["version"], json!("2.1.314"));
    assert_eq!(receipt["attempts"].as_array().unwrap().len(), 6);
    assert_eq!(h.review_folders(), 0);
    assert!(!serde_json::to_string(&h.log()).unwrap().contains("synthetic-api-key-not-real"));
    runtime.review(&snapshot(), &token(), &ReviewContext { evaluation_id: "second".into(), ..context() }).await.unwrap();
    assert_eq!(h.account.usages().len(), 3);
    // A persisted or forged receipt cannot enable a new daemon process: acceptance runs again.
    create_project_review_runtime(h.options())
        .unwrap()
        .review(&snapshot(), &token(), &ReviewContext { evaluation_id: "restart".into(), ..context() })
        .await
        .unwrap();
    assert_eq!(h.account.usages().len(), 5);
}

#[tokio::test]
async fn never_mints_a_passing_acceptance_receipt_when_the_fixture_mutates_or_is_incomplete() {
    for mode in ["mutate", "incomplete"] {
        let h = Harness::new(mode);
        let result = h.runtime().review(&snapshot(), &token(), &context()).await.unwrap();
        assert_eq!(result.state, EvaluationState::Unavailable);
        assert!(result.coverage.is_none());
        assert!(h.receipts().is_empty(), "{mode}: a receipt was written");
        assert_eq!(h.account.usages().len(), 1, "{mode}");
        assert_eq!(h.account.released(), 1, "{mode}");
    }
}

#[tokio::test]
async fn stops_exposed_tools_and_a_cancelled_owned_process_before_releasing_its_lease_and_profile() {
    let exposed = Harness::new("tools");
    let result = exposed.runtime().review(&snapshot(), &token(), &context()).await.unwrap();
    assert_eq!(result.state, EvaluationState::Unavailable);
    assert_eq!(exposed.account.released(), 1);
    assert!(exposed.receipts().is_empty());

    let waiting = Arc::new(Harness::new("wait"));
    let runtime = Arc::new(waiting.runtime());
    let cancel = token();
    let (snap, ctx, signal) = (snapshot(), context(), cancel.clone());
    let operation = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.review(&snap, &signal, &ctx).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while waiting.log().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the stub CLI started");
    cancel.cancel();
    let result = operation.await.unwrap().unwrap();
    assert_eq!(result.state, EvaluationState::Unavailable);
    assert_eq!(waiting.account.released(), 1);
    let usages = waiting.account.usages();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].model, "test");
    assert!(usages[0].cost_usd.is_none());
    assert_eq!((usages[0].tokens.input, usages[0].tokens.output), (0.0, 0.0));
    assert_eq!(waiting.review_folders(), 0);
}

#[tokio::test]
async fn denies_unsupported_subscription_and_codex_accounts_before_any_cli_turn_and_stops_when_acceptance_consumes_the_allowance() {
    let codex = FakeAccount { provider: Provider::Codex, ..FakeAccount::api() };
    let subscription = FakeAccount { kind: AccountKind::Subscription, key: None, ..FakeAccount::api() };
    for account in [codex, subscription] {
        let h = Harness::with_account("good", account);
        let result = h.runtime().review(&snapshot(), &token(), &context()).await.unwrap();
        assert_eq!(result.state, EvaluationState::Unavailable);
        assert!(h.log().is_empty());
        assert_eq!(h.account.released(), 1);
    }
    let h = Harness::with_account("good", FakeAccount { stop_when_charged: true, ..FakeAccount::api() });
    let result = h.runtime().review(&snapshot(), &token(), &context()).await.unwrap();
    assert_eq!(result.state, EvaluationState::Unavailable);
    assert_eq!(h.account.usages().len(), 1);
    assert_eq!(h.account.released(), 1);
}

#[tokio::test]
async fn invalidates_an_acceptance_if_machine_policy_changes_during_the_paid_turn() {
    let policy = Arc::new(Mutex::new("a".repeat(64)));
    let changing = policy.clone();
    let account = FakeAccount {
        on_usage: Some(Box::new(move || *changing.lock().unwrap() = "b".repeat(64))),
        ..FakeAccount::api()
    };
    let h = Harness::with_account("good", account);
    let seam_policy = policy.clone();
    let runtime = create_project_review_runtime(ProjectReviewOptions {
        machine_policy_hash: Some(Arc::new(move || seam_policy.lock().unwrap().clone())),
        ..h.options()
    })
    .unwrap();
    let result = runtime.review(&snapshot(), &token(), &context()).await.unwrap();
    assert!(result.reason.unwrap_or_default().contains("review_policy_changed"));
    assert!(h.receipts().is_empty());
}

#[tokio::test]
#[ignore = "M5-integration: needs the project task services (projectFixture, TaskIntegrations), not yet ported"]
async fn runs_the_synthetic_developer_enforced_reviewer_real_host_check_and_exact_integration_through_the_task_services() {
    // TypeScript (project-review-runtime.test.ts:175-191): a synthetic developer commits a change; this enforced
    // reviewer (mode "good") and the test and review evaluations pass; TaskIntegrations integrates the accepted
    // attempt into the delivery branch; the owner's working bytes are unchanged. Port with the task services.
    todo!("M5-integration: port with project_support's fixture once Core's runner is merged");
}

#[cfg(windows)]
mod worker {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use daemon::cli::{BoxFuture, CliChild, CliCommand, CliExit};
    use daemon::win::boundary::{
        BoundaryError, BoundaryOptions, LaunchFailure, NativeLaunch, Observer, PolicyReceipt,
        Receipt, Running, WorkerBoundary, WorkerNative,
    };
    use daemon::win::worker::{Purpose, WorkerConfiguration};
    use super::*;

    /// The worker's policy observations and failure switches (the TypeScript `syntheticWorker` state).
    pub struct WorkerState {
        pub observations: AtomicUsize,
        pub receipt: Mutex<PolicyReceipt>,
        pub paid_launches: AtomicUsize,
        pub fail_paid_start: AtomicBool,
        pub fail_after: AtomicBool,
    }

    pub struct Worker {
        pub boundary: Arc<WorkerBoundary>,
        pub state: Arc<WorkerState>,
        pub config: WorkerConfiguration,
        /// The CLI executable inside the worker's runtime root: the boundary launches nothing outside it.
        pub cli: PathBuf,
    }

    fn policy() -> PolicyReceipt {
        PolicyReceipt {
            worker_sid: "S-1-5-21-1-2-3-1001".into(),
            provider_sid: "S-1-5-21-1-2-3-1002".into(),
            configuration_sha256: "c".repeat(64),
            observer_sha256: "a".repeat(64),
            policy_sha256: "b".repeat(64),
        }
    }

    /// Launches stub children as the worker's native process, reading the arguments from the launch frame.
    struct StubNative {
        stub: Arc<StubState>,
        state: Arc<WorkerState>,
        worker_sid: String,
    }

    fn frame_args(frame: &[u8]) -> Vec<String> {
        let body: Value = serde_json::from_slice(&frame[4..]).expect("the launch frame is JSON");
        body["args"].as_array().map(|args| args.iter().map(|arg| arg.as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default()
    }

    struct StubLaunch {
        stub: Arc<StubState>,
        state: Arc<WorkerState>,
        cwd: PathBuf,
        worker_sid: String,
    }

    impl NativeLaunch for StubLaunch {
        fn receipt(&self) -> Receipt {
            Receipt {
                purpose: Purpose::Review,
                worker_sid: self.worker_sid.clone(),
                worker_pid: 4242,
                process: None,
            }
        }

        fn resume(self: Box<Self>, frame: Vec<u8>) -> BoxFuture<'static, Result<Running, LaunchFailure>> {
            let args = frame_args(&frame);
            let paid = args.iter().any(|arg| arg == "--output-format");
            if paid {
                self.state.paid_launches.fetch_add(1, Ordering::SeqCst);
            }
            Box::pin(async move {
                if paid && self.state.fail_paid_start.load(Ordering::SeqCst) {
                    return Err(LaunchFailure {
                        message: "synthetic paid-start failure".into(),
                        stopped: false,
                    });
                }
                let child: CliChild = stub_child(self.stub.clone(), args, self.cwd.clone());
                let CliChild { stdin, stdout, stderr, exit, kill_tree, .. } = child;
                Ok(Running {
                    stdin,
                    stdout,
                    stderr,
                    exited: Box::pin(async move {
                        match exit.wait().await {
                            CliExit::Exited { code, .. } => code,
                            CliExit::Failed(_) => None,
                        }
                    }),
                    retire: Arc::new(move || {
                        let kill_tree = kill_tree.clone();
                        Box::pin(async move { kill_tree().await.is_ok() })
                    }),
                })
            })
        }

        fn abandon(self: Box<Self>) -> BoxFuture<'static, bool> {
            Box::pin(async { true })
        }
    }

    impl WorkerNative for StubNative {
        fn verify<'a>(&'a self, _config: &'a WorkerConfiguration, _known: &'a [Receipt]) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn read_credential<'a>(&'a self, _target: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
            let saved = json!({ "username": "TjReview-12345678", "password": "synthetic-worker-password" });
            Box::pin(async move { Ok(Some(saved.to_string())) })
        }

        fn grant_directory<'a>(&'a self, _config: &'a WorkerConfiguration, _path: &'a std::path::Path) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn launch<'a>(
            &'a self,
            _config: &'a WorkerConfiguration,
            _password: String,
            cwd: &'a std::path::Path,
            _environment: Vec<(String, String)>,
        ) -> BoxFuture<'a, Result<Box<dyn NativeLaunch>, LaunchFailure>> {
            let launch = StubLaunch {
                stub: self.stub.clone(),
                state: self.state.clone(),
                cwd: cwd.to_path_buf(),
                worker_sid: self.worker_sid.clone(),
            };
            Box::pin(async move { Ok(Box::new(launch) as Box<dyn NativeLaunch>) })
        }
    }

    /// The review worker: a real `WorkerBoundary` over a stub native layer, with its policy observation replaced.
    pub fn worker(h: &Harness) -> Worker {
        let root = h.temp.path();
        let config = WorkerConfiguration {
            version: 2,
            purpose: Purpose::Review,
            owner_sid: "S-1-5-21-1-2-3-1000".into(),
            worker_sid: "S-1-5-21-1-2-3-1001".into(),
            peer_sid: "S-1-5-21-1-2-3-1002".into(),
            worker_name: "TjReview-12345678".into(),
            worker_root: root.join("worker"),
            peer_root: root.join("provider"),
            runtime_root: root.join("runtime"),
            owner_data_root: root.join("owner"),
            provisioned_at: "2026-11-03T00:00:00.000Z".into(),
            credential_target: format!("Tjommie/{}/isolated-review", "a".repeat(24)),
        };
        for folder in [&config.worker_root, &config.runtime_root, &config.owner_data_root, &root.join("daemon")] {
            fs::create_dir_all(folder).unwrap();
        }
        // The CLI executable must sit inside the worker's runtime root.
        let cli = config.runtime_root.join("claude.exe");
        fs::write(&cli, b"").unwrap();
        let configuration_path = config.owner_data_root.join(Purpose::Review.configuration_file());
        fs::write(&configuration_path, serde_json::to_string(&config).unwrap()).unwrap();

        let state = Arc::new(WorkerState {
            observations: AtomicUsize::new(0),
            receipt: Mutex::new(policy()),
            paid_launches: AtomicUsize::new(0),
            fail_paid_start: AtomicBool::new(false),
            fail_after: AtomicBool::new(false),
        });
        let observed = state.clone();
        let observer: Observer = Arc::new(move || {
            let state = observed.clone();
            Box::pin(async move {
                state.observations.fetch_add(1, Ordering::SeqCst);
                if state.fail_after.load(Ordering::SeqCst) && state.paid_launches.load(Ordering::SeqCst) >= 2 {
                    return Err(BoundaryError::Termination);
                }
                Ok(state.receipt.lock().unwrap().clone())
            })
        });
        let native = Arc::new(StubNative {
            stub: h.stub.clone(),
            state: state.clone(),
            worker_sid: config.worker_sid.clone(),
        });
        let boundary = WorkerBoundary::new(BoundaryOptions {
            configuration_path,
            purpose: Purpose::Review,
            daemon_dir: root.join("daemon"),
            exclusive: Arc::new(|| true),
            native,
            observer: Some(observer),
        });
        Worker { boundary: Arc::new(boundary), state, config, cli }
    }

    /// The review runs through the worker: its CLI is the one in the runtime root, and its policy is observed for real.
    pub fn options(h: &Harness, worker: &Worker) -> ProjectReviewOptions {
        let command = CliCommand { file: worker.cli.to_string_lossy().into_owned(), args: vec![] };
        ProjectReviewOptions {
            worker: Some(worker.boundary.clone()),
            cli: Some(Arc::new(move || Some(command.clone()))),
            machine_policy_hash: None,
            ..h.options()
        }
    }

    /// Whether the worker's review lease is free: a second review scope can start.
    pub async fn lease_free(worker: &Worker) -> bool {
        worker.boundary.with_review_policy(|_| async { Ok(()) }).await.is_ok()
    }

    pub fn scratch_entries(worker: &Worker) -> usize {
        read_names(&worker.config.worker_root.join("accounts").join("reviewer-scratch")).len()
    }
}

#[cfg(windows)]
#[tokio::test]
async fn requires_fresh_worker_evidence_before_each_paid_turn_and_releases_only_after_the_final_policy_observation() {
    let h = Harness::new("good");
    let worker = worker::worker(&h);
    let runtime = create_project_review_runtime(worker::options(&h, &worker)).unwrap();
    let result = runtime.review(&snapshot(), &token(), &context()).await.unwrap();
    assert_eq!(result.state, EvaluationState::Passed);
    assert!(worker.state.observations.load(Ordering::SeqCst) >= 7);
    assert!(worker::lease_free(&worker).await);
    assert_eq!(h.account.released(), 1);
    let receipts = h.receipts();
    let receipt = read_json(&receipts[0]);
    assert_eq!(
        receipt["identity"]["workerPolicy"],
        json!({
            "purpose": "review",
            "workerSid": "S-1-5-21-1-2-3-1001",
            "providerSid": "S-1-5-21-1-2-3-1002",
            "configurationSha256": "c".repeat(64),
            "observerSha256": "a".repeat(64),
            "policySha256": "b".repeat(64),
        })
    );
    assert_eq!(worker::scratch_entries(&worker), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn blocks_the_review_turn_when_fresh_worker_evidence_changes_after_acceptance() {
    let policy_state = Arc::new(Mutex::new(None::<Arc<worker::WorkerState>>));
    let slot = policy_state.clone();
    let account = FakeAccount {
        on_usage: Some(Box::new(move || {
            if let Some(state) = slot.lock().unwrap().as_ref() {
                state.receipt.lock().unwrap().policy_sha256 = "c".repeat(64);
            }
        })),
        ..FakeAccount::api()
    };
    let h = Harness::with_account("good", account);
    let worker = worker::worker(&h);
    *policy_state.lock().unwrap() = Some(worker.state.clone());
    let runtime = create_project_review_runtime(worker::options(&h, &worker)).unwrap();
    let result = runtime.review(&snapshot(), &token(), &context()).await.unwrap();
    assert_eq!(result.state, EvaluationState::Unavailable);
    assert!(result.reason.unwrap_or_default().contains("review_policy_changed"));
    assert_eq!(h.account.usages().len(), 1);
    assert!(!h.reviews().join("receipts").exists());
    assert!(worker::lease_free(&worker).await);
    assert_eq!(h.account.released(), 1);
}

#[cfg(windows)]
#[tokio::test]
async fn retains_worker_account_and_scratch_leases_after_unconfirmed_paid_start_or_final_observer_retirement() {
    for failure in ["paidStart", "after"] {
        let h = Harness::new("good");
        let worker = worker::worker(&h);
        worker.state.fail_paid_start.store(failure == "paidStart", Ordering::SeqCst);
        worker.state.fail_after.store(failure == "after", Ordering::SeqCst);
        let runtime = create_project_review_runtime(worker::options(&h, &worker)).unwrap();
        let first = runtime.review(&snapshot(), &token(), &context()).await;
        assert!(matches!(first, Err(CheckError::Termination(_))), "{failure}: {first:?}");
        let second = runtime.review(&snapshot(), &token(), &context()).await.unwrap();
        assert!(second.reason.unwrap_or_default().contains("review_worker_busy"), "{failure}");
        assert!(!worker::lease_free(&worker).await, "{failure}: the worker lease was released");
        assert_eq!(worker::scratch_entries(&worker), 1, "{failure}");
        assert!(h.evaluations().is_empty(), "{failure}");
        assert_eq!(h.account.released(), 0, "{failure}");
        let usages = h.account.usages();
        if failure == "paidStart" {
            assert_eq!(usages.len(), 1);
            assert!(usages[0].cost_usd.is_none());
        } else {
            assert_eq!(usages.len(), 2);
        }
    }
}

