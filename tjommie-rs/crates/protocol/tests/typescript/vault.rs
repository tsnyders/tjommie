//! `vault.test.ts`

use super::{params, rejects, with};
use protocol::parse;
use protocol::vault::{LoginOrigin, VaultHostCommand};
use serde_json::json;

#[test]
fn accepts_only_canonical_exact_https_origins() {
    for origin in ["https://example.com", "https://example.com:8443"] {
        assert!(LoginOrigin::new(origin).is_ok(), "{origin}");
    }
    for origin in [
        "http://example.com",
        "https://example.com/",
        "https://example.com/login",
        "https://EXAMPLE.com",
        "https://u:p@example.com",
        "https://example.com?next=evil",
        "https://example.com#secret",
        "file:///tmp/a",
    ] {
        assert!(LoginOrigin::new(origin).is_err(), "{origin}");
    }
}

#[test]
fn rejects_secrets_and_caller_supplied_field_selectors_at_every_vault_boundary() {
    let target = json!({ "id": "00000000-0000-4000-8000-000000000001", "revision": 1, "botId": "bot", "generation": "g", "pageId": "p", "controlEpoch": 3 });
    params("vault.fill", target.clone());
    for extra in [
        json!({ "password": "synthetic" }),
        json!({ "username": "synthetic" }),
        json!({ "selector": "#other" }),
        json!({ "script": "return x" }),
        json!({ "origin": "https://evil.invalid" }),
    ] {
        rejects("vault.fill", with(target.clone(), extra.clone()));
        let action = with(with(target.clone(), json!({ "kind": "fill" })), extra);
        assert!(
            parse::<VaultHostCommand>(json!({ "controllerId": "rpc:1", "action": action }))
                .is_err()
        );
    }
    assert!(
        parse::<VaultHostCommand>(
            json!({ "controllerId": "rpc:1", "action": with(target, json!({ "kind": "fill" })) })
        )
        .is_ok()
    );
    rejects(
        "vault.entry",
        json!({ "botId": "bot", "password": "synthetic" }),
    );
}
