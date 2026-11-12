//! `browser.test.ts`

use super::{invalid, params, rejects, round_trip, with, without};
use protocol::browser::{BrowserFrameResult, BrowserState, BrowserStreamFrame};
use protocol::wire::Empty;
use serde_json::{Value, json};

fn target() -> Value {
    json!({ "botId": "b1", "generation": "generation", "pageId": "page" })
}

#[test]
fn requires_a_page_and_session_generation_for_input() {
    rejects(
        "browser.input",
        json!({ "botId": "b1", "input": { "kind": "text", "text": "hello" } }),
    );
    let parsed = params(
        "browser.input",
        with(
            target(),
            json!({ "input": { "kind": "key", "action": "down", "key": "Control" } }),
        ),
    );
    for key in ["botId", "generation", "pageId"] {
        assert_eq!(parsed[key], target()[key]);
    }
}

#[test]
fn rejects_nonfinite_and_oversized_coordinate_and_viewport_payloads() {
    for x in [json!(-1), Value::Null, json!(5000)] {
        rejects(
            "browser.input",
            with(
                target(),
                json!({ "input": { "kind": "mouse", "action": "down", "x": x, "y": 0 } }),
            ),
        );
    }
    rejects(
        "browser.resize",
        with(target(), json!({ "width": 100_000, "height": 900 })),
    );
    rejects(
        "browser.input",
        with(
            target(),
            json!({ "input": { "kind": "text", "text": "x".repeat(100_001) } }),
        ),
    );
}

#[test]
fn keeps_frame_cursors_css_viewport_dimensions_and_input_layout_fences_on_the_wire() {
    let after = json!({ "streamId": "stream", "sequence": 3 });
    assert_eq!(
        params("browser.frame", json!({ "botId": "b1", "after": after }))["after"],
        after
    );
    let input = with(
        target(),
        json!({ "viewportRevision": 4, "input": { "kind": "text", "text": "hello" } }),
    );
    assert_eq!(
        params("browser.input", input.clone())["viewportRevision"],
        4
    );
    rejects(
        "browser.input",
        with(input, json!({ "viewportRevision": -1 })),
    );
    let result: BrowserFrameResult = round_trip(
        "browser.frame",
        json!({ "state": { "botId": "b1", "generation": "generation", "control": "bot", "activePageId": "page", "tabs": [], "dialog": null,
                "revision": 7, "viewportRevision": 4, "busy": true },
            "frame": { "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "viewportWidth": 2560, "viewportHeight": 1600,
                "pageId": "page", "viewportRevision": 4, "streamId": "stream", "sequence": 3 } }),
    );
    let frame = result.frame.unwrap();
    assert_eq!(
        (frame.width, frame.viewport_width, frame.viewport_revision),
        (1280, Some(2560), Some(4))
    );
    assert_eq!(
        (result.state.revision, result.state.busy),
        (Some(7), Some(true))
    );
}

fn streamed() -> Value {
    json!({ "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "viewportWidth": 1280, "viewportHeight": 800,
        "pageId": "page", "viewportRevision": 4, "streamId": "stream", "sequence": 9 })
}

#[test]
fn turns_the_stream_on_or_off_for_a_bot_with_an_optional_frame_rate_cap_of_1_to_60() {
    assert_eq!(
        params("browser.stream", json!({ "botId": "b1", "on": true })),
        json!({ "botId": "b1", "on": true })
    );
    assert_eq!(
        params(
            "browser.stream",
            json!({ "botId": "b1", "on": true, "maxFps": 60 })
        )["maxFps"],
        60
    );
    for max_fps in [json!(0), json!(61), json!(29.5), json!(-1)] {
        rejects(
            "browser.stream",
            json!({ "botId": "b1", "on": true, "maxFps": max_fps }),
        );
    }
    for bad in [
        json!({ "botId": "b1" }),
        json!({ "botId": "", "on": true }),
        json!({ "botId": "b1", "on": "yes" }),
    ] {
        rejects("browser.stream", bad);
    }
}

#[test]
fn answers_a_stream_request_with_the_browser_state_as_every_browser_method_does() {
    let state: BrowserState = round_trip(
        "browser.stream",
        json!({ "botId": "b1", "generation": "generation", "control": "bot", "activePageId": "page", "tabs": [], "dialog": null,
            "privateActive": false }),
    );
    assert_eq!(
        (state.bot_id.as_str(), state.private_active),
        ("b1", Some(false))
    );
}

#[test]
fn acknowledges_a_frame_by_bot_and_sequence() {
    assert_eq!(
        params("browser.streamAck", json!({ "botId": "b1", "sequence": 9 })),
        json!({ "botId": "b1", "sequence": 9 })
    );
    for sequence in [json!(-1), json!(1.5), json!("9")] {
        rejects(
            "browser.streamAck",
            json!({ "botId": "b1", "sequence": sequence }),
        );
    }
    rejects("browser.streamAck", json!({ "sequence": 9 }));
    let _: Empty = round_trip("browser.streamAck", json!({}));
}

#[test]
fn carries_a_frame_with_its_stream_id_and_ends_the_stream_with_a_null_frame() {
    let event: BrowserStreamFrame = round_trip(
        "browser.streamFrame",
        json!({ "botId": "b1", "frame": streamed() }),
    );
    let frame = event.frame.unwrap();
    assert_eq!(
        (frame.stream_id.as_deref(), frame.sequence),
        (Some("stream"), 9)
    );
    let ended: BrowserStreamFrame =
        round_trip("browser.streamFrame", json!({ "botId": "b1", "frame": null }));
    assert_eq!(ended.frame, None);
    invalid("browser.streamFrame", json!({ "botId": "b1" }));
    invalid(
        "browser.streamFrame",
        json!({ "botId": "b1", "frame": without(streamed(), "streamId") }),
    );
    invalid(
        "browser.streamFrame",
        json!({ "botId": "b1", "frame": with(streamed(), json!({ "mime": "image/png" })) }),
    );
}
