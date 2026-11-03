import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, test } from "node:test";
import { captureCheckout, createFixture, prepareIntegration, promoteIntegration, reviewPolicy, sessionKey, snapshotReader } from "./probe.mjs";

const fixtures = [];
afterEach(() => { for (const fixture of fixtures.splice(0)) fixture.dispose(); });
const fixture = () => { const created = createFixture(); fixtures.push(created); return created; };
const check = { command: process.execPath, args: ["check.mjs"] };
const accepted = (repo, id, commit, targetRef = "refs/heads/main", expectedTarget = repo.base) => ({ id, targetRef, expectedTarget,
  acceptedCommit: commit, reviewedCommit: commit, testedCommit: commit, attemptPath: repo.commits.get(commit), check });

function dirtyOwner(repo) {
  writeFileSync(join(repo.repo, "README.md"), "staged owner change\n");
  repo.git(repo.repo, "add", "--", "README.md");
  writeFileSync(join(repo.repo, "README.md"), "staged owner change\nunstaged owner change\n");
  writeFileSync(join(repo.repo, "private-note.txt"), "synthetic untracked owner bytes\n");
  return captureCheckout(repo);
}

test("two concurrent synthetic developers and an isolated tester preserve staged, unstaged and untracked owner bytes", async () => {
  const repo = fixture();
  const before = dirtyOwner(repo);
  const first = repo.attempt("task-a");
  const second = repo.attempt("task-b");
  const run = async (work, path, content) => {
    const worker = spawn(process.execPath, ["--input-type=module", "-e", 'import { writeFileSync } from "node:fs"; writeFileSync(process.argv[1], process.argv[2]);', path, content],
      { cwd: work.path, env: repo.env, windowsHide: true });
    const [code] = await once(worker, "exit");
    assert.equal(code, 0);
  };
  await Promise.all([run(first, "src/a.txt", "A1\n"), run(second, "src/b.txt", "B1\n")]);
  const a = repo.commit(first, "src/a.txt", "A1\n");
  const b = repo.commit(second, "src/b.txt", "B1\n");
  assert.notEqual(first.path, second.path);
  assert.equal(readFileSync(join(first.path, "src/b.txt"), "utf8"), "B0\n");
  assert.equal(readFileSync(join(second.path, "src/a.txt"), "utf8"), "A0\n");
  const tester = repo.attempt("tester-a", a);
  const result = spawnSync(check.command, check.args, { cwd: tester.path, env: repo.env, windowsHide: true });
  assert.equal(result.status, 0);
  assert(existsSync(join(tester.path, "check-output.txt")));
  assert(!existsSync(join(first.path, "check-output.txt")));
  assert.equal(repo.git(second.path, "rev-parse", "HEAD").trim(), b);
  assert.deepEqual(captureCheckout(repo), before);
});

test("Stop terminates an owned synthetic worker without deleting in-progress edits or the branch", { timeout: 5000 }, async () => {
  const repo = fixture();
  const before = dirtyOwner(repo);
  const work = repo.attempt("stopped");
  const worker = spawn(process.execPath, ["--input-type=module", "-e",
    'import { writeFileSync } from "node:fs"; writeFileSync("src/a.txt", "IN-PROGRESS\\n"); console.log("ready"); setInterval(() => {}, 1000);'],
  { cwd: work.path, env: repo.env, windowsHide: true });
  const exited = once(worker, "exit");
  try {
    await once(worker.stdout, "data");
    assert(worker.kill());
    await exited;
    assert.equal(readFileSync(join(work.path, "src/a.txt"), "utf8"), "IN-PROGRESS\n");
    assert.equal(repo.git(work.path, "rev-parse", "HEAD").trim(), repo.base);
    assert.equal(repo.git(work.path, "branch", "--show-current").trim(), work.branch);
    assert.deepEqual(captureCheckout(repo), before);
  } finally {
    if (worker.exitCode === null && worker.signalCode === null) worker.kill();
    await exited;
  }
});

test("the immutable reviewer gateway rejects mutation, process execution, extra arguments and traversal", () => {
  const repo = fixture();
  const work = repo.attempt("review-source");
  const commit = repo.commit(work, "src/a.txt", "REVIEW-ME\n");
  const before = captureCheckout(repo);
  const review = snapshotReader(repo, repo.base, commit);
  assert(review.invoke("list_files").includes("src/a.txt"));
  assert.equal(review.invoke("read_file", { path: "src/a.txt" }), "REVIEW-ME\n");
  assert(review.invoke("read_diff").includes("+REVIEW-ME"));
  for (const tool of ["write_file", "edit", "apply_patch", "Bash", "PowerShell", "exec", "git", "message_bot", "share_file", "browser_evaluate"]) {
    assert.throws(() => review.invoke(tool, { path: "src/a.txt", text: "changed" }), /Reviewer tool denied/);
  }
  for (const path of ["../README.md", "src/../../README.md", "C:\\outside.txt", "/outside.txt", ".git/config", "src//a.txt", "src/a.txt\0"]) {
    assert.throws(() => review.invoke("read_file", { path }));
  }
  assert.throws(() => review.invoke("read_file", { path: "src/a.txt", command: "write" }));
  writeFileSync(join(work.path, "src/a.txt"), "uncommitted replacement\n");
  assert.equal(review.invoke("read_file", { path: "src/a.txt" }), "REVIEW-ME\n");
  assert.deepEqual(captureCheckout(repo), before);
  assert.throws(() => reviewPolicy("claude"), /review_isolation_unavailable/);
  assert.throws(() => reviewPolicy("codex"), /review_isolation_unavailable/);
});

test("session identity separates worktrees, roles, commits, policy and accounts", () => {
  const repo = fixture();
  const work = repo.attempt("session-a");
  const other = repo.attempt("session-b");
  const context = { botId: "bot-a", provider: "claude", accountId: "account-a", projectId: "project-a",
    attemptId: "attempt-a", phase: "develop", cwd: work.path, policyHash: "role-policy-1" };
  const key = sessionKey(context);
  assert.equal(sessionKey({ ...context }), key);
  for (const patch of [{ cwd: other.path }, { phase: "review" }, { phase: "test" }, { attemptId: "attempt-b" },
    { projectId: null }, { accountId: "account-b" }, { policyHash: "role-policy-2" }, { sourceCommit: repo.base }]) {
    assert.notEqual(sessionKey({ ...context, ...patch }), key);
  }
});

test("exact reviewed commits merge with combined checks while checked-out targets remain untouched", () => {
  const repo = fixture();
  const before = dirtyOwner(repo);
  const first = repo.attempt("accepted-a");
  const second = repo.attempt("accepted-b");
  const a = repo.commit(first, "src/a.txt", "A1\n");
  const b = repo.commit(second, "src/b.txt", "B1\n");
  const candidateA = prepareIntegration(repo, accepted(repo, "a", a));
  assert.equal(candidateA.state, "ready");
  assert.equal(repo.tryGit(repo.repo, "merge-base", "--is-ancestor", a, candidateA.candidate).status, 0);
  assert.deepEqual(promoteIntegration(repo, candidateA), { state: "blocked", reason: "target_checked_out", candidate: candidateA.candidate });
  assert.deepEqual(captureCheckout(repo), before);
  repo.git(repo.repo, "branch", "delivery", repo.base);
  const deliveryA = prepareIntegration(repo, accepted(repo, "delivery-a", a, "refs/heads/delivery"));
  assert.equal(promoteIntegration(repo, deliveryA).state, "integrated");
  const deliveryB = prepareIntegration(repo, accepted(repo, "delivery-b", b, "refs/heads/delivery", deliveryA.candidate));
  assert.equal(deliveryB.state, "ready");
  assert.equal(readFileSync(join(deliveryB.path, "src/a.txt"), "utf8"), "A1\n");
  assert.equal(readFileSync(join(deliveryB.path, "src/b.txt"), "utf8"), "B1\n");
  assert.equal(promoteIntegration(repo, deliveryB).state, "integrated");
  assert.deepEqual(captureCheckout(repo), before);
});

test("stale review and tests, target movement, conflicts and failed combined checks block promotion", () => {
  const repo = fixture();
  const before = dirtyOwner(repo);
  const work = repo.attempt("changed");
  const commit = repo.commit(work, "src/a.txt", "A1\n");
  assert.throws(() => prepareIntegration(repo, { ...accepted(repo, "stale-review", commit), reviewedCommit: repo.base }), /Review is stale/);
  assert.throws(() => prepareIntegration(repo, { ...accepted(repo, "stale-test", commit), testedCommit: repo.base }), /Tests are stale/);
  repo.git(repo.repo, "branch", "moving-target", repo.base);
  const ready = prepareIntegration(repo, accepted(repo, "moving", commit, "refs/heads/moving-target"));
  repo.git(repo.repo, "update-ref", "refs/heads/moving-target", commit, repo.base);
  assert.equal(promoteIntegration(repo, ready).reason, "target_head_moved");
  assert.equal(prepareIntegration(repo, accepted(repo, "already-moved", commit, "refs/heads/moving-target")).reason, "target_head_moved");
  const conflicting = repo.attempt("conflict");
  const conflictCommit = repo.commit(conflicting, "src/a.txt", "CONFLICT\n");
  repo.git(repo.repo, "branch", "conflicting-target", conflictCommit);
  const conflict = prepareIntegration(repo, accepted(repo, "conflict", commit, "refs/heads/conflicting-target", conflictCommit));
  assert.equal(conflict.reason, "conflict");
  assert(repo.git(conflict.path, "status", "--porcelain").includes("UU src/a.txt"));
  const failed = repo.attempt("failed");
  const badCommit = repo.commit(failed, "src/b.txt", "FAIL\n");
  const failure = prepareIntegration(repo, accepted(repo, "failed", badCommit));
  assert.equal(failure.reason, "combined_checks_failed");
  assert.equal(failure.exitCode, 1);
  writeFileSync(join(work.path, "src/a.txt"), "uncommitted edit\n");
  assert.equal(prepareIntegration(repo, accepted(repo, "dirty", commit)).reason, "attempt_dirty");
  repo.commit(work, "src/a.txt", "A2\n");
  assert.equal(prepareIntegration(repo, accepted(repo, "head-moved", commit)).reason, "attempt_head_moved");
  assert.deepEqual(captureCheckout(repo), before);
});

test("persistent attempt recovery preserves completed work and invalidates late review/test acceptance", () => {
  const repo = fixture();
  const work = repo.attempt("persisted");
  const missing = repo.attempt("missing");
  const file = join(repo.root, "lifecycle.sqlite");
  let db = new DatabaseSync(file);
  db.exec("CREATE TABLE attempts (id TEXT PRIMARY KEY, state TEXT NOT NULL, phase TEXT NOT NULL, worktree TEXT NOT NULL, head TEXT NOT NULL, revision INTEGER NOT NULL, reviewed_sha TEXT, tested_sha TEXT, reason TEXT)");
  const insert = db.prepare("INSERT INTO attempts VALUES (?, ?, 'develop', ?, ?, 1, ?, ?, NULL)");
  insert.run("running", "running", work.path, repo.base, null, null);
  insert.run("done", "completed", work.path, repo.base, repo.base, repo.base);
  insert.run("missing", "queued", missing.path, repo.base, null, null);
  insert.run("cancelled", "cancelled", work.path, repo.base, null, null);
  db.close();
  // Remove only the owned synthetic directory; Git's stale worktree registration is retained for recovery.
  assert(missing.path.startsWith(`${repo.root}\\`) || missing.path.startsWith(`${repo.root}/`));
  rmSync(missing.path, { recursive: true });
  db = new DatabaseSync(file);
  for (const row of db.prepare("SELECT id, state, worktree FROM attempts WHERE state IN ('queued', 'running')").all()) {
    if (!existsSync(row.worktree)) db.prepare("UPDATE attempts SET state='blocked', reason='worktree_missing' WHERE id=?").run(row.id);
    else if (row.state === "running") db.prepare("UPDATE attempts SET state='interrupted', reason='daemon_restart' WHERE id=?").run(row.id);
  }
  assert.equal(db.prepare("SELECT state FROM attempts WHERE id='done'").get().state, "completed");
  assert.equal(db.prepare("SELECT state FROM attempts WHERE id='cancelled'").get().state, "cancelled");
  assert.equal(db.prepare("SELECT state FROM attempts WHERE id='running'").get().state, "interrupted");
  assert.equal(db.prepare("SELECT reason FROM attempts WHERE id='missing'").get().reason, "worktree_missing");
  const old = db.prepare("SELECT head, revision FROM attempts WHERE id='running'").get();
  const changed = repo.commit(work, "src/a.txt", "A2\n");
  db.prepare("UPDATE attempts SET head=?, revision=revision+1, reviewed_sha=NULL, tested_sha=NULL WHERE id='running'").run(changed);
  const late = db.prepare("UPDATE attempts SET reviewed_sha=?, tested_sha=? WHERE id='running' AND head=? AND revision=?")
    .run(old.head, old.head, old.head, old.revision);
  assert.equal(Number(late.changes), 0);
  assert.equal(db.prepare("SELECT reviewed_sha FROM attempts WHERE id='running'").get().reviewed_sha, null);
  db.close();
});
