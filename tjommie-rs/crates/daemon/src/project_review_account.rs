//! `project-review-account.ts`: leases a reviewer's dedicated Claude API account for one review, charges the usage its
//! turns report, and releases the lease exactly once. Subscription and Codex accounts are refused before any read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};

use protocol::Event;
use protocol::accounts::{AccountKind, UsageChanged};
use protocol::model::Provider;
use tokio_util::sync::CancellationToken;

use crate::account_pricing::price_usage;
use crate::account_runtime::{AccountRunContext, AccountRuntime, AccountSelection};
use crate::bots::get_stored_bot;
use crate::cli::BoxFuture;
use crate::context::Db;
use crate::project_review::ReviewContext;
use crate::project_review_runtime::{
    ReviewAccount, ReviewAccountResolver, ReviewAccountSelection,
};
use crate::providers::{ApiKey, ProviderEvent, Usage};
use crate::server::Broadcaster;
use crate::usage::{CostSource, UsageInput, record_usage};

/// `signal.throwIfAborted()`'s message, which the review reports as its generic reason.
const ABORTED: &str = "This operation was aborted";
const UNSUPPORTED: &str = "Enforced review currently requires an explicitly assigned Claude API account. Subscription and Codex review isolation is unavailable.";

/// `reviewAccountResolver(db, accounts, broadcast)` (project-review-account.ts:10).
struct Resolver {
    db: Db,
    accounts: AccountRuntime,
    broadcast: Broadcaster,
}

/// `reviewAccountResolver(db, accounts, broadcast)` (project-review-account.ts:10).
pub fn review_account_resolver(
    db: Db,
    accounts: AccountRuntime,
    broadcast: Broadcaster,
) -> Arc<dyn ReviewAccountResolver> {
    Arc::new(Resolver {
        db,
        accounts,
        broadcast,
    })
}

impl ReviewAccountResolver for Resolver {
    fn resolve<'a>(
        &'a self,
        context: &'a ReviewContext,
        signal: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<ReviewAccountSelection, String>> {
        Box::pin(async move {
            if signal.is_cancelled() {
                return Err(ABORTED.into());
            }
            let bot = {
                let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
                get_stored_bot(&db, &context.bot_id).map_err(|error| error.message)?
            };
            let identity = match self.accounts.inspect(&bot) {
                AccountSelection::NotReady { reason, .. } => {
                    return Ok(ReviewAccountSelection::Unavailable(reason));
                }
                AccountSelection::Ready(identity) => identity,
            };
            if identity.provider != Provider::Claude
                || identity.kind != AccountKind::ApiKey
                || identity.using_fallback
            {
                return Ok(ReviewAccountSelection::Unavailable(UNSUPPORTED.into()));
            }
            let prepared = self
                .accounts
                .prepare(&identity)
                .await
                .map_err(|error| error.message)?;
            let checked = if signal.is_cancelled() {
                Err(ABORTED.to_string())
            } else {
                self.accounts
                    .assert_current(&identity)
                    .map_err(|error| error.message)
            };
            if let Err(error) = checked {
                self.accounts.release(&identity);
                return Err(error);
            }
            Ok(ReviewAccountSelection::Ready(Arc::new(Lease {
                identity,
                accounts: self.accounts.clone(),
                db: self.db.clone(),
                broadcast: self.broadcast.clone(),
                signal: signal.clone(),
                api_key: prepared.api_key,
                released: AtomicBool::new(false),
            })))
        })
    }
}

/// One review's account lease: the `ReviewAccount` object of the TypeScript resolver.
struct Lease {
    identity: AccountRunContext,
    accounts: AccountRuntime,
    db: Db,
    broadcast: Broadcaster,
    signal: CancellationToken,
    api_key: Option<ApiKey>,
    released: AtomicBool,
}

impl ReviewAccount for Lease {
    fn provider(&self) -> Provider {
        self.identity.provider
    }

    fn model(&self) -> &str {
        &self.identity.model
    }

    fn kind(&self) -> AccountKind {
        self.identity.kind
    }

    fn api_key(&self) -> Option<&ApiKey> {
        self.api_key.as_ref()
    }

    /// `getMaxBudgetUsd`: the cap less what is known spent, floored at zero; `None` without a cap.
    fn max_budget_usd(&self) -> Result<Option<f64>, String> {
        self.accounts
            .assert_current(&self.identity)
            .map_err(|error| error.message)?;
        let spending = self
            .accounts
            .info(&self.identity.effective_account_id)
            .map_err(|error| error.message)?
            .spending;
        Ok(spending
            .cap_usd
            .map(|cap| (cap - spending.known_usd).max(0.0)))
    }

    fn assert_current(&self) -> Result<(), String> {
        if self.signal.is_cancelled() {
            return Err(ABORTED.into());
        }
        self.accounts
            .assert_current(&self.identity)
            .map_err(|error| error.message)
    }

    /// `onEvent`: limits update the account's state; usage is charged once per stable event id.
    fn on_event(&self, event: ProviderEvent) -> Result<(), String> {
        match event {
            ProviderEvent::Limit(limit) => {
                self.accounts.observe_limit(&self.identity, &limit);
                Ok(())
            }
            ProviderEvent::Usage(usage) => self.record(&usage),
            _ => Ok(()),
        }
    }

    fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            self.accounts.release(&self.identity);
        }
    }
}

impl Lease {
    /// The usage branch of `onEvent`: a reported cost is kept; without one, the local estimate (if any) is recorded.
    fn record(&self, usage: &Usage) -> Result<(), String> {
        let estimated = match usage.cost_usd {
            Some(_) => None,
            None => price_usage(self.identity.provider, &usage.model, usage.tokens),
        };
        let input = UsageInput {
            id: usage
                .event_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| format!("review:{}:{id}", self.identity.run_id)),
            bot_id: self.identity.bot_id.clone(),
            account_id: self.identity.effective_account_id.clone(),
            provider: self.identity.provider,
            model: usage.model.clone(),
            input: usage.tokens.input,
            cached_input: usage.tokens.cached_input,
            output: usage.tokens.output,
            reasoning: usage.tokens.reasoning,
            cost_usd: usage.cost_usd.or(estimated.map(|priced| priced.cost_usd)),
            cost_source: match (usage.cost_usd, estimated) {
                (Some(_), _) => CostSource::Reported,
                (None, Some(_)) => CostSource::Estimated,
                (None, None) => CostSource::Unknown,
            },
            pricing_revision: estimated.map(|priced| priced.pricing_revision.to_string()),
            created_at: None,
        };
        let inserted = {
            let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            let transaction = db.unchecked_transaction().map_err(|error| error.to_string())?;
            let inserted = record_usage(&transaction, &input).map_err(|error| error.message)?;
            transaction.commit().map_err(|error| error.to_string())?;
            inserted
        };
        if inserted {
            self.accounts
                .usage_changed(&self.identity.effective_account_id);
            self.broadcast.send(Event::UsageChanged(UsageChanged {
                account_id: self.identity.effective_account_id.clone(),
                bot_id: self.identity.bot_id.clone(),
            }));
        }
        Ok(())
    }
}
