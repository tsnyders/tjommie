//! Fake CLI processes for the provider tests, standing in for the `vi.mock("../cli.ts")` fakes in TypeScript.
#![allow(dead_code)] // Each test binary uses part of this.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use daemon::cli::{
    BoxFuture, ChildExit, CliChild, CliCommand, CliExit, CliHost, KillTree, LaunchOptions,
};
use daemon::providers::ProviderError;
use daemon::providers::codex_rpc::{CodexServer, start_codex_server};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex};
use tokio::sync::oneshot;

/// The test's ends of a fake child's pipes.
pub struct FakeChild {
    /// What the provider wrote to the child's stdin.
    pub stdin: BufReader<DuplexStream>,
    pub stdout: DuplexStream,
    pub stderr: DuplexStream,
    pub exit: ChildExit,
    pub pid: u32,
    /// How often the provider stopped the process tree.
    pub kills: Arc<AtomicUsize>,
}

impl FakeChild {
    pub async fn print(&mut self, line: &str) {
        self.stdout
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        settle().await;
    }

    pub async fn read_line(&mut self) -> String {
        let mut line = String::new();
        self.stdin.read_line(&mut line).await.unwrap();
        line
    }

    pub fn exit_with(&self, code: Option<i32>, signal: Option<&str>) {
        self.exit.fire(CliExit::Exited {
            code,
            signal: signal.map(str::to_string),
        });
    }

    pub fn kills(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }
}

pub fn fake_child(pid: u32) -> (CliChild, FakeChild) {
    let (stdin, stdin_test) = duplex(1 << 20);
    let (stdout, stdout_test) = duplex(1 << 20);
    let (stderr, stderr_test) = duplex(1 << 20);
    let exit = ChildExit::new();
    let kills = Arc::new(AtomicUsize::new(0));
    let kill_tree: KillTree = {
        let kills = Arc::clone(&kills);
        Arc::new(move || {
            kills.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        })
    };
    let child = CliChild {
        pid: Some(pid),
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        stderr: Box::new(stderr),
        exit: exit.clone(),
        kill_tree,
    };
    (
        child,
        FakeChild {
            stdin: BufReader::new(stdin_test),
            stdout: stdout_test,
            stderr: stderr_test,
            exit,
            pid,
            kills,
        },
    )
}

type Launch = Box<dyn FnOnce(&[String]) -> CliChild + Send>;
type StartServer = BoxFuture<'static, Result<CodexServer, ProviderError>>;
pub type LaunchCall = (CliCommand, Vec<String>, LaunchOptions);
/// `(env, cwd, ephemeral_auth)`.
pub type ServerStart = (BTreeMap<String, String>, String, bool);

/// Launches queued fake children, or hands out queued Codex servers in place of `codex app-server`.
#[derive(Default)]
pub struct FakeHost {
    pub launches: Mutex<Vec<LaunchCall>>,
    children: Mutex<VecDeque<Launch>>,
    servers: Mutex<VecDeque<StartServer>>,
    pub server_starts: Mutex<Vec<ServerStart>>,
}

impl FakeHost {
    pub fn push_child(&self, child: CliChild) {
        self.push_launch(move |_| child);
    }

    /// Like `mockImplementationOnce`: `launch` sees the CLI arguments.
    pub fn push_launch(&self, launch: impl FnOnce(&[String]) -> CliChild + Send + 'static) {
        self.children.lock().unwrap().push_back(Box::new(launch));
    }

    pub fn push_server(&self, server: CodexServer) {
        self.servers
            .lock()
            .unwrap()
            .push_back(Box::pin(async { Ok(server) }));
    }

    /// A server whose start finishes when the test sends it.
    pub fn push_pending_server(&self) -> oneshot::Sender<CodexServer> {
        let (sender, receiver) = oneshot::channel();
        self.servers.lock().unwrap().push_back(Box::pin(async {
            Ok(receiver.await.expect("the test sends the server"))
        }));
        sender
    }

    pub fn last_launch(&self) -> (CliCommand, Vec<String>, LaunchOptions) {
        self.launches
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a launch")
    }
}

impl CliHost for FakeHost {
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, std::io::Result<CliChild>> {
        self.launches
            .lock()
            .unwrap()
            .push((cmd.clone(), args.clone(), options));
        let launch = self.children.lock().unwrap().pop_front();
        Box::pin(async move { Ok(launch.expect("a queued fake child")(&args)) })
    }

    fn start_codex_server<'a>(
        &'a self,
        cmd: &'a CliCommand,
        env: &'a BTreeMap<String, String>,
        cwd: &'a str,
        ephemeral_auth: bool,
    ) -> BoxFuture<'a, Result<CodexServer, ProviderError>> {
        self.server_starts
            .lock()
            .unwrap()
            .push((env.clone(), cwd.into(), ephemeral_auth));
        match self.servers.lock().unwrap().pop_front() {
            Some(server) => server,
            None => Box::pin(start_codex_server(self, cmd, env, cwd, ephemeral_auth)),
        }
    }
}

pub fn command(file: &str) -> CliCommand {
    CliCommand {
        file: file.into(),
        args: vec![],
    }
}

/// Lets spawned tasks (line readers, writers, timers due now) catch up. The tests run on one thread.
pub async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

/// Every event received so far.
pub fn drain(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<daemon::providers::ProviderEvent>,
) -> Vec<daemon::providers::ProviderEvent> {
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

/// A fresh folder whose path has a space and an accented letter, like many Windows profile paths.
pub fn temp_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("tjommie test ñ-")
        .tempdir()
        .unwrap()
}
