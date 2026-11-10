//! `tjommie-daemon.exe`: started by the desktop app with `--token-stdin`, or by a developer with `--dev`.

use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use daemon::startup::{self, Auth, Mode, StartupError, StdinEvent};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(StartupError::Plain(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        Err(StartupError::Unexpected(error)) => {
            eprintln!("Tjommie's daemon could not start: {error:?}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), StartupError> {
    if args.first().is_some_and(|arg| arg == "import") {
        return import(&args[1..]);
    }
    let launch = startup::parse_launch(args, |key| std::env::var(key).ok())?;
    let (auth, stdin) = match launch.mode {
        Mode::Development => (Auth::Development, None),
        Mode::Production => {
            let stdin = startup::watch_stdin(std::io::stdin());
            let line = match stdin.recv_timeout(Duration::from_secs(5)) {
                Ok(StdinEvent::Line(line)) => line,
                Ok(StdinEvent::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(StartupError::Plain(
                        "stdin closed before a daemon token arrived.".into(),
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(StartupError::Plain(
                        "No daemon token arrived on stdin within 5 seconds.".into(),
                    ));
                }
            };
            (startup::production_auth(&line)?, Some(stdin))
        }
    };
    if launch.accounts_dir.is_some() {
        eprintln!("Using account folders from TJOMMIE_ACCOUNTS_DIR.");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let daemon = startup::start(&launch, &auth).await?;
        // The desktop app and updater.real.test.ts find the port with a regex on exactly this line.
        println!(
            "Tjommie daemon listening on http://127.0.0.1:{}",
            daemon.port()
        );
        tokio::select! {
            () = interrupted() => {}
            () = stdin_closed(stdin) => {}
        }
        daemon.close().await;
        Ok(())
    })
}

/// `import --from <root> [--replace]`: copies another install into this build's data root (spec §4).
fn import(args: &[String]) -> Result<(), StartupError> {
    let usage = || {
        StartupError::Plain("Usage: tjommie-daemon import --from <data root> [--replace]".into())
    };
    let (mut from, mut replace) = (None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" => from = Some(args.next().ok_or_else(usage)?),
            "--replace" => replace = true,
            _ => return Err(usage()),
        }
    }
    let from = from.ok_or_else(usage)?;
    let to = daemon::paths::data_root();
    let secrets = daemon::import::run(Path::new(from), &to, replace)
        .map_err(|error| StartupError::Plain(error.to_string()))?;
    println!("Imported {from} into {}.", to.display());
    println!(
        "Re-wrote {} account keys and {} logins in this install's Credential Manager namespace.",
        secrets.account_keys, secrets.logins
    );
    println!(
        "Project teams weren't imported: their tasks and worktrees stay with {from}, and queued task work was marked failed."
    );
    Ok(())
}

/// Ctrl+C. If no handler can be installed, the daemon waits for stdin instead.
async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Production stops when the desktop app closes stdin; development never does.
async fn stdin_closed(stdin: Option<Receiver<StdinEvent>>) {
    let Some(stdin) = stdin else {
        return std::future::pending().await;
    };
    let (closed, wait) = tokio::sync::oneshot::channel();
    // A plain thread, not spawn_blocking: the runtime must not wait for it when the daemon stops another way.
    std::thread::spawn(move || {
        while let Ok(StdinEvent::Line(_)) = stdin.recv() {}
        let _ = closed.send(());
    });
    let _ = wait.await;
}
