"use client";

import { useRef, useState } from "react";
import type { Bot } from "@tjommie/protocol";
import { TaskInput, type Project, type ProjectTask } from "../../../protocol/src/projects";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { ifAlive, useAlive } from "@/lib/use-draft";
import { FIELD, Label } from "../controls";
import { Dialog } from "../dialog";
import { Pill } from "../pill";

export function TaskEditor({ project, task, bots, onClose, onSaved }: { project: Project; task?: ProjectTask; bots: Bot[]; onClose(): void; onSaved(task: ProjectTask): void }) {
  const [title, setTitle] = useState(task?.title ?? ""), [instructions, setInstructions] = useState(task?.instructions ?? ""), [acceptance, setAcceptance] = useState(task?.acceptance.join("\n") ?? "");
  const [assigneeId, setAssignee] = useState(task?.assigneeId ?? project.members.find((member) => member.roles.includes("developer"))?.botId ?? "");
  const [reviewerId, setReviewer] = useState(task?.reviewerId ?? project.members.find((member) => member.roles.includes("reviewer"))?.botId ?? "");
  const [testerId, setTester] = useState(task?.testerId ?? project.members.find((member) => member.roles.includes("tester"))?.botId ?? "");
  const [busy, setBusy] = useState(false), [problem, setProblem] = useState<string | null>(null), focus = useRef<HTMLInputElement>(null), alive = useAlive();
  async function save(): Promise<void> {
    if (busy) return; setProblem(null);
    try {
      const input = TaskInput.parse({ projectId: project.id, title, instructions, acceptance: acceptance.split("\n").map((line) => line.trim()).filter(Boolean), assigneeId, reviewerId: reviewerId || null, testerId: testerId || null });
      setBusy(true);
      const saved = task ? await request("tasks.update", { id: task.id, expectedRevision: task.revision, patch: { title: input.title, instructions: input.instructions, acceptance: input.acceptance, assigneeId: input.assigneeId, reviewerId: input.reviewerId, testerId: input.testerId } }) : await request("tasks.create", input);
      ifAlive(alive, () => onSaved(saved));
    } catch (error) { ifAlive(alive, () => { setProblem(errorText(error)); setBusy(false); }); }
  }
  return <Dialog open width={520} title={task ? "Edit task" : "New task"} closable initialFocus={focus} onOpenChange={(open) => { if (!open) onClose(); }}>
    <form className="mt-4 space-y-4" onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div><Label htmlFor="task-title">Title</Label><input id="task-title" ref={focus} className={`${FIELD} mt-1 h-11`} value={title} maxLength={200} onChange={(event) => setTitle(event.target.value)} required /></div>
      <div><Label htmlFor="task-instructions">Instructions</Label><textarea id="task-instructions" className={`${FIELD} mt-1 min-h-28 py-2`} value={instructions} onChange={(event) => setInstructions(event.target.value)} required /></div>
      <div><Label htmlFor="task-acceptance">Acceptance checklist (one item per line)</Label><textarea id="task-acceptance" className={`${FIELD} mt-1 min-h-24 py-2`} value={acceptance} onChange={(event) => setAcceptance(event.target.value)} /></div>
      {([ ["Developer", "developer", assigneeId, setAssignee], ["Reviewer", "reviewer", reviewerId, setReviewer], ["Tester", "tester", testerId, setTester] ] as const).map(([label, role, value, change]) => <div key={role}>
        <Label htmlFor={`task-${role}`}>{label}</Label><select id={`task-${role}`} className={`${FIELD} mt-1 h-11`} value={value} onChange={(event) => change(event.target.value)} required={role === "developer"}>
          <option value="">{role === "developer" ? "Choose a developer" : "Unassigned"}</option>{project.members.filter((member) => member.roles.includes(role)).map((member) => <option key={member.botId} value={member.botId}>{bots.find((bot) => bot.id === member.botId)?.name ?? "Unavailable bot"}</option>)}
        </select></div>)}
      <p className="text-xs text-secondary">Save creates a task draft. Start explicitly begins developer work and may use the bot&apos;s provider allowance. Integration needs a reviewer, tester and accepted checks.</p>
      {problem && <p role="alert" className="text-sm text-text">{problem}</p>}
      <div className="flex justify-end gap-2"><Pill className="min-h-11" onClick={onClose}>Cancel</Pill><Pill primary className="min-h-11" type="submit" disabled={busy || !title.trim() || !instructions.trim() || !assigneeId}>{busy ? "Saving…" : "Save task"}</Pill></div>
    </form>
  </Dialog>;
}
