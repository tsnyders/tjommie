//! `native-browser.test.ts`

use super::with;
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserMount, NativeBrowserUiCommand};
use protocol::parse;
use serde_json::{Value, json};

#[test]
fn accepts_internal_automation_and_cleanup_but_excludes_both_from_renderer_commands() {
    for command in [
        json!({ "kind": "close" }),
        json!({ "kind": "delete_profile" }),
        json!({ "kind": "cancel_tools" }),
        json!({ "kind": "tool", "name": "browser_snapshot", "args": {} }),
    ] {
        assert!(
            parse::<NativeBrowserCommand>(command.clone()).is_ok(),
            "{command}"
        );
        assert!(parse::<NativeBrowserUiCommand>(command).is_err());
    }
    let remote = json!({ "kind": "remote", "controllerId": "trusted-connection", "action": { "kind": "input", "generation": "one", "pageId": "page",
        "controlEpoch": 2, "viewportRevision": 3, "input": { "kind": "text", "text": "synthetic PIN" } } });
    assert!(parse::<NativeBrowserCommand>(remote.clone()).is_ok());
    assert!(parse::<NativeBrowserUiCommand>(remote.clone()).is_err());
    let mut without_epoch = remote;
    without_epoch["action"]
        .as_object_mut()
        .unwrap()
        .remove("controlEpoch");
    assert!(parse::<NativeBrowserCommand>(without_epoch).is_err());
}

#[test]
fn requires_a_session_and_page_identity_for_page_mutations() {
    assert!(
        parse::<NativeBrowserUiCommand>(
            json!({ "kind": "navigate", "url": "https://example.test" })
        )
        .is_err()
    );
    assert!(parse::<NativeBrowserUiCommand>(json!({ "kind": "navigate", "generation": "generation", "pageId": "page", "url": "https://example.test" })).is_ok());
}

#[test]
fn distinguishes_a_temporary_overlay_from_leaving_the_browser() {
    let mount =
        json!({ "visible": true, "bounds": { "x": 0, "y": 44, "width": 640, "height": 480 } });
    assert!(parse::<NativeBrowserMount>(with(mount.clone(), json!({ "occluded": true }))).is_ok());
    assert!(parse::<NativeBrowserMount>(with(mount, json!({ "occluded": "true" }))).is_err());
}

#[test]
fn rejects_extra_privileged_keys_nonfinite_bounds_negative_bounds_and_oversized_tools() {
    assert!(
        parse::<NativeBrowserUiCommand>(json!({ "kind": "state", "script": "process.exit()" }))
            .is_err()
    );
    for x in [Value::Null, json!(-1), json!(40000)] {
        assert!(parse::<NativeBrowserMount>(json!({ "visible": true, "bounds": { "x": x, "y": 44, "width": 100, "height": 100 } })).is_err());
    }
    let oversized = json!({ "kind": "tool", "name": "browser_evaluate", "args": { "expression": "x".repeat(1_000_001) } });
    assert!(parse::<NativeBrowserCommand>(oversized).is_err());
}
