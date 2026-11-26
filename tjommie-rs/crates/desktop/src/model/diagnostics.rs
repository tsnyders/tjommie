//! Port of `components/settings/diagnostics.ts`: the diagnostic report Settings, Diagnostics previews, copies and
//! saves. It is built only from checked versions and fixed health categories: every input is read as untrusted JSON, as
//! the TypeScript reads it, so a name, a path, a URL, a token or an error text in it never reaches the report. The
//! protocol's allowlist (`parse_diagnostic_report`) checks the result.

use desktop_host::diagnostics::diagnostic_version_ok;
use protocol::diagnostics::parse_diagnostic_report;
use serde_json::{Map, Value, json};

/// What a report is made from: the device's and the host's facts and the speech status as they came (any JSON), the
/// connection as `{ status, reason }`, whether the host is a paired PC, the platform when the device doesn't say, and
/// whether this device has a file session.
pub struct ReportInput {
    pub device: Value,
    pub host: Value,
    pub speech: Value,
    pub connection: Value,
    pub paired: bool,
    pub platform: &'static str,
    pub attachments: bool,
}

fn object(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    }
}

/// `value` when it is one of `choices`, else `fallback`.
fn one_of(value: Option<&Value>, choices: &[&'static str], fallback: &'static str) -> &'static str {
    let given = value.and_then(Value::as_str);
    choices
        .iter()
        .copied()
        .find(|choice| Some(*choice) == given)
        .unwrap_or(fallback)
}

/// A version as data: `major.minor.patch`, maybe `-alpha`, `-beta` or `-rc` and a number; anything else (a CLI's
/// output, a build label, an error text) is no version.
pub fn diagnostic_version(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|version| diagnostic_version_ok(version))
        .map(str::to_owned)
}

/// The report, as JSON text (`diagnosticReport`).
pub fn diagnostic_report(input: &ReportInput) -> String {
    let (device, host, speech, connection) = (
        object(&input.device),
        object(&input.host),
        object(&input.speech),
        object(&input.connection),
    );
    let model = speech.get("model").map(object).unwrap_or_default();
    let providers = match host.get("providers") {
        Some(Value::Array(providers)) => providers.clone(),
        _ => Vec::new(),
    };
    let status = one_of(
        connection.get("status"),
        &[
            "connecting",
            "ready",
            "reconnecting",
            "offline",
            "removed",
            "update",
            "failed",
            "outside",
            "unknown",
        ],
        "unknown",
    );
    let issue = match status {
        "offline" => Some(one_of(
            connection.get("reason"),
            &["host_offline", "unreachable", "unknown"],
            "unknown",
        )),
        "removed" => Some("device_removed"),
        "update" => Some("version_mismatch"),
        "failed" => Some("connection_failed"),
        _ => None,
    };
    let platform_given = device
        .get("platform")
        .cloned()
        .unwrap_or_else(|| Value::from(input.platform));
    let version_of = |provider: &str| {
        let entry = providers
            .iter()
            .map(object)
            .find(|entry| entry.get("provider").and_then(Value::as_str) == Some(provider))
            .unwrap_or_default();
        diagnostic_version(entry.get("version"))
    };
    let versions: Vec<Value> = ["claude", "codex"]
        .into_iter()
        .map(|provider| json!({ "provider": provider, "version": version_of(provider) }))
        .collect();
    let availability = match speech.get("available") {
        Some(Value::Bool(true)) => "available",
        Some(Value::Bool(false)) => "unavailable",
        _ => "unknown",
    };
    let report = json!({
        "reportVersion": 1,
        "device": {
            "platform": one_of(Some(&platform_given), &["windows", "macos", "linux", "android", "browser", "unknown"], "unknown"),
            "appVersion": diagnostic_version(device.get("appVersion")),
            "fileBridge": if input.attachments { "present" } else { "unavailable" },
        },
        "host": {
            "selection": if input.paired { "paired" } else { "this-device" },
            "appVersion": diagnostic_version(host.get("appVersion")),
            "daemonVersion": diagnostic_version(host.get("daemonVersion")),
            "providers": versions,
            "relay": one_of(host.get("relay"), &["off", "connecting", "online", "error", "unknown"], "unknown"),
            "storage": one_of(host.get("storage"), &["ok", "unavailable", "error", "unknown"], "unknown"),
        },
        "connection": { "status": status, "issue": issue, "route": "unknown" },
        "speech": {
            "engine": one_of(speech.get("engine"), &["whisper.cpp", "android-on-device", "unknown"], "unknown"),
            "availability": availability,
            "permission": one_of(speech.get("permission"), &["unknown", "prompt", "granted", "denied"], "unknown"),
            "model": one_of(model.get("state"), &["missing", "downloading", "verifying", "ready", "error", "system-managed", "unknown"], "unknown"),
        },
    })
    .to_string();
    // Built from the allowlist's own values, so the check passes; the checked text is what is shown and saved.
    parse_diagnostic_report(&report).unwrap_or(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ReportInput {
        ReportInput {
            device: json!({ "appVersion": "0.3.2", "platform": "windows" }),
            host: json!({
                "appVersion": null, "daemonVersion": "0.3.2",
                "providers": [{ "provider": "claude", "version": "2.1.3" }], "relay": "online", "storage": "ok"
            }),
            speech: json!({
                "engine": "whisper.cpp", "available": false, "permission": "denied", "model": { "state": "missing" }
            }),
            connection: json!({ "status": "offline", "reason": "host_offline" }),
            paired: true,
            platform: "browser",
            attachments: true,
        }
    }

    fn parsed(input: &ReportInput) -> Value {
        serde_json::from_str(&diagnostic_report(input)).unwrap()
    }

    /// diagnostics.test.ts: "exports only structured versions and health categories, with distinct device/host scope".
    #[test]
    fn exports_only_structured_versions_and_health_categories_with_distinct_device_and_host_scope()
    {
        let report = parsed(&input());
        assert_eq!(report["device"]["appVersion"], "0.3.2");
        assert_eq!(report["device"]["platform"], "windows");
        assert_eq!(report["host"]["appVersion"], Value::Null);
        assert_eq!(report["host"]["daemonVersion"], "0.3.2");
        assert_eq!(report["host"]["selection"], "paired");
        assert_eq!(
            report["host"]["providers"],
            json!([{ "provider": "claude", "version": "2.1.3" }, { "provider": "codex", "version": null }])
        );
        assert_eq!(
            report["connection"],
            json!({ "status": "offline", "issue": "host_offline", "route": "unknown" })
        );
        assert_eq!(report["speech"]["availability"], "unavailable");
        assert_eq!(report["speech"]["model"], "missing");
        assert_eq!(report["speech"]["permission"], "denied");
    }

    /// diagnostics.test.ts: "cannot leak synthetic credentials, identity, paths, URLs, raw errors, transcripts or message
    /// contents".
    #[test]
    fn cannot_leak_credentials_identity_paths_urls_raw_errors_or_contents() {
        let secret = "SYNTHETIC_DO_NOT_EXPORT_9f4c";
        let dirty = ReportInput {
            device: json!({
                "appVersion": format!("0.3.2-{secret}"), "platform": "windows", "username": secret,
                "path": format!("C:\\Users\\{secret}")
            }),
            host: json!({
                "name": secret, "token": secret, "pairingCode": secret, "privateKey": secret,
                "relay": format!("https://user:{secret}@relay.invalid"), "appVersion": secret, "daemonVersion": secret,
                "providers": [
                    { "provider": "claude", "version": format!("2.1.3 ({secret})"), "account": secret },
                    { "provider": secret, "version": "1.2.3" }
                ],
                "logs": [secret], "storage": "ok"
            }),
            speech: json!({
                "engine": "whisper.cpp", "available": false, "permission": "denied", "model": { "state": "missing" },
                "problem": { "code": secret, "message": secret }, "transcript": secret, "audio": secret
            }),
            connection: json!({ "status": "failed", "reason": secret }),
            ..input()
        };
        let report = diagnostic_report(&dirty);
        assert!(!report.contains(secret));
        assert!(!report.contains("C:\\"));
        assert!(!report.contains("https:"));
        let report: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report["device"]["appVersion"], Value::Null);
        assert_eq!(report["host"]["appVersion"], Value::Null);
        assert_eq!(report["host"]["daemonVersion"], Value::Null);
        assert_eq!(report["host"]["relay"], "unknown");
        assert_eq!(report["connection"]["issue"], "connection_failed");
    }

    /// diagnostics.test.ts: "handles absent and malformed fields without serializing untrusted objects".
    #[test]
    fn handles_absent_and_malformed_fields() {
        let report = parsed(&ReportInput {
            device: Value::Null,
            host: json!(["wrong"]),
            speech: json!({ "model": null }),
            connection: json!({ "status": "bogus", "reason": { "password": "synthetic" } }),
            paired: false,
            platform: "android",
            attachments: false,
        });
        assert_eq!(report["device"]["platform"], "android");
        assert_eq!(report["connection"]["status"], "unknown");
        assert!(!report.to_string().contains("synthetic"));
        for value in [
            Value::Null,
            json!("v0.3.2"),
            json!("0.3.2+secret"),
            json!("0.3.2\nC:\\private"),
            json!("https://example.test"),
            json!("2.1.3 (CLI)"),
        ] {
            assert_eq!(diagnostic_version(Some(&value)), None, "{value}");
        }
        assert_eq!(
            diagnostic_version(Some(&json!("0.4.0-rc.2"))).as_deref(),
            Some("0.4.0-rc.2")
        );
    }
}
