//! `tools/remote-probe.ts`: the real TypeScript client and modules, driven one JSON command per line.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use daemon::spawn;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::timeout;

pub struct Probe {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Probe {
    pub fn start() -> Probe {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/remote-probe.ts");
        let mut command = spawn::command("node");
        command.arg(script).env("NODE_NO_WARNINGS", "1");
        let mut command = tokio::process::Command::from(std::process::Command::from(command));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn().expect("node is on PATH");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        Probe {
            child,
            stdin,
            stdout,
        }
    }

    pub async fn send(&mut self, command: Value) -> Value {
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        let line = timeout(Duration::from_secs(60), self.stdout.next_line())
            .await
            .unwrap_or_else(|_| panic!("the probe answers {command}"))
            .unwrap()
            .expect("the probe is running");
        serde_json::from_str(&line).unwrap()
    }

    pub async fn stop(mut self) {
        drop(self.stdin);
        let _ = timeout(Duration::from_secs(30), self.child.wait()).await;
    }
}
