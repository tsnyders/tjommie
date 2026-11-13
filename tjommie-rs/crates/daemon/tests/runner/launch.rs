//! `cliSessionFactory`: it finds the CLI through its host (`find_cli`, TypeScript's `resolveCli`) and starts sessions that
//! launch through that host. Whether a launch goes through the Windows worker boundary is the host's choice (lane C's
//! `SystemCli::with_worker`, tested in `tests/cli_worker.rs`), so a double stands in for it here.

use std::collections::BTreeMap;

use super::*;
use daemon::cli::{BoxFuture as CliFuture, CliChild, CliCommand, CliHost, LaunchOptions};
use daemon::runner::cli_session_factory;
use protocol::model::{Autonomy, Provider};

/// Finds only `installed` CLIs, records each launch and refuses it, so nothing reaches a real Claude or Codex.
struct Host {
    installed: Vec<Provider>,
    launches: Recorded<String>,
}

impl CliHost for Host {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        _args: Vec<String>,
        _options: LaunchOptions,
    ) -> CliFuture<'a, std::io::Result<CliChild>> {
        Box::pin(async move {
            self.launches.lock().unwrap().push(cmd.file.clone());
            Err(std::io::Error::other("launch refused"))
        })
    }

    fn find_cli(&self, name: Provider) -> Option<CliCommand> {
        let file = if name == Provider::Claude {
            "claude.exe"
        } else {
            "codex.exe"
        };
        self.installed.contains(&name).then(|| CliCommand {
            file: file.into(),
            args: Vec::new(),
            codex_package: None,
        })
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
async fn reports_a_missing_cli_as_not_installed() {
    let h = harness();
    let claude = h.bot(json!({}));
    let codex = h.bot(json!({ "provider": "codex", "model": "gpt-5.6-sol" }));
    let launches: Recorded<String> = Arc::default();
    let factory = cli_session_factory(Arc::new(Host {
        installed: Vec::new(),
        launches: launches.clone(),
    }));
    assert_eq!(
        factory(&claude, start_options(".")).err().unwrap(),
        "Claude Code is not installed"
    );
    assert_eq!(
        factory(&codex, start_options(".")).err().unwrap(),
        "Codex is not installed"
    );
    assert_eq!(*recorded(&launches), Vec::<String>::new());
}

#[tokio::test]
async fn starts_each_providers_session_through_its_host() {
    let h = harness();
    let claude = h.bot(json!({}));
    let codex = h.bot(json!({ "provider": "codex", "model": "gpt-5.6-sol" }));
    let cwd = h.path("cwd");
    std::fs::create_dir(&cwd).unwrap();
    let launches: Recorded<String> = Arc::default();
    let host = Host {
        installed: vec![Provider::Claude, Provider::Codex],
        launches: launches.clone(),
    };
    let factory = cli_session_factory(Arc::new(host));
    for bot in [&claude, &codex] {
        let session = factory(bot, start_options(&cwd)).unwrap();
        assert!(session.start().await.is_err());
    }
    assert_eq!(*recorded(&launches), ["claude.exe", "codex.exe"]);
}
