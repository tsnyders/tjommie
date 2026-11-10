//! Port of `packages/daemon/src/accounts-store.test.ts`. The database is the test schema until lane A's `db::open`
//! lands.

mod common;

use daemon::accounts_store::{
    AccountInput, AccountPatch, account_budget, bind_legacy_bots, claim_cap_notification,
    create_account, get_account, get_default_account, get_fallback_account, list_account_limits,
    list_accounts, remove_account, replace_account_limits, update_account,
};
use daemon::providers::{LimitState, LimitWindow};
use daemon::usage::{CostSource, UsageInput, record_usage};
use protocol::accounts::{AccountKind, AccountSpending};
use protocol::model::Provider;
use rusqlite::Connection;

fn db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let db = common::test_db(dir.path());
    (dir, db)
}

fn subscription(db: &Connection, id: &str) {
    create_account(
        db,
        AccountInput {
            id: Some(id.into()),
            provider: Provider::Claude,
            label: id.into(),
            kind: AccountKind::Subscription,
            folder: format!("test/{id}"),
            credential_ref: None,
            is_default: false,
            is_fallback: false,
            spending_cap_usd: None,
        },
    )
    .unwrap();
}

fn key(db: &Connection, id: &str) {
    create_account(
        db,
        AccountInput {
            id: Some(id.into()),
            provider: Provider::Claude,
            label: id.into(),
            kind: AccountKind::ApiKey,
            folder: format!("test/{id}"),
            credential_ref: Some(format!("vault:{id}")),
            is_default: false,
            is_fallback: false,
            spending_cap_usd: Some(10.0),
        },
    )
    .unwrap();
}

fn bot_account(db: &Connection) -> Option<String> {
    db.query_row("SELECT account_id FROM bots WHERE id = 'bot'", [], |row| {
        row.get(0)
    })
    .unwrap()
}

const INSERT_BOT: &str = "INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
    VALUES ('bot', 'Test', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'synthetic', 'then')";

fn window(
    key: &str,
    duration: Option<f64>,
    used: f64,
    resets_at: Option<&str>,
    state: LimitState,
) -> LimitWindow {
    LimitWindow {
        window_key: key.into(),
        duration_minutes: duration,
        used_percent: Some(used),
        resets_at: resets_at.map(str::to_string),
        state,
    }
}

#[test]
fn assigns_defaults_to_accounts_inserted_after_fresh_migration_without_reviving_tombstones() {
    let (_dir, db) = db();
    db.execute_batch(&format!(
        "INSERT INTO accounts (id, provider, label, kind, folder, created_at, deleted_at)
           VALUES ('old', 'claude', 'Removed', 'subscription', 'test/old', '2020-01-01', '2026-01-01');
         INSERT INTO accounts (id, provider, label, kind, folder, created_at)
           VALUES ('claude-main', 'claude', 'Claude', 'subscription', 'test/claude-main', '2026-01-01'),
             ('claude-second', 'claude', 'Second', 'subscription', 'test/claude-second', '2026-02-01'),
             ('codex-main', 'codex', 'Codex', 'subscription', 'test/codex-main', '2026-01-01');
         {INSERT_BOT};"
    ))
    .unwrap();
    assert!(
        list_accounts(&db, false)
            .unwrap()
            .iter()
            .all(|entry| !entry.is_default)
    );
    bind_legacy_bots(&db).unwrap();
    let defaults: Vec<String> = list_accounts(&db, false)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.is_default)
        .map(|entry| entry.id)
        .collect();
    assert_eq!(defaults, ["claude-main", "codex-main"]);
    let old = get_account(&db, "old", true).unwrap();
    assert_eq!(
        (old.is_default, old.deleted_at.as_deref()),
        (false, Some("2026-01-01"))
    );
    assert_eq!(bot_account(&db).as_deref(), Some("claude-main"));
    update_account(
        &db,
        "claude-second",
        AccountPatch {
            is_default: Some(true),
            ..AccountPatch::default()
        },
    )
    .unwrap();
    bind_legacy_bots(&db).unwrap();
    assert_eq!(
        get_default_account(&db, Provider::Claude).unwrap().id,
        "claude-second"
    );
    assert_eq!(bot_account(&db).as_deref(), Some("claude-main"));
}

#[test]
fn keeps_defaults_deterministic_and_tombstones_without_removing_history_metadata() {
    let (_dir, db) = db();
    subscription(&db, "main");
    subscription(&db, "other");
    assert_eq!(
        get_default_account(&db, Provider::Claude).unwrap().id,
        "main"
    );
    remove_account(&db, "main").unwrap();
    assert!(get_account(&db, "main", false).is_err());
    assert_eq!(get_account(&db, "main", true).unwrap().folder, "test/main");
    assert_eq!(
        get_default_account(&db, Provider::Claude).unwrap().id,
        "other"
    );
    assert_eq!(list_accounts(&db, false).unwrap().len(), 1);
    assert_eq!(list_accounts(&db, true).unwrap().len(), 2);
    remove_account(&db, "main").unwrap();
}

#[test]
fn allows_one_active_api_fallback_and_requires_a_positive_finite_cap() {
    let (_dir, db) = db();
    subscription(&db, "main");
    key(&db, "key");
    key(&db, "other");
    let fallback = || AccountPatch {
        is_fallback: Some(true),
        ..AccountPatch::default()
    };
    update_account(&db, "key", fallback()).unwrap();
    update_account(&db, "other", fallback()).unwrap();
    assert_eq!(
        get_fallback_account(&db, Provider::Claude)
            .unwrap()
            .map(|account| account.id)
            .as_deref(),
        Some("other")
    );
    assert!(!get_account(&db, "key", false).unwrap().is_fallback);
    assert!(update_account(&db, "main", fallback()).is_err());
    for cap in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::INFINITY),
        Some(f64::NAN),
    ] {
        assert!(
            update_account(
                &db,
                "key",
                AccountPatch {
                    spending_cap_usd: Some(cap),
                    ..AccountPatch::default()
                }
            )
            .is_err(),
            "{cap:?}"
        );
    }
}

#[test]
fn changes_defaults_atomically_and_refuses_removal_of_pinned_accounts() {
    let (_dir, db) = db();
    subscription(&db, "main");
    subscription(&db, "second");
    let make_default = || AccountPatch {
        is_default: Some(true),
        ..AccountPatch::default()
    };
    update_account(&db, "second", make_default()).unwrap();
    assert_eq!(
        get_default_account(&db, Provider::Claude).unwrap().id,
        "second"
    );
    assert!(!get_account(&db, "main", false).unwrap().is_default);
    db.execute_batch(INSERT_BOT).unwrap();
    bind_legacy_bots(&db).unwrap();
    assert_eq!(bot_account(&db).as_deref(), Some("second"));
    assert!(
        remove_account(&db, "second")
            .unwrap_err()
            .message
            .contains("Move")
    );
    update_account(&db, "main", make_default()).unwrap();
    bind_legacy_bots(&db).unwrap();
    assert_eq!(bot_account(&db).as_deref(), Some("second"));
    let duplicate = AccountInput {
        id: Some("main".into()),
        provider: Provider::Claude,
        label: "Duplicate".into(),
        kind: AccountKind::Subscription,
        folder: "test/duplicate".into(),
        credential_ref: None,
        is_default: true,
        is_fallback: false,
        spending_cap_usd: None,
    };
    assert!(create_account(&db, duplicate).is_err());
    assert_eq!(
        get_default_account(&db, Provider::Claude).unwrap().id,
        "main"
    );
}

#[test]
fn keeps_account_limit_windows_separate_and_ignores_older_snapshots() {
    let (_dir, db) = db();
    subscription(&db, "main");
    let windows = [
        window(
            "primary",
            Some(300.0),
            10.0,
            Some("2026-10-22T13:00:00.000Z"),
            LimitState::Ok,
        ),
        window(
            "secondary",
            Some(10080.0),
            100.0,
            Some("2026-10-28T13:00:00.000Z"),
            LimitState::Reached,
        ),
    ];
    replace_account_limits(&db, "main", "codex", "2026-10-22T12:00:00.000Z", &windows).unwrap();
    assert_eq!(
        get_account(&db, "main", false)
            .unwrap()
            .limited_until
            .as_deref(),
        Some("2026-10-28T13:00:00.000Z")
    );
    replace_account_limits(&db, "main", "codex", "2026-10-21T12:00:00.000Z", &[]).unwrap();
    assert_eq!(list_account_limits(&db, "main").unwrap().len(), 2);
    let unknown_reset = [window("secondary", None, 100.0, None, LimitState::Reached)];
    replace_account_limits(
        &db,
        "main",
        "codex",
        "2026-10-22T13:00:00.000Z",
        &unknown_reset,
    )
    .unwrap();
    let limits = list_account_limits(&db, "main").unwrap();
    assert_eq!(
        limits
            .iter()
            .map(|limit| (limit.state, limit.resets_at.clone()))
            .collect::<Vec<_>>(),
        vec![(protocol::accounts::LimitState::Reached, None)]
    );
    assert_eq!(get_account(&db, "main", false).unwrap().limited_until, None);
    replace_account_limits(&db, "main", "codex", "2026-10-22T14:00:00.000Z", &[]).unwrap();
    replace_account_limits(
        &db,
        "main",
        "codex",
        "2026-10-22T13:30:00.000Z",
        &unknown_reset,
    )
    .unwrap();
    assert!(list_account_limits(&db, "main").unwrap().is_empty());
}

fn cost(id: Option<&str>, cost_usd: Option<f64>, created_at: Option<&str>) -> UsageInput {
    UsageInput {
        id: id.map(str::to_string),
        bot_id: "deleted-bot".into(),
        account_id: "key".into(),
        provider: Provider::Claude,
        model: "m".into(),
        input: 1.0,
        cached_input: 0.0,
        output: 1.0,
        reasoning: 0.0,
        cost_usd,
        cost_source: if cost_usd.is_some() {
            CostSource::Reported
        } else {
            CostSource::Unknown
        },
        pricing_revision: None,
        created_at: created_at.map(str::to_string),
    }
}

#[test]
fn uses_lifetime_local_spend_blocks_unknown_costs_and_claims_each_cap_threshold_once_per_revision()
{
    let (_dir, db) = db();
    key(&db, "key");
    assert!(!account_budget(&db, "key").unwrap().blocked);
    record_usage(&db, &cost(None, Some(8.0), None)).unwrap();
    assert_eq!(claim_cap_notification(&db, "key", 80), Ok(true));
    assert_eq!(claim_cap_notification(&db, "key", 80), Ok(false));
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(false));
    record_usage(&db, &cost(None, None, None)).unwrap();
    let budget = account_budget(&db, "key").unwrap();
    assert_eq!(
        (budget.known_usd, budget.unknown_rows, budget.blocked),
        (8.0, 1, true)
    );
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(true));
    update_account(
        &db,
        "key",
        AccountPatch {
            spending_cap_usd: Some(Some(20.0)),
            ..AccountPatch::default()
        },
    )
    .unwrap();
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(true));
    remove_account(&db, "key").unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn preserves_cap_notification_claims_across_restart_without_resetting_lifetime_spend() {
    let (dir, db) = db();
    key(&db, "key");
    record_usage(
        &db,
        &cost(Some("cost-event"), Some(10.0), Some("2020-01-01T00:00:00Z")),
    )
    .unwrap();
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(true));
    drop(db);
    let db = Connection::open(dir.path().join("t.db")).unwrap();
    assert_eq!(
        account_budget(&db, "key").unwrap(),
        AccountSpending {
            known_usd: 10.0,
            unknown_rows: 0,
            cap_usd: Some(10.0),
            blocked: true
        }
    );
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(false));
    update_account(
        &db,
        "key",
        AccountPatch {
            label: Some("Renamed".into()),
            spending_cap_usd: Some(Some(10.0)),
            ..AccountPatch::default()
        },
    )
    .unwrap();
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(false));
    update_account(
        &db,
        "key",
        AccountPatch {
            spending_cap_usd: Some(Some(5.0)),
            ..AccountPatch::default()
        },
    )
    .unwrap();
    assert_eq!(claim_cap_notification(&db, "key", 100), Ok(true));
}

#[test]
fn rejects_invalid_quota_replacements_without_losing_the_last_valid_account_state() {
    let (_dir, db) = db();
    subscription(&db, "main");
    subscription(&db, "other");
    let reached = window(
        "primary",
        Some(300.0),
        100.0,
        Some("2026-10-22T14:00:00Z"),
        LimitState::Reached,
    );
    replace_account_limits(
        &db,
        "main",
        "model-1",
        "2026-10-22T12:00:00Z",
        std::slice::from_ref(&reached),
    )
    .unwrap();
    let invalid = LimitWindow {
        used_percent: Some(101.0),
        ..reached.clone()
    };
    assert!(
        replace_account_limits(&db, "main", "model-1", "2026-10-22T13:00:00Z", &[invalid]).is_err()
    );
    let limits = list_account_limits(&db, "main").unwrap();
    assert_eq!(
        limits
            .iter()
            .map(|limit| (limit.used_percent, limit.limit_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(Some(100.0), "model-1")]
    );
    let weekly = LimitWindow {
        window_key: "weekly".into(),
        resets_at: Some("2026-10-28T00:00:00Z".into()),
        ..reached
    };
    replace_account_limits(&db, "main", "model-2", "2026-10-22T12:00:00Z", &[weekly]).unwrap();
    assert_eq!(
        get_account(&db, "main", false)
            .unwrap()
            .limited_until
            .as_deref(),
        Some("2026-10-28T00:00:00.000Z")
    );
    assert!(list_account_limits(&db, "other").unwrap().is_empty());
}
