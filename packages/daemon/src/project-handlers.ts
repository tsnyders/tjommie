import type { ProjectHandlers, ProjectMethod } from "../../protocol/src/projects.ts";
import { Projects } from "./projects.ts";
import type { RequestContext } from "./request-context.ts";
import { RpcError } from "./server.ts";
import { TaskIntegrations } from "./task-integration.ts";
import { Tasks } from "./tasks.ts";

export type WorkspaceHandlers = { [M in ProjectMethod]: (params: Parameters<ProjectHandlers[M]>[0], context?: RequestContext) => ReturnType<ProjectHandlers[M]> };
export function projectHandlers(projects: Projects, tasks: Tasks, integrations: TaskIntegrations): WorkspaceHandlers {
  const live = (context?: RequestContext): void => { if (context?.signal.aborted) throw new RpcError("bad_state", "The initiating connection closed."); };
  const configureChecks = (context?: RequestContext): void => {
    if (!context || context.principal.kind !== "local" || context.signal.aborted) throw new RpcError("forbidden", "Configure executable project checks on the host PC.");
  };
  const stop = async (id: string, revision: number, cancel: boolean) => {
    const task = tasks.expected(id, revision); if (task.currentAttemptId) integrations.stopAttempt(task.currentAttemptId);
    return tasks.stop(id, revision, cancel);
  };
  return {
    "projects.repositories": () => projects.repositories(),
    "projects.registerRepository": ({ path }, context) => {
      if (!context || context.principal.kind !== "local" || context.signal.aborted) throw new RpcError("bad_state", "Register repositories using the host PC's folder picker.");
      return projects.register(path);
    },
    "projects.list": () => projects.list(), "projects.get": ({ id }) => projects.get(id), "projects.heads": ({ id }) => projects.heads(id),
    "projects.create": (input, context) => { if (input.checks.length) configureChecks(context); return projects.create(input); },
    "projects.update": ({ id, expectedRevision, patch }, context) => {
      if (patch.checks && JSON.stringify(patch.checks) !== JSON.stringify(projects.get(id).checks)) configureChecks(context);
      return projects.update(id, expectedRevision, patch);
    },
    "projects.archive": ({ id, expectedRevision }) => projects.archive(id, expectedRevision),
    "tasks.list": ({ projectId }) => tasks.list(projectId), "tasks.create": (input) => tasks.create(input),
    "tasks.update": ({ id, expectedRevision, patch }) => tasks.update(id, expectedRevision, patch), "tasks.get": ({ id }) => tasks.detail(id),
    "tasks.start": ({ id, expectedRevision, requestId }, context) => { live(context); return tasks.start(id, expectedRevision, requestId); },
    "tasks.resume": ({ id, expectedRevision, requestId }, context) => { live(context); return tasks.resume(id, expectedRevision, requestId); },
    "tasks.stop": ({ id, expectedRevision }) => stop(id, expectedRevision, false), "tasks.cancel": ({ id, expectedRevision }) => stop(id, expectedRevision, true),
    "tasks.diff": ({ attemptId }) => tasks.diff(attemptId),
    "tasks.cleanupPreview": ({ attemptId }) => tasks.cleanupPreview(attemptId),
    "tasks.cleanup": ({ attemptId, expectedHeadSha }, context) => { live(context); return tasks.cleanup(attemptId, expectedHeadSha); },
    "tasks.evaluate": ({ attemptId, expectedRevision, sourceSha, kind }, context) => { live(context); return tasks.evaluate(attemptId, expectedRevision, sourceSha, kind); },
    "tasks.integrate": (input, context) => { live(context); return integrations.start(input); }, "tasks.promote": ({ id, expectedTargetSha }, context) => { live(context); return integrations.promote(id, expectedTargetSha); },
  };
}
