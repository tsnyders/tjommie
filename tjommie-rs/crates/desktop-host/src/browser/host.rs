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

/// A job on its way to the UI thread, with where its answer goes.
pub struct Queued {
    pub job: Job,
    pub reply: oneshot::Sender<Result<Reply, String>>,
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
        let sent = self.sender.try_send(Queued { job, reply });
        Box::pin(async move {
            if sent.is_err() {
                return Err("The native browser is not ready.".to_string());
            }
            answer
                .await
                .unwrap_or_else(|_| Err("The native browser is not ready.".to_string()))
        })
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
        let engine = engine.clone();
        let automation = automation.clone();
        spawn(Box::pin(async move {
            let result = run(&engine, &automation, queued.job).await;
            let _ = queued.reply.send(result);
        }));
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
