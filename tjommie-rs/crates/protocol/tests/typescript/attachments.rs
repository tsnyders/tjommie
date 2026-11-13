//! `attachments.test.ts` and `attachment-companions.test.ts`

use super::{invalid, params, rejects, round_trip, with};
use protocol::attachments::{
    ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentCompanions, attachment_companion_name,
};
use serde_json::json;

const TRANSFER_ID: &str = "83c70656-e163-478e-a934-6a5801f5ef18";

#[test]
fn accepts_only_a_message_owned_reference_never_a_filesystem_or_caller_context() {
    let reference = json!({ "messageId": "message", "attachmentIndex": 0 });
    assert_eq!(params("attachments.metadata", reference.clone()), reference);
    for extra in [
        json!({ "path": "C:\\private.txt" }),
        json!({ "connectionId": "owner" }),
        json!({ "deviceId": "owner" }),
    ] {
        rejects("attachments.metadata", with(reference.clone(), extra));
    }
    rejects(
        "attachments.metadata",
        json!({ "messageId": "x".repeat(129), "attachmentIndex": 0 }),
    );
}

#[test]
fn rejects_oversized_negative_fractional_and_unsafe_byte_ranges() {
    let read = json!({ "transferId": TRANSFER_ID, "offset": 0, "length": 1 });
    for patch in [
        json!({ "length": ATTACHMENT_CHUNK_BYTES + 1 }),
        json!({ "length": 0 }),
        json!({ "offset": -1 }),
        json!({ "offset": 0.5 }),
        json!({ "offset": 9_007_199_254_740_992_u64 }),
    ] {
        rejects("attachments.read", with(read.clone(), patch));
    }
}

#[test]
fn rejects_malformed_or_oversized_base64_responses_before_a_native_write() {
    let chunk = json!({ "transferId": TRANSFER_ID, "revision": "a".repeat(64), "offset": 0, "bytes": "YWJj", "eof": true });
    let _: AttachmentChunk = round_trip("attachments.read", chunk.clone());
    let oversized = "A".repeat(4 * (ATTACHMENT_CHUNK_BYTES as usize).div_ceil(3) + 4);
    for bytes in ["abc", "YWJj\n", "!!!!", oversized.as_str()] {
        invalid(
            "attachments.read",
            with(chunk.clone(), json!({ "bytes": bytes })),
        );
    }
}

#[test]
fn normalizes_explicit_companion_names_without_accepting_host_paths_urls_or_traversal() {
    assert_eq!(
        attachment_companion_name("./images/chart%20one.png").as_deref(),
        Some("images/chart one.png")
    );
    for name in [
        "../private.png",
        "a/../private.png",
        "%2e%2e/private.png",
        "a/%2f../private",
        "/private.png",
        "//host/file",
        "C:\\private.png",
        "https://host/image",
        "image.png?secret",
        "a/%252e%252e/file",
        "a//b",
        "a\u{0}b",
    ] {
        assert_eq!(attachment_companion_name(name), None, "{name}");
    }
    let companion = |index: u32| json!({ "name": "chart.png", "attachmentId": "41f8bac7-358c-48cf-9f48-a3a358804da6", "attachmentIndex": index });
    assert!(serde_json::from_value::<AttachmentCompanions>(json!([companion(1)])).is_ok());
    assert!(
        serde_json::from_value::<AttachmentCompanions>(json!([companion(1), companion(2)]))
            .is_err()
    );
}
