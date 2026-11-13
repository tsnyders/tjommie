//! `projects.test.ts`. Its first test, which needs `rooms::delete_room` (M4), is in `projects_room_retention.rs`.

mod common;
mod project_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use daemon::fsid::real_path;
use daemon::projects::{AssertAvailable, Projects, SqlRooms};
use daemon::server::RpcError;
use daemon::tasks::{Tasks, TasksOptions};
use project_support::fixture;
use protocol::ErrorCode;
use protocol::projects::{
    AttemptState, ProjectInput, ProjectPatch, ProjectState, TaskInput, TaskState,
};

fn rename(name: &str) -> ProjectPatch {
    ProjectPatch {
        name: Some(name.to_owned()),
        ..ProjectPatch::default()
    }
}

#[tokio::test]
async fn blocks_membership_and_task_leases_while_a_bot_lifecycle_change_holds_it() {
    let h = fixture(TasksOptions::default());
    let held = Arc::new(AtomicBool::new(true));
    let available: AssertAvailable = {
        let held = held.clone();
        Arc::new(move |_| {
            if held.load(Ordering::SeqCst) {
                Err(RpcError::new(
                    ErrorCode::Internal,
                    "Bot lifecycle is changing",
                ))
            } else {
                Ok(())
            }
        })
    };
    let projects = Arc::new(
        Projects::new(h.db.clone(), h.worktrees.clone(), Arc::new(SqlRooms))
            .with_assert_available(available),
    );
    let tasks = Tasks::new(projects.clone(), h.runner.clone(), TasksOptions::default());
    let task = h.new_task(0);
    let error = projects
        .update(
            &projects.lock(),
            &h.project.id,
            h.project.revision,
            rename("Changed"),
        )
        .unwrap_err();
    assert!(error.message.contains("lifecycle"), "{}", error.message);
    let error = tasks.start(&task.id, task.revision, "held").unwrap_err();
    assert!(error.message.contains("lifecycle"), "{}", error.message);
    assert!(h.runner.delivered().is_empty());
    held.store(false, Ordering::SeqCst);
    let changed = projects
        .update(
            &projects.lock(),
            &h.project.id,
            h.project.revision,
            rename("Changed"),
        )
        .unwrap();
    assert_eq!(changed.input.name, "Changed");
}

#[tokio::test]
async fn registers_canonical_host_repositories_once_creates_a_normal_room_and_starts_no_work() {
    let h = fixture(TasksOptions::default());
    {
        let db = h.db.lock().unwrap();
        let again = h
            .projects
            .register(&db, h.repo_path.to_str().unwrap())
            .unwrap();
        assert_eq!(again.id, h.repository.id);
        assert_eq!(h.projects.repositories(&db).unwrap().len(), 1);
        let inside = h.repo_path.join("src");
        let error = h
            .projects
            .register(&db, inside.to_str().unwrap())
            .unwrap_err();
        assert!(error.message.contains("root folder"), "{}", error.message);
    }
    assert!(h.project.input.room_id.is_some());
    h.new_task(0);
    assert!(h.runner.delivered().is_empty());
    assert_eq!(h.work_item_count(), 0);
}

#[tokio::test]
async fn rejects_duplicate_membership_assignment_outside_a_role_stale_edits_and_busy_project_changes()
 {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    {
        let db = h.db.lock().unwrap();
        let other = ProjectInput {
            name: "Other project".into(),
            repository_id: h.repository.id.clone(),
            base_ref: h.project.input.base_ref.clone(),
            target_ref: h.project.input.target_ref.clone(),
            room_id: h.project.input.room_id.clone(),
            members: h.project.input.members.clone(),
            checks: h.project.input.checks.clone(),
        };
        let error = h.projects.create(&db, other).unwrap_err();
        assert!(
            error.message.contains("another active project"),
            "{}",
            error.message
        );
        let wrong = TaskInput {
            project_id: task.input.project_id.clone(),
            title: "Wrong role".into(),
            instructions: "Check".into(),
            acceptance: Vec::new(),
            assignee_id: h.bots[2].id.clone(),
            reviewer_id: None,
            tester_id: None,
        };
        let error = h.tasks.create(&db, wrong).unwrap_err();
        assert!(error.message.contains("developer"), "{}", error.message);
        let error = h
            .projects
            .update(&db, &h.project.id, 9, rename("Changed"))
            .unwrap_err();
        assert!(error.message.contains("changed"), "{}", error.message);
    }
    h.tasks.start(&task.id, task.revision, "start").unwrap();
    let db = h.db.lock().unwrap();
    let error = h
        .projects
        .update(&db, &h.project.id, 1, rename("Busy"))
        .unwrap_err();
    assert!(error.message.contains("Stop project"), "{}", error.message);
    let error = h.projects.archive(&db, &h.project.id, 1).unwrap_err();
    assert!(error.message.contains("Stop project"), "{}", error.message);
}

#[tokio::test]
async fn archives_a_team_without_deleting_its_repository_tasks_room_or_worktree_history() {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    {
        let db = h.db.lock().unwrap();
        let archived = h
            .projects
            .archive(&db, &h.project.id, h.project.revision)
            .unwrap();
        assert_eq!(archived.state, ProjectState::Archived);
        assert_eq!(h.tasks.task(&db, &task.id).unwrap(), task);
        let repository = h.projects.repository(&db, &h.repository.id).unwrap();
        assert_eq!(
            repository.path,
            real_path(&h.repo_path).unwrap().to_string_lossy()
        );
        let room: String = db
            .query_row(
                "SELECT id FROM conversations WHERE id = ?1",
                [h.project.input.room_id.as_deref().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(Some(room), h.project.input.room_id);
    }
    let error = h
        .tasks
        .start(&task.id, task.revision, "after-archive")
        .unwrap_err();
    assert!(error.message.contains("archived"), "{}", error.message);
}

#[tokio::test]
async fn retains_persistent_tasks_after_a_new_service_instance_without_rerunning_settled_work() {
    let h = fixture(TasksOptions::default());
    let task = h.new_task(0);
    let attempt = h.tasks.start(&task.id, task.revision, "start").unwrap();
    let complete = h.complete(&attempt, "src/a.txt", "A1\n").await;
    let next = Tasks::new(
        h.projects.clone(),
        h.runner.clone(),
        TasksOptions::default(),
    );
    let db = h.db.lock().unwrap();
    next.recover(&db).unwrap();
    assert_eq!(
        next.attempt(&db, &complete.id).unwrap().state,
        AttemptState::Completed
    );
    assert_eq!(h.runner.delivered().len(), 1);
    assert_eq!(next.task(&db, &task.id).unwrap().state, TaskState::Review);
}

#[tokio::test]
async fn does_not_let_a_removed_members_new_project_lease_block_its_former_project() {
    let h = fixture(TasksOptions::default());
    let moved = h.project.input.members[0].clone();
    let (previous, next, task) = {
        let db = h.db.lock().unwrap();
        let previous = h
            .projects
            .update(
                &db,
                &h.project.id,
                h.project.revision,
                ProjectPatch {
                    members: Some(h.project.input.members[1..].to_vec()),
                    ..ProjectPatch::default()
                },
            )
            .unwrap();
        let next = h
            .projects
            .create(
                &db,
                ProjectInput {
                    name: "Next project".into(),
                    repository_id: h.repository.id.clone(),
                    base_ref: h.project.input.base_ref.clone(),
                    target_ref: h.project.input.target_ref.clone(),
                    room_id: None,
                    members: vec![moved.clone()],
                    checks: Vec::new(),
                },
            )
            .unwrap();
        let task = h
            .tasks
            .create(
                &db,
                TaskInput {
                    project_id: next.id.clone(),
                    title: "New project task".into(),
                    instructions: "Work here".into(),
                    acceptance: Vec::new(),
                    assignee_id: moved.bot_id.clone(),
                    reviewer_id: None,
                    tester_id: None,
                },
            )
            .unwrap();
        (previous, next, task)
    };
    h.tasks
        .start(&task.id, task.revision, "start-moved-member")
        .unwrap();
    let db = h.db.lock().unwrap();
    assert!(h.projects.busy(&db, &next.id).unwrap());
    assert!(!h.projects.busy(&db, &previous.id).unwrap());
    let archived = h
        .projects
        .archive(&db, &previous.id, previous.revision)
        .unwrap();
    assert_eq!(archived.state, ProjectState::Archived);
}
