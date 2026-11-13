//! `accounts.test.ts`

use super::{invalid, params, rejects, with};
use protocol::accounts::{AccountInfo, AccountProviderUsage};
use protocol::assert_valid;
use serde_json::{Value, json};

fn account() -> Value {
    json!({ "id": "claude-main", "provider": "claude", "label": "Claude", "kind": "subscription", "isDefault": true, "isFallback": false,
        "spendingCapUsd": null, "createdAt": "2026-10-22T00:00:00.000Z", "hasCredential": false, "setup": null, "limits": [], "signIn": null,
        "spending": { "knownUsd": 0, "unknownRows": 0, "capUsd": null, "blocked": false } })
}

#[test]
fn requires_a_cap_and_secret_only_when_creating_an_api_key_account() {
    assert_eq!(
        params(
            "accounts.create",
            json!({ "provider": "claude", "label": " Work ", "kind": "subscription" })
        )["label"],
        "Work"
    );
    rejects(
        "accounts.create",
        json!({ "provider": "claude", "label": "Work", "kind": "api_key", "apiKey": "synthetic" }),
    );
    rejects(
        "accounts.create",
        json!({ "provider": "claude", "label": "Work", "kind": "subscription", "apiKey": "synthetic" }),
    );
    for spending_cap_usd in [json!(0), json!(-1), Value::Null] {
        rejects(
            "accounts.create",
            json!({ "provider": "codex", "label": "API", "kind": "api_key", "apiKey": "synthetic", "spendingCapUsd": spending_cap_usd }),
        );
    }
}

#[test]
fn does_not_let_account_updates_change_the_provider_kind_or_credential_reference() {
    for patch in [
        json!({ "provider": "claude" }),
        json!({ "kind": "api_key" }),
        json!({ "credentialRef": "private" }),
        json!({ "folder": "C:/private" }),
    ] {
        rejects(
            "accounts.update",
            json!({ "id": "claude-main", "patch": patch }),
        );
    }
}

#[test]
fn bounds_api_keys_by_native_utf8_bytes_as_well_as_rejecting_whitespace() {
    for api_key in [
        "a".repeat(2561),
        "é".repeat(1281),
        "synthetic\nkey".into(),
        "synthetic key".into(),
    ] {
        rejects(
            "accounts.create",
            json!({ "provider": "codex", "label": "API", "kind": "api_key", "apiKey": api_key, "spendingCapUsd": 1 }),
        );
        rejects(
            "accounts.update",
            json!({ "id": "claude-main", "patch": { "apiKey": api_key } }),
        );
    }
    params(
        "accounts.update",
        json!({ "id": "claude-main", "patch": { "apiKey": "é".repeat(1280) } }),
    );
}

#[test]
fn returns_safe_metadata_without_a_submitted_secret_or_private_folder() {
    let submitted = with(
        account(),
        json!({ "apiKey": "synthetic", "credentialRef": "private", "folder": "private" }),
    );
    let value =
        serde_json::to_value(serde_json::from_value::<AccountInfo>(submitted).unwrap()).unwrap();
    for key in ["apiKey", "credentialRef", "folder"] {
        assert!(value.get(key).is_none(), "{key}");
    }
    assert_valid("accounts.list", &json!([value]));
}

#[test]
fn bounds_queries_by_instants_and_validates_the_chart_time_zone() {
    let range = json!({ "from": "2026-10-01T00:00:00.000Z", "to": "2026-11-01T00:00:00.000Z" });
    assert_eq!(params("usage.query", range.clone())["timeZone"], "UTC");
    assert_eq!(
        params(
            "usage.query",
            with(range.clone(), json!({ "timeZone": "Africa/Johannesburg" }))
        )["timeZone"],
        "Africa/Johannesburg"
    );
    for bad in [
        with(range.clone(), json!({ "timeZone": "invented/zone" })),
        with(range.clone(), json!({ "to": range["from"] })),
        with(range.clone(), json!({ "to": "2028-01-01T00:00:00.000Z" })),
        with(range, json!({ "from": "yesterday" })),
    ] {
        rejects("usage.query", bad);
    }
}

#[test]
fn correlates_cancellation_to_an_account_and_a_specific_attempt() {
    rejects("accounts.cancelSignIn", json!({ "id": "claude-main" }));
    assert_eq!(
        params(
            "accounts.cancelSignIn",
            json!({ "id": "claude-main", "attempt": "current" })
        ),
        json!({ "id": "claude-main", "attempt": "current" })
    );
}

#[test]
fn requires_an_explicit_account_for_a_queued_work_limit_retry() {
    rejects("accounts.retryLimit", json!({}));
    assert_eq!(
        params("accounts.retryLimit", json!({ "id": "claude-main" })),
        json!({ "id": "claude-main" })
    );
}

#[test]
fn keeps_provider_allowances_separate_from_costs_preserving_unknown_readings_and_credit_units() {
    let reading = json!({
        "status": "available", "checkedAt": "2026-10-22T12:00:00.000Z", "lastSuccessfulAt": "2026-10-22T12:00:00.000Z", "error": null,
        "planLabel": "Pro", "windows": [{ "id": "weekly", "label": "Weekly", "scope": null, "durationMinutes": 10080, "usedPercent": null, "resetsAt": null }],
        "credits": { "balance": "38917.83", "hasCredits": true, "unlimited": false }, "extraUsageEnabled": null,
    });
    // AccountProviderUsage has no schema of its own; it is checked inside the accounts.list result.
    let listed = |usage: Value| json!([with(account(), json!({ "providerUsage": usage }))]);
    assert_valid("accounts.list", &listed(reading.clone()));
    let parsed: AccountProviderUsage = serde_json::from_value(reading.clone()).unwrap();
    assert_eq!(parsed.windows[0].used_percent, None);
    assert_eq!(parsed.windows[0].resets_at, None);
    assert_eq!(
        parsed
            .credits
            .as_ref()
            .and_then(|credits| credits.balance.as_deref()),
        Some("38917.83")
    );
    invalid(
        "accounts.list",
        listed(with(
            reading.clone(),
            json!({ "credits": with(reading["credits"].clone(), json!({ "balance": "$38917.83" })) }),
        )),
    );
    invalid(
        "accounts.list",
        listed(with(
            reading.clone(),
            json!({ "windows": [with(reading["windows"][0].clone(), json!({ "usedPercent": 101 }))] }),
        )),
    );
    invalid(
        "accounts.list",
        listed(with(
            reading.clone(),
            json!({ "windows": vec![reading["windows"][0].clone(); 65] }),
        )),
    );
    let stripped = serde_json::from_value::<AccountProviderUsage>(with(
        reading,
        json!({ "accessToken": "synthetic-secret", "credentialPath": "private" }),
    ));
    let stripped = serde_json::to_value(stripped.unwrap()).unwrap();
    assert!(stripped.get("accessToken").is_none());
    assert!(stripped.get("credentialPath").is_none());
}
