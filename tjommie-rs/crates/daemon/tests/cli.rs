//! Port of `packages/daemon/src/cli.test.ts`. Process tests run Node, as the TypeScript ones do; nothing runs a real
//! Claude or Codex.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use daemon::cli::{
    CliCommand, CliHost, CodexPackage, Env, KillTreeError, LaunchOptions, Runtime, SystemCli,
    TaskkillReport, cli_env, confirm_tree_stopped, kill_tree_and_wait, node_env, open_url,
    system32,
};
use daemon::providers::ApiKey;
use protocol::model::Provider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap()
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

/// The Node that runs the JS-entry tests, from the test process's PATH.
fn node() -> String {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("node.exe"))
        .find(|file| file.is_file())
        .map(|file| text(&file))
        .expect("node.exe on PATH")
}

fn command(file: &str) -> CliCommand {
    CliCommand {
        file: file.into(),
        args: vec![],
        codex_package: None,
    }
}

fn win(arch: &'static str) -> Runtime {
    Runtime {
        platform: "win32",
        arch,
    }
}

// describe("resolveCli")

struct CodexInstall {
    bin: PathBuf,
    root: PathBuf,
    entry: PathBuf,
    native: PathBuf,
    vendor: PathBuf,
    triple: &'static str,
    _dir: tempfile::TempDir,
}

fn codex_install(arch: &str, layout: &str) -> CodexInstall {
    let temp = dir();
    // Resolving the npm install goes through the file system, so the daemon returns canonical paths. Build the expected ones
    // from the canonical folder: a hosted runner's TEMP is the 8.3 form of its user folder (RUNNER~1), which resolves to a
    // different spelling.
    let bin = common::canonical(temp.path());
    let root = bin.join("node_modules").join("@openai").join("codex");
    let entry = root.join("bin").join("codex.js");
    let triple = if arch == "x64" {
        "x86_64-pc-windows-msvc"
    } else {
        "aarch64-pc-windows-msvc"
    };
    let vendor = if layout == "legacy" {
        root.join("vendor")
    } else {
        let base = if layout == "nested" { &root } else { &bin };
        base.join("node_modules")
            .join("@openai")
            .join(format!("codex-win32-{arch}"))
            .join("vendor")
    };
    let native = vendor
        .join(triple)
        .join(if layout == "legacy" { "codex" } else { "bin" })
        .join("codex.exe");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::create_dir_all(native.parent().unwrap()).unwrap();
    std::fs::write(&entry, "").unwrap();
    std::fs::write(&native, "").unwrap();
    if layout != "legacy" {
        std::fs::write(vendor.parent().unwrap().join("package.json"), "{}").unwrap();
    }
    // The JS entry runs on the Node next to it on PATH (TypeScript runs it on its own Node).
    std::fs::write(bin.join("node.exe"), "").unwrap();
    CodexInstall {
        bin,
        root,
        entry,
        native,
        vendor,
        triple,
        _dir: temp,
    }
}

#[test]
fn runs_the_npm_native_windows_binary_without_its_console_opening_wrapper() {
    for arch in ["x64", "arm64"] {
        let install = codex_install(arch, "nested");
        assert_eq!(
            SystemCli::new().resolve_cli(
                Provider::Codex,
                &env(&[("Path", &text(&install.bin))]),
                win(arch)
            ),
            Some(CliCommand {
                file: text(&install.native),
                args: vec![],
                codex_package: Some(CodexPackage {
                    root: text(&install.root),
                    path_dir: None
                }),
            }),
            "{arch}"
        );
    }
}

#[test]
fn resolves_a_hoisted_optional_platform_package_relative_to_the_npm_entry() {
    let install = codex_install("x64", "hoisted");
    let resolved = SystemCli::new().resolve_cli(
        Provider::Codex,
        &env(&[("PATH", &text(&install.bin))]),
        win("x64"),
    );
    assert_eq!(
        resolved.map(|command| command.file),
        Some(text(&install.native))
    );
}

#[test]
fn supports_the_legacy_vendor_executable_and_its_bundled_helper_directory() {
    let install = codex_install("x64", "legacy");
    let path_dir = install.vendor.join(install.triple).join("path");
    std::fs::create_dir(&path_dir).unwrap();
    assert_eq!(
        SystemCli::new().resolve_cli(
            Provider::Codex,
            &env(&[("PATH", &text(&install.bin))]),
            win("x64")
        ),
        Some(CliCommand {
            file: text(&install.native),
            args: vec![],
            codex_package: Some(CodexPackage {
                root: text(&install.root),
                path_dir: Some(text(&path_dir))
            }),
        })
    );
}

#[test]
fn keeps_the_wrapper_for_unsupported_native_resolution() {
    for runtime in [
        Runtime {
            platform: "win32",
            arch: "ia32",
        },
        Runtime {
            platform: "linux",
            arch: "x64",
        },
    ] {
        let install = codex_install("x64", "nested");
        let wrapper = CliCommand {
            file: text(&install.bin.join("node.exe")),
            args: vec![text(&install.entry)],
            codex_package: None,
        };
        assert_eq!(
            SystemCli::new().resolve_cli(
                Provider::Codex,
                &env(&[("PATH", &text(&install.bin))]),
                runtime
            ),
            Some(wrapper)
        );
    }
}

#[test]
fn does_not_run_a_bundled_executable_for_the_wrong_architecture() {
    let install = codex_install("arm64", "nested");
    let wrapper = CliCommand {
        file: text(&install.bin.join("node.exe")),
        args: vec![text(&install.entry)],
        codex_package: None,
    };
    assert_eq!(
        SystemCli::new().resolve_cli(
            Provider::Codex,
            &env(&[("PATH", &text(&install.bin))]),
            win("x64")
        ),
        Some(wrapper)
    );
}

#[test]
fn prefers_a_native_executable_on_path() {
    let bin = dir();
    let exe = bin.path().join("codex.exe");
    std::fs::write(&exe, "").unwrap();
    assert_eq!(
        SystemCli::new().resolve_cli(
            Provider::Codex,
            &env(&[("PATH", &text(bin.path()))]),
            win("x64")
        ),
        Some(command(&text(&exe)))
    );
}

#[test]
fn ignores_relative_path_entries_so_the_lookup_never_depends_on_the_working_directory() {
    // In cargo's per-target temp folder, not the system one: a relative spelling needs the folder on the same drive as the
    // working directory, and a hosted runner checks out to D: but keeps TEMP on C:.
    let bin = tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    std::fs::write(bin.path().join("codex.exe"), "").unwrap();
    // The same folder, spelled relative to this process's current directory.
    let cwd = std::env::current_dir().unwrap();
    let normal = |path: &Path| {
        path.components()
            .filter(|part| matches!(part, std::path::Component::Normal(_)))
            .count()
    };
    let mut relative = PathBuf::new();
    (0..normal(&cwd)).for_each(|_| relative.push(".."));
    relative.extend(
        bin.path()
            .components()
            .filter(|part| matches!(part, std::path::Component::Normal(_))),
    );
    assert!(
        relative.is_relative() && relative.join("codex.exe").exists(),
        "needs the target folder on the working directory's drive: {relative:?} from {cwd:?}"
    );
    assert_eq!(
        SystemCli::new().resolve_cli(
            Provider::Codex,
            &env(&[("PATH", &format!(".;{}", text(&relative)))]),
            win("x64")
        ),
        None
    );
}

#[test]
fn runs_an_npm_installed_clis_js_entry_with_node_path_key_in_any_case() {
    let bin = dir();
    let entry = bin
        .path()
        .join("node_modules")
        .join("@openai")
        .join("codex")
        .join("bin")
        .join("codex.js");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, "").unwrap();
    std::fs::write(bin.path().join("node.exe"), "").unwrap();
    assert_eq!(
        SystemCli::new().resolve_cli(
            Provider::Codex,
            &env(&[("Path", &text(bin.path()))]),
            win("x64")
        ),
        Some(CliCommand {
            file: text(&bin.path().join("node.exe")),
            args: vec![text(&entry)],
            codex_package: None
        })
    );
}

#[test]
fn returns_none_when_the_cli_isnt_installed() {
    let bin = dir();
    assert_eq!(
        SystemCli::new().resolve_cli(
            Provider::Claude,
            &env(&[("PATH", &text(bin.path()))]),
            win("x64")
        ),
        None
    );
}

// describe("cliEnv")

#[test]
fn strips_endpoint_overrides_secret_suffixes_and_cloud_variables_while_preserving_ordinary_cli_environment()
 {
    let denied = [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_CUSTOM_HEADERS",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
        "gh_token",
        "NPM_SECRET",
        "DB_PASSWORD",
        "DB_PASSWD",
        "CREDENTIAL",
        "CREDENTIALS",
        "PRIVATE_KEY",
        "API_KEY",
        "ACCESS_KEY",
        "THING_KEY",
        "AWS_REGION",
        "azure_location",
        "GOOGLE_PROJECT",
        "GCP_PROJECT",
    ];
    let mut base = env(&[
        ("Path", "bin"),
        ("TEMP", "tmp"),
        ("USERPROFILE", "profile"),
        ("KEEP", "1"),
    ]);
    base.extend(
        denied
            .iter()
            .map(|name| (name.to_string(), "discard".to_string())),
    );
    assert_eq!(
        cli_env(&env(&[("CODEX_HOME", "account")]), &base),
        env(&[
            ("Path", "bin"),
            ("TEMP", "tmp"),
            ("USERPROFILE", "profile"),
            ("KEEP", "1"),
            ("CODEX_HOME", "account")
        ])
    );
}

#[test]
fn strips_inherited_keys_and_config_folders_case_insensitively_then_adds_extras() {
    let base = env(&[
        ("Path", "x"),
        ("anthropic_api_key", "sk"),
        ("CODEX_HOME", "h"),
        ("ClaudeCode", "1"),
        ("KEEP", "1"),
    ]);
    assert_eq!(
        cli_env(&env(&[("CLAUDE_CONFIG_DIR", "C:\\acct")]), &base),
        env(&[
            ("Path", "x"),
            ("KEEP", "1"),
            ("CLAUDE_CONFIG_DIR", "C:\\acct")
        ])
    );
}

#[test]
fn removes_daemon_token_variables_before_building_a_cli_environment() {
    let token = daemon::paths::new_token();
    let built = cli_env(
        &env(&[("CODEX_HOME", "account"), ("TJOMMIE_DAEMON_TOKEN", &token)]),
        &env(&[
            ("PATH", "bin"),
            ("TJOMMIE_DAEMON_TOKEN", &token),
            ("tjommie_token", &token),
            ("SESSION_SECRET", &token),
        ]),
    );
    assert_eq!(built, env(&[("PATH", "bin"), ("CODEX_HOME", "account")]));
    assert!(!built.values().any(|value| *value == token));
}

// describe("runCli")

/// TypeScript checks the spawn call for status and session launches; both build their environment with nodeEnv.
#[test]
fn preserves_the_native_npm_environment_without_inherited_managed_markers_or_duplicate_path_keys() {
    let cmd = CliCommand {
        file: "C:\\native\\codex.exe".into(),
        args: vec![],
        codex_package: Some(CodexPackage {
            root: "C:\\npm\\codex".into(),
            path_dir: Some("C:\\npm\\vendor\\path".into()),
        }),
    };
    let given = env(&[
        ("Path", "C:\\tools"),
        ("CODEX_HOME", "C:\\owned-account"),
        ("CODEX_MANAGED_PACKAGE_ROOT", "unrelated-install"),
        ("codex_managed_by_bun", "1"),
        ("CODEX_MANAGED_BY_PNPM", "1"),
        ("OPENAI_API_KEY", "secret"),
    ]);
    assert_eq!(
        node_env(&cmd, &given),
        env(&[
            ("PATH", "C:\\npm\\vendor\\path;C:\\tools"),
            ("CODEX_HOME", "C:\\owned-account"),
            ("CODEX_MANAGED_PACKAGE_ROOT", "C:\\npm\\codex"),
            ("CODEX_MANAGED_BY_NPM", "1"),
        ])
    );
}

#[tokio::test]
async fn captures_stdout_and_the_exit_code() {
    let cmd = command(&node());
    let out = SystemCli::new()
        .run_cli(
            &cmd,
            &["-e".into(), "console.log('hi'); process.exit(3)".into()],
            &SystemCli::new().cli_env(&Env::new()),
            Duration::from_secs(20),
        )
        .await
        .unwrap();
    assert_eq!(out.code, Some(3));
    assert_eq!(out.stdout.trim(), "hi");
}

#[tokio::test]
async fn gives_a_session_cli_its_own_stdin_pipe() {
    let token = daemon::paths::new_token();
    let script = "let input = ''; process.stdin.on('data', chunk => input += chunk); process.stdin.on('end', () => process.stdout.write(JSON.stringify({ input, hasToken: 'TJOMMIE_DAEMON_TOKEN' in process.env })));";
    let mut child = SystemCli::new()
        .launch(
            &command(&node()),
            vec!["-e".into(), script.into()],
            LaunchOptions {
                cwd: text(&std::env::current_dir().unwrap()),
                env: env(&[
                    ("TJOMMIE_DAEMON_TOKEN", &token),
                    ("SystemRoot", &std::env::var("SystemRoot").unwrap()),
                ]),
                claude_api_key: None,
            },
        )
        .await
        .unwrap();
    child.stdin.write_all(b"CLI protocol only\n").await.unwrap();
    child.stdin.shutdown().await.unwrap();
    drop(child.stdin);
    let mut output = String::new();
    child.stdout.read_to_string(&mut output).await.unwrap();
    assert_eq!(
        child.exit.wait().await,
        daemon::cli::CliExit::Exited {
            code: Some(0),
            signal: None
        }
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({ "input": "CLI protocol only\n", "hasToken": false })
    );
    assert!(!output.contains(&token));
}

/// Windows argument quoting: the arguments a CLI receives are exactly the ones passed, whatever they contain. Node
/// (Claude's npm build) and Rust (codex.exe) both split the command line by the Microsoft C runtime rules.
#[tokio::test]
async fn passes_awkward_arguments_through_unchanged() {
    let long = "x".repeat(30_000);
    let args: Vec<String> = [
        "plain",
        "with space",
        "",
        " ",
        "\"quoted\"",
        "trailing\\",
        "trailing space\\",
        "C:\\Users\\Thabo Mokoena\\",
        "a\\\\b",
        "\\\"",
        "tab\there",
        "new\nline",
        "100% ^caret & | < > ( ) !bang!",
        "naïve ✓ 日本",
        &long,
    ]
    .map(String::from)
    .to_vec();
    let mut all = vec![
        "-e".to_string(),
        "process.stdout.write(JSON.stringify(process.argv.slice(1)))".into(),
    ];
    all.extend(args.iter().cloned());
    let out = SystemCli::new()
        .run_cli(
            &command(&node()),
            &all,
            &SystemCli::new().cli_env(&Env::new()),
            Duration::from_secs(20),
        )
        .await
        .unwrap();
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&out.stdout).unwrap(),
        args
    );
}

// describe("openUrl")

#[test]
fn refuses_anything_but_https() {
    assert!(
        open_url("file:///C:/Windows/System32/calc.exe")
            .unwrap_err()
            .contains("https")
    );
}

// describe("killTreeAndWait")

#[tokio::test]
async fn rejects_a_missing_process_identity() {
    assert!(
        kill_tree_and_wait(None)
            .await
            .unwrap_err()
            .to_string()
            .contains("process ID")
    );
}

const ROOT: u32 = 100;
const OWN: u32 = 5000;

fn success(pid: u32, parent: u32) -> String {
    format!(
        "SUCCESS: The process with PID {pid} (child process of PID {parent}) has been terminated.\r\n"
    )
}

fn failure(pid: u32, parent: u32) -> String {
    format!(
        "ERROR: The process with PID {pid} (child process of PID {parent}) could not be terminated.\r\nReason: The operation attempted is not supported.\r\r\n"
    )
}

fn stdout() -> String {
    success(200, 100) + &success(100, OWN)
}

fn stderr() -> String {
    failure(400, 300) + &failure(300, 200)
}

type Probes = std::rc::Rc<std::cell::RefCell<Vec<u32>>>;

/// Runs the verification for a nonzero taskkill report. `gone(pid)` is the process probe.
async fn taskkill(
    out: &str,
    err: &str,
    code: Option<i32>,
    gone: impl Fn(u32) -> bool + 'static,
) -> (Result<(), KillTreeError>, Probes) {
    let probes = Probes::default();
    let seen = probes.clone();
    let result = confirm_tree_stopped(ROOT, OWN, TaskkillReport::new(code, out, err), move |pid| {
        seen.borrow_mut().push(pid);
        gone(pid)
    })
    .await;
    (result, probes)
}

#[tokio::test(start_paused = true)]
async fn verifies_every_reported_pid_after_unsupported_sandbox_descendants() {
    for code in [128, 255] {
        let (result, probes) = taskkill(&stdout(), &stderr(), Some(code), |_| true).await;
        assert!(result.is_ok(), "{code}");
        let probed: std::collections::BTreeSet<u32> = probes.borrow().iter().copied().collect();
        assert_eq!(probed, [100, 200, 300, 400].into());
    }
}

#[tokio::test(start_paused = true)]
async fn waits_for_the_reported_descendants_to_disappear() {
    let started = tokio::time::Instant::now();
    let (result, _) = taskkill(&stdout(), &stderr(), Some(128), move |pid| {
        pid != 400 || started.elapsed() >= Duration::from_millis(250)
    })
    .await;
    assert!(result.is_ok());
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "it waited for PID 400"
    );
}

#[tokio::test(start_paused = true)]
async fn rejects_when_a_reported_pid_remains_even_though_the_root_is_gone() {
    // "running", "EPERM" and "unknown" all probe as not gone.
    let (result, _) = taskkill(&stdout(), &stderr(), Some(128), |pid| pid != 400).await;
    let report = result.unwrap_err().report.expect("the report is kept");
    assert_eq!(
        (report.code, report.stdout, report.stderr),
        (Some(128), stdout(), stderr())
    );
}

#[tokio::test(start_paused = true)]
async fn rejects_reports_that_do_not_prove_a_complete_tree() {
    let newlines = "\n".repeat(70_000);
    for (label, out, err) in [
        ("omitted root", success(200, 100), stderr()),
        ("unowned root", success(999, 900), String::new()),
        (
            "wrong root parent",
            success(200, 100) + &success(100, OWN + 1),
            stderr(),
        ),
        (
            "unknown line",
            stdout(),
            format!("{}Unknown termination error\n", stderr()),
        ),
        (
            "missing error reason",
            stdout(),
            "ERROR: The process with PID 400 (child process of PID 300) could not be terminated.\n"
                .into(),
        ),
        (
            "disconnected child",
            stdout() + &success(500, 600),
            stderr(),
        ),
        (
            "cyclic ancestry",
            stdout() + &success(500, 600) + &success(600, 500),
            stderr(),
        ),
        (
            "contradictory parent",
            stdout() + &success(200, 900),
            stderr(),
        ),
        (
            "already absent root without a tree",
            String::new(),
            "ERROR: The process \"100\" not found.\n".into(),
        ),
        (
            "failed root without enumeration proof",
            String::new(),
            failure(100, OWN),
        ),
        (
            "truncated stdout",
            format!("{}{newlines}", stdout()),
            stderr(),
        ),
        (
            "truncated stderr",
            stdout(),
            format!("{}{newlines}", stderr()),
        ),
    ] {
        let (result, probes) = taskkill(&out, &err, Some(128), |_| true).await;
        assert!(
            result.unwrap_err().to_string().contains("Could not stop"),
            "{label}"
        );
        assert!(probes.borrow().is_empty(), "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn does_not_recover_when_taskkill_itself_was_interrupted() {
    let (result, probes) = taskkill(&stdout(), &stderr(), None, |_| true).await;
    assert!(result.unwrap_err().to_string().contains("Could not stop"));
    assert!(probes.borrow().is_empty());
}

#[tokio::test(start_paused = true)]
async fn retains_a_bounded_diagnostic_prefix_and_explicit_truncation_flags() {
    let long = format!("{}{}", stdout(), "\n".repeat(70_000));
    let (result, _) = taskkill(&long, &stderr(), Some(128), |_| true).await;
    let error = result.unwrap_err();
    assert_eq!(error.pid, Some(ROOT));
    let report = error.report.unwrap();
    assert_eq!(report.stdout, long[..65_536]);
    assert_eq!(
        (
            report.stderr,
            report.stdout_truncated,
            report.stderr_truncated
        ),
        (stderr(), true, false)
    );
}

#[tokio::test(start_paused = true)]
async fn waits_for_the_owned_root_to_finish_exiting_before_probing_descendants() {
    let started = tokio::time::Instant::now();
    let (result, probes) = taskkill(&stdout(), &stderr(), Some(128), move |pid| {
        pid != 100 || started.elapsed() >= Duration::from_millis(250)
    })
    .await;
    assert!(result.is_ok());
    let probes = probes.borrow();
    let first_other = probes.iter().position(|pid| *pid != 100).unwrap();
    assert!(
        probes[..first_other].len() >= 3,
        "probed the root until it was gone: {probes:?}"
    );
    assert_eq!(
        probes
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        [100, 200, 300, 400].into()
    );
}

#[tokio::test(start_paused = true)]
async fn never_probes_descendants_while_the_owned_root_remains() {
    let (result, probes) = taskkill(&stdout(), &stderr(), Some(128), |_| false).await;
    assert!(result.unwrap_err().to_string().contains("Could not stop"));
    assert!(probes.borrow().iter().all(|pid| *pid == ROOT));
}

#[tokio::test(start_paused = true)]
async fn keeps_clean_exit_zero_behavior_unchanged() {
    let (result, probes) = taskkill("", "", Some(0), |_| false).await;
    assert!(result.is_ok());
    assert!(probes.borrow().is_empty());
}

#[tokio::test]
async fn terminates_an_owned_parent_and_descendant() {
    let script = "const { spawn } = require('node:child_process'); const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore', windowsHide: true }); console.log(child.pid); setInterval(() => {}, 1000);";
    let work = dir();
    let mut parent = SystemCli::new()
        .launch(
            &command(&node()),
            vec!["-e".into(), script.into()],
            LaunchOptions {
                cwd: text(work.path()),
                env: SystemCli::new().cli_env(&Env::new()),
                claude_api_key: None,
            },
        )
        .await
        .unwrap();
    let mut first = [0u8; 64];
    let read = parent.stdout.read(&mut first).await.unwrap();
    let descendant: u32 = String::from_utf8_lossy(&first[..read])
        .trim()
        .parse()
        .unwrap();
    // TypeScript also empties PATH: this taskkill runs by its full System32 path, never a PATH lookup.
    kill_tree_and_wait(parent.pid).await.unwrap();
    parent.exit.wait().await;
    assert!(daemon::cli::process_gone(parent.pid.unwrap()));
    assert!(daemon::cli::process_gone(descendant));
}

#[tokio::test]
async fn injects_only_an_explicitly_selected_claude_api_key_and_requests_child_tool_environment_scrubbing()
 {
    let script = "process.stdout.write(JSON.stringify({key:process.env.ANTHROPIC_API_KEY,scrub:process.env.CLAUDE_CODE_SUBPROCESS_ENV_SCRUB,managed:process.env.CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST}))";
    let work = dir();
    let base = SystemCli::new().cli_env_from(
        &Env::new(),
        &env(&[
            ("ANTHROPIC_API_KEY", "ambient-secret"),
            ("SystemRoot", &std::env::var("SystemRoot").unwrap()),
        ]),
    );
    let mut child = SystemCli::new()
        .launch(
            &command(&node()),
            vec!["-e".into(), script.into()],
            LaunchOptions {
                cwd: text(work.path()),
                env: base,
                claude_api_key: Some(ApiKey("synthetic-explicit-key".into())),
            },
        )
        .await
        .unwrap();
    let mut output = String::new();
    child.stdout.read_to_string(&mut output).await.unwrap();
    child.exit.wait().await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({ "key": "synthetic-explicit-key", "scrub": "1", "managed": "1" })
    );
}

#[test]
fn recognizes_a_worker_termination_by_type_never_by_its_message() {
    use daemon::cli::WorkerTermination;
    use daemon::providers::ProviderError;
    let marked = ProviderError::from(std::io::Error::other(WorkerTermination));
    assert!(marked.is_worker_termination());
    assert_eq!(marked.to_string(), WorkerTermination.to_string());
    let text = WorkerTermination.to_string();
    assert!(!ProviderError::Failed(text.clone()).is_worker_termination());
    assert!(!ProviderError::from(std::io::Error::other(text)).is_worker_termination());
}

// Git for Windows puts its own Unix `whoami` (which rejects `/user`) ahead of System32's on PATH, and any folder on PATH
// could hold a look-alike. The system tools the daemon runs are found in System32 by absolute path.
#[test]
fn finds_system_tools_in_system32_by_absolute_path() {
    for tool in ["whoami.exe", "icacls.exe", "taskkill.exe"] {
        let path = system32(tool);
        assert!(path.is_absolute(), "{path:?}");
        assert!(path.ends_with(Path::new("System32").join(tool)), "{path:?}");
        assert!(path.exists(), "{path:?}");
    }
}
