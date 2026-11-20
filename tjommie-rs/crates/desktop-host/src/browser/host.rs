//! The UI-thread end of the bridge (`main.ts:331-343`): jobs the bridge thread received are queued here, run on the
//! engine, and answered. The bridge and the engine never touch each other's state: only typed jobs and replies cross.

use std::rc::Rc;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use futures::channel::oneshot;
use futures::future::{BoxFuture, LocalBoxFuture};
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserHostCommand};

use super::BotIdentity;
use super::engine::{Engine, Reply};
use super::host_bridge::{Executor, Job};
use super::tools::Automation;
use crate::hosts::BotDirectory;

/// What is on its way to the UI thread: a job, with where its answer goes, or the news that the daemon is gone.
pub enum Queued {
    Job {
        // Boxed: a job is far larger than the news that the daemon is gone.
        job: Box<Job>,
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
        let sent = self.sender.try_send(Queued::Job {
            job: Box::new(job),
            reply,
        });
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
/// closes. A bot's own jobs arrive one at a time: the daemon's broker sees to that. `bots` is who the daemon's bots are,
/// asked before every command (`resolveNativeBot`); it is awaited on the UI thread, so it must not need a Tokio runtime
/// of its own to make progress.
pub async fn serve(
    engine: Rc<Engine>,
    jobs: Receiver<Queued>,
    bots: Arc<dyn BotDirectory>,
    spawn: impl Fn(LocalBoxFuture<'static, ()>),
) {
    let automation = Automation::new(engine.clone());
    while let Ok(queued) = jobs.recv().await {
        match queued {
            Queued::Job { job, reply } => {
                let engine = engine.clone();
                let automation = automation.clone();
                let bots = bots.clone();
                spawn(Box::pin(async move {
                    let result = run(&engine, &automation, bots.as_ref(), *job).await;
                    let _ = reply.send(result);
                }));
            }
            // The old daemon's connections are gone with it: no `disconnect` job will ever come for a phone that held a
            // browser, so the engine lets go of those holds now (before the next job, which may be the new daemon's).
            Queued::DaemonLost => engine.daemon_restarted().await,
        }
    }
}

const BOT_OFF: &str = "This agent's browser is off or the agent no longer exists.";
const NO_SERVICE: &str = "The native browser needs the desktop background service.";

/// The bot as the daemon lists it now, if its browser is on (`resolveNativeBot` with `requireBrowser`): the daemon's own
/// word is the gate, so a bot deleted or switched off while the watcher was away is refused here, whatever the engine has
/// been told. The command runs for this record, not for the job's copy of it.
async fn resolve(
    engine: &Engine,
    bots: &dyn BotDirectory,
    id: &str,
) -> Result<BotIdentity, String> {
    match bots.find(id).await {
        Err(_) => Err(NO_SERVICE.to_owned()),
        Ok(Some(bot)) if bot.browser && !engine.bot_is_off(id) => Ok(bot.identity),
        Ok(_) => Err(BOT_OFF.to_owned()),
    }
}

async fn run(
    engine: &Rc<Engine>,
    automation: &Rc<Automation>,
    bots: &dyn BotDirectory,
    job: Job,
) -> Result<Reply, String> {
    match job.command {
        // Running tools stop before their next step; with none running there is nothing to cancel.
        NativeBrowserCommand::Host(NativeBrowserHostCommand::CancelTools) => {
            automation.cancel(&job.bot.id);
            Ok(Reply::empty())
        }
        // Closing a browser and deleting its profile are for a bot the daemon is losing, so they ask nothing of the list.
        command @ NativeBrowserCommand::Host(
            NativeBrowserHostCommand::Close | NativeBrowserHostCommand::DeleteProfile,
        ) => engine
            .execute(&job.bot, command)
            .await
            .map_err(|error| error.to_string()),
        command => {
            let bot = resolve(engine, bots, &job.bot.id).await?;
            let reply = match command {
                NativeBrowserCommand::Host(NativeBrowserHostCommand::Tool(tool)) => {
                    Reply::tool(automation.run(&bot, &tool).await)
                }
                command => engine
                    .execute(&bot, command)
                    .await
                    .map_err(|error| error.to_string())?,
            };
            // A bot deleted or switched off while its command ran: the browser closes, the command fails.
            if engine.bot_is_off(&bot.id) {
                let _ = engine.close_bot(&bot.id).await;
                return Err("This agent's browser is off.".to_owned());
            }
            Ok(reply)
        }
    }
}
