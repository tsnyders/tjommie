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
  it("are built from one build job, and the two that install run on fresh runners of their own", () => {
    for (const name of ["update-path", "production-over-r1"]) expect(jobs[name]?.needs, name).toBe("acceptance-build");
    expect(jobs["acceptance-build"].needs).toBeUndefined();
    // The e2e job builds its own package and installs nothing of Tjommie's, so it waits for nothing and runs beside the others.
    expect(jobs.e2e.needs).toBeUndefined();
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

  // T8.4 review n1: the build artifact is a production-identity installer with the real payload on a public repository. Like the release workflow's dry run, it
  // says in the artifact that it is not to be installed; and it is kept a day, since only this workflow's own jobs use it.
  it("label the acceptance-build artifact as not for installing, after everything is in it, and keep it one day", () => {
    const mark = steps("acceptance-build").find((step) => /Mark the build as not for installing/.test(step.name ?? ""));
    expect(mark?.run).toContain("DO NOT INSTALL ANYTHING IN THIS ARTIFACT ON A REAL PC.");
    expect(mark?.run).toContain("acceptance-build\\DO-NOT-INSTALL.txt");
    expect(mark?.run).toContain("Nothing in here is published");
    const upload = steps("acceptance-build").filter((step) => step.uses?.startsWith("actions/upload-artifact@"));
    expect(upload).toHaveLength(1);
    expect(upload[0].with).toMatchObject({ name: "acceptance-build", "retention-days": 1, "if-no-files-found": "error" });
    expect(index("acceptance-build", /Mark the build as not for installing/)).toBeGreaterThan(index("acceptance-build", /Package R1's update-test build/));
    expect(index("acceptance-build", /Mark the build as not for installing/)).toBeLessThan(index("acceptance-build", /upload-artifact/));
  });

  // The fixtures lane's override (tests/common): TJOMMIE_E2E_EXE is the ABSOLUTE path of an existing desktop exe, which every e2e test launches; the
  // fixtures daemon sits beside it as tjommie-daemon.exe; the package is the e2e identity with the test driver, never production.
  it("run the e2e set against a packaged e2e build through TJOMMIE_E2E_EXE, as the override's contract asks, and never a production build", () => {
    expect(jobs.e2e.if, "the job is on").toBeUndefined();
    const build = steps("e2e").find((step) => /Build the e2e desktop app/.test(step.name ?? ""))?.run ?? "";
    // The desktop app is the e2e identity with the test driver; the daemon is the fixtures build, beside it after packaging.
    expect(build).toContain("$env:TJOMMIE_IDENTITY = 'e2e'");
    expect(build).toContain("cargo build --release --locked -p desktop --features desktop/test-driver");
    // Run 3: the daemon's provider-fixtures feature refuses a release build (a compile_error in its lib.rs), so the fixtures daemon is the dev profile's,
    // built as ci.yml builds it, and the package takes it from target\debug. The desktop app is the release one.
    expect(build).toContain("cargo build --locked -p daemon --bin tjommie-daemon --features provider-fixtures");
    expect(build).not.toMatch(/--release[^\n]*-p daemon|-p daemon[^\n]*--release/);
    expect(build.indexOf("$env:TJOMMIE_IDENTITY = 'e2e'")).toBeLessThan(build.indexOf("-p desktop --features desktop/test-driver"));
    const packageStep = steps("e2e").find((step) => /Package the e2e build/.test(step.name ?? ""))?.run ?? "";
    expect(packageStep).toContain("Copy-Item tjommie-rs\\target\\release\\tjommie-desktop.exe -Destination $package");
    expect(packageStep).toContain("Copy-Item tjommie-rs\\target\\debug\\tjommie-daemon.exe -Destination $package");
    expect(readFileSync(join(repo, "tjommie-rs", "crates", "daemon", "src", "lib.rs"), "utf8")).toMatch(/provider-fixtures feature must not be built without debug assertions/);
    // The package is checked before it is tested, and the tests run with the variable set to its absolute path.
    expect(index("e2e", /check-e2e-package\.ps1/)).toBeGreaterThan(index("e2e", /Package the e2e build/));
    expect(index("e2e", /check-e2e-package\.ps1/)).toBeLessThan(index("e2e", /cargo test/));
    expect(steps("e2e").find((step) => /check-e2e-package/.test(step.run ?? ""))?.run).toContain('-Desktop "$env:RUNNER_TEMP\\e2e-package\\tjommie-desktop.exe" -Daemon "$env:RUNNER_TEMP\\e2e-package\\tjommie-daemon.exe"');
    const tests = steps("e2e").find((step) => /cargo test/.test(step.run ?? ""));
    expect(tests?.env).toEqual({ TJOMMIE_IDENTITY: "e2e", TJOMMIE_E2E_EXE: "${{ runner.temp }}\\e2e-package\\tjommie-desktop.exe" });
    expect(tests?.run).toBe("cargo test --workspace --locked --features desktop/test-driver --test 'e2e_*' --no-fail-fast");
    expect(tests?.["working-directory"]).toBe("tjommie-rs");
    // The pdfium the e2e app's PDF viewer needs, as ci.yml gives it.
    expect(text("e2e")).toContain("TJOMMIE_PDFIUM_DLL=$dll");
    // The override this relies on is in the tests, and says what the contract says.
    const common = readFileSync(join(repo, "tjommie-rs", "crates", "desktop", "tests", "common", "mod.rs"), "utf8");
    expect(common).toContain('pub const E2E_EXE_VAR: &str = "TJOMMIE_E2E_EXE"');
    expect(common).toContain("must be an absolute path");
    expect(common).toContain('with_file_name("tjommie-daemon.exe")');
    // No production identity anywhere in the job, and every step that sets one sets the e2e one.
    const identitySteps = steps("e2e").filter((step) => step.env?.TJOMMIE_IDENTITY !== undefined);
    expect(identitySteps.length).toBeGreaterThanOrEqual(1);
    for (const step of identitySteps) expect(step.env?.TJOMMIE_IDENTITY, step.name).toBe("e2e");
    const body = text("e2e");
    expect(body).not.toMatch(/TJOMMIE_IDENTITY\s*=\s*'(production|updatetest|dev)'|"TJOMMIE_IDENTITY":"(production|updatetest|dev)"/);
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

  it("on the new app finding its own daemon: nothing hands it one, and U7 reads the image path and the parent of every daemon running", () => {
    // The product runs the daemon bundled in an installed update-test app's folder (daemon_exe_for), so the update path must not supply one.
    expect(update).not.toMatch(/DAEMON_EXE/);
    const u7 = update.slice(update.indexOf("Invoke-Step 'U7 "), update.indexOf("Invoke-Step 'U8 "));
    expect(u7).toContain("resources\\daemon\\tjommie-daemon.exe");
    // The daemon is found by its exact image path, its parent by its id and image path, and no process is looked up by its image name (a repository rule,
    // r1-facts.test.ts: that is how a real Tjommie would be reached).
    expect(u7).toContain("Get-ProcessesOfExe $daemon");
    expect(u7).not.toMatch(/Win32_Process[^\n]*Name\s*=/i);
    expect(u7).toMatch(/Win32_Process -Filter "ProcessId = \$\(\$_\.ParentProcessId\)"/);
    expect(u7).toMatch(/\$parent\.ExecutablePath -ieq \$newLong/);
    // The harness side: the driver builds the update-test R1's environment without any daemon.
    const driver = readFileSync(join(repo, "scripts", "phase3-acceptance.ts"), "utf8");
    const env = driver.slice(driver.indexOf("export function r1UpdateEnv"), driver.indexOf("export function", driver.indexOf("export function r1UpdateEnv") + 1));
    expect(env).not.toMatch(/TJOMMIE_DAEMON_EXE:/);
  });

  it("on the saved-logins vault: seeded with invented logins in both scenarios, and held to the baseline by every data-root comparison", () => {
    const lib = codeOf(join(installer, "accept-phase3-lib.ps1"));
    expect(lib).toContain("Test-VaultAsBaseline $report.vault $Label");
    for (const name of ["accept-update-path.ps1", "accept-production-over-r1.ps1"]) {
      const script = codeOf(join(installer, name));
      expect(script, name).toContain("data\\logins.sqlite");
      expect(script, name).toContain("Seeded the saved-logins vault");
    }
    // The comparisons that carry it: the update path's U7, and the production takeover and the rollback.
    expect(update).toContain("Test-DataRootAsBaseline $dataRoot $baseline $report 'U7'");
    const production = codeOf(join(installer, "accept-production-over-r1.ps1"));
    expect(production).toContain("'P5'");
    expect(production).toContain("'P6'");
  });

  it("on what U7 and U8 can and cannot show: the daemon is the new app's by when it started, and a notification is an inference", () => {
    // R1's exe has the new exe's name and path, so a daemon is told to be the new app's by starting, under a new-exe process, after R1 had quit (U3).
    const u3 = update.slice(update.indexOf("Invoke-Step 'U3 "), update.indexOf("Invoke-Step 'U4 "));
    expect(u3).toContain("$state['handover'] = Get-Date");
    const u7 = update.slice(update.indexOf("Invoke-Step 'U7 "), update.indexOf("Invoke-Step 'U8 "));
    expect(u7).toContain("$parent.CreationDate -gt $handover");
    expect(u7).toContain("$_.CreationDate -gt $handover");
    const u8 = update.slice(update.indexOf("Invoke-Step 'U8 "), update.indexOf("Invoke-Step 'U9 "));
    expect(u8).toContain("is an inference from the app''s contract");
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
