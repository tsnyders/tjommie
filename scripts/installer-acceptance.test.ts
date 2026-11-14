import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { codeOf, installer, powershell, repo, runBody, temp, windows } from "./ps-support.ts";

// T8.2: the scripts that install, update and uninstall on a GitHub-hosted runner (installer/accept-*.ps1) and the workflow jobs
// that run them. Here they are only parsed, shown to refuse to run anywhere but a hosted runner, and their pure helpers are
// tried on temp folders. Nothing in this file installs, updates or uninstalls anything, or reads a Tjommie location: the helpers
// that read the registry for production's keys are never called here.

vi.setConfig({ testTimeout: 90_000 });

const acceptance = ["accept-fresh.ps1", "accept-over-r1.ps1"];

describe.skipIf(!windows)("the installer acceptance scripts", () => {
  // Each is started with an environment that is not a hosted runner's, and with every argument it needs, so the guard (which
  // comes first) is what stops it; nothing it would act on is in the environment either.
  it.each(acceptance)("%s refuses to run anywhere but a GitHub-hosted runner", (name) => {
    const base = { ...process.env };
    for (const key of ["GITHUB_ACTIONS", "RUNNER_ENVIRONMENT", "GH_REPO", "GH_TOKEN", "R1_TAG", "R1_INSTALL_RESULT", "RUNNER_TEMP", "GITHUB_ENV"]) delete base[key];
    const args = ["-InstallersDir", join(temp(), "none"), ...(name === "accept-over-r1.ps1" ? ["-InstallResult", join(temp(), "none.json")] : [])];
    for (const env of [base, { ...base, GITHUB_ACTIONS: "true" }, { ...base, GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "self-hosted" }, { ...base, RUNNER_ENVIRONMENT: "github-hosted" }]) {
      const run = powershell(join(installer, name), args, env);
      expect(run.status, `${name}: ${run.stdout}${run.stderr}`).not.toBe(0);
      expect(`${run.stdout}${run.stderr}`).toContain("GitHub-hosted runner");
    }
  });

  it("starts with the guard, before it loads its library or does anything", () => {
    for (const name of acceptance) {
      const text = codeOf(join(installer, name));
      const guard = text.indexOf("Assert-HostedRunner");
      expect(guard, name).toBeGreaterThan(-1);
      for (const action of ["accept-lib.ps1", "Start-Process", "Invoke-Setup", "Invoke-Step", "Set-RunValue", "Set-InstallLocation", "Add-Sentinels", "Get-ChildItem", "Invoke-RegisteredUninstall", "Start-Acceptance"]) {
        const first = text.indexOf(action);
        if (first !== -1) expect(guard, `${name}: ${action}`).toBeLessThan(first);
      }
    }
    // The fresh-runner check comes with it where the script installs from nothing.
    const fresh = codeOf(join(installer, "accept-fresh.ps1"));
    expect(fresh.indexOf("Assert-FreshRunner")).toBeGreaterThan(fresh.indexOf("Assert-HostedRunner"));
    expect(fresh.indexOf("Assert-FreshRunner")).toBeLessThan(fresh.indexOf("accept-lib.ps1"));
  });

  it("ends by failing the run if any check failed, after the evidence is written", () => {
    for (const name of acceptance) expect(codeOf(join(installer, name)).trimEnd().endsWith("Complete-Acceptance"), name).toBe(true);
    const lib = codeOf(join(installer, "accept-lib.ps1"));
    expect(lib.indexOf("Save-Acceptance\n    $failed")).toBeGreaterThan(-1);
    expect(lib).toMatch(/function Invoke-Step[\s\S]*catch[\s\S]*Save-Acceptance/);
  });

  it("never plants or removes a value outside the identity under test", () => {
    // Registry writes in the scripts are to the Run value named by an app id, and to HKCU\Software\<GUID>\InstallLocation.
    for (const name of [...acceptance, "accept-lib.ps1"]) {
      const text = codeOf(join(installer, name));
      // Nothing is written under HKLM (reading it, to show it is untouched, goes through Get-RegistryKeyFact) or by provider path.
      expect(text, name).not.toMatch(/HKLM:|\]::LocalMachine\.(CreateSubKey|OpenSubKey\([^)]*\$true)|Remove-Item\s+HKCU|Remove-ItemProperty|Set-ItemProperty|New-ItemProperty/i);
      // And no data root is ever removed: the scripts only add to them and then look.
      expect(text, name).not.toMatch(/Remove-Item[^\n]*\$dataRoots/i);
    }
  });
});

// These scripts run only on a hosted runner, so a misspelled variable or command in a branch nobody has run would cost a whole
// run to find. scripts/ps-lint.ps1 parses them (it runs nothing in them) and reports every variable read but never assigned and
// every command that is neither a cmdlet nor a function defined in the script or the libraries it loads.
describe.skipIf(!windows)("the installer scripts, linted", () => {
  const lint = (folder: string, files: string[]): Record<string, string[]> => {
    const run = powershell(join(repo, "scripts", "ps-lint.ps1"), ["-Installer", folder, "-Files", files.join(",")], { ...process.env });
    if (run.status !== 0) throw new Error(`lint failed (${run.status}): ${run.stderr || run.stdout}`);
    return JSON.parse(run.stdout) as Record<string, string[]>;
  };

  it("finds no variable that is read and never assigned, and no command that is not defined", () => {
    const files = ["accept-fresh.ps1", "accept-over-r1.ps1", "accept-lib.ps1", "build-installer.ps1", "build-acceptance-installers.ps1", "build-stub-payload.ps1", "stop-install-processes.ps1", "set-shortcut-aumid.ps1"];
    const report = lint(installer, files);
    for (const file of files) expect(report[file], file).toEqual([]);
  });

  it("does find a misspelled variable and a misspelled command (so the empty report above means something)", () => {
    const folder = temp();
    for (const file of ["hosted-runner.ps1", "accept-lib.ps1", "r1-facts-lib.ps1", "scoped-processes.ps1"]) writeFileSync(join(folder, file), readFileSync(join(installer, file)));
    writeFileSync(join(folder, "bad.ps1"), "param([string]$Folder)\n$state = @{}\n$state['a'] = $Folderr\nInvoke-Setpu 'x' '/S'\nStart-Acceptance 'x' $Folder\n");
    const problems = lint(folder, ["bad.ps1"])["bad.ps1"] ?? [];
    expect(problems.some((problem) => problem.includes("$Folderr is read but never assigned"))).toBe(true);
    expect(problems.some((problem) => problem.includes("command Invoke-Setpu is not defined"))).toBe(true);
    expect(problems).toHaveLength(2);
  });
});

describe.skipIf(!windows)("the acceptance library's pure helpers", () => {
  // Only the helpers that take a folder: the ones that read production's registry keys are never called in a test.
  it("snapshots a tree, finds a file that is gone, changed or added, and adds sentinels of known size", () => {
    const root = temp();
    const result = runBody("accept-lib.ps1", String.raw`
      $dir = '${join(root, "data")}'
      $made = Add-Sentinels $dir
      $before = Get-TreeSnapshot $dir
      $same = @(Compare-Snapshot $before (Get-TreeSnapshot $dir))
      Remove-Item -LiteralPath (Join-Path $dir 'sentinel\notes.md')
      [IO.File]::WriteAllBytes((Join-Path $dir 'sentinel\acceptance.db'), [byte[]](1,2,3))
      [IO.File]::WriteAllBytes((Join-Path $dir 'new.txt'), [byte[]](9))
      $after = Get-TreeSnapshot $dir
      [ordered]@{
        made       = @($made)
        count      = @($before.Keys).Count
        same       = $same
        differences = @(Compare-Snapshot $before $after)
        withoutAdded = @(Compare-Snapshot $before $after -IgnoreAdded)
        empty      = @((Get-TreeSnapshot '${join(root, "missing")}').Keys).Count
        sizes      = @(Get-ChildItem -LiteralPath $dir -Recurse -File -Filter 'secret.bin' | ForEach-Object { $_.Length })
      } | ConvertTo-Json -Compress
    `) as { made: string[]; count: number; same: string[]; differences: string[]; withoutAdded: string[]; empty: number; sizes: number[] };
    expect(result.made).toHaveLength(3);
    expect(result.count).toBe(3);
    expect(result.same).toEqual([]);
    expect([...result.differences].sort()).toEqual(["added: new.txt", "changed: sentinel\\acceptance.db", "missing: sentinel\\notes.md"]);
    expect([...result.withoutAdded].sort()).toEqual(["changed: sentinel\\acceptance.db", "missing: sentinel\\notes.md"]);
    expect(result.empty).toBe(0);
    expect(result.sizes).toEqual([4096]);
  });

  it("lists the files a stub install holds, and the files under a folder, the same way", () => {
    const root = temp();
    mkdirSync(join(root, "resources", "daemon"), { recursive: true });
    mkdirSync(join(root, "installer-helpers"), { recursive: true });
    for (const file of ["Tjommie.exe", "resources\\daemon\\tjommie-daemon.exe", "Uninstall Tjommie.exe", "installer-helpers\\scoped-processes.ps1", "installer-helpers\\stop-install-processes.ps1"]) writeFileSync(join(root, file), "x");
    const result = runBody("accept-lib.ps1", String.raw`
      [ordered]@{
        expected  = @(Get-ExpectedFiles 'Tjommie' 'Tjommie.exe')
        installed = @(Get-InstalledFiles '${root}')
        none      = @(Get-InstalledFiles '${join(root, "missing")}')
        dev       = @(Get-ExpectedFiles 'Tjommie RS' 'TjommieRS.exe')
      } | ConvertTo-Json -Compress
    `) as { expected: string[]; installed: string[]; none: string[]; dev: string[] };
    expect(result.installed).toEqual(result.expected);
    expect(result.none).toEqual([]);
    expect(result.dev).toContain("Uninstall Tjommie RS.exe");
    expect(result.dev).toContain("TjommieRS.exe");
  });

  it("records checks and notes, saves them as it goes, and fails the run only at the end", () => {
    const out = temp();
    const result = runBody("accept-lib.ps1", String.raw`
      Start-Acceptance 'unit' '${out}'
      Assert-Check $true 'a passing check' | Out-Null
      Add-Note 'a note' 7 | Out-Null
      Invoke-Step 'a step that throws' { throw 'boom' } | Out-Null
      Assert-Check $true 'a check after the failed step' | Out-Null
      $threw = $false
      try { Complete-Acceptance | Out-Null } catch { $threw = $true }
      # Complete-Acceptance writes the file before it fails, so the last check is in it.
      $saved = Get-Content -LiteralPath '${join(out, "installer-acceptance-unit.json")}' -Raw | ConvertFrom-Json
      [ordered]@{ savedPassed = $saved.passed; savedFailed = $saved.failed; checks = @($saved.checks | ForEach-Object { $_.check }); threw = $threw } | ConvertTo-Json -Compress
    `) as { savedPassed: boolean; savedFailed: number; checks: string[]; threw: boolean };
    expect(result.savedPassed).toBe(false);
    expect(result.savedFailed).toBe(1);
    expect(result.checks).toEqual(["a passing check", "a note", "a step that throws stopped: boom", "a check after the failed step"]);
    expect(result.threw).toBe(true);
  });

  // The setup runner is tried on cmd.exe, which installs nothing. A refusal is an exit code that is not zero, so a code that could
  // not be read must be an error, not a value that happens to differ from zero.
  it("runs a program and returns its real exit code, even one that exits at once, sees its children, and gives up on one that hangs", () => {
    const result = runBody("accept-lib.ps1", String.raw`
      $cmd = "$env:SystemRoot\System32\cmd.exe"
      $zero = Invoke-Setup $cmd '/c exit 0'
      $seven = Invoke-Setup $cmd '/c exit 7'
      $watched = Invoke-Setup $cmd '/c ping -n 3 127.0.0.1 >nul' -Watch
      $timedOut = $false
      try { [void](Invoke-Setup $cmd '/c ping -n 6 127.0.0.1 >nul' -TimeoutSeconds 2) } catch { $timedOut = ($_.Exception.Message -match 'did not finish') }
      [ordered]@{ zero = $zero['exitCode']; seven = $seven['exitCode']; sawPing = [bool](@($watched['processes'] | Where-Object { $_['name'] -eq 'PING.EXE' }).Count); timedOut = $timedOut } | ConvertTo-Json -Compress
    `) as { zero: number; seven: number; sawPing: boolean; timedOut: boolean };
    expect(result).toEqual({ zero: 0, seven: 7, sawPing: true, timedOut: true });
  });

  it("removes a registry value when given $null, rather than writing an empty one", () => {
    // These two write under HKCU, so they are not called here; what matters is that their value parameter is untyped. A [string]
    // parameter would turn $null into "" and "remove" would write an empty value, which the checks would then misread.
    const lib = codeOf(join(installer, "accept-lib.ps1"));
    expect(lib).toMatch(/function Set-RunValue\(\[string\]\$Name, \$Data\)/);
    expect(lib).toMatch(/function Set-InstallLocation\(\[string\]\$Guid, \$Location\)/);
    expect(lib).toMatch(/if \(\$null -eq \$Data\) \{ \$key\.DeleteValue/);
    expect(lib).toMatch(/if \(\$null -eq \$Location\) \{ \$key\.DeleteValue/);
  });

  it("starts a stub only when asked and reads the stub log, or nothing, safely", () => {
    const folder = temp();
    const result = runBody("accept-lib.ps1", String.raw`
      $env:STUB_LOG = '${join(folder, "stub.log")}'
      $none = @(Get-StubLog)
      Set-Content -LiteralPath $env:STUB_LOG -Value @('Tjommie.exe 12 --updated', 'tjommie-daemon.exe 13 ')
      $lines = @(Get-StubLog)
      [ordered]@{ none = $none.Count; lines = $lines; alive = (Test-Alive $PID); dead = (Test-Alive 2147483000) } | ConvertTo-Json -Compress
    `) as { none: number; lines: string[]; alive: boolean; dead: boolean };
    expect(result.none).toBe(0);
    expect(result.lines).toEqual(["Tjommie.exe 12 --updated", "tjommie-daemon.exe 13 "]);
    expect(result.alive).toBe(true);
    expect(result.dead).toBe(false);
  });
});

describe("the desktop-acceptance workflow's installer jobs", () => {
  const workflow = readFileSync(join(repo, ".github", "workflows", "desktop-acceptance.yml"), "utf8").replace(/\r\n/g, "\n");
  const job = (name: string): string => {
    const start = workflow.indexOf(`\n  ${name}:`);
    expect(start, name).toBeGreaterThan(-1);
    const next = workflow.slice(start + 1).search(/\n {2}[a-z][\w-]*:\n/);
    return next === -1 ? workflow.slice(start) : workflow.slice(start, start + 1 + next);
  };

  it.each(["installer-fresh", "installer-over-r1"])("%s checks the runner first, installs a pinned NSIS, builds the installers and uploads its evidence whatever happens", (name) => {
    const text = job(name);
    expect(text).toContain("runs-on: windows-latest");
    // The guard and the fresh-runner check are the first thing after the checkout.
    const guard = text.indexOf("Assert-HostedRunner");
    expect(guard).toBeGreaterThan(text.indexOf("actions/checkout"));
    expect(text.indexOf("Assert-FreshRunner")).toBeGreaterThan(guard);
    for (const later of ["choco install", "build-acceptance-installers.ps1", "accept-"]) expect(text.indexOf(later), later).toBeGreaterThan(text.indexOf("Assert-FreshRunner"));
    expect(text).toMatch(/choco install nsis --version=\d+\.\d+(\.\d+)? /);
    expect(text).toMatch(/shell: powershell\s+run: \.\/installer\/build-acceptance-installers\.ps1/); // Windows PowerShell: it compiles the stub
    const upload = text.slice(text.indexOf("actions/upload-artifact"));
    expect(upload).toMatch(/if: \$\{\{ always\(\) \}\}/);
    expect(upload).toMatch(/if-no-files-found: warn/);
    expect(upload).toContain(`installer-acceptance-${name === "installer-fresh" ? "fresh" : "over-r1"}.json`);
  });

  it("runs each scenario script, and installs the real R1 only where the scenario needs it", () => {
    expect(job("installer-fresh")).toContain("./installer/accept-fresh.ps1");
    expect(job("installer-fresh")).not.toContain("install-latest-r1.ps1");
    const over = job("installer-over-r1");
    expect(over).toContain("./installer/install-latest-r1.ps1");
    expect(over).toContain("./installer/accept-over-r1.ps1");
    expect(over.indexOf("install-latest-r1.ps1")).toBeLessThan(over.indexOf("accept-over-r1.ps1"));
  });

  it("names only scripts that exist, and never runs on a pull request", () => {
    for (const script of workflow.match(/installer\/[a-z0-9-]+\.ps1/g) ?? []) expect(existsSync(join(repo, script)), script).toBe(true);
    expect(workflow).not.toMatch(/pull_request/);
    expect(workflow).toMatch(/permissions:\s+contents: read/);
  });
});
