//! Port of `packages/ui/lib/projects.ts` (T1.4): whether a task can be integrated, and the evaluation it is judged by.
//! The wire types are the protocol crate's; the TypeScript's typing helpers (`ProjectHandlers`, `ProjectBroadcast`) have
//! no port.

use protocol::projects::{
    AttemptState, EvaluationKind, EvaluationState, Project, ProjectState, TaskAttempt, TaskDetail,
    TaskEvaluation,
};

/// The latest evaluation of `kind` for this attempt at its current revision and head commit (`latestEvaluation`).
pub fn latest_evaluation<'a>(
    detail: &'a TaskDetail,
    attempt: &TaskAttempt,
    kind: EvaluationKind,
) -> Option<&'a TaskEvaluation> {
    // `.at(-1)`: the last one that matches.
    detail.evaluations.iter().rfind(|evaluation| {
        evaluation.attempt_id == attempt.id
            && evaluation.revision == attempt.revision
            && attempt.head_sha.as_deref() == Some(evaluation.source_sha.as_str())
            && evaluation.kind == kind
    })
}

/// A reference that JavaScript's truthiness takes as present: an empty id is no one.
fn named(id: Option<&str>) -> bool {
    id.is_some_and(|id| !id.is_empty())
}

/// Whether integration is allowed: an active project, a clean completed attempt, both reviewer and tester named, and
/// fresh passing review and test results from the current project revision (`canIntegrate`).
pub fn can_integrate(project: &Project, detail: &TaskDetail, dirty: bool) -> bool {
    let Some(attempt) = detail
        .attempts
        .iter()
        .find(|candidate| Some(candidate.id.as_str()) == detail.task.current_attempt_id.as_deref())
    else {
        return false;
    };
    if project.state != ProjectState::Active
        || dirty
        || attempt.state != AttemptState::Completed
        || !named(detail.task.input.reviewer_id.as_deref())
        || !named(detail.task.input.tester_id.as_deref())
        || project.input.checks.is_empty()
    {
        return false;
    }
    // A review only counts for the commit it covered: its coverage must name the attempt's head.
    let covers_head = |evaluation: &TaskEvaluation| {
        evaluation
            .review_coverage
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|coverage| {
                Some(coverage.source_sha.as_str()) == attempt.head_sha.as_deref()
            })
    };
    [EvaluationKind::Review, EvaluationKind::Test]
        .into_iter()
        .all(|kind| {
            latest_evaluation(detail, attempt, kind).is_some_and(|result| {
                result.state == EvaluationState::Passed
                    && result.project_revision == project.revision
                    && (kind != EvaluationKind::Review || covers_head(result))
            })
        })
}

/// The first twelve characters of a commit, or "Not recorded" (`shortSha`). JavaScript counts UTF-16 units; a commit is
/// ASCII hex, so counting characters gives the same text.
pub fn short_sha(sha: Option<&str>) -> String {
    match sha {
        Some(sha) => sha.chars().take(12).collect(),
        None => "Not recorded".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// The TypeScript fixture: an active project with one check, a task on attempt "a" whose review and test both passed.
    fn fixtures() -> (Project, TaskDetail) {
        let sha = "a".repeat(40);
        let timestamp = "2026-11-03T00:00:00.000Z";
        let project = serde_json::from_value::<Project>(json!({
            "id": "p", "name": "Project", "repositoryId": "r", "roomId": null,
            "baseRef": "refs/heads/main", "targetRef": "refs/heads/main",
            "members": [{ "botId": "dev", "roles": ["developer"] }],
            "checks": [{ "id": "test", "label": "Test", "command": "node", "args": ["--test"] }],
            "state": "active", "revision": 1, "createdAt": timestamp, "updatedAt": timestamp
        }))
        .unwrap();
        let evaluations: Vec<Value> = ["review", "test"]
            .iter()
            .map(|kind| {
                let coverage = if *kind == "review" {
                    json!({
                        "sourceSha": sha, "snapshotSha256": "b".repeat(64),
                        "diffSha256": "c".repeat(64), "complete": true, "files": []
                    })
                } else {
                    Value::Null
                };
                json!({
                    "id": kind, "attemptId": "a", "revision": 1, "projectRevision": 1,
                    "sourceSha": sha, "kind": kind, "botId": kind, "worktreePath": null,
                    "reviewCoverage": coverage, "state": "passed", "reason": null,
                    "findings": [], "checks": [], "createdAt": timestamp, "updatedAt": timestamp
                })
            })
            .collect();
        let detail = serde_json::from_value::<TaskDetail>(json!({
            "task": {
                "id": "t", "projectId": "p", "title": "Task", "instructions": "Change",
                "assigneeId": "dev", "reviewerId": "review", "testerId": "test", "state": "done",
                "currentAttemptId": "a", "revision": 2, "createdAt": timestamp, "updatedAt": timestamp
            },
            "attempts": [{
                "id": "a", "taskId": "t", "ordinal": 1, "baseSha": sha, "headSha": sha,
                "branch": "refs/heads/tjommie/a", "worktreePath": "synthetic-worktree",
                "phase": "develop", "state": "completed", "revision": 1, "botId": "dev",
                "workItemId": "w", "contextKey": "ctx", "reason": null,
                "createdAt": timestamp, "updatedAt": timestamp
            }],
            "evaluations": evaluations,
            "integrations": []
        }))
        .unwrap();
        (project, detail)
    }

    #[test]
    fn enables_exact_integration_only_for_fresh_independent_results_and_a_clean_completed_attempt()
    {
        let (project, detail) = fixtures();
        assert!(can_integrate(&project, &detail, false));
        assert!(!can_integrate(&project, &detail, true));

        let mut newer_project = project.clone();
        newer_project.revision = 2;
        assert!(!can_integrate(&newer_project, &detail, false));

        let mut stale = detail.clone();
        stale.attempts[0].revision = 2;
        assert!(!can_integrate(&project, &stale, false));

        let mut unavailable = detail.clone();
        unavailable.evaluations[0].state = EvaluationState::Unavailable;
        assert_eq!(
            latest_evaluation(
                &unavailable,
                &unavailable.attempts[0],
                EvaluationKind::Review
            )
            .map(|evaluation| evaluation.state),
            Some(EvaluationState::Unavailable)
        );
        assert!(!can_integrate(&project, &unavailable, false));

        let mut old = detail.clone();
        old.evaluations[0].review_coverage = Some(None);
        assert!(!can_integrate(&project, &old, false));
    }

    #[test]
    fn an_empty_reviewer_is_no_reviewer_as_in_javascript() {
        let (project, mut detail) = fixtures();
        assert!(can_integrate(&project, &detail, false));
        detail.task.input.reviewer_id = Some(String::new());
        assert!(!can_integrate(&project, &detail, false));
    }

    #[test]
    fn short_sha_cuts_to_twelve_characters_and_says_when_none() {
        let sha = "a".repeat(40);
        assert_eq!(short_sha(Some(&sha)), "a".repeat(12));
        assert_eq!(short_sha(Some("")), "");
        assert_eq!(short_sha(None), "Not recorded");
    }
}
