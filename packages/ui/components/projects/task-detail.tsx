import { useState } from "react";
import type { Project, TaskCleanupPreview, TaskDetail as Detail, TaskDiff } from "../../../protocol/src/projects";
import { request } from "@/lib/connection";
import { canIntegrate, shortSha } from "@/lib/projects";
import { Pill } from "../pill";
import { TaskDiffView } from "./task-diff";

export function TaskDetailView({ project, detail, diff, targetSha, busy, connected, onAction, onEdit }: { project: Project; detail: Detail; diff: TaskDiff | null; targetSha: string | null; busy: boolean; connected: boolean; onAction(action: () => Promise<unknown>, concurrent?: boolean): void; onEdit(): void }) {
  const { task } = detail, attempt = detail.attempts.find((candidate) => candidate.id === task.currentAttemptId);
  const [cleanup, setCleanup] = useState<TaskCleanupPreview | null>(null);
  const running = !!attempt && ["provisioning", "queued", "running"].includes(attempt.state);
  const evaluating = detail.evaluations.some((evaluation) => evaluation.state === "running") || detail.integrations.some((integration) => ["preparing", "checking"].includes(integration.state));
  const disabled = busy || !connected || project.state !== "active";
  return <article className="min-w-0 space-y-5 rounded-2xl border border-divider bg-card p-4 sm:p-5" aria-label={`Task: ${task.title}`}>
    <div className="flex flex-wrap items-start justify-between gap-3"><div className="min-w-0"><h2 className="break-words text-lg font-semibold text-title">{task.title}</h2><p role="status" className="mt-1 text-sm capitalize text-secondary">{task.state} · {attempt ? `Attempt ${attempt.ordinal}: ${attempt.state}` : "Not started"}</p></div>
      <Pill className="min-h-11" disabled={disabled || running || evaluating} onClick={onEdit}>Edit task</Pill></div>
    <p className="whitespace-pre-wrap break-words text-sm text-text">{task.instructions}</p>
    {!!task.acceptance.length && <ul aria-label="Acceptance checklist" className="list-disc space-y-1 pl-5 text-sm text-secondary">{task.acceptance.map((item, index) => <li key={index}>{item}</li>)}</ul>}
    <div className="flex flex-wrap gap-2">
      <Pill primary className="min-h-11" disabled={disabled || running || evaluating} onClick={() => onAction(() => attempt && attempt.state !== "cleaned" ? request("tasks.resume", { id: task.id, expectedRevision: task.revision, requestId: crypto.randomUUID() }) : request("tasks.start", { id: task.id, expectedRevision: task.revision, requestId: crypto.randomUUID() }))}>{attempt && attempt.state !== "cleaned" ? "Return to developer" : "Start task"}</Pill>
      <Pill className="min-h-11" disabled={!connected || project.state !== "active" || !running && !evaluating} onClick={() => onAction(() => request("tasks.stop", { id: task.id, expectedRevision: task.revision }), true)}>Stop</Pill>
      <Pill className="min-h-11" disabled={!connected || project.state !== "active" || task.state === "cancelled"} onClick={() => onAction(() => request("tasks.cancel", { id: task.id, expectedRevision: task.revision }), true)}>Cancel task</Pill>
      {(["review", "test"] as const).map((kind) => <Pill key={kind} className="min-h-11" disabled={disabled || evaluating || attempt?.state !== "completed" || !!diff?.dirty} onClick={() => { if (attempt?.headSha) onAction(() => request("tasks.evaluate", { attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, kind })); }}>{kind === "review" ? "Run review" : "Run checks"}</Pill>)}
    </div>
    <p className="text-xs text-secondary">Start, Return to developer and Run review use the assigned bot&apos;s provider allowance. The first review verifies this host&apos;s tool restrictions before reviewing a frozen source snapshot. Unsupported accounts show an unavailable result. Stop and Cancel retain all branches and files.</p>
    {attempt && <section aria-label="Attempt workspace" className="space-y-1 text-xs text-secondary"><p>Branch: <code className="break-all text-text">{attempt.branch}</code></p><p>Worktree on host: <code className="break-all text-text">{attempt.worktreePath}</code></p><p>Commit: <code className="break-all text-text">{attempt.headSha ?? "Not recorded"}</code></p>{attempt.reason && <p role="status" className="pt-2 text-sm">{attempt.reason}</p>}</section>}
    {attempt && attempt.state !== "cleaned" && <section aria-label="Worktree cleanup"><Pill className="min-h-11" disabled={busy || !connected || running || evaluating} onClick={() => onAction(async () => setCleanup(await request("tasks.cleanupPreview", { attemptId: attempt.id })))}>Preview worktree cleanup</Pill>
      {cleanup && <div className="mt-3 rounded-xl border border-divider p-3 text-sm text-secondary"><p>{cleanup.reason}</p>{!!cleanup.files.length && <ul className="mt-2 list-disc pl-4">{cleanup.files.map((file) => <li key={file} className="break-all">{file}</li>)}</ul>}{cleanup.truncated && <p>More changed files are retained.</p>}
        {cleanup.canRemove && cleanup.headSha && <Pill className="mt-3 min-h-11" disabled={busy || !connected} onClick={() => onAction(async () => { await request("tasks.cleanup", { attemptId: cleanup.attemptId, expectedHeadSha: cleanup.headSha!, confirm: "remove_clean_worktree" }); setCleanup(null); })}>Remove this clean worktree</Pill>}</div>}
    </section>}
    <TaskDiffView diff={diff} />
    <section aria-label="Review and check history" className="space-y-3"><h3 className="text-sm font-medium text-title">Review and checks</h3>
      {!detail.evaluations.length && <p className="text-sm text-secondary">No independent results yet. A developer&apos;s reported checks do not count as verified results.</p>}
      {detail.evaluations.map((evaluation) => <div key={evaluation.id} className="rounded-xl border border-divider p-3 text-sm"><p className="capitalize text-text">{evaluation.kind} · {evaluation.state} · {shortSha(evaluation.sourceSha)}</p>
        {evaluation.reason && <p className="mt-1 break-words text-secondary">{evaluation.reason}</p>}
        {evaluation.kind === "review" && (evaluation.reviewCoverage ? <details className="mt-2" aria-label="Review coverage"><summary className="cursor-pointer text-secondary">Complete snapshot supplied · {evaluation.reviewCoverage.files.length} files</summary><p className="mt-2 break-all text-xs text-secondary">Snapshot SHA-256: {evaluation.reviewCoverage.snapshotSha256}</p><ul className="mt-2 space-y-1 text-xs text-secondary">{evaluation.reviewCoverage.files.map((file) => <li key={file.path} className="break-all">{file.path} · {file.bytes} bytes · {file.sha256}</li>)}</ul></details> : <p className="mt-1 text-xs text-secondary">Complete source coverage is unavailable for this result.</p>)}
        {!!evaluation.findings.length && <ul className="mt-2 space-y-2">{evaluation.findings.map((finding, index) => <li key={index} className="break-words text-secondary"><strong className="capitalize">{finding.severity}</strong> · {finding.path}{finding.line ? `:${finding.line}` : ""}: {finding.message}</li>)}</ul>}
        {evaluation.checks.map((check, index) => <details key={`${check.checkId}-${index}`} className="mt-2"><summary className="cursor-pointer break-words text-secondary">{check.command} {check.args.join(" ")} · exit {check.exitCode ?? "unavailable"}</summary><pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-words text-xs">{check.output}</pre>{check.truncated && <p className="text-xs text-secondary">Output was truncated.</p>}</details>)}
      </div>)}
    </section>
    <section aria-label="Local integration" className="space-y-3"><h3 className="text-sm font-medium text-title">Integrate locally</h3><p className="text-xs text-secondary">Accepted source: {shortSha(attempt?.headSha ?? null)} · current target: {shortSha(targetSha)}</p>
      <Pill primary className="min-h-11" disabled={disabled || !targetSha || !canIntegrate(project, detail, diff?.dirty ?? true)} onClick={() => { if (attempt?.headSha && targetSha) onAction(() => request("tasks.integrate", { attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: targetSha, requestId: crypto.randomUUID() })); }}>Integrate accepted commit</Pill>
      <p className="text-xs text-secondary">Requires passing review and checks for this exact commit. Combined checks run again in a separate candidate. A target checked out by the user stays untouched. Pushing and PRs are separate actions.</p>
      {detail.integrations.map((integration) => <div key={integration.id} className="rounded-xl border border-divider p-3 text-sm text-secondary"><p className="capitalize text-text">{integration.state} · {shortSha(integration.candidateSha)}</p>{integration.reason && <p className="mt-1 break-words">{integration.reason}</p>}
        <p className="mt-1 break-all text-xs">Retained candidate: {integration.worktreePath}</p>
        {integration.reason === "target_checked_out" && <p className="mt-2 text-xs">Save the target checkout&apos;s work and leave that branch, then retry promotion. Its index and files are preserved.</p>}
        {integration.candidateSha && integration.state === "blocked" && integration.reason === "target_checked_out" && <Pill className="mt-2 min-h-11" disabled={disabled} onClick={() => onAction(() => request("tasks.promote", { id: integration.id, expectedTargetSha: integration.expectedTargetSha }))}>Retry target promotion</Pill>}
      </div>)}
    </section>
  </article>;
}
