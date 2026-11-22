import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

// installer/write-apk-manifest.ps1: what a dry run of the release workflow uploads in place of the Android APK. The repository is public and the
// APK is signed with the release key, so the dry run keeps only facts about it. Here the Android build tools are two stand-in .cmd files and the
// "APK" is a few bytes: nothing real is signed, installed or run.

vi.setConfig({ testTimeout: 90_000 });

const repo = resolve(import.meta.dirname, "..");
const script = join(repo, "installer", "write-apk-manifest.ps1");
const windows = process.platform === "win32";
const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  return "powershell";
})();

const folders: string[] = [];
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-apk-manifest-test-"));
  folders.push(folder);
  return folder;
};
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

const SIGNER = "ab".repeat(32);
const OTHER = "cd".repeat(32);

type Stub = { root: string; apk: string; out: string; apksigner: string; aapt: string };
function stub(options: { signers?: string[]; signerExit?: number; badging?: string[]; aaptExit?: number; apkName?: string } = {}): Stub {
  const root = temp();
  const tools = join(root, "tools");
  mkdirSync(tools);
  const apk = join(root, options.apkName ?? "Tjommie-Android-0.6.0.apk");
  writeFileSync(apk, "not a real apk, only bytes to hash\n");
  const signerLines = (options.signers ?? [SIGNER]).map((digest, index) => `echo Signer #${index + 1} certificate SHA-256 digest: ${digest}`);
  writeFileSync(join(tools, "apksigner.cmd"), ["@echo off", "echo Verified using v2 scheme: true", ...signerLines, `exit /b ${options.signerExit ?? 0}`, ""].join("\r\n"));
  const badging = options.badging ?? ["package: name='dev.tjommie.app' versionCode='11' versionName='0.6.0' platformBuildVersionName='16'", "sdkVersion:'24'"];
  writeFileSync(join(tools, "aapt.cmd"), ["@echo off", ...badging.map((line) => `echo ${line}`), `exit /b ${options.aaptExit ?? 0}`, ""].join("\r\n"));
  return { root, apk, out: join(root, "dist", "apk-manifest.json"), apksigner: join(tools, "apksigner.cmd"), aapt: join(tools, "aapt.cmd") };
}

function run(s: Stub, extra: string[] = []): { status: number | null; output: string } {
  const result = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script, "-Apk", s.apk, "-Out", s.out, "-Apksigner", s.apksigner, "-Aapt", s.aapt, ...extra], { encoding: "utf8", windowsHide: true });
  return { status: result.status, output: `${result.stdout}${result.stderr}` };
}
const manifest = (s: Stub): Record<string, unknown> => JSON.parse(readFileSync(s.out, "utf8").replace(/^﻿/, "")) as Record<string, unknown>;

describe.skipIf(!windows)("the APK manifest a dry run uploads in place of the APK", () => {
  it("records the file's name, size and SHA-256, the verified signer, and the package and version read from the APK", () => {
    const s = stub();
    const result = run(s, ["-ExpectedSigner", SIGNER]);
    expect(result.status, result.output).toBe(0);
    const bytes = readFileSync(s.apk);
    expect(manifest(s)).toEqual({
      schema: 1,
      file: "Tjommie-Android-0.6.0.apk",
      bytes: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      signerSha256: SIGNER,
      signerMatchesExpected: true,
      package: "dev.tjommie.app",
      versionCode: 11,
      versionName: "0.6.0",
    });
    // The APK itself is never copied beside it.
    expect(existsSync(join(s.root, "dist", "Tjommie-Android-0.6.0.apk"))).toBe(false);
  });

  it("takes the recorded signer with or without colons and in any case, and says when none was recorded", () => {
    const colons = SIGNER.toUpperCase().replace(/(..)(?!$)/g, "$1:");
    const s = stub();
    expect(run(s, ["-ExpectedSigner", colons]).status).toBe(0);
    expect(manifest(s).signerMatchesExpected).toBe(true);
    const t = stub();
    expect(run(t).status).toBe(0);
    expect(manifest(t).signerMatchesExpected).toBe(false);
    expect(manifest(t).signerSha256).toBe(SIGNER);
  });

  it("fails, writing nothing, on another signer, no signer, two signers, a failed verification, and an APK aapt cannot read", () => {
    const cases: Array<[string, Stub, string[], RegExp]> = [
      ["another signer than the recorded one", stub(), ["-ExpectedSigner", OTHER], /does\s+not\s+match/],
      ["a recorded signer that is not a digest", stub(), ["-ExpectedSigner", "not-a-digest"], /not\s+a\s+SHA-256/],
      ["no signer", stub({ signers: [] }), [], /exactly\s+one/],
      ["two signers", stub({ signers: [SIGNER, OTHER] }), [], /exactly\s+one/],
      ["a failed verification", stub({ signerExit: 1 }), [], /verification\s+failed/],
      ["aapt failing", stub({ aaptExit: 1 }), [], /aapt\s+could\s+not/],
      ["no package line", stub({ badging: ["sdkVersion:'24'"] }), [], /did\s+not\s+report/],
    ];
    for (const [label, s, extra, message] of cases) {
      const result = run(s, extra);
      expect(result.status, label).not.toBe(0);
      expect(result.output, label).toMatch(message);
      expect(existsSync(s.out), label).toBe(false);
    }
  });

  it("refuses a missing APK, a file that is not named for a release, and a missing build tool", () => {
    const missing = stub();
    rmSync(missing.apk);
    expect(run(missing).output).toMatch(/APK\s+is\s+missing/);
    const misnamed = stub({ apkName: "app-release.apk" });
    const named = run(misnamed);
    expect(named.status).not.toBe(0);
    expect(named.output).toMatch(/Tjommie-Android-<version>\.apk/);
    const noTool = stub();
    rmSync(noTool.aapt);
    expect(run(noTool).output).toMatch(/build\s+tool\s+is\s+missing/);
    for (const s of [missing, misnamed, noTool]) expect(existsSync(s.out)).toBe(false);
  });

  it("never prints the signer where a log would keep it unmasked", () => {
    const s = stub();
    const result = run(s, ["-ExpectedSigner", SIGNER]);
    expect(result.status, result.output).toBe(0);
    // Every line that holds the digest is a ::add-mask:: command, which the runner turns into a mask; the summary line has none.
    for (const line of result.output.split(/\r?\n/).filter((text) => text.toLowerCase().includes(SIGNER))) expect(line).toMatch(/^::add-mask::/);
  });
});
