//! `diagnostics.test.ts`. The `null` case has no port: `parse_diagnostic_report` takes a `&str`.

use super::with;
use protocol::diagnostics::parse_diagnostic_report;
use serde_json::{Value, json};

const INVALID: &str = "Invalid diagnostic report.";

fn valid() -> String {
    serde_json::to_string_pretty(&json!({
        "reportVersion": 1, "device": { "platform": "windows", "appVersion": "0.3.2", "fileBridge": "present" },
        "host": { "selection": "this-device", "appVersion": null, "daemonVersion": null,
            "providers": [{ "provider": "claude", "version": null }, { "provider": "codex", "version": null }], "relay": "unknown", "storage": "unknown" },
        "connection": { "status": "ready", "issue": null, "route": "unknown" },
        "speech": { "engine": "unknown", "availability": "unknown", "permission": "unknown", "model": "unknown" },
    }))
    .unwrap()
}

#[test]
fn accepts_a_fixed_structured_report_and_normalizes_its_json() {
    // JSON.stringify(report, null, 2) from the TypeScript test, with the schema's key order.
    let report = "{\n  \"reportVersion\": 1,\n  \"device\": {\n    \"platform\": \"windows\",\n    \"appVersion\": \"0.3.2\",\n    \"fileBridge\": \"present\"\n  },\n  \"host\": {\n    \"selection\": \"this-device\",\n    \"appVersion\": null,\n    \"daemonVersion\": null,\n    \"providers\": [\n      {\n        \"provider\": \"claude\",\n        \"version\": null\n      },\n      {\n        \"provider\": \"codex\",\n        \"version\": null\n      }\n    ],\n    \"relay\": \"unknown\",\n    \"storage\": \"unknown\"\n  },\n  \"connection\": {\n    \"status\": \"ready\",\n    \"issue\": null,\n    \"route\": \"unknown\"\n  },\n  \"speech\": {\n    \"engine\": \"unknown\",\n    \"availability\": \"unknown\",\n    \"permission\": \"unknown\",\n    \"model\": \"unknown\"\n  }\n}";
    assert_eq!(parse_diagnostic_report(report).unwrap(), report);
    assert_eq!(parse_diagnostic_report(&valid()).unwrap(), report);
    let compact = serde_json::to_string(&serde_json::from_str::<Value>(report).unwrap()).unwrap();
    assert_eq!(parse_diagnostic_report(&compact).unwrap(), report);
}

#[test]
fn rejects_unknown_fields_arbitrary_text_in_known_fields_and_credential_bearing_values() {
    let report: Value = serde_json::from_str(&valid()).unwrap();
    for bad in [
        with(report.clone(), json!({ "logs": ["SYNTHETIC_SECRET"] })),
        with(
            report.clone(),
            json!({ "device": { "platform": "windows", "appVersion": "0.3.2+SYNTHETIC_SECRET", "fileBridge": "present" } }),
        ),
        with(
            report,
            json!({ "connection": { "status": "failed", "issue": "https://user:secret@example.test", "route": "unknown" } }),
        ),
    ] {
        assert_eq!(
            parse_diagnostic_report(&bad.to_string())
                .unwrap_err()
                .to_string(),
            INVALID
        );
    }
}

#[test]
fn bounds_byte_length_before_json_parsing_including_multibyte_input() {
    for report in [
        "x".repeat(16385),
        "é".repeat(10000),
        "{malformed".to_owned(),
    ] {
        assert_eq!(
            parse_diagnostic_report(&report).unwrap_err().to_string(),
            INVALID
        );
    }
}
