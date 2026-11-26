//! Projects (T5.3): `project-workspace.test.tsx`, `task-detail.test.tsx` and `task-diff.test.tsx` one for one, through
//! the store's fake. Sample data is invented.

use gpui::{IntoElement as _, TestAppContext};
use protocol::model::BotState;
use protocol::projects::{Project, ProjectTask, TaskDetail, TaskDiff};
use protocol::wire::method;
use serde_json::{Value, json};

use super::detail::diff_view;
use crate::model::testing::FakeRpc;
use crate::ui::shell::route::Route;
use crate::ui::testing::{
    app, bot, click, dialog_fits, dialog_fits_with_problem, draw, ids, node, state, text,
};

const STAMP: &str = "2026-11-03T00:00:00.000Z";

fn project_json(members: Value) -> Value {
    json!({
        "id": "project", "name": "UI team", "repositoryId": "repo", "roomId": null,
        "baseRef": "refs/heads/main", "targetRef": "refs/heads/phase4", "members": members, "checks": [],
        "state": "active", "revision": 7, "createdAt": STAMP, "updatedAt": STAMP
    })
}

fn project(members: Value) -> Project {
    serde_json::from_value(project_json(members)).unwrap()
}

fn task(index: usize, title: &str) -> ProjectTask {
    serde_json::from_value(json!({
        "id": format!("task-{index}"), "projectId": "project", "title": title, "instructions": "Review UI",
        "acceptance": [], "assigneeId": "dev", "reviewerId": "review", "testerId": "test", "state": "todo",
        "currentAttemptId": null, "revision": 1, "createdAt": STAMP, "updatedAt": STAMP
    }))
    .unwrap()
}

fn detail_of(task: &ProjectTask, evaluations: Value) -> TaskDetail {
    serde_json::from_value(json!({
        "task": serde_json::to_value(task).unwrap(), "attempts": [], "evaluations": evaluations, "integrations": []
    }))
    .unwrap()
}

/// The workspace's fake daemon: one team (`members`) with two tasks; `evaluations` for every task's detail.
fn rpc(members: Value, evaluations: Value) -> FakeRpc {
    let team = project(members);
    let tasks = vec![task(0, "First task"), task(1, "Second task")];
    let (listed, archived, updated) = (team.clone(), team.clone(), team.clone());
    let (all, read, saved) = (
        tasks.clone(),
        tasks.clone(),
        tasks.first().cloned().unwrap(),
    );
    FakeRpc::new()
        .on::<method::ProjectsList>(move |_| Ok(vec![listed.clone()]))
        .on::<method::ProjectsRepositories>(|_| Ok(vec![]))
        .on::<method::ProjectsHeads>(|_| {
            Ok(serde_json::from_value(
                json!({ "baseSha": "b".repeat(40), "targetSha": "c".repeat(40) }),
            )
            .unwrap())
        })
        .on::<method::TasksList>(move |_| Ok(all.clone()))
        .on::<method::TasksGet>(move |params| {
            let id = params["id"].as_str().unwrap_or_default();
            let task = read.iter().find(|task| task.id == id).cloned().unwrap();
            Ok(detail_of(&task, evaluations.clone()))
        })
        .on::<method::ProjectsArchive>(move |_| {
            let mut gone = archived.clone();
            gone.state = protocol::projects::ProjectState::Archived;
            Ok(gone)
        })
        .on::<method::ProjectsUpdate>(move |_| Ok(updated.clone()))
        .on::<method::TasksUpdate>(move |_| Ok(saved.clone()))
}

fn calls_of(calls: &[(&'static str, Value)], method: &str) -> Vec<Value> {
    calls
        .iter()
        .filter(|(name, _)| *name == method)
        .map(|(_, params)| params.clone())
        .collect()
}

fn people() -> Vec<protocol::model::Bot> {
    ["dev", "dev2", "review", "test"]
        .into_iter()
        .map(|id| bot(id, id, BotState::Idle))
        .collect()
}

/// `project-workspace.test.tsx`: "requires a separate confirmation to archive the selected team and preserves its
/// revision".
#[gpui::test]
fn archiving_a_team_asks_first_and_sends_its_revision(cx: &mut TestAppContext) {
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let (cx, calls) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, json!([])),
    );
    assert_eq!(text("screen.projects"), "Developer teams");
    click(cx, "projects.team.project");
    assert_eq!(text("projects.name"), "UI team");
    assert_eq!(text("projects.refs"), "refs/heads/main → refs/heads/phase4");
    click(cx, "projects.archive");
    assert!(calls_of(&calls.calls(), "projects.archive").is_empty());
    assert_eq!(text("dialog.title"), "Archive UI team?");
    click(cx, "dialog.cancel");
    assert!(calls_of(&calls.calls(), "projects.archive").is_empty());
    click(cx, "projects.archive");
    click(cx, "dialog.confirm");
    assert_eq!(
        calls_of(&calls.calls(), "projects.archive"),
        [json!({ "id": "project", "expectedRevision": 7 })]
    );
}

/// `project-workspace.test.tsx`: "brings a selected task into view and focus once, without stealing focus on
/// background refresh".
#[gpui::test]
fn a_chosen_task_takes_the_focus_once_and_a_refresh_leaves_it(cx: &mut TestAppContext) {
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let (cx, calls) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, json!([])),
    );
    click(cx, "projects.team.project");
    assert_eq!(
        ids("projects.task."),
        ["projects.task.task-0", "projects.task.task-1"]
    );
    click(cx, "projects.task.task-1");
    draw(cx);
    assert_eq!(text("projects.detail"), "Task: Second task");
    assert!(
        node("projects.detail").unwrap().focused,
        "the detail has the focus"
    );
    // The focus moves on; a background refresh doesn't take it back.
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    assert!(!node("projects.detail").unwrap().focused);
    let reads = calls_of(&calls.calls(), "tasks.get").len();
    cx.update(|_, cx| {
        crate::ui::store::fake_event(cx, &protocol::Event::ProjectUpdated(project(json!([]))))
    });
    draw(cx);
    draw(cx);
    assert!(
        calls_of(&calls.calls(), "tasks.get").len() > reads,
        "a project event read it again"
    );
    assert!(
        !node("projects.detail").unwrap().focused,
        "and left the focus where it was"
    );
    // Choosing it again brings it again.
    click(cx, "projects.task.task-1");
    draw(cx);
    assert!(node("projects.detail").unwrap().focused);
}

/// `project-workspace.test.tsx`: "uses the existing role switches without changing repository identity or starting
/// work when saving a team".
#[gpui::test]
fn editing_a_team_keeps_its_repository_and_saves_only_on_save(cx: &mut TestAppContext) {
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let bots = vec![bot("dev", "Developer", BotState::Idle)];
    let (cx, calls) = app(
        cx,
        Route::Projects,
        state(bots, vec![], vec![]),
        rpc(members, json!([])),
    );
    click(cx, "projects.team.project");
    click(cx, "projects.edit");
    assert_eq!(text("dialog.title"), "Edit project team");
    // The repository can't change once the team exists.
    click(cx, "project_form.repository");
    assert!(
        ids("project_form.repository.")
            .iter()
            .all(|id| id.ends_with(".value") || id.ends_with(".menu")),
        "no repositories to choose"
    );
    assert!(node("project_form.role.dev.developer").unwrap().toggled);
    click(cx, "project_form.role.dev.tester");
    assert!(node("project_form.role.dev.tester").unwrap().toggled);
    assert!(calls_of(&calls.calls(), "projects.update").is_empty());
    click(cx, "project_form.save");
    assert_eq!(
        calls_of(&calls.calls(), "projects.update"),
        [json!({
            "id": "project", "expectedRevision": 7,
            "patch": {
                "name": "UI team", "baseRef": "refs/heads/main", "targetRef": "refs/heads/phase4",
                "members": [{ "botId": "dev", "roles": ["developer", "tester"], "instructions": "" }], "checks": []
            }
        })]
    );
    assert!(
        node("project_form.save").is_none(),
        "saved: the editor closed"
    );
    assert!(
        calls_of(&calls.calls(), "tasks.start").is_empty(),
        "saving starts no work"
    );
}

/// `project-workspace.test.tsx`: "keeps task pickers limited to assigned roles and submits the selected developer
/// with the original revision".
#[gpui::test]
fn task_pickers_offer_the_role_holders_and_save_with_the_revision(cx: &mut TestAppContext) {
    let members = json!([
        { "botId": "dev", "roles": ["developer"] }, { "botId": "dev2", "roles": ["developer"] },
        { "botId": "review", "roles": ["reviewer"] }, { "botId": "test", "roles": ["tester"] }
    ]);
    let (cx, calls) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, json!([])),
    );
    click(cx, "projects.team.project");
    click(cx, "projects.task.task-0");
    click(cx, "projects.detail.edit");
    assert_eq!(text("dialog.title"), "Edit task");
    assert_eq!(text("task_form.developer.value"), "dev");
    click(cx, "task_form.developer");
    let offered: Vec<String> = ids("task_form.developer.")
        .into_iter()
        .filter(|id| !id.ends_with(".value") && !id.ends_with(".menu"))
        .collect();
    assert_eq!(
        offered,
        [
            "task_form.developer.none",
            "task_form.developer.dev",
            "task_form.developer.dev2"
        ],
        "only developers"
    );
    click(cx, "task_form.developer.dev2");
    assert!(calls_of(&calls.calls(), "tasks.update").is_empty());
    click(cx, "task_form.save");
    assert_eq!(
        calls_of(&calls.calls(), "tasks.update"),
        [json!({
            "id": "task-0", "expectedRevision": 1,
            "patch": {
                "title": "First task", "instructions": "Review UI", "acceptance": [], "assigneeId": "dev2",
                "reviewerId": "review", "testerId": "test"
            }
        })]
    );
}

/// `task-detail.test.tsx`: "shows the exact supplied coverage manifest and explains paid enforcement acceptance".
#[gpui::test]
fn a_review_shows_its_coverage_manifest_or_says_it_has_none(cx: &mut TestAppContext) {
    let source = "a".repeat(40);
    let evaluation = |coverage: Value| {
        json!([{
            "id": "e", "attemptId": "a", "revision": 1, "projectRevision": 1, "sourceSha": source, "kind": "review",
            "botId": "reviewer", "worktreePath": null, "state": "passed", "reason": null, "findings": [], "checks": [],
            "createdAt": STAMP, "updatedAt": STAMP, "reviewCoverage": coverage
        }])
    };
    let coverage = json!({
        "sourceSha": source, "snapshotSha256": "b".repeat(64), "diffSha256": "c".repeat(64), "complete": true,
        "files": [{ "path": "source.ts", "bytes": 10, "sha256": "d".repeat(64) }]
    });
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let (cx, _) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, evaluation(coverage)),
    );
    click(cx, "projects.team.project");
    click(cx, "projects.task.task-0");
    assert_eq!(
        text("projects.open.coverage.e"),
        "Complete snapshot supplied · 1 files"
    );
    click(cx, "projects.open.coverage.e");
    assert!(text("projects.coverage.file.0").contains("source.ts"));
    assert!(text("projects.coverage.snapshot").contains(&"b".repeat(64)));
    assert!(text("projects.detail.allowance").contains("first review verifies"));
    assert!(node("projects.evaluation.e.coverage").is_none());
}

#[gpui::test]
fn a_review_without_coverage_says_so(cx: &mut TestAppContext) {
    let evaluation = json!([{
        "id": "e", "attemptId": "a", "revision": 1, "projectRevision": 1, "sourceSha": "a".repeat(40),
        "kind": "review", "botId": "reviewer", "worktreePath": null, "state": "passed", "reason": null,
        "findings": [], "checks": [], "createdAt": STAMP, "updatedAt": STAMP, "reviewCoverage": null
    }]);
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let (cx, _) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, evaluation),
    );
    click(cx, "projects.team.project");
    click(cx, "projects.task.task-0");
    assert_eq!(
        text("projects.evaluation.e.coverage"),
        "Complete source coverage is unavailable for this result."
    );
    assert!(node("projects.open.coverage.e").is_none());
}

/// `task-diff.test.tsx`: "renders source as text and exposes dirty/truncated limitations accessibly".
#[gpui::test]
fn the_diff_shows_as_text_with_its_limits(cx: &mut TestAppContext) {
    let diff: TaskDiff = serde_json::from_value(json!({
        "attemptId": "a", "revision": 1, "sourceSha": "a".repeat(40), "baseSha": "b".repeat(40),
        "text": "<script>bad()</script>", "truncated": true, "dirty": true
    }))
    .unwrap();
    let cx = crate::ui::primitives::test_window(cx, move |_, cx| {
        diff_view(Some(&diff), cx).into_any_element()
    });
    draw(cx);
    assert_eq!(text("projects.diff.text"), "Committed diff");
    assert_eq!(text("projects.diff.text.body"), "<script>bad()</script>");
    assert!(text("projects.diff.dirty").contains("outside this commit"));
    assert!(text("projects.diff.truncated").contains("cannot claim complete coverage"));
}

#[gpui::test]
fn offline_projects_say_how_to_see_them(cx: &mut TestAppContext) {
    let offline = crate::model::state::AppState {
        connection: crate::model::state::Connection::Connecting,
        ..state(people(), vec![], vec![])
    };
    let (_, calls) = app(cx, Route::Projects, offline, FakeRpc::new());
    assert_eq!(
        text("projects.offline"),
        "Connect to the host PC to view and run project work."
    );
    assert!(
        calls_of(&calls.calls(), "projects.list").is_empty(),
        "nothing is asked meanwhile"
    );
}

/// M5 review M4: another PC starts Projects afresh (the TypeScript's `key={host}`): the last PC's team and task aren't
/// chosen there, and nothing of theirs is asked for.
#[gpui::test]
fn another_pc_starts_projects_with_nothing_chosen(cx: &mut TestAppContext) {
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let (cx, calls) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, json!([])),
    );
    click(cx, "projects.team.project");
    click(cx, "projects.task.task-1");
    draw(cx);
    assert_eq!(text("projects.detail"), "Task: Second task");
    let before = calls.calls().len();
    cx.update(|_, cx| {
        crate::ui::store::update(cx, |state| {
            state.host = Some("pc-2".into());
            true
        })
    });
    draw(cx);
    assert!(
        node("projects.detail").is_none(),
        "no task chosen on the other PC"
    );
    assert!(
        node("projects.name").is_none(),
        "no team chosen on the other PC"
    );
    let after = calls.calls().split_off(before);
    assert!(
        !calls_of(&after, "projects.list").is_empty(),
        "the other PC's teams are read"
    );
    for method in ["tasks.list", "tasks.get", "projects.heads"] {
        assert!(
            calls_of(&after, method).is_empty(),
            "the last PC's {method} asked of the other: {after:?}"
        );
    }
}

/// A narrow window (360 × 600, the window's minimum): the sidebar is the compact navigation, the Projects header wraps
/// with New project on screen, and the navigation still works from the keyboard (`app-shell.tsx` and `page.tsx` under
/// `sm`).
#[gpui::test]
fn a_narrow_window_has_compact_navigation_and_projects_fits(cx: &mut TestAppContext) {
    use gpui::{px, size};
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let (cx, _) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        rpc(members, json!([])),
    );
    assert!(node("sidebar.wordmark").is_some(), "wide: the full sidebar");
    cx.simulate_resize(size(px(360.), px(600.)));
    draw(cx);
    assert!(
        node("sidebar.compact").is_some(),
        "narrow: the compact navigation"
    );
    assert!(node("sidebar.wordmark").is_none() && node("sidebar.view_all").is_none());
    // M7 review m1: switching PCs stays at hand, at the rail's foot.
    let host = node("sidebar.host").expect("the host button at the rail's foot");
    assert!(
        host.bounds.x + host.bounds.width <= 56. && host.bounds.width <= 48.,
        "a face in the rail: {:?}",
        host.bounds
    );
    let new = node("projects.new").expect("New project");
    assert!(
        new.bounds.x >= 0. && new.bounds.x + new.bounds.width <= 360.,
        "New project is on screen: {:?}",
        new.bounds
    );
    let title = node("screen.projects").expect("the title");
    assert!(
        title.bounds.width >= 200.,
        "the title has a line of its own, not a word per line: {:?}",
        title.bounds
    );
    assert!(
        new.bounds.y < title.bounds.y,
        "New project above the title, as `page.tsx` put it"
    );
    let rooms = node("sidebar.nav.rooms").expect("the Rooms link");
    assert!(
        rooms.bounds.width <= 48.,
        "an icon link: {:?}",
        rooms.bounds
    );
    // The keys still go: Enter on a focused section opens it.
    cx.update(|window, cx| {
        assert!(crate::ui::primitives::focus_by_id(
            "sidebar.nav.rooms",
            window,
            cx
        ));
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(
        cx.update(|_, cx| crate::ui::shell::route::current(cx)),
        Route::Rooms
    );
    // The host button's menu opens by Enter, as the full sidebar's does.
    cx.update(|window, cx| {
        assert!(crate::ui::primitives::focus_by_id(
            "sidebar.host",
            window,
            cx
        ));
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(
        node("host.connect").is_some(),
        "its menu offers the other PCs"
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    // Wide again: the full sidebar comes back.
    cx.simulate_resize(size(px(1000.), px(700.)));
    draw(cx);
    assert!(node("sidebar.wordmark").is_some());
}

/// The dialog fit (every dialog's check): New project team, Edit project team, New task, Edit task and Archive, at
/// 1000 x 600 and at 360 x 600.
#[gpui::test]
fn dialog_fits_every_projects_dialog(cx: &mut TestAppContext) {
    let members = json!([{ "botId": "dev", "roles": ["developer"] }]);
    let refusing = rpc(members, json!([])).on::<method::ProjectsUpdate>(|_| {
        Err(client::RpcError::Daemon {
            code: protocol::ErrorCode::Internal,
            message: "The host is busy.".into(),
        })
    });
    let (cx, _) = app(
        cx,
        Route::Projects,
        state(people(), vec![], vec![]),
        refusing,
    );
    let team = ["project_form.cancel", "project_form.save"];
    let task = ["task_form.cancel", "task_form.save"];
    click(cx, "projects.new");
    assert_eq!(text("dialog.title"), "New project team");
    dialog_fits(cx, &team);
    click(cx, "project_form.cancel");
    click(cx, "projects.team.project");
    click(cx, "projects.edit");
    dialog_fits(cx, &team);
    click(cx, "project_form.save");
    dialog_fits_with_problem(cx, "project_form.problem", &team);
    click(cx, "project_form.cancel");
    click(cx, "projects.new_task");
    assert_eq!(text("dialog.title"), "New task");
    dialog_fits(cx, &task);
    click(cx, "task_form.cancel");
    click(cx, "projects.task.task-0");
    click(cx, "projects.detail.edit");
    assert_eq!(text("dialog.title"), "Edit task");
    dialog_fits(cx, &task);
    click(cx, "task_form.cancel");
    click(cx, "projects.archive");
    dialog_fits(cx, &["dialog.cancel", "dialog.confirm"]);
}
