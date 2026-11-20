//! A stand-in for `tjommie-daemon.exe` in desktop-host's process tests. It reads the token line the way the daemon does,
//! then behaves as `FAKE_DAEMON_MODE` says:
//!
//! - `serve` (default): prints the listening line and runs until its stdin closes, then exits 0.
//! - `crash`: prints the listening line, then exits 7 after `FAKE_DAEMON_LIFETIME_MS` (default 50).
//! - `exit1`: writes a reason to stderr and exits 1 at once; `exit1-quiet` says nothing.
//!
//! Every mode first prints `pid <its pid>`, so a test can check the process is really gone.
//! - `silent`: never says it is listening.
//! - `hang`: prints the listening line and ignores its stdin closing, so only a kill stops it.
//! - `hang-with-child`: like `hang`, and first starts a copy of itself in `hang` mode that inherits the pipes, printing
//!   `child-pid <pid>`. Killing only this process leaves the pipes open; the tree kill must reach the child too.
//!
//! `FAKE_DAEMON_RECORD`, when set, names a file that receives the token line as it arrived.

use std::io::{BufRead, Write};
use std::time::Duration;

fn say(text: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
}

fn listening() {
    say("Tjommie daemon listening on http://127.0.0.1:47999");
}

fn forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() {
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    if let Ok(record) = std::env::var("FAKE_DAEMON_RECORD") {
        let _ = std::fs::write(record, &line);
    }
    say(&format!("pid {}", std::process::id()));
    match std::env::var("FAKE_DAEMON_MODE")
        .unwrap_or_default()
        .as_str()
    {
        "exit1" => {
            eprintln!("The fake daemon refuses to start.");
            std::process::exit(1);
        }
        "exit1-quiet" => std::process::exit(1),
        "silent" => forever(),
        "crash" => {
            listening();
            let millis = std::env::var("FAKE_DAEMON_LIFETIME_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(50);
            std::thread::sleep(Duration::from_millis(millis));
            std::process::exit(7);
        }
        "hang" => {
            listening();
            forever()
        }
        "hang-with-child" => {
            if let Ok(exe) = std::env::current_exe() {
                let child = desktop_host::spawn::command(exe)
                    .env("FAKE_DAEMON_MODE", "hang")
                    .stdin(std::process::Stdio::null())
                    .spawn();
                if let Ok(child) = child {
                    say(&format!("child-pid {}", child.id()));
                }
            }
            listening();
            forever()
        }
        _ => {
            listening();
            // Stdin ends when the app closes it: that is how the real daemon is told to stop.
            let mut rest = String::new();
            while matches!(std::io::stdin().lock().read_line(&mut rest), Ok(read) if read > 0) {
                rest.clear();
            }
        }
    }
}
