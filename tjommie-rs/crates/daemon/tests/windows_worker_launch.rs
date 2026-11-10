//! T2.3 port of `windows-worker-launch.test.ts`, plus native job and logon launches.
//! Not ported: the `vm` test of the bootstrap's partial reads. The bootstrap is the same JavaScript, pinned below,
//! and its TypeScript test keeps covering it.
#![cfg(windows)]

use daemon::spawn;
use daemon::win::cred::{self, Namespace};
use daemon::win::logon::{self, Job, NativeWorker};
use daemon::win::worker::{BOOTSTRAP, command_line, launch_environment, launch_frame};
use serde_json::{Value, json};
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn transports_long_windows_argv_privately_while_keeping_the_logon_command_below_its_limit() {
    let file = r"C:\ProgramData\Tjommie\Worker-123456789012345678901234\runtime\node.exe";
    let long = format!("/* synthetic */{}", "x".repeat(5000));
    let args = strings(&[
        "-e",
        &long,
        "",
        "quoted \"value\"",
        "line\nvalue",
        r"C:\trailing slash\",
        "English 🤖",
    ]);
    let frame = launch_frame(file, &args).unwrap();
    assert_eq!(
        u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize,
        frame.len() - 4
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&frame[4..]).unwrap(),
        json!({ "file": file, "args": args })
    );
    assert!(command_line(file, &args).unwrap().encode_utf16().count() > 1024);
    assert!(
        command_line(file, &strings(&["-e", BOOTSTRAP]))
            .unwrap()
            .encode_utf16()
            .count()
            < 1024
    );
}

#[test]
fn counts_utf16_code_units_and_windows_quoting_including_the_final_terminator() {
    let file = r"C:\runtime\node.exe";
    let remaining = 32766 - file.len() - 1;
    assert!(launch_frame(file, &["x".repeat(remaining)]).is_ok());
    assert!(
        launch_frame(file, &["x".repeat(remaining + 1)])
            .unwrap_err()
            .contains("launch limit")
    );
    assert_eq!(
        command_line(file, &strings(&["", "a \"b\"", "space\\", "🤖"])).unwrap(),
        format!(r#"{file} "" "a \"b\"" space\ 🤖"#)
    );
    assert!(
        launch_frame(file, &["🤖".repeat(remaining / 2 + 1)])
            .unwrap_err()
            .contains("launch limit")
    );
}

#[test]
fn rejects_nul_and_injected_environment_entries() {
    assert!(
        launch_frame("node.exe\0other", &[])
            .unwrap_err()
            .contains("launch input")
    );
    assert!(
        launch_frame("node.exe", &strings(&["hello\0world"]))
            .unwrap_err()
            .contains("launch input")
    );
    for (key, value) in [
        ("harmless", "x\0NODE_OPTIONS=--require bad"),
        ("bad\0key", "x"),
        ("bad=key", "x"),
        ("", "x"),
    ] {
        assert!(
            launch_environment(&[(key.into(), value.into())])
                .unwrap_err()
                .contains("launch input"),
            "{key:?}"
        );
    }
    let input = [
        ("NODE_OPTIONS", "--require synthetic"),
        ("node_path", "synthetic"),
        ("Node_Compile_Cache", "synthetic"),
        ("ELECTRON_RUN_AS_NODE", "1"),
        ("SYNTHETIC_ALLOWED", "yes"),
    ];
    let kept = launch_environment(&input.map(|(k, v)| (k.to_string(), v.to_string()))).unwrap();
    assert_eq!(kept, [("SYNTHETIC_ALLOWED".to_string(), "yes".to_string())]);
}

#[test]
fn the_bootstrap_is_the_typescript_one_byte_for_byte() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/daemon/src/windows-worker.ts"),
    )
    .unwrap();
    assert!(source.contains(&format!(
        "WINDOWS_WORKER_BOOTSTRAP = String.raw`{BOOTSTRAP}`;"
    )));
}

#[test]
fn runs_the_actual_fixed_bootstrap_with_long_argv_binary_stdin_inherited_stdio_and_exit_status() {
    let cwd = tempfile::TempDir::new().unwrap();
    let args = strings(&[
        &"x".repeat(6000),
        "",
        "quoted \"value\"",
        "line\nvalue",
        "🤖",
    ]);
    let input = [0u8, 255, 10, 13, 3, 4, 5];
    let script = "let b=[];process.stdin.on('data',v=>b.push(v));process.stdin.on('end',()=>{process.stdout.write(JSON.stringify({args:process.argv.slice(1),stdin:Buffer.concat(b).toString('base64'),cwd:process.cwd(),marker:process.env.TJOMMIE_SYNTHETIC}));process.stderr.write('synthetic-stderr');process.exitCode=17})";
    let node = which_node();
    let mut target = strings(&["-e", script, "--"]);
    target.extend(args.clone());
    let frame = launch_frame(node.to_str().unwrap(), &target).unwrap();
    let mut command = spawn::command(&node);
    command
        .args(["-e", BOOTSTRAP])
        .current_dir(cwd.path())
        .env_clear();
    command
        .env("SystemRoot", std::env::var("SystemRoot").unwrap())
        .env("TJOMMIE_SYNTHETIC", "yes");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for part in [&frame[..1], &frame[1..3], &frame[3..], &input[..]] {
        stdin.write_all(part).unwrap();
        stdin.flush().unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(17));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "synthetic-stderr");
    let seen: Value = serde_json::from_slice(&output.stdout).unwrap();
    use base64::Engine;
    let expected_cwd = cwd.path().to_str().unwrap();
    assert_eq!(
        seen,
        json!({ "args": args, "stdin": base64::engine::general_purpose::STANDARD.encode(input), "cwd": expected_cwd, "marker": "yes" })
    );
}

fn which_node() -> PathBuf {
    let output = spawn::command("where").arg("node").output().unwrap();
    PathBuf::from(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .trim(),
    )
}

/// `cmd` that starts a grandchild only after a second, so the job is assigned first.
fn process_tree() -> std::process::Child {
    let mut command = spawn::command("cmd");
    command.args(["/c", "ping -n 2 127.0.0.1 >NUL & ping -n 60 127.0.0.1 >NUL"]);
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn stopping_a_job_kills_the_whole_tree() {
    let owner = logon::current_user_sid().unwrap();
    let mut child = process_tree();
    let job = Job::bind(HANDLE(child.as_raw_handle()), None, &owner).unwrap();
    wait_until("the grandchild", || job.active().unwrap() >= 2);
    assert!(job.stop());
    assert_eq!(job.active().unwrap(), 0);
    assert_eq!(child.wait().unwrap().code(), Some(71));
}

#[test]
fn closing_the_job_kills_the_whole_tree() {
    let owner = logon::current_user_sid().unwrap();
    let mut child = process_tree();
    let job = Job::bind(HANDLE(child.as_raw_handle()), None, &owner).unwrap();
    wait_until("the grandchild", || job.active().unwrap() >= 2);
    drop(job);
    wait_until("the tree to die", || child.try_wait().unwrap().is_some());
}

/// Admin, CI-runner only: a disposable `TjWorker-<test ns8>` account runs a process tree inside
/// `Global\TjommieWorkerJob-…`; stopping the job kills the tree; the account is removed by its ns8.
#[test]
fn admin_worker_launch_runs_inside_a_global_job_and_dies_with_it() {
    if std::env::var("TJOMMIE_RS_ADMIN_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set TJOMMIE_RS_ADMIN_TESTS=1 on a disposable machine");
        return;
    }
    let root =
        std::env::temp_dir().join(format!("tjommie-rs-worker-test-{}", uuid::Uuid::new_v4()));
    assert!(
        !cred::is_production_root(&root),
        "refusing the production root"
    );
    let ns = Namespace::new(&root).unwrap();
    let name = format!("TjWorker-{}", &ns.short()[..8]);
    assert!(
        logon::account_sid(&name).unwrap().is_none(),
        "{name} already exists"
    );
    let password = logon::new_password().unwrap();
    let result = std::panic::catch_unwind(|| {
        let sid = logon::create_account(&name, &password, "Tjommie rs admin test worker").unwrap();
        let owner = logon::current_user_sid().unwrap();
        let system = std::env::var("SystemRoot").unwrap();
        let cmd = format!(r"{system}\System32\cmd.exe");
        let line = format!(r#""{cmd}" /c "ping -n 2 127.0.0.1 >NUL & ping -n 60 127.0.0.1 >NUL""#);
        let env = logon::environment_block(&[("SystemRoot".into(), system.clone())]);
        let worker = NativeWorker::start(
            &name,
            &password,
            &cmd,
            &line,
            Path::new(&system),
            &env,
            &owner,
            &sid,
        )
        .unwrap();
        logon::verify_token(worker.process(), &sid).unwrap();
        let job_name = format!(r"Global\TjommieWorkerJob-provider-{}", uuid::Uuid::new_v4());
        let job = Job::bind(worker.process(), Some(&job_name), &owner).unwrap();
        worker.resume().unwrap();
        wait_until("the grandchild", || job.active().unwrap() >= 2);
        assert!(job.stop());
        assert_eq!(worker.wait().unwrap(), 71);
    });
    logon::delete_account(&name).unwrap();
    assert!(logon::account_sid(&name).unwrap().is_none());
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
