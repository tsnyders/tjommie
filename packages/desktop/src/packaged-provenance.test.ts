import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, expect, it } from "vitest";
import { buildInputHash, packagedExecutable, provenanceFile, verifyPackagedProvenance, writePackagedProvenance } from "../scripts/packaged-provenance.ts";

const roots: string[] = [];
function put(repo: string, path: string, content = "original"): void {
  mkdirSync(dirname(join(repo, path)), { recursive: true });
  writeFileSync(join(repo, path), content);
}
function git(repo: string, args: string[]): void {
  execFileSync("git", args, { cwd: repo, windowsHide: true, stdio: "pipe" });
}
function fixture(): string {
  const repo = mkdtempSync(join(tmpdir(), "tjommie-provenance-"));
  roots.push(repo);
  git(repo, ["init", "--quiet"]);
  put(repo, ".gitignore", "node_modules/\ndist-desktop/\n.next/\nout/\n*.tsbuildinfo\nnext-env.d.ts\n");
  for (const path of ["package.json", "package-lock.json", "tsconfig.json", "vitest.config.ts", "packages/desktop/src/main.ts", "packages/desktop/assets/icon.png"]) put(repo, path);
  git(repo, ["add", "."]);
  put(repo, packagedExecutable, "executable bytes");
  put(repo, "dist-desktop/installer/Tjommie Setup 0.1.0.exe", "installer bytes");
  put(repo, "dist-desktop/installer/win-unpacked/resources/app.asar", "desktop payload");
  put(repo, "dist-desktop/installer/win-unpacked/resources/daemon/main.mjs", "daemon payload");
  put(repo, "dist-desktop/installer/win-unpacked/resources/ui/index.html", "UI payload");
  return repo;
}
afterEach(() => {
  for (const repo of roots.splice(0)) {
    const target = resolve(repo);
    if (dirname(target) !== resolve(tmpdir()) || !target.startsWith(join(resolve(tmpdir()), "tjommie-provenance-")) || lstatSync(target).isSymbolicLink()) {
      throw new Error("Refusing to remove a non-disposable provenance fixture.");
    }
    rmSync(target, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

it("hashes worktree content consistently before and after staging an untracked source", () => {
  const repo = fixture();
  const original = buildInputHash(repo);
  put(repo, "packages/desktop/src/new source.ts", "new code");
  const withUntracked = buildInputHash(repo);
  expect(withUntracked).not.toBe(original);
  git(repo, ["add", "packages/desktop/src/new source.ts"]);
  expect(buildInputHash(repo)).toBe(withUntracked);
  expect(buildInputHash(repo)).toBe(withUntracked);
});

it.each(["package.json", "package-lock.json", "tsconfig.json", "vitest.config.ts", "packages/desktop/src/main.ts", "packages/desktop/assets/icon.png", "packages/desktop/scripts/package.ts", "packages/ui/next.config.ts", "packages/ui/app/page.tsx"])("detects changed or added build input %s", (path) => {
  const repo = fixture();
  const original = buildInputHash(repo);
  put(repo, path, "changed");
  expect(buildInputHash(repo)).not.toBe(original);
});

it("detects deletions and renames even when the file content is unchanged", () => {
  const repo = fixture();
  const original = buildInputHash(repo);
  const source = join(repo, "packages/desktop/src/main.ts");
  renameSync(source, join(repo, "packages/desktop/src/renamed.ts"));
  expect(buildInputHash(repo)).not.toBe(original);
  rmSync(join(repo, "packages/desktop/src/renamed.ts"));
  expect(buildInputHash(repo)).not.toBe(original);
});

it("ignores documentation and generated files, including accidentally tracked generated output", () => {
  const repo = fixture();
  const original = buildInputHash(repo);
  for (const path of ["README.md", "docs/plan.md", "packages/ui/AGENTS.md", "packages/ui/docs/readme.md", "packages/ui/out/index.html", "packages/ui/.next/build.js", "packages/ui/next-env.d.ts", "packages/ui/tsconfig.tsbuildinfo", "packages/desktop/node_modules/dependency.js"]) put(repo, path, "generated or docs");
  git(repo, ["add", "--force", "packages/ui/out/index.html"]);
  expect(buildInputHash(repo)).toBe(original);
});

it("accepts a completed build after documentation edits without depending on a Git commit", () => {
  const repo = fixture();
  writePackagedProvenance(repo, buildInputHash(repo));
  put(repo, "docs/review.md", "documentation after packaging");
  expect(() => verifyPackagedProvenance(repo)).not.toThrow();
});

it("rejects missing, malformed and unsupported provenance", () => {
  const repo = fixture();
  expect(() => verifyPackagedProvenance(repo)).toThrow("provenance is missing");
  put(repo, provenanceFile, "{");
  expect(() => verifyPackagedProvenance(repo)).toThrow("unreadable");
  put(repo, provenanceFile, JSON.stringify({ version: 2 }));
  expect(() => verifyPackagedProvenance(repo)).toThrow("provenance is invalid");
});

it("rejects an older build when current worktree source changes", () => {
  const repo = fixture();
  writePackagedProvenance(repo, buildInputHash(repo));
  put(repo, "packages/desktop/src/main.ts", "new source after packaging");
  expect(() => verifyPackagedProvenance(repo)).toThrow("build is stale");
});

it.each([packagedExecutable, "dist-desktop/installer/Tjommie Setup 0.1.0.exe", "dist-desktop/installer/win-unpacked/resources/app.asar", "dist-desktop/installer/win-unpacked/resources/daemon/main.mjs", "dist-desktop/installer/win-unpacked/resources/ui/index.html"])("rejects a replaced packaged artifact %s", (path) => {
  const repo = fixture();
  writePackagedProvenance(repo, buildInputHash(repo));
  put(repo, path, "different artifact bytes");
  expect(() => verifyPackagedProvenance(repo)).toThrow("artifacts do not match their provenance");
});

it("rejects a missing packaged executable or NSIS installer", () => {
  const repo = fixture();
  writePackagedProvenance(repo, buildInputHash(repo));
  rmSync(join(repo, packagedExecutable));
  expect(() => verifyPackagedProvenance(repo)).toThrow("executable is missing");
  put(repo, packagedExecutable, "executable bytes");
  rmSync(join(repo, "dist-desktop/installer/Tjommie Setup 0.1.0.exe"));
  expect(() => verifyPackagedProvenance(repo)).toThrow("NSIS installer is required");
});

it("rejects added or deleted payload files", () => {
  const repo = fixture();
  writePackagedProvenance(repo, buildInputHash(repo));
  const extra = "dist-desktop/installer/win-unpacked/resources/extra.js";
  put(repo, extra, "unexpected payload");
  expect(() => verifyPackagedProvenance(repo)).toThrow("artifacts do not match their provenance");
  rmSync(join(repo, extra));
  rmSync(join(repo, "dist-desktop/installer/win-unpacked/resources/ui/index.html"));
  expect(() => verifyPackagedProvenance(repo)).toThrow("artifacts do not match their provenance");
});

it("does not stamp a build whose inputs changed while packaging", () => {
  const repo = fixture();
  const initial = buildInputHash(repo);
  put(repo, "packages/desktop/src/main.ts", "changed during build");
  expect(() => writePackagedProvenance(repo, initial)).toThrow("changed during packaging");
  expect(existsSync(join(repo, provenanceFile))).toBe(false);
});

it("rejects malformed artifact entries in the provenance manifest", () => {
  const repo = fixture();
  writePackagedProvenance(repo, buildInputHash(repo));
  const manifest = JSON.parse(readFileSync(join(repo, provenanceFile), "utf8")) as Record<string, unknown>;
  put(repo, provenanceFile, JSON.stringify({ ...manifest, artifacts: [[packagedExecutable, "invalid digest"]] }));
  expect(() => verifyPackagedProvenance(repo)).toThrow("provenance is invalid");
});
