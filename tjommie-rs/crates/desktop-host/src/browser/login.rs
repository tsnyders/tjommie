//! The browser side of the login vault, seen from the vault's thread (`native-browser.ts` `vaultDestination`,
//! `beginVault`, `writeVault`, `finishVault`, and `main.ts:228-241`).
//!
//! The vault runs on the `/vault-host` client's thread; the engine and everything of WebView2 live on the UI thread. So
//! [`EngineLogin`], the [`LoginBrowser`] the vault calls, only turns each call into a typed request and awaits the typed
//! answer, and [`serve`] runs the requests on the UI thread, one local task each. Nothing of WebView2 crosses: the bridge
//! rule (the engine never leaves its thread) holds for the vault as it does for `/browser-host` (`host`).
//!
//! A call names its bot by id only. The daemon's list says which bot that is (`BotLookup`, off the UI thread, as
//! `resolveNativeBot` was), and only a bot whose browser is on can be filled.

use std::rc::Rc;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use futures::channel::oneshot;
use futures::future::{BoxFuture, LocalBoxFuture};
use protocol::vault::{FinishMode, LoginOrigin, VaultFillTarget};

use super::BotIdentity;
use super::engine::{Engine, EngineError, VaultFinish};
use crate::vault::login::{Check, LoginBrowser, LoginError};
use crate::vault::storage::VaultSecret;

/// What the engine says when a step stops because the vault's check says the operation was cancelled, locked or moved on.
/// It is the vault's own sentence (`login.rs`), so the two read the same.
const CHANGED: &str = "The Logins operation was cancelled or its security boundary changed.";

/// Finds the bot a Logins call names, from the daemon's list (`resolveNativeBot(id)`): its identity, if the bot exists and
/// its browser is on. Called on the vault's thread, so it may wait for the daemon.
pub trait BotLookup: Send + Sync {
    fn bot(&self, id: &str) -> BoxFuture<'_, Result<BotIdentity, LoginError>>;
}

type Answer<T> = oneshot::Sender<Result<T, LoginError>>;

/// What is on its way to the UI thread. The bot is already resolved; every request carries where its answer goes.
pub enum Request {
    Destination {
        bot: BotIdentity,
        target: VaultFillTarget,
        owner: String,
        answer: Answer<LoginOrigin>,
    },
    Begin {
        bot: BotIdentity,
        target: VaultFillTarget,
        owner: String,
        origin: LoginOrigin,
        check: Check,
        answer: Answer<u64>,
    },
    Fill {
        bot: BotIdentity,
        epoch: u64,
        secret: VaultSecret,
        check: Check,
        answer: Answer<()>,
    },
    Finish {
        bot: BotIdentity,
        generation: String,
        epoch: u64,
        mode: FinishMode,
        check: Check,
        answer: Answer<()>,
    },
}

/// The vault's [`LoginBrowser`]: `Send`, so the vault can call it from its own thread.
pub struct EngineLogin {
    requests: Sender<Request>,
    bots: Arc<dyn BotLookup>,
}

impl EngineLogin {
    /// The browser the vault calls, and the requests [`serve`] runs on the UI thread.
    pub fn new(bots: Arc<dyn BotLookup>) -> (EngineLogin, Receiver<Request>) {
        let (requests, inbox) = async_channel::unbounded();
        (EngineLogin { requests, bots }, inbox)
    }

    /// Sends a request and waits for its answer. A UI thread that has gone (the app is closing) is a failed step.
    async fn ask<T>(&self, request: impl FnOnce(Answer<T>) -> Request) -> Result<T, LoginError> {
        let (answer, answered) = oneshot::channel();
        self.requests
            .try_send(request(answer))
            .map_err(|_| LoginError::Failed)?;
        answered.await.unwrap_or(Err(LoginError::Failed))
    }
}

impl LoginBrowser for EngineLogin {
    fn destination(
        &self,
        target: VaultFillTarget,
        owner: String,
    ) -> BoxFuture<'_, Result<LoginOrigin, LoginError>> {
        Box::pin(async move {
            let bot = self.bots.bot(&target.bot_id).await?;
            self.ask(|answer| Request::Destination {
                bot,
                target,
                owner,
                answer,
            })
            .await
        })
    }

    fn begin(
        &self,
        target: VaultFillTarget,
        owner: String,
        origin: LoginOrigin,
        check: Check,
    ) -> BoxFuture<'_, Result<u64, LoginError>> {
        Box::pin(async move {
            let bot = self.bots.bot(&target.bot_id).await?;
            check()?;
            self.ask(|answer| Request::Begin {
                bot,
                target,
                owner,
                origin,
                check,
                answer,
            })
            .await
        })
    }

    fn fill(
        &self,
        bot_id: String,
        private_epoch: u64,
        secret: VaultSecret,
        check: Check,
    ) -> BoxFuture<'_, Result<(), LoginError>> {
        Box::pin(async move {
            let bot = self.bots.bot(&bot_id).await?;
            check()?;
            self.ask(|answer| Request::Fill {
                bot,
                epoch: private_epoch,
                secret,
                check,
                answer,
            })
            .await
        })
    }

    fn finish(
        &self,
        bot_id: String,
        generation: String,
        private_epoch: u64,
        mode: FinishMode,
        check: Check,
    ) -> BoxFuture<'_, Result<(), LoginError>> {
        Box::pin(async move {
            let bot = self.bots.bot(&bot_id).await?;
            check()?;
            self.ask(|answer| Request::Finish {
                bot,
                generation,
                epoch: private_epoch,
                mode,
                check,
                answer,
            })
            .await
        })
    }
}

/// Runs the vault's requests on the engine, each as its own task (`spawn` is the UI thread's local spawner), until the
/// requests stop coming (the vault is gone).
pub async fn serve(
    engine: Rc<Engine>,
    requests: Receiver<Request>,
    spawn: impl Fn(LocalBoxFuture<'static, ()>),
) {
    while let Ok(request) = requests.recv().await {
        let engine = engine.clone();
        spawn(Box::pin(async move { run(&engine, request).await }));
    }
}

/// The vault's check, as the engine's: a step stops, in the vault's own words, once the operation has ended.
fn stopped(check: &Check) -> impl Fn() -> Result<(), EngineError> + '_ {
    move || check().map_err(|_| EngineError::refused(CHANGED))
}

/// One request, answered. Whatever the engine says is the generic failure to the vault: `login.rs` turns every failure
/// inside an operation into its own sentence, and an engine's words never reach the person.
async fn run(engine: &Rc<Engine>, request: Request) {
    match request {
        Request::Destination {
            bot,
            target,
            owner,
            answer,
        } => {
            let found = engine.vault_destination(&bot, &target, &owner);
            let _ = answer.send(found.map_err(failed));
        }
        Request::Begin {
            bot,
            target,
            owner,
            origin,
            check,
            answer,
        } => {
            let began = engine
                .begin_vault(&bot, &target, &owner, &origin, &stopped(&check))
                .await;
            let _ = answer.send(began.map_err(failed));
        }
        Request::Fill {
            bot,
            epoch,
            secret,
            check,
            answer,
        } => {
            let filled = engine
                .write_vault(&bot, epoch, &secret, &stopped(&check))
                .await;
            let _ = answer.send(filled.map_err(failed));
        }
        Request::Finish {
            bot,
            generation,
            epoch,
            mode,
            check,
            answer,
        } => {
            let mode = match mode {
                FinishMode::Clear => VaultFinish::Clear,
                FinishMode::Close => VaultFinish::Close,
            };
            let finished = engine
                .finish_vault(&bot, &generation, epoch, mode, &stopped(&check))
                .await;
            let _ = answer.send(finished.map_err(failed));
        }
    }
}

fn failed(_: EngineError) -> LoginError {
    LoginError::Failed
}
