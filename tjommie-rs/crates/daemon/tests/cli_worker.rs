//! Port of `packages/daemon/src/cli-worker.test.ts`: provider launches through a Windows worker boundary. The
//! boundary here is a synthetic fake; T2.3 implements the real one.
//!
//! "keeps normal work on the provider SID and selects only the registered review SID inside policy" is ported only as
//! far as `installWindowsWorker` and `workerCliExclusive` go. Its review-policy scope (`withWorkerPolicy`) comes with
//! M5's project review and T2.3.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::cli::{
    BoxFuture, ChildExit, CliChild, CliCommand, CliHost, Env, LaunchOptions, Runtime, SystemCli,
    WorkerBoundary, WorkerInstall, WorkerPurpose, WorkerTermination,
};
use daemon::providers::ApiKey;
use protocol::model::Provider;

type Spawn = (String, Vec<String>, String, Env);

struct FakeWorker {
    purpose: WorkerPurpose,
    /// `(workerSid, peerSid)`.
    sids: Option<(String, String)>,
    worker_root: PathBuf,
    runtime_root: PathBuf,
    spawns: Mutex<Vec<Spawn>>,
    grants: Mutex<Vec<PathBuf>>,
    terminate_next: Mutex<bool>,
}

impl WorkerBoundary for FakeWorker {
    fn required(&self) -> bool {
        true
    }

    fn purpose(&self) -> WorkerPurpose {
        self.purpose
    }

    fn sids(&self) -> Option<(String, String)> {
        self.sids.clone()
    }

    fn worker_root(&self) -> PathBuf {
        self.worker_root.clone()
    }

    fn runtime_root(&self) -> PathBuf {
        self.runtime_root.clone()
    }

    fn grant_directory<'a>(&'a self, folder: &'a Path) -> BoxFuture<'a, io::Result<()>> {
        self.grants.lock().unwrap().push(folder.to_path_buf());
        Box::pin(async { Ok(()) })
    }

    fn spawn<'a>(
        &'a self,
        file: &'a str,
        args: Vec<String>,
        cwd: &'a str,
        env: Env,
    ) -> BoxFuture<'a, io::Result<CliChild>> {
        self.spawns
            .lock()
            .unwrap()
            .push((file.into(), args, cwd.into(), env));
        let terminate = std::mem::take(&mut *self.terminate_next.lock().unwrap());
        Box::pin(async move {
            if terminate {
                return Err(io::Error::other(WorkerTermination));
            }
            let (stdin, _) = tokio::io::duplex(64);
            let (stdout, _) = tokio::io::duplex(64);
            let (stderr, _) = tokio::io::duplex(64);
            Ok(CliChild {
                pid: Some(7),
                stdin: Box::new(stdin),
                stdout: Box::new(stdout),
                stderr: Box::new(stderr),
                exit: ChildExit::new(),
                kill_tree: Arc::new(|| Box::pin(async { Ok(()) })),
                retirement: None,
            })
        })
    }
}

impl FakeWorker {
    fn new(purpose: WorkerPurpose, worker_root: PathBuf, runtime_root: PathBuf) -> Self {
        let (provider, review) = ("S-1-5-21-1-1001".to_string(), "S-1-5-21-1-1002".to_string());
        FakeWorker {
            purpose,
            sids: Some(match purpose {
                WorkerPurpose::Provider => (provider, review),
                WorkerPurpose::Review => (review, provider),
            }),
            worker_root,
            runtime_root,
            spawns: Mutex::default(),
            grants: Mutex::default(),
            terminate_next: Mutex::new(false),
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    worker: Arc<FakeWorker>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("tjommie test ñ-")
            .tempdir()
            .unwrap();
        for folder in ["worker", "review", "runtime", "owner"] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
        }
        let worker = Arc::new(FakeWorker::new(
            WorkerPurpose::Provider,
            root.path().join("worker"),
            root.path().join("runtime"),
        ));
        Fixture { root, worker }
    }

    fn cli(&self) -> SystemCli {
        let worker: Arc<dyn WorkerBoundary> = self.worker.clone();
        SystemCli::with_worker(worker)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }
}

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn command(file: &str) -> CliCommand {
    CliCommand {
        file: file.into(),
        args: vec![],
        codex_package: None,
    }
}

#[tokio::test]
async fn maps_accounts_into_fresh_worker_folders_routes_launches_and_grants_and_strips_inherited_keys()
 {
    let f = Fixture::new();
    let owner_account = text(&f.path("owner").join("accounts").join("codex-main"));
    let cli = f.cli();
    let account = cli.provider_account_folder(&owner_account);
    assert!(
        account.starts_with(&text(&f.path("worker").join("accounts"))),
        "{account}"
    );
    assert_eq!(cli.provider_account_folder(&account), account);
    assert_eq!(
        cli.worker_accounts_root(),
        Some(f.path("worker").join("accounts"))
    );

    let built = cli.cli_env_from(
        &env(&[("CODEX_HOME", &owner_account)]),
        &env(&[
            ("Path", "bin"),
            ("OPENAI_API_KEY", "synthetic-inherited-key"),
            ("TJOMMIE_DAEMON_TOKEN", "synthetic-host-token"),
        ]),
    );
    let codex = text(&f.path("runtime").join("codex.exe"));
    cli.launch(
        &command(&codex),
        vec!["--version".into()],
        LaunchOptions {
            cwd: owner_account.clone(),
            env: built,
            claude_api_key: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        *f.worker.spawns.lock().unwrap(),
        vec![(
            codex,
            vec!["--version".to_string()],
            account.clone(),
            env(&[("Path", "bin"), ("CODEX_HOME", &account)])
        )]
    );
    cli.prepare_provider_directories(vec![PathBuf::from(&account)])
        .await
        .unwrap();
    assert_eq!(
        *f.worker.grants.lock().unwrap(),
        vec![PathBuf::from(&account)]
    );

    // Without a boundary, account folders stay where they are.
    assert_eq!(
        SystemCli::new().provider_account_folder(&owner_account),
        owner_account
    );
}

#[test]
fn resolves_only_protected_runtime_files_instead_of_using_an_owner_path_command() {
    let f = Fixture::new();
    let owner_bin = f.path("bin");
    std::fs::create_dir(&owner_bin).unwrap();
    std::fs::write(owner_bin.join("claude.exe"), "synthetic").unwrap();
    let cli = f.cli();
    let runtime = Runtime {
        platform: "win32",
        arch: "x64",
    };
    assert_eq!(
        cli.resolve_cli(
            Provider::Claude,
            &env(&[("PATH", &text(&owner_bin))]),
            runtime
        ),
        None
    );
    let protected = f.path("runtime").join("claude.exe");
    std::fs::write(&protected, "synthetic").unwrap();
    assert_eq!(
        cli.resolve_cli(
            Provider::Claude,
            &env(&[("PATH", &text(&owner_bin))]),
            runtime
        ),
        Some(command(&text(&protected)))
    );
}

#[tokio::test]
async fn permits_only_an_explicit_private_claude_api_key_through_the_transport() {
    let f = Fixture::new();
    let cwd = text(f.root.path());
    f.cli()
        .launch(
            &command("protected.exe"),
            vec![],
            LaunchOptions {
                cwd: cwd.clone(),
                env: env(&[("ANTHROPIC_API_KEY", "inherited"), ("TOKEN", "discard")]),
                claude_api_key: Some(ApiKey("synthetic-explicit-key".into())),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        *f.worker.spawns.lock().unwrap(),
        vec![(
            "protected.exe".to_string(),
            vec![],
            cwd,
            env(&[
                ("ANTHROPIC_API_KEY", "synthetic-explicit-key"),
                ("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1"),
                ("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", "1"),
            ])
        )]
    );
}

#[tokio::test]
async fn does_not_turn_unknown_worker_termination_into_an_ordinary_failed_status_command() {
    let f = Fixture::new();
    *f.worker.terminate_next.lock().unwrap() = true;
    let error = f
        .cli()
        .run_cli(
            &command("protected.exe"),
            &["--version".into()],
            &Env::new(),
            Duration::from_secs(20),
        )
        .await
        .unwrap_err();
    assert!(daemon::cli::is_worker_termination(&error), "{error}");
}

// `installWindowsWorker` and `workerCliExclusive`.
#[tokio::test]
async fn installs_one_provider_and_review_pair_whose_exclusivity_ends_with_uninstall_or_a_direct_launch()
 {
    let f = Fixture::new();
    let worker: Arc<dyn WorkerBoundary> = f.worker.clone();
    let review: Arc<dyn WorkerBoundary> = Arc::new(FakeWorker::new(
        WorkerPurpose::Review,
        f.path("review"),
        f.path("runtime"),
    ));
    let install = Arc::new(WorkerInstall::default());
    let swapped = install
        .install(review.clone(), worker.clone())
        .err()
        .unwrap();
    assert!(
        swapped.contains("review_isolated_identity_unavailable"),
        "{swapped}"
    );
    let installed = install.install(worker.clone(), review.clone()).unwrap();
    assert!(install.exclusive(WorkerPurpose::Provider) && install.exclusive(WorkerPurpose::Review));
    let other = install
        .install(worker.clone(), review.clone())
        .err()
        .unwrap();
    assert!(
        other.contains("different Windows worker boundary"),
        "{other}"
    );
    // Launches through the worker keep the pair exclusive.
    let options = LaunchOptions {
        cwd: text(&f.path("worker")),
        env: Env::new(),
        claude_api_key: None,
    };
    f.cli()
        .reporting_to(install.clone())
        .launch(&command("protected.exe"), vec![], options)
        .await
        .unwrap();
    assert!(install.exclusive(WorkerPurpose::Provider));
    drop(installed);
    assert!(
        !install.exclusive(WorkerPurpose::Provider) && !install.exclusive(WorkerPurpose::Review)
    );
    let _again = install.install(worker, review).unwrap();
    assert!(install.exclusive(WorkerPurpose::Provider));
    // A CLI started outside the workers ends exclusivity for good, even one that never starts.
    let missing = command(&text(&f.path("missing\no-such-cli.exe")));
    let direct = SystemCli::new().reporting_to(install.clone());
    let output = direct
        .run_cli(&missing, &[], &Env::new(), Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(output.code, None);
    assert!(
        !install.exclusive(WorkerPurpose::Provider) && !install.exclusive(WorkerPurpose::Review)
    );
}

// `installWindowsWorker`'s identity checks: two different worker accounts, each the other's peer.
#[test]
fn refuses_a_pair_whose_worker_identities_are_not_each_others_peers() {
    let f = Fixture::new();
    let review = |sids: Option<(&str, &str)>| -> Arc<dyn WorkerBoundary> {
        let mut review =
            FakeWorker::new(WorkerPurpose::Review, f.path("review"), f.path("runtime"));
        review.sids = sids.map(|(sid, peer)| (sid.to_string(), peer.to_string()));
        Arc::new(review)
    };
    let worker: Arc<dyn WorkerBoundary> = f.worker.clone();
    let install = Arc::new(WorkerInstall::default());
    for wrong in [
        Some(("S-1-5-21-1-1001", "S-1-5-21-1-1001")), // the provider's own account
        Some(("S-1-5-21-1-1003", "S-1-5-21-1-1001")), // not the provider's peer
        Some(("S-1-5-21-1-1002", "S-1-5-21-1-1003")), // its peer is not the provider
        None,                                         // configuration unreadable
    ] {
        let refused = install
            .install(worker.clone(), review(wrong))
            .err()
            .unwrap();
        assert_eq!(refused, "review_isolated_identity_unavailable", "{wrong:?}");
        assert!(!install.exclusive(WorkerPurpose::Provider));
    }
    let _installed = install
        .install(worker, review(Some(("S-1-5-21-1-1002", "S-1-5-21-1-1001"))))
        .unwrap();
    assert!(install.exclusive(WorkerPurpose::Review));
}
