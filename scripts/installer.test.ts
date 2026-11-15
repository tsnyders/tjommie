import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { codeOf, installer, powershell, runBody, runWith, temp, windows, windowsPowerShells } from "./ps-support.ts";

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

// F10: a process started through an 8.3 path (RUNNER~1) reports that path, and a folder given the other way round must still match.
describe.skipIf(!windows)("process scoping and long and short names", () => {
  it("matches a path and a folder however either is spelled, 8.3 names included", () => {
    const root = temp();
    const folder = join(root, "tjommie-a-long-folder-name");
    mkdirSync(folder);
    writeFileSync(join(folder, "Tjommie.exe"), "x");
    const result = runBody("scoped-processes.ps1", String.raw`
      $folder = '${folder}'
      $fso = New-Object -ComObject Scripting.FileSystemObject
      $short = $fso.GetFolder($folder).ShortPath
      [ordered]@{
        short       = $short
        differs     = ($short -ine $folder)
        spelledOut  = (Get-Item -LiteralPath $folder).FullName
        longOfShort = (Get-LongPath $short)
        shortPathInLong  = (Test-PathInFolder (Join-Path $short 'Tjommie.exe') $folder)
        longPathInShort  = (Test-PathInFolder (Join-Path $folder 'Tjommie.exe') $short)
        missingLeaf = (Get-LongPath (Join-Path $short 'not-there\deeper.txt'))
        sibling     = (Test-PathInFolder (Join-Path (Split-Path $short -Parent) 'tjommie-a-long-folder-name-2\Tjommie.exe') $folder)
        elsewhere   = (Test-PathInFolder 'D:\nowhere\Tjommie.exe' $folder)
      } | ConvertTo-Json -Compress
    `) as { short: string; differs: boolean; spelledOut: string; longOfShort: string; shortPathInLong: boolean; longPathInShort: boolean; missingLeaf: string; sibling: boolean; elsewhere: boolean };
    // The long spelling expected is what the file system says, not what this test built the folder from: on a GitHub-hosted runner tmpdir()
    // is itself spelled with an 8.3 alias (a profile folder such as RUNNER~1), and Get-LongPath rightly spells that out (runneradmin), so comparing with
    // `folder` as built would expect the alias back. PowerShell's own FullName is the independent answer to check it against.
    // Where the volume makes no 8.3 names, the two spellings are one and the checks still hold.
    const longFolder = result.spelledOut;
    expect(longFolder, "the folder's long spelling has no alias left in it").not.toMatch(/~\d/);
    expect(result.longOfShort.toLowerCase()).toBe(longFolder.toLowerCase());
    expect(result.shortPathInLong).toBe(true);
    expect(result.longPathInShort).toBe(true);
    expect(result.missingLeaf.toLowerCase()).toBe(join(longFolder, "not-there", "deeper.txt").toLowerCase());
    expect(result.sibling).toBe(false);
    expect(result.elsewhere).toBe(false);
    if (!result.differs) console.warn("This volume has no 8.3 name for the test folder, so only the long spelling was exercised.");
  });
});

// F6: the walk that takes the app's children with it must not take a program the user is looking at (a browser the app opened).
describe.skipIf(!windows)("closing the app's children, but not a program with a window of its own", () => {
  it("leaves a descendant outside the folder that owns a visible window, closes the rest, and says which it left", () => {
    const root = temp();
    const inside = join(root, "install");
    mkdirSync(inside);
    const result = runBody("scoped-processes.ps1", String.raw`
      $system = "$env:SystemRoot\System32"
      Copy-Item "$system\cmd.exe" '${join(inside, "Tjommie.exe")}'
      $started = @()
      $windowed = 0
      try {
        # The stub app, with a child outside the folder: ping, which stands in for a browser and is given a window below.
        $app = Start-Process -FilePath '${join(inside, "Tjommie.exe")}' -ArgumentList '/c','ping -n 60 127.0.0.1' -WindowStyle Hidden -PassThru
        $started += $app
        $children = @()
        for ($i = 0; $i -lt 60 -and $children.Count -lt 1; $i++) { Start-Sleep -Milliseconds 100; $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($app.Id)" | Where-Object { $_.Name -eq 'PING.EXE' }) }
        if ($children.Count -lt 1) { throw 'The stub app never started a child.' }
        $windowed = [int]$children[0].ProcessId
        $real = Test-OwnsVisibleWindow $windowed
        # A real program with a window is not needed: the probe is replaced for this one id.
        function Test-OwnsVisibleWindow([int]$ProcessId) { return ($ProcessId -eq $windowed) }
        $stopped = @(Stop-ProcessesInFolder '${inside}')
        Start-Sleep -Milliseconds 700
        $alive = { param($id) [bool](Get-Process -Id $id -ErrorAction SilentlyContinue) }
        [ordered]@{
          realProbeOnAHiddenProcess = $real
          appAlive      = (& $alive $app.Id)
          windowedAlive = (& $alive $windowed)
          stopped       = @($stopped | ForEach-Object { $_['id'] })
          windowedId    = $windowed
          skipped       = @($script:LastSkippedWindowed.Keys)
        } | ConvertTo-Json -Compress
      } finally {
        foreach ($process in $started) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        if ($windowed) { Stop-Process -Id $windowed -Force -ErrorAction SilentlyContinue }
      }
    `) as { realProbeOnAHiddenProcess: boolean; appAlive: boolean; windowedAlive: boolean; stopped: number[]; windowedId: number; skipped: number[] };
    expect(result.realProbeOnAHiddenProcess).toBe(false); // a hidden console program owns no visible window
    expect(result.appAlive).toBe(false);
    expect(result.windowedAlive).toBe(true);
    expect(result.stopped).not.toContain(result.windowedId);
    expect(result.skipped).toEqual([result.windowedId]);
  });
});

// F4: NSIS's RMDir /r follows junctions, so before anything is deleted the links inside the install folder are removed as links.
describe.skipIf(!windows)("the helper that removes links from an install folder before it is deleted", () => {
  const helper = join(installer, "unlink-reparse-points.ps1");
  const seed = (folder: string): void => {
    mkdirSync(folder, { recursive: true });
    writeFileSync(join(folder, "keep.txt"), "must survive");
    mkdirSync(join(folder, "deeper"), { recursive: true });
    writeFileSync(join(folder, "deeper", "secret.bin"), "also must survive");
  };

  it("removes a junction (and one nested deeper) as a link and never touches what it points at, and leaves real folders alone", () => {
    const root = temp();
    const install = join(root, "install");
    const outside = join(root, "outside-sentinel");
    seed(install);
    seed(outside);
    const link = (path: string): string => `New-Item -ItemType Junction -Path '${path}' -Target '${outside}' | Out-Null`;
    const make = join(temp(), "make.ps1");
    writeFileSync(make, `${link(join(install, "resources-link"))}\n${link(join(install, "deeper", "nested-link"))}\n`);
    expect(powershell(make, [], { ...process.env }).status).toBe(0);
    const first = powershell(helper, ["-Folder", install], { ...process.env });
    expect(first.status, `${first.stdout}${first.stderr}`).toBe(0);
    expect(first.stdout).toMatch(/Removed 2 link/);
    for (const kept of [join(outside, "keep.txt"), join(outside, "deeper", "secret.bin"), join(install, "keep.txt"), join(install, "deeper", "secret.bin")]) expect(readFileSync(kept, "utf8").length, kept).toBeGreaterThan(0);
    expect(existsSync(join(install, "resources-link"))).toBe(false);
    expect(existsSync(join(install, "deeper", "nested-link"))).toBe(false);
    // Nothing is left to remove the second time.
    const second = powershell(helper, ["-Folder", install], { ...process.env });
    expect(second.status).toBe(0);
    expect(second.stdout).not.toMatch(/Removed/);
  });

  it("refuses an install folder that is itself a link, and one that is too broad, and does nothing for one that is not there", () => {
    const root = temp();
    const target = join(root, "real");
    seed(target);
    const asLink = join(root, "install-link");
    const make = join(temp(), "make.ps1");
    writeFileSync(make, `New-Item -ItemType Junction -Path '${asLink}' -Target '${target}' | Out-Null\n`);
    expect(powershell(make, [], { ...process.env }).status).toBe(0);
    const linked = powershell(helper, ["-Folder", asLink], { ...process.env });
    expect(linked.status).toBe(4);
    expect(linked.stdout).toMatch(/is itself a link/);
    expect(readFileSync(join(target, "keep.txt"), "utf8")).toBe("must survive");
    expect(powershell(helper, ["-Folder", "C:\\"], { ...process.env }).status).not.toBe(0);
    expect(powershell(helper, ["-Folder", join(root, "missing")], { ...process.env }).status).toBe(0);
  });
});

// The first runner run of the T8.2 fix round: the installer exited 8 ("the helper could not run") over a real R1, with no reason, and
// every uninstall after it failed the same way. The helpers are now exercised the way the installer runs them: under the Windows
// PowerShell engines (32-bit first), with -File, through an Electron-like tree, and the failure path must say what failed.
describe.skipIf(!windows)("the helpers, under the engines the installer runs them with", () => {
  it("has the engines it needs", () => {
    expect(windowsPowerShells.map(([label]) => label).join(",")).toContain("32-bit");
  });

  // Nothing in the helper path may compile code: Add-Type starts csc.exe, which the runner's Defender and a locked-down machine can
  // refuse, and then the helper fails closed for a reason that has nothing to do with the install. (The AUMID helper still uses it, and
  // an install without the ID is allowed to finish.)
  it("needs no compiler: no stop, unlink or library code calls Add-Type or P/Invokes", () => {
    for (const name of ["scoped-processes.ps1", "stop-install-processes.ps1", "unlink-reparse-points.ps1", "helper-log.ps1"]) {
      const code = codeOf(join(installer, name));
      expect(code, name).not.toMatch(/Add-Type|DllImport|\[System\.Runtime\.InteropServices/i);
    }
  });

  it.each(windowsPowerShells)("%s: closes an Electron-like tree by path, keeps the excluded process and the decoys, and logs every decision", (label, engine) => {
    const root = temp();
    const inside = join(root, "install");
    const outside = join(root, "elsewhere");
    const logs = join(root, "logs");
    mkdirSync(inside);
    mkdirSync(outside);
    const driver = join(root, "tree.ps1");
    writeFileSync(driver, String.raw`
      $ErrorActionPreference = 'Stop'
      $system = "$env:SystemRoot\System32"
      Copy-Item "$system\cmd.exe" '${join(inside, "Tjommie.exe")}'
      Copy-Item "$system\cmd.exe" '${join(inside, "launcher.exe")}'
      Copy-Item "$system\ping.exe" '${join(inside, "tjommie-daemon.exe")}'
      Copy-Item "$system\ping.exe" '${join(inside, "Uninstall Tjommie.exe")}'
      Copy-Item "$system\cmd.exe" '${join(outside, "Tjommie.exe")}'
      $started = @()
      try {
        # The app, with its daemon in the folder and a command-line tool outside it; the uninstaller that runs the helper (excluded by id);
        # a same-named program elsewhere; and a launcher whose child outlives it, so that child's parent no longer exists.
        $app = Start-Process '${join(inside, "Tjommie.exe")}' -ArgumentList '/c','ping -n 88 127.0.0.1 >nul' -WindowStyle Hidden -PassThru
        $daemon = Start-Process '${join(inside, "tjommie-daemon.exe")}' -ArgumentList '-n','87','127.0.0.1' -WindowStyle Hidden -PassThru
        $excluded = Start-Process '${join(inside, "Uninstall Tjommie.exe")}' -ArgumentList '-n','87','127.0.0.1' -WindowStyle Hidden -PassThru
        $decoy = Start-Process '${join(outside, "Tjommie.exe")}' -ArgumentList '/c','ping -n 87 127.0.0.1 >nul' -WindowStyle Hidden -PassThru
        $launcher = Start-Process '${join(inside, "launcher.exe")}' -ArgumentList '/c','start /b ping -n 86 127.0.0.1 >nul' -WindowStyle Hidden -PassThru
        $started += $app, $daemon, $excluded, $decoy
        $pings = @()
        for ($i = 0; $i -lt 80 -and $pings.Count -lt 1; $i++) { Start-Sleep -Milliseconds 100; $pings = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($app.Id)" | Where-Object { $_.Name -eq 'PING.EXE' }) }
        if ($pings.Count -lt 1) { throw 'The stub app never started its children.' }
        Start-Sleep -Milliseconds 800
        $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($app.Id)" | ForEach-Object { [int]$_.ProcessId })
        $orphans = @(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match 'ping -n 86 127' -or ($_.Name -eq 'PING.EXE' -and $_.CommandLine -match '-n 86 127') } | ForEach-Object { [int]$_.ProcessId })
        $env:TJOMMIE_HELPER_LOG_DIR = '${logs}'
        $output = & '${engine}' -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '${join(installer, "stop-install-processes.ps1")}' -Folder '${inside}' -ExceptProcessId $excluded.Id 2>&1 | Out-String
        $code = $LASTEXITCODE
        Start-Sleep -Milliseconds 500
        $alive = { param($id) [bool](Get-Process -Id $id -ErrorAction SilentlyContinue) }
        [ordered]@{
          code = $code; output = $output.Trim()
          appAlive = (& $alive $app.Id); daemonAlive = (& $alive $daemon.Id); excludedAlive = (& $alive $excluded.Id); decoyAlive = (& $alive $decoy.Id)
          childrenAlive = @($children | Where-Object { & $alive $_ }).Count; childCount = $children.Count
          orphansAlive = @($orphans | Where-Object { & $alive $_ }).Count; orphanCount = $orphans.Count
          appId = $app.Id; excludedId = $excluded.Id
        } | ConvertTo-Json -Compress
      } finally {
        foreach ($process in $started) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        foreach ($id in @(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match '-n 8[678] 127' } | ForEach-Object { [int]$_.ProcessId })) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
      }
    `);
    const run = powershell(driver, [], { ...process.env });
    expect(run.status, `${run.stdout}${run.stderr}`).toBe(0);
    const result = JSON.parse(run.stdout) as { code: number; output: string; appAlive: boolean; daemonAlive: boolean; excludedAlive: boolean; decoyAlive: boolean; childrenAlive: number; childCount: number; orphansAlive: number; orphanCount: number; appId: number; excludedId: number };
    expect(result.code, `${label}: ${result.output}`).toBe(0);
    expect(result.appAlive).toBe(false);
    expect(result.daemonAlive).toBe(false); // in the folder, though nothing in the tree started it
    expect(result.childCount).toBeGreaterThanOrEqual(1);
    expect(result.childrenAlive).toBe(0); // the daemon in the folder, and the tool it started outside it
    expect(result.excludedAlive).toBe(true); // the caller itself: an uninstaller that runs from inside the folder
    expect(result.decoyAlive).toBe(true); // the same name, another folder
    expect(result.orphanCount).toBeGreaterThanOrEqual(1);
    expect(result.orphansAlive).toBe(result.orphanCount); // a child whose parent is gone is nobody's child: left alone
    // The log says what it did: what it ran as, and each process it chose, with the reason.
    const log = readdirSync(logs).filter((file) => file.startsWith("stop-install-processes-"));
    expect(log).toHaveLength(1);
    const text = readFileSync(join(logs, log[0] ?? ""), "utf8");
    expect(text).toContain("started: PowerShell 5.1");
    expect(text).toContain(label.startsWith("32") ? "32-bit process" : "64-bit process");
    expect(text, text).toMatch(new RegExp(`select ${result.appId} Tjommie\\.exe .*\\(in the folder\\)`));
    expect(text, text).toMatch(/select \d+ tjommie-daemon\.exe .*\(in the folder\)/);
    expect(text, text).toMatch(/select \d+ \S+ .*\(child of \d+\)/);
    expect(text, text).toMatch(/nothing runs from the folder any more/);
    expect(text, text).not.toContain(`select ${result.excludedId} `);
  });

  it.each(windowsPowerShells)("%s: an unexpected error exits 1, says what it was, and leaves the exception and its stack in the log", (label, engine) => {
    const root = temp();
    const logs = join(root, "logs");
    const env = { ...process.env, TJOMMIE_HELPER_LOG_DIR: logs };
    const run = runWith(engine, join(installer, "stop-install-processes.ps1"), ["-Folder", "C:\\", "-ExceptProcessId", "0"], env);
    expect(run.status, label).toBe(1);
    expect(run.stdout).toMatch(/HELPER FAILED: System\.Management\.Automation\.RuntimeException: Refusing to stop processes in C:: it is too broad/);
    expect(run.stdout).toContain(logs);
    const text = readFileSync(join(logs, readdirSync(logs)[0] ?? ""), "utf8");
    expect(text).toMatch(/FAILED: .*too broad/);
    expect(text).toMatch(/stack: .*Assert-NarrowFolder/);
  });

  // What went wrong last time may have been the library failing to load, before any of the helper's own code ran. That must be
  // caught and reported too, not left as a bare exit code.
  it.each(windowsPowerShells)("%s: a library that fails to load is reported, with its message, and the helper exits 1", (label, engine) => {
    const root = temp();
    const copy = join(root, "helpers");
    const logs = join(root, "logs");
    mkdirSync(copy);
    for (const name of ["stop-install-processes.ps1", "helper-log.ps1"]) writeFileSync(join(copy, name), readFileSync(join(installer, name)));
    writeFileSync(join(copy, "scoped-processes.ps1"), "throw 'the library could not load'\n");
    const run = runWith(engine, join(copy, "stop-install-processes.ps1"), ["-Folder", join(root, "install"), "-ExceptProcessId", "0"], { ...process.env, TJOMMIE_HELPER_LOG_DIR: logs });
    expect(run.status, label).toBe(1);
    expect(run.stdout).toContain("HELPER FAILED");
    expect(run.stdout).toContain("the library could not load");
    expect(readFileSync(join(logs, readdirSync(logs)[0] ?? ""), "utf8")).toContain("the library could not load");
  });

  // Windows PowerShell 5.1 (.NET Framework) throws PathTooLongException past 260 characters. A developer's deep tree must not make the
  // link remover refuse an install it can see no link in, nor miss a link that is deep.
  it.each(windowsPowerShells)("%s: walks a tree deeper than 260 characters, and removes a junction that sits in it", (label, engine) => {
    const root = temp();
    const install = join(root, "install");
    const outside = join(root, "outside");
    const logs = join(root, "logs");
    mkdirSync(install);
    mkdirSync(outside);
    writeFileSync(join(outside, "sentinel.txt"), "keep");
    const deep = Array.from({ length: 12 }, (_, index) => `level-${index}-${"x".repeat(24)}`);
    const make = join(temp(), "make.ps1");
    writeFileSync(make, String.raw`
      $ErrorActionPreference = 'Stop'
      $path = '${install}'
      foreach ($part in @(${deep.map((part) => `'${part}'`).join(",")})) { $path = Join-Path $path $part }
      [void][IO.Directory]::CreateDirectory('\\?\' + $path)
      [IO.File]::WriteAllText('\\?\' + (Join-Path $path 'deep.txt'), 'x')
      # a junction one level above the deepest folder, made with a path that is still short enough for mklink
      New-Item -ItemType Junction -Path '${join(install, "shallow-link")}' -Target '${outside}' | Out-Null
      "$($path.Length)"
    `);
    const made = powershell(make, [], { ...process.env });
    expect(made.status, `${made.stdout}${made.stderr}`).toBe(0);
    expect(Number(made.stdout.trim())).toBeGreaterThan(260);
    const run = runWith(engine, join(installer, "unlink-reparse-points.ps1"), ["-Folder", install], { ...process.env, TJOMMIE_HELPER_LOG_DIR: logs });
    expect(run.status, `${label}: ${run.stdout}${run.stderr}`).toBe(0);
    expect(run.stdout).toMatch(/Removed 1 link/);
    expect(readFileSync(join(outside, "sentinel.txt"), "utf8")).toBe("keep");
  });

  it.each(windowsPowerShells)("%s: the link remover logs the links it removed, and fails with a reason for a folder that is itself a link", (label, engine) => {
    const root = temp();
    const install = join(root, "install");
    const outside = join(root, "outside");
    const logs = join(root, "logs");
    mkdirSync(join(install, "resources"), { recursive: true });
    mkdirSync(outside);
    writeFileSync(join(outside, "sentinel.txt"), "keep");
    const make = join(temp(), "make.ps1");
    writeFileSync(make, `New-Item -ItemType Junction -Path '${join(install, "resources", "linked")}' -Target '${outside}' | Out-Null\nNew-Item -ItemType Junction -Path '${join(root, "install-link")}' -Target '${install}' | Out-Null\n`);
    expect(powershell(make, [], { ...process.env }).status).toBe(0);
    const env = { ...process.env, TJOMMIE_HELPER_LOG_DIR: logs };
    const removed = runWith(engine, join(installer, "unlink-reparse-points.ps1"), ["-Folder", install], env);
    expect(removed.status, `${label}: ${removed.stdout}${removed.stderr}`).toBe(0);
    expect(removed.stdout).toMatch(/Removed 1 link/);
    expect(readFileSync(join(outside, "sentinel.txt"), "utf8")).toBe("keep");
    const refused = runWith(engine, join(installer, "unlink-reparse-points.ps1"), ["-Folder", join(root, "install-link")], env);
    expect(refused.status).toBe(4);
    expect(refused.stdout).toMatch(/is itself a link/);
    const texts = readdirSync(logs).map((file) => readFileSync(join(logs, file), "utf8")).join("\n");
    expect(texts).toMatch(/removed link .*linked/);
    expect(texts).toMatch(/is itself a link/);
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
