import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

// T8.3, the review's F1: the tools the release build fetches by version only (NSIS from Chocolatey) are checked against SHA-256 values
// recorded in installer/tool-hashes.json. The script reads and hashes files; here it is tried on a small stand-in NSIS folder and on the
// recorded values themselves. Nothing here installs anything, and nothing runs a file it hashes.

vi.setConfig({ testTimeout: 90_000 });

const repo = resolve(import.meta.dirname, "..");
const script = join(repo, "installer", "check-tool-hashes.ps1");
const windows = process.platform === "win32";
const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  return "powershell";
})();

const folders: string[] = [];
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-tools-test-"));
  folders.push(folder);
  return folder;
};
const sha256 = (text: string): string => createHash("sha256").update(text).digest("hex");

type Run = { status: number | null; output: string };
function check(args: string[]): Run {
  const run = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { encoding: "utf8", windowsHide: true });
  return { status: run.status, output: `${run.stdout}${run.stderr}` };
}

// A stand-in NSIS: the files the manifest's include list names, and one that it does not (Docs), which must not matter.
const nsisFiles: Record<string, string> = {
  "makensis.exe": "compiler",
  "Bin\\makensis.exe": "real compiler",
  "Stubs\\lzma": "stub",
  "Include\\LogicLib.nsh": "include",
  "Plugins\\x86-unicode\\nsExec.dll": "plugin",
};
const include = ["makensis.exe", "Bin", "Stubs", "Include", "Plugins\\x86-unicode"];

function stand(root = temp()): { root: string; folder: string } {
  const folder = join(root, "tjommie-a-long-stand-in-nsis-folder");
  for (const [name, content] of Object.entries({ ...nsisFiles, "Docs\\readme.txt": "not part of the build" })) {
    const path = join(folder, name);
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(path, content);
  }
  return { root, folder };
}

function manifestFile(root: string, nsis: { files?: Record<string, string> | null }, pdfium: { sha256?: string | null } = { sha256: null }): string {
  const file = join(root, "tool-hashes.json");
  writeFileSync(file, JSON.stringify({ schema: 1, nsis: { version: "3.11", include, files: nsis.files ?? null }, pdfium: { version: null, source: null, sha256: pdfium.sha256 ?? null } }));
  return file;
}
const recorded = (): Record<string, string> => Object.fromEntries(Object.entries(nsisFiles).map(([name, content]) => [name, sha256(content)]));

describe.skipIf(!windows)("the tool hash check: NSIS", () => {
  it("passes a tree whose files are the recorded ones, ignores the files it was not told about, and writes what it saw", () => {
    const { root, folder } = stand();
    const observed = join(root, "observed.json");
    const run = check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifestFile(root, { files: recorded() }), "-Observed", observed]);
    expect(run.status, run.output).toBe(0);
    expect(run.output).toContain("matches the 5 recorded");
    const seen = JSON.parse(readFileSync(observed, "utf8")) as { tool: string; version: string; status: string; fileCount: number; files: Record<string, string> };
    expect(seen).toMatchObject({ tool: "nsis", version: "3.11", status: "verified", fileCount: 5 });
    expect(seen.files).toEqual(recorded());
    expect(Object.keys(seen.files)).not.toContain("Docs\\readme.txt");
  });

  it("fails a file that changed, one that is missing and one that is extra, and names each", () => {
    const { root, folder } = stand();
    writeFileSync(join(folder, "Bin", "makensis.exe"), "a different compiler");
    rmSync(join(folder, "Stubs", "lzma"));
    writeFileSync(join(folder, "Stubs", "bzip2"), "an extra stub");
    const run = check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifestFile(root, { files: recorded() })]);
    expect(run.status).not.toBe(0);
    expect(run.output).toContain("changed: Bin\\makensis.exe");
    expect(run.output).toContain(`recorded ${sha256("real compiler")}`);
    expect(run.output).toContain(`observed ${sha256("a different compiler")}`);
    expect(run.output).toContain("missing: Stubs\\lzma");
    expect(run.output).toContain("extra: Stubs\\bzip2");
  });

  it("fails an NSIS that lacks a folder the build needs, and a folder that is not there", () => {
    const { root, folder } = stand();
    rmSync(join(folder, "Include"), { recursive: true, force: true });
    const lacking = check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifestFile(root, { files: recorded() })]);
    expect(lacking.status).not.toBe(0);
    expect(lacking.output).toContain("NSIS is missing Include");
    expect(check(["-Tool", "nsis", "-Path", join(root, "nowhere"), "-Manifest", manifestFile(root, { files: recorded() })]).status).not.toBe(0);
  });

  it("says a tool is pending when nothing is recorded: a dry run passes with a warning and the values to record, a release fails", () => {
    const { root, folder } = stand();
    const manifest = manifestFile(root, { files: null });
    const observed = join(root, "observed.json");
    const dry = check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifest, "-Observed", observed]);
    expect(dry.status, dry.output).toBe(0);
    expect(dry.output).toContain("::warning::");
    expect(dry.output).toContain("no recorded hashes");
    const seen = JSON.parse(readFileSync(observed, "utf8")) as { status: string; files: Record<string, string> };
    expect(seen.status).toBe("pending");
    expect(seen.files).toEqual(recorded()); // what a person records, after reviewing it
    const release = check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifest, "-Release"]);
    expect(release.status).not.toBe(0);
    expect(release.output).toContain("A release is not built with a tool nobody has pinned");
  });

  it("holds a release to the recorded values too: a mismatch fails with -Release, a match passes", () => {
    const { root, folder } = stand();
    const manifest = manifestFile(root, { files: recorded() });
    expect(check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifest, "-Release"]).status).toBe(0);
    writeFileSync(join(folder, "Include", "LogicLib.nsh"), "tampered");
    expect(check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifest, "-Release"]).status).not.toBe(0);
    expect(check(["-Tool", "nsis", "-Path", folder, "-Manifest", manifest]).status, "and in a dry run").not.toBe(0);
  });

  // A hosted runner's profile and temp folders are spelled with an 8.3 alias; a root given that way must not leave debris in the names
  // (the same fault the acceptance library had). Where the volume makes no short name there is nothing to try.
  it("reads the same names whether the folder is given by its long name or its 8.3 alias", () => {
    const { root, folder } = stand();
    const alias = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-Command", `(New-Object -ComObject Scripting.FileSystemObject).GetFolder('${folder}').ShortPath`], { encoding: "utf8", windowsHide: true }).stdout.trim();
    const manifest = manifestFile(root, { files: recorded() });
    for (const path of [folder, alias]) {
      const run = check(["-Tool", "nsis", "-Path", path, "-Manifest", manifest]);
      expect(run.status, `${path}: ${run.output}`).toBe(0);
    }
    if (alias.toLowerCase() === folder.toLowerCase()) console.warn("This volume has no 8.3 name for the test folder, so only the long spelling was exercised.");
  });
});

describe.skipIf(!windows)("the tool hash check: pdfium (the hook the Chat lane fills in T4.4)", () => {
  const dll = (root: string, content = "pdfium"): string => {
    const file = join(root, "pdfium.dll");
    writeFileSync(file, content);
    return file;
  };

  it("passes the recorded file, fails another, and fails one that is missing", () => {
    const root = temp();
    const manifest = manifestFile(root, { files: recorded() }, { sha256: sha256("pdfium") });
    expect(check(["-Tool", "pdfium", "-Path", dll(root), "-Manifest", manifest, "-Release"]).status).toBe(0);
    const other = check(["-Tool", "pdfium", "-Path", dll(root, "a different one"), "-Manifest", manifest]);
    expect(other.status).not.toBe(0);
    expect(other.output).toContain("changed: pdfium.dll");
    expect(check(["-Tool", "pdfium", "-Path", join(root, "absent.dll"), "-Manifest", manifest]).status).not.toBe(0);
  });

  it("is pending until a hash is recorded: a dry run passes with a warning, a release fails", () => {
    const root = temp();
    const manifest = manifestFile(root, { files: recorded() });
    const dry = check(["-Tool", "pdfium", "-Path", dll(root), "-Manifest", manifest]);
    expect(dry.status, dry.output).toBe(0);
    expect(dry.output).toContain("::warning::");
    expect(check(["-Tool", "pdfium", "-Path", dll(root), "-Manifest", manifest, "-Release"]).status).not.toBe(0);
  });
});

describe("the recorded values", () => {
  const manifest = JSON.parse(readFileSync(join(repo, "installer", "tool-hashes.json"), "utf8")) as {
    schema: number;
    nsis: { version: string; include: string[]; files: Record<string, string>; recordedFrom: { sha256: string; bytes: number; source: string } };
    pdfium: { version: string | null; source: string | null; sha256: string | null; note: string };
  };

  it("hold a hash for every file the build can use, each a SHA-256, each under a folder the check reads", () => {
    const entries = Object.entries(manifest.nsis.files);
    expect(entries.length).toBeGreaterThanOrEqual(60);
    for (const [name, hash] of entries) {
      expect(hash, name).toMatch(/^[0-9a-f]{64}$/);
      expect(manifest.nsis.include.some((part) => name === part || name.startsWith(`${part}\\`)), `${name} is under an included path`).toBe(true);
    }
    for (const needed of ["Bin\\makensis.exe", "Include\\LogicLib.nsh", "Include\\FileFunc.nsh", "Plugins\\x86-unicode\\nsExec.dll", "Plugins\\x86-unicode\\System.dll"]) expect(Object.keys(manifest.nsis.files), needed).toContain(needed);
    // The stub the installer is compiled with: Unicode, solid LZMA (the installer script says so).
    expect(Object.keys(manifest.nsis.files).some((name) => /^Stubs\\lzma_solid-x86-unicode$/.test(name))).toBe(true);
    expect(manifest.nsis.recordedFrom.sha256).toMatch(/^[0-9a-f]{64}$/);
    expect(manifest.nsis.recordedFrom.source).toMatch(/^https:\/\//);
  });

  it("are for the NSIS version the workflow installs", () => {
    const workflow = readFileSync(join(repo, ".github", "workflows", "release.yml"), "utf8");
    expect(/choco install nsis --version=([\d.]+)/.exec(workflow)?.[1]).toBe(manifest.nsis.version);
  });

  it("leave pdfium pending, with the way to fill it in written down where the Chat lane will look", () => {
    expect(manifest.pdfium).toMatchObject({ version: null, source: null, sha256: null });
    expect(manifest.pdfium.note).toContain("T4.4");
    expect(manifest.pdfium.note).toContain("check-tool-hashes.ps1 -Tool pdfium");
    expect(manifest.pdfium.note).toContain("assemble-payload.ps1 -Pdfium");
  });
});
