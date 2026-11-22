import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

// tools/bump-version.ps1 moves every place that carries Tjommie's version in one step. It is tried here on a COPY of those files in a temporary
// folder (the repository is never touched), to a version far above any real one, so the test holds before and after the real 0.6.0 bump. The
// check is what changed, line by line: only the version-carrying lines, and nothing else, in any file.

vi.setConfig({ testTimeout: 90_000 });

const repo = resolve(import.meta.dirname, "..");
const script = join(repo, "tools", "bump-version.ps1");
const windows = process.platform === "win32";
const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  return "powershell";
})();
const TARGET = "9.9.9";

const folders: string[] = [];
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-bump-test-"));
  folders.push(folder);
  return folder;
};
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

// The files the script reads, as relative paths: what is under packages/ and tjommie-rs/crates/ is found, not listed, so a new package or crate is
// copied too.
function versionFiles(): string[] {
  const files = ["package.json", "package-lock.json", "packages/mobile/android/app/build.gradle", "packages/mobile/scripts/apply-native.mjs", "tjommie-rs/Cargo.toml", "tjommie-rs/Cargo.lock"];
  for (const entry of readdirSync(join(repo, "packages"), { withFileTypes: true })) if (entry.isDirectory() && existsSync(join(repo, "packages", entry.name, "package.json"))) files.push(`packages/${entry.name}/package.json`);
  for (const entry of readdirSync(join(repo, "tjommie-rs", "crates"), { withFileTypes: true })) if (entry.isDirectory()) files.push(`tjommie-rs/crates/${entry.name}/Cargo.toml`);
  return files;
}

function copyOfRepo(): string {
  const root = temp();
  for (const file of versionFiles()) {
    mkdirSync(dirname(join(root, file)), { recursive: true });
    cpSync(join(repo, file), join(root, file));
  }
  return root;
}
const text = (root: string, file: string): string => readFileSync(join(root, file), "utf8");
const snapshot = (root: string): Record<string, string> => Object.fromEntries(versionFiles().map((file) => [file, text(root, file)]));

function bump(root: string, version: string): { status: number | null; output: string } {
  const run = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script, "-Version", version, "-Root", root], { encoding: "utf8", windowsHide: true });
  return { status: run.status, output: `${run.stdout}${run.stderr}` };
}

// The lines that differ, by number, after checking both have the same number of lines (line endings stay as they were, so a CRLF file is
// still CRLF and nothing is added or dropped).
function changedLines(before: string, after: string): { before: string; after: string }[] {
  const a = before.split("\n");
  const b = after.split("\n");
  expect(b.length).toBe(a.length);
  return a.flatMap((line, index) => (line === b[index] ? [] : [{ before: line.replace(/\r$/, ""), after: b[index].replace(/\r$/, "") }]));
}

describe.skipIf(!windows)("the version bump", () => {
  const current = (JSON.parse(readFileSync(join(repo, "package.json"), "utf8")) as { version: string }).version;

  it("changes exactly the lines that carry Tjommie's version, in every file, and no others", () => {
    const root = copyOfRepo();
    const before = snapshot(root);
    const run = bump(root, TARGET);
    expect(run.status, run.output).toBe(0);
    const after = snapshot(root);

    const changed = (file: string): { before: string; after: string }[] => changedLines(before[file], after[file]);
    // npm: one line in package.json, in each package's package.json, and nine in the lock (its own top, the root entry, seven packages).
    const packageFiles = versionFiles().filter((file) => file.endsWith("package.json"));
    expect(packageFiles.length).toBe(8);
    for (const file of packageFiles) expect(changed(file), file).toEqual([{ before: expect.stringMatching(/^\s*"version": "[^"]+",$/), after: `  "version": "${TARGET}",` }]);
    const lock = changed("package-lock.json");
    expect(lock.length).toBe(9);
    for (const line of lock) expect(line.after.trim().replace(/,$/, "")).toBe(`"version": "${TARGET}"`);
    // Android: the name, and the code after the highest the table already holds.
    const codes = [...text(repo, "packages/mobile/scripts/apply-native.mjs").matchAll(/\["\d+\.\d+\.\d+", (\d+)\]/g)].map((match) => Number(match[1]));
    const nextCode = Math.max(...codes) + 1;
    expect(changed("packages/mobile/android/app/build.gradle").map((line) => line.after.trim())).toEqual([`versionCode ${nextCode}`, `versionName "${TARGET}"`]);
    const table = changed("packages/mobile/scripts/apply-native.mjs");
    expect(table.length).toBe(1);
    expect(table[0].after).toBe(table[0].before.replace(/\]\);$/, `, ["${TARGET}", ${nextCode}]]);`));
    // Rust: the workspace version, each pin on another workspace crate, and the lock's entries for the workspace's own crates, nothing else.
    expect(changed("tjommie-rs/Cargo.toml")).toEqual([{ before: expect.stringMatching(/^version = "[^"]+"$/), after: `version = "${TARGET}"` }]);
    for (const file of versionFiles().filter((name) => /crates\/[^/]+\/Cargo\.toml$/.test(name))) {
      const pins = before[file].split("\n").filter((line) => /\{\s*version = "[^"]+",\s*path = /.test(line)).length;
      const lines = changed(file);
      expect(lines.length, file).toBe(pins);
      for (const line of lines) expect(line.after, file).toContain(`version = "${TARGET}"`);
    }
    const lockChanges = changed("tjommie-rs/Cargo.lock");
    expect(lockChanges).toHaveLength(6);
    for (const line of lockChanges) expect(line.after).toBe(`version = "${TARGET}"`);
  });

  it("leaves no stale version of Tjommie's own: every workspace crate, pin and package is at the new one", () => {
    const root = copyOfRepo();
    expect(bump(root, TARGET).status).toBe(0);
    for (const file of versionFiles().filter((name) => name.endsWith("package.json"))) expect((JSON.parse(text(root, file)) as { version: string }).version, file).toBe(TARGET);
    const lock = JSON.parse(text(root, "package-lock.json")) as { version: string; packages: Record<string, { version?: string }> };
    expect(lock.version).toBe(TARGET);
    for (const [name, entry] of Object.entries(lock.packages)) if (name === "" || /^packages\/[^/]+$/.test(name)) expect(entry.version, name).toBe(TARGET);
    // Every pin in a crate's manifest on another workspace crate asks for the new version (an old pin would stop cargo resolving it).
    for (const file of versionFiles().filter((name) => /crates\/[^/]+\/Cargo\.toml$/.test(name))) {
      for (const pin of text(root, file).matchAll(/\{\s*version = "([^"]+)",\s*path = /g)) expect(pin[1], file).toBe(TARGET);
    }
    // In the lock, every package that has no source is one of the workspace's own, and is at the new version.
    const entries = text(root, "tjommie-rs/Cargo.lock").replaceAll("\r\n", "\n").split("\n\n").filter((entry) => entry.startsWith("[[package]]"));
    const own = entries.filter((entry) => !/^source = /m.test(entry) && /^name = "(?:client|daemon|desktop|desktop-host|protocol|transport)"$/m.test(entry));
    expect(own).toHaveLength(6);
    for (const entry of own) expect(entry).toContain(`version = "${TARGET}"`);
    // Other crates' versions are not Tjommie's: the lock still holds versions that are not the new one.
    expect(text(root, "tjommie-rs/Cargo.lock").match(/^version = "(?!9\.9\.9")/gm)?.length ?? 0).toBeGreaterThan(100);
    // And the Android table now knows the version.
    expect(text(root, "packages/mobile/scripts/apply-native.mjs")).toContain(`["${TARGET}", `);
  });

  it("does nothing the second time, and never goes down or accepts a version that is not x.y.z", () => {
    const root = copyOfRepo();
    expect(bump(root, TARGET).status).toBe(0);
    const done = snapshot(root);
    const again = bump(root, TARGET);
    expect(again.status, again.output).toBe(0);
    expect(again.output).toContain(`Already at ${TARGET}`);
    expect(snapshot(root)).toEqual(done);
    for (const [version, message] of [["0.0.1", /lower\s+than\s+the\s+current/], ["9.9", /x\.y\.z/], ["v9.9.10", /x\.y\.z/], ["9.9.10-rc1", /x\.y\.z/]] as const) {
      const refused = bump(root, version);
      expect(refused.status, version).not.toBe(0);
      expect(refused.output, version).toMatch(message);
    }
    expect(snapshot(root)).toEqual(done);
  });

  it("writes nothing at all when a file has changed shape, so a half-bumped tree is never left behind", () => {
    const root = copyOfRepo();
    writeFileSync(join(root, "packages/mobile/scripts/apply-native.mjs"), text(root, "packages/mobile/scripts/apply-native.mjs").replace(/const versionCodes = new Map\(/, "const codes = new Map("));
    const before = snapshot(root);
    const run = bump(root, TARGET);
    expect(run.status).not.toBe(0);
    expect(run.output).toMatch(/versionCodes\s+table/);
    expect(snapshot(root)).toEqual(before);
    // A manifest whose pin the script cannot find the lock entry for is the same: refused whole.
    const root2 = copyOfRepo();
    writeFileSync(join(root2, "tjommie-rs/Cargo.lock"), text(root2, "tjommie-rs/Cargo.lock").replace(/^name = "transport"\r?\n/m, (match) => match.replace("transport", "transport-x")));
    const before2 = snapshot(root2);
    expect(bump(root2, TARGET).status).not.toBe(0);
    expect(snapshot(root2)).toEqual(before2);
  });
});

describe("the real tree", () => {
  it("is at one version everywhere the script looks, so the script starts from a consistent state", () => {
    const root = JSON.parse(readFileSync(join(repo, "package.json"), "utf8")) as { version: string };
    for (const entry of readdirSync(join(repo, "packages"), { withFileTypes: true })) {
      const file = join(repo, "packages", entry.name, "package.json");
      if (entry.isDirectory() && existsSync(file)) expect((JSON.parse(readFileSync(file, "utf8")) as { version: string }).version, entry.name).toBe(root.version);
    }
    expect(readFileSync(join(repo, "tjommie-rs", "Cargo.toml"), "utf8")).toMatch(new RegExp(`^version = "${root.version.replaceAll(".", "\\.")}"`, "m"));
    expect(readFileSync(join(repo, "packages", "mobile", "android", "app", "build.gradle"), "utf8")).toContain(`versionName "${root.version}"`);
  });
});
