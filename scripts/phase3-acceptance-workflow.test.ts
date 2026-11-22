import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { codeOf, installer, repo } from "./ps-support.ts";

// T8.4: the jobs of .github/workflows/desktop-acceptance.yml that install the real binaries over R1, and the scripts they run. Nothing here runs a
// job or an installer; it holds the shape that makes the run mean something: what is built under which identity, what runs on a fresh runner behind
// the guard, what is gated, and the paths the scripts and the installer script have to agree on.

type Step = { name?: string; uses?: string; run?: string; if?: string | boolean; env?: Record<string, string>; "working-directory"?: string; with?: Record<string, string | number | boolean> };
type Job = { needs?: string | string[]; if?: string | boolean; steps?: Step[]; "timeout-minutes"?: number };

const file = join(repo, ".github", "workflows", "desktop-acceptance.yml");
const parsed = spawnSync(process.execPath, [join(import.meta.dirname, "parse-yaml.mjs"), file], { encoding: "utf8", windowsHide: true });
if (parsed.status !== 0) throw new Error(`${file} is not valid YAML: ${parsed.stderr}`);
const workflow = JSON.parse(parsed.stdout) as { env?: Record<string, string>; jobs: Record<string, Job> };
const jobs = workflow.jobs;
const steps = (name: string): Step[] => jobs[name]?.steps ?? [];
const text = (name: string): string => steps(name).map((step) => `${step.name ?? ""}\n${step.run ?? ""}\n${step.uses ?? ""}\n${JSON.stringify(step.env ?? {})}`).join("\n---\n");
const index = (name: string, pattern: RegExp): number => steps(name).findIndex((step) => pattern.test(`${step.name ?? ""}\n${step.run ?? ""}\n${step.uses ?? ""}`));
const identities = (JSON.parse(readFileSync(join(installer, "identities.json"), "utf8")) as { identities: Record<string, { folder: string; product: string; exe: string; appId: string; guid: string }> }).identities;

describe("the T8.4 jobs", () => {
  it("are built from one build job, and the three that install run on fresh runners of their own", () => {
    for (const name of ["update-path", "production-over-r1", "e2e"]) expect(jobs[name]?.needs, name).toBe("acceptance-build");
    expect(jobs["acceptance-build"].needs).toBeUndefined();
    // The jobs that install something start with the guard and the fresh-runner check, before anything they install or run.
    for (const name of ["update-path", "production-over-r1"]) {
      const guard = index(name, /Assert-HostedRunner/);
      expect(guard, name).toBeGreaterThan(-1);
      expect(text(name).indexOf("Assert-FreshRunner"), name).toBeGreaterThan(text(name).indexOf("Assert-HostedRunner"));
      // (Only the production job installs the published R1; the update test builds its own.)
      for (const later of [/download-artifact/, /accept-/, /npm ci/, ...(name === "production-over-r1" ? [/install-latest-r1/] : [])]) expect(index(name, later), `${name}: ${later}`).toBeGreaterThan(guard);
    }
  });

  it("build production first and the update-test identity last, never with the test driver, and the production build is the one the production payload gets", () => {
    const build = steps("acceptance-build");
    const cargo = build.filter((step) => /cargo build/.test(step.run ?? ""));
    expect(cargo.map((step) => step.env?.TJOMMIE_IDENTITY)).toEqual(["production", "updatetest"]);
    for (const step of cargo) {
      expect(step.run, "the same invocation as the release workflow's").toBe("cargo build --release --locked -p daemon -p desktop");
      expect(step["working-directory"]).toBe("tjommie-rs");
    }
    expect(text("acceptance-build")).not.toMatch(/test-driver|provider-fixtures/);
    // The production binaries are copied out before the update-test build overwrites target\release, and the production payload is made from the copy.
    expect(index("acceptance-build", /Keep the production binaries/)).toBeGreaterThan(index("acceptance-build", /Build the production/));
    expect(index("acceptance-build", /Keep the production binaries/)).toBeLessThan(index("acceptance-build", /Build the update-test/));
    const assemble = build.find((step) => /assemble-payload\.ps1/.test(step.run ?? ""))?.run ?? "";
    const [production, updatetest] = assemble.split("\n").filter((line) => line.includes("assemble-payload.ps1"));
    expect(production).toContain("-Desktop \"$env:RUNNER_TEMP\\bin-production\\tjommie-desktop.exe\"");
    expect(production).not.toContain("-ExeName");
    expect(updatetest).toContain("-Desktop tjommie-rs\\target\\release\\tjommie-desktop.exe");
    expect(updatetest).toContain("-ExeName 'Tjommie Update Test.exe'");
    // R1's update-test package is built after the update-test daemon, and only from the isolated identity.
    expect(index("acceptance-build", /Package R1's update-test build/)).toBeGreaterThan(index("acceptance-build", /Build the update-test/));
    const r1 = build.find((step) => /Package R1's update-test build/.test(step.name ?? ""))?.run ?? "";
    expect(r1).toContain("$env:TJOMMIE_PACKAGE_APP_ID = 'com.tsnyders.tjommie.updatetest'");
    expect(r1).toContain("$env:TJOMMIE_PACKAGE_PRODUCT = 'Tjommie Update Test'");
    expect(r1).toContain("tjommie-updatetest");
    expect(identities.updatetest.appId).toBe("com.tsnyders.tjommie.updatetest");
    expect(identities.updatetest.product).toBe("Tjommie Update Test");
  });

  // Run 1: "Update test packaging requires a disposable data root". The guard compares the folder's parent with Node's os.tmpdir(), spelled as TEMP is
  // set, and a hosted runner's TEMP is an 8.3 alias that .NET spells in its long form. R1's own NSIS guard and the Phase 3 installer compare spellings
  // too. So the jobs that build and install the update-test app spell TEMP and TMP in their long form first, and everything after uses that.
  it("spell the temp folder in its long form before anything that builds or installs the update-test app, and the scripts read it from TEMP", () => {
    for (const name of ["acceptance-build", "update-path"]) {
      const step = steps(name).find((candidate) => /Spell the temp folder in its long form/.test(candidate.name ?? ""));
      expect(step, name).toBeDefined();
      expect(step?.run).toContain("(Get-Item -LiteralPath $env:TEMP -Force).FullName");
      expect(step?.run).toContain('"TEMP=$long", "TMP=$long" | Out-File -FilePath $env:GITHUB_ENV -Append');
      const at = index(name, /Spell the temp folder/);
      for (const later of [/npm ci/, /build-speech-runtime|cargo build|accept-|download-artifact/]) expect(index(name, later), `${name}: ${later}`).toBeGreaterThan(at);
    }
    // The package step builds the test root from TEMP, not from .NET's idea of the temp folder, and the script that installs there does too.
    const build = steps("acceptance-build").find((step) => /Package R1's update-test build/.test(step.name ?? ""))?.run ?? "";
    expect(build).toContain("$testRoot = Join-Path ($env:TEMP.TrimEnd('\\')) 'tjommie-updatetest'");
    expect(build).not.toContain("GetTempPath");
    const update = codeOf(join(installer, "accept-update-path.ps1"));
    expect(update).toContain("$temp = $env:TEMP.TrimEnd('\\')");
    expect(update).not.toContain("GetTempPath");
  });

  it("make R1's update-test build older than the update, so its updater takes it", () => {
    const [phase3, r1] = [workflow.env?.PHASE3_VERSION ?? "", workflow.env?.R1_TEST_VERSION ?? ""];
    expect(phase3).toMatch(/^\d+\.\d+\.\d+$/);
    expect(r1).toMatch(/^\d+\.\d+\.\d+-[a-z]+$/);
    const core = (version: string): number[] => version.split("-")[0].split(".").map(Number);
    const [a, b] = [core(phase3), core(r1)];
    // Semantic versioning: a prerelease is below its own x.y.z, so 0.5.1-test is below 0.5.1 and below 0.6.0.
    expect(a[0] > b[0] || (a[0] === b[0] && a[1] > b[1]) || (a[0] === b[0] && a[1] === b[1] && a[2] > b[2])).toBe(true);
  });

  it("run each scenario script after what it needs, and upload the evidence whatever happens", () => {
    const update = steps("update-path");
    expect(update.some((step) => /accept-update-path\.ps1 -InstallersDir "\$env:RUNNER_TEMP\\acceptance-build"/.test(step.run ?? ""))).toBe(true);
    const production = steps("production-over-r1");
    expect(index("production-over-r1", /install-latest-r1\.ps1/)).toBeLessThan(index("production-over-r1", /accept-production-over-r1\.ps1/));
    // R1's baseline is the 0.5.x release with the export (T1.6), unless a run asks for another tag.
    expect(production.find((step) => /install-latest-r1/.test(step.run ?? ""))?.env?.R1_TAG).toBe("${{ inputs.r1_tag || 'v0.5.1' }}");
    for (const [name, artifact] of [["update-path", "acceptance-update-path"], ["production-over-r1", "acceptance-production-over-r1"]] as const) {
      const upload = steps(name).filter((step) => step.uses?.startsWith("actions/upload-artifact@"));
      expect(upload, name).toHaveLength(1);
      expect(upload[0].if).toBe("${{ always() }}");
      expect(upload[0].with).toMatchObject({ name: artifact, "if-no-files-found": "warn" });
      expect(String(upload[0].with?.path)).toContain("installer-acceptance");
    }
    // Every action is pinned to a commit.
    for (const name of Object.keys(jobs)) for (const step of steps(name)) if (step.uses) expect(step.uses, `${name}: ${step.uses}`).toMatch(/@[0-9a-f]{40}\b/);
  });

  it("keep the e2e job off until the tests can be pointed at a packaged build, and then run only a non-production identity with the test driver", () => {
    expect(jobs.e2e.if).toBe("${{ false }}");
    const body = text("e2e");
    expect(body).toContain("TJOMMIE_E2E_EXE");
    expect(body).toContain("--features desktop/test-driver");
    // Every step that sets an identity sets the e2e one, and at least the build and the test run do.
    const identitySteps = steps("e2e").filter((step) => step.env?.TJOMMIE_IDENTITY !== undefined);
    expect(identitySteps.length).toBeGreaterThanOrEqual(2);
    for (const step of identitySteps) expect(step.env?.TJOMMIE_IDENTITY, step.name).toBe("e2e");
    expect(body).not.toMatch(/"TJOMMIE_IDENTITY":"(production|updatetest|dev)"/);
    // Nothing else in the file builds the test driver: the production and update-test builds have none.
    for (const name of Object.keys(jobs).filter((job) => job !== "e2e")) expect(text(name), name).not.toMatch(/test-driver/);
  });

  it("have each job install only under its own identity: the update test never reaches %LOCALAPPDATA%\\Tjommie or %APPDATA%\\Tjommie", () => {
    const update = codeOf(join(installer, "accept-update-path.ps1"));
    expect(update).not.toMatch(/\$env:LOCALAPPDATA|\$env:APPDATA|Programs\\tjommie\b/i);
    expect(update).toContain("$test = $table.identities.updatetest");
    // The production job acts only on the production identity's own folders.
    const production = codeOf(join(installer, "accept-production-over-r1.ps1"));
    expect(production).toContain("$prod = $table.identities.production");
    expect(production).not.toMatch(/identities\.(dev|updatetest)/);
  });
});

describe("the update-path script and the installer script agree", () => {
  const nsi = readFileSync(join(installer, "tjommie.nsi"), "utf8");
  const update = codeOf(join(installer, "accept-update-path.ps1"));

  it("on where the update-test app installs: two levels under %TEMP%, named for the product, which is the installer's own default", () => {
    // The Phase 3 update-test installer installs to $TEMP\<folder>\<product> unless it is told otherwise, and R1's update test installs to the same
    // place, so the update runs over it. The script builds that path from the identity table, not from a second copy of the names.
    expect(nsi).toMatch(/InstallDir "\$TEMP\\\$\{FOLDER_NAME\}\\\$\{PRODUCT_NAME\}"/);
    expect(identities.updatetest.folder).toBe("tjommie-updatetest");
    expect(update).toContain("$testRoot = Join-Path $temp 'tjommie-updatetest'");
    expect(update).toContain("$installDir = Join-Path $testRoot $test.product");
    // The workflow's R1 package is built in the same folder.
    expect(text("acceptance-build")).toContain("'tjommie-updatetest'");
  });

  it("on the order of what it asserts: R1 installed and seeded, the update, then the new app, the uninstall entry, the shortcut, the data and the notice", () => {
    const labels = [...update.matchAll(/Invoke-Step '(U\d) /g)].map((match) => match[1]);
    expect(labels).toEqual(["U0", "U1", "U2", "U3", "U4", "U5", "U6", "U7", "U8", "U9"]);
    expect(update).toMatch(/--expect-version/);
    expect(update).toMatch(/-match '--updated'/);
    expect(update).toContain("'routine_failure_notifications'");
    // The notice is judged by the new app's acknowledgement, which happens only after the OS has taken the toast.
    expect(update).toMatch(/'notice', '--root', \$dataRoot/);
    expect(update.indexOf("Invoke-Step 'U3 ")).toBeGreaterThan(update.indexOf("Invoke-Step 'U2 "));
    expect(update.indexOf("Invoke-Step 'U7 ")).toBeGreaterThan(update.indexOf("Invoke-Step 'U4 "));
  });

  it("with the production script on the order of install, seed, export, takeover, start at login, carry-over and rollback", () => {
    const production = codeOf(join(installer, "accept-production-over-r1.ps1"));
    const labels = [...production.matchAll(/Invoke-Step '(P\d) /g)].map((match) => match[1]);
    expect(labels).toEqual(["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7"]);
    expect(production).toContain("'/S --updated'");
    expect(production).toMatch(/Invoke-Setup \(\[string\]\$r1\.installerPath\) '\/S'/);
    expect(production).toContain("Start-RunValue $run");
    expect(production).toContain("Test-CarryOver $localRoot $fixture 'P5'");
    // The baseline is taken with R1 stopped and before the new installer runs.
    expect(production.indexOf("'snapshot'")).toBeLessThan(production.indexOf("Invoke-Setup $installer"));
    // The rollback checks that no migration ran, against the same baseline as the takeover.
    const rollback = production.slice(production.indexOf("Invoke-Step 'P6 "));
    expect(rollback).toContain("Test-DataRootAsBaseline $localRoot $baseline");
    expect(rollback).toContain("Test-InstalledAsR1Did");
    expect(existsSync(join(repo, "tjommie-rs", "crates", "desktop-host", "tests", "fixtures", "desktop-ui-export.json"))).toBe(true);
  });
});
