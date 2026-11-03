"use client";

import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { FolderGit2, Plus } from "lucide-react";
import type { Project, ProjectRepository, ProjectTask, TaskDetail, TaskDiff } from "../../../protocol/src/projects";
import { ProjectEditor } from "@/components/projects/project-editor";
import { TaskEditor } from "@/components/projects/task-editor";
import { TaskDetailView } from "@/components/projects/task-detail";
import { Pill } from "@/components/pill";
import { ConfirmDialog } from "@/components/dialog";
import { BotAvatar } from "@/components/bot-avatar";
import { ROW_BUTTON, RowHeader } from "@/components/bot-form/rows";
import { cn } from "@/lib/utils";
import { daemon, request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { phoneCaptionClear } from "@/lib/native-capability";
import { useApp } from "@/lib/store";
import { ifAlive, useAlive } from "@/lib/use-draft";

export default function ProjectsPage() {
  const host = useApp((state) => state.host);
  return <ProjectWorkspace key={host === null ? "local" : `remote-${host}`} />;
}

function ProjectWorkspace() {
  const ready = useApp((state) => state.connection.status === "ready"), bots = useApp((state) => state.bots);
  const [projects, setProjects] = useState<Project[]>([]), [repositories, setRepositories] = useState<ProjectRepository[]>([]), [tasks, setTasks] = useState<ProjectTask[]>([]);
  const [projectId, setProjectId] = useState<string | null>(null), [taskId, setTaskId] = useState<string | null>(null), [detail, setDetail] = useState<TaskDetail | null>(null), [diff, setDiff] = useState<TaskDiff | null>(null), [targetSha, setTargetSha] = useState<string | null>(null);
  const [revision, reload] = useState(0), [pending, setPending] = useState(0), [loading, setLoading] = useState(false), [problem, setProblem] = useState<string | null>(null);
  const busy = pending > 0;
  const [projectForm, setProjectForm] = useState<"new" | "edit" | null>(null), [taskForm, setTaskForm] = useState<"new" | "edit" | null>(null), alive = useAlive();
  const [archiving, setArchiving] = useState<Project | null>(null);
  const detailPanel = useRef<HTMLElement>(null), focusTask = useRef<string | null>(null);
  const project = projects.find((candidate) => candidate.id === projectId);
  useEffect(() => {
    const query = new URLSearchParams(window.location.search); setProjectId(query.get("project")); setTaskId(query.get("task"));
  }, []);
  useEffect(() => {
    if (!ready) return;
    let live = true;
    setLoading(true);
    const client = daemon();
    const unsubscribe = client.subscribe((event) => { if (event.event === "project.updated" || event.event.startsWith("task.")) reload((value) => value + 1); });
    const load = async (): Promise<void> => {
      try {
        const [nextProjects, nextRepositories, nextTasks, nextDetail, heads] = await Promise.all([
          request("projects.list", {}), request("projects.repositories", {}), projectId ? request("tasks.list", { projectId }) : Promise.resolve([]),
          taskId ? request("tasks.get", { id: taskId }) : Promise.resolve(null), projectId ? request("projects.heads", { id: projectId }) : Promise.resolve(null),
        ]);
        let nextDiff: TaskDiff | null = null;
        if (nextDetail?.task.currentAttemptId) {
          try { nextDiff = await request("tasks.diff", { attemptId: nextDetail.task.currentAttemptId }); } catch { /* A missing/provisioning worktree has an explicit attempt state. */ }
        }
        if (!live) return;
        setProjects(nextProjects); setRepositories(nextRepositories); setTasks(nextTasks); setDetail(nextDetail); setDiff(nextDiff); setTargetSha(heads?.targetSha ?? null); setProblem(null);
      } catch (error) { if (live) setProblem(errorText(error)); }
      finally { if (live) setLoading(false); }
    };
    void load(); return () => { live = false; unsubscribe(); };
  }, [ready, projectId, taskId, revision]);
  useEffect(() => {
    if (!detail || focusTask.current !== detail.task.id || !detailPanel.current) return;
    focusTask.current = null;
    detailPanel.current.scrollIntoView({ block: "start" });
    detailPanel.current.focus({ preventScroll: true });
  }, [detail]);
  function selectProject(id: string): void { focusTask.current = null; setProjectId(id); setTaskId(null); setDetail(null); setDiff(null); window.history.replaceState(null, "", `/projects/?project=${encodeURIComponent(id)}`); }
  function selectTask(id: string): void {
    focusTask.current = id;
    if (id === taskId) {
      if (detailPanel.current) {
        focusTask.current = null;
        detailPanel.current.scrollIntoView({ block: "start" });
        detailPanel.current.focus({ preventScroll: true });
      }
      return;
    }
    setTaskId(id); setDetail(null); setDiff(null);
    window.history.replaceState(null, "", `/projects/?project=${encodeURIComponent(projectId ?? "")}&task=${encodeURIComponent(id)}`);
  }
  async function action(operation: () => Promise<unknown>, concurrent = false): Promise<void> {
    if (busy && !concurrent || !ready) return; setPending((value) => value + 1); setProblem(null);
    try { await operation(); ifAlive(alive, () => reload((value) => value + 1)); }
    catch (error) { ifAlive(alive, () => setProblem(errorText(error))); }
    finally { ifAlive(alive, () => setPending((value) => value - 1)); }
  }
  return <div className="flex min-h-0 flex-1 flex-col">
    <header className="drag shrink-0 px-6 pb-7 pt-12 max-sm:px-4 max-sm:pb-3 max-sm:pt-4">
      <div className={cn("flex items-center justify-between gap-6 max-sm:gap-2", phoneCaptionClear())}>
        <div className="min-w-0 flex-1"><h1 className="text-[20px] font-semibold leading-[26px] text-title">Developer teams</h1><p className="mt-1 text-[13px] leading-[18px] text-secondary">Project tasks, isolated workspaces and exact-commit integration.</p></div>
        <Pill primary aria-label="New project" title="New project" className="max-sm:size-11 max-sm:justify-center max-sm:px-0" disabled={!ready || busy} onClick={() => setProjectForm("new")}><Plus className="size-3.5 max-sm:size-5" aria-hidden /><span className="max-sm:sr-only">New project</span></Pill>
      </div>
    </header>
    <main data-nav-clear className="min-h-0 flex-1 overflow-y-auto px-6 pb-6 max-sm:px-4">
      {!ready && <p role="status" className="text-[13px] leading-[18px] text-secondary">Connect to the host PC to view and run project work.</p>}
      {problem && <div className="mb-4 flex flex-wrap items-center gap-2"><p role="alert" className="text-[12px] leading-[18px] text-danger">{problem}</p><Pill className="max-sm:min-h-11" onClick={() => reload((value) => value + 1)}>Refresh</Pill></div>}
      {loading && <p role="status" className="mb-3 text-[12px] leading-[18px] text-secondary">Refreshing project state…</p>}
      <div className="grid min-w-0 gap-4 xl:grid-cols-[240px_minmax(0,1fr)]">
        <nav aria-label="Project teams" className="min-w-0"><ul className="space-y-2">{projects.map((item) => <li key={item.id}>
          <button type="button" title={item.name} aria-current={item.id === projectId ? "page" : undefined} className={cn(ROW_BUTTON, "h-auto! min-h-14 border border-card-stroke py-3 [&>span]:truncate", item.id === projectId ? "bg-selected text-text" : "bg-card text-secondary hover:bg-pill")} onClick={() => selectProject(item.id)}><RowHeader icon={FolderGit2} title={item.name} value={item.state === "archived" ? "Archived" : undefined} /></button>
        </li>)}</ul>{ready && !projects.length && !loading && <p className="text-[13px] leading-[18px] text-secondary">Create a project team to assign existing bots and prepare task drafts.</p>}</nav>
        <div className="min-w-0 space-y-4">{project && <><div className="flex flex-wrap items-start justify-between gap-4"><div className="min-w-0"><h2 className="break-words text-[17px] font-semibold leading-[22px] text-title">{project.name}</h2><p className="mt-1 break-all text-[12px] leading-[18px] text-secondary">{project.baseRef} → {project.targetRef}</p>{project.roomId && <Link className="mt-2 inline-flex min-h-8 items-center text-[13px] text-accent hover:underline max-sm:min-h-11" href={`/room/?id=${encodeURIComponent(project.roomId)}`}>Open team room</Link>}</div><div className="flex flex-wrap gap-2"><Pill className="max-sm:min-h-11" disabled={busy || project.state !== "active"} onClick={() => setProjectForm("edit")}>Edit team</Pill><Pill className="max-sm:min-h-11" disabled={busy || project.state !== "active"} onClick={() => setArchiving(project)}>Archive team</Pill><Pill primary className="max-sm:min-h-11" disabled={busy || project.state !== "active"} onClick={() => setTaskForm("new")}>New task</Pill></div></div>
          <ul aria-label="Project tasks" className="grid gap-4 sm:grid-cols-2 max-sm:gap-2">{tasks.map((task) => { const developer = bots.find((bot) => bot.id === task.assigneeId); return <li key={task.id}><button type="button" aria-pressed={taskId === task.id} className={cn("min-h-14 w-full rounded-[14px] border border-card-stroke p-4 text-left", taskId === task.id ? "bg-selected" : "bg-card hover:bg-pill")} onClick={() => selectTask(task.id)}><span className="block break-words text-[14px] font-medium leading-[21px] text-text">{task.title}</span><span className="mt-2 flex min-w-0 items-center gap-2">{developer && <BotAvatar color={developer.color} shape={developer.shape} size={18} className="shrink-0" />}<span className="text-[12px] capitalize leading-[18px] text-secondary">{task.state}</span></span></button></li>; })}</ul>
          {!tasks.length && <p className="text-[13px] leading-[18px] text-secondary">No tasks yet. Saving a task does not start provider work.</p>}
          {detail && detail.task.projectId === project.id && <TaskDetailView key={detail.task.id} focusRef={detailPanel} project={project} detail={detail} diff={diff} targetSha={targetSha} busy={busy} connected={ready} onAction={(operation, concurrent) => void action(operation, concurrent)} onEdit={() => setTaskForm("edit")} />}
        </>}</div>
      </div>
    </main>
    {archiving && <ConfirmDialog open onOpenChange={(open) => { if (!open) setArchiving(null); }} title={`Archive ${archiving.name}?`} body="You will no longer be able to edit this team or start new tasks. Existing branches and files are kept. This cannot be undone in Tjommie." confirm="Archive team" onConfirm={() => { const selected = archiving; setArchiving(null); void action(() => request("projects.archive", { id: selected.id, expectedRevision: selected.revision })); }} />}
    {projectForm && <ProjectEditor project={projectForm === "edit" ? project : undefined} repositories={repositories} bots={bots} onClose={() => setProjectForm(null)} onSaved={(saved) => { setProjectForm(null); selectProject(saved.id); reload((value) => value + 1); }} />}
    {taskForm && project && <TaskEditor project={project} task={taskForm === "edit" ? detail?.task : undefined} bots={bots} onClose={() => setTaskForm(null)} onSaved={(saved) => { setTaskForm(null); selectTask(saved.id); reload((value) => value + 1); }} />}
  </div>;
}
