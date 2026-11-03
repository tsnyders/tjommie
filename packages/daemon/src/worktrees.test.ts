import { createHash } from "node:crypto";
import { join } from "node:path";
import { existsSync, readFileSync, rmSync, rmdirSync, writeFileSync } from "node:fs";
import { afterEach, expect, it, vi } from "vitest";
import { projectFixture } from "./project-test-support.ts";
import { reviewSnapshot } from "./project-review.ts";

const open: ReturnType<typeof projectFixture>[] = [];
vi.setConfig({ testTimeout: 30_000 });
afterEach(() => { for (const h of open.splice(0)) h.db.close(); });
const fixture = () => { const h = projectFixture(); open.push(h); return h; };

it("materializes independent exact Git objects without running repository checkout hooks or filters", () => {
  const h = fixture();
  writeFileSync(join(h.repoPath, ".git/hooks/post-checkout"), "#!/bin/sh\ntouch ../hook-ran\n");
  h.git(h.repoPath, "config", "filter.untrusted.smudge", "touch ../filter-ran");
  writeFileSync(join(h.repoPath, ".gitattributes"), "src/* filter=untrusted\n"); h.git(h.repoPath, "add", ".gitattributes"); h.git(h.repoPath, "commit", "-m", "Synthetic attributes");
  const sha = h.git(h.repoPath, "rev-parse", "HEAD"), a = h.worktrees.describe("a", sha), b = h.worktrees.describe("b", sha);
  const before = h.owner(); h.worktrees.create(h.repository, a); h.worktrees.create(h.repository, b);
  expect(readFileSync(join(a.path, "src/a.txt"), "utf8")).toBe("A0\n");
  expect(existsSync(join(h.root, "hook-ran"))).toBe(false); expect(existsSync(join(h.root, "filter-ran"))).toBe(false);
  expect(h.worktrees.dirty(h.repository, a)).toBe(false);
  writeFileSync(join(a.path, "src/a.txt"), "working A\n");
  expect(h.worktrees.dirty(h.repository, a)).toBe(true);
  expect(readFileSync(join(b.path, "src/a.txt"), "utf8")).toBe("A0\n"); expect(h.owner()).toEqual(before);
});

it("merges an exact accepted commit in an owned candidate and blocks a user's checked-out target", () => {
  const h = fixture(), task = h.worktrees.describe("task", h.base); h.worktrees.create(h.repository, task);
  writeFileSync(join(task.path, "src/a.txt"), "A1\n"); h.git(task.path, "add", "src/a.txt"); h.git(task.path, "commit", "-m", "Accepted task");
  const accepted = h.worktrees.head(h.repository, task);
  writeFileSync(join(h.repoPath, "src/b.txt"), "owner staged\n"); h.git(h.repoPath, "add", "src/b.txt"); writeFileSync(join(h.repoPath, "src/b.txt"), "owner unstaged\n"); writeFileSync(join(h.repoPath, "owner-note"), "owner untracked\n");
  const before = h.owner(), bytes = createHash("sha256").update(readFileSync(join(h.repoPath, "src/b.txt"))).digest("hex");
  const candidate = h.worktrees.describe("candidate", h.base), merged = h.worktrees.mergeCandidate(h.repository, candidate, h.base, accepted);
  expect(merged.conflict).toBe(false); expect(merged.sha).toBeTruthy(); expect(h.worktrees.dirty(h.repository, candidate)).toBe(false);
  expect(h.worktrees.promote(h.repository, "refs/heads/main", h.base, merged.sha!).reason).toBe("target_checked_out");
  h.git(h.repoPath, "branch", "delivery", h.base);
  expect(h.worktrees.promote(h.repository, "refs/heads/delivery", h.base, merged.sha!).reason).toBeNull();
  expect(h.git(h.repoPath, "rev-parse", "delivery")).toBe(merged.sha);
  expect(h.owner()).toEqual(before); expect(createHash("sha256").update(readFileSync(join(h.repoPath, "src/b.txt"))).digest("hex")).toBe(bytes);
});

it("preserves conflict-marker source and refuses cleanup of dirty or foreign worktrees", () => {
  const h = fixture(), a = h.worktrees.describe("left", h.base), b = h.worktrees.describe("right", h.base);
  for (const [work, text] of [[a, "LEFT\n"], [b, "RIGHT\n"]] as const) { h.worktrees.create(h.repository, work); writeFileSync(join(work.path, "src/a.txt"), text); h.git(work.path, "add", "src/a.txt"); h.git(work.path, "commit", "-m", "Synthetic conflict"); }
  const target = h.worktrees.head(h.repository, a), accepted = h.worktrees.head(h.repository, b), work = h.worktrees.describe("conflicted", target);
  expect(h.worktrees.mergeCandidate(h.repository, work, target, accepted).conflict).toBe(true);
  expect(readFileSync(join(work.path, "src/a.txt"), "utf8")).toContain("<<<<<<<");
  expect(() => h.worktrees.remove(h.repository, work, target)).toThrow(/Preserve/);
  expect(() => h.worktrees.assertOwned(h.repository, { ...work, path: h.repoPath })).toThrow();
  expect(h.git(h.repoPath, "rev-parse", "HEAD")).toBe(h.base);
});

it("offers immutable source-only review and fails on incomplete binary coverage", () => {
  const h = fixture(), snapshot = reviewSnapshot(h.worktrees, h.repository, h.base, h.base);
  expect(snapshot.read("src/a.txt")).toBe("A0\n"); expect(Object.isFrozen(snapshot)).toBe(true);
  expect(() => snapshot.read("../owner-note")).toThrow(/outside/);
  writeFileSync(join(h.repoPath, "src/a.txt"), "new uncommitted bytes\n"); expect(snapshot.read("src/a.txt")).toBe("A0\n");
  writeFileSync(join(h.repoPath, "binary.bin"), Buffer.from([0, 1, 2])); h.git(h.repoPath, "add", "binary.bin"); h.git(h.repoPath, "commit", "-m", "Synthetic binary");
  expect(() => reviewSnapshot(h.worktrees, h.repository, h.base, h.git(h.repoPath, "rev-parse", "HEAD"))).toThrow(/binary/);
});

it("rejects hostile Git tree names before creating a worktree", () => {
  const h = fixture(), oid = h.worktrees.git(h.repoPath, ["hash-object", "-w", "--stdin"], "Synthetic object\n").toString("utf8").trim();
  for (const [index, name] of [".GIT", "NUL.txt"].entries()) {
    const tree = h.worktrees.git(h.repoPath, ["mktree"], `100644 blob ${oid}\t${name}\n`).toString("utf8").trim();
    const commit = h.worktrees.git(h.repoPath, ["commit-tree", tree, "-p", h.base], "Synthetic hostile tree\n").toString("utf8").trim();
    const work = h.worktrees.describe(`unsafe-${index}`, commit);
    expect(() => h.worktrees.create(h.repository, work)).toThrow(/unsupported_tree_path/); expect(existsSync(work.path)).toBe(false);
  }
});

it("materializes a directory-to-file change without retaining deleted tracked files", () => {
  const h = fixture(), task = h.worktrees.describe("refactor", h.base); h.worktrees.create(h.repository, task);
  rmSync(join(task.path, "src/a.txt")); rmSync(join(task.path, "src/b.txt")); rmdirSync(join(task.path, "src")); writeFileSync(join(task.path, "src"), "replacement file\n");
  h.git(task.path, "add", "-A"); h.git(task.path, "commit", "-m", "Synthetic directory refactor");
  const candidate = h.worktrees.describe("refactor-candidate", h.base), merged = h.worktrees.mergeCandidate(h.repository, candidate, h.base, h.worktrees.head(h.repository, task));
  expect(merged.conflict).toBe(false); expect(readFileSync(join(candidate.path, "src"), "utf8")).toBe("replacement file\n"); expect(h.worktrees.dirty(h.repository, candidate)).toBe(false);
  expect(existsSync(join(h.repoPath, "src/a.txt"))).toBe(true);
});

it("uses actual Git object bytes even when the repository has replacement refs", () => {
  const h = fixture(), original = h.git(h.repoPath, "rev-parse", "HEAD:src/a.txt");
  const replacement = h.worktrees.git(h.repoPath, ["hash-object", "-w", "--stdin"], "REPLACED\n").toString("utf8").trim();
  h.git(h.repoPath, "replace", original, replacement); expect(h.git(h.repoPath, "show", "HEAD:src/a.txt")).toBe("REPLACED");
  const work = h.worktrees.describe("actual-objects", h.base); h.worktrees.create(h.repository, work);
  expect(readFileSync(join(work.path, "src/a.txt"), "utf8")).toBe("A0\n"); expect(h.worktrees.dirty(h.repository, work)).toBe(false);
});

it("preserves ignored target files when a later accepted commit starts tracking their path", () => {
  const h = fixture(); h.git(h.repoPath, "branch", "delivery", h.base);
  const first = h.worktrees.promote(h.repository, "refs/heads/delivery", h.base, h.base);
  expect(first.reason).toBeNull(); expect(first.path).toBeTruthy();
  const retained = join(first.path!, "check-output.txt"); writeFileSync(retained, "owner bytes\n");
  const task = h.worktrees.describe("track-ignored", h.base); h.worktrees.create(h.repository, task);
  writeFileSync(join(task.path, "check-output.txt"), "candidate bytes\n");
  h.git(task.path, "add", "-f", "check-output.txt"); h.git(task.path, "commit", "-m", "Synthetic tracked output");
  const candidate = h.worktrees.describe("track-ignored-candidate", h.base);
  const merged = h.worktrees.mergeCandidate(h.repository, candidate, h.base, h.worktrees.head(h.repository, task));
  expect(h.worktrees.promote(h.repository, "refs/heads/delivery", h.base, merged.sha!).reason).toBe("promotion_worktree_dirty");
  expect(readFileSync(retained, "utf8")).toBe("owner bytes\n");
  expect(h.git(h.repoPath, "rev-parse", "delivery")).toBe(h.base);
});
