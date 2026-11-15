//! The UI-thread end of the bridge (`main.ts:331-343`): jobs the bridge thread received are queued here, run on the
//! engine, and answered. The bridge and the engine never touch each other's state: only typed jobs and replies cross.

use std::rc::Rc;

use async_channel::{Receiver, Sender};
use futures::channel::oneshot;
use futures::future::{BoxFuture, LocalBoxFuture};
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserHostCommand};

use super::engine::{Engine, Reply};
use super::host_bridge::{Executor, Job};
use super::tools::Automation;

/// What is on its way to the UI thread: a job, with where its answer goes, or the news that the daemon is gone.
pub enum Queued {
    Job {
        job: Job,
        reply: oneshot::Sender<Result<Reply, String>>,
    },
    DaemonLost,
}

/// The bridge's executor: it queues each job for the UI thread and waits for the answer.
pub struct JobQueue {
    sender: Sender<Queued>,
}

impl JobQueue {
    pub fn new() -> (JobQueue, Receiver<Queued>) {
        let (sender, receiver) = async_channel::unbounded();
        (JobQueue { sender }, receiver)
    }
}

impl Executor for JobQueue {
    fn execute(&self, job: Job) -> BoxFuture<'static, Result<Reply, String>> {
        let (reply, answer) = oneshot::channel();
        let sent = self.sender.try_send(Queued::Job { job, reply });
        Box::pin(async move {
            if sent.is_err() {
                return Err("The native browser is not ready.".to_string());
            }
            answer
                .await
                .unwrap_or_else(|_| Err("The native browser is not ready.".to_string()))
        })
    }

    fn daemon_lost(&self) {
        let _ = self.sender.try_send(Queued::DaemonLost);
    }
}

/// Runs queued jobs on the engine, each as its own task (`spawn` is the UI thread's local spawner), until the queue
/// closes. A bot's own jobs arrive one at a time: the daemon's broker sees to that.
pub async fn serve(
    engine: Rc<Engine>,
    jobs: Receiver<Queued>,
    spawn: impl Fn(LocalBoxFuture<'static, ()>),
) {
    let automation = Automation::new(engine.clone());
    while let Ok(queued) = jobs.recv().await {
        match queued {
            Queued::Job { job, reply } => {
                let engine = engine.clone();
                let automation = automation.clone();
                spawn(Box::pin(async move {
                    let result = run(&engine, &automation, job).await;
                    let _ = reply.send(result);
                }));
            }
            // The old daemon's connections are gone with it: no `disconnect` job will ever come for a phone that held a
            // browser, so the engine lets go of those holds now (before the next job, which may be the new daemon's).
            Queued::DaemonLost => engine.daemon_restarted().await,
        }
    }
}

async fn run(engine: &Rc<Engine>, automation: &Rc<Automation>, job: Job) -> Result<Reply, String> {
    match job.command {
        // Running tools stop before their next step; with none running there is nothing to cancel.
        NativeBrowserCommand::Host(NativeBrowserHostCommand::CancelTools) => {
            automation.cancel(&job.bot.id);
            Ok(Reply::empty())
        }
        NativeBrowserCommand::Host(NativeBrowserHostCommand::Tool(tool)) => {
            Ok(Reply::tool(automation.run(&job.bot, &tool).await))
        }
        command => engine
            .execute(&job.bot, command)
            .await
            .map_err(|error| error.to_string()),
    }
}
