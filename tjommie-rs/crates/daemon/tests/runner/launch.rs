//! The launch seam: `cliSessionFactory`, and `launchCli`'s choice between the Windows worker boundary and a direct
//! launch (`cli.ts:153-154`). Lane B's `WorkerBoundary` implements [`WorkerIsolation`]; these are test doubles.

use std::collections::BTreeMap;

use super::*;
use daemon::cli::{BoxFuture as CliFuture, CliChild, CliCommand, CliHost, LaunchOptions};
use daemon::runner::{BoundaryHost, WorkerIsolation, cli_session_factory};
use protocol::model::{Autonomy, Provider};

/// Records each launch under `name` and refuses it, so a test sees which side was asked.
struct Recorder {
    name: &'static str,
    launches: Recorded<String>,
}

impl Recorder {
    fn refuse(&self, cmd: &CliCommand) -> std::io::Result<CliChild> {
        self.launches
            .lock()
            .unwrap()
            .push(format!("{}:{}", self.name, cmd.file));
        Err(std::io::Error::other(format!(
            "{} launch refused",
            self.name
        )))
    }
}

impl CliHost for Recorder {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> CliFuture<'a, std::io::Result<CliChild>> {
        Box::pin(async move { self.refuse(cmd) })
    }
}

struct Boundary {
    required: bool,
    recorder: Recorder,
}

impl WorkerIsolation for Boundary {
    fn required(&self) -> bool {
        self.required
    }
}

impl CliHost for Boundary {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> CliFuture<'a, std::io::Result<CliChild>> {
        Box::pin(async move { self.recorder.refuse(cmd) })
    }
}

fn host(launches: &Recorded<String>, boundary: Option<bool>) -> BoundaryHost {
    let direct = Arc::new(Recorder {
        name: "direct",
        launches: launches.clone(),
    });
    let boundary = boundary.map(|required| {
        Arc::new(Boundary {
            required,
            recorder: Recorder {
                name: "boundary",
                launches: launches.clone(),
            },
        }) as Arc<dyn WorkerIsolation>
    });
    BoundaryHost { direct, boundary }
}

fn launch_options() -> LaunchOptions {
    LaunchOptions {
        cwd: ".".into(),
        env: BTreeMap::new(),
        claude_api_key: None,
    }
}

#[tokio::test]
async fn launches_through_the_worker_boundary_only_when_isolation_requires_it() {
    let cmd = CliCommand {
        file: "claude.exe".into(),
        args: Vec::new(),
    };
    for (boundary, expected) in [
        (None, "direct"),
        (Some(false), "direct"),
        (Some(true), "boundary"),
    ] {
        let launches: Recorded<String> = Arc::default();
        let error = host(&launches, boundary)
            .launch(&cmd, Vec::new(), launch_options())
            .await
            .err()
            .unwrap();
        assert_eq!(error.to_string(), format!("{expected} launch refused"));
        assert_eq!(
            *recorded(&launches),
            [format!("{expected}:claude.exe")],
            "{boundary:?}"
        );
    }
}

fn start_options(cwd: &str) -> StartOptions {
    StartOptions {
        cwd: cwd.into(),
        extra_dirs: Vec::new(),
        model: "haiku".into(),
        instructions: String::new(),
        autonomy: Autonomy::Folder,
        codex_sandbox_ready: false,
        allow_rules: Vec::new(),
        resume_id: None,
        resume_cost_usd: None,
        env: BTreeMap::new(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers: Vec::new(),
    }
}

#[tokio::test]
async fn reports_a_missing_cli_as_not_installed_and_starts_installed_ones_through_the_host() {
    let h = harness();
    let claude = h.bot(json!({}));
    let codex = h.bot(json!({ "provider": "codex", "model": "gpt-5.6-sol" }));
    let cwd = h.path("cwd");
    std::fs::create_dir(&cwd).unwrap();
    let launches: Recorded<String> = Arc::default();
    let missing = cli_session_factory(Arc::new(host(&launches, Some(true))), Arc::new(|_| None));
    assert_eq!(
        missing(&claude, start_options(&cwd)).err().unwrap(),
        "Claude Code is not installed"
    );
    assert_eq!(
        missing(&codex, start_options(&cwd)).err().unwrap(),
        "Codex is not installed"
    );

    let resolve = |provider: Provider| {
        Some(CliCommand {
            file: format!(
                "{}.exe",
                if provider == Provider::Claude {
                    "claude"
                } else {
                    "codex"
                }
            ),
            args: Vec::new(),
        })
    };
    let factory = cli_session_factory(Arc::new(host(&launches, Some(true))), Arc::new(resolve));
    for bot in [&claude, &codex] {
        let session = factory(bot, start_options(&cwd)).unwrap();
        assert!(session.start().await.is_err());
    }
    assert_eq!(
        *recorded(&launches),
        ["boundary:claude.exe", "boundary:codex.exe"]
    );
}
