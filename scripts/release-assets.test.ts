import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseUpdateInfo } from "electron-updater/out/providers/Provider.js";
import { afterEach, describe, expect, it } from "vitest";
import { renderLatestYml } from "./make-latest-yml.ts";

// T8.3: the release's `latest.yml`. R1's electron-updater reads it, and so does the Rust updater (M6), so it must be exactly electron-builder's
// format. The reference is the real file published with v0.5.1, kept byte for byte in scripts/fixtures (.gitattributes keeps its line endings).
// Nothing here runs an installer; the "installer" below is a few bytes.

const repo = resolve(import.meta.dirname, "..");
const script = join(import.meta.dirname, "make-latest-yml.ts");
const real = readFileSync(join(import.meta.dirname, "fixtures", "latest-v0.5.1.yml"), "utf8");

const folders: string[] = [];
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-release-assets-"));
  folders.push(folder);
  return folder;
};
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

type Fields = { version: string; installerName: string; sha512: string; size: number; releaseDate: string };
// The fields, read by electron-updater's own parser (the one R1 uses), so what is checked is what R1 would see.
function fieldsOf(text: string): Fields {
  const info = parseUpdateInfo(text, "latest.yml", new URL("https://example.com/latest.yml"));
  const file = info.files[0];
  if (!file) throw new Error("no files entry");
  if (typeof file.size !== "number") throw new Error("the files entry has no size");
  return { version: info.version, installerName: info.path, sha512: info.sha512, size: file.size, releaseDate: info.releaseDate };
}

function run(args: string[]): { status: number | null; stdout: string; stderr: string } {
  const result = spawnSync(process.execPath, [script, ...args], { encoding: "utf8", windowsHide: true, cwd: repo });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

describe("latest.yml", () => {
  it("is reproduced byte for byte from the real v0.5.1 file's own fields", () => {
    const fields = fieldsOf(real);
    expect(fields.version).toBe("0.5.1");
    expect(renderLatestYml(fields)).toBe(real);
  });

  it("has the shape electron-builder writes: version, one file entry, path, sha512 and a quoted releaseDate, and no block map", () => {
    const text = renderLatestYml({ version: "0.6.0", installerName: "Tjommie-Setup-0.6.0.exe", sha512: "AAAA==", size: 123, releaseDate: "2026-12-01T10:00:00.000Z" });
    expect(text.split("\n")).toEqual([
      "version: 0.6.0",
      "files:",
      "  - url: Tjommie-Setup-0.6.0.exe",
      "    sha512: AAAA==",
      "    size: 123",
      "path: Tjommie-Setup-0.6.0.exe",
      "sha512: AAAA==",
      "releaseDate: '2026-12-01T10:00:00.000Z'",
      "",
    ]);
    expect(text).not.toMatch(/blockMap/i);
    expect(text).not.toContain("\r");
  });

  // R1 reads this with electron-updater's own parser (as the update-test guards do), so what it makes of ours is what R1 would see.
  it("is read by electron-updater's parser as it reads the real one", () => {
    const fields = fieldsOf(real);
    const generated = renderLatestYml({ ...fields, version: "0.6.0", installerName: "Tjommie-Setup-0.6.0.exe" });
    const url = new URL("https://example.com/latest.yml");
    const info = parseUpdateInfo(generated, "latest.yml", url);
    const reference = parseUpdateInfo(real, "latest.yml", url);
    expect(info.version).toBe("0.6.0");
    expect(info.path).toBe("Tjommie-Setup-0.6.0.exe");
    expect(info.sha512).toBe(reference.sha512);
    expect(info.files).toHaveLength(1);
    expect(info.files[0]).toMatchObject({ url: "Tjommie-Setup-0.6.0.exe", sha512: reference.files[0]?.sha512, size: reference.files[0]?.size });
    expect(Object.keys(info).sort()).toEqual(Object.keys(reference).sort());
  });

  it("is written by the command from an installer's own bytes: its SHA-512 in base64, its size, and the name it was given", () => {
    const dir = temp();
    const installer = join(dir, "Tjommie-Setup-0.6.0.exe");
    const bytes = Buffer.from("not a real installer, only bytes to hash\n".repeat(50));
    writeFileSync(installer, bytes);
    const out = join(dir, "latest.yml");
    const result = run(["--installer", installer, "--version", "0.6.0", "--out", out, "--release-date", "2026-12-01T10:00:00.000Z"]);
    expect(result.status, result.stderr).toBe(0);
    const expected = createHash("sha512").update(bytes).digest("base64");
    const text = readFileSync(out, "utf8");
    expect(text).toBe(renderLatestYml({ version: "0.6.0", installerName: "Tjommie-Setup-0.6.0.exe", sha512: expected, size: bytes.length, releaseDate: "2026-12-01T10:00:00.000Z" }));
    expect(fieldsOf(text).sha512).toBe(expected);
    expect(fieldsOf(text).size).toBe(bytes.length);
  });

  it("takes the release time from the clock when none is given, in the same ISO form", () => {
    const dir = temp();
    const installer = join(dir, "Tjommie-Setup-0.6.0.exe");
    writeFileSync(installer, "x");
    const out = join(dir, "latest.yml");
    expect(run(["--installer", installer, "--version", "0.6.0", "--out", out]).status).toBe(0);
    expect(fieldsOf(readFileSync(out, "utf8")).releaseDate).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
  });

  it("refuses an installer that is not named for its version, a version that is not x.y.z, a missing file, and missing arguments", () => {
    const dir = temp();
    const good = join(dir, "Tjommie-Setup-0.6.0.exe");
    writeFileSync(good, "x");
    const out = join(dir, "latest.yml");
    const wrongName = join(dir, "Setup.exe");
    writeFileSync(wrongName, "x");
    const cases: Array<[string[], RegExp]> = [
      [["--installer", wrongName, "--version", "0.6.0", "--out", out], /must be named Tjommie-Setup-0\.6\.0\.exe/],
      [["--installer", good, "--version", "0.6", "--out", out], /x\.y\.z/],
      [["--installer", good, "--version", "0.6.0-rc1", "--out", out], /x\.y\.z/],
      [["--installer", join(dir, "Tjommie-Setup-0.7.0.exe"), "--version", "0.7.0", "--out", out], /not a file|no such file|ENOENT/i],
      [["--installer", good, "--version", "0.6.0"], /--out/],
      [["--installer", good, "--version", "0.6.0", "--out", out, "--release-date", "yesterday"], /ISO/],
    ];
    for (const [args, message] of cases) {
      const result = run(args);
      expect(result.status, args.join(" ")).not.toBe(0);
      expect(result.stderr).toMatch(message);
    }
  });
});
