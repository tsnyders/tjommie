//! The first test of `projects.test.ts`. It needs lane A's `rooms.rs` (M4), which is not in this branch yet. It
//! assumes `daemon::rooms::{delete_room, get_room}` as they are in lane A's worktree:
//! `delete_room(&Connection, &str) -> Result<(), RpcError>` and `get_room(&Connection, &str) -> Result<Room, RpcError>`.
// TODO(M5-integration): `daemon::rooms` is lane A's (M4) and is not in `rs/core` yet, so this file compiles to nothing.
// When it lands: delete the `cfg(any())` line, and check the two signatures above.
#![cfg(any())]

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
