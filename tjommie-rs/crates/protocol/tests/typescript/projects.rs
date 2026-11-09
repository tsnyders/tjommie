//! `projects.test.ts`

use super::{params, rejects};
use serde_json::json;

#[test]
fn rejects_remote_style_arbitrary_execution_fields_and_invalid_exact_commit_or_ref_inputs() {
    rejects(
        "tasks.evaluate",
        json!({ "attemptId": "a", "expectedRevision": 1, "sourceSha": "a".repeat(40), "kind": "test", "command": "injected" }),
    );
    rejects(
        "tasks.integrate",
        json!({ "attemptId": "a", "expectedRevision": 1, "sourceSha": "HEAD~1", "expectedTargetSha": "a".repeat(40), "requestId": "r" }),
    );
    rejects(
        "projects.create",
        json!({ "name": "Project", "repositoryId": "r", "baseRef": "--upload-pack=bad", "targetRef": "refs/heads/main", "members": [] }),
    );
    rejects(
        "tasks.create",
        json!({ "projectId": "p", "title": "", "instructions": "x", "assigneeId": "b" }),
    );
}

#[test]
fn does_not_apply_create_defaults_when_patching_unrelated_metadata() {
    let project = params(
        "projects.update",
        json!({ "id": "p", "expectedRevision": 1, "patch": { "targetRef": "refs/heads/delivery" } }),
    );
    assert_eq!(
        project["patch"],
        json!({ "targetRef": "refs/heads/delivery" })
    );
    let task = params(
        "tasks.update",
        json!({ "id": "t", "expectedRevision": 1, "patch": { "title": "New title" } }),
    );
    assert_eq!(task["patch"], json!({ "title": "New title" }));
}
