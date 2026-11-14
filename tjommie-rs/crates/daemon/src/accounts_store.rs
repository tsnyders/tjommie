//! `accounts-store.ts`: provider accounts, their provider-limit windows and spending-cap notifications.

use protocol::ErrorCode;
use protocol::accounts::{AccountKind, AccountLimitWindow};
use protocol::model::Provider;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::ids::{new_id, now_iso};
use crate::providers::{LimitState, LimitWindow};
use crate::server::RpcError;
pub use crate::usage::account_budget;

#[derive(Debug, Clone, PartialEq)]
pub struct StoredAccount {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub kind: AccountKind,
    pub folder: String,
    pub credential_ref: Option<String>,
    pub is_default: bool,
    pub is_fallback: bool,
    pub spending_cap_usd: Option<f64>,
    pub limited_until: Option<String>,
    pub created_at: String,
    pub deleted_at: Option<String>,
    pub cap_revision: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountInput {
    pub id: Option<String>,
    pub provider: Provider,
    pub label: String,
    pub kind: AccountKind,
    pub folder: String,
    pub credential_ref: Option<String>,
    pub is_default: bool,
    pub is_fallback: bool,
    pub spending_cap_usd: Option<f64>,
}

/// Only the set fields change. `Some(None)` clears a credential reference or a cap.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountPatch {
    pub label: Option<String>,
    pub credential_ref: Option<Option<String>>,
    pub is_default: Option<bool>,
    pub is_fallback: Option<bool>,
    pub spending_cap_usd: Option<Option<f64>>,
}

pub(crate) fn bad(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadParams, message)
}

pub fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

pub(crate) fn provider_from(name: &str) -> rusqlite::Result<Provider> {
    match name {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        other => Err(rusqlite::Error::InvalidColumnType(
            0,
            format!("provider {other}"),
            rusqlite::types::Type::Text,
        )),
    }
}

pub fn kind_name(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::Subscription => "subscription",
        AccountKind::ApiKey => "api_key",
    }
}

fn kind_from(name: &str) -> rusqlite::Result<AccountKind> {
    match name {
        "subscription" => Ok(AccountKind::Subscription),
        "api_key" => Ok(AccountKind::ApiKey),
        other => Err(rusqlite::Error::InvalidColumnType(
            0,
            format!("account kind {other}"),
            rusqlite::types::Type::Text,
        )),
    }
}

pub(crate) fn state_name(state: LimitState) -> &'static str {
    match state {
        LimitState::Ok => "ok",
        LimitState::Near => "near",
        LimitState::Reached => "reached",
    }
}

const COLUMNS: &str = "id, provider, label, kind, folder, credential_ref, is_default, is_fallback, spending_cap_usd, limited_until, created_at, deleted_at, cap_revision";

fn account(row: &Row) -> rusqlite::Result<StoredAccount> {
    Ok(StoredAccount {
        id: row.get(0)?,
        provider: provider_from(&row.get::<_, String>(1)?)?,
        label: row.get(2)?,
        kind: kind_from(&row.get::<_, String>(3)?)?,
        folder: row.get(4)?,
        credential_ref: row.get(5)?,
        is_default: row.get::<_, i64>(6)? == 1,
        is_fallback: row.get::<_, i64>(7)? == 1,
        spending_cap_usd: row.get(8)?,
        limited_until: row.get(9)?,
        created_at: row.get(10)?,
        deleted_at: row.get(11)?,
        cap_revision: row.get(12)?,
    })
}

/// `new Date(text).toISOString()`, read as `Date.parse` does.
pub(crate) fn iso(text: &str) -> Option<String> {
    crate::providers::js::iso_from_millis(crate::providers::js::date_parse(text)?)
}

pub fn list_accounts(
    db: &Connection,
    include_deleted: bool,
) -> Result<Vec<StoredAccount>, RpcError> {
    let filter = if include_deleted {
        ""
    } else {
        "WHERE deleted_at IS NULL"
    };
    let mut statement = db.prepare(&format!(
        "SELECT {COLUMNS} FROM accounts {filter} ORDER BY created_at, id"
    ))?;
    let accounts = statement
        .query_map([], account)?
        .collect::<Result<_, _>>()?;
    Ok(accounts)
}

pub fn get_account(
    db: &Connection,
    id: &str,
    include_deleted: bool,
) -> Result<StoredAccount, RpcError> {
    let filter = if include_deleted {
        ""
    } else {
        "AND deleted_at IS NULL"
    };
    db.query_row(
        &format!("SELECT {COLUMNS} FROM accounts WHERE id = ?1 {filter}"),
        [id],
        account,
    )
    .optional()?
    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "Account not found"))
}

pub fn get_default_account(db: &Connection, provider: Provider) -> Result<StoredAccount, RpcError> {
    let found: Vec<StoredAccount> = list_accounts(db, false)?
        .into_iter()
        .filter(|entry| entry.provider == provider)
        .collect();
    let selected = found
        .iter()
        .find(|entry| entry.is_default)
        .or(found.first())
        .cloned();
    selected.ok_or_else(|| {
        RpcError::new(
            ErrorCode::NoAccount,
            format!("No {} account", provider_name(provider)),
        )
    })
}

pub fn get_fallback_account(
    db: &Connection,
    provider: Provider,
) -> Result<Option<StoredAccount>, RpcError> {
    Ok(list_accounts(db, false)?
        .into_iter()
        .find(|entry| entry.provider == provider && entry.is_fallback))
}

fn validate(
    label: &str,
    kind: AccountKind,
    cap: Option<f64>,
    is_fallback: bool,
    credential_ref: Option<&str>,
) -> Result<(), RpcError> {
    let label = label.trim();
    if label.is_empty() || label.encode_utf16().count() > 60 {
        return Err(bad("Choose an account name of 1 to 60 characters"));
    }
    if cap.is_some_and(|cap| !cap.is_finite() || cap <= 0.0 || cap > 1_000_000.0) {
        return Err(bad(
            "Spending cap must be a positive finite amount up to 1,000,000 USD",
        ));
    }
    if kind == AccountKind::ApiKey && cap.is_none() {
        return Err(bad("API keys require a spending cap"));
    }
    if kind == AccountKind::Subscription
        && (is_fallback || credential_ref.is_some() || cap.is_some())
    {
        return Err(bad("Subscription accounts cannot use API-key settings"));
    }
    if credential_ref.is_some_and(|reference| {
        reference.trim().is_empty() || reference.encode_utf16().count() > 512
    }) {
        return Err(bad("Invalid credential reference"));
    }
    Ok(())
}

pub fn create_account(db: &Connection, input: AccountInput) -> Result<StoredAccount, RpcError> {
    if input.folder.trim().is_empty() {
        return Err(bad("Account folder is required"));
    }
    let id = input.id.unwrap_or_else(new_id);
    let label = input.label.trim().to_string();
    if id.trim().is_empty() {
        return Err(bad("Account id is required"));
    }
    validate(
        &label,
        input.kind,
        input.spending_cap_usd,
        input.is_fallback,
        input.credential_ref.as_deref(),
    )?;
    let transaction = db.unchecked_transaction()?;
    let provider = provider_name(input.provider);
    let is_default = input.is_default
        || !list_accounts(&transaction, false)?
            .iter()
            .any(|entry| entry.provider == input.provider);
    if is_default {
        transaction.execute(
            "UPDATE accounts SET is_default = 0 WHERE provider = ?1",
            [provider],
        )?;
    }
    if input.is_fallback {
        transaction.execute(
            "UPDATE accounts SET is_fallback = 0 WHERE provider = ?1",
            [provider],
        )?;
    }
    transaction.execute(
        "INSERT INTO accounts (id, provider, label, kind, folder, created_at, credential_ref, is_default, is_fallback, spending_cap_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            provider,
            label,
            kind_name(input.kind),
            input.folder,
            now_iso(),
            input.credential_ref,
            is_default,
            input.is_fallback,
            input.spending_cap_usd
        ],
    )?;
    transaction.commit()?;
    get_account(db, &id, false)
}

pub fn update_account(
    db: &Connection,
    id: &str,
    patch: AccountPatch,
) -> Result<StoredAccount, RpcError> {
    let current = get_account(db, id, false)?;
    if current.is_default && patch.is_default == Some(false) {
        return Err(bad(
            "Choose another default account before changing this one",
        ));
    }
    let label = patch
        .label
        .map_or_else(|| current.label.clone(), |label| label.trim().to_string());
    let credential_ref = patch
        .credential_ref
        .unwrap_or_else(|| current.credential_ref.clone());
    let is_default = patch.is_default.unwrap_or(current.is_default);
    let is_fallback = patch.is_fallback.unwrap_or(current.is_fallback);
    let cap = patch.spending_cap_usd.unwrap_or(current.spending_cap_usd);
    validate(
        &label,
        current.kind,
        cap,
        is_fallback,
        credential_ref.as_deref(),
    )?;
    let transaction = db.unchecked_transaction()?;
    let provider = provider_name(current.provider);
    if is_default {
        transaction.execute(
            "UPDATE accounts SET is_default = 0 WHERE provider = ?1",
            [provider],
        )?;
    }
    if is_fallback {
        transaction.execute(
            "UPDATE accounts SET is_fallback = 0 WHERE provider = ?1",
            [provider],
        )?;
    }
    let cap_revision = current.cap_revision + i64::from(cap != current.spending_cap_usd);
    transaction.execute(
        "UPDATE accounts SET label = ?1, credential_ref = ?2, is_default = ?3, is_fallback = ?4, spending_cap_usd = ?5, cap_revision = ?6 WHERE id = ?7",
        params![label, credential_ref, is_default, is_fallback, cap, cap_revision, id],
    )?;
    transaction.commit()?;
    get_account(db, id, false)
}

pub fn remove_account(db: &Connection, id: &str) -> Result<(), RpcError> {
    let current = get_account(db, id, true)?;
    if current.deleted_at.is_some() {
        return Ok(());
    }
    if db
        .query_row(
            "SELECT 1 FROM bots WHERE account_id = ?1 LIMIT 1",
            [id],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Err(RpcError::new(
            ErrorCode::Busy,
            "Move this account's agents before removing it",
        ));
    }
    let transaction = db.unchecked_transaction()?;
    transaction.execute(
        "UPDATE accounts SET deleted_at = ?1, is_default = 0, is_fallback = 0 WHERE id = ?2",
        params![now_iso(), id],
    )?;
    if current.is_default
        && let Some(replacement) = list_accounts(&transaction, false)?
            .into_iter()
            .find(|entry| entry.provider == current.provider)
    {
        transaction.execute(
            "UPDATE accounts SET is_default = 1 WHERE id = ?1",
            [replacement.id],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Fresh installations seed the legacy accounts after migration. Repairs only missing defaults, leaving explicit
/// choices and retired account identities untouched, then pins unpinned bots.
pub fn bind_legacy_bots(db: &Connection) -> Result<(), RpcError> {
    db.execute_batch(
        "UPDATE accounts SET is_default = 1 WHERE deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.provider = accounts.provider AND a.deleted_at IS NULL AND a.is_default = 1)
           AND id = (SELECT a.id FROM accounts a WHERE a.provider = accounts.provider AND a.deleted_at IS NULL ORDER BY a.created_at, a.id LIMIT 1);
         UPDATE bots SET account_id = COALESCE(
           (SELECT a.id FROM sessions s JOIN accounts a ON a.id = s.account_id WHERE s.bot_id = bots.id AND s.provider = bots.provider AND a.provider = bots.provider AND a.deleted_at IS NULL ORDER BY s.updated_at DESC, a.id LIMIT 1),
           (SELECT a.id FROM accounts a WHERE a.provider = bots.provider AND a.deleted_at IS NULL ORDER BY a.is_default DESC, a.created_at, a.id LIMIT 1))
           WHERE account_id IS NULL;",
    )?;
    Ok(())
}

fn limit_state_from(name: &str) -> rusqlite::Result<protocol::accounts::LimitState> {
    match name {
        "ok" => Ok(protocol::accounts::LimitState::Ok),
        "near" => Ok(protocol::accounts::LimitState::Near),
        "reached" => Ok(protocol::accounts::LimitState::Reached),
        other => Err(rusqlite::Error::InvalidColumnType(
            0,
            format!("limit state {other}"),
            rusqlite::types::Type::Text,
        )),
    }
}

pub fn list_account_limits(
    db: &Connection,
    account_id: &str,
) -> Result<Vec<AccountLimitWindow>, RpcError> {
    get_account(db, account_id, false)?;
    let mut statement = db.prepare(
        "SELECT account_id, limit_id, window_key, duration_minutes, used_percent, resets_at, state, observed_at FROM account_limit_windows
         WHERE account_id = ?1 ORDER BY limit_id, window_key",
    )?;
    let limits = statement
        .query_map([account_id], |row| {
            Ok(AccountLimitWindow {
                account_id: row.get(0)?,
                limit_id: row.get(1)?,
                window_key: row.get(2)?,
                duration_minutes: row.get(3)?,
                used_percent: row.get(4)?,
                resets_at: row.get(5)?,
                state: limit_state_from(&row.get::<_, String>(6)?)?,
                observed_at: row.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(limits)
}

/// Replaces one limit's windows with a newer snapshot, ignoring older ones, and recomputes the account's
/// `limited_until`: the latest known reset of a reached window, or none if any reached reset is unknown.
pub fn replace_account_limits(
    db: &Connection,
    account_id: &str,
    limit_id: &str,
    observed_at: &str,
    windows: &[LimitWindow],
) -> Result<(), RpcError> {
    get_account(db, account_id, false)?;
    let observed_at = iso(observed_at)
        .filter(|_| !limit_id.trim().is_empty())
        .ok_or_else(|| bad("Invalid limit snapshot"))?;
    let mut keys = std::collections::HashSet::new();
    let mut checked = Vec::with_capacity(windows.len());
    for window in windows {
        if window.window_key.trim().is_empty() || !keys.insert(window.window_key.as_str()) {
            return Err(bad("Quota window identities must be unique"));
        }
        if window
            .used_percent
            .is_some_and(|used| !used.is_finite() || !(0.0..=100.0).contains(&used))
        {
            return Err(bad("Invalid quota percentage"));
        }
        if window
            .duration_minutes
            .is_some_and(|minutes| !minutes.is_finite() || minutes <= 0.0)
        {
            return Err(bad("Invalid quota duration"));
        }
        let resets_at = match &window.resets_at {
            None => None,
            Some(reset) => Some(iso(reset).ok_or_else(|| bad("Invalid quota reset"))?),
        };
        checked.push(LimitWindow {
            resets_at,
            ..window.clone()
        });
    }
    let transaction = db.unchecked_transaction()?;
    let previous: Option<String> = transaction
        .query_row(
            "SELECT observed_at FROM account_limit_snapshots WHERE account_id = ?1 AND limit_id = ?2",
            [account_id, limit_id],
            |row| row.get(0),
        )
        .optional()?;
    if previous.is_some_and(|previous| previous >= observed_at) {
        return Ok(());
    }
    transaction.execute(
        "INSERT INTO account_limit_snapshots VALUES (?1, ?2, ?3) ON CONFLICT(account_id, limit_id) DO UPDATE SET observed_at = excluded.observed_at",
        [account_id, limit_id, &observed_at],
    )?;
    transaction.execute(
        "DELETE FROM account_limit_windows WHERE account_id = ?1 AND limit_id = ?2",
        [account_id, limit_id],
    )?;
    for window in &checked {
        transaction.execute(
            "INSERT INTO account_limit_windows VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                account_id,
                limit_id,
                window.window_key,
                window.duration_minutes,
                window.used_percent,
                window.resets_at,
                state_name(window.state),
                observed_at
            ],
        )?;
    }
    let reached: Vec<Option<String>> = list_account_limits(&transaction, account_id)?
        .into_iter()
        .filter(|window| window.state == protocol::accounts::LimitState::Reached)
        .map(|window| window.resets_at)
        .collect();
    let until = if reached.is_empty() || reached.contains(&None) {
        None
    } else {
        reached.into_iter().flatten().max()
    };
    transaction.execute(
        "UPDATE accounts SET limited_until = ?1 WHERE id = ?2",
        params![until, account_id],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Claims the 80% or 100% spending notification for the account's current cap, once.
pub fn claim_cap_notification(db: &Connection, id: &str, threshold: u8) -> Result<bool, RpcError> {
    if threshold != 80 && threshold != 100 {
        return Err(bad("Invalid spending threshold"));
    }
    let current = get_account(db, id, false)?;
    let Some(cap) = current
        .spending_cap_usd
        .filter(|_| current.kind == AccountKind::ApiKey)
    else {
        return Ok(false);
    };
    let budget = account_budget(db, id)?;
    if !(budget.known_usd >= cap * f64::from(threshold) / 100.0
        || (threshold == 100 && budget.unknown_rows > 0))
    {
        return Ok(false);
    }
    let inserted = db.execute(
        "INSERT OR IGNORE INTO account_cap_notifications VALUES (?1, ?2, ?3, ?4)",
        params![id, current.cap_revision, threshold, now_iso()],
    )?;
    Ok(inserted == 1)
}
