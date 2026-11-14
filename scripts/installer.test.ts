import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { installer, powershell, runBody, temp, windows } from "./ps-support.ts";

// T8.2: the NSIS installer (installer/tjommie.nsi) and the scripts it runs. Nothing in this file runs an installer or an
// uninstaller, here or anywhere but a GitHub-hosted runner: it reads the script's text, compiles it when a compiler is
// there (compiling only writes an .exe into a temp folder), and starts PowerShell helpers against stubs it makes itself.

vi.setConfig({ testTimeout: 90_000 });

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
