//! `project-handlers.ts`: the `projects.*` and `tasks.*` methods. Each handler's synchronous part runs on the blocking
//! pool (see `Handlers::on`), so it may take the database lock and run git. Async parts never hold the lock.

use std::sync::Arc;

use protocol::wire::method;
use protocol::ErrorCode;

use crate::projects::Projects;
use crate::server::{Handlers, Principal, RequestContext, RpcError};
use crate::task_integration::TaskIntegrations;
use crate::tasks::{Tasks, bad_state};

/// What the handlers share.
#[derive(Clone)]
pub struct ProjectServices {
    pub projects: Arc<Projects>,
    pub tasks: Arc<Tasks>,
    pub integrations: Arc<TaskIntegrations>,
}

/// `context.principal.kind === "local"` on a live connection.
pub fn local_and_live(context: &Option<RequestContext>) -> bool {
    context
        .as_ref()
        .is_some_and(|context| context.principal == Principal::Local && !context.signal.is_cancelled())
}

/// `live`: a connection that closed can no longer start paid work. Public so the closed-connection case can be tested
/// directly: `Server::dispatch` refuses a closed connection before any handler runs.
pub fn live(context: &Option<RequestContext>) -> Result<(), RpcError> {
    if context
        .as_ref()
        .is_some_and(|context| context.signal.is_cancelled())
    {
        return Err(bad_state("The initiating connection closed."));
    }
    Ok(())
}

/// `configureChecks`: executable project checks are configured on the host PC only.
fn configure_checks(context: &Option<RequestContext>) -> Result<(), RpcError> {
    if !local_and_live(context) {
        return Err(RpcError::new(
            ErrorCode::Forbidden,
            "Configure executable project checks on the host PC.",
        ));
    }
    Ok(())
}

/// `stop`: ends the integration preparations of the attempt, then the attempt.
async fn stop_task(
    services: &ProjectServices,
    id: &str,
    revision: u64,
    cancel: bool,
) -> Result<protocol::projects::ProjectTask, RpcError> {
    {
        let db = services.tasks.lock();
        let task = services.tasks.expected(&db, id, revision)?;
        if let Some(attempt_id) = &task.current_attempt_id {
            services.integrations.stop_attempt(&db, attempt_id)?;
        }
    }
    services.tasks.stop(id, revision, cancel).await
}

/// Registers the `projects.*` and `tasks.*` methods.
pub fn register(handlers: Handlers, services: &ProjectServices) -> Handlers {
    let s = services.clone();
    let (repositories, register_repository, list, get, heads) =
        (s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    let (create, update, archive) = (s.clone(), s.clone(), s.clone());
    let (tasks_list, tasks_create, tasks_update, tasks_get) =
        (s.clone(), s.clone(), s.clone(), s.clone());
    let (start, resume, stop, cancel) = (s.clone(), s.clone(), s.clone(), s.clone());
    let (diff, cleanup_preview, cleanup) = (s.clone(), s.clone(), s.clone());
    let (evaluate, integrate, promote) = (s.clone(), s.clone(), s.clone());
    handlers
        .on::<method::ProjectsRepositories, _>(move |_, _| {
            let result = repositories
                .projects
                .repositories(&repositories.projects.lock());
            async move { result }
        })
        .on::<method::ProjectsRegisterRepository, _>(move |params, context| {
            let result = if local_and_live(&context) {
                register_repository
                    .projects
                    .register(&register_repository.projects.lock(), &params.path)
            } else {
                Err(bad_state(
                    "Register repositories using the host PC's folder picker.",
                ))
            };
            async move { result }
        })
        .on::<method::ProjectsList, _>(move |_, _| {
            let result = list.projects.list(&list.projects.lock());
            async move { result }
        })
        .on::<method::ProjectsGet, _>(move |params, _| {
            let result = get.projects.get(&get.projects.lock(), &params.id);
            async move { result }
        })
        .on::<method::ProjectsHeads, _>(move |params, _| {
            let result = heads.projects.heads(&heads.projects.lock(), &params.id);
            async move { result }
        })
        .on::<method::ProjectsCreate, _>(move |input, context| {
            let result = (|| {
                if !input.checks.is_empty() {
                    configure_checks(&context)?;
                }
                create.projects.create(&create.projects.lock(), input)
            })();
            async move { result }
        })
        .on::<method::ProjectsUpdate, _>(move |params, context| {
            let result = (|| {
                let db = update.projects.lock();
                if let Some(checks) = &params.patch.checks
                    && *checks != update.projects.get(&db, &params.id)?.input.checks
                {
                    configure_checks(&context)?;
                }
                update
                    .projects
                    .update(&db, &params.id, params.expected_revision, params.patch)
            })();
            async move { result }
        })
        .on::<method::ProjectsArchive, _>(move |params, _| {
            let result = archive.projects.archive(
                &archive.projects.lock(),
                &params.id,
                params.expected_revision,
            );
            async move { result }
        })
        .on::<method::TasksList, _>(move |params, _| {
            let result = tasks_list
                .tasks
                .list(&tasks_list.tasks.lock(), &params.project_id);
            async move { result }
        })
        .on::<method::TasksCreate, _>(move |input, _| {
            let result = tasks_create.tasks.create(&tasks_create.tasks.lock(), input);
            async move { result }
        })
        .on::<method::TasksUpdate, _>(move |params, _| {
            let result = tasks_update.tasks.update(
                &tasks_update.tasks.lock(),
                &params.id,
                params.expected_revision,
                params.patch,
            );
            async move { result }
        })
        .on::<method::TasksGet, _>(move |params, _| {
            let result = tasks_get.tasks.detail(&tasks_get.tasks.lock(), &params.id);
            async move { result }
        })
        .on::<method::TasksStart, _>(move |params, context| {
            let result = live(&context).and_then(|()| {
                start
                    .tasks
                    .start(&params.id, params.expected_revision, &params.request_id)
            });
            async move { result }
        })
        .on::<method::TasksResume, _>(move |params, context| {
            let result = live(&context).and_then(|()| {
                resume
                    .tasks
                    .resume(&params.id, params.expected_revision, &params.request_id)
            });
            async move { result }
        })
        .on::<method::TasksStop, _>(move |params, _| {
            let services = stop.clone();
            async move { stop_task(&services, &params.id, params.expected_revision, false).await }
        })
        .on::<method::TasksCancel, _>(move |params, _| {
            let services = cancel.clone();
            async move { stop_task(&services, &params.id, params.expected_revision, true).await }
        })
        .on::<method::TasksDiff, _>(move |params, _| {
            let result = diff.tasks.diff(&diff.tasks.lock(), &params.attempt_id);
            async move { result }
        })
        .on::<method::TasksCleanupPreview, _>(move |params, _| {
            let result = cleanup_preview
                .tasks
                .cleanup_preview(&cleanup_preview.tasks.lock(), &params.attempt_id);
            async move { result }
        })
        .on::<method::TasksCleanup, _>(move |params, context| {
            let result = live(&context).and_then(|()| {
                cleanup.tasks.cleanup(
                    &cleanup.tasks.lock(),
                    &params.attempt_id,
                    &params.expected_head_sha,
                )
            });
            async move { result }
        })
        .on::<method::TasksEvaluate, _>(move |params, context| {
            let ready = live(&context);
            let services = evaluate.clone();
            async move {
                ready?;
                services
                    .tasks
                    .evaluate(
                        &params.attempt_id,
                        params.expected_revision,
                        &params.source_sha,
                        params.kind,
                    )
                    .await
            }
        })
        .on::<method::TasksIntegrate, _>(move |params, context| {
            let result = live(&context).and_then(|()| integrate.integrations.start(&params));
            async move { result }
        })
        .on::<method::TasksPromote, _>(move |params, context| {
            let result = live(&context).and_then(|()| {
                promote.integrations.promote(
                    &promote.tasks.lock(),
                    &params.id,
                    &params.expected_target_sha,
                )
            });
            async move { result }
        })
}
