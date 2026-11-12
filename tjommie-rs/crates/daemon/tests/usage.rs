//! Port of `packages/daemon/src/usage.test.ts`. The database is the test schema until lane A's `db::open` lands.

mod common;

use daemon::accounts_store::{AccountInput, create_account, remove_account};
use daemon::usage::{CostSource, UsageInput, query_usage, record_usage};
use protocol::accounts::{AccountKind, UsageQuery, UsageReport};
use protocol::model::Provider;
use rusqlite::Connection;
use serde_json::{Value, json};

fn db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let db = common::test_db(dir.path());
    let account = |id: &str, kind, cap| AccountInput {
        id: Some(id.into()),
        provider: Provider::Codex,
        label: if kind == AccountKind::Subscription {
            "Subscription".into()
        } else {
            "API".into()
        },
        kind,
        folder: format!("test/{id}"),
        credential_ref: None,
        is_default: false,
        is_fallback: false,
        spending_cap_usd: cap,
    };
    create_account(&db, account("sub", AccountKind::Subscription, None)).unwrap();
    create_account(&db, account("key", AccountKind::ApiKey, Some(10.0))).unwrap();
    (dir, db)
}

fn input() -> UsageInput {
    UsageInput {
        id: None,
        bot_id: "bot".into(),
        account_id: "sub".into(),
        provider: Provider::Codex,
        model: "model".into(),
        input: 10.0,
        cached_input: 5.0,
        output: 3.0,
        reasoning: 2.0,
        cost_usd: None,
        cost_source: CostSource::Unknown,
        pricing_revision: None,
        created_at: Some("2026-10-22T23:30:00.000Z".into()),
    }
}

fn query(extra: Value) -> UsageQuery {
    let mut value = json!({ "from": "2026-10-22T00:00:00Z", "to": "2026-10-24T00:00:00Z" });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).expect("a valid usage query")
}

fn report(db: &Connection, extra: Value) -> UsageReport {
    query_usage(db, &query(extra)).unwrap()
}

#[test]
fn separates_subscription_value_from_api_spend_and_never_converts_unknown_into_zero() {
    let (_dir, db) = db();
    record_usage(
        &db,
        &UsageInput {
            cost_usd: Some(2.0),
            cost_source: CostSource::Estimated,
            ..input()
        },
    )
    .unwrap();
    record_usage(
        &db,
        &UsageInput {
            account_id: "key".into(),
            cost_usd: Some(0.0),
            cost_source: CostSource::Reported,
            ..input()
        },
    )
    .unwrap();
    record_usage(
        &db,
        &UsageInput {
            account_id: "key".into(),
            ..input()
        },
    )
    .unwrap();
    let result = report(&db, json!({}));
    let tokens = &result.summary.tokens;
    assert_eq!(
        (
            tokens.input,
            tokens.cached_input,
            tokens.output,
            tokens.reasoning,
            tokens.total
        ),
        (30, 15, 9, 6, 54)
    );
    assert_eq!(
        (
            result.summary.costs.api_value.usd,
            result.summary.costs.api_value.estimated_usd
        ),
        (Some(2.0), Some(2.0))
    );
    let spend = &result.summary.costs.api_spend;
    assert_eq!(
        (spend.usd, spend.known_usd, spend.unknown_records),
        (None, Some(0.0), 1)
    );
    assert_eq!(result.by_account.len(), 2);
    assert_eq!(
        report(&db, json!({ "accountId": "sub" })).summary.records,
        1
    );
}

#[test]
fn uses_local_date_buckets_through_dst_and_inclusive_exclusive_utc_boundaries() {
    let (_dir, db) = db();
    for created_at in [
        "2026-11-01T05:30:00Z",
        "2026-11-01T06:30:00Z",
        "2026-11-02T05:00:00Z",
    ] {
        record_usage(
            &db,
            &UsageInput {
                created_at: Some(created_at.into()),
                ..input()
            },
        )
        .unwrap();
    }
    let dst: UsageQuery = serde_json::from_value(json!({ "from": "2026-11-01T05:30:00Z", "to": "2026-11-02T05:00:00Z", "timeZone": "America/New_York" })).unwrap();
    let result = query_usage(&db, &dst).unwrap();
    assert_eq!(
        result
            .daily
            .iter()
            .map(|day| (day.day.as_str(), day.usage.records))
            .collect::<Vec<_>>(),
        vec![("2026-11-01", 2)]
    );
    assert!(
        report(&db, json!({ "timeZone": "Africa/Johannesburg" }))
            .daily
            .is_empty()
    );
    record_usage(&db, &input()).unwrap();
    assert_eq!(
        report(&db, json!({ "timeZone": "Africa/Johannesburg" })).daily[0].day,
        "2026-10-23"
    );
}

#[test]
fn validates_timezones_ranges_counts_and_cost_provenance() {
    let (_dir, db) = db();
    for bad in [
        json!({ "from": "2026-10-22T00:00:00Z", "to": "2026-10-24T00:00:00Z", "timeZone": "Mars/Olympus" }),
        json!({ "from": "2026-10-24T00:00:00Z", "to": "2026-10-22T00:00:00Z" }),
        json!({ "from": "2020-01-01T00:00:00Z", "to": "2026-10-24T00:00:00Z" }),
        json!({ "from": "2026-10-22", "to": "2026-10-24T00:00:00Z" }),
    ] {
        assert!(
            serde_json::from_value::<UsageQuery>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    assert!(
        record_usage(
            &db,
            &UsageInput {
                input: -1.0,
                ..input()
            }
        )
        .is_err()
    );
    assert!(
        record_usage(
            &db,
            &UsageInput {
                cost_usd: Some(f64::NAN),
                cost_source: CostSource::Reported,
                ..input()
            }
        )
        .is_err()
    );
    assert!(
        record_usage(
            &db,
            &UsageInput {
                cost_usd: Some(2.0),
                cost_source: CostSource::Unknown,
                ..input()
            }
        )
        .is_err()
    );
}

#[test]
fn leaves_transaction_ownership_with_its_caller() {
    let (_dir, db) = db();
    db.execute_batch("BEGIN").unwrap();
    record_usage(&db, &input()).unwrap();
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(report(&db, json!({})).summary.records, 0);
}

#[test]
fn deduplicates_stable_event_identities_but_rejects_conflicting_deliveries() {
    let (_dir, db) = db();
    let event = || UsageInput {
        id: Some("event".into()),
        ..input()
    };
    assert_eq!(record_usage(&db, &event()), Ok(true));
    assert_eq!(record_usage(&db, &event()), Ok(false));
    assert!(
        record_usage(
            &db,
            &UsageInput {
                input: 20.0,
                ..event()
            }
        )
        .unwrap_err()
        .message
        .contains("Conflicting")
    );
    assert_eq!(report(&db, json!({})).summary.records, 1);
}

#[test]
fn preserves_unclassified_legacy_history_and_absent_pricing_without_making_up_costs() {
    let (_dir, db) = db();
    db.execute_batch(
        "INSERT INTO usage (id, bot_id, account_id, provider, model, input, cached_input, output, reasoning, cost_usd, cost_source, created_at)
         VALUES ('legacy', 'deleted', 'missing', 'claude', 'unknown-model', 1, 0, 2, 1, NULL, 'unknown', '2026-10-22T12:00:00Z')",
    )
    .unwrap();
    record_usage(&db, &input()).unwrap();
    let revision: Option<String> = db
        .query_row(
            "SELECT pricing_revision FROM usage WHERE account_id = 'sub'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, None);
    remove_account(&db, "sub").unwrap();
    record_usage(
        &db,
        &UsageInput {
            id: Some("late-event".into()),
            ..input()
        },
    )
    .unwrap(); // account identity survives retirement
    let result = report(&db, json!({}));
    protocol::assert_valid("usage.query", &serde_json::to_value(&result).unwrap());
    let unclassified = &result.summary.costs.unclassified;
    assert_eq!(
        (
            unclassified.usd,
            unclassified.known_usd,
            unclassified.known_records,
            unclassified.unknown_records
        ),
        (None, None, 0, 1)
    );
    assert_eq!(
        (unclassified.reported_usd, unclassified.estimated_usd),
        (None, None)
    );
    assert_eq!(result.summary.costs.api_value.unknown_records, 2);
    assert!(result.by_bot.iter().any(|group| group.bot_id == "deleted"));
}

#[test]
fn does_not_round_tiny_measured_costs_to_zero_and_applies_every_grouping_filter() {
    let (_dir, db) = db();
    record_usage(
        &db,
        &UsageInput {
            cost_usd: Some(0.000000123),
            cost_source: CostSource::Reported,
            ..input()
        },
    )
    .unwrap();
    record_usage(
        &db,
        &UsageInput {
            bot_id: "other".into(),
            account_id: "key".into(),
            model: "other-model".into(),
            cost_usd: Some(1.0),
            cost_source: CostSource::Estimated,
            pricing_revision: Some("test-prices".into()),
            ..input()
        },
    )
    .unwrap();
    let result = report(
        &db,
        json!({ "botId": "bot", "provider": "codex", "model": "model" }),
    );
    assert_eq!(result.summary.records, 1);
    assert_eq!(
        result.summary.costs.api_value.reported_usd,
        Some(0.000000123)
    );
    assert_eq!(
        result
            .by_model
            .iter()
            .map(|group| (group.provider, group.model.as_str(), group.usage.records))
            .collect::<Vec<_>>(),
        vec![(Provider::Codex, "model", 1)]
    );
    assert_eq!(result.summary.costs.api_spend.known_usd, None);
    assert_eq!(
        report(&db, json!({ "provider": "claude" })).summary.records,
        0
    );
}
