import { Project, ProjectInput, ProjectPatch, type ProjectRepository } from "../../protocol/src/projects.ts";
import { getStoredBot } from "./bots.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { createRoom, getRoom } from "./rooms.ts";
import { RpcError, type Broadcast } from "./server.ts";
import { Worktrees, type Repository } from "./worktrees.ts";

export function storedJson(row: { data: string } | undefined): unknown {
  if (!row) throw new RpcError("not_found", "No such project record.");
  const value: unknown = JSON.parse(row.data);
  return value;
}

export class Projects {
  readonly db: Db;
  readonly worktrees: Worktrees;
  readonly emit: Broadcast;
  readonly #assertAvailable: (botId: string) => void;
  constructor(db: Db, worktrees: Worktrees, emit: Broadcast = () => {}, assertAvailable: (botId: string) => void = () => {}) {
    this.db = db; this.worktrees = worktrees; this.emit = emit; this.#assertAvailable = assertAvailable;
  }

  repositories(): ProjectRepository[] {
    return (this.db.prepare("SELECT id,name,created_at FROM project_repositories ORDER BY rowid").all() as { id: string; name: string; created_at: string }[])
      .map((row) => ({ id: row.id, name: row.name, createdAt: row.created_at }));
  }
  repository(id: string): Repository {
    const row = this.db.prepare("SELECT * FROM project_repositories WHERE id=?").get(id) as { id: string; path: string; common_dir: string; name: string; created_at: string } | undefined;
    if (!row) throw new RpcError("not_found", "No such host repository.");
    return { id: row.id, path: row.path, commonDir: row.common_dir, name: row.name, createdAt: row.created_at };
  }
  register(path: string): ProjectRepository {
    const repo = this.worktrees.repository(path);
    const found = this.db.prepare("SELECT id FROM project_repositories WHERE common_dir=?").get(repo.commonDir) as { id: string } | undefined;
    if (found) { const saved = this.repository(found.id); return { id: saved.id, name: saved.name, createdAt: saved.createdAt }; }
    const record = { id: newId(), ...repo, createdAt: nowIso() };
    this.db.prepare("INSERT INTO project_repositories VALUES (?,?,?,?,?)").run(record.id, record.path, record.commonDir, record.name, record.createdAt);
    return { id: record.id, name: record.name, createdAt: record.createdAt };
  }
  list(): Project[] { return (this.db.prepare("SELECT data FROM projects ORDER BY rowid").all() as { data: string }[]).map((row) => Project.parse(storedJson(row))); }
  get(id: string): Project { return Project.parse(storedJson(this.db.prepare("SELECT data FROM projects WHERE id=?").get(id) as { data: string } | undefined)); }
  active(id: string): Project { const project = this.get(id); if (project.state !== "active") throw new RpcError("bad_state", "This project is archived."); return project; }
  heads(id: string): { baseSha: string; targetSha: string } {
    const project = this.get(id), repo = this.repository(project.repositoryId);
    return { baseSha: this.worktrees.resolveRef(repo, project.baseRef), targetSha: this.worktrees.resolveRef(repo, project.targetRef) };
  }

  busy(projectId: string): boolean {
    return !!this.db.prepare("SELECT 1 FROM task_attempts a JOIN project_tasks t ON t.id=a.task_id WHERE t.project_id=? AND a.state IN ('provisioning','queued','running') LIMIT 1").get(projectId)
      || !!this.db.prepare("SELECT 1 FROM project_bot_leases l JOIN project_members m ON m.bot_id=l.bot_id WHERE m.project_id=? AND m.active=1 LIMIT 1").get(projectId);
  }
  botInUse(botId: string): boolean {
    return !!this.db.prepare("SELECT 1 FROM project_members WHERE bot_id=? AND active=1").get(botId);
  }
  assertBotIdle(botId: string): void {
    this.#assertAvailable(botId);
    if (this.db.prepare("SELECT 1 FROM work_items WHERE bot_id=? AND status IN ('queued','running') LIMIT 1").get(botId)) throw new RpcError("busy", "This bot has queued or running work.");
    if (this.db.prepare("SELECT 1 FROM project_bot_leases WHERE bot_id=?").get(botId)) throw new RpcError("busy", "This bot owns a task or check that has not finished.");
  }
  #members(project: Project, except: string | null): void {
    const ids = new Set<string>();
    for (const member of project.members) {
      if (ids.has(member.botId) || new Set(member.roles).size !== member.roles.length) throw new RpcError("bad_params", "Each bot and role must occur once.");
      ids.add(member.botId); getStoredBot(this.db, member.botId); this.assertBotIdle(member.botId);
      const other = this.db.prepare("SELECT project_id FROM project_members WHERE bot_id=? AND active=1 AND project_id IS NOT ?").get(member.botId, except);
      if (other) throw new RpcError("busy", "This bot already belongs to another active project.");
    }
    if (new Set(project.checks.map((check) => check.id)).size !== project.checks.length) throw new RpcError("bad_params", "Project check IDs must be unique.");
  }
  #save(project: Project): void {
    this.db.prepare("UPDATE projects SET state=?,revision=?,data=? WHERE id=?").run(project.state, project.revision, JSON.stringify(project), project.id);
    this.db.prepare("UPDATE project_members SET active=0 WHERE project_id=?").run(project.id);
    for (const member of project.members) this.db.prepare("INSERT INTO project_members VALUES (?,?,?) ON CONFLICT(project_id,bot_id) DO UPDATE SET active=excluded.active")
      .run(project.id, member.botId, project.state === "active" ? 1 : 0);
  }
  create(input: ProjectInput): Project {
    input = ProjectInput.parse(input);
    const repo = this.repository(input.repositoryId);
    this.worktrees.resolveRef(repo, input.baseRef); this.worktrees.resolveRef(repo, input.targetRef);
    const timestamp = nowIso();
    const project: Project = { ...input, id: newId(), state: "active", revision: 1, createdAt: timestamp, updatedAt: timestamp };
    this.#members(project, null);
    if (project.roomId) getRoom(this.db, project.roomId);
    else project.roomId = createRoom(this.db, { name: project.name.slice(0, 60), botIds: project.members.map((member) => member.botId) }).id;
    transaction(this.db, () => {
      this.db.prepare("INSERT INTO projects VALUES (?,?,?,?,?)").run(project.id, project.repositoryId, project.state, project.revision, JSON.stringify(project));
      this.#save(project);
    });
    if (input.roomId === null && project.roomId) this.emit("room.updated", getRoom(this.db, project.roomId));
    this.emit("project.updated", project); return project;
  }
  update(id: string, expectedRevision: number, patch: ProjectPatch): Project {
    const previous = this.active(id);
    if (previous.revision !== expectedRevision) throw new RpcError("bad_state", "This project changed. Refresh it before saving.");
    if (this.busy(id)) throw new RpcError("busy", "Stop project work before changing its configuration.");
    const project = Project.parse({ ...previous, ...ProjectPatch.parse(patch), revision: previous.revision + 1, updatedAt: nowIso() });
    this.#members(project, id);
    const repo = this.repository(project.repositoryId);
    this.worktrees.resolveRef(repo, project.baseRef); this.worktrees.resolveRef(repo, project.targetRef);
    if (project.roomId) getRoom(this.db, project.roomId);
    transaction(this.db, () => this.#save(project));
    this.emit("project.updated", project); return project;
  }
  archive(id: string, expectedRevision: number): Project {
    const previous = this.get(id);
    if (previous.revision !== expectedRevision) throw new RpcError("bad_state", "This project changed. Refresh it before archiving.");
    if (this.busy(id)) throw new RpcError("busy", "Stop project work before archiving its team.");
    const project: Project = { ...previous, state: "archived", revision: previous.revision + 1, updatedAt: nowIso() };
    transaction(this.db, () => this.#save(project));
    this.emit("project.updated", project); return project;
  }
}
