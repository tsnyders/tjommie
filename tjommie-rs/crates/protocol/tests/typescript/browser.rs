//! `browser.test.ts`

use super::{params, rejects, round_trip, with};
use protocol::browser::BrowserFrameResult;
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
