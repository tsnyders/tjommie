//! Admin diagnostics for the worker launch path, for finding out why a real launch cannot confirm a tree's stop.
//!
//! The acceptance check reaches the native launch only through the boundary, which poisons both identities for good
//! at the first unconfirmed stop. This test drives the lowest layer (`logon::launch`, the same call the boundary
//! makes) with small programs, one launch at a time, so every experiment starts clean. For each one it prints a
//! timeline: what the worker's job holds right after the launch, while it runs, right after the root process exits,
//! whether the job empties by itself, whether the output pipes reach end of file by themselves, and what the real
//! stop (`WorkerProcess::retire`) reports. It asserts nothing about Windows: it is a probe, and the printed timeline
//! is its result. Run it with `--nocapture`.
//!
//! Admin-gated: it creates two real accounts. No workflow runs it automatically; the opt-in `Worker acceptance
//! (admin, disposable runner)` workflow runs it before the acceptance test. Never run it on a PC with a real Tjommie
//! install.
#![cfg(windows)]

use daemon::win::cred::{self, Namespace};
use daemon::win::logon;
use daemon::win::worker::{Purpose, WorkerConfiguration, launch_frame, worker_name};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn which_node() -> PathBuf {
    let output = daemon::spawn::command("where")
        .arg("node")
        .output()
        .unwrap();
    PathBuf::from(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .trim(),
    )
}

/// A command's output as one line, for the facts at the top of the timeline.
fn fact(program: &str, args: &[&str]) -> String {
    match daemon::spawn::command(program).args(args).output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        Err(error) => format!("(could not run: {error})"),
    }
}

/// The saved worker password for `config`, read from the owner's Credential Manager. It is only ever passed to
/// `logon::launch`, never printed.
fn password(config: &WorkerConfiguration) -> String {
    let saved = cred::read(&config.credential_target).unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    value["password"].as_str().unwrap().to_owned()
}

/// One launch of the bootstrap under `config`'s worker, running `node.exe` with `args`, observed throughout.
fn experiment(label: &str, config: &WorkerConfiguration, args: &[&str]) {
    let started = Instant::now();
    let say = |text: String| {
        eprintln!(
            "[experiment {label} +{:>6}ms] {text}",
            started.elapsed().as_millis()
        )
    };
    let scratch = config.worker_root.join(format!("experiment-{label}"));
    fs::create_dir_all(&scratch).unwrap();
    let node = config.runtime_root.join("node.exe");
    let mut process = match logon::launch(config, &password(config), &scratch, &[]) {
        Ok(process) => process,
        Err((message, stopped)) => {
            say(format!(
                "LAUNCH FAILED (its tree confirmed stopped: {stopped}): {message}"
            ));
            return;
        }
    };
    say(format!(
        "launched, still suspended: process {}; job: {}",
        process.worker.pid(),
        process.job.describe()
    ));
    let frame = launch_frame(
        node.to_str().unwrap(),
        &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
    )
    .unwrap();
    // The frame is written while the worker is resumed (it can be larger than the pipe), and the output pipes are read
    // on their own threads so that a pipe that never reaches end of file shows up as a timeout and not as a hang.
    let mut stdin = process.worker.stdin().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&frame);
        stdin
    });
    let reader = |mut pipe: std::fs::File| {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
        receiver
    };
    let out = reader(process.worker.stdout().unwrap());
    let err = reader(process.worker.stderr().unwrap());
    let process = Arc::new(process);
    match process.worker.resume() {
        Ok(()) => say("resumed".into()),
        Err(error) => say(format!("RESUME FAILED: {error}")),
    }
    let (exit_sender, exit) = mpsc::channel();
    let waiting = process.clone();
    std::thread::spawn(move || {
        let _ = exit_sender.send(waiting.worker.wait());
    });

    let mut last = String::new();
    let code = loop {
        match exit.recv_timeout(Duration::from_millis(100)) {
            Ok(code) => break Some(code),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break None,
        }
        let now = process.job.describe();
        if now != last {
            say(format!("running; job: {now}"));
            last = now;
        }
        if started.elapsed() > Duration::from_secs(60) {
            say("TIMEOUT: the root process had not exited after 60s".into());
            break None;
        }
    };
    say(format!(
        "root process exited: {code:?}; job right after: {}",
        process.job.describe()
    ));

    // Does the job empty by itself, as it would if nothing outlived the root process?
    let settling = Instant::now();
    let emptied = loop {
        if process.job.active().unwrap_or(u32::MAX) == 0 {
            break true;
        }
        if settling.elapsed() > Duration::from_secs(5) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    say(format!(
        "after the root exited the job {} ({}ms); job: {}",
        if emptied {
            "emptied by itself"
        } else {
            "STILL HELD PROCESSES"
        },
        settling.elapsed().as_millis(),
        process.job.describe()
    ));
    for (name, pipe) in [("stdout", &out), ("stderr", &err)] {
        match pipe.recv_timeout(Duration::from_secs(2)) {
            Ok(bytes) => say(format!(
                "{name} reached end of file by itself: {}",
                String::from_utf8_lossy(&bytes[..bytes.len().min(600)])
            )),
            Err(_) => say(format!(
                "{name} did NOT reach end of file within 2s (something outside the root process still holds its write end)"
            )),
        }
    }
    let stopping = Instant::now();
    let settled = process.retire();
    say(format!(
        "retire() returned {settled} after {}ms; job: {}",
        stopping.elapsed().as_millis(),
        process.job.describe()
    ));
    let _ = writer.join();
    // After the stop every pipe must be at end of file.
    for (name, pipe) in [("stdout", out), ("stderr", err)] {
        if let Ok(bytes) = pipe.recv_timeout(Duration::from_secs(5)) {
            say(format!(
                "{name} after the stop ({} bytes): {}",
                bytes.len(),
                String::from_utf8_lossy(&bytes[..bytes.len().min(600)])
            ));
        }
    }
}

/// Admin, opt-in (`TJOMMIE_RS_ADMIN_TESTS=1`): see the file's header. It prints; the printed timeline is the result.
#[test]
#[ignore = "admin: needs elevation; run with TJOMMIE_RS_ADMIN_TESTS=1 cargo test -- --ignored"]
fn admin_worker_launch_experiments() {
    if std::env::var("TJOMMIE_RS_ADMIN_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set TJOMMIE_RS_ADMIN_TESTS=1 on a disposable machine");
        return;
    }
    let folder = TempDir::new().unwrap();
    let owner_root = folder.path().join("owner");
    assert!(
        !cred::is_production_root(&owner_root),
        "refusing the production root"
    );
    let ns = Namespace::new(&owner_root).unwrap();
    let names = [
        worker_name(&ns, Purpose::Provider),
        worker_name(&ns, Purpose::Review),
    ];
    let program_data = PathBuf::from(std::env::var_os("ProgramData").unwrap());
    let base = program_data.join(format!(r"Tjommie\Worker-{}", ns.short()));
    for name in &names {
        assert!(
            logon::account_sid(name).unwrap().is_none(),
            "{name} already exists"
        );
    }
    let root = owner_root.clone();
    let result = std::panic::catch_unwind(move || {
        eprintln!("[facts] {}", fact("cmd", &["/c", "ver"]));
        eprintln!("[facts] seclogon: {}", fact("sc", &["query", "seclogon"]));
        eprintln!(
            "[facts] node {}: {}",
            which_node().display(),
            fact(which_node().to_str().unwrap(), &["--version"])
        );
        logon::provision(&root).unwrap();
        fs::copy(which_node(), base.join("runtime").join("node.exe")).unwrap();
        let provider =
            WorkerConfiguration::load(&root.join("worker.json"), Purpose::Provider).unwrap();
        let review =
            WorkerConfiguration::load(&root.join("review-worker.json"), Purpose::Review).unwrap();
        eprintln!(
            "[facts] provider {} ({}), review {} ({})",
            provider.worker_name, provider.worker_sid, review.worker_name, review.worker_sid
        );

        // Nothing here is read from an environment value: the programs are fixed text, and use forward slashes.
        let trivial = ["-e", "process.stdout.write('hello from the worker')"];
        let powershell = [
            "-e",
            r#"const t=Date.now();const r=require('child_process').spawnSync(process.env.SystemRoot+'/System32/WindowsPowerShell/v1.0/powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command','Write-Output ok'],{encoding:'utf8',windowsHide:true,timeout:60000});process.stdout.write(JSON.stringify({ms:Date.now()-t,status:r.status,out:r.stdout,err:r.stderr}))"#,
        ];
        // Compiling C# with Add-Type is what the denial probe does first, and the slowest thing it does.
        let compile = [
            "-e",
            r#"const t=Date.now();const r=require('child_process').spawnSync(process.env.SystemRoot+'/System32/WindowsPowerShell/v1.0/powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command','Add-Type -TypeDefinition "public class Probe { public static int Value() { return 1; } }"; [Probe]::Value()'],{encoding:'utf8',windowsHide:true,timeout:60000});process.stdout.write(JSON.stringify({ms:Date.now()-t,status:r.status,out:r.stdout,err:r.stderr}))"#,
        ];
        for (label, config) in [("provider", &provider), ("review", &review)] {
            experiment(&format!("{label}-trivial"), config, &trivial);
            experiment(&format!("{label}-powershell"), config, &powershell);
            experiment(&format!("{label}-compile"), config, &compile);
        }
    });
    // Cleanup by ns8, as in the other admin tests: the accounts, their credential targets and their folders.
    let base = program_data.join(format!(r"Tjommie\Worker-{}", ns.short()));
    for name in &names {
        let _ = logon::delete_account(name);
    }
    let _ = cred::delete(&ns.isolated_worker_target());
    let _ = cred::delete(&ns.isolated_review_target());
    if base.starts_with(program_data.join("Tjommie")) {
        let _ = fs::remove_dir_all(&base);
    }
    for name in &names {
        assert!(
            logon::account_sid(name).unwrap().is_none(),
            "{name} was not removed"
        );
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
