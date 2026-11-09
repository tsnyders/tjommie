//! `google.test.ts`

use super::{invalid, params, rejects, round_trip, with};
use protocol::google::GoogleStatus;
use serde_json::json;

#[test]
fn accepts_write_only_desktop_client_settings_and_rejects_arbitrary_auth_urls() {
    let config =
        json!({ "clientId": "123-test.apps.googleusercontent.com", "clientSecret": "synthetic" });
    assert_eq!(
        params("google.configure", config.clone())["clientSecret"],
        "synthetic"
    );
    rejects(
        "google.configure",
        with(config.clone(), json!({ "clientId": "https://evil.test" })),
    );
    rejects(
        "google.configure",
        with(config, json!({ "tokenUrl": "https://evil.test" })),
    );
}

#[test]
fn keeps_credentials_and_authorization_urls_out_of_status() {
    let status = json!({ "configured": true, "connected": true, "email": "fixture@example.test", "services": ["gmail"], "grantedBotIds": ["one"], "attempt": null });
    let _: GoogleStatus = round_trip("google.status", status.clone());
    for key in ["accessToken", "refreshToken", "clientSecret", "url"] {
        invalid(
            "google.status",
            with(status.clone(), json!({ key: "synthetic" })),
        );
    }
}

#[test]
fn requires_explicit_unique_read_service_selection_and_an_explicit_bot_grant() {
    assert_eq!(
        params("google.begin", json!({ "services": ["gmail"] })),
        json!({ "services": ["gmail"] })
    );
    for services in [
        json!([]),
        json!(["gmail", "gmail"]),
        json!(["gmail.send"]),
        json!(["calendar.write"]),
    ] {
        rejects("google.begin", json!({ "services": services }));
    }
    rejects("google.grant", json!({ "botId": "one" }));
    assert_eq!(
        params("google.grant", json!({ "botId": "one", "enabled": false })),
        json!({ "botId": "one", "enabled": false })
    );
}
