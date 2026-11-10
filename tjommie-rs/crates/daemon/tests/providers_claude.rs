//! Port of `packages/daemon/src/providers/claude.test.ts`. Fake children stand in for the CLI; no real Claude runs.

mod providers_fakes;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use daemon::cli::CliHost;
use daemon::providers::claude::{ClaudeSession, SessionFiles, claude_args, claude_mode};
use daemon::providers::{ApiKey, McpServer, ProviderEvent, ProviderSession, StartOptions, Usage};
use protocol::model::Autonomy;
use providers_fakes::{FakeHost, command, drain, fake_child, settle, temp_root};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

fn base(cwd: &Path) -> StartOptions {
    StartOptions {
        cwd: cwd.to_string_lossy().into_owned(),
        extra_dirs: vec![],
        model: "sonnet".into(),
        instructions: "Be brief.".into(),
        autonomy: Autonomy::Folder,
        codex_sandbox_ready: false,
        allow_rules: vec![],
        resume_id: None,
        resume_cost_usd: None,
        env: BTreeMap::new(),
        api_key: None,
        max_budget_usd: None,
        mcp_servers: vec![],
    }
}

fn files() -> SessionFiles {
    SessionFiles {
        instructions: "instructions.txt".into(),
        mcp: "mcp.json".into(),
    }
}

fn arg_after(args: &[String], flag: &str) -> String {
    args[args.iter().position(|arg| arg == flag).expect(flag) + 1].clone()
}

fn session(host: &Arc<FakeHost>, options: StartOptions) -> ClaudeSession {
    let host: Arc<dyn CliHost> = host.clone();
    ClaudeSession::new(host, command("claude"), options)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

// describe("claudeArgs")

#[test]
fn maps_autonomy_to_claudes_permission_modes() {
    assert_eq!(
        [
            claude_mode(Autonomy::Ask),
            claude_mode(Autonomy::Folder),
            claude_mode(Autonomy::Full)
        ],
        ["manual", "acceptEdits", "bypassPermissions"]
    );
}

#[test]
fn starts_a_headless_stream_json_session_with_approvals_over_stdio_and_no_outside_mcp_servers() {
    let root = temp_root();
    assert_eq!(
        claude_args(&base(root.path()), &files()).unwrap(),
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "acceptEdits",
            "--model",
            "sonnet",
            "--append-system-prompt-file",
            "instructions.txt",
            "--strict-mcp-config",
        ]
    );
}

#[test]
fn adds_extra_folders_the_session_to_resume_and_saved_rules_last() {
    let root = temp_root();
    let options = StartOptions {
        extra_dirs: vec!["C:\\home".into()],
        resume_id: Some("abc".into()),
        allow_rules: vec!["Write".into(), "Bash(npm test *)".into()],
        ..base(root.path())
    };
    let args = claude_args(&options, &files()).unwrap();
    assert_eq!(
        args[args.len() - 7..],
        [
            "--add-dir",
            "C:\\home",
            "--resume",
            "abc",
            "--allowedTools",
            "Write",
            "Bash(npm test *)"
        ]
    );
}

#[test]
fn attaches_mcp_servers_and_allows_the_tools_of_the_ones_marked_auto_allow() {
    let root = temp_root();
    let options = StartOptions {
        allow_rules: vec!["Write".into()],
        mcp_servers: vec![
            McpServer::Http {
                name: "tjommie".into(),
                url: "http://127.0.0.1:47100/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer t0k".into())]),
                auto_allow: true,
            },
            McpServer::Stdio {
                name: "browser".into(),
                command: "C:\\node.exe".into(),
                args: vec!["cli.js".into(), "--headless".into()],
                env: BTreeMap::new(),
                auto_allow: false,
            },
        ],
        ..base(root.path())
    };
    let args = claude_args(&options, &files()).unwrap();
    assert_eq!(arg_after(&args, "--mcp-config"), files().mcp);
    assert!(!args.join(" ").contains("Bearer t0k"));
    assert_eq!(
        args[args.len() - 3..],
        ["--allowedTools", "mcp__tjommie", "Write"]
    );
}

#[test]
fn passes_awkward_instructions_through_process_arguments_unchanged_argument_round_trip() {
    let root = temp_root();
    let instructions =
        "Say \"hi\" & 'bye'\n100% ^caret <tag> | pipe naïve ✓ C:\\Users\\Thabo Mokoena";
    let args = claude_args(
        &StartOptions {
            instructions: instructions.into(),
            ..base(root.path())
        },
        &files(),
    )
    .unwrap();
    let script = root.path().join("print-args.js");
    std::fs::write(
        &script,
        "process.stdout.write(JSON.stringify(process.argv.slice(2)))",
    )
    .unwrap();
    let out = daemon::spawn::command("node")
        .arg(&script)
        .args(&args)
        .output()
        .expect("node runs");
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&out.stdout).unwrap(),
        args
    );
}

// describe("ClaudeSession folder permissions")

/// TypeScript counts the whoami and icacls calls. This checks their effect: each session folder ends up with one
/// non-inherited ACE, full control for the user.
#[tokio::test]
async fn applies_a_private_acl_to_every_new_session_folder() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let mut fakes = Vec::new();
    let mut sessions = Vec::new();
    for pid in [1, 2] {
        let (child, fake) = fake_child(pid);
        host.push_child(child);
        fakes.push(fake);
        let claude = session(&host, base(root.path()));
        claude.start().await.unwrap();
        sessions.push(claude);
    }
    for (_, args, _) in host.launches.lock().unwrap().iter() {
        let folder = PathBuf::from(arg_after(args, "--append-system-prompt-file"))
            .parent()
            .unwrap()
            .to_path_buf();
        let acl = daemon::spawn::command("icacls")
            .arg(&folder)
            .output()
            .unwrap();
        let acl = String::from_utf8_lossy(&acl.stdout);
        assert!(acl.contains("(OI)(CI)(F)") && !acl.contains("(I)"), "{acl}");
        assert_eq!(acl.matches(":(").count(), 1, "{acl}");
    }
    sessions.iter().for_each(ProviderSession::close);
    fakes.iter().for_each(|fake| fake.exit_with(Some(0), None));
}

// describe("ClaudeSession activity")

#[tokio::test]
async fn emits_no_activity_for_tool_loading_and_emits_a_plain_label_for_the_selected_tool() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    let mut events = claude.subscribe();
    claude.start().await.unwrap();
    let assistant = |id: &str, name: &str, input: Value| {
        json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } }).to_string()
    };
    fake.print(&assistant(
        "search-1",
        "ToolSearch",
        json!({ "query": "select:mcp__tjommie__add_routine" }),
    ))
    .await;
    assert_eq!(drain(&mut events), vec![]);

    fake.print(&assistant(
        "routine-1",
        "mcp__tjommie__add_routine",
        json!({ "name": "Morning check" }),
    ))
    .await;
    assert_eq!(
        drain(&mut events),
        vec![ProviderEvent::Tool {
            id: "routine-1".into(),
            name: "Proposed a routine: Morning check".into(),
            summary: "Proposed a routine: Morning check".into(),
        }]
    );
    fake.exit_with(Some(0), None);
}

// describe("ClaudeSession cost accounting")

fn usages(events: Vec<ProviderEvent>) -> Vec<Usage> {
    events
        .into_iter()
        .filter_map(|event| {
            if let ProviderEvent::Usage(usage) = event {
                Some(usage)
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test]
async fn reports_only_new_cost() {
    struct Case {
        name: &'static str,
        resume_id: Option<&'static str>,
        resume_cost_usd: Option<f64>,
        total: f64,
        expected: Option<f64>,
        init_session_id: Option<&'static str>,
    }
    for case in [
        Case {
            name: "a fresh session",
            resume_id: None,
            resume_cost_usd: None,
            total: 0.0140522,
            expected: Some(0.0140522),
            init_session_id: None,
        },
        Case {
            name: "a fresh session with an unrelated baseline",
            resume_id: None,
            resume_cost_usd: Some(0.0169623),
            total: 0.0140522,
            expected: Some(0.0140522),
            init_session_id: None,
        },
        Case {
            name: "a resumed session with its saved baseline",
            resume_id: Some("saved"),
            resume_cost_usd: Some(0.0169623),
            total: 0.0198706,
            expected: Some(0.0029083),
            init_session_id: None,
        },
        // TypeScript's undefined and null baselines are both None here.
        Case {
            name: "a legacy resumed session",
            resume_id: Some("saved"),
            resume_cost_usd: None,
            total: 0.0198706,
            expected: None,
            init_session_id: None,
        },
        Case {
            name: "a replacement CLI session",
            resume_id: Some("saved"),
            resume_cost_usd: Some(0.0169623),
            total: 0.0140522,
            expected: Some(0.0140522),
            init_session_id: Some("replacement"),
        },
    ] {
        let host = Arc::new(FakeHost::default());
        let root = temp_root();
        let (child, mut fake) = fake_child(4321);
        host.push_child(child);
        let options = StartOptions {
            resume_id: case.resume_id.map(str::to_string),
            resume_cost_usd: case.resume_cost_usd,
            ..base(root.path())
        };
        let claude = session(&host, options);
        let mut events = claude.subscribe();
        claude.start().await.unwrap();
        let init = json!({ "type": "system", "subtype": "init", "session_id": case.init_session_id.or(case.resume_id).unwrap_or("fresh") }).to_string();

        fake.print(&init).await;
        fake.print(
            &json!({ "type": "result", "subtype": "success", "total_cost_usd": case.total })
                .to_string(),
        )
        .await;
        let first = usages(drain(&mut events));
        assert_eq!(first.len(), 1, "{}", case.name);
        match case.expected {
            None => assert_eq!(first[0].cost_usd, None, "{}", case.name),
            Some(expected) => assert!(
                (first[0].cost_usd.unwrap() - expected).abs() < 1e-10,
                "{}",
                case.name
            ),
        }
        assert_eq!(first[0].total_cost_usd, Some(case.total), "{}", case.name);

        fake.print(&init).await;
        fake.print(&json!({ "type": "result", "subtype": "success", "total_cost_usd": case.total + 0.003 }).to_string()).await;
        let second = usages(drain(&mut events));
        assert_eq!(second.len(), 1, "{}", case.name);
        assert!(
            (second[0].cost_usd.unwrap() - 0.003).abs() < 1e-10,
            "{}",
            case.name
        );
        assert_eq!(
            second[0].total_cost_usd,
            Some(case.total + 0.003),
            "{}",
            case.name
        );
        fake.exit_with(Some(0), None);
    }
}

// describe("ClaudeSession fallback kills")

#[tokio::test(start_paused = true)]
async fn cancels_the_delayed_close_kill_when_that_child_exits() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    claude.start().await.unwrap();
    claude.close();
    fake.exit_with(Some(0), None);
    tokio::time::sleep(Duration::from_millis(5000)).await;
    settle().await;
    assert_eq!(fake.kills(), 0);
}

/// The other half of the TypeScript test's timer count: close arms a 5-second tree kill.
#[tokio::test(start_paused = true)]
async fn kills_the_tree_when_the_child_outlives_close_by_five_seconds() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    claude.start().await.unwrap();
    claude.close();
    tokio::time::sleep(Duration::from_millis(5000)).await;
    settle().await;
    assert_eq!(fake.kills(), 1);
    fake.exit_with(Some(1), None);
}

#[tokio::test(start_paused = true)]
async fn cancels_the_delayed_interrupt_kill_when_that_child_exits() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    claude.start().await.unwrap();
    fake.print(
        &json!({ "type": "system", "subtype": "init", "session_id": "session" }).to_string(),
    )
    .await;
    claude.interrupt().await.unwrap();
    fake.exit_with(Some(0), None);
    tokio::time::sleep(Duration::from_millis(10_000)).await;
    settle().await;
    assert_eq!(fake.kills(), 0);
}

/// The other half of the TypeScript test's timer count: an interrupt arms a 10-second tree kill.
#[tokio::test(start_paused = true)]
async fn kills_the_tree_when_an_interrupted_turn_outlives_its_grace_period() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    let mut events = claude.subscribe();
    claude.start().await.unwrap();
    fake.print(
        &json!({ "type": "system", "subtype": "init", "session_id": "session" }).to_string(),
    )
    .await;
    claude.interrupt().await.unwrap();
    tokio::time::sleep(Duration::from_millis(9999)).await;
    settle().await;
    assert_eq!(fake.kills(), 0);
    tokio::time::sleep(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(fake.kills(), 1);
    fake.exit_with(Some(1), None); // the killed CLI's exit is intentional
    assert_eq!(
        drain(&mut events).last(),
        Some(&ProviderEvent::Exit {
            error: None,
            completed_turn: None
        })
    );
}

// describe("ClaudeSession lifecycle")

#[tokio::test]
async fn stores_instructions_and_mcp_config_in_private_files_and_removes_them_when_closing() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, fake) = fake_child(4321);
    host.push_launch(move |args| {
        let prompt_file = PathBuf::from(arg_after(args, "--append-system-prompt-file"));
        let owner = read_json(&prompt_file.parent().unwrap().join("owner.json"));
        assert_eq!(owner, json!({ "version": 1, "ownerPid": std::process::id(), "phase": "starting", "childPid": null }));
        child
    });
    let instructions = "Say \"hi\" & 'bye'\n100% ^caret <tag> | pipe";
    let options = StartOptions {
        instructions: instructions.into(),
        mcp_servers: vec![McpServer::Http {
            name: "test".into(),
            url: "http://127.0.0.1:1/mcp".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer test-only-value".into())]),
            auto_allow: true,
        }],
        ..base(root.path())
    };
    let claude = session(&host, options);
    claude.start().await.unwrap();
    let (_, args, _) = host.last_launch();
    let prompt_file = PathBuf::from(arg_after(&args, "--append-system-prompt-file"));
    let mcp_file = PathBuf::from(arg_after(&args, "--mcp-config"));
    assert_eq!(std::fs::read_to_string(&prompt_file).unwrap(), instructions);
    assert_eq!(
        read_json(&mcp_file)["mcpServers"]["test"]["headers"],
        json!({ "Authorization": "Bearer test-only-value" })
    );
    assert!(!args.join(" ").contains(instructions));
    assert!(!args.join(" ").contains("test-only-value"));
    assert_eq!(
        read_json(&prompt_file.parent().unwrap().join("owner.json")),
        json!({ "version": 1, "ownerPid": std::process::id(), "phase": "started", "childPid": fake.pid })
    );
    claude.close();
    assert!(!prompt_file.exists());
    assert!(!mcp_file.exists());
    fake.exit_with(Some(0), None);
}

#[tokio::test]
async fn keeps_managing_a_live_child_if_recording_its_pid_fails() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, fake) = fake_child(4321);
    let folder = Arc::new(std::sync::Mutex::new(PathBuf::new()));
    host.push_launch({
        let folder = Arc::clone(&folder);
        move |args| {
            let path = PathBuf::from(arg_after(args, "--append-system-prompt-file"))
                .parent()
                .unwrap()
                .to_path_buf();
            std::fs::create_dir(path.join("owner.json.tmp")).unwrap();
            *folder.lock().unwrap() = path;
            child
        }
    });
    let claude = session(&host, base(root.path()));
    // TypeScript also checks the error is logged once; this logs to stderr.
    assert_eq!(claude.start().await, Ok(()));
    assert_eq!(claude.pid(), Some(fake.pid));
    let folder = folder.lock().unwrap().clone();
    let owner = read_json(&folder.join("owner.json"));
    assert_eq!(
        (owner["phase"].clone(), owner["childPid"].clone()),
        (json!("starting"), Value::Null)
    );
    claude.close();
    fake.exit_with(Some(0), None);
    assert!(!folder.exists());
}

#[tokio::test]
async fn does_not_write_after_close_and_reports_the_intentional_exit_quietly() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    let mut events = claude.subscribe();
    claude.start().await.unwrap();
    claude.close();
    assert_eq!(claude.send("too late").await, Ok(()));
    settle().await;
    assert_eq!(
        fake.read_line().await,
        "",
        "stdin ended with nothing written"
    );
    assert_eq!(claude.steer("too late").await, Ok(false));

    fake.exit_with(None, Some("SIGTERM"));
    assert_eq!(
        drain(&mut events),
        vec![ProviderEvent::Exit {
            error: None,
            completed_turn: None
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn can_interrupt_immediately_after_send_before_claude_emits_init() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    claude.start().await.unwrap();
    claude.send("start").await.unwrap();
    claude.interrupt().await.unwrap();
    settle().await;

    assert!(fake.read_line().await.contains(r#""content":"start""#));
    assert!(fake.read_line().await.contains(r#""subtype":"interrupt""#));
    fake.exit_with(Some(0), None);
    tokio::time::sleep(Duration::from_millis(10_000)).await;
    settle().await;
    assert_eq!(fake.kills(), 0);
}

#[tokio::test]
async fn reports_the_signal_when_an_unexpected_exit_has_no_code() {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, fake) = fake_child(4321);
    host.push_child(child);
    let claude = session(&host, base(root.path()));
    let mut events = claude.subscribe();
    claude.start().await.unwrap();
    fake.exit_with(None, Some("SIGTERM"));
    assert_eq!(
        drain(&mut events),
        vec![ProviderEvent::Exit {
            error: Some("Claude exited with signal SIGTERM".into()),
            completed_turn: None
        }]
    );
}

// describe("Claude API accounts")

#[test]
fn passes_only_the_positive_remaining_local_allowance_as_a_provider_estimate_threshold() {
    let root = temp_root();
    let args = claude_args(
        &StartOptions {
            max_budget_usd: Some(0.25),
            ..base(root.path())
        },
        &files(),
    )
    .unwrap();
    assert!(args.contains(&"--max-budget-usd".to_string()));
    assert!(args.contains(&"0.25".to_string()));
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let error = claude_args(
            &StartOptions {
                max_budget_usd: Some(value),
                ..base(root.path())
            },
            &files(),
        )
        .unwrap_err();
        assert!(error.message().contains("remaining"), "{value}");
    }
}

#[tokio::test]
async fn keeps_api_authentication_out_of_argv_and_redacts_provider_errors_while_deduplicating_results()
 {
    let host = Arc::new(FakeHost::default());
    let root = temp_root();
    let (child, mut fake) = fake_child(4321);
    host.push_child(child);
    let options = StartOptions {
        api_key: Some(ApiKey("synthetic-private-key".into())),
        max_budget_usd: Some(1.0),
        ..base(root.path())
    };
    let claude = session(&host, options);
    let mut events = claude.subscribe();
    claude.start().await.unwrap();
    let (_, args, launch) = host.last_launch();
    assert!(!args.join(" ").contains("synthetic-private-key"));
    assert_eq!(
        launch.claude_api_key,
        Some(ApiKey("synthetic-private-key".into()))
    );

    let result = json!({ "type": "result", "uuid": "result-one", "subtype": "success", "total_cost_usd": 0.1 }).to_string();
    fake.print(&result).await;
    fake.print(&result).await;
    let mut seen = drain(&mut events);
    assert_eq!(
        seen.iter()
            .filter(|event| matches!(event, ProviderEvent::Usage(_)))
            .count(),
        1
    );

    fake.stderr
        .write_all(b"Rejected synthetic-private-key")
        .await
        .unwrap();
    settle().await;
    fake.exit_with(Some(1), None);
    seen.extend(drain(&mut events));
    assert!(!format!("{seen:?}").contains("synthetic-private-key"));
    assert_eq!(
        seen.last(),
        Some(&ProviderEvent::Exit {
            error: Some("Rejected [redacted]".into()),
            completed_turn: None
        })
    );
    claude.close();
}
