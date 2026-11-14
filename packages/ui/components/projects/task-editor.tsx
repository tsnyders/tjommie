"use client";

import { useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { Check, UserRound } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { TaskInput, type Project, type ProjectTask } from "../../../protocol/src/projects";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { ifAlive, useAlive } from "@/lib/use-draft";
import { FIELD, Label } from "../controls";
import { Dialog } from "../dialog";
import { Pill } from "../pill";
import { BotAvatar } from "../bot-avatar";
import { ROW_BUTTON, RowHeader } from "../bot-form/rows";
import { MENU_ITEM, MENU_POPUP } from "../menu";

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
    <form className="mt-4 min-w-0 space-y-4" onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div><Label htmlFor="task-title">Title</Label><input id="task-title" ref={focus} className={`${FIELD} mt-1 h-11`} value={title} maxLength={200} onChange={(event) => setTitle(event.target.value)} required /></div>
      <div><Label htmlFor="task-instructions">Instructions</Label><textarea id="task-instructions" className={`${FIELD} mt-1 min-h-28 py-2`} value={instructions} onChange={(event) => setInstructions(event.target.value)} required /></div>
      <div><Label htmlFor="task-acceptance">Acceptance checklist (one item per line)</Label><textarea id="task-acceptance" className={`${FIELD} mt-1 min-h-24 py-2`} value={acceptance} onChange={(event) => setAcceptance(event.target.value)} /></div>
      {([ ["Developer", "developer", assigneeId, setAssignee], ["Reviewer", "reviewer", reviewerId, setReviewer], ["Tester", "tester", testerId, setTester] ] as const).map(([label, role, value, change]) => <div key={role}>
        <Label htmlFor={`task-${role}`}>{label}</Label>
        <Menu.Root><Menu.Trigger id={`task-${role}`} aria-label={label} title={bots.find((bot) => bot.id === value)?.name ?? (role === "developer" ? "Choose a developer" : "Unassigned")} className={`${ROW_BUTTON} mt-1 min-w-0 border border-input-stroke bg-card sm:[&>span:first-of-type]:min-w-max max-sm:grid max-sm:grid-cols-[auto_minmax(0,1fr)_auto] max-sm:gap-y-1 max-sm:[&>span]:col-start-2 max-sm:[&>span]:max-w-full max-sm:[&>svg]:row-span-2 max-sm:[&>svg:last-child]:col-start-3 max-sm:[&>svg:last-child]:row-start-1`}>
          <RowHeader icon={UserRound} title={label} value={<span className="truncate">{bots.find((bot) => bot.id === value)?.name ?? (role === "developer" ? "Choose a developer" : "Unassigned")}</span>} chevron="down" />
        </Menu.Trigger><Menu.Portal><Menu.Positioner side="bottom" align="start" sideOffset={6} className="z-[60]"><Menu.Popup className={`${MENU_POPUP} z-50 max-h-[min(320px,60dvh)] max-w-[calc(100vw-32px)] overflow-y-auto`}>
          <Menu.RadioGroup value={value} onValueChange={(next: unknown) => { if (typeof next === "string") change(next); }}>
            <Menu.RadioItem value="" closeOnClick className={MENU_ITEM}><span className="min-w-0 flex-1">{role === "developer" ? "Choose a developer" : "Unassigned"}</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>
            {project.members.filter((member) => member.roles.includes(role)).map((member) => { const bot = bots.find((candidate) => candidate.id === member.botId); return <Menu.RadioItem key={member.botId} value={member.botId} closeOnClick className={MENU_ITEM}>{bot && <BotAvatar color={bot.color} shape={bot.shape} size={18} className="shrink-0" />}<span className="min-w-0 flex-1 truncate">{bot?.name ?? "Unavailable agent"}</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>; })}
          </Menu.RadioGroup>
        </Menu.Popup></Menu.Positioner></Menu.Portal></Menu.Root>
      </div>)}
      <p className="text-[12px] leading-[18px] text-secondary">Save creates a task draft. Start explicitly begins developer work and may use the agent&apos;s provider allowance. Integration needs a reviewer, tester and accepted checks.</p>
      {problem && <p role="alert" className="text-[12px] leading-[18px] text-danger">{problem}</p>}
      <div className="flex flex-wrap justify-end gap-2"><Pill className="max-sm:min-h-11" onClick={onClose}>Cancel</Pill><Pill primary className="max-sm:min-h-11" type="submit" disabled={busy || !title.trim() || !instructions.trim() || !assigneeId}>{busy ? "Saving…" : "Save task"}</Pill></div>
    </form>
  </Dialog>;
}
