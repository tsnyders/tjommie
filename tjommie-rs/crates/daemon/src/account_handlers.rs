//! `account-handlers.ts`: the `accounts.*` and `usage.query` methods. With them, the account-runtime parts of
//! `daemon.ts` (193-235, 300-333, 422-437): account events, the `setup.*` methods over the default accounts, and the
//! per-account change barrier that bots check too.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::future::try_join_all;
use protocol::accounts::{
    AccountAttempt, AccountInfo, AccountInput as NewAccount, AccountKind, AccountSignInEnded,
    AccountsCheck, AccountsUpdate, SignInStart,
};
use protocol::model::{Provider, ProviderSetup};
use protocol::wire::{Empty, Id, Notify, SetupSignInEnded, method};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, OptionalExtension};

use crate::account_quota::now_ms;
use crate::account_runtime::{
    AccountHold, AccountOps, AccountRuntime, AccountRuntimeOptions, CredentialStore,
};
use crate::accounts_store::{
    AccountInput, AccountPatch, create_account, get_account, get_default_account, list_accounts,
    remove_account, update_account,
};
use crate::bots::list_stored_bots;
use crate::context::{BotView, Context, Db};
use crate::ids::new_id;
use crate::server::{Broadcaster, Handlers, RequestContext, RpcError};
use crate::setup::Setup;
use crate::usage::query_usage;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn busy(message: &str) -> RpcError {
    RpcError::new(ErrorCode::Busy, message)
}

fn bad_params(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadParams, message)
}

/// The default accounts' setups, as `setup.changed` reports them.
fn default_setups(accounts: &[AccountInfo]) -> Vec<ProviderSetup> {
    accounts
        .iter()
        .filter(|account| account.is_default)
        .filter_map(|account| account.setup.clone())
        .collect()
}

/// What account changes tell clients.
#[derive(Clone)]
struct Feed {
    db: Db,
    events: Broadcaster,
    bot_view: Arc<dyn BotView>,
    provider_setup: Arc<Mutex<Vec<ProviderSetup>>>,
}

impl Feed {
    fn bots_updated(&self) {
        let bots = {
            let db = lock(&self.db);
            list_stored_bots(&db).map(|bots| {
                bots.into_iter()
                    .map(|bot| self.bot_view.to_bot(&db, bot))
                    .collect::<Vec<_>>()
            })
        };
        for bot in bots.unwrap_or_default() {
            self.events.send(Event::BotUpdated(bot));
        }
    }

    fn changed(&self, runtime: &AccountRuntime) {
        let Ok(current) = runtime.list() else {
            return;
        };
        let setup = default_setups(&current);
        *lock(&self.provider_setup) = setup.clone();
        self.events.send(Event::AccountsChanged(current));
        self.events.send(Event::SetupChanged(setup));
        // T3.5: runner.setupChanged().
        self.bots_updated();
    }

    /// `refreshSetup`: checks the default accounts and reports their setup.
    async fn refresh_setup(
        &self,
        runtime: &AccountRuntime,
    ) -> Result<Vec<ProviderSetup>, RpcError> {
        // T2.2 refreshes folder protection first, and logs a Codex version that differs from its measured ACL recipe.
        let accounts = list_accounts(&lock(&self.db), false)?;
        let defaults = accounts.iter().filter(|account| account.is_default);
        try_join_all(defaults.map(|account| runtime.check(&account.id))).await?;
        let setup = default_setups(&runtime.list()?);
        *lock(&self.provider_setup) = setup.clone();
        self.events.send(Event::SetupChanged(setup.clone()));
        self.bots_updated();
        // T3.5: runner.setupChanged().
        Ok(setup)
    }

    fn sign_in_ended(&self, runtime: &AccountRuntime, account_id: &str, attempt: &str) {
        self.events
            .send(Event::AccountsSignInEnded(AccountSignInEnded {
                account_id: account_id.into(),
                attempt: attempt.into(),
            }));
        let default = {
            let db = lock(&self.db);
            get_account(&db, account_id, false).and_then(|account| {
                let default = get_default_account(&db, account.provider)?.id == account_id;
                Ok((account.provider, default))
            })
        };
        let Ok((provider, true)) = default else {
            return;
        };
        // The fresh setup goes out first, so one that worked already reads as signed in; then setup.signInEnded,
        // because one that failed or was abandoned changes nothing a client could notice.
        let (feed, runtime, attempt) = (self.clone(), runtime.clone(), attempt.to_string());
        tokio::spawn(async move {
            if feed.refresh_setup(&runtime).await.is_err() {
                eprintln!("Failed to refresh provider setup.");
            }
            feed.events.send(Event::SetupSignInEnded(SetupSignInEnded {
                provider,
                attempt,
            }));
        });
    }

    /// Tells every bot on an API account that reached 80 or 100 percent of its local cap.
    fn cap(&self, runtime: &AccountRuntime, account_id: &str, threshold: u8) {
        let Ok(info) = runtime.info(account_id) else {
            return;
        };
        let bots: Vec<String> = {
            let db = lock(&self.db);
            let on_account = |bot: &crate::bots::StoredBot| {
                bot.account_id.as_deref() == Some(account_id)
                    || self
                        .bot_view
                        .to_bot(&db, bot.clone())
                        .effective_account_id
                        .flatten()
                        .as_deref()
                        == Some(account_id)
            };
            list_stored_bots(&db)
                .unwrap_or_default()
                .into_iter()
                .filter(|bot| on_account(bot))
                .map(|bot| bot.id)
                .collect()
        };
        let label = &info.label;
        let (title, body) = if info.spending.unknown_rows > 0 {
            (
                format!("{label} has unpriced usage"),
                "This key is blocked because a turn's cost is unknown. Review its usage in Accounts.",
            )
        } else if threshold == 100 {
            (
                format!("{label} reached {threshold}% of its local cap"),
                "This key reached its local cap. Review its recorded spend in Accounts.",
            )
        } else {
            (
                format!("{label} reached {threshold}% of its local cap"),
                "Review this key's recorded spend in Accounts. Provider charges may differ from local estimates.",
            )
        };
        for bot_id in bots {
            // T3.5: at 100 percent the runner stops the bot, and notifies "Check this bot after its spending cap" if the
            // provider does not confirm Stop.
            self.events.send(Event::Notify(Notify::Limit {
                bot_id,
                title: title.clone(),
                body: body.into(),
            }));
        }
    }
}

pub struct AccountsOptions {
    pub db: Db,
    /// The data root and its `accounts` folder.
    pub root: PathBuf,
    pub accounts_dir: PathBuf,
    pub events: Broadcaster,
    pub bot_view: Arc<dyn BotView>,
    pub setup: Setup,
    /// `Setup` and the root's Credential Manager namespace in production.
    pub ops: Arc<dyn AccountOps>,
    pub credentials: Arc<dyn CredentialStore>,
}

/// The account services the handlers share. Cloning shares them.
#[derive(Clone)]
pub struct Accounts {
    pub runtime: AccountRuntime,
    pub setup: Setup,
    accounts_dir: PathBuf,
    /// Accounts with a change in flight: one change per account at a time (`changingAccounts`).
    changing: Arc<Mutex<HashSet<String>>>,
    feed: Feed,
}

/// One account change in flight. Dropping it ends the change, then releases its hold.
struct Change {
    changing: Arc<Mutex<HashSet<String>>>,
    id: String,
    hold: Option<AccountHold>,
}

impl Drop for Change {
    fn drop(&mut self) {
        lock(&self.changing).remove(&self.id);
        if let Some(hold) = self.hold.take() {
            hold.release();
        }
    }
}

impl Accounts {
    pub fn new(options: AccountsOptions) -> Self {
        let feed = Feed {
            db: options.db.clone(),
            events: options.events,
            bot_view: options.bot_view,
            provider_setup: Arc::default(),
        };
        let (changed, ended, cap) = (feed.clone(), feed.clone(), feed.clone());
        let runtime = AccountRuntime::new(AccountRuntimeOptions {
            db: options.db,
            root: options.root,
            accounts: options.accounts_dir.clone(),
            credentials: options.credentials,
            ops: options.ops,
            on_changed: Some(Arc::new(move |runtime: &AccountRuntime, _: &str| {
                changed.changed(runtime)
            })),
            on_sign_in_ended: Some(Arc::new(
                move |runtime: &AccountRuntime, id: &str, attempt: &str| {
                    ended.sign_in_ended(runtime, id, attempt)
                },
            )),
            on_cap: Some(Arc::new(
                move |runtime: &AccountRuntime, id: &str, threshold: u8| {
                    cap.cap(runtime, id, threshold)
                },
            )),
            now: Arc::new(now_ms),
        });
        Accounts {
            runtime,
            setup: options.setup,
            accounts_dir: options.accounts_dir,
            changing: Arc::default(),
            feed,
        }
    }

    /// The default accounts' latest setup checks.
    pub fn provider_setup(&self) -> Vec<ProviderSetup> {
        lock(&self.feed.provider_setup).clone()
    }

    /// `checkAccountAvailable`: a bot can't take an account mid-change. No account given means the provider default.
    pub fn check_available(
        &self,
        db: &Connection,
        provider: Provider,
        requested: Option<&str>,
    ) -> Result<(), RpcError> {
        let id = match requested {
            Some(id) => id.to_string(),
            None => get_default_account(db, provider)?.id,
        };
        if lock(&self.changing).contains(&id) {
            return Err(busy("This account is changing. Try again shortly."));
        }
        Ok(())
    }

    fn not_running(&self, id: &str) -> Result<(), RpcError> {
        if self.runtime.in_use(id) {
            return Err(busy(
                "Stop the bots using this account before changing its sign-in or key.",
            ));
        }
        Ok(())
    }

    /// `change`: one change per account. `idle` refuses while a turn uses the account; `hold` blocks new work on it
    /// until the change ends.
    fn enter(&self, id: &str, idle: bool, hold: bool) -> Result<Change, RpcError> {
        {
            let mut changing = lock(&self.changing);
            if changing.contains(id) {
                return Err(busy("This account is already changing. Try again shortly."));
            }
            if idle {
                self.not_running(id)?;
            }
            changing.insert(id.into());
        }
        Ok(Change {
            changing: Arc::clone(&self.changing),
            id: id.into(),
            hold: hold.then(|| self.runtime.hold(id)),
        })
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        lock(&self.feed.db)
    }

    async fn check(
        &self,
        AccountsCheck { id }: AccountsCheck,
    ) -> Result<Vec<AccountInfo>, RpcError> {
        match id {
            Some(id) => {
                self.runtime.check(&id).await?;
                self.runtime.list()
            }
            None => self.runtime.check_all().await,
        }
    }

    async fn create(&self, input: NewAccount) -> Result<AccountInfo, RpcError> {
        let id = new_id();
        let folder = self.accounts_dir.join(&id);
        std::fs::create_dir_all(&folder)?;
        let (provider, label, kind, key, cap, fallback) = match input {
            NewAccount::Subscription { provider, label } => (
                provider,
                label,
                AccountKind::Subscription,
                None,
                None,
                false,
            ),
            NewAccount::ApiKey {
                provider,
                label,
                api_key,
                spending_cap_usd,
                is_fallback,
            } => (
                provider,
                label,
                AccountKind::ApiKey,
                Some(api_key),
                Some(spending_cap_usd),
                is_fallback.unwrap_or(false),
            ),
        };
        let input = AccountInput {
            id: Some(id.clone()),
            provider,
            label,
            kind,
            folder: folder.to_string_lossy().into_owned(),
            credential_ref: None,
            is_default: false,
            is_fallback: fallback,
            spending_cap_usd: cap,
        };
        create_account(&self.db(), input)?;
        let _change = self.enter(&id, false, true)?;
        let made = async {
            // T2.2 refreshes folder protection here (`protect()`).
            if let Some(key) = &key {
                self.runtime.set_api_key(&id, key.as_str()).await?;
            }
            self.runtime.changed(&id);
            self.runtime.info(&id)
        }
        .await;
        if made.is_err() {
            // Discoverable metadata stays if the credential cleanup fails, so the owner can retry removal.
            let removed = async {
                self.runtime.delete_credential(&id).await?;
                remove_account(&self.db(), &id)
            }
            .await;
            if removed.is_err() {
                self.runtime.changed(&id);
            }
            return Err(RpcError::new(
                ErrorCode::BadState,
                "Account setup did not finish. Check Accounts before trying again.",
            ));
        }
        made
    }

    async fn update(
        &self,
        AccountsUpdate { id, patch }: AccountsUpdate,
    ) -> Result<AccountInfo, RpcError> {
        let key = patch.api_key.clone();
        let _change = self.enter(&id, key.is_some(), key.is_some())?;
        let account = get_account(&self.db(), &id, false)?;
        match key {
            Some(key) => {
                if account.kind != AccountKind::ApiKey {
                    return Err(bad_params("Only API-key accounts can store an API key."));
                }
                let metadata = patch.label.is_some()
                    || patch.is_default.is_some()
                    || patch.is_fallback.is_some()
                    || patch.spending_cap_usd.is_some();
                if metadata {
                    return Err(bad_params(
                        "Save a replacement key separately from account settings.",
                    ));
                }
                self.not_running(&id)?;
                self.runtime.set_api_key(&id, key.as_str()).await?;
                self.runtime.changed(&id);
            }
            None => {
                let metadata = AccountPatch {
                    label: patch.label,
                    credential_ref: None,
                    is_default: patch.is_default,
                    is_fallback: patch.is_fallback,
                    spending_cap_usd: patch.spending_cap_usd.map(Some),
                };
                update_account(&self.db(), &id, metadata)?;
                self.runtime.metadata_changed(&id);
            }
        }
        self.runtime.info(&id)
    }

    async fn remove(&self, id: String) -> Result<Empty, RpcError> {
        let _change = self.enter(&id, true, true)?;
        let pinned = {
            let db = self.db();
            get_account(&db, &id, false)?;
            db.query_row(
                "SELECT id FROM bots WHERE account_id = ?1 LIMIT 1",
                [&id],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        };
        self.not_running(&id)?;
        if pinned {
            return Err(busy(
                "Choose another account for the bots assigned to this account before removing it.",
            ));
        }
        self.runtime.delete_credential(&id).await?;
        remove_account(&self.db(), &id)?;
        self.runtime.changed(&id);
        Ok(Empty {})
    }

    async fn sign_in(&self, id: String) -> Result<SignInStart, RpcError> {
        let _change = self.enter(&id, true, true)?;
        self.not_running(&id)?;
        // T2.2 refreshes folder protection here (`protect()`).
        self.runtime.sign_in(&id).await
    }

    async fn cancel_sign_in(
        &self,
        AccountAttempt { id, attempt }: AccountAttempt,
    ) -> Result<Empty, RpcError> {
        let _change = self.enter(&id, false, true)?;
        self.runtime.cancel_sign_in(&id, &attempt).await?;
        Ok(Empty {})
    }

    async fn prepare_codex_sandbox(&self, id: String) -> Result<AccountInfo, RpcError> {
        let _change = self.enter(&id, true, true)?;
        self.not_running(&id)?;
        // T2.2 refreshes folder protection after this, whether or not it worked (`protect()`).
        self.runtime.prepare_sandbox(&id).await?;
        self.runtime.info(&id)
    }

    fn default_id(&self, provider: Provider) -> Result<String, RpcError> {
        Ok(get_default_account(&self.db(), provider)?.id)
    }

    async fn setup_prepare_codex_sandbox(&self) -> Result<ProviderSetup, RpcError> {
        let prepared = match self.default_id(Provider::Codex) {
            Ok(id) => self.prepare_codex_sandbox(id).await.map(drop),
            Err(error) => Err(error),
        };
        let refreshed = self.feed.refresh_setup(&self.runtime).await;
        prepared?;
        refreshed?
            .into_iter()
            .find(|setup| matches!(setup, ProviderSetup::Codex(_)))
            .ok_or_else(|| RpcError::new(ErrorCode::Internal, "Codex setup status is missing."))
    }
}

/// A handler over a clone of the shared accounts.
fn serve<P, Fut>(
    accounts: &Accounts,
    handler: impl Fn(Accounts, P) -> Fut + Send + Sync + 'static,
) -> impl Fn(P, Option<RequestContext>) -> Fut + Send + Sync + 'static {
    let accounts = accounts.clone();
    move |params, _| handler(accounts.clone(), params)
}

/// The `accounts.*`, `usage.query`, `setup.*` and `models.list` methods.
pub fn register(handlers: Handlers, context: &Context) -> Handlers {
    let accounts = &context.accounts;
    handlers
        .on::<method::AccountsList, _>(serve(
            accounts,
            |a, _: Empty| async move { a.runtime.list() },
        ))
        .on::<method::AccountsCheck, _>(serve(
            accounts,
            |a, check| async move { a.check(check).await },
        ))
        .on::<method::AccountsRetryLimit, _>(serve(accounts, |a, Id { id }| async move {
            let _change = a.enter(&id, true, true)?;
            a.runtime.retry_limit(&id)
        }))
        .on::<method::AccountsCreate, _>(serve(accounts, |a, input| async move {
            a.create(input).await
        }))
        .on::<method::AccountsUpdate, _>(serve(accounts, |a, update| async move {
            a.update(update).await
        }))
        .on::<method::AccountsRemove, _>(serve(accounts, |a, Id { id }| async move {
            a.remove(id).await
        }))
        .on::<method::AccountsSignIn, _>(serve(accounts, |a, Id { id }| async move {
            a.sign_in(id).await
        }))
        .on::<method::AccountsCancelSignIn, _>(serve(accounts, |a, attempt| async move {
            a.cancel_sign_in(attempt).await
        }))
        .on::<method::AccountsPrepareCodexSandbox, _>(serve(accounts, |a, Id { id }| async move {
            a.prepare_codex_sandbox(id).await
        }))
        .on::<method::UsageQuery, _>(serve(accounts, |a, query| async move {
            query_usage(&a.db(), &query)
        }))
        .on::<method::SetupStatus, _>(serve(accounts, |a, _: Empty| async move {
            a.feed.refresh_setup(&a.runtime).await
        }))
        .on::<method::SetupSignIn, _>(serve(
            accounts,
            |a, params: protocol::wire::ProviderParams| async move {
                let id = a.default_id(params.provider)?;
                a.sign_in(id).await
            },
        ))
        .on::<method::SetupPrepareCodexSandbox, _>(serve(accounts, |a, _: Empty| async move {
            a.setup_prepare_codex_sandbox().await
        }))
        .on::<method::ModelsList, _>(serve(
            accounts,
            |a, params: protocol::wire::ProviderParams| {
                a.setup.list_models(&a.db(), params.provider)
            },
        ))
}
