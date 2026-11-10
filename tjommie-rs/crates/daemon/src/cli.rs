//! The seam between the providers and real processes: what `launchCli`, `killTree` and `killTreeAndWait` in
//! `packages/daemon/src/cli.ts` give them. T3.4 ports the rest of `cli.ts` here, including the real `CliHost`, which
//! launches through `spawn::command()` with `cliEnv`'s scrubbed environment.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use crate::providers::codex_rpc::{self, CodexServer};
use crate::providers::{ApiKey, ProviderError};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub struct CliCommand {
    pub file: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchOptions {
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    /// Only this explicit private capability restores a key after inherited-environment scrubbing.
    pub claude_api_key: Option<ApiKey>,
}

/// How a child ended: Node's `exit` event, or its `error` event (it failed to start or was lost).
#[derive(Debug, Clone, PartialEq)]
pub enum CliExit {
    Exited {
        code: Option<i32>,
        signal: Option<String>,
    },
    Failed(String),
}

/// Stops the child's whole process tree and waits for it (`killTreeAndWait(child.pid)`). Claude's fire-and-forget
/// `killTree` is the same call, unawaited.
pub type KillTree = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

type ExitListener = Box<dyn FnOnce(&CliExit) + Send>;

struct ExitState {
    status: watch::Sender<Option<CliExit>>,
    listeners: Mutex<Vec<ExitListener>>,
}

/// A child's exit, like Node's `exit` event plus `exitCode`: listeners run synchronously when it fires.
#[derive(Clone)]
pub struct ChildExit(Arc<ExitState>);

impl Default for ChildExit {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildExit {
    pub fn new() -> Self {
        ChildExit(Arc::new(ExitState {
            status: watch::channel(None).0,
            listeners: Mutex::new(Vec::new()),
        }))
    }

    /// Records the exit and runs the listeners. Only the first call counts.
    pub fn fire(&self, exit: CliExit) {
        let first = self
            .0
            .status
            .send_if_modified(|status| status.is_none() && status.replace(exit.clone()).is_none());
        if first {
            let listeners =
                std::mem::take(&mut *self.0.listeners.lock().expect("exit listeners lock"));
            listeners.into_iter().for_each(|listener| listener(&exit));
        }
    }

    /// Runs `listener` when the child exits, or now if it already has.
    pub fn on_exit(&self, listener: impl FnOnce(&CliExit) + Send + 'static) {
        let mut listeners = self.0.listeners.lock().expect("exit listeners lock");
        match self.status() {
            Some(exit) => {
                drop(listeners);
                listener(&exit);
            }
            None => listeners.push(Box::new(listener)),
        }
    }

    pub fn status(&self) -> Option<CliExit> {
        self.0.status.borrow().clone()
    }

    pub async fn wait(&self) -> CliExit {
        let mut status = self.0.status.subscribe();
        let exit = status
            .wait_for(Option::is_some)
            .await
            .expect("the sender lives in self");
        exit.clone().expect("waited for an exit")
    }
}

/// A launched CLI with piped stdio.
pub struct CliChild {
    pub pid: Option<u32>,
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Box<dyn AsyncRead + Send + Unpin>,
    pub exit: ChildExit,
    pub kill_tree: KillTree,
}

pub trait CliHost: Send + Sync {
    /// `launchCli`: starts `cmd` with `args` appended, in `options.cwd` with exactly `options.env`.
    fn launch<'a>(
        &'a self,
        cmd: &'a CliCommand,
        args: Vec<String>,
        options: LaunchOptions,
    ) -> BoxFuture<'a, std::io::Result<CliChild>>;

    /// `startCodexServer`. Tests replace this to drive a Codex session's RPC directly, as the TypeScript tests mock it.
    fn start_codex_server<'a>(
        &'a self,
        cmd: &'a CliCommand,
        env: &'a BTreeMap<String, String>,
        cwd: &'a str,
        ephemeral_auth: bool,
    ) -> BoxFuture<'a, Result<CodexServer, ProviderError>> {
        Box::pin(codex_rpc::start_codex_server(
            self,
            cmd,
            env,
            cwd,
            ephemeral_auth,
        ))
    }
}
