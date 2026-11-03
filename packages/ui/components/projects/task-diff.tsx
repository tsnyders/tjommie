import type { TaskDiff } from "../../../protocol/src/projects";

export function TaskDiffView({ diff }: { diff: TaskDiff | null }) {
  return <section aria-label="Task diff" className="min-w-0">
    <h3 className="text-sm font-medium text-title">Committed changes</h3>
    {diff ? <>
      {diff.dirty && <p role="status" className="mt-2 text-sm text-secondary">The worktree has changes outside this commit. Review and integration are blocked until they are committed.</p>}
      <pre tabIndex={0} aria-label="Committed diff" className="mt-2 max-h-96 overflow-auto rounded-xl border border-stroke bg-card p-3 text-xs text-text">{diff.text || "No committed changes from the attempt base."}</pre>
      {diff.truncated && <p role="status" className="mt-2 text-sm text-secondary">This diff exceeds the preview limit. Automated review cannot claim complete coverage.</p>}
    </> : <p className="mt-2 text-sm text-secondary">The diff is available after the attempt has a managed worktree.</p>}
  </section>;
}
