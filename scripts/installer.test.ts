import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { codeOf, installer, powershell, runBody, temp, windows } from "./ps-support.ts";

// T8.2: the NSIS installer (installer/tjommie.nsi) and the scripts it runs. Nothing in this file runs an installer or an
// uninstaller, here or anywhere but a GitHub-hosted runner: it reads the script's text, compiles it when a compiler is
// there (compiling only writes an .exe into a temp folder), and starts PowerShell helpers against stubs it makes itself.

vi.setConfig({ testTimeout: 90_000 });

// F2 of the T8.2 review: the guard was two environment variables, which anything can set. It now also needs what a real hosted runner
// is, as T8.1 recorded it (user runneradmin, RUNNER_TEMP under D:\a\_temp, a run id and attempt), and the install result that a
// script reads must carry this run's id and attempt. Both are pure functions of the facts they are given, so every missing
// condition is tried here without a script that installs anything being started, and without pretending to be another user.
describe.skipIf(!windows)("the hosted-runner guard", () => {
  const hosted = { GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "github-hosted", RUNNER_TEMP: "D:\\a\\_temp", GITHUB_RUN_ID: "43653342616", GITHUB_RUN_ATTEMPT: "1" };
  const problems = (user: string, environment: Record<string, string>): string[] => {
    const pairs = Object.entries(environment).map(([key, value]) => `'${key}' = '${value}'`).join("; ");
    return runBody("hosted-runner.ps1", `ConvertTo-Json -Compress -InputObject @(Get-HostedRunnerProblems '${user}' @{ ${pairs} })`) as string[];
  };

  it("accepts what a real hosted runner is, and nothing less", () => {
    expect(problems("runneradmin", hosted)).toEqual([]);
    expect(problems("RunnerAdmin", hosted)).toEqual([]); // Windows user names do not care about case
    expect(problems("runneradmin", { ...hosted, RUNNER_TEMP: "D:\\a\\_temp\\r1-latest" })).toEqual([]);
  });

  it.each([
    ["GITHUB_ACTIONS is not true", "runneradmin", { ...hosted, GITHUB_ACTIONS: "" }],
    ["GITHUB_ACTIONS is not true", "runneradmin", { ...hosted, GITHUB_ACTIONS: "yes" }],
    ["RUNNER_ENVIRONMENT is not github-hosted", "runneradmin", { ...hosted, RUNNER_ENVIRONMENT: "self-hosted" }],
    ["the user is not runneradmin", "owner", hosted],
    ["the user is not runneradmin", "", hosted],
    ["RUNNER_TEMP is not under D:\\a\\_temp", "runneradmin", { ...hosted, RUNNER_TEMP: "" }],
    ["RUNNER_TEMP is not under D:\\a\\_temp", "runneradmin", { ...hosted, RUNNER_TEMP: "C:\\Users\\alice\\AppData\\Local\\Temp" }],
    ["RUNNER_TEMP is not under D:\\a\\_temp", "runneradmin", { ...hosted, RUNNER_TEMP: "D:\\a\\_temporary" }], // a prefix of the name, not a folder inside it
    ["RUNNER_TEMP is not under D:\\a\\_temp", "runneradmin", { ...hosted, RUNNER_TEMP: "D:\\a\\_temp\\..\\.." }],
    ["GITHUB_RUN_ID is not a run id", "runneradmin", { ...hosted, GITHUB_RUN_ID: "" }],
    ["GITHUB_RUN_ID is not a run id", "runneradmin", { ...hosted, GITHUB_RUN_ID: "run-1" }],
    ["GITHUB_RUN_ATTEMPT is not an attempt number", "runneradmin", { ...hosted, GITHUB_RUN_ATTEMPT: "" }],
  ])("refuses when %s", (reason, user, environment) => {
    const found = problems(user, environment);
    expect(found).toHaveLength(1);
    expect(found[0]).toBe(reason);
  });

  it("names every condition that fails, not just the first", () => {
    expect(problems("owner", {})).toHaveLength(6);
  });

  // The install result that accept-over-r1.ps1 and collect-r1-facts.ps1 read says which R1 to act on. A hand-written one, or one left by
  // another run, must not be accepted.
  it("accepts an install result that carries this run's id and attempt, and nothing else", () => {
    const binding = (result: string, runId: string, attempt: string): string[] =>
      runBody("hosted-runner.ps1", `$result = '${result}' | ConvertFrom-Json\nConvertTo-Json -Compress -InputObject @(Get-InstallResultBindingProblems $result '${runId}' '${attempt}')`) as string[];
    const bound = JSON.stringify({ installFolder: "C:\\x", githubRunId: "7", githubRunAttempt: "2", fresh: true });
    expect(binding(bound, "7", "2")).toEqual([]);
    expect(binding(JSON.stringify({ installFolder: "C:\\x" }), "7", "2")).toEqual(["the install result does not carry githubRunId", "the install result does not carry githubRunAttempt", "the install result was not made on a fresh runner"]);
    expect(binding(bound, "8", "2")).toEqual(["the install result is from another run (githubRunId is 7, this run's is 8)"]);
    expect(binding(bound, "7", "3")).toEqual(["the install result is from another run (githubRunAttempt is 2, this run's is 3)"]);
    expect(binding(bound, "", "")).toHaveLength(2); // no run id here at all: it cannot be this run's
    expect(binding(JSON.stringify({ installFolder: "C:\\x", githubRunId: "7", githubRunAttempt: "2", fresh: false }), "7", "2")).toEqual(["the install result was not made on a fresh runner"]);
  });

  it("makes every script that installs, uninstalls or stops anything check the guard first, and the ones that read an install result check its binding", () => {
    for (const name of ["accept-fresh.ps1", "accept-over-r1.ps1", "collect-r1-facts.ps1", "install-latest-r1.ps1"]) expect(codeOf(join(installer, name)), name).toContain("Assert-HostedRunner");
    for (const name of ["accept-over-r1.ps1", "collect-r1-facts.ps1"]) {
      const text = codeOf(join(installer, name));
      expect(text, name).toContain("Assert-InstallResultBoundToThisRun");
      // Before anything that acts on the install it names.
      expect(text.indexOf("Assert-InstallResultBoundToThisRun"), name).toBeLessThan(text.search(/Start-Process|Invoke-Step|Invoke-Phase|Stop-ProcessesInFolder|Add-Sentinels/));
    }
    const install = codeOf(join(installer, "install-latest-r1.ps1"));
    expect(install).toMatch(/githubRunId\s*=\s*\$env:GITHUB_RUN_ID/);
    expect(install).toMatch(/githubRunAttempt\s*=\s*\$env:GITHUB_RUN_ATTEMPT/);
    expect(install).toMatch(/fresh\s*=\s*\$true/);
    expect(install.indexOf("Assert-FreshRunner")).toBeLessThan(install.indexOf("fresh"));
  });
});

describe.skipIf(!windows)("the installer's helper that sets a shortcut's AppUserModelID", () => {
  // NSIS's CreateShortCut cannot set the property that Windows keys notifications by, so the installer makes the shortcut and
  // this sets the property. The shortcut here is made in a temp folder, with an ID that is not Tjommie's, and is read back the
  // way the T8.1 facts read the real ones.
  it("writes the ID into the .lnk, leaves its target alone, and fails on a missing shortcut", () => {
    const folder = temp();
    const lnk = join(folder, "Test App.lnk");
    const create = join(folder, "create.ps1");
    writeFileSync(create, `$shell = New-Object -ComObject WScript.Shell\n$link = $shell.CreateShortcut('${lnk}')\n$link.TargetPath = "$env:SystemRoot\\System32\\cmd.exe"\n$link.Description = 'a test shortcut'\n$link.Save()\n`);
    expect(powershell(create, [], { ...process.env }).status).toBe(0);
    const helper = join(installer, "set-shortcut-aumid.ps1");
    const run = powershell(helper, ["-Path", lnk, "-AppUserModelId", "tjommie.installer-test.aumid"], { ...process.env });
    expect(`${run.stdout}${run.stderr}`.trim()).toBe("");
    expect(run.status).toBe(0);
    const facts = runBody("r1-facts-lib.ps1", `Get-ShortcutFacts '${folder}' | ConvertTo-Json -Compress -AsArray`) as Array<{ path: string; target: string; description: string; appUserModelId: string }>;
    expect(facts).toHaveLength(1);
    expect(facts[0]?.appUserModelId).toBe("tjommie.installer-test.aumid");
    expect(facts[0]?.target.toLowerCase()).toBe(`${process.env.SystemRoot ?? "C:\\Windows"}\\System32\\cmd.exe`.toLowerCase());
    expect(facts[0]?.description).toBe("a test shortcut");
    expect(powershell(helper, ["-Path", join(folder, "missing.lnk"), "-AppUserModelId", "x.y"], { ...process.env }).status).not.toBe(0);
  });
});

describe.skipIf(!windows)("the installer's helper that closes what runs from an install folder", () => {
  // The uninstaller runs from inside the folder when it is given `_?=<folder>` (as R1's installer gives it), so a stop that
  // took "everything running from the folder" would stop the uninstaller itself.
  it("leaves the processes it is told to leave, stops the rest, and says so when something survives", () => {
    const root = temp();
    const inside = join(root, "install");
    mkdirSync(inside);
    const result = runBody("scoped-processes.ps1", String.raw`
      $system = "$env:SystemRoot\System32"
      Copy-Item "$system\ping.exe" '${join(inside, "Uninstall Tjommie.exe")}'
      Copy-Item "$system\ping.exe" '${join(inside, "tjommie-daemon.exe")}'
      $started = @()
      try {
        $uninstaller = Start-Process -FilePath '${join(inside, "Uninstall Tjommie.exe")}' -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
        $daemon = Start-Process -FilePath '${join(inside, "tjommie-daemon.exe")}' -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
        $started += $uninstaller, $daemon
        Start-Sleep -Milliseconds 300
        $stopped = @(Stop-ProcessesInFolder '${inside}' -ExceptIds @($uninstaller.Id))
        Start-Sleep -Milliseconds 700
        $alive = { param($id) [bool](Get-Process -Id $id -ErrorAction SilentlyContinue) }
        [ordered]@{
          stopped              = @($stopped | ForEach-Object { $_['id'] })
          uninstallerId        = $uninstaller.Id
          daemonId             = $daemon.Id
          uninstallerAlive     = (& $alive $uninstaller.Id)
          daemonAlive          = (& $alive $daemon.Id)
        } | ConvertTo-Json -Compress
      } finally {
        foreach ($process in $started) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
      }
    `) as { stopped: number[]; uninstallerId: number; daemonId: number; uninstallerAlive: boolean; daemonAlive: boolean };
    expect(result.uninstallerAlive).toBe(true);
    expect(result.daemonAlive).toBe(false);
    expect(result.stopped).toContain(result.daemonId);
    expect(result.stopped).not.toContain(result.uninstallerId);
  });

  it("exits 0 when the folder is not there or nothing runs from it, and refuses a folder that is too broad", () => {
    const root = temp();
    const empty = join(root, "empty");
    mkdirSync(empty);
    const script = join(installer, "stop-install-processes.ps1");
    const env = { ...process.env };
    expect(powershell(script, ["-Folder", join(root, "missing"), "-ExceptProcessId", "0"], env).status).toBe(0);
    expect(powershell(script, ["-Folder", empty, "-ExceptProcessId", "0"], env).status).toBe(0);
    // A folder that is too broad is refused, and nothing is stopped.
    const broad = powershell(script, ["-Folder", "C:\\", "-ExceptProcessId", "0"], env);
    expect(broad.status).not.toBe(0);
    expect(`${broad.stdout}${broad.stderr}`).toMatch(/too broad/i);
  });
});
