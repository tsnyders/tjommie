import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

const spikeRoot = realpathSync.native(import.meta.dirname);
const shaPattern = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/;
const digest = (value) => createHash("sha256").update(value).digest("hex");

function inside(root, path) {
  const rel = relative(root, path);
  return rel === "" || (!isAbsolute(rel) && rel !== ".." && !rel.startsWith(`..${sep}`));
}

export function createFixture() {
  const root = mkdtempSync(join(spikeRoot, ".fixture-"));
  const repo = join(root, "owner");
  const hooks = join(root, "empty-hooks");
  mkdirSync(repo);
  mkdirSync(hooks);
  writeFileSync(join(root, "fixture-only"), "tjommie-phase4-teams-p0\n");
  const env = {};
  for (const key of ["PATH", "SystemRoot", "SystemDrive", "WINDIR", "PATHEXT", "TEMP", "TMP", "COMSPEC"]) {
    if (process.env[key] !== undefined) env[key] = process.env[key];
  }
  Object.assign(env, { GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(root, "empty-gitconfig"),
    GIT_TERMINAL_PROMPT: "0", GIT_OPTIONAL_LOCKS: "0", GIT_AUTHOR_NAME: "Synthetic P0", GIT_AUTHOR_EMAIL: "p0@example.invalid",
    GIT_COMMITTER_NAME: "Synthetic P0", GIT_COMMITTER_EMAIL: "p0@example.invalid" });
  writeFileSync(env.GIT_CONFIG_GLOBAL, "");
  const options = (cwd) => {
    assert(inside(root, realpathSync.native(cwd)), "Git may operate only inside the new synthetic fixture");
    return { cwd, env, encoding: "utf8", windowsHide: true, maxBuffer: 4 * 1024 * 1024 };
  };
  const argsWithPolicy = (args) => ["-c", `core.hooksPath=${hooks}`, "-c", "core.fsmonitor=false", "-c", "commit.gpgSign=false", ...args];
  const git = (cwd, ...args) => execFileSync("git", argsWithPolicy(args), options(cwd));
  const tryGit = (cwd, ...args) => spawnSync("git", argsWithPolicy(args), options(cwd));
  git(repo, "init", "-b", "main");
  mkdirSync(join(repo, "src"));
  writeFileSync(join(repo, "README.md"), "Synthetic P0 repository\n");
  writeFileSync(join(repo, "src", "a.txt"), "A0\n");
  writeFileSync(join(repo, "src", "b.txt"), "B0\n");
  writeFileSync(join(repo, "check.mjs"), 'import { readFileSync, writeFileSync } from "node:fs";\nwriteFileSync("check-output.txt", "synthetic check ran\\n");\nif (["src/a.txt", "src/b.txt"].some((path) => readFileSync(path, "utf8").includes("FAIL"))) process.exitCode = 1;\n');
  git(repo, "add", "--", "README.md", "src/a.txt", "src/b.txt", "check.mjs");
  git(repo, "commit", "-m", "Synthetic base");
  const base = git(repo, "rev-parse", "HEAD").trim();
  const fixture = {
    root, repo, base, git, tryGit, env, commits: new Map(), promotions: new Map(),
    attempt(id, commit = base) {
      assert.match(id, /^[a-z0-9-]{1,40}$/);
      assert.match(commit, shaPattern);
      const path = join(root, `work-${id}`);
      const branch = `tjommie/p0/${id}`;
      git(repo, "worktree", "add", "-b", branch, path, commit);
      return { path, branch, base: commit };
    },
    commit(worktree, path, bytes) {
      assert(!isAbsolute(path) && inside(worktree.path, resolve(worktree.path, path)));
      writeFileSync(join(worktree.path, path), bytes);
      git(worktree.path, "add", "--", path);
      git(worktree.path, "commit", "-m", "Synthetic task change");
      const sha = git(worktree.path, "rev-parse", "HEAD").trim();
      fixture.commits.set(sha, worktree.path);
      return sha;
    },
    dispose() {
      const target = realpathSync.native(root);
      assert(inside(spikeRoot, target) && dirname(target) === spikeRoot && target.startsWith(join(spikeRoot, ".fixture-")));
      assert.equal(readFileSync(join(target, "fixture-only"), "utf8"), "tjommie-phase4-teams-p0\n");
      rmSync(target, { recursive: true, force: true });
    },
  };
  return fixture;
}

export function captureCheckout(fixture) {
  const files = {};
  function visit(path, prefix = "") {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      if (entry.name === ".git") continue;
      const name = prefix + entry.name;
      if (entry.isDirectory()) visit(join(path, entry.name), `${name}/`);
      else files[name] = digest(readFileSync(join(path, entry.name)));
    }
  }
  visit(fixture.repo);
  return { files, index: digest(readFileSync(join(fixture.repo, ".git", "index"))),
    status: fixture.git(fixture.repo, "status", "--porcelain=v1", "-z"),
    staged: fixture.git(fixture.repo, "diff", "--cached", "--no-ext-diff", "--no-textconv", "--binary"),
    unstaged: fixture.git(fixture.repo, "diff", "--no-ext-diff", "--no-textconv", "--binary"),
    head: fixture.git(fixture.repo, "rev-parse", "HEAD").trim() };
}

export function snapshotReader(fixture, base, commit) {
  assert.match(base, shaPattern);
  assert.match(commit, shaPattern);
  assert.equal(fixture.git(fixture.repo, "rev-parse", "--verify", `${commit}^{commit}`).trim(), commit);
  const files = new Map();
  let totalBytes = 0;
  for (const entry of fixture.git(fixture.repo, "ls-tree", "-r", "-z", commit).split("\0").filter(Boolean)) {
    const tab = entry.indexOf("\t");
    const [mode, type, oid] = entry.slice(0, tab).split(" ");
    const path = entry.slice(tab + 1);
    // shortcut: symlinks and submodules are excluded, add explicit object-only readers if projects require them.
    if (type !== "blob" || !["100644", "100755"].includes(mode)) continue;
    const size = Number(fixture.git(fixture.repo, "cat-file", "-s", oid));
    totalBytes += size;
    assert(Number.isSafeInteger(size) && size <= 256 * 1024 && totalBytes <= 2 * 1024 * 1024, "Snapshot exceeds P0 review limits");
    const content = fixture.git(fixture.repo, "cat-file", "blob", oid);
    if (!content.includes("\0")) files.set(path, content);
  }
  const diff = fixture.git(fixture.repo, "diff", "--no-ext-diff", "--no-textconv", "--unified=3", base, commit, "--");
  assert(Buffer.byteLength(diff) <= 512 * 1024, "Diff exceeds P0 review limits");
  return Object.freeze({
    commit,
    invoke(name, params = {}) {
      assert.equal(typeof params, "object");
      assert(params !== null && !Array.isArray(params));
      if (name === "list_files") {
        assert.equal(Object.keys(params).length, 0);
        return [...files.keys()];
      }
      if (name === "read_diff") {
        assert.equal(Object.keys(params).length, 0);
        return diff;
      }
      if (name === "read_file") {
        assert.deepEqual(Object.keys(params), ["path"]);
        const { path } = params;
        assert(typeof path === "string" && !path.includes("\\") && !path.includes("\0") && !isAbsolute(path)
          && !path.split("/").some((part) => ["", ".", ".."].includes(part)), "Invalid snapshot path");
        assert(files.has(path), "File is not in the immutable review snapshot");
        return files.get(path);
      }
      throw new Error("Reviewer tool denied");
    },
  });
}

export function reviewPolicy() {
  // CLI help establishes candidate flags, not effective enforcement by a running provider.
  throw new Error("review_isolation_unavailable");
}

export function sessionKey(context) {
  return digest(JSON.stringify([context.botId, context.provider, context.accountId, context.projectId ?? null,
    context.attemptId ?? null, context.phase, realpathSync.native(context.cwd), context.policyHash, context.sourceCommit ?? null]));
}

export function prepareIntegration(fixture, { id, targetRef, expectedTarget, acceptedCommit, reviewedCommit, testedCommit, attemptPath, check }) {
  assert.match(id, /^[a-z0-9-]{1,40}$/);
  assert.match(targetRef, /^refs\/heads\/[a-zA-Z0-9][a-zA-Z0-9/._-]*$/);
  fixture.git(fixture.repo, "check-ref-format", targetRef);
  for (const sha of [expectedTarget, acceptedCommit, reviewedCommit, testedCommit]) assert.match(sha, shaPattern);
  assert.equal(acceptedCommit, reviewedCommit, "Review is stale");
  assert.equal(acceptedCommit, testedCommit, "Tests are stale");
  if (fixture.git(attemptPath, "rev-parse", "HEAD").trim() !== acceptedCommit) return { state: "blocked", reason: "attempt_head_moved" };
  if (fixture.git(attemptPath, "status", "--porcelain=v1", "-z")) return { state: "blocked", reason: "attempt_dirty" };
  const currentTarget = fixture.git(fixture.repo, "rev-parse", "--verify", targetRef).trim();
  if (currentTarget !== expectedTarget) return { state: "blocked", reason: "target_head_moved" };
  const work = fixture.attempt(`integration-${id}`, expectedTarget);
  const merge = fixture.tryGit(work.path, "merge", "--no-ff", "--no-edit", acceptedCommit);
  if (merge.status !== 0) return { state: "blocked", reason: "conflict", path: work.path, branch: work.branch };
  const result = spawnSync(check.command, check.args, { cwd: work.path, env: fixture.env, windowsHide: true, encoding: "utf8" });
  const candidate = fixture.git(work.path, "rev-parse", "HEAD").trim();
  if (result.status !== 0) return { state: "blocked", reason: "combined_checks_failed", candidate, path: work.path, branch: work.branch, exitCode: result.status };
  return { state: "ready", candidate, acceptedCommit, expectedTarget, targetRef, path: work.path, branch: work.branch, exitCode: 0 };
}

export function promoteIntegration(fixture, candidate) {
  assert.equal(candidate.state, "ready");
  const current = fixture.git(fixture.repo, "rev-parse", "--verify", candidate.targetRef).trim();
  if (current !== candidate.expectedTarget) return { state: "blocked", reason: "target_head_moved" };
  const records = fixture.git(fixture.repo, "worktree", "list", "--porcelain", "-z").split("\0\0");
  const promotion = fixture.promotions.get(candidate.targetRef);
  for (const record of records) {
    const fields = record.split("\0");
    if (fields.includes(`branch ${candidate.targetRef}`) && !fields.includes(`worktree ${promotion?.replaceAll("\\", "/")}`)) {
      return { state: "blocked", reason: "target_checked_out", candidate: candidate.candidate };
    }
  }
  assert.equal(fixture.tryGit(fixture.repo, "merge-base", "--is-ancestor", candidate.expectedTarget, candidate.candidate).status, 0);
  let path = promotion;
  if (!path) {
    path = join(fixture.root, `promotion-${digest(candidate.targetRef).slice(0, 12)}`);
    const claim = fixture.tryGit(fixture.repo, "worktree", "add", path, candidate.targetRef.slice("refs/heads/".length));
    if (claim.status !== 0) return { state: "blocked", reason: "target_checkout_race" };
    fixture.promotions.set(candidate.targetRef, path);
  }
  if (fixture.git(path, "rev-parse", "HEAD").trim() !== candidate.expectedTarget) return { state: "blocked", reason: "target_head_moved" };
  if (fixture.git(path, "status", "--porcelain=v1", "-z")) return { state: "blocked", reason: "promotion_worktree_dirty" };
  const promote = fixture.tryGit(path, "merge", "--ff-only", candidate.candidate);
  if (promote.status !== 0) return { state: "blocked", reason: "promotion_failed" };
  assert.equal(fixture.git(path, "rev-parse", "HEAD").trim(), candidate.candidate);
  return { state: "integrated", commit: candidate.candidate };
}
