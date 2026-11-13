//! The first test of `projects.test.ts`, against lane A's `rooms.rs`: `delete_room(&Connection, &str)` and
//! `get_room(&Connection, &str) -> Result<Room, RpcError>`.

mod common;
mod project_support;

use daemon::rooms::{delete_room, get_room};
use daemon::tasks::TasksOptions;
use project_support::fixture;

#[tokio::test]
async fn retains_an_idle_active_projects_room_until_its_project_is_archived_or_unlinked() {
    let h = fixture(TasksOptions::default());
    let room_id = h.project.input.room_id.clone().unwrap();
    let db = h.db.lock().unwrap();
    let error = delete_room(&db, &room_id).unwrap_err();
    assert!(
        error.message.contains("Archive or unlink"),
        "{}",
        error.message
    );
    assert_eq!(get_room(&db, &room_id).unwrap().id, room_id);
    h.projects
        .archive(&db, &h.project.id, h.project.revision)
        .unwrap();
    delete_room(&db, &room_id).unwrap();
    assert!(get_room(&db, &room_id).is_err());
}
