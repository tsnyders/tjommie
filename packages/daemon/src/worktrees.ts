import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, rmSync, rmdirSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { CommitSha } from "../../protocol/src/projects.ts";
import { RpcError } from "./server.ts";

export type Repository = { id: string; path: string; commonDir: string; name: string; createdAt: string };
export type ManagedWorktree = { id: string; path: string; branch: string; baseSha: string };
type TreeEntry = { path: string; oid: string; mode: string; size: number };
const MAX_FILE = 64 * 1024 * 1024;
const MAX_TREE = 1024 * 1024 * 1024;
const equalPath = (a: string, b: string): boolean => process.platform === "win32" ? a.toLowerCase() === b.toLowerCase() : a === b;
const within = (root: string, path: string): boolean => { const rel = relative(root, path); return rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel); };
const reason = (message: string): RpcError => new RpcError("bad_state", message);

export class Worktrees {
  readonly root: string;
  readonly #hooks: string;
  constructor(root: string) {
    mkdirSync(root, { recursive: true });
    this.root = realpathSync.native(root);
    this.#hooks = join(this.root, ".empty-hooks");
    mkdirSync(this.#hooks, { recursive: true });
    mkdirSync(join(this.root, ".owners"), { recursive: true });
  }

  #env(): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = {};
    for (const key of ["PATH", "SystemRoot", "SystemDrive", "WINDIR", "PATHEXT", "TEMP", "TMP", "COMSPEC", "HOME", "USERPROFILE", "XDG_CONFIG_HOME"]) {
      if (process.env[key] !== undefined) env[key] = process.env[key];
    }
    return env;
  }

  #args(args: string[]): string[] {
    // Git hooks and fsmonitor are code execution; accepted project checks run separately.
    return ["--no-replace-objects", "-c", `core.hooksPath=${this.#hooks}`, "-c", "core.fsmonitor=false", ...args];
  }

  git(path: string, args: string[], input?: string): Buffer {
    try {
      return execFileSync("git", this.#args(args), { cwd: path, env: this.#env(), windowsHide: true, input,
        maxBuffer: Math.max(MAX_FILE + 65_536, 4 * 1024 * 1024), stdio: ["pipe", "pipe", "pipe"] });
    } catch { throw reason("Git could not complete this operation. The repository and managed worktree were retained."); }
  }

  #text(path: string, args: string[], input?: string): string { return this.git(path, args, input).toString("utf8").trim(); }

  repository(path: string): Omit<Repository, "id" | "createdAt"> {
    let canonical: string;
    try { canonical = realpathSync.native(path); } catch { throw new RpcError("bad_params", "Choose an existing Git repository on this host."); }
    const top = realpathSync.native(this.#text(canonical, ["rev-parse", "--show-toplevel"]));
    if (!equalPath(top, canonical)) throw new RpcError("bad_params", "Choose the repository's root folder.");
    const commonDir = realpathSync.native(resolve(canonical, this.#text(canonical, ["rev-parse", "--git-common-dir"])));
    if (existsSync(join(commonDir, "info", "grafts"))) throw reason("Legacy Git grafts require reconciliation before exact-commit task work.");
    return { path: canonical, commonDir, name: basename(canonical) };
  }

  assertRepository(repo: Repository): void {
    if (existsSync(join(repo.commonDir, "info", "grafts"))) throw reason("Legacy Git grafts require reconciliation before exact-commit task work.");
    if (!equalPath(realpathSync.native(repo.path), repo.path)
      || !equalPath(realpathSync.native(resolve(repo.path, this.#text(repo.path, ["rev-parse", "--git-common-dir"]))), repo.commonDir)) throw reason("repository_changed");
  }

  validateRef(repo: Repository, ref: string): void {
    if (!/^refs\/heads\/[A-Za-z0-9][A-Za-z0-9/._-]*$/.test(ref)) throw new RpcError("bad_params", "Choose a full local branch ref.");
    this.git(repo.path, ["check-ref-format", ref]);
  }

  resolveRef(repo: Repository, ref: string): string {
    this.assertRepository(repo); this.validateRef(repo, ref);
    return CommitSha.parse(this.#text(repo.path, ["rev-parse", "--verify", `${ref}^{commit}`]));
  }

  describe(id: string, baseSha: string, branch = `refs/heads/tjommie/${id}`): ManagedWorktree {
    if (!/^[A-Za-z0-9_-]{1,100}$/.test(id)) throw new RpcError("bad_params", "Invalid managed worktree identity.");
    CommitSha.parse(baseSha);
    return { id, path: join(this.root, id), branch, baseSha };
  }

  #entries(repo: Repository, sha: string): TreeEntry[] {
    CommitSha.parse(sha);
    const entries: TreeEntry[] = [];
    const paths = new Set<string>();
    let total = 0;
    for (const record of this.git(repo.path, ["ls-tree", "-r", "-l", "-z", sha]).toString("utf8").split("\0").filter(Boolean)) {
      const tab = record.indexOf("\t");
      const [mode, kind, oid, rawSize] = record.slice(0, tab).trim().split(/ +/);
      const path = record.slice(tab + 1);
      const key = process.platform === "win32" ? path.toLowerCase() : path;
      if (kind !== "blob" || !["100644", "100755"].includes(mode)) throw reason("unsupported_tree_entry: symlinks and submodules require a supported isolated checkout.");
      if (!path || path.includes("\\") || path.includes("\uFFFD") || path.split("/").some((part) => ["", ".", "..", ".git"].includes(part.toLowerCase())
        || /[<>:"|?*\x00-\x1f]/.test(part) || /[. ]$/.test(part) || /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$/i.test(part)) || paths.has(key)) throw reason("unsupported_tree_path");
      paths.add(key);
      const size = Number(rawSize);
      total += size;
      if (!Number.isSafeInteger(size) || size > MAX_FILE || total > MAX_TREE || entries.length >= 100_000) throw reason("worktree_size_limit");
      entries.push({ path, oid, mode, size });
    }
    return entries;
  }

  #materialize(repo: Repository, work: ManagedWorktree, sha: string, entries: TreeEntry[]): void {
    const next = new Set(entries.map((entry) => entry.path));
    for (const old of this.git(work.path, ["ls-files", "-z"]).toString("utf8").split("\0").filter(Boolean)) {
      if (!next.has(old)) {
        const path = this.#safePath(work.path, old);
        if (existsSync(path)) rmSync(path);
        for (let parent = dirname(path); !equalPath(parent, work.path); parent = dirname(parent)) {
          try { rmdirSync(parent); } catch (error) {
            if (error instanceof Error && "code" in error && ["ENOTEMPTY", "EEXIST"].includes(String(error.code))) break;
            if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error;
          }
        }
      }
    }
    this.git(work.path, ["read-tree", sha]);
    for (let offset = 0; offset < entries.length;) {
      const batch: TreeEntry[] = []; let bytes = 0;
      do { const entry = entries[offset++]; batch.push(entry); bytes += entry.size; } while (offset < entries.length && bytes + entries[offset].size < 16 * 1024 * 1024);
      const objects = this.git(repo.path, ["cat-file", "--batch"], batch.map((entry) => entry.oid).join("\n") + "\n");
      let cursor = 0;
      for (const entry of batch) {
        const newline = objects.indexOf(10, cursor), header = objects.subarray(cursor, newline).toString("utf8");
        if (newline < 0 || header !== `${entry.oid} blob ${entry.size}`) throw reason("Git object changed during checkout.");
        const path = this.#safePath(work.path, entry.path); cursor = newline + 1;
        if (cursor + entry.size >= objects.length || objects[cursor + entry.size] !== 10) throw reason("Incomplete Git object.");
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, objects.subarray(cursor, cursor + entry.size), { mode: entry.mode === "100755" ? 0o755 : 0o644 }); cursor += entry.size + 1;
      }
    }
  }

  #safePath(root: string, name: string): string {
    const path = resolve(root, name);
    if (!within(root, path) || equalPath(root, path)) throw reason("unsupported_tree_path");
    for (let parent = path; !equalPath(parent, root); parent = dirname(parent)) {
      try { if (lstatSync(parent).isSymbolicLink()) throw reason("worktree_link_changed"); }
      catch (error) { if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error; }
    }
    return path;
  }

  create(repo: Repository, work: ManagedWorktree, existingBranch = false): void {
    this.assertRepository(repo); this.validateRef(repo, work.branch);
    const expected = this.describe(work.id, work.baseSha, work.branch);
    if (!equalPath(expected.path, work.path) || existsSync(work.path)) throw reason("worktree_path_occupied");
    const entries = this.#entries(repo, work.baseSha);
    this.git(repo.path, ["worktree", "add", "--no-checkout", "--lock", "--reason", "Tjommie managed workspace",
      ...(existingBranch ? [work.path, work.branch.slice("refs/heads/".length)] : ["-b", work.branch.slice("refs/heads/".length), work.path, work.baseSha])]);
    this.#materialize(repo, work, work.baseSha, entries);
    writeFileSync(join(this.root, ".owners", `${work.id}.json`), JSON.stringify({ ...work, commonDir: repo.commonDir }), { flag: "wx" });
  }

  assertOwned(repo: Repository, work: ManagedWorktree): void {
    this.assertRepository(repo);
    const expected = this.describe(work.id, work.baseSha, work.branch);
    if (!equalPath(expected.path, work.path) || lstatSync(work.path).isSymbolicLink() || !equalPath(realpathSync.native(work.path), work.path)) throw reason("worktree_ownership_changed");
    let owner: unknown;
    try { owner = JSON.parse(readFileSync(join(this.root, ".owners", `${work.id}.json`), "utf8")); } catch { throw reason("worktree_ownership_missing"); }
    if (!owner || typeof owner !== "object" || !("commonDir" in owner) || owner.commonDir !== repo.commonDir || !("branch" in owner) || owner.branch !== work.branch) throw reason("worktree_ownership_changed");
    const [rawCommon, branch] = this.#text(work.path, ["rev-parse", "--path-format=absolute", "--git-common-dir", "--symbolic-full-name", "HEAD"]).split(/\r?\n/);
    const common = realpathSync.native(rawCommon);
    if (!equalPath(common, repo.commonDir) || branch !== work.branch) throw reason("worktree_ownership_changed");
  }

  head(repo: Repository, work: ManagedWorktree): string { this.assertOwned(repo, work); return CommitSha.parse(this.#text(work.path, ["rev-parse", "HEAD"])); }

  dirty(repo: Repository, work: ManagedWorktree, sha = this.head(repo, work), allowUntracked = false): boolean {
    this.assertOwned(repo, work);
    if (this.#text(work.path, ["rev-parse", "HEAD"]) !== sha || this.#text(work.path, ["write-tree"]) !== this.#text(repo.path, ["rev-parse", `${sha}^{tree}`])) return true;
    if (!allowUntracked && this.git(work.path, ["ls-files", "--others", "--exclude-standard", "-z"]).length) return true;
    const hash = sha.length === 64 ? "sha256" : "sha1";
    for (const entry of this.#entries(repo, sha)) {
      const path = this.#safePath(work.path, entry.path);
      try {
        if (!lstatSync(path).isFile() || lstatSync(path).isSymbolicLink()) return true;
        const bytes = readFileSync(path);
        if (createHash(hash).update(`blob ${bytes.length}\0`).update(bytes).digest("hex") !== entry.oid) return true;
      } catch { return true; }
    }
    return false;
  }

  diff(repo: Repository, base: string, sha: string): { text: string; truncated: boolean } {
    CommitSha.parse(base); CommitSha.parse(sha);
    const bytes = this.git(repo.path, ["diff", "--no-ext-diff", "--no-textconv", "--unified=3", base, sha, "--"]);
    return { text: bytes.subarray(0, 512 * 1024).toString("utf8"), truncated: bytes.length > 512 * 1024 };
  }

  changedFiles(repo: Repository, work: ManagedWorktree, sha: string): string[] {
    this.assertOwned(repo, work); CommitSha.parse(sha);
    const changed = new Set(this.git(work.path, ["diff", "--cached", "--name-only", "--no-ext-diff", "--no-textconv", "-z", sha, "--"]).toString("utf8").split("\0").filter(Boolean));
    for (const path of this.git(work.path, ["ls-files", "--others", "-z"]).toString("utf8").split("\0").filter(Boolean)) changed.add(path);
    const hash = sha.length === 64 ? "sha256" : "sha1";
    for (const entry of this.#entries(repo, sha)) {
      try {
        const bytes = readFileSync(this.#safePath(work.path, entry.path));
        if (createHash(hash).update(`blob ${bytes.length}\0`).update(bytes).digest("hex") !== entry.oid) changed.add(entry.path);
      } catch { changed.add(entry.path); }
    }
    return [...changed];
  }

  mergeCandidate(repo: Repository, work: ManagedWorktree, target: string, accepted: string): { sha: string | null; conflict: boolean } {
    CommitSha.parse(target); CommitSha.parse(accepted);
    const merge = spawnSync("git", this.#args(["merge-tree", "--write-tree", "--name-only", target, accepted]),
      { cwd: repo.path, env: this.#env(), windowsHide: true, encoding: "utf8", maxBuffer: 4 * 1024 * 1024 });
    if (merge.error || merge.status === null || merge.status > 1) throw reason("merge_unavailable");
    const tree = CommitSha.parse(merge.stdout.split(/\r?\n/)[0]);
    this.create(repo, work);
    if (merge.status === 1) {
      this.#materialize(repo, work, tree, this.#entries(repo, tree));
      return { sha: null, conflict: true };
    }
    const sha = CommitSha.parse(this.#text(repo.path, ["commit-tree", tree, "-p", target, "-p", accepted], `Integrate accepted Tjommie task ${work.id}\n`));
    this.#materialize(repo, work, sha, this.#entries(repo, sha));
    this.git(work.path, ["update-ref", "HEAD", sha, target]);
    return { sha, conflict: false };
  }

  promote(repo: Repository, targetRef: string, expected: string, candidate: string): { path: string | null; reason: string | null } {
    if (this.resolveRef(repo, targetRef) !== expected) return { path: null, reason: "target_head_moved" };
    CommitSha.parse(candidate);
    const id = `target-${createHash("sha256").update(repo.commonDir + targetRef).digest("hex").slice(0, 32)}`;
    const work = this.describe(id, expected, targetRef);
    const records = this.git(repo.path, ["worktree", "list", "--porcelain", "-z"]).toString("utf8").split("\0\0");
    for (const record of records) {
      const fields = record.split("\0");
      if (fields.includes(`branch ${targetRef}`) && !fields.some((field) => field.startsWith("worktree ") && equalPath(field.slice(9).replaceAll("/", sep), work.path))) {
        return { path: null, reason: "target_checked_out" };
      }
    }
    if (!existsSync(work.path)) this.create(repo, work, true);
    this.assertOwned(repo, work);
    if (this.head(repo, work) !== expected) return { path: work.path, reason: "target_head_moved" };
    if (this.dirty(repo, work, expected) || this.changedFiles(repo, work, expected).length) return { path: work.path, reason: "promotion_worktree_dirty" };
    if (spawnSync("git", this.#args(["merge-base", "--is-ancestor", expected, candidate]), { cwd: repo.path, env: this.#env(), windowsHide: true }).status !== 0) return { path: work.path, reason: "target_not_ancestor" };
    // Only the owned target index is materialized; the user's checked-out branch is never advanced.
    this.#materialize(repo, work, candidate, this.#entries(repo, candidate));
    this.git(work.path, ["update-ref", "HEAD", candidate, expected]);
    return { path: work.path, reason: null };
  }

  remove(repo: Repository, work: ManagedWorktree, expectedSha: string): void {
    this.assertOwned(repo, work);
    if (this.head(repo, work) !== expectedSha || this.changedFiles(repo, work, expectedSha).length) throw reason("The worktree has changed. Preserve its files before cleanup.");
    const paths = this.git(work.path, ["ls-files", "-z"]).toString("utf8");
    const attributes = this.git(work.path, ["check-attr", "--cached", "-z", "--stdin", "filter"], paths).toString("utf8").split("\0");
    for (let index = 2; index < attributes.length; index += 3) if (attributes[index] && !["unspecified", "unset"].includes(attributes[index])) throw reason("Automatic cleanup of a filtered checkout is unavailable. Its files and branch are retained.");
    this.git(repo.path, ["worktree", "unlock", work.path]);
    this.git(repo.path, ["worktree", "remove", work.path]);
    const marker = join(this.root, ".owners", `${work.id}.json`);
    if (within(this.root, marker)) rmSync(marker);
  }
}
