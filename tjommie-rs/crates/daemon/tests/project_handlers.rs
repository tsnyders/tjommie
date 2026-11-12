//! `project-handlers.test.ts`, through `Server::dispatch`. One deviation: the TypeScript test calls the handler with an
//! aborted signal and expects "connection closed" from the handler. `dispatch` refuses a closed connection itself,
//! before any handler runs (with another message), so the handler's own `live` check is tested directly here.

mod common;
mod project_support;

use daemon::project_handlers::{ProjectServices, live, local_and_live, register};
use daemon::server::{Handlers, Principal, RequestContext, Server, ServerOptions};
use daemon::task_integration::TaskIntegrations;
use daemon::tasks::TasksOptions;
use project_support::{Fixture, fixture};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn context(principal: Principal, signal: CancellationToken) -> RequestContext {
    RequestContext {
        connection_id: "synthetic".into(),
        principal,
        signal,
    }
}

fn local(signal: &CancellationToken) -> RequestContext {
    context(Principal::Local, signal.clone())
}

fn remote(signal: &CancellationToken) -> RequestContext {
    context(
        Principal::Device {
            device_id: "device-synthetic".into(),
        },
        signal.clone(),
    )
}

async fn serve(h: &Fixture) -> Server {
    let services = ProjectServices {
        projects: h.projects.clone(),
        tasks: h.tasks.clone(),
        integrations: TaskIntegrations::new(h.tasks.clone(), None, None),
    };
    Server::start(ServerOptions {
        port: 0,
        token: "token".into(),
        handlers: register(Handlers::default(), &services),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn requires_an_authenticated_live_local_context_to_register_host_paths_and_refuses_paid_starts_from_a_closed_connection() {
    let h = fixture(TasksOptions::default());
    let server = serve(&h).await;
    let signal = CancellationToken::new();
    let params = json!({ "path": h.repo_path });
    for context in [Some(remote(&signal)), None] {
        let error = server
            .dispatch("projects.registerRepository", params.clone(), context)
            .await
            .unwrap_err();
        assert!(error.message.contains("host PC"), "{}", error.message);
    }
    let registered = server
        .dispatch("projects.registerRepository", params, Some(local(&signal)))
        .await
        .unwrap();
    assert_eq!(
        registered,
        json!({ "id": h.repository.id, "name": h.repository.name, "createdAt": h.repository.created_at })
    );
    // A connection that closed can no longer start paid work, and a local one can still register.
    let task = h.new_task(0);
    signal.cancel();
    let closed = Some(local(&signal));
    assert!(!local_and_live(&closed));
    let error = live(&closed).unwrap_err();
    assert!(error.message.contains("connection closed"), "{}", error.message);
    let error = server
        .dispatch(
            "tasks.start",
            json!({ "id": task.id, "expectedRevision": task.revision, "requestId": "closed" }),
            closed,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("closed"), "{}", error.message);
    assert!(h.runner.delivered().is_empty());
    server.close().await;
}

#[tokio::test]
async fn keeps_executable_check_configuration_host_local_while_allowing_remote_team_edits() {
    let h = fixture(TasksOptions::default());
    let server = serve(&h).await;
    let signal = CancellationToken::new();
    let changed: Value = {
        let mut check = serde_json::to_value(&h.project.input.checks[0]).unwrap();
        check["command"] = json!("unaccepted-program");
        json!([check])
    };
    let update = |revision: u64, patch: Value| json!({ "id": h.project.id, "expectedRevision": revision, "patch": patch });
    let error = server
        .dispatch(
            "projects.update",
            update(h.project.revision, json!({ "checks": changed })),
            Some(remote(&signal)),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("host PC"), "{}", error.message);
    let mut copy = serde_json::to_value(&h.project.input).unwrap();
    copy["name"] = json!("Remote copy");
    copy["checks"] = changed.clone();
    let error = server
        .dispatch("projects.create", copy, Some(remote(&signal)))
        .await
        .unwrap_err();
    assert!(error.message.contains("host PC"), "{}", error.message);
    let renamed = server
        .dispatch(
            "projects.update",
            update(
                h.project.revision,
                json!({ "name": "Remote edit", "checks": h.project.input.checks }),
            ),
            Some(remote(&signal)),
        )
        .await
        .unwrap();
    assert_eq!(renamed["name"], "Remote edit");
    assert_eq!(renamed["checks"], serde_json::to_value(&h.project.input.checks).unwrap());
    let edited = server
        .dispatch(
            "projects.update",
            update(h.project.revision + 1, json!({ "checks": changed })),
            Some(local(&signal)),
        )
        .await
        .unwrap();
    assert_eq!(edited["checks"], changed);
    server.close().await;
}
