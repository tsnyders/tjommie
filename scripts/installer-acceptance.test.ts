import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { codeOf, installer, powershell, repo, runBody, temp, windows } from "./ps-support.ts";

// T8.2: the scripts that install, update and uninstall on a GitHub-hosted runner (installer/accept-*.ps1) and the workflow jobs
// that run them. Here they are only parsed, shown to refuse to run anywhere but a hosted runner, and their pure helpers are
// tried on temp folders. Nothing in this file installs, updates or uninstalls anything, or reads a Tjommie location: the helpers
// that read the registry for production's keys are never called here.

vi.setConfig({ testTimeout: 90_000 });

const acceptance = ["accept-fresh.ps1", "accept-over-r1.ps1", "accept-update-path.ps1", "accept-production-over-r1.ps1"];

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

  // The review's scenarios (F1, F4, F5, F7 and the data-root comparison), pinned by their shape: what each must assert, and the order
  // that makes it mean something.
  it("plants a stray InstallLocation before an update, over a fresh install and over the real R1, and asserts nothing was touched (F1)", () => {
    const fresh = codeOf(join(installer, "accept-fresh.ps1"));
    const over = codeOf(join(installer, "accept-over-r1.ps1"));
    for (const [name, text] of [["accept-fresh", fresh], ["accept-over-r1", over]] as const) {
      expect(text, name).toMatch(/exitCode'\]\s*-eq 7\)/);
      expect(text, name).toContain("old-uninstaller.exe");
      expect(text, name).toMatch(/Set-InstallLocation \$guid \$planted/);
      expect(text, name).toContain("the value was not corrected");
      for (const planted of ["'C:\\'", "$scratch"]) expect(text, `${name}: ${planted}`).toContain(planted);
    }
    // A sibling folder, and the dev folder, hold files of known content that must survive.
    expect(fresh).toContain("tjommie-old");
    expect(fresh).toMatch(/Join-Path \$programs 'tjommie-rs'/);
    expect(over).toMatch(/\$sibling = Join-Path \$programs 'tjommie-rs'/);
    // Over R1 it happens before R1 is started, so R1's files can be compared byte for byte.
    expect(over.indexOf("'O1b ")).toBeGreaterThan(over.indexOf("'O1 "));
    expect(over.indexOf("'O1b ")).toBeLessThan(over.indexOf("'O2 "));
    expect(over.indexOf("'O1b ")).toBeLessThan(over.indexOf("Start-Process -FilePath (Join-Path $folder $prod.exe)"));
  });

  it("compares the data roots ignoring only the named run-time set, and everything else that is missing still counts (harness fix)", () => {
    const over = codeOf(join(installer, "accept-over-r1.ps1"));
    expect(over).toContain("$r1RuntimeOnly = @(Get-R1RuntimeOnlyEntries $facts)");
    const compares = over.match(/Compare-Snapshot \$state\['dataBefore'\][^\n]*/g) ?? [];
    expect(compares).toHaveLength(1); // one shared comparison, used by O3 and O4
    expect(compares[0]).toContain("-IgnorePaths $r1RuntimeOnly");
    expect(over).toMatch(/Test-DataRootsKept 'O3'/);
    expect(over).toMatch(/Test-DataRootsKept 'O4'/);
    // Nothing else is filtered out: the only filter is the one that keeps missing files and changed sentinels.
    expect(compares[0]).toContain("'missing:*'");
    expect(over).not.toMatch(/-IgnorePaths @\(/);
  });

  it("puts a junction in the install folder before an update and an uninstall, and refuses a folder that is itself a link (F4)", () => {
    const fresh = codeOf(join(installer, "accept-fresh.ps1"));
    expect(fresh).toMatch(/New-Junction \$link \$target/);
    expect(fresh).toContain("what the junctions pointed at is untouched by the update");
    expect(fresh).toContain("what the junctions pointed at is untouched by the uninstall");
    expect(fresh).toMatch(/exitCode'\]\s*-eq 9\)/);
    expect(fresh).toContain("[IO.Directory]::Delete($folder)"); // the link is removed as a link in the test too
  });

  // The default leaves the start-at-login value, as R1 does. The code that removes it (only when it points into the folder, never in an
  // update) stays built behind a switch, and a variant installer built with the switch on keeps it exercised on the runner.
  it("keeps the start-at-login removal tested through a variant installer, whatever the default is", () => {
    const fresh = codeOf(join(installer, "accept-fresh.ps1"));
    const build = codeOf(join(installer, "build-acceptance-installers.ps1"));
    expect(build).toContain("-RemoveRunValueOnUninstall");
    expect(build).toMatch(/removerun/);
    expect(fresh).toMatch(/removerun\s*=\s*Join-Path \$InstallersDir "production-\$OldVersion-removerun\.exe"/);
    const step = fresh.slice(fresh.indexOf("'S13 "));
    expect(step).toContain("$installers['removerun']");
    // Removed only for a value that points into the folder (quoted); left for one elsewhere or at a sibling; kept by an update.
    expect(step).toMatch(/\$null -eq \(Get-RunValue \$prod\.appId\)/);
    expect(step).toContain("tjommie-old");
    expect(step).toContain("C:\\elsewhere");
    expect(step).toContain("/S --updated");
    // The scenarios that follow the build's own default read it from the script, so flipping the default does not break them.
    expect(fresh).toMatch(/\$removeRun = \[bool\]\(\(Get-Content -LiteralPath \(Join-Path \$PSScriptRoot 'tjommie\.nsi'\) -Raw\) -match '\(\?m\)\^!define REMOVE_RUN_VALUE_ON_UNINSTALL/);
  });

  it("points the uninstaller at the data roots with the right InstallLocation, the half of the guard that protects them (F7)", () => {
    const fresh = codeOf(join(installer, "accept-fresh.ps1"));
    expect(fresh).toMatch(/foreach \(\$target in @\(\$dataRoots\['local'\], \$dataRoots\['roaming'\], 'C:\\', /);
    expect(fresh).toContain('"/S _?=$target"');
    // Only a drive root may exit 2 (NSIS refuses it itself, before the guard runs); every other target must reach the guard and exit 5.
    expect(fresh).toMatch(/\$isDriveRoot = \(\$target\.TrimEnd\('\\'\)\.Length -le 2\)/);
    expect(fresh).toMatch(/\$expected = if \(\$isDriveRoot\) \{ @\(2, 5\) \} else \{ @\(5\) \}/);
    expect(fresh).toMatch(/\$expected -contains \$redirected\['exitCode'\]/);
    // And nothing is weakened: for every target, the install folder and the data roots are still compared byte for byte.
    expect(fresh).toContain("...and nothing in the install folder is touched (_?=$target)");
    expect(fresh).toContain('Test-DataUnchanged "by an uninstaller pointed at $target"');
  });

  it("records what R1's own uninstaller closes, by prefix and by image name, as notes and never as promises (F5)", () => {
    const over = codeOf(join(installer, "accept-over-r1.ps1"));
    expect(over).toContain("tjommie-sibling");
    expect(over).toContain("TjommieRS.exe");
    expect(over).toMatch(/Add-Note "O3: \$\(\$pair\[0\]\) was/);
    expect(over).toMatch(/Add-Note "O3: processes with tjommie in their path/);
    // Neither decoy is asserted: that would promise what only R1's code decides.
    expect(over).not.toMatch(/Assert-Check \(?Test-Alive \$state\['(sameName|sibling)'\]/);
    const plan = readFileSync(join(repo, "docs", "superpowers", "plans", "2026-11-13-tjommie-rs-phase3-gpui-desktop.md"), "utf8");
    expect(plan).toContain("Close the dev (`Tjommie RS`) and update-test apps first");
    expect(plan).toContain("*starts with* the install folder's path");
    expect(plan).toContain("Recovery:** run the Phase 3 installer again");
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
    const files = ["accept-fresh.ps1", "accept-over-r1.ps1", "accept-update-path.ps1", "accept-production-over-r1.ps1", "accept-phase3-lib.ps1", "accept-lib.ps1", "build-installer.ps1", "build-acceptance-installers.ps1", "build-stub-payload.ps1", "stop-install-processes.ps1", "unlink-reparse-points.ps1", "helper-log.ps1", "scoped-processes.ps1", "set-shortcut-aumid.ps1", "hosted-runner.ps1", "install-latest-r1.ps1"];
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

  // The first runner run failed O3 and O4 with "missing: lockfile": Electron's lockfile exists only while R1 runs, and the "before"
  // snapshot is taken with R1 running. The comparison ignores exactly the entries R1 has while it runs and not after, read from the
  // T8.1 facts (what its first start left, against what was left after it was stopped), and nothing else.
  it("ignores only the files R1 has while it runs, read from the facts, and a real missing file still fails", () => {
    const result = runBody("accept-lib.ps1", String.raw`
      $facts = Read-Json '${join(installer, "r1-install-facts.json")}'
      $before = [ordered]@{ 'lockfile' = 'AA'; 'sentinel\acceptance.db' = 'BB'; 'bots\x.db' = 'CC'; 'Cache\f_1' = 'DD' }
      $after  = [ordered]@{ 'sentinel\acceptance.db' = 'BB'; 'bots\x.db' = 'CC' }
      $runtime = @(Get-R1RuntimeOnlyEntries $facts)
      [ordered]@{
        runtime         = $runtime
        unfiltered      = @(Compare-Snapshot $before $after -IgnoreAdded)
        filtered        = @(Compare-Snapshot $before $after -IgnoreAdded -IgnorePaths $runtime)
        realMissing     = @(Compare-Snapshot $before ([ordered]@{ 'sentinel\acceptance.db' = 'BB'; 'Cache\f_1' = 'DD' }) -IgnoreAdded -IgnorePaths $runtime)
        sentinelChanged = @(Compare-Snapshot $before ([ordered]@{ 'sentinel\acceptance.db' = 'XX'; 'bots\x.db' = 'CC'; 'Cache\f_1' = 'DD' }) -IgnoreAdded -IgnorePaths $runtime)
        caseless        = @(Compare-Snapshot $before $after -IgnoreAdded -IgnorePaths @('LOCKFILE', 'cache\F_1'))
      } | ConvertTo-Json -Compress
    `) as { runtime: string[]; unfiltered: string[]; filtered: string[]; realMissing: string[]; sentinelChanged: string[]; caseless: string[] };
    expect(result.runtime).toEqual(["lockfile"]); // what the committed facts say R1 has only while it runs
    expect([...result.unfiltered].sort()).toEqual(["missing: Cache\\f_1", "missing: lockfile"]);
    expect(result.filtered).toEqual(["missing: Cache\\f_1"]); // lockfile is ignored, a rotated cache file is not named so it still counts
    expect(result.realMissing).toEqual(["missing: bots\\x.db"]);
    expect(result.sentinelChanged).toEqual(["changed: sentinel\\acceptance.db"]);
    expect(result.caseless).toEqual([]);
  });

  it("lists the files a stub install holds, and the files under a folder, the same way", () => {
    const root = temp();
    mkdirSync(join(root, "resources", "daemon"), { recursive: true });
    // The payload and the uninstaller, and nothing else: the helpers live inside the uninstaller since the T8.2 review (F3).
    for (const file of ["Tjommie.exe", "resources\\daemon\\tjommie-daemon.exe", "Uninstall Tjommie.exe"]) writeFileSync(join(root, file), "x");
    const result = runBody("accept-lib.ps1", String.raw`
      [ordered]@{
        expected  = @(Get-ExpectedFiles 'Tjommie' 'Tjommie.exe')
        installed = @(Get-InstalledFiles '${root}')
        none      = @(Get-InstalledFiles '${join(root, "missing")}')
        dev       = @(Get-ExpectedFiles 'Tjommie RS' 'TjommieRS.exe')
      } | ConvertTo-Json -Compress
    `) as { expected: string[]; installed: string[]; none: string[]; dev: string[] };
    expect(result.installed).toEqual(result.expected);
    expect([...result.expected].sort()).toEqual(["Tjommie.exe", "Uninstall Tjommie.exe", "resources\\daemon\\tjommie-daemon.exe"]);
    expect(result.none).toEqual([]);
    expect(result.dev).toContain("Uninstall Tjommie RS.exe");
    expect(result.dev).toContain("TjommieRS.exe");
  });

  // A GitHub-hosted runner spells its profile and temp folders with an 8.3 alias (a profile folder such as RUNNER~1). Resolve-Path keeps an alias as it
  // was given while Get-ChildItem returns the long spelling, so a path made relative by the length of the root came out with debris on the
  // front ("ta\new.txt"), and a real install's file list would never have matched its expected one. The same folder must give the same
  // relative paths however its root is spelled; where the volume makes no 8.3 name the two spellings are one and only that is checked.
  it("gives the same relative paths whether a folder is named by its long name or its 8.3 alias", () => {
    const folder = join(temp(), "tjommie-a-long-install-folder-name");
    mkdirSync(join(folder, "resources", "daemon"), { recursive: true });
    for (const file of ["Tjommie.exe", "resources\\daemon\\tjommie-daemon.exe"]) writeFileSync(join(folder, file), "x");
    const result = runBody("accept-lib.ps1", String.raw`
      $long = '${folder}'
      $short = (New-Object -ComObject Scripting.FileSystemObject).GetFolder($long).ShortPath
      [ordered]@{
        differs        = ($short -ine $long)
        installedLong  = @(Get-InstalledFiles $long)
        installedShort = @(Get-InstalledFiles $short)
        snapshotLong   = @((Get-TreeSnapshot $long).Keys)
        snapshotShort  = @((Get-TreeSnapshot $short).Keys)
      } | ConvertTo-Json -Compress
    `) as { differs: boolean; installedLong: string[]; installedShort: string[]; snapshotLong: string[]; snapshotShort: string[] };
    const expected = ["Tjommie.exe", "resources\\daemon\\tjommie-daemon.exe"].sort();
    for (const [name, found] of Object.entries({ installedLong: result.installedLong, installedShort: result.installedShort, snapshotLong: result.snapshotLong, snapshotShort: result.snapshotShort })) {
      expect([...found].sort(), name).toEqual(expected);
    }
    if (!result.differs) console.warn("This volume has no 8.3 name for the test folder, so only the long spelling was exercised.");
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

  // The second runner run failed O2 with "Specified argument was out of the range of valid values (Parameter 'index')": a process table
  // kept in an ordered dictionary, which reads an int as a position. Every table keyed by a process id is a plain hashtable.
  it("keeps a table of processes by process id that can be read by id", () => {
    const result = runBody("accept-lib.ps1", String.raw`
      $table = Get-ProcessPathsMatching '.exe'
      $own = [int]$PID
      [ordered]@{
        type     = $table.GetType().FullName
        count    = $table.Count
        hasOwn   = $table.ContainsKey($own)
        ownPath  = [string]$table[$own]
        expected = (Get-Process -Id $PID).Path
      } | ConvertTo-Json -Compress
    `) as { type: string; count: number; hasOwn: boolean; ownPath: string; expected: string };
    expect(result.type).toBe("System.Collections.Hashtable");
    expect(result.count).toBeGreaterThan(5);
    expect(result.hasOwn).toBe(true);
    expect(result.ownPath.toLowerCase()).toBe(result.expected.toLowerCase());
  });

  it("never indexes an ordered dictionary by a process id, in any installer script", () => {
    for (const name of ["accept-fresh.ps1", "accept-over-r1.ps1", "accept-lib.ps1", "collect-r1-facts.ps1", "r1-facts-lib.ps1", "scoped-processes.ps1", "stop-install-processes.ps1", "unlink-reparse-points.ps1"]) {
      const code = codeOf(join(installer, name));
      for (const match of code.matchAll(/\$(\w+)\[\[int\]/g)) expect(code, `${name}: $${match[1]}`).not.toMatch(new RegExp(`\\$${match[1]} = \\[ordered\\]@\\{\\}`));
    }
  });

  // The helper and installer logs travel with the evidence: a failed run must carry the helper's own exception and stack.
  it("puts the helper logs into the evidence file and copies them next to it", () => {
    const root = temp();
    const logs = join(root, "logs");
    const out = join(root, "out");
    mkdirSync(logs);
    writeFileSync(join(logs, "stop-install-processes-20261115-024200-1234.log"), "02:42:00.000 started\n02:42:01.000 FAILED: System.Exception: boom\n02:42:01.001 stack: at here\n");
    writeFileSync(join(logs, "installer.log"), "C:\\x\\stop-install-processes.ps1 exit 1\nHELPER FAILED: boom\n");
    const result = runBody("accept-lib.ps1", String.raw`
      $env:TJOMMIE_HELPER_LOG_DIR = '${logs}'
      Start-Acceptance 'unit' '${out}'
      Assert-Check $true 'a check' | Out-Null
      Save-Acceptance
      $saved = Get-Content -LiteralPath '${join(out, "installer-acceptance-unit.json")}' -Raw | ConvertFrom-Json
      [ordered]@{
        files   = @($saved.helperLogs | ForEach-Object { $_.file })
        failed  = @($saved.helperLogs | ForEach-Object { $_.lines } | Where-Object { $_ -match 'FAILED' })
        copied  = @(Get-ChildItem '${join(out, "helper-logs")}' | ForEach-Object { $_.Name })
      } | ConvertTo-Json -Compress
    `) as { files: string[]; failed: string[]; copied: string[] };
    expect(result.files).toEqual(["installer.log", "stop-install-processes-20261115-024200-1234.log"]);
    expect(result.failed.join("\n")).toContain("FAILED: System.Exception: boom");
    expect(result.failed.join("\n")).toContain("HELPER FAILED: boom");
    expect(result.copied).toEqual(result.files);
  });

  it("skips a step once the run's time budget is spent, and says so, instead of running on", () => {
    const out = temp();
    const marker = join(out, "ran.txt");
    const result = runBody("accept-lib.ps1", String.raw`
      Start-Acceptance 'unit' '${out}' 0
      Start-Sleep -Milliseconds 50
      Invoke-Step 'a step after the budget' { Set-Content -LiteralPath '${marker}' 'it ran' } | Out-Null
      $saved = Get-Content -LiteralPath '${join(out, "installer-acceptance-unit.json")}' -Raw | ConvertFrom-Json
      [ordered]@{ failed = $saved.failed; message = [string]@($saved.checks | ForEach-Object { $_.check })[0] } | ConvertTo-Json -Compress
    `) as { failed: number; message: string };
    expect(result.failed).toBe(1);
    expect(result.message).toMatch(/was skipped: this run's time budget of 0 minutes is spent/);
    expect(existsSync(marker)).toBe(false);
  });

  it("does not wait out the whole limit for an uninstall that cannot finish (the fresh job crawled for that)", () => {
    const lib = codeOf(join(installer, "accept-lib.ps1"));
    const wait = lib.slice(lib.indexOf("function Invoke-RegisteredUninstall"), lib.indexOf("# ---- processes"));
    expect(wait).toContain("Get-ProcessPathsMatching '~nsu'"); // the uninstaller's temp copy runs from %TEMP%\~nsu*.tmp
    expect(wait).toMatch(/elseif \(\$sawUninstaller -or \$watch\.Elapsed\.TotalSeconds -gt 20\) \{ break \}/);
    expect(wait).not.toMatch(/Wait-Until \{ -not \(Test-Path -LiteralPath \$Folder\) \} \$Seconds/);
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
    // The logs the installer and its helpers wrote travel with it: a failed helper must leave its exception and its decisions.
    expect(upload).toContain("installer-acceptance/helper-logs");
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
