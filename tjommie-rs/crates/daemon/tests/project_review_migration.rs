//! Port of `packages/daemon/src/project-review-migration.test.ts`.
// TODO(M5-integration): the TypeScript test makes its legacy row through `projectFixture`, `Tasks.start` and
// `Tasks.evaluate`. Those modules are not ported yet, so this seeds the same parent rows and the legacy evaluation
// with SQL. Switch to the fixture flow when Projects and Tasks land.

use daemon::db::{MIGRATIONS, open};
use rusqlite::Connection;
use serde_json::Value;

/// `PROJECT_MIGRATION` (project-schema.ts:2) is migration 11 here.
const PROJECT_MIGRATION: &str = include_str!("../migrations/11.sql");
/// `PROJECT_REVIEW_COVERAGE_MIGRATION` (project-schema.ts:28) is migration 12 here.
const PROJECT_REVIEW_COVERAGE_MIGRATION: &str = include_str!("../migrations/12.sql");

const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// The parent rows of one review evaluation, and the evaluation itself with a `reviewCoverage` that the legacy
/// database did not have. Foreign keys are on, as in the daemon.
fn seed_legacy_evaluation(db: &Connection) {
    db.execute_batch(&format!(
        r#"
        INSERT INTO project_repositories (id, path, common_dir, name, created_at)
            VALUES ('repo', 'C:/synthetic/owner', 'C:/synthetic/owner/.git', 'owner', '2026-11-12T00:00:00.000Z');
        INSERT INTO projects (id, repository_id, state, revision, data)
            VALUES ('project', 'repo', 'active', 1, '{{}}');
        INSERT INTO project_tasks (id, project_id, state, revision, data)
            VALUES ('task', 'project', 'done', 1, '{{}}');
        INSERT INTO task_attempts (id, task_id, ordinal, state, bot_id, worktree_path, branch, data)
            VALUES ('attempt', 'task', 1, 'done', 'bot', 'C:/synthetic/managed/attempt', 'tjommie/attempt',
                    '{{"headSha":"{HEAD}"}}');
        INSERT INTO task_evaluations (id, attempt_id, revision, source_sha, kind, state, data)
            VALUES ('review', 'attempt', 1, '{HEAD}', 'review', 'passed',
                    '{{"sourceSha":"{HEAD}","state":"passed","reason":null,"findings":[],"reviewCoverage":{{"complete":true}}}}');
        UPDATE task_evaluations SET data=json_remove(data,'$.reviewCoverage') WHERE id='review';
        "#
    ))
    .unwrap();
}

#[test]
fn upgrades_legacy_evaluation_json_to_explicit_null_coverage_without_changing_settled_source_results()
{
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("test.sqlite");
    let db = open(&path).unwrap();
    seed_legacy_evaluation(&db);

    // The coverage migration is the one at its index in the list; setting `user_version` to that index makes it
    // pending again, as `PRAGMA user_version=${version}` does in the TypeScript test.
    let version = MIGRATIONS
        .iter()
        .position(|migration| *migration == PROJECT_REVIEW_COVERAGE_MIGRATION)
        .unwrap();
    let project = MIGRATIONS
        .iter()
        .position(|migration| *migration == PROJECT_MIGRATION)
        .unwrap();
    assert!(version > project);
    db.execute_batch(&format!("PRAGMA user_version={version}"))
        .unwrap();
    drop(db);

    let db = open(&path).unwrap();
    let (state, source_sha, data): (String, String, String) = db
        .query_row(
            "SELECT state,source_sha,data FROM task_evaluations WHERE id='review'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "passed");
    assert_eq!(source_sha, HEAD);
    let data: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(data.get("reviewCoverage"), Some(&Value::Null));
    assert_eq!(data["sourceSha"], HEAD);
    assert_eq!(data["state"], "passed");
}
