import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

// T8.3: the scripts the release workflow runs on the Windows runner to assemble the installer's payload, check the production binaries and
// check the installer against its latest.yml. They only read and copy files, so they are tried here against stub files (never a real binary,
// and never an installer: nothing in this file runs one).

vi.setConfig({ testTimeout: 90_000 });

const repo = resolve(import.meta.dirname, "..");
const installer = join(repo, "installer");
const windows = process.platform === "win32";
const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  return "powershell";
})();

const folders: string[] = [];
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-release-test-"));
  folders.push(folder);
  return folder;
};
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

type Run = { status: number | null; output: string };
function powershell(script: string, args: string[]): Run {
  const run = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(installer, script), ...args], { encoding: "utf8", windowsHide: true });
  return { status: run.status, output: `${run.stdout}${run.stderr}` };
}
const sha256 = (path: string): string => createHash("sha256").update(readFileSync(path)).digest("hex");

const speechNames = ["whisper-cli.exe", "WHISPER-LICENSE.txt", "GGML-LICENSE.txt", "OPENAI-WHISPER-LICENSE.txt", "MODEL-NOTICE.txt", "manifest.json"];
// pdfium's licence texts as fetch-pdfium.ps1 -Licenses leaves them: one flat folder, pdfium's own LICENSE and the 18 texts of its licenses folder.
const pdfiumLicenseNames = ["LICENSE", ...Array.from({ length: 18 }, (_, index) => `third-party-${String(index + 1).padStart(2, "0")}.txt`)];
// The stub inputs: a desktop exe, a daemon exe, the speech bundle, pdfium and pdfium's licence texts.
function inputs(): { root: string; desktop: string; daemon: string; speech: string; pdfium: string; licenses: string } {
  const root = temp();
  const speech = join(root, "speech");
  mkdirSync(speech);
  for (const name of speechNames) writeFileSync(join(speech, name), `stub ${name}`);
  writeFileSync(join(root, "tjommie-desktop.exe"), "desktop stub");
  writeFileSync(join(root, "tjommie-daemon.exe"), "daemon stub");
  writeFileSync(join(root, "pdfium.dll"), "pdfium stub");
  const licenses = join(root, "pdfium-licenses");
  mkdirSync(licenses);
  for (const name of pdfiumLicenseNames) writeFileSync(join(licenses, name), `stub ${name}`);
  return { root, desktop: join(root, "tjommie-desktop.exe"), daemon: join(root, "tjommie-daemon.exe"), speech, pdfium: join(root, "pdfium.dll"), licenses };
}
const walk = (folder: string, base = folder): string[] =>
  readdirSync(folder, { withFileTypes: true }).flatMap((entry) => (entry.isDirectory() ? walk(join(folder, entry.name), base) : [join(folder, entry.name).slice(base.length + 1)])).sort();

describe.skipIf(!windows)("the payload assembly", () => {
  it("lays the files out as R1's facts and the Rust app expect: Tjommie.exe, resources\\daemon\\, resources\\speech\\, pdfium and its licence texts", () => {
    const stub = inputs();
    const out = join(stub.root, "payload");
    const manifest = join(stub.root, "payload-manifest.json");
    const run = powershell("assemble-payload.ps1", ["-OutDir", out, "-Desktop", stub.desktop, "-Daemon", stub.daemon, "-SpeechDir", stub.speech, "-Pdfium", stub.pdfium, "-PdfiumLicenses", stub.licenses, "-Release", "-ManifestPath", manifest]);
    expect(run.status, run.output).toBe(0);
    expect(walk(out)).toEqual(
      ["Tjommie.exe", "resources\\daemon\\tjommie-daemon.exe", "resources\\pdfium.dll", ...speechNames.map((name) => `resources\\speech\\${name}`), ...pdfiumLicenseNames.map((name) => `resources\\pdfium-licenses\\${name}`)].sort(),
    );
    // 8 files before pdfium; the DLL and its 19 licence texts make 28 (the release's payload-manifest.json is checked against this count).
    expect(walk(out)).toHaveLength(28);
    expect(run.output).toContain("Payload: 28 files");
    for (const name of pdfiumLicenseNames) expect(sha256(join(out, "resources", "pdfium-licenses", name)), name).toBe(sha256(join(stub.licenses, name)));
    expect(sha256(join(out, "resources", "pdfium.dll"))).toBe(sha256(stub.pdfium));
    // The desktop binary is renamed to the product's exe, and nothing is altered on the way.
    expect(sha256(join(out, "Tjommie.exe"))).toBe(sha256(stub.desktop));
    expect(sha256(join(out, "resources", "daemon", "tjommie-daemon.exe"))).toBe(sha256(stub.daemon));
    const written = JSON.parse(readFileSync(manifest, "utf8")) as { files: Array<{ path: string; bytes: number; sha256: string }>; pending: string[] };
    expect(written.pending).toEqual([]);
    expect(written.files.map((file) => file.path).sort()).toEqual(walk(out));
    for (const file of written.files) expect(file.sha256, file.path).toBe(sha256(join(out, file.path)));
    // The manifest is not part of the payload: it must not be installed.
    expect(existsSync(join(out, "payload-manifest.json"))).toBe(false);
  });

  // A dry run can be built before pdfium is fetched and says what is missing; a release does not go without the DLL or its licence texts.
  it("records pdfium and its licences as pending in a dry run, and refuses a release that lacks either", () => {
    const stub = inputs();
    const base = ["-Desktop", stub.desktop, "-Daemon", stub.daemon, "-SpeechDir", stub.speech];
    const dry = powershell("assemble-payload.ps1", ["-OutDir", join(stub.root, "dry"), ...base, "-ManifestPath", join(stub.root, "dry.json")]);
    expect(dry.status, dry.output).toBe(0);
    const written = JSON.parse(readFileSync(join(stub.root, "dry.json"), "utf8")) as { pending: string[] };
    expect(written.pending).toEqual(["resources\\pdfium.dll", "resources\\pdfium-licenses"]);
    expect(existsSync(join(stub.root, "dry", "resources", "pdfium.dll"))).toBe(false);
    expect(existsSync(join(stub.root, "dry", "resources", "pdfium-licenses"))).toBe(false);
    const noDll = powershell("assemble-payload.ps1", ["-OutDir", join(stub.root, "rel"), ...base, "-PdfiumLicenses", stub.licenses, "-Release"]);
    expect(noDll.status).not.toBe(0);
    expect(noDll.output).toMatch(/pdfium/i);
    const noLicenses = powershell("assemble-payload.ps1", ["-OutDir", join(stub.root, "rel"), ...base, "-Pdfium", stub.pdfium, "-Release"]);
    expect(noLicenses.status).not.toBe(0);
    expect(noLicenses.output).toContain("-PdfiumLicenses");
    expect(existsSync(join(stub.root, "rel"))).toBe(false);
  });

  it("refuses pdfium's licences without the DLL, an empty or missing licence folder, and one with no LICENSE, and writes nothing for them", () => {
    const stub = inputs();
    const base = ["-Desktop", stub.desktop, "-Daemon", stub.daemon, "-SpeechDir", stub.speech];
    const empty = join(stub.root, "empty-licenses");
    mkdirSync(empty);
    const noMain = join(stub.root, "no-main-license");
    mkdirSync(noMain);
    writeFileSync(join(noMain, "third-party-01.txt"), "x");
    const cases: Array<[string, string[], RegExp]> = [
      // (PowerShell wraps a long error message at a space, so the patterns take any whitespace between words.)
      ["licences without the DLL", ["-PdfiumLicenses", stub.licenses], /without\s+the\s+DLL/],
      ["a missing licence folder", ["-Pdfium", stub.pdfium, "-PdfiumLicenses", join(stub.root, "absent")], /not\s+there/],
      ["an empty licence folder", ["-Pdfium", stub.pdfium, "-PdfiumLicenses", empty], /is\s+empty/],
      ["a licence folder with no LICENSE", ["-Pdfium", stub.pdfium, "-PdfiumLicenses", noMain], /no\s+LICENSE/],
    ];
    for (const [label, args, message] of cases) {
      const out = join(stub.root, `out-${label.replaceAll(" ", "-")}`);
      const run = powershell("assemble-payload.ps1", ["-OutDir", out, ...base, ...args]);
      expect(run.status, label).not.toBe(0);
      expect(run.output, label).toMatch(message);
      expect(existsSync(out), label).toBe(false);
    }
  });

  it("refuses a folder that is not empty, a missing binary, an incomplete speech bundle and any Electron file, and writes nothing for them", () => {
    const stub = inputs();
    const base = ["-Desktop", stub.desktop, "-Daemon", stub.daemon, "-SpeechDir", stub.speech];
    const occupied = join(stub.root, "occupied");
    mkdirSync(occupied);
    writeFileSync(join(occupied, "old.txt"), "x");
    const cases: Array<[string, string[], RegExp]> = [
      ["a folder that already has files", ["-OutDir", occupied, ...base], /not empty/],
      ["a missing desktop binary", ["-OutDir", join(stub.root, "a"), "-Desktop", join(stub.root, "nope.exe"), "-Daemon", stub.daemon, "-SpeechDir", stub.speech], /desktop/i],
      ["a missing daemon", ["-OutDir", join(stub.root, "b"), "-Desktop", stub.desktop, "-Daemon", join(stub.root, "nope.exe"), "-SpeechDir", stub.speech], /daemon/i],
    ];
    for (const [label, args, message] of cases) {
      const run = powershell("assemble-payload.ps1", args);
      expect(run.status, label).not.toBe(0);
      expect(run.output, label).toMatch(message);
    }
    rmSync(join(stub.speech, "manifest.json"));
    const incomplete = powershell("assemble-payload.ps1", ["-OutDir", join(stub.root, "c"), ...base]);
    expect(incomplete.status).not.toBe(0);
    expect(incomplete.output).toContain("manifest.json");
    expect(existsSync(join(stub.root, "c"))).toBe(false);
    writeFileSync(join(stub.speech, "manifest.json"), "stub");
    writeFileSync(join(stub.speech, "app.asar"), "an Electron file");
    const electron = powershell("assemble-payload.ps1", ["-OutDir", join(stub.root, "d"), ...base]);
    expect(electron.status).not.toBe(0);
    expect(electron.output).toMatch(/Electron/);
    expect(existsSync(join(stub.root, "d"))).toBe(false);
  });
});

const FEED = "https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml";
const filler = (text: string): Buffer => Buffer.concat([Buffer.alloc(200, 0), Buffer.from(text), Buffer.alloc(200, 0xff)]);

describe.skipIf(!windows)("the production binary check (the M0 review's N3)", () => {
  const check = (desktop: Buffer, daemon: Buffer): Run => {
    const root = temp();
    writeFileSync(join(root, "d.exe"), desktop);
    writeFileSync(join(root, "n.exe"), daemon);
    return powershell("check-production-binaries.ps1", ["-Desktop", join(root, "d.exe"), "-Daemon", join(root, "n.exe")]);
  };

  it("passes a production desktop binary that carries the update feed and no test driver, and a daemon with no test driver", () => {
    const run = check(filler(`some strings ${FEED} more`), filler("a daemon"));
    expect(run.status, run.output).toBe(0);
    expect(run.output).toMatch(/update feed/i);
  });

  it("fails a desktop binary without the feed (a dev build), one with the test driver's pipe, and a daemon with it, naming which", () => {
    const noFeed = check(filler("no feed here"), filler("a daemon"));
    expect(noFeed.status).not.toBe(0);
    expect(noFeed.output).toContain("does not contain the update feed");
    const driver = check(filler(`${FEED} \\\\.\\pipe\\tjommie-ui-1234`), filler("a daemon"));
    expect(driver.status).not.toBe(0);
    expect(driver.output).toContain("test driver");
    const daemonDriver = check(filler(FEED), filler("tjommie-ui-"));
    expect(daemonDriver.status).not.toBe(0);
    expect(daemonDriver.output).toMatch(/daemon[\s\S]*test driver/);
  });

  it("fails every check that fails, not only the first, and a missing file", () => {
    const both = check(filler("tjommie-ui-"), filler("tjommie-ui-"));
    expect(both.status).not.toBe(0);
    expect(both.output).toContain("does not contain the update feed");
    expect((both.output.match(/test driver/g) ?? []).length).toBeGreaterThanOrEqual(2);
    const missing = powershell("check-production-binaries.ps1", ["-Desktop", join(temp(), "none.exe"), "-Daemon", join(temp(), "none2.exe")]);
    expect(missing.status).not.toBe(0);
  });

  // The provider-fixtures review's F2: a daemon built with the fixtures swaps the real claude and codex for scripted ones, and must never ship.
  // Its marker is a constant that only that build has (provider_fixtures.rs: BUILD_MARKER).
  const MARKER = "TJOMMIE-PROVIDER-FIXTURES-BUILD-7f3a9c41";

  it("fails a daemon that carries the provider-fixtures marker, naming it, alongside any other problem", () => {
    const fixtures = check(filler(FEED), filler(`a daemon ${MARKER} with scripted providers`));
    expect(fixtures.status).not.toBe(0);
    expect(fixtures.output).toContain(MARKER);
    expect(fixtures.output).toMatch(/daemon[\s\S]*provider-fixtures/);
    const everything = check(filler("a dev build"), filler(`tjommie-ui-${MARKER}`));
    expect(everything.status).not.toBe(0);
    for (const reported of ["does not contain the update feed", "test driver", MARKER]) expect(everything.output, reported).toContain(reported);
    // The marker is looked for in the daemon, wherever it sits among other bytes: first, last, and in a file that is nothing else.
    for (const daemon of [Buffer.from(MARKER), Buffer.concat([Buffer.from(MARKER), filler("x")]), Buffer.concat([filler("x"), Buffer.from(MARKER)])]) {
      expect(check(filler(FEED), daemon).status, daemon.length.toString()).not.toBe(0);
    }
  });

  describe("shows that its search can find before it trusts an absence", () => {
    const run = (script: string, folder: string, scratch: string): Run => {
      const stubs = temp();
      writeFileSync(join(stubs, "d.exe"), filler(FEED));
      writeFileSync(join(stubs, "n.exe"), filler("a daemon"));
      const result = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script, "-Desktop", join(stubs, "d.exe"), "-Daemon", join(stubs, "n.exe")], {
        encoding: "utf8",
        windowsHide: true,
        env: { ...process.env, RUNNER_TEMP: scratch },
        cwd: folder,
      });
      return { status: result.status, output: `${result.stdout}${result.stderr}` };
    };

    it("plants a file of its own in the job's temp folder, finds every string in it, and takes it away again", () => {
      const scratch = temp();
      const result = run(join(installer, "check-production-binaries.ps1"), scratch, scratch);
      expect(result.status, result.output).toBe(0);
      expect(readdirSync(scratch), "no planted file is left behind").toEqual([]);
    });

    it("stops, and says nothing was checked, when the search finds nothing in what it planted", () => {
      // A copy of the script whose reader returns nothing, so every absence check would pass: the self-test has to stop it before any of them.
      const original = readFileSync(join(installer, "check-production-binaries.ps1"), "utf8");
      const broken = original.replace(/return \[Text\.Encoding\]::GetEncoding\(28591\)[^\r\n]*/, "return ''");
      expect(broken, "the reader was found, and broken").not.toBe(original);
      const folder = temp();
      writeFileSync(join(folder, "check-production-binaries.ps1"), broken);
      const scratch = temp();
      const result = run(join(folder, "check-production-binaries.ps1"), folder, scratch);
      expect(result.status).not.toBe(0);
      expect(result.output).toContain("could not find a string planted");
      expect(result.output).toContain("Nothing was checked");
      expect(readdirSync(scratch), "the planted file is removed even when it fails").toEqual([]);
    });

    it("plants every string it looks for, so the search for each is shown, not only one of them", () => {
      const text = readFileSync(join(installer, "check-production-binaries.ps1"), "utf8");
      expect(text).toContain("Assert-SearchFinds @($feed, $driver, $fixturesMarker)");
      // And it runs before any real binary is read.
      expect(text.indexOf("Assert-SearchFinds @(")).toBeLessThan(text.indexOf("Read-Binary $Desktop"));
    });
  });
});

describe.skipIf(!windows)("the release manifest", () => {
  it("records what was built, from what, with what, and every file with its hash, and says it is a dry run", () => {
    const stub = inputs();
    const out = join(stub.root, "payload");
    const payloadManifest = join(stub.root, "payload-manifest.json");
    expect(powershell("assemble-payload.ps1", ["-OutDir", out, "-Desktop", stub.desktop, "-Daemon", stub.daemon, "-SpeechDir", stub.speech, "-ManifestPath", payloadManifest]).status).toBe(0);
    const setup = join(stub.root, "Tjommie-Setup-0.6.0.exe");
    writeFileSync(setup, "stub installer bytes");
    const yml = join(stub.root, "latest.yml");
    expect(spawnSync(process.execPath, [join(repo, "scripts", "make-latest-yml.ts"), "--installer", setup, "--version", "0.6.0", "--out", yml], { windowsHide: true }).status).toBe(0);
    const manifest = join(stub.root, "release-manifest.json");
    // What check-tool-hashes.ps1 writes for NSIS (its -Observed file), which the manifest carries so it says which compiler built the installer.
    const toolHashes = join(stub.root, "tool-hashes-nsis.json");
    writeFileSync(toolHashes, JSON.stringify({ tool: "nsis", version: "3.11", status: "verified", fileCount: 1, files: { "makensis.exe": "ab".repeat(32) } }));
    const run = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(installer, "write-release-manifest.ps1"),
      "-Version", "0.6.0", "-Installer", setup, "-LatestYml", yml, "-PayloadManifest", payloadManifest, "-Desktop", stub.desktop, "-Daemon", stub.daemon,
      "-Out", manifest, "-DryRun", "true", "-Rustc", "rustc 1.97.1 (stub)", "-Nsis", "v3.11", "-VersionCheck", "warning: the desktop crate is 0.5.1, but package.json is 0.6.0", "-ToolHashes", toolHashes],
    { encoding: "utf8", windowsHide: true, env: { ...process.env, GITHUB_SHA: "a".repeat(40), GITHUB_REF: "refs/heads/release-dryrun-1", GITHUB_EVENT_NAME: "workflow_dispatch", GITHUB_RUN_ID: "42", ImageVersion: "20261103.298.1" } });
    expect(run.status, `${run.stdout}${run.stderr}`).toBe(0);
    const written = JSON.parse(readFileSync(manifest, "utf8")) as {
      schema: number; dryRun: boolean; published: boolean; version: string; commit: string; ref: string; event: string; runId: string;
      build: { identity: string; profile: string; locked: boolean; rustc: string; nsis: string };
      installer: { name: string; bytes: number; sha256: string; sha512: string }; latestYml: { name: string; sha256: string; content: string };
      binaries: { desktop: { sha256: string }; daemon: { sha256: string } }; payload: { files: Array<{ path: string }>; pending: string[] }; checks: { crateVersions: string };
      tools: { nsis: { status: string; version: string; files: Record<string, string> } };
    };
    expect(written).toMatchObject({ schema: 1, dryRun: true, published: false, version: "0.6.0", commit: "a".repeat(40), ref: "refs/heads/release-dryrun-1", event: "workflow_dispatch", runId: "42" });
    expect(written.build).toMatchObject({ identity: "production", profile: "release", locked: true, rustc: "rustc 1.97.1 (stub)", nsis: "v3.11" });
    expect(written.installer).toMatchObject({ name: "Tjommie-Setup-0.6.0.exe", bytes: "stub installer bytes".length, sha256: sha256(setup) });
    expect(written.installer.sha512).toBe(createHash("sha512").update(readFileSync(setup)).digest("base64"));
    expect(written.latestYml).toMatchObject({ name: "latest.yml", sha256: sha256(yml), content: readFileSync(yml, "utf8") });
    expect(written.binaries.desktop.sha256).toBe(sha256(stub.desktop));
    expect(written.binaries.daemon.sha256).toBe(sha256(stub.daemon));
    expect(written.payload.files.map((file) => file.path)).toContain("resources\\daemon\\tjommie-daemon.exe");
    expect(written.payload.pending).toEqual(["resources\\pdfium.dll", "resources\\pdfium-licenses"]);
    expect(written.checks.crateVersions).toMatch(/^warning: /);
    expect(written.tools.nsis).toMatchObject({ status: "verified", version: "3.11", files: { "makensis.exe": "ab".repeat(32) } });
    // Nothing secret goes in it, and no token or release URL.
    expect(readFileSync(manifest, "utf8")).not.toMatch(/token|secret|password|releases\/download/i);
  });
});

// A stand-in installer: any bytes, with the UTF-16 text an NSIS version resource holds, which is what the Rust updater reads AppId from.
const wide = (text: string): Buffer => Buffer.from(text, "utf16le");
const appIdBytes = (appId: string): Buffer => Buffer.concat([Buffer.from("stub installer"), wide("AppId"), Buffer.alloc(8), wide(appId), Buffer.alloc(8)]);

describe.skipIf(!windows)("the installer and latest.yml check", () => {
  const node = process.execPath;
  const setup = (bytes: Buffer, version = "0.6.0"): { dir: string; installer: string; yml: string } => {
    const dir = temp();
    const installerPath = join(dir, `Tjommie-Setup-${version}.exe`);
    writeFileSync(installerPath, bytes);
    const yml = join(dir, "latest.yml");
    const made = spawnSync(node, [join(repo, "scripts", "make-latest-yml.ts"), "--installer", installerPath, "--version", version, "--out", yml], { encoding: "utf8", windowsHide: true });
    if (made.status !== 0) throw new Error(made.stderr);
    return { dir, installer: installerPath, yml };
  };
  const verify = (s: { installer: string; yml: string }, version = "0.6.0", appId = "com.tsnyders.tjommie"): Run =>
    powershell("check-release-installer.ps1", ["-Installer", s.installer, "-LatestYml", s.yml, "-Version", version, "-AppId", appId]);

  it("passes an installer whose latest.yml names it, hashes it and sizes it, and whose version resource carries the app id", () => {
    const run = verify(setup(appIdBytes("com.tsnyders.tjommie")));
    expect(run.status, run.output).toBe(0);
  });

  it("fails a latest.yml that does not match the installer: another SHA-512, another size, another version, another name", () => {
    const s = setup(appIdBytes("com.tsnyders.tjommie"));
    const yml = readFileSync(s.yml, "utf8");
    const cases: Array<[string, string, RegExp]> = [
      ["sha512", yml.replaceAll(/sha512: .*/g, "sha512: AAAAAA=="), /SHA-512/],
      ["size", yml.replace(/size: \d+/, "size: 1"), /size/],
      ["version", yml.replace("version: 0.6.0", "version: 0.6.1"), /version/],
      ["path", yml.replace(/path: .*/, "path: Other.exe"), /path/],
      ["url", yml.replace(/url: .*/, "url: Other.exe"), /url/],
    ];
    for (const [label, text, message] of cases) {
      writeFileSync(s.yml, text);
      const run = verify(s);
      expect(run.status, label).not.toBe(0);
      expect(run.output, label).toMatch(message);
    }
  });

  it("fails a block map beside the installer (none is published), an installer without the app id, and one with another app's id", () => {
    const withMap = setup(appIdBytes("com.tsnyders.tjommie"));
    writeFileSync(`${withMap.installer}.blockmap`, "x");
    const mapRun = verify(withMap);
    expect(mapRun.status).not.toBe(0);
    expect(mapRun.output).toMatch(/block ?map/i);
    const noId = verify(setup(Buffer.from("an installer with no version resource")));
    expect(noId.status).not.toBe(0);
    expect(noId.output).toContain("AppId");
    const other = verify(setup(appIdBytes("com.tsnyders.tjommie-rs")));
    expect(other.status).not.toBe(0);
    // com.tsnyders.tjommie is a prefix of com.tsnyders.tjommie-rs: the check must see the whole value, not a prefix.
    const prefix = verify(setup(appIdBytes("com.tsnyders.tjommie-rs")), "0.6.0", "com.tsnyders.tjommie");
    expect(prefix.status, prefix.output).not.toBe(0);
  });

  it("fails an installer that is not named for its version", () => {
    const s = setup(appIdBytes("com.tsnyders.tjommie"));
    const run = verify(s, "0.7.0");
    expect(run.status).not.toBe(0);
    expect(run.output).toMatch(/Tjommie-Setup-0\.7\.0\.exe/);
  });
});
