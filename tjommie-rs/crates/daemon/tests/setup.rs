//! Port of `packages/daemon/src/setup.test.ts`. A scriptable fake host stands in for the CLIs, as the TypeScript
//! mocks of `cli.ts` and `codex-rpc.ts` do. No real CLI runs and no real account is touched.

mod common;

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use daemon::account_quota::{AccountUsageRead, UsageFetch, UsageResponse};
use daemon::cli::{
    BoxFuture, ChildExit, CliChild, CliCommand, CliExit, CliHost, CliOutput, Env, KillTree,
    LaunchOptions,
};
use daemon::providers::codex_rpc::{CodexServer, JsonRpcLines};
use daemon::providers::{LimitState, LimitWindow, ProviderError};
use daemon::setup::{
    Account, Setup, parse_claude_status, parse_codex_account_limits, parse_version,
};
use protocol::ErrorCode;
use protocol::model::{CodexSandboxReadiness, Provider, ProviderSetup, SetupFields};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

type Run = Arc<dyn Fn(&CliCommand, &[String]) -> BoxFuture<'static, CliOutput> + Send + Sync>;
type StartServer = BoxFuture<'static, Result<CodexServer, ProviderError>>;

fn output(code: Option<i32>, stdout: &str) -> CliOutput {
    CliOutput {
        code,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

/// `successfulCliRun`.
fn successful(cmd: &CliCommand, args: &[String]) -> CliOutput {
    if args.first().map(String::as_str) == Some("--version") {
        return output(
            Some(0),
            if cmd.file == "claude" {
                "2.1.280 (Claude Code)\n"
            } else {
                "codex-cli 0.155.1\n"
            },
        );
    }
    output(
        Some(0),
        r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#,
    )
}

/// A hidden child the test can end, and its stdio ends.
struct Launched {
    exit: ChildExit,
    stdout: DuplexStream,
    stderr: DuplexStream,
}

#[derive(Default)]
struct Host {
    runs: Mutex<Vec<(CliCommand, Vec<String>, Env)>>,
    run: Mutex<Option<Run>>,
    run_once: Mutex<VecDeque<Run>>,
    opened: Mutex<Vec<String>>,
    launches: Mutex<Vec<(CliCommand, Vec<String>, LaunchOptions)>>,
    children: Mutex<Vec<Launched>>,
    fail_launch: Mutex<bool>,
    servers: Mutex<VecDeque<StartServer>>,
    server_starts: Mutex<Vec<(CliCommand, Env, String)>>,
    kills: Arc<Mutex<Vec<u32>>>,
    fail_kill_once: Arc<Mutex<bool>>,
}

impl Host {
    fn set_run(&self, run: impl Fn(&CliCommand, &[String]) -> CliOutput + Send + Sync + 'static) {
        let run = Arc::new(run);
        *self.run.lock().unwrap() = Some(Arc::new(move |cmd: &CliCommand, args: &[String]| {
            let out = run(cmd, args);
            Box::pin(async move { out })
        }));
    }

    fn run_once(&self, run: Run) {
        self.run_once.lock().unwrap().push_back(run);
    }

    fn runs(&self) -> usize {
        self.runs.lock().unwrap().len()
    }

    fn push_server(&self, server: CodexServer) {
        self.servers
            .lock()
            .unwrap()
            .push_back(Box::pin(async { Ok(server) }));
    }

    fn launched(&self) -> usize {
        self.children.lock().unwrap().len()
    }

    fn end_child(&self, index: usize, code: i32) {
        self.children.lock().unwrap()[index]
            .exit
            .fire(CliExit::Exited {
                code: Some(code),
                signal: None,
            });
    }
}

impl CliHost for Host {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        self.launches
            .lock()
            .unwrap()
            .push((cmd.clone(), args, options));
        Box::pin(async move {
            if std::mem::take(&mut *self.fail_launch.lock().unwrap()) {
                return Err(io::Error::other("Hidden launcher unavailable"));
            }
            let (stdin, mut stdin_test) = duplex(1 << 16);
            let (stdout, stdout_test) = duplex(1 << 16);
            let (stderr, stderr_test) = duplex(1 << 16);
            let exit = ChildExit::new();
            // Like Claude's login ("Paste code here if prompted"), the child aborts when its stdin reaches end-of-input.
            tokio::spawn({
                let exit = exit.clone();
                async move {
                    let mut buffer = [0u8; 64];
                    while matches!(stdin_test.read(&mut buffer).await, Ok(read) if read > 0) {}
                    exit.fire(CliExit::Exited {
                        code: Some(1),
                        signal: None,
                    });
                }
            });
            let kill_tree: KillTree = {
                let (exit, fail, kills) = (
                    exit.clone(),
                    Arc::clone(&self.fail_kill_once),
                    Arc::clone(&self.kills),
                );
                Arc::new(move || {
                    kills.lock().unwrap().push(12345);
                    let (exit, fail) = (exit.clone(), Arc::clone(&fail));
                    Box::pin(async move {
                        if std::mem::take(&mut *fail.lock().unwrap()) {
                            return Err("Containment failed".to_string());
                        }
                        exit.fire(CliExit::Exited {
                            code: Some(1),
                            signal: None,
                        });
                        Ok(())
                    })
                })
            };
            self.children.lock().unwrap().push(Launched {
                exit: exit.clone(),
                stdout: stdout_test,
                stderr: stderr_test,
            });
            Ok(CliChild {
                pid: Some(12345),
                stdin: Box::new(stdin),
                stdout: Box::new(stdout),
                stderr: Box::new(stderr),
                exit,
                kill_tree,
                retirement: None,
            })
        })
    }

    fn start_codex_server<'a>(
        &'a self,
        cmd: &'a CliCommand,
        env: &'a Env,
        cwd: &'a str,
        _ephemeral: bool,
    ) -> StartServer {
        self.server_starts
            .lock()
            .unwrap()
            .push((cmd.clone(), env.clone(), cwd.into()));
        let next = self.servers.lock().unwrap().pop_front();
        next.unwrap_or_else(|| {
            Box::pin(async { Ok(server(|_, _| Ok(json!({ "account": null }))).0) })
        })
    }

    fn find_cli(&self, name: Provider) -> Option<CliCommand> {
        let file = if name == Provider::Claude {
            "claude"
        } else {
            "codex"
        };
        Some(CliCommand {
            file: file.into(),
            args: vec![],
            codex_package: None,
        })
    }

    fn run_cli<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: &'a [String],
        env: &'a Env,
        _timeout: Duration,
    ) -> BoxFuture<'a, io::Result<CliOutput>> {
        self.runs
            .lock()
            .unwrap()
            .push((cmd.clone(), args.to_vec(), env.clone()));
        let once = self.run_once.lock().unwrap().pop_front();
        let run = once.or_else(|| self.run.lock().unwrap().clone());
        Box::pin(async move {
            Ok(match run {
                Some(run) => run(cmd, args).await,
                None => successful(cmd, args),
            })
        })
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        self.opened.lock().unwrap().push(url.into());
        Ok(())
    }
}

/// A fake app-server answering requests with `respond(method, params)`. Returns its close count and RPC.
fn server(
    respond: impl Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'static,
) -> (CodexServer, Arc<AtomicUsize>, Arc<JsonRpcLines>, ChildExit) {
    server_with(move |_, method, params| respond(method, params))
}

/// `server`, where `respond` can also send notifications before it answers.
fn server_with(
    respond: impl Fn(&JsonRpcLines, &str, &Value) -> Result<Value, String> + Send + Sync + 'static,
) -> (CodexServer, Arc<AtomicUsize>, Arc<JsonRpcLines>, ChildExit) {
    let rpc = Arc::new_cyclic(|weak: &Weak<JsonRpcLines>| {
        let weak = weak.clone();
        JsonRpcLines::new(move |line| {
            let message: Value = serde_json::from_str(&line).unwrap();
            if let (Some(id), Some(method)) = (message.get("id"), message["method"].as_str()) {
                let rpc = weak.upgrade().unwrap();
                let reply = match respond(&rpc, method, &message["params"]) {
                    Ok(result) => json!({ "id": id, "result": result }),
                    Err(error) => {
                        json!({ "id": id, "error": { "code": -32603, "message": error } })
                    }
                };
                rpc.handle_line(&reply.to_string());
            }
            Ok(())
        })
    });
    let closes = Arc::new(AtomicUsize::new(0));
    let exit = ChildExit::new();
    let close = {
        let (closes, exit) = (Arc::clone(&closes), exit.clone());
        Arc::new(move || {
            closes.fetch_add(1, Ordering::SeqCst);
            exit.fire(CliExit::Exited {
                code: Some(0),
                signal: None,
            });
        })
    };
    let codex = CodexServer {
        rpc: Arc::clone(&rpc),
        pid: Some(4242),
        exit: exit.clone(),
        kill_tree: Arc::new(|| Box::pin(async { Ok(()) })),
        close,
        retirement: None,
    };
    (codex, closes, rpc, exit)
}

struct NoFetch;

impl UsageFetch for NoFetch {
    fn get<'a>(
        &'a self,
        _: &'a str,
        _: &'a [(&'static str, String)],
        _: Duration,
    ) -> BoxFuture<'a, Result<UsageResponse, String>> {
        Box::pin(async { Err("no network in tests".to_string()) })
    }
}

struct Fixture {
    root: tempfile::TempDir,
    db: Connection,
    host: Arc<Host>,
    setup: Setup,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("tjommie test ñ-")
            .tempdir()
            .unwrap();
        let db = common::test_db(root.path());
        let host = Arc::new(Host::default());
        let cli: Arc<dyn CliHost> = host.clone();
        let setup = Setup::new(cli, Arc::new(NoFetch));
        Fixture {
            root,
            db,
            host,
            setup,
        }
    }

    fn accounts_dir(&self) -> PathBuf {
        self.root.path().join("accounts")
    }

    fn ensure(&self) -> Vec<Account> {
        self.setup
            .ensure_accounts(&self.db, &self.accounts_dir(), self.root.path())
            .unwrap()
    }

    fn account(&self, provider: Provider) -> Account {
        self.setup.account_for(&self.db, provider).unwrap()
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// describe("authoritative Codex quota checks")

fn window() -> Value {
    json!({ "usedPercent": 25, "windowDurationMins": 300, "resetsAt": null })
}

fn limit(key: &str, used: Option<f64>, duration: Option<f64>, state: LimitState) -> LimitWindow {
    LimitWindow {
        window_key: key.into(),
        duration_minutes: duration,
        used_percent: used,
        resets_at: None,
        state,
    }
}

#[tokio::test]
async fn the_full_usage_probe_retains_credit_metadata_and_permission_denial_without_a_model_turn_or_auth_file_read()
 {
    let f = Fixture::new();
    let account = Account {
        id: "selected".into(),
        provider: Provider::Codex,
        label: "Synthetic".into(),
        folder: text(&f.root.path().join("selected")),
    };
    let read = json!({ "ordinaryUsageAllowed": false, "rateLimits": { "primary": window(), "spendControlReached": false,
        "credits": { "balance": "38917.83", "hasCredits": true, "unlimited": false }, "planType": "pro" } });
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (first, closes, _, _) = server({
        let (read, requests) = (read.clone(), Arc::clone(&requests));
        move |method, params| {
            requests
                .lock()
                .unwrap()
                .push((method.to_string(), params.clone()));
            Ok(read.clone())
        }
    });
    f.host.push_server(first);
    let result = f.setup.check_account_usage(&account, f.root.path()).await;
    let AccountUsageRead::Available {
        usage,
        limits,
        recovery_allowed,
    } = &result
    else {
        panic!("{result:?}")
    };
    assert!(!recovery_allowed);
    assert_eq!(
        (
            usage.credits.as_ref().unwrap().balance.as_deref(),
            usage.plan_label.as_deref()
        ),
        (Some("38917.83"), Some("pro"))
    );
    assert!(
        limits
            .iter()
            .any(|snapshot| snapshot.limit_id == "provider")
    );
    assert_eq!(
        *requests.lock().unwrap(),
        vec![("account/rateLimits/read".to_string(), json!({}))]
    );
    assert_eq!(closes.load(Ordering::SeqCst), 1);

    let (failing, failing_closes, _, _) =
        server(|_, _| Err("synthetic private credential response".into()));
    f.host.push_server(failing);
    assert_eq!(
        f.setup.check_account_usage(&account, f.root.path()).await,
        AccountUsageRead::Error {
            error: Some("Provider usage could not be read. Try again later.".into())
        }
    );
    assert_eq!(failing_closes.load(Ordering::SeqCst), 1);
}

#[test]
fn parses_every_returned_bucket_and_window_and_preserves_unknown_resets_and_provider_only_exhaustion()
 {
    let value = json!({ "rateLimitsByLimitId": {
        "codex": { "limitId": "codex", "primary": window(), "secondary": { "usedPercent": 100, "windowDurationMins": 10080, "resetsAt": null } },
        "other": { "primary": window(), "rateLimitReachedType": "unknown" },
    } });
    let snapshots = parse_codex_account_limits(&value).unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.limit_id.as_str())
            .collect::<Vec<_>>(),
        ["codex", "other"]
    );
    assert_eq!(
        snapshots[0].windows,
        vec![
            limit("primary", Some(25.0), Some(300.0), LimitState::Ok),
            limit("secondary", Some(100.0), Some(10080.0), LimitState::Reached),
        ]
    );
    assert_eq!(
        snapshots[1].windows,
        vec![
            limit("primary", Some(25.0), Some(300.0), LimitState::Ok),
            limit("provider", None, None, LimitState::Reached)
        ]
    );
    assert_eq!(
        parse_codex_account_limits(&json!({ "rateLimits": { "primary": window() } })).unwrap()[0]
            .limit_id,
        "codex"
    );
}

#[test]
fn rejects_incomplete_or_malformed_authoritative_responses() {
    let with = |key: &str, value: Value| {
        let mut window = window();
        window[key] = value;
        window
    };
    for value in [
        json!({}),
        json!({ "rateLimitsByLimitId": {} }),
        json!({ "rateLimits": { "primary": null, "secondary": null } }),
        json!({ "rateLimits": { "primary": with("usedPercent", Value::Null) } }),
        json!({ "rateLimits": { "primary": with("resetsAt", json!("unknown")) } }),
        json!({ "rateLimitsByLimitId": { "codex": { "primary": window() }, "other": { "primary": with("usedPercent", json!(101)) } } }),
        json!({ "rateLimitsByLimitId": { "codex": { "limitId": "different", "primary": window() } } }),
    ] {
        assert_eq!(parse_codex_account_limits(&value), None, "{value}");
    }
}

#[tokio::test]
async fn uses_only_a_bounded_metadata_rpc_in_the_selected_account_and_awaits_owned_cleanup() {
    let f = Fixture::new();
    let folder = tempfile::tempdir().unwrap();
    let account = Account {
        id: "selected".into(),
        provider: Provider::Codex,
        label: "Selected".into(),
        folder: text(folder.path()),
    };
    let (mut codex, closes, _, exit) =
        server(|_, _| Ok(json!({ "rateLimits": { "primary": window() } })));
    // close() doesn't end this server: the test does, after checking that the read waits for it.
    codex.close = {
        let closes = Arc::clone(&closes);
        Arc::new(move || {
            closes.fetch_add(1, Ordering::SeqCst);
        })
    };
    f.host.push_server(codex);
    let checking = tokio::spawn({
        let setup = f.setup.clone();
        let account = account.clone();
        async move { setup.check_account_limits(&account).await }
    });
    providers_settle().await;
    assert!(!checking.is_finished());
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    let (cmd, env, cwd) = f.host.server_starts.lock().unwrap()[0].clone();
    assert_eq!(
        (cmd.file.as_str(), env.get("CODEX_HOME"), cwd.as_str()),
        ("codex", Some(&account.folder), account.folder.as_str())
    );
    exit.fire(CliExit::Exited {
        code: Some(0),
        signal: None,
    });
    assert_eq!(
        checking.await.unwrap().map(|snapshots| snapshots.len()),
        Some(1)
    );

    let (failing, failing_closes, _, _) = server(|_, _| Err("private provider error".into()));
    f.host.push_server(failing);
    assert_eq!(f.setup.check_account_limits(&account).await, None);
    assert_eq!(failing_closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.setup
            .check_account_limits(&Account {
                provider: Provider::Claude,
                ..account
            })
            .await,
        None
    );
    assert_eq!(f.host.server_starts.lock().unwrap().len(), 2);
}

async fn providers_settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

// describe("accounts")

#[test]
fn sweeps_only_confirmed_stale_session_folders_and_preserves_live_or_uncertain_owners() {
    let f = Fixture::new();
    f.ensure();
    let folder = f.accounts_dir().join("claude-main");
    let cases: [(&str, Option<Value>, bool); 8] = [
        (
            "stale",
            Some(json!({ "version": 1, "ownerPid": 100, "phase": "started", "childPid": 200 })),
            true,
        ),
        (
            "preparing",
            Some(json!({ "version": 1, "ownerPid": 100, "phase": "preparing", "childPid": null })),
            true,
        ),
        (
            "live-owner",
            Some(json!({ "version": 1, "ownerPid": 101, "phase": "started", "childPid": 200 })),
            false,
        ),
        (
            "surviving-child",
            Some(json!({ "version": 1, "ownerPid": 100, "phase": "started", "childPid": 201 })),
            false,
        ),
        (
            "unknown-process",
            Some(json!({ "version": 1, "ownerPid": 102, "phase": "started", "childPid": 200 })),
            false,
        ),
        (
            "starting",
            Some(json!({ "version": 1, "ownerPid": 100, "phase": "starting", "childPid": null })),
            false,
        ),
        (
            "malformed",
            Some(json!({ "version": 1, "ownerPid": "wrong", "phase": "started", "childPid": 200 })),
            false,
        ),
        ("markerless", None, false),
    ];
    for (name, owner, _) in &cases {
        let dir = folder.join(format!(".tjommie-session-{name}"));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("instructions.txt"),
            "Preserve unless confirmed stale",
        )
        .unwrap();
        if let Some(owner) = owner {
            std::fs::write(dir.join("owner.json"), owner.to_string()).unwrap();
        }
    }
    let outside = f.root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(
        outside.join("owner.json"),
        cases[0].1.as_ref().unwrap().to_string(),
    )
    .unwrap();
    std::fs::write(outside.join("keep.txt"), "outside").unwrap();
    common::junction(&outside, &folder.join(".tjommie-session-junction"));
    std::fs::create_dir(folder.join("unrelated")).unwrap();
    std::fs::write(
        folder.join("unrelated").join("owner.json"),
        cases[0].1.as_ref().unwrap().to_string(),
    )
    .unwrap();
    // TypeScript mocks process.kill: 101 and 201 are alive, 102 is unknown (EPERM), the rest are gone.
    daemon::providers::claude_files::sweep_claude_session_files_with(&folder, |pid| {
        !matches!(pid, 101 | 201 | 102)
    });
    for (name, _, removed) in &cases {
        assert_eq!(
            folder.join(format!(".tjommie-session-{name}")).exists(),
            !removed,
            "{name}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "outside"
    );
    assert!(folder.join("unrelated").exists());
}

/// `Read(//c/...)`: Claude matches absolute Windows paths as POSIX drive paths.
fn rules_for(folder: &Path) -> Vec<String> {
    let text = text(folder).replace('\\', "/");
    let absolute = match text.split_once(':') {
        Some((drive, rest)) if drive.len() == 1 => format!("/{}{rest}", drive.to_lowercase()),
        _ => text,
    };
    vec![
        format!("Read(/{absolute}/accounts/**)"),
        format!("Read(/{absolute}/daemon-token)"),
        format!("Read(/{absolute}/data/**)"),
    ]
}

fn settings(f: &Fixture) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(f.accounts_dir().join("claude-main").join("settings.json"))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn merges_claude_deny_rules_without_dropping_existing_settings_or_permissions() {
    let f = Fixture::new();
    f.ensure();
    let file = f.accounts_dir().join("claude-main").join("settings.json");
    std::fs::write(&file, json!({ "theme": "dark", "permissions": { "allow": ["Read"], "deny": ["Bash(rm *)"], "defaultMode": "manual" } }).to_string()).unwrap();
    f.ensure();
    let merged = settings(&f);
    assert_eq!(
        (
            merged["theme"].clone(),
            merged["permissions"]["allow"].clone(),
            merged["permissions"]["defaultMode"].clone()
        ),
        (json!("dark"), json!(["Read"]), json!("manual"))
    );
    let mut expected = vec![
        "Bash(rm *)".to_string(),
        "Edit(**/.claude/**)".into(),
        "Write(**/.claude/**)".into(),
    ];
    expected.extend(rules_for(f.root.path()));
    assert_eq!(merged["permissions"]["deny"], json!(expected));
    f.ensure();
    assert_eq!(settings(&f), merged);
}

#[test]
fn prunes_only_managed_deny_rules_for_missing_data_roots_and_preserves_live_roots_and_user_rules() {
    let f = Fixture::new();
    let stale_root = f.root.path().join("removed data");
    let live_root = f.root.path().join("live data");
    std::fs::create_dir(&live_root).unwrap();
    f.ensure();
    let legacy_db = |folder: &Path| rules_for(folder)[0].replace("accounts/**", "tjommie.db*");
    let (stale_rules, live_rules) = (rules_for(&stale_root), rules_for(&live_root));
    let (stale_legacy, live_legacy, legacy_only) = (
        legacy_db(&stale_root),
        legacy_db(&live_root),
        legacy_db(&f.root.path().join("legacy-only")),
    );
    let mut user_rules: Vec<String> = vec![
        "Bash(rm *)".into(),
        "Read(//unrelated/private/**)".into(),
        "Read(**/daemon-token)".into(),
    ];
    user_rules.push(rules_for(&f.root.path().join("user-only"))[0].clone());
    user_rules.extend(rules_for(&f.root.path().join("user-*")));
    user_rules.push(stale_rules[0].replace("Read(", "Write("));
    user_rules.push(stale_rules[2].replace("data/**", "private/**"));
    let mut deny: Vec<String> = stale_rules.clone();
    deny.push(stale_legacy);
    deny.extend(live_rules.clone());
    deny.push(live_legacy.clone());
    deny.push(legacy_only);
    deny.extend(user_rules.clone());
    let file = f.accounts_dir().join("claude-main").join("settings.json");
    std::fs::write(
        &file,
        json!({ "theme": "dark", "permissions": { "allow": ["Read"], "deny": deny } }).to_string(),
    )
    .unwrap();
    f.ensure();
    let merged = settings(&f);
    assert_eq!(
        (
            merged["theme"].clone(),
            merged["permissions"]["allow"].clone()
        ),
        (json!("dark"), json!(["Read"]))
    );
    let mut expected = live_rules;
    expected.push(live_legacy);
    expected.extend(user_rules);
    expected.extend([
        "Edit(**/.claude/**)".to_string(),
        "Write(**/.claude/**)".to_string(),
    ]);
    expected.extend(rules_for(f.root.path()));
    assert_eq!(merged["permissions"]["deny"], json!(expected));
    f.ensure();
    assert_eq!(settings(&f), merged);
}

#[test]
fn logs_stale_folder_removal_failures_and_continues_account_setup() {
    let f = Fixture::new();
    f.ensure();
    let folder = f
        .accounts_dir()
        .join("claude-main")
        .join(".tjommie-session-locked");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("owner.json"),
        json!({ "version": 1, "ownerPid": 100, "phase": "started", "childPid": 200 }).to_string(),
    )
    .unwrap();
    // Held open without delete sharing, a file keeps its folder from being removed (TypeScript mocks rmSync).
    let locked = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0)
            .open(folder.join("in-use.txt"))
            .unwrap()
    };
    daemon::providers::claude_files::sweep_claude_session_files_with(
        &f.accounts_dir().join("claude-main"),
        |_| true,
    );
    assert_eq!(f.ensure().len(), 2);
    assert!(folder.exists());
    drop(locked);
}

#[test]
fn keeps_malformed_claude_settings_intact_and_continues_account_setup() {
    for invalid in [
        "not JSON",
        "null",
        "[]",
        "3",
        "\"text\"",
        r#"{"permissions":null}"#,
        r#"{"permissions":[]}"#,
        r#"{"permissions":"wrong"}"#,
        r#"{"permissions":{"deny":null}}"#,
        r#"{"permissions":{"deny":"wrong"}}"#,
        r#"{"permissions":{"deny":{}}}"#,
        r#"{"permissions":{"deny":[3]}}"#,
    ] {
        let f = Fixture::new();
        f.ensure();
        let file = f.accounts_dir().join("claude-main").join("settings.json");
        assert!(file.exists());
        let bytes = format!("  {invalid}\r\n").into_bytes();
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(f.ensure().len(), 2, "{invalid}");
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "{invalid}");
    }
}

#[test]
fn still_reports_filesystem_failures_while_reading_claude_settings() {
    let f = Fixture::new();
    f.ensure();
    let file = f.accounts_dir().join("claude-main").join("settings.json");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(
        f.setup
            .ensure_accounts(&f.db, &f.accounts_dir(), f.root.path())
            .is_err()
    );
}

#[test]
fn creates_one_account_per_provider_once_each_with_its_folder() {
    let f = Fixture::new();
    let first = f.ensure();
    assert_eq!(f.ensure(), first);
    assert_eq!(
        first
            .iter()
            .map(|account| account.id.as_str())
            .collect::<Vec<_>>(),
        ["claude-main", "codex-main"]
    );
    assert!(
        first
            .iter()
            .all(|account| Path::new(&account.folder).exists())
    );
}

#[test]
fn points_each_cli_at_its_account_folder_and_never_passes_inherited_keys() {
    let f = Fixture::new();
    f.ensure();
    let inherited: Env = [
        ("ANTHROPIC_API_KEY", "sk-inherited"),
        ("CODEX_HOME", "C:\\somewhere-else"),
        ("Path", "bin"),
    ]
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .into();
    let claude = f
        .setup
        .account_env_from(&f.account(Provider::Claude), &inherited);
    assert_eq!(
        claude.get("CLAUDE_CONFIG_DIR"),
        Some(&text(&f.accounts_dir().join("claude-main")))
    );
    assert_eq!(claude.get("ANTHROPIC_API_KEY"), None);
    let codex = f
        .setup
        .account_env_from(&f.account(Provider::Codex), &inherited);
    assert_eq!(
        codex.get("CODEX_HOME"),
        Some(&text(&f.accounts_dir().join("codex-main")))
    );
}

// describe("parsers")

#[test]
fn reads_both_clis_version_output() {
    assert_eq!(
        parse_version("2.1.280 (Claude Code)\n").as_deref(),
        Some("2.1.280")
    );
    assert_eq!(
        parse_version("codex-cli 0.155.1\n").as_deref(),
        Some("0.155.1")
    );
    assert_eq!(parse_version("command not found"), None);
}

#[test]
fn reads_claude_auth_status_json() {
    let signed_out = parse_claude_status(
        r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#,
    );
    assert_eq!(
        (signed_out.signed_in, signed_out.account, signed_out.plan),
        (false, None, None)
    );
    let signed_in = parse_claude_status(
        r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "person@example.com", "subscriptionType": "max"}"#,
    );
    assert_eq!(
        (
            signed_in.signed_in,
            signed_in.account.as_deref(),
            signed_in.plan.as_deref()
        ),
        (true, Some("person@example.com"), Some("max"))
    );
    let garbage = parse_claude_status("not json");
    assert_eq!(
        (garbage.signed_in, garbage.account, garbage.plan),
        (false, None, None)
    );
}

// describe("setup checks")

fn codex_sandbox(setups: &[ProviderSetup]) -> CodexSandboxReadiness {
    match &setups[1] {
        ProviderSetup::Codex(codex) => codex.sandbox,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn requires_both_elevated_account_config_and_app_server_readiness() {
    let f = Fixture::new();
    f.ensure();
    let folder = f.account(Provider::Codex).folder;
    for (sandbox, readiness, expected) in [
        (
            "elevated",
            Ok(json!({ "status": "ready" })),
            CodexSandboxReadiness::Ready,
        ),
        (
            "unelevated",
            Ok(json!({ "status": "ready" })),
            CodexSandboxReadiness::NotConfigured,
        ),
        (
            "elevated",
            Err("query failed".to_string()),
            CodexSandboxReadiness::Unavailable,
        ),
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (codex, _, _, _) = server({
            let requests = Arc::clone(&requests);
            move |method, params| {
                requests
                    .lock()
                    .unwrap()
                    .push((method.to_string(), params.clone()));
                match method {
                    "account/read" => Ok(json!({ "account": null })),
                    "config/read" => Ok(json!({ "config": { "windows": { "sandbox": sandbox } } })),
                    "windowsSandbox/readiness" => readiness.clone(),
                    other => Err(other.to_string()),
                }
            }
        });
        f.host.push_server(codex);
        assert_eq!(codex_sandbox(&f.setup.check_setup(&f.db).await), expected);
        let requests = requests.lock().unwrap();
        assert!(requests.contains(&("config/read".to_string(), json!({ "cwd": folder }))));
        assert!(requests.contains(&("windowsSandbox/readiness".to_string(), json!({}))));
    }
}

fn claude_setup(version: Option<&str>, error: Option<&str>) -> ProviderSetup {
    ProviderSetup::Claude(SetupFields {
        installed: true,
        version: version.map(str::to_string),
        signed_in: false,
        account: None,
        plan: None,
        error: error.map(str::to_string),
    })
}

#[tokio::test]
async fn treats_claudes_exit_1_signed_out_json_as_a_successful_check() {
    let f = Fixture::new();
    f.ensure();
    f.host.set_run(|cmd, args| {
        if cmd.file == "claude" && args[0] == "auth" {
            return output(
                Some(1),
                r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#,
            );
        }
        successful(cmd, args)
    });
    assert_eq!(
        f.setup.check_setup(&f.db).await[0],
        claude_setup(Some("2.1.280"), None)
    );
}

#[tokio::test]
async fn reports_a_cli_spawn_failure() {
    let f = Fixture::new();
    f.ensure();
    f.host.set_run(|cmd, args| {
        if cmd.file == "claude" && args[0] == "--version" {
            return CliOutput {
                code: None,
                stdout: String::new(),
                stderr: "spawn ENOENT".into(),
            };
        }
        successful(cmd, args)
    });
    assert_eq!(
        f.setup.check_setup(&f.db).await[0],
        claude_setup(None, Some("Claude Code version check failed"))
    );
}

#[tokio::test]
async fn reports_garbage_status_output_and_a_non_zero_version_exit() {
    let f = Fixture::new();
    f.ensure();
    f.host
        .set_run(|cmd, args| match (cmd.file.as_str(), args[0].as_str()) {
            ("claude", "auth") => CliOutput {
                code: Some(2),
                stdout: String::new(),
                stderr: "not signed in".into(),
            },
            ("codex", "--version") => CliOutput {
                code: Some(4),
                stdout: String::new(),
                stderr: "failed".into(),
            },
            _ => successful(cmd, args),
        });
    let setups = f.setup.check_setup(&f.db).await;
    assert_eq!(
        setups[0],
        claude_setup(Some("2.1.280"), Some("Claude sign-in check failed"))
    );
    let ProviderSetup::Codex(codex) = &setups[1] else {
        panic!()
    };
    assert_eq!(
        (
            codex.setup.installed,
            codex.setup.version.clone(),
            codex.setup.signed_in,
            codex.setup.error.as_deref()
        ),
        (true, None, false, Some("Codex version check failed"))
    );
    assert!(f.host.server_starts.lock().unwrap().is_empty());
}

// describe("Codex sandbox setup action")

#[tokio::test]
async fn starts_once_waits_for_completion_restarts_the_app_server_and_checks_readiness() {
    let f = Fixture::new();
    f.ensure();
    let first_requests = Arc::new(Mutex::new(Vec::new()));
    let (first, first_closes, first_rpc, _) = server({
        let requests = Arc::clone(&first_requests);
        move |method, params| {
            requests.lock().unwrap().push(method.to_string());
            if method != "windowsSandbox/setupStart" {
                return Err(method.to_string());
            }
            assert_eq!(params["mode"], "elevated");
            assert!(
                params["cwd"]
                    .as_str()
                    .unwrap()
                    .contains("tjommie-codex-setup-")
            );
            Ok(json!({ "started": true }))
        }
    });
    let completes = Arc::clone(&first_rpc);
    // The completion arrives right after setupStart's response, as the TypeScript fake queues it.
    let (second, second_closes, _, _) = server(|method, _| {
        Ok(if method == "config/read" {
            json!({ "config": { "windows": { "sandbox": "elevated" } } })
        } else {
            json!({ "status": "ready" })
        })
    });
    f.host.push_server(first);
    f.host.push_server(second);
    let preparing = tokio::spawn({
        let setup = f.setup.clone();
        let account = f.account(Provider::Codex);
        async move {
            setup
                .prepare_account_codex_sandbox(&account, Duration::from_millis(5000))
                .await
        }
    });
    providers_settle().await;
    completes.handle_line(&json!({ "method": "windowsSandbox/setupCompleted", "params": { "mode": "elevated", "success": true, "error": null } }).to_string());
    preparing.await.unwrap().unwrap();
    assert_eq!(
        *first_requests.lock().unwrap(),
        ["windowsSandbox/setupStart"]
    );
    assert_eq!(
        (
            first_closes.load(Ordering::SeqCst),
            second_closes.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
}

#[tokio::test]
async fn reports_a_declined_setup_without_retrying() {
    let f = Fixture::new();
    f.ensure();
    let starts = Arc::new(AtomicUsize::new(0));
    let (codex, _, rpc, _) = server({
        let starts = Arc::clone(&starts);
        move |_, _| {
            starts.fetch_add(1, Ordering::SeqCst);
            Ok(json!({ "started": true }))
        }
    });
    f.host.push_server(codex);
    let preparing = tokio::spawn({
        let setup = f.setup.clone();
        let account = f.account(Provider::Codex);
        async move {
            setup
                .prepare_account_codex_sandbox(&account, Duration::from_millis(5000))
                .await
        }
    });
    providers_settle().await;
    rpc.handle_line(&json!({ "method": "windowsSandbox/setupCompleted", "params": { "mode": "elevated", "success": false, "error": "Declined" } }).to_string());
    assert!(
        preparing
            .await
            .unwrap()
            .unwrap_err()
            .message
            .contains("Declined")
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stops_waiting_after_the_deadline_without_retrying() {
    let f = Fixture::new();
    f.ensure();
    let starts = Arc::new(AtomicUsize::new(0));
    let (codex, _, _, _) = server({
        let starts = Arc::clone(&starts);
        move |_, _| {
            starts.fetch_add(1, Ordering::SeqCst);
            Ok(json!({ "started": true }))
        }
    });
    f.host.push_server(codex);
    let error = f
        .setup
        .prepare_account_codex_sandbox(&f.account(Provider::Codex), Duration::from_millis(5))
        .await
        .unwrap_err();
    assert!(
        error.message.to_lowercase().contains("timed out"),
        "{}",
        error.message
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

// describe("sign-in")

type Done = Arc<Mutex<Vec<(bool, String)>>>;

fn done() -> (Done, Arc<dyn Fn(bool, String) + Send + Sync>) {
    let calls = Done::default();
    let record = Arc::clone(&calls);
    (
        calls,
        Arc::new(move |ok, attempt| record.lock().unwrap().push((ok, attempt))),
    )
}

fn claude_status(email: Option<&str>) -> CliOutput {
    output(
        Some(if email.is_some() { 0 } else { 1 }),
        &json!({ "loggedIn": email.is_some(), "email": email }).to_string(),
    )
}

fn use_claude_status(host: &Host, email: Option<&'static str>) {
    host.set_run(move |cmd, args| {
        if args[0] == "auth" {
            claude_status(email)
        } else {
            successful(cmd, args)
        }
    });
}

#[tokio::test]
async fn runs_hidden_native_claude_callback_login_and_accepts_successful_re_authentication_of_the_same_account()
 {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, Some("same@example.invalid"));
    let (calls, on_done) = done();
    let attempt = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap();
    let (cmd, args, options) = f.host.launches.lock().unwrap()[0].clone();
    assert_eq!(
        (cmd.file.as_str(), args, options.cwd),
        (
            "claude",
            vec!["auth".to_string(), "login".into(), "--claudeai".into()],
            f.account(Provider::Claude).folder
        )
    );
    assert_eq!(attempt.url, None);
    assert!(calls.lock().unwrap().is_empty());
    f.host.end_child(0, 0);
    assert_eq!(*calls.lock().unwrap(), vec![(true, attempt.attempt)]);
}

#[tokio::test]
async fn keeps_the_hidden_claude_login_stdin_open_until_the_login_ends() {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, Some("same@example.invalid"));
    let (calls, on_done) = done();
    let attempt = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap();
    providers_settle().await;
    assert!(
        calls.lock().unwrap().is_empty(),
        "the login saw end-of-input"
    );
    f.host.end_child(0, 0);
    assert_eq!(*calls.lock().unwrap(), vec![(true, attempt.attempt)]);
}

#[tokio::test]
async fn does_not_expose_claudes_manual_code_url_or_raw_auth_diagnostics() {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, None);
    let (calls, on_done) = done();
    let attempt = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap();
    let mut child = f.host.children.lock().unwrap().remove(0);
    child.stdout.write_all(b"If the browser didn't open, visit: https://claude.ai/oauth/authorize?secret=synthetic\nPaste code here if prompted > ").await.unwrap();
    child
        .stderr
        .write_all(b"private authentication diagnostics")
        .await
        .unwrap();
    assert_eq!(attempt.url, None);
    assert!(f.host.opened.lock().unwrap().is_empty());
    child.exit.fire(CliExit::Exited {
        code: Some(1),
        signal: None,
    });
    assert_eq!(*calls.lock().unwrap(), vec![(false, attempt.attempt)]);
}

#[tokio::test(start_paused = true)]
async fn ends_when_the_hidden_claude_process_fails_without_polling_the_old_account() {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, Some("old@example.com"));
    let (calls, on_done) = done();
    f.setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap();
    f.host.end_child(0, 1);
    tokio::time::sleep(Duration::from_millis(6000)).await;
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].0);
    assert_eq!(f.host.runs(), 1);
}

#[tokio::test(start_paused = true)]
async fn ends_claude_sign_in_at_the_deadline() {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, Some("old@example.com"));
    let (calls, on_done) = done();
    f.setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(10 * 60)).await;
    providers_settle().await;
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].0);
}

#[tokio::test]
async fn reuses_a_running_hidden_claude_login_and_starts_fresh_after_it_closes() {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, Some("old@example.com"));
    let (first_done, on_first) = done();
    let (repeat_done, on_repeat) = done();
    let first = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_first)
        .await
        .unwrap();
    let repeat = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_repeat)
        .await
        .unwrap();
    assert_eq!(repeat, first);
    assert_eq!((f.host.launched(), f.host.runs()), (1, 1));
    f.host.end_child(0, 0);
    assert_eq!(
        *first_done.lock().unwrap(),
        vec![(true, first.attempt.clone())]
    );
    assert!(repeat_done.lock().unwrap().is_empty());
    let second = f
        .setup
        .sign_in(&f.db, Provider::Claude, done().1)
        .await
        .unwrap();
    assert_eq!(second.url, None);
    assert_ne!(
        second.attempt, first.attempt,
        "a new sign-in is a new attempt"
    );
    assert_eq!(f.host.launched(), 2);
    f.host.end_child(1, 0);
}

#[tokio::test]
async fn rejects_a_second_claude_request_while_the_initial_status_check_is_pending() {
    let f = Fixture::new();
    f.ensure();
    let (release, released) = tokio::sync::oneshot::channel::<CliOutput>();
    let released = Arc::new(Mutex::new(Some(released)));
    f.host.run_once(Arc::new(move |_, _| {
        let released = released.lock().unwrap().take().unwrap();
        Box::pin(async move { released.await.unwrap() })
    }));
    let first = tokio::spawn({
        let setup = f.setup.clone();
        let account = f.account(Provider::Claude);
        async move { setup.sign_in_account(&account, done().1).await }
    });
    providers_settle().await;
    let busy = f
        .setup
        .sign_in(&f.db, Provider::Claude, done().1)
        .await
        .unwrap_err();
    assert_eq!(busy.code, ErrorCode::Busy);
    release.send(claude_status(None)).unwrap();
    first.await.unwrap().unwrap();
    assert_eq!(f.host.launched(), 1);
    f.host.end_child(0, 0);
}

fn login_server(rpcs: &Arc<Mutex<Vec<Arc<JsonRpcLines>>>>) -> CodexServer {
    let (codex, _, rpc, _) =
        server(|_, _| Ok(json!({ "authUrl": "https://example.com/login", "loginId": "login-1" })));
    rpcs.lock().unwrap().push(rpc);
    codex
}

fn complete(rpc: &JsonRpcLines, login: &str, success: bool) {
    rpc.handle_line(&json!({ "method": "account/login/completed", "params": { "success": success, "loginId": login } }).to_string());
}

#[tokio::test]
async fn reopens_the_same_codex_sign_in_page_without_a_second_app_server_then_starts_fresh_after_completion()
 {
    let f = Fixture::new();
    f.ensure();
    let rpcs = Arc::new(Mutex::new(Vec::new()));
    f.host.push_server(login_server(&rpcs));
    let (calls, on_done) = done();
    let first = f
        .setup
        .sign_in(&f.db, Provider::Codex, on_done)
        .await
        .unwrap();
    let (repeat_calls, on_repeat) = done();
    let repeat = f
        .setup
        .sign_in(&f.db, Provider::Codex, on_repeat)
        .await
        .unwrap();
    assert_eq!(
        (repeat.url.as_deref(), repeat.attempt.as_str()),
        (Some("https://example.com/login"), first.attempt.as_str())
    );
    assert_eq!(f.host.server_starts.lock().unwrap().len(), 1);
    assert_eq!(
        *f.host.opened.lock().unwrap(),
        ["https://example.com/login", "https://example.com/login"]
    );
    complete(&rpcs.lock().unwrap()[0], "login-1", true);
    assert_eq!(*calls.lock().unwrap(), vec![(true, first.attempt.clone())]);
    assert!(repeat_calls.lock().unwrap().is_empty());
    f.host.push_server(login_server(&rpcs));
    let fresh = f
        .setup
        .sign_in(&f.db, Provider::Codex, done().1)
        .await
        .unwrap();
    assert_ne!(fresh.attempt, first.attempt);
    assert_eq!(f.host.server_starts.lock().unwrap().len(), 2);
    assert_eq!(f.host.opened.lock().unwrap().len(), 3);
    complete(&rpcs.lock().unwrap()[1], "login-1", true);
}

#[tokio::test]
async fn rejects_a_second_codex_request_while_its_app_server_is_starting() {
    let f = Fixture::new();
    f.ensure();
    let (release, released) = tokio::sync::oneshot::channel::<CodexServer>();
    f.host
        .servers
        .lock()
        .unwrap()
        .push_back(Box::pin(async move { Ok(released.await.unwrap()) }));
    let first = tokio::spawn({
        let setup = f.setup.clone();
        let account = f.account(Provider::Codex);
        async move { setup.sign_in_account(&account, done().1).await }
    });
    providers_settle().await;
    assert_eq!(
        f.setup
            .sign_in(&f.db, Provider::Codex, done().1)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert_eq!(f.host.server_starts.lock().unwrap().len(), 1);
    let rpcs = Arc::new(Mutex::new(Vec::new()));
    assert!(release.send(login_server(&rpcs)).is_ok());
    first.await.unwrap().unwrap();
    complete(&rpcs.lock().unwrap()[0], "login-1", true);
}

#[tokio::test]
async fn clears_the_sign_in_guard_after_a_launcher_error() {
    let f = Fixture::new();
    f.ensure();
    use_claude_status(&f.host, None);
    *f.host.fail_launch.lock().unwrap() = true;
    let (calls, on_done) = done();
    let error = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap_err();
    assert!(
        error.message.contains("Hidden launcher unavailable"),
        "{}",
        error.message
    );
    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].0);
    assert_eq!(
        f.setup
            .sign_in(&f.db, Provider::Claude, done().1)
            .await
            .unwrap()
            .url,
        None
    );
}

#[tokio::test]
async fn explicit_logout_uses_only_the_selected_accounts_environment() {
    for provider in [Provider::Claude, Provider::Codex] {
        let f = Fixture::new();
        f.ensure();
        f.host.set_run(|_, _| output(Some(0), ""));
        let account = f.account(provider);
        f.setup.logout_account(&account).await.unwrap();
        let (cmd, args, env) = f.host.runs.lock().unwrap().last().cloned().unwrap();
        let (file, expected_args, key) = if provider == Provider::Claude {
            ("claude", vec!["auth", "logout"], "CLAUDE_CONFIG_DIR")
        } else {
            ("codex", vec!["logout"], "CODEX_HOME")
        };
        assert_eq!(
            (cmd.file.as_str(), args),
            (
                file,
                expected_args
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(env.get(key), Some(&account.folder));
        f.host.set_run(|_, _| CliOutput {
            code: Some(1),
            stdout: "private payload".into(),
            stderr: "private payload".into(),
        });
        assert!(
            f.setup
                .logout_account(&account)
                .await
                .unwrap_err()
                .message
                .contains("kept")
        );
    }
}

#[tokio::test]
async fn cancels_only_the_matching_hidden_claude_sign_in_and_retains_an_uncontained_attempt_for_retry()
 {
    let f = Fixture::new();
    f.ensure();
    let (calls, on_done) = done();
    let account = f.account(Provider::Claude);
    let started = f
        .setup
        .sign_in(&f.db, Provider::Claude, on_done)
        .await
        .unwrap();
    assert!(
        f.setup
            .cancel_account_sign_in(&account, "stale")
            .await
            .unwrap_err()
            .message
            .contains("no longer active")
    );
    *f.host.fail_kill_once.lock().unwrap() = true;
    assert!(
        f.setup
            .cancel_account_sign_in(&account, &started.attempt)
            .await
            .unwrap_err()
            .message
            .contains("Containment failed")
    );
    assert!(calls.lock().unwrap().is_empty());
    f.setup
        .cancel_account_sign_in(&account, &started.attempt)
        .await
        .unwrap();
    assert_eq!(*f.host.kills.lock().unwrap(), [12345, 12345]);
    assert_eq!(*calls.lock().unwrap(), vec![(false, started.attempt)]);
}

#[tokio::test]
async fn correlates_codex_early_completion_and_ignores_a_stale_login_id() {
    let f = Fixture::new();
    f.ensure();
    // Both completions arrive before login/start answers: the stale one first, then the current one.
    let (codex, closes, _, _) = server_with(|rpc, _, _| {
        complete(rpc, "old", true);
        complete(rpc, "current", true);
        Ok(json!({ "loginId": "current", "authUrl": "https://example.invalid/login" }))
    });
    f.host.push_server(codex);
    let (calls, on_done) = done();
    let result = f
        .setup
        .sign_in(&f.db, Provider::Codex, on_done)
        .await
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), vec![(true, result.attempt)]);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(f.host.opened.lock().unwrap().is_empty());
}
