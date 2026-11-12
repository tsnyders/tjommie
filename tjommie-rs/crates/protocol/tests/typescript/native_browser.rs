//! `native-browser.test.ts`

use super::{with, without};
use protocol::native_browser::{
    NativeBrowserCommand, NativeBrowserMount, NativeBrowserStreamMessage, NativeBrowserUiCommand,
};
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
fn lets_a_remote_controller_turn_the_live_stream_on_or_off_but_not_the_renderer() {
    let stream = json!({ "kind": "remote", "controllerId": "rpc:one", "action": { "kind": "stream", "on": true, "maxFps": 30 } });
    assert!(parse::<NativeBrowserCommand>(stream.clone()).is_ok());
    assert!(
        parse::<NativeBrowserCommand>(with(
            stream.clone(),
            json!({ "action": { "kind": "stream", "on": false } })
        ))
        .is_ok()
    );
    assert!(parse::<NativeBrowserUiCommand>(stream["action"].clone()).is_err());
    for action in [
        json!({ "kind": "stream" }),
        json!({ "kind": "stream", "on": true, "maxFps": 0 }),
        json!({ "kind": "stream", "on": true, "maxFps": 61 }),
        json!({ "kind": "stream", "on": true, "extra": 1 }),
    ] {
        let command = with(stream.clone(), json!({ "action": action }));
        assert!(parse::<NativeBrowserCommand>(command.clone()).is_err(), "{command}");
    }
}

#[test]
fn accepts_only_a_streamed_frame_or_an_end_from_the_desktop_hosts_own_posts() {
    let frame = json!({ "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "sequence": 4, "streamId": "stream" });
    assert!(
        parse::<NativeBrowserStreamMessage>(
            json!({ "kind": "stream_frame", "botId": "b1", "frame": frame })
        )
        .is_ok()
    );
    for reason in ["private", "closed", "failed"] {
        let message = json!({ "kind": "stream_end", "botId": "b1", "reason": reason });
        assert!(
            parse::<NativeBrowserStreamMessage>(message.clone()).is_ok(),
            "{message}"
        );
    }
    for message in [
        json!({ "kind": "stream_frame", "botId": "b1", "frame": without(frame.clone(), "streamId") }),
        json!({ "kind": "stream_frame", "botId": "", "frame": frame }),
        json!({ "kind": "stream_frame", "botId": "b1", "frame": frame, "extra": 1 }),
        json!({ "kind": "stream_end", "botId": "b1", "reason": "bored" }),
        json!({ "kind": "stream_end", "botId": "b1" }),
        json!({ "kind": "frame", "botId": "b1", "frame": frame }),
    ] {
        assert!(
            parse::<NativeBrowserStreamMessage>(message.clone()).is_err(),
            "{message}"
        );
    }
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
