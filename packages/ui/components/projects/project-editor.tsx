"use client";

import { useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { Check, FolderGit2 } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { z } from "zod";
import { ProjectCheck, ProjectInput, type Project, type ProjectMember, type ProjectRepository, ProjectRole } from "../../../protocol/src/projects";
import { request } from "@/lib/connection";
import { localFileActions } from "@/lib/native-capability";
import { errorText } from "@/lib/labels";
import { ifAlive, useAlive } from "@/lib/use-draft";
import { FIELD, Label, Switch } from "../controls";
import { BotAvatar } from "../bot-avatar";
import { ROW_BUTTON, RowHeader } from "../bot-form/rows";
import { MENU_ITEM, MENU_POPUP } from "../menu";
import { Dialog } from "../dialog";
import { Pill } from "../pill";

export function ProjectEditor({ project, repositories, bots, onClose, onSaved }: { project?: Project; repositories: ProjectRepository[]; bots: Bot[]; onClose(): void; onSaved(project: Project): void }) {
  const [name, setName] = useState(project?.name ?? ""), [repositoryId, setRepositoryId] = useState(project?.repositoryId ?? repositories[0]?.id ?? "");
  const [localRepositories, setRepositories] = useState(repositories), [baseRef, setBaseRef] = useState(project?.baseRef ?? "refs/heads/main"), [targetRef, setTargetRef] = useState(project?.targetRef ?? "refs/heads/main");
  const [members, setMembers] = useState<ProjectMember[]>(project?.members ?? []), [checks, setChecks] = useState(JSON.stringify(project?.checks ?? [], null, 2));
  const [busy, setBusy] = useState(false), [problem, setProblem] = useState<string | null>(null), focus = useRef<HTMLInputElement>(null), alive = useAlive();
  function role(botId: string, value: z.infer<typeof ProjectRole>, checked: boolean): void {
    setMembers((current) => {
      const member = current.find((candidate) => candidate.botId === botId) ?? { botId, roles: [], instructions: "" };
      const roles = checked ? [...member.roles, value] : member.roles.filter((candidate) => candidate !== value);
      return [...current.filter((candidate) => candidate.botId !== botId), ...(roles.length ? [{ ...member, roles }] : [])];
    });
  }
  async function register(): Promise<void> {
    const native = localFileActions(); if (!native || busy) return;
    setBusy(true); setProblem(null);
    try {
      const path = await native.pickFolder(); if (!path) return;
      const saved = await request("projects.registerRepository", { path });
      ifAlive(alive, () => { setRepositories((current) => [...current.filter((repo) => repo.id !== saved.id), saved]); setRepositoryId(saved.id); });
    } catch (error) { ifAlive(alive, () => setProblem(errorText(error))); }
    finally { ifAlive(alive, () => setBusy(false)); }
  }
  async function save(): Promise<void> {
    if (busy) return; setProblem(null);
    try {
      const parsedChecks: unknown = JSON.parse(checks);
      const input = ProjectInput.parse({ name, repositoryId, baseRef, targetRef, roomId: project?.roomId ?? null, members, checks: z.array(ProjectCheck).max(20).parse(parsedChecks) });
      setBusy(true);
      const saved = project ? await request("projects.update", { id: project.id, expectedRevision: project.revision, patch: { name: input.name, baseRef: input.baseRef, targetRef: input.targetRef, members: input.members, checks: input.checks } }) : await request("projects.create", input);
      ifAlive(alive, () => onSaved(saved));
    } catch (error) { ifAlive(alive, () => { setProblem(errorText(error)); setBusy(false); }); }
  }
  const repository = localRepositories.find((item) => item.id === repositoryId);
  return <Dialog open title={project ? "Edit project team" : "New project team"} width={600} closable initialFocus={focus} onOpenChange={(open) => { if (!open) onClose(); }}>
    <form className="mt-4 min-w-0 space-y-4" onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div><Label htmlFor="project-name">Project name</Label><input id="project-name" ref={focus} className={`${FIELD} mt-1 h-11`} value={name} maxLength={80} onChange={(event) => setName(event.target.value)} required /></div>
      <div><Label htmlFor="project-repo">Repository on this host</Label>
        <Menu.Root><Menu.Trigger id="project-repo" aria-label="Repository on this host" title={repository?.name ?? "Choose a registered repository"} disabled={!!project || busy || !localRepositories.length} className={`${ROW_BUTTON} mt-1 min-w-0 border border-input-stroke bg-card disabled:opacity-50 sm:[&>span:first-of-type]:min-w-max max-sm:grid max-sm:grid-cols-[auto_minmax(0,1fr)_auto] max-sm:gap-y-1 max-sm:[&>span]:col-start-2 max-sm:[&>span]:max-w-full max-sm:[&>svg]:row-span-2 max-sm:[&>svg:last-child]:col-start-3 max-sm:[&>svg:last-child]:row-start-1`}>
          <RowHeader icon={FolderGit2} title="Repository" value={<span className="truncate">{repository?.name ?? "Choose a registered repository"}</span>} chevron="down" />
        </Menu.Trigger><Menu.Portal><Menu.Positioner side="bottom" align="start" sideOffset={6} className="z-[60]"><Menu.Popup className={`${MENU_POPUP} z-50 max-h-[min(320px,60dvh)] max-w-[calc(100vw-32px)] overflow-y-auto`}>
          <Menu.RadioGroup value={repositoryId} onValueChange={(value: unknown) => { if (typeof value === "string") setRepositoryId(value); }}>
            {localRepositories.map((repo) => <Menu.RadioItem key={repo.id} value={repo.id} closeOnClick className={MENU_ITEM}><FolderGit2 aria-hidden /><span className="min-w-0 flex-1 truncate">{repo.name}</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>)}
          </Menu.RadioGroup>
        </Menu.Popup></Menu.Positioner></Menu.Portal></Menu.Root>
        {!project && localFileActions() ? <Pill className="mt-2 h-auto! min-h-8 max-w-full whitespace-normal! py-2 max-sm:min-h-11" disabled={busy} onClick={() => void register()}>Choose a repository folder</Pill> : <p className="mt-2 text-[12px] leading-[18px] text-secondary">Repositories are registered on the host PC. Saved downloads and folders on this device are separate.</p>}
      </div>
      <div className="grid gap-3 sm:grid-cols-2"><div><Label htmlFor="project-base">Base branch ref</Label><input id="project-base" className={`${FIELD} mt-1 h-11`} value={baseRef} onChange={(event) => setBaseRef(event.target.value)} required /></div>
        <div><Label htmlFor="project-target">Integration target ref</Label><input id="project-target" className={`${FIELD} mt-1 h-11`} value={targetRef} onChange={(event) => setTargetRef(event.target.value)} required /></div></div>
      <fieldset className="min-w-0"><legend className="text-[13px] font-semibold leading-[18px] text-label">Agents and roles</legend><p className="mt-1 text-[12px] leading-[18px] text-secondary">One active project per agent. Existing messaging, account and autonomy choices still apply.</p>
        {bots.map((bot) => { const member = members.find((candidate) => candidate.botId === bot.id); return <div key={bot.id} className="mt-3 rounded-[12px] border border-divider p-3">
          <div className="flex min-w-0 items-center gap-3"><BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" /><p className="min-w-0 truncate text-[14px] leading-[21px] text-text">{bot.name}</p></div>
          <div className="mt-2 grid gap-x-4 sm:grid-cols-3">{ProjectRole.options.map((value) => <div key={value} className="flex min-h-11 items-center justify-between gap-3 text-[13px] capitalize leading-[18px] text-secondary [&_[role=switch]]:before:absolute [&_[role=switch]]:before:-inset-x-1 [&_[role=switch]]:before:-inset-y-3 [&_[role=switch]]:before:content-['']"><span>{value}</span><Switch label={`${bot.name}: ${value}`} checked={member?.roles.includes(value) ?? false} onChange={(checked) => role(bot.id, value, checked)} /></div>)}</div>
          {member && <textarea aria-label={`${bot.name} role instructions`} className={`${FIELD} min-h-20 py-2`} placeholder="Editable role instructions" value={member.instructions} onChange={(event) => setMembers((current) => current.map((candidate) => candidate.botId === bot.id ? { ...candidate, instructions: event.target.value } : candidate))} />}
        </div>; })}
      </fieldset>
      <div><Label htmlFor="project-checks">Accepted checks (JSON array)</Label><textarea id="project-checks" className={`${FIELD} mt-1 min-h-32 py-2 font-mono text-[12px] leading-[18px]`} value={checks} readOnly={!localFileActions()} onChange={(event) => setChecks(event.target.value)} />
        {!localFileActions() && <p className="mt-2 text-[12px] leading-[18px] text-secondary">Executable checks are configured on the host PC. You can run its accepted checks from this device.</p>}
        <p className="mt-2 text-[12px] leading-[18px] text-secondary">Each check needs id, label, command and args. Checks run project code only after Evaluate or Integrate. Full auto permits host execution; other autonomy levels need an available isolated executor. Saving does not run them.</p>
        <p className="mt-1 break-all text-[12px] leading-[18px] text-secondary">Example: [{'{"id":"test","label":"Tests","command":"node","args":["--test"],"timeoutMs":120000}'}]</p></div>
      {problem && <p role="alert" className="break-words text-[12px] leading-[18px] text-danger">{problem}</p>}
      <div className="flex flex-wrap justify-end gap-2"><Pill className="max-sm:min-h-11" onClick={onClose}>Cancel</Pill><Pill primary className="max-sm:min-h-11" type="submit" disabled={busy || !name.trim() || !repositoryId || !members.length}>{busy ? "Saving…" : "Save team"}</Pill></div>
    </form>
  </Dialog>;
}
