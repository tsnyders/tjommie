import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import type { TaskAttempt } from "../../protocol/src/projects.ts";
import { createBot } from "./bots.ts";
import { openDb } from "./db.ts";
import { PROJECT_MIGRATION } from "./project-schema.ts";
import { Projects } from "./projects.ts";
import { getWorkItem, setStatus, type WorkItem } from "./queue.ts";
import { ensureAccounts } from "./setup.ts";
import { Tasks, type TaskRunner } from "./tasks.ts";
import { tempRoot } from "./testing.ts";
import { Worktrees } from "./worktrees.ts";

export function projectFixture(options: ConstructorParameters<typeof Tasks>[2] = {}) {
  const root = tempRoot(), repoPath = join(root, "owner"), managed = join(root, "managed"); mkdirSync(repoPath);
  const emptyConfig = join(root, "empty-gitconfig"); writeFileSync(emptyConfig, "");
  const env: NodeJS.ProcessEnv = { GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: emptyConfig, GIT_AUTHOR_NAME: "Synthetic fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Synthetic fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
  for (const key of ["PATH", "SystemRoot", "SystemDrive", "WINDIR", "PATHEXT", "TEMP", "TMP", "COMSPEC"]) if (process.env[key] !== undefined) env[key] = process.env[key];
  const git = (path: string, ...args: string[]): string => {
    assert(!relative(realpathSync.native(root), realpathSync.native(path)).startsWith(".."));
    return execFileSync("git", args, { cwd: path, env, encoding: "utf8", windowsHide: true, stdio: ["pipe", "pipe", "pipe"] }).trim();
  };
  git(repoPath, "init", "-b", "main"); git(repoPath, "config", "user.name", "Synthetic fixture"); git(repoPath, "config", "user.email", "fixture@example.invalid"); git(repoPath, "config", "commit.gpgsign", "false");
  mkdirSync(join(repoPath, "src")); writeFileSync(join(repoPath, "src/a.txt"), "A0\n"); writeFileSync(join(repoPath, "src/b.txt"), "B0\n");
  writeFileSync(join(repoPath, ".gitignore"), "check-output.txt\n"); writeFileSync(join(repoPath, "check.mjs"), 'import{writeFileSync}from"node:fs";writeFileSync("check-output.txt","passed");');
  git(repoPath, "add", "."); git(repoPath, "commit", "-m", "Synthetic base"); const base = git(repoPath, "rev-parse", "HEAD");
  const db = openDb(join(root, "test.sqlite"));
  if (!db.prepare("SELECT name FROM sqlite_master WHERE name='projects'").get()) db.exec(PROJECT_MIGRATION);
  ensureAccounts(db, join(root, "accounts"));
  const bots = ["Developer A", "Developer B", "Reviewer", "Tester"].map((name) => createBot(db, join(root, "bots"), { name, provider: "claude", model: "test", color: "blue", shape: "circle", instructions: "", autonomy: "full" }));
  const worktrees = new Worktrees(managed), projects = new Projects(db, worktrees), repository = projects.register(repoPath);
  const project = projects.create({ name: "Synthetic project", repositoryId: repository.id, baseRef: "refs/heads/main", targetRef: "refs/heads/main", roomId: null,
    members: bots.map((bot, index) => ({ botId: bot.id, roles: index < 2 ? ["developer"] : index === 2 ? ["reviewer"] : ["tester"], instructions: "" })),
    checks: [{ id: "check", label: "Synthetic accepted check", command: process.execPath, args: ["check.mjs"], timeoutMs: 1000 }] });
  const delivered: WorkItem[] = [], active = new Map<string, WorkItem>();
  let tasks: Tasks;
  const runner: TaskRunner = {
    deliver(item) { delivered.push(item); active.set(item.botId, item); setStatus(db, item.id, "running"); tasks.turnStarted(item); return item; },
    currentWork(botId) { return active.get(botId) ?? null; },
    async stopItem(id) { const item = getWorkItem(db, id); setStatus(db, id, "interrupted"); active.delete(item.botId); tasks.turnEnded(item, { outcome: "interrupted", text: null, textMessageId: null, error: null }); },
    retired: () => Promise.resolve(),
  };
  tasks = new Tasks(projects, runner, options);
  const newTask = (index = 0) => tasks.create({ projectId: project.id, title: `Task ${index}`, instructions: "Change the assigned file.", acceptance: ["Check the change"], assigneeId: bots[index].id, reviewerId: bots[2].id, testerId: bots[3].id });
  const complete = async (attempt: TaskAttempt, file: string, bytes: string) => {
    writeFileSync(join(attempt.worktreePath, file), bytes); git(attempt.worktreePath, "add", "--", file); git(attempt.worktreePath, "commit", "-m", "Synthetic task commit");
    const item = getWorkItem(db, attempt.workItemId!); setStatus(db, item.id, "done"); active.delete(item.botId);
    tasks.turnEnded(item, { outcome: "done", text: "Completed", textMessageId: null, error: null });
    await new Promise<void>((resolve) => setImmediate(resolve)); return tasks.attempt(attempt.id);
  };
  return { root, repoPath, base, db, bots, worktrees, projects, repository: projects.repository(repository.id), project, tasks, runner, delivered, git, newTask, complete,
    owner: () => ({ head: git(repoPath, "rev-parse", "HEAD"), status: git(repoPath, "status", "--porcelain=v1"), index: readFileSync(join(repoPath, ".git/index")).toString("base64") }) };
}
