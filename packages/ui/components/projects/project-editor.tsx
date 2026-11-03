"use client";

import { useRef, useState } from "react";
import type { Bot } from "@tjommie/protocol";
import { z } from "zod";
import { ProjectCheck, ProjectInput, type Project, type ProjectMember, type ProjectRepository, ProjectRole } from "../../../protocol/src/projects";
import { request } from "@/lib/connection";
import { localFileActions } from "@/lib/native-capability";
import { errorText } from "@/lib/labels";
import { ifAlive, useAlive } from "@/lib/use-draft";
import { FIELD, Label } from "../controls";
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
  return <Dialog open title={project ? "Edit project team" : "New project team"} width={600} closable initialFocus={focus} onOpenChange={(open) => { if (!open) onClose(); }}>
    <form className="mt-4 space-y-4" onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div><Label htmlFor="project-name">Project name</Label><input id="project-name" ref={focus} className={`${FIELD} mt-1 h-11`} value={name} maxLength={80} onChange={(event) => setName(event.target.value)} required /></div>
      <div><Label htmlFor="project-repo">Repository on this host</Label><select id="project-repo" className={`${FIELD} mt-1 h-11`} value={repositoryId} onChange={(event) => setRepositoryId(event.target.value)} disabled={!!project || busy} required>
        <option value="">Choose a registered repository</option>{localRepositories.map((repo) => <option key={repo.id} value={repo.id}>{repo.name}</option>)}
      </select>{!project && localFileActions() ? <Pill className="mt-2 min-h-11" disabled={busy} onClick={() => void register()}>Choose a repository folder</Pill> : <p className="mt-2 text-xs text-secondary">Repositories are registered on the host PC. Saved downloads and folders on this device are separate.</p>}</div>
      <div className="grid gap-3 sm:grid-cols-2"><div><Label htmlFor="project-base">Base branch ref</Label><input id="project-base" className={`${FIELD} mt-1 h-11`} value={baseRef} onChange={(event) => setBaseRef(event.target.value)} required /></div>
        <div><Label htmlFor="project-target">Integration target ref</Label><input id="project-target" className={`${FIELD} mt-1 h-11`} value={targetRef} onChange={(event) => setTargetRef(event.target.value)} required /></div></div>
      <fieldset><legend className="text-sm font-medium text-title">Bots and roles</legend><p className="mt-1 text-xs text-secondary">One active project per bot. Existing messaging, account and autonomy choices still apply.</p>
        {bots.map((bot) => { const member = members.find((candidate) => candidate.botId === bot.id); return <div key={bot.id} className="mt-3 rounded-xl border border-divider p-3">
          <p className="text-sm text-text">{bot.name}</p><div className="flex flex-wrap gap-x-4">{ProjectRole.options.map((value) => <label key={value} className="flex min-h-11 items-center gap-2 text-sm capitalize text-secondary"><input type="checkbox" aria-label={`${bot.name}: ${value}`} checked={member?.roles.includes(value) ?? false} onChange={(event) => role(bot.id, value, event.target.checked)} />{value}</label>)}</div>
          {member && <textarea aria-label={`${bot.name} role instructions`} className={`${FIELD} min-h-20 py-2`} placeholder="Editable role instructions" value={member.instructions} onChange={(event) => setMembers((current) => current.map((candidate) => candidate.botId === bot.id ? { ...candidate, instructions: event.target.value } : candidate))} />}
        </div>; })}
      </fieldset>
      <div><Label htmlFor="project-checks">Accepted checks (JSON array)</Label><textarea id="project-checks" className={`${FIELD} mt-1 min-h-32 py-2 font-mono text-xs`} value={checks} readOnly={!localFileActions()} onChange={(event) => setChecks(event.target.value)} />
        {!localFileActions() && <p className="mt-2 text-xs text-secondary">Executable checks are configured on the host PC. You can run its accepted checks from this device.</p>}
        <p className="mt-2 text-xs text-secondary">Each check needs id, label, command and args. Checks run project code only after Evaluate or Integrate. Full auto permits host execution; other autonomy levels need an available isolated executor. Saving does not run them.</p>
        <p className="mt-1 break-all text-xs text-secondary">Example: [{'{"id":"test","label":"Tests","command":"node","args":["--test"],"timeoutMs":120000}'}]</p></div>
      {problem && <p role="alert" className="break-words text-sm text-text">{problem}</p>}
      <div className="flex justify-end gap-2"><Pill className="min-h-11" onClick={onClose}>Cancel</Pill><Pill primary className="min-h-11" type="submit" disabled={busy || !name.trim() || !repositoryId || !members.length}>{busy ? "Saving…" : "Save team"}</Pill></div>
    </form>
  </Dialog>;
}
