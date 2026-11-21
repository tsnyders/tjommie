import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SPEECH_FILES, renderNotices } from "./make-third-party-notices.ts";

// The third-party notices (0.6.0): scripts/make-third-party-notices.ts puts the Rust crates (from cargo-about), pdfium, the speech runtime,
// the fonts and the Lucide icon set into the one file the About screen shows; installer/fetch-cargo-about.ps1 fetches the tool at a
// pinned hash. Nothing here runs cargo-about or fetches from the internet: the generator reads stand-in files, and the fetch script is
// served a stand-in archive from this machine.

vi.setConfig({ testTimeout: 90_000 });

const repo = resolve(import.meta.dirname, "..");
const installer = join(repo, "installer");
const script = join(import.meta.dirname, "make-third-party-notices.ts");
const windows = process.platform === "win32";
const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  return "powershell";
})();

const folders: string[] = [];
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-notices-test-"));
  folders.push(folder);
  return folder;
};
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

const CRATES = "================\nMIT License (MIT)\n\nUsed by:\n  serde 1.0.0 - https://example.test/serde\n\nPermission is hereby granted, free of charge.\n";
type Stub = { root: string; crates: string; speech: string; pdfium: string; out: string };
function stub(): Stub {
  const root = temp();
  const speech = join(root, "speech");
  const pdfium = join(root, "pdfium-licenses");
  mkdirSync(speech);
  mkdirSync(pdfium);
  for (const name of SPEECH_FILES) writeFileSync(join(speech, name), `the text of ${name}\r\nsecond line\r\n`);
  writeFileSync(join(pdfium, "LICENSE"), "pdfium's own licence text, BSD-3-Clause\r\n");
  writeFileSync(join(pdfium, "freetype.txt"), "a library inside pdfium");
  writeFileSync(join(root, "crates.txt"), CRATES);
  return { root, crates: join(root, "crates.txt"), speech, pdfium, out: join(root, "out", "third-party-notices.txt") };
}
const run = (args: string[]): { status: number | null; output: string } => {
  const result = spawnSync(process.execPath, [script, ...args], { encoding: "utf8", windowsHide: true, cwd: repo });
  return { status: result.status, output: `${result.stdout}${result.stderr}` };
};
const base = (s: Stub): string[] => ["--version", "0.6.0", "--crates", s.crates, "--speech", s.speech, "--out", s.out];

describe("the third-party notices", () => {
  it("name every part the app ships with: the crates, pdfium, whisper, the fonts and Lucide, each with its licence text", () => {
    const s = stub();
    const made = run([...base(s), "--pdfium-licenses", s.pdfium, "--strict"]);
    expect(made.status, made.output).toBe(0);
    const text = readFileSync(s.out, "utf8");
    expect(text).toContain("Tjommie 0.6.0 - third-party notices");
    // 1. the crates, as cargo-about wrote them
    expect(text).toContain("serde 1.0.0 - https://example.test/serde");
    expect(text).toContain("Permission is hereby granted, free of charge.");
    // 2. pdfium: named with the release the workflow pins, its own licence, and where the rest is installed
    expect(text).toMatch(/pdfium, the PDF engine/);
    expect(text).toContain("chromium/8125");
    expect(text).toContain("pdfium's own licence text, BSD-3-Clause");
    expect(text).toContain("resources\\pdfium-licenses");
    // 3. the speech runtime: whisper.cpp, ggml, OpenAI Whisper and the model notice
    for (const name of SPEECH_FILES) expect(text, name).toContain(`the text of ${name}`);
    expect(text).toMatch(/whisper\.cpp/);
    // 4. the fonts, with the real OFL text from the repository, which names both families
    expect(text).toContain("Archivo");
    expect(text).toContain("JetBrains Mono");
    expect(text).toContain("SIL OPEN FONT LICENSE");
    // 5. Lucide, with the real ISC text and the MIT text for Feather's icons
    expect(text).toContain("Lucide");
    expect(text).toContain("ISC License");
    expect(text).toContain("Cole Bemis");
  });

  it("is LF only, and ends with one newline", () => {
    const s = stub();
    expect(run([...base(s), "--pdfium-licenses", s.pdfium]).status).toBe(0);
    const text = readFileSync(s.out, "utf8");
    expect(text).not.toContain("\r");
    expect(text.endsWith("\n")).toBe(true);
    expect(text.endsWith("\n\n")).toBe(false);
  });

  it("goes on without pdfium in a dry run, and says so in the file; a release (--strict) refuses", () => {
    const s = stub();
    const dry = run(base(s));
    expect(dry.status, dry.output).toBe(0);
    expect(dry.output).toContain("without pdfium");
    expect(readFileSync(s.out, "utf8")).toContain("pdfium is not part of this build");
    const release = run([...base(s), "--strict", "--out", join(s.root, "strict.txt")]);
    expect(release.status).not.toBe(0);
    expect(release.output).toContain("--pdfium-licenses");
  });

  it("never writes a shorter file: a missing or empty part is an error, and so is a cargo-about output with no crates in it", () => {
    const s = stub();
    const cases: Array<[string, () => void, RegExp]> = [
      ["a missing speech licence", () => rmSync(join(s.speech, "WHISPER-LICENSE.txt")), /WHISPER-LICENSE\.txt is missing/],
      ["an empty speech licence", () => writeFileSync(join(s.speech, "GGML-LICENSE.txt"), "  \n"), /GGML-LICENSE\.txt is empty/],
      ["no model notice", () => rmSync(join(s.speech, "MODEL-NOTICE.txt")), /MODEL-NOTICE\.txt is missing/],
      ["a pdfium folder with no LICENSE", () => rmSync(join(s.pdfium, "LICENSE")), /LICENSE is missing/],
      ["a cargo-about output with no crates", () => writeFileSync(s.crates, "Nothing was found.\n"), /no crates in it/],
    ];
    for (const [label, spoil, message] of cases) {
      spoil();
      const result = run([...base(s), "--pdfium-licenses", s.pdfium]);
      expect(result.status, label).not.toBe(0);
      expect(result.output, label).toMatch(message);
      // put the stand-in back for the next case
      for (const name of SPEECH_FILES) writeFileSync(join(s.speech, name), `the text of ${name}`);
      writeFileSync(join(s.pdfium, "LICENSE"), "pdfium's own licence");
      writeFileSync(s.crates, CRATES);
    }
    expect(run(base(s).slice(2)).output).toContain("--version is required");
    expect(run(["--version", "six", "--crates", s.crates, "--speech", s.speech, "--out", s.out]).output).toContain("x.y.z");
  });

  it("renders the parts in the order its own contents list gives", () => {
    const text = renderNotices({
      version: "1.2.3",
      crates: "C",
      pdfium: { release: "r", source: "s", license: "P" },
      speech: [{ name: "n.txt", text: "S" }],
      fonts: "F",
      lucide: "L",
    });
    const at = ["1. Rust crates\n", "2. pdfium\n", "3. Speech recognition", "4. Fonts", "5. Icons"].map((heading) => text.lastIndexOf(`${"*".repeat(80)}\n${heading}`));
    expect(at.every((index) => index > 0)).toBe(true);
    expect(at).toEqual([...at].sort((a, b) => a - b));
  });
});

// The settings cargo-about is run with: what is in installer/about.toml and about.hbs is what the licences of the shipped crates are held to.
describe("the cargo-about settings", () => {
  const config = readFileSync(join(installer, "about.toml"), "utf8");
  const template = readFileSync(join(installer, "about.hbs"), "utf8");
  const accepted = [...config.matchAll(/^\s*"([^"]+)",/gm)].map((match) => match[1]);

  it("accept permissive licences by name, and none of the strong copyleft or the unknown", () => {
    // ISC is what libloading (pdfium-render's loader) is under, so the PDF viewer's crates are on the list.
    for (const id of ["MIT", "Apache-2.0", "BSD-3-Clause", "Unicode-3.0", "ISC"]) expect(accepted, id).toContain(id);
    for (const id of accepted) expect(id, "an SPDX identifier, not an expression").toMatch(/^[A-Za-z0-9][A-Za-z0-9.+-]*$/);
    expect(accepted.filter((id) => /^(?:A?GPL|LGPL|SSPL|BUSL|CC-BY-NC|Commons-Clause|LicenseRef)/i.test(id))).toEqual([]);
    // Each is on the list on purpose (the file says which crate needed it); a duplicate is a mistake.
    expect(new Set(accepted).size).toBe(accepted.length);
  });

  it("cover the Windows binaries only: no build or dev dependency, no target but Windows, none of Tjommie's own crates", () => {
    expect(config).toMatch(/^targets = \["x86_64-pc-windows-msvc"\]$/m);
    expect(config).toMatch(/^ignore-build-dependencies = true$/m);
    expect(config).toMatch(/^ignore-dev-dependencies = true$/m);
    expect(config).toMatch(/^private = \{ ignore = true \}$/m);
  });

  it("render plain text: every value is written unescaped, or a licence with a < or an & in it would be garbled", () => {
    // Every tag that prints a value (not a #block, a /close or a !comment) opens with three braces, which Handlebars does not escape.
    const prints = [...template.matchAll(/\{\{(?![#/!])(\{?)/g)];
    expect(prints.length).toBeGreaterThan(0);
    for (const open of prints) expect(open[1], "a plain {{x}} is HTML-escaped; use {{{x}}}").toBe("{");
    for (const field of ["name", "id", "text", "crate.name", "crate.version"]) expect(template).toContain(`{{{${field}}}}`);
    expect(template).toContain("Used by:");
  });
});

const sha256 = (data: Buffer): string => createHash("sha256").update(data).digest("hex");

// installer/fetch-cargo-about.ps1: the archive is checked before it is extracted, the exe after, and nothing lands in -Destination on a mismatch.
// A stand-in archive (made here with Windows' own tar) is served from 127.0.0.1; no real cargo-about is involved.
describe.skipIf(!windows)("fetching cargo-about at the recorded hash", () => {
  const tar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");

  async function withServer<T>(body: Buffer, use: (url: string) => Promise<T>): Promise<T> {
    const server = createServer((_, response) => { response.end(body); });
    await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
    try {
      const address = server.address();
      if (address === null || typeof address === "string") throw new Error("the stand-in server has no port");
      return await use(`http://127.0.0.1:${address.port}/cargo-about.tar.gz`);
    } finally {
      server.closeAllConnections();
      await new Promise<void>((done) => server.close(() => done()));
    }
  }

  function standIn(exeBytes: string): { archive: Buffer; exe: Buffer } {
    const root = temp();
    const folder = join(root, "cargo-about-9.9.9-x86_64-pc-windows-msvc");
    mkdirSync(folder);
    writeFileSync(join(folder, "cargo-about.exe"), exeBytes);
    writeFileSync(join(folder, "README.md"), "readme");
    const archive = join(root, "a.tar.gz");
    expect(spawnSync(tar, ["-czf", archive, "-C", root, "cargo-about-9.9.9-x86_64-pc-windows-msvc"], { windowsHide: true }).status).toBe(0);
    return { archive: readFileSync(archive), exe: readFileSync(join(folder, "cargo-about.exe")) };
  }

  const manifest = (archive: Buffer, exe: Buffer, extra: Record<string, string> = {}): string => {
    const file = join(temp(), "tool-hashes.json");
    writeFileSync(file, JSON.stringify({ schema: 1, "cargo-about": { version: "9.9.9", source: "https://example.test/unused", archiveSha256: sha256(archive), sha256: sha256(exe), ...extra } }));
    return file;
  };
  // Asynchronous, on purpose: the stand-in server lives in this process, and a blocking spawnSync would stop it from ever answering.
  const fetchTo = (destination: string, manifestFile: string, url: string): Promise<{ status: number | null; output: string }> =>
    new Promise((done) => {
      const child = spawn(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(installer, "fetch-cargo-about.ps1"), "-Destination", destination, "-Manifest", manifestFile, "-Url", url], { windowsHide: true });
      let output = "";
      child.stdout.on("data", (chunk: Buffer) => { output += chunk.toString("utf8"); });
      child.stderr.on("data", (chunk: Buffer) => { output += chunk.toString("utf8"); });
      child.on("close", (status) => done({ status, output }));
    });

  it("extracts the exe into the destination when the archive and the exe are the recorded ones", async () => {
    const { archive, exe } = standIn("a stand-in cargo-about");
    const destination = join(temp(), "tool");
    const result = await withServer(archive, async (url) => fetchTo(destination, manifest(archive, exe), url));
    expect(result.status, result.output).toBe(0);
    expect(readFileSync(join(destination, "cargo-about.exe"), "utf8")).toBe("a stand-in cargo-about");
    expect(readFileSync(join(destination, "cargo-about.exe")).equals(exe)).toBe(true);
  });

  it("refuses an archive that is not the recorded one, before extracting it, and writes nothing", async () => {
    const { archive, exe } = standIn("a stand-in cargo-about");
    const other = standIn("a different one");
    const destination = join(temp(), "tool");
    const result = await withServer(other.archive, async (url) => fetchTo(destination, manifest(archive, exe), url));
    expect(result.status).not.toBe(0);
    expect(result.output).toMatch(/archive has SHA-256/);
    expect(result.output).toMatch(/Nothing\s+was\s+extracted/);
    expect(() => readFileSync(join(destination, "cargo-about.exe"))).toThrow();
  });

  it("refuses an exe that is not the recorded one even when the archive is, and a tool that has no pin", async () => {
    const { archive, exe } = standIn("a stand-in cargo-about");
    const destination = join(temp(), "tool");
    const wrongExe = await withServer(archive, async (url) => fetchTo(destination, manifest(archive, Buffer.from("not the exe")), url));
    expect(wrongExe.status).not.toBe(0);
    expect(wrongExe.output).toMatch(/changed: cargo-about\.exe/);
    expect(() => readFileSync(join(destination, "cargo-about.exe"))).toThrow();
    const unpinned = await withServer(archive, async (url) => fetchTo(join(temp(), "tool"), manifest(archive, exe, { archiveSha256: "" }), url));
    expect(unpinned.status).not.toBe(0);
    expect(unpinned.output).toMatch(/archiveSha256\s+is\s+not\s+recorded/);
  });
});

describe("the recorded cargo-about", () => {
  const entry = (JSON.parse(readFileSync(join(installer, "tool-hashes.json"), "utf8")) as { "cargo-about": { version: string; source: string; archiveSha256: string; sha256: string } })["cargo-about"];

  it("is a version, the archive it comes in and the exe inside it, from the project's own release page", () => {
    expect(entry.version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(entry.source).toBe(`https://github.com/EmbarkStudios/cargo-about/releases/download/${entry.version}/cargo-about-${entry.version}-x86_64-pc-windows-msvc.tar.gz`);
    expect(entry.archiveSha256).toMatch(/^[0-9a-f]{64}$/);
    expect(entry.sha256).toMatch(/^[0-9a-f]{64}$/);
    expect(entry.sha256).not.toBe(entry.archiveSha256);
  });
});
