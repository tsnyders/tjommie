import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

// Each test starts PowerShell (one to four times), which is slow on a loaded machine.
vi.setConfig({ testTimeout: 60_000 });

// T8.1: the scripts that record what a published R1 install looks like (installer/*.ps1, run by
// .github/workflows/desktop-acceptance.yml on a GitHub-hosted runner). The readers are tested here against fixtures; the
// scripts that install and uninstall are only parse-checked and shown to refuse to run anywhere but a hosted runner. Nothing
// in this file installs or uninstalls anything, or looks at a Tjommie location.

const repo = resolve(import.meta.dirname, "..");
const installer = join(repo, "installer");
const windows = process.platform === "win32";

const shell = ((): string => {
  for (const candidate of ["pwsh", "powershell"]) {
    if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
  }
  return "powershell";
})();

const folders: string[] = [];
function temp(): string {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-r1-facts-test-"));
  folders.push(folder);
  return folder;
}
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); });

type Run = { status: number | null; stdout: string; stderr: string };

// Runs a script from a file, with this environment (not ours: a CI runner's own GITHUB_ACTIONS must not leak in).
function powershell(file: string, args: string[], env: NodeJS.ProcessEnv): Run {
  const run = spawnSync(shell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", file, ...args], { encoding: "utf8", windowsHide: true, env });
  return { status: run.status, stdout: run.stdout, stderr: run.stderr };
}

// A script of our own that dot-sources the readers, runs `body` and prints its result as JSON.
function readers(body: string): unknown {
  const file = join(temp(), "run.ps1");
  writeFileSync(file, `$ErrorActionPreference = 'Stop'\n. '${join(installer, "r1-facts-lib.ps1")}'\n${body}\n`);
  const run = powershell(file, [], { ...process.env });
  if (run.status !== 0) throw new Error(`PowerShell failed (${run.status}): ${run.stderr || run.stdout}`);
  return JSON.parse(run.stdout) as unknown;
}

// The same, with the process-scoping functions (installer/scoped-processes.ps1) loaded instead.
function scoped(body: string): unknown {
  const file = join(temp(), "run.ps1");
  writeFileSync(file, `$ErrorActionPreference = 'Stop'
. '${join(installer, "scoped-processes.ps1")}'
${body}
`);
  const run = powershell(file, [], { ...process.env });
  if (run.status !== 0) throw new Error(`PowerShell failed (${run.status}): ${run.stderr || run.stdout}`);
  return JSON.parse(run.stdout) as unknown;
}

const scripts = existsSync(installer) ? readdirSync(installer).filter((name) => name.endsWith(".ps1")) : [];

describe.skipIf(!windows)("the R1 install-facts scripts", () => {
  it("has the scripts the workflow runs", () => {
    expect(scripts).toEqual(expect.arrayContaining(["collect-r1-facts.ps1", "hosted-runner.ps1", "install-latest-r1.ps1", "r1-facts-lib.ps1", "scoped-processes.ps1"]));
  });

  // The second defence, behind the hosted-runner guard: processes are stopped by where they run from, never by name.
  // These tests start only stubs they make themselves (copies of ping.exe and cmd.exe under the name Tjommie.exe, in temp
  // folders), and never touch a real Tjommie: the stop function is only ever pointed at a stub folder.
  it("stops only what runs from the install folder, and its children, and leaves a same-named stub elsewhere alone", () => {
    const root = temp();
    const inside = join(root, "install");
    const outside = join(root, "somewhere else");
    mkdirSync(inside);
    mkdirSync(outside);
    const result = scoped(String.raw`
      $system = "$env:SystemRoot\System32"
      Copy-Item "$system\cmd.exe" '${join(inside, "Tjommie.exe")}'
      Copy-Item "$system\ping.exe" '${join(inside, "tjommie-daemon.exe")}'
      Copy-Item "$system\ping.exe" '${join(outside, "Tjommie.exe")}'
      Copy-Item "$system\ping.exe" '${join(outside, "tjommie-daemon.exe")}'
      $started = @()
      try {
        # Outside the folder, with the same names: these must survive.
        $started += Start-Process -FilePath '${join(outside, "Tjommie.exe")}' -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
        $started += Start-Process -FilePath '${join(outside, "tjommie-daemon.exe")}' -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
        # Inside: a Tjommie.exe that starts a child (ping, which lives in System32, outside the folder), and a daemon.
        $app = Start-Process -FilePath '${join(inside, "Tjommie.exe")}' -ArgumentList '/c','ping -n 60 127.0.0.1' -WindowStyle Hidden -PassThru
        $daemon = Start-Process -FilePath '${join(inside, "tjommie-daemon.exe")}' -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
        $started += $app, $daemon
        $child = $null
        for ($i = 0; $i -lt 50 -and -not $child; $i++) { Start-Sleep -Milliseconds 100; $child = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($app.Id)" | Select-Object -First 1 }
        if (-not $child) { throw 'The stub app never started its child.' }
        $stopped = @(Stop-ProcessesInFolder '${inside}')
        Start-Sleep -Milliseconds 700
        $alive = { param($id) [bool](Get-Process -Id $id -ErrorAction SilentlyContinue) }
        [ordered]@{
          stopped        = @($stopped | ForEach-Object { $_['id'] })
          appId          = $app.Id
          daemonId       = $daemon.Id
          childId        = [int]$child.ProcessId
          outsideIds     = @($started[0].Id, $started[1].Id)
          outsideApp     = (& $alive $started[0].Id)
          outsideDaemon  = (& $alive $started[1].Id)
          insideApp      = (& $alive $app.Id)
          insideDaemon   = (& $alive $daemon.Id)
          insideChild    = (& $alive ([int]$child.ProcessId))
        } | ConvertTo-Json -Compress
      } finally {
        # Only the stubs this test started, by id, and the child it saw.
        foreach ($process in $started) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        if ($child) { Stop-Process -Id ([int]$child.ProcessId) -Force -ErrorAction SilentlyContinue }
      }
    `) as { stopped: number[]; appId: number; daemonId: number; childId: number; outsideIds: number[]; outsideApp: boolean; outsideDaemon: boolean; insideApp: boolean; insideDaemon: boolean; insideChild: boolean };
    expect(result.insideApp).toBe(false);
    expect(result.insideDaemon).toBe(false);
    expect(result.insideChild).toBe(false); // the child of the stub app, though its own executable is outside the folder
    expect(result.outsideApp).toBe(true); // same name, other folder
    expect(result.outsideDaemon).toBe(true);
    // The stub app, its daemon and its child are stopped (and anything the app's console started, which is its child too); the
    // same-named stubs elsewhere are not on the list.
    expect(result.stopped).toEqual(expect.arrayContaining([result.appId, result.daemonId, result.childId]));
    for (const id of result.outsideIds) expect(result.stopped).not.toContain(id);
  });

  // Pure checks: a refused folder stops nothing, because the function that stops things calls this first.
  it("refuses a folder that is a drive, a system or profile folder, or holds one, and accepts an install folder", () => {
    const local = process.env.LOCALAPPDATA ?? "C:\\Users\\alice\\AppData\\Local";
    const result = scoped(String.raw`
      function Check([string]$Folder) { try { Assert-NarrowFolder $Folder; 'accepted' } catch { 'refused' } }
      $cases = [ordered]@{}
      foreach ($folder in @('', 'C:\', 'C:\Users', $env:SystemRoot, $env:USERPROFILE, (Split-Path $env:USERPROFILE -Parent), $env:LOCALAPPDATA, (Split-Path $env:LOCALAPPDATA -Parent), $env:APPDATA, $env:TEMP, $env:ProgramFiles, 'C:\Windows\..')) { $cases["refused: $folder"] = Check $folder }
      foreach ($folder in @((Join-Path $env:LOCALAPPDATA 'Programs\Tjommie'), (Join-Path $env:TEMP 'tjommie-stub\install'), 'D:\a\work\install')) { $cases["accepted: $folder"] = Check $folder }
      $cases | ConvertTo-Json -Compress
    `) as Record<string, string>;
    for (const [name, outcome] of Object.entries(result)) expect(outcome, name).toBe(name.startsWith("refused") ? "refused" : "accepted");
    expect(Object.keys(result)).toContain(`accepted: ${join(local, "Programs\\Tjommie")}`);
  });

  it("reads which program a command line starts, and which paths are inside a folder", () => {
    const result = scoped(String.raw`
      [ordered]@{
        quoted   = Get-CommandExecutable '"C:\Users\alice\AppData\Local\Programs\Tjommie\Uninstall Tjommie.exe" /currentuser /S'
        plain    = Get-CommandExecutable 'C:\x\u.exe /S'
        bare     = Get-CommandExecutable '  u.exe  '
        empty    = Get-CommandExecutable '   '
        unclosed = Get-CommandExecutable '"C:\x\u.exe /S'
        inside   = Test-PathInFolder 'C:\Users\alice\AppData\Local\Programs\Tjommie\Tjommie.exe' 'C:\Users\alice\AppData\Local\Programs\Tjommie'
        shouted  = Test-PathInFolder 'c:\USERS\alice\appdata\local\programs\tjommie\resources\a.exe' 'C:\Users\alice\AppData\Local\Programs\Tjommie\'
        sibling  = Test-PathInFolder 'C:\Users\alice\AppData\Local\Programs\Tjommie-old\Tjommie.exe' 'C:\Users\alice\AppData\Local\Programs\Tjommie'
        dotdot   = Test-PathInFolder 'C:\Users\alice\AppData\Local\Programs\Tjommie\..\Other\a.exe' 'C:\Users\alice\AppData\Local\Programs\Tjommie'
        elsewhere = Test-PathInFolder 'D:\Tjommie.exe' 'C:\Users\alice\AppData\Local\Programs\Tjommie'
      } | ConvertTo-Json -Compress
    `) as Record<string, string | boolean | null>;
    expect(result).toEqual({
      quoted: "C:\\Users\\alice\\AppData\\Local\\Programs\\Tjommie\\Uninstall Tjommie.exe", plain: "C:\\x\\u.exe", bare: "u.exe", empty: null, unclosed: null,
      inside: true, shouted: true, sibling: false, dotdot: false, elsewhere: false,
    });
  });

  // No script may stop, kill or look up a process by its image name: that is how a real Tjommie would be reached.
  it("never stops or looks up a process by name", () => {
    for (const name of scripts) {
      const code = readFileSync(join(installer, name), "utf8").split("\n").filter((line) => !line.trim().startsWith("#")).join("\n");
      expect(code, name).not.toMatch(/taskkill|\bpkill\b|killall|wmic/i);
      expect(code, name).not.toMatch(/\/IM\b/);
      expect(code, name).not.toMatch(/Stop-Process\s+(-Name|-InputObject|\$)/i);
      expect(code, name).not.toMatch(/Get-Process\s+(-Name|[A-Za-z'"*])/i);
      expect(code, name).not.toMatch(/Win32_Process[^\n]*Name\s*=/i);
      // Every Stop-Process names an id.
      for (const stop of code.match(/\bStop-Process\b[^\n]*/gi) ?? []) expect(stop, `${name}: ${stop}`).toMatch(/-Id\b/);
    }
  });

  it("parses: every installer script is valid PowerShell", () => {
    for (const name of scripts) {
      const path = join(installer, name);
      const run = powershell(
        (() => { const file = join(temp(), "parse.ps1"); writeFileSync(file, `$errors = $null; $tokens = $null\n[void][System.Management.Automation.Language.Parser]::ParseFile('${path}', [ref]$tokens, [ref]$errors)\nif ($errors.Count -gt 0) { $errors | ForEach-Object { Write-Output ($_.Message + ' at line ' + $_.Extent.StartLineNumber) }; exit 1 }\n`); return file; })(),
        [], { ...process.env },
      );
      expect(run.stdout.trim(), name).toBe("");
      expect(run.status, name).toBe(0);
    }
  });

  // The guard that keeps these scripts off a real PC. Each is started with an environment that is not a hosted runner's, so
  // the guard (which comes first) refuses before anything is read or installed.
  it.each(["collect-r1-facts.ps1", "install-latest-r1.ps1"])("%s refuses to run anywhere but a GitHub-hosted runner", (name) => {
    // Nothing the scripts would act on is in the environment either (no repository, no install result, no runner folders), so
    // even a script whose guard had been lost could do nothing here.
    const base = { ...process.env };
    for (const name of ["GITHUB_ACTIONS", "RUNNER_ENVIRONMENT", "GH_REPO", "GH_TOKEN", "R1_TAG", "R1_INSTALL_RESULT", "RUNNER_TEMP", "GITHUB_ENV"]) delete base[name];
    for (const env of [base, { ...base, GITHUB_ACTIONS: "true" }, { ...base, GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "self-hosted" }, { ...base, RUNNER_ENVIRONMENT: "github-hosted" }]) {
      const run = powershell(join(installer, name), [], env);
      expect(run.status, `${name}: ${run.stdout}${run.stderr}`).not.toBe(0);
      expect(`${run.stdout}${run.stderr}`).toContain("GitHub-hosted runner");
    }
  });

  it("counts a folder's files, apart from the uninstaller NSIS adds", () => {
    const dir = temp();
    mkdirSync(join(dir, "resources", "daemon"), { recursive: true });
    writeFileSync(join(dir, "Tjommie.exe"), "12345");
    writeFileSync(join(dir, "Uninstall Tjommie.exe"), "123");
    writeFileSync(join(dir, "resources", "app.asar"), "1234567");
    writeFileSync(join(dir, "resources", "daemon", "Uninstall Other.exe"), "1");
    const inventory = readers(`Get-FileInventory '${dir}' | ConvertTo-Json -Depth 6 -Compress`) as { count: number; countWithoutUninstaller: number; uninstallers: string[]; totalBytes: number; files: { path: string; bytes: number }[] };
    expect(inventory.count).toBe(4);
    // Only an uninstaller at the top of the install folder is NSIS's.
    expect(inventory.uninstallers).toEqual(["Uninstall Tjommie.exe"]);
    expect(inventory.countWithoutUninstaller).toBe(3);
    expect(inventory.totalBytes).toBe(16);
    expect(inventory.files.map((file) => file.path)).toEqual(["resources\\app.asar", "resources\\daemon\\Uninstall Other.exe", "Tjommie.exe", "Uninstall Tjommie.exe"]);
  });

  it("lists what is directly inside a folder, never its contents, and says when there is no folder", () => {
    const dir = temp();
    mkdirSync(join(dir, "speech"));
    writeFileSync(join(dir, "login-item-set"), "");
    const entries = readers(`ConvertTo-Json -InputObject @(Get-FolderEntries '${dir}') -Depth 4 -Compress`) as { name: string; type: string; bytes: number | null }[];
    expect(entries).toEqual([{ name: "login-item-set", type: "file", bytes: 0 }, { name: "speech", type: "folder", bytes: null }]);
    expect(readers(`ConvertTo-Json -InputObject @{ missing = (Get-FolderEntries '${join(dir, "nope")}') } -Compress`)).toEqual({ missing: null });
  });

  // A private key of our own under HKCU, removed afterwards: the readers are pointed at it, never at a real location.
  it("reads registry values as they are stored, and finds the app's uninstall entries", () => {
    const key = `Software\\TjommieFactsTest\\${crypto.randomUUID()}`;
    const result = readers(String.raw`
      $key = '${key}'
      $root = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($key + '\Uninstall\{GUID-1}')
      $root.SetValue('DisplayName', 'Tjommie')
      $root.SetValue('InstallLocation', 'C:\Users\alice\AppData\Local\Programs\Tjommie')
      $root.SetValue('UninstallString', '"C:\Users\alice\AppData\Local\Programs\Tjommie\Uninstall Tjommie.exe" /currentuser')
      $root.SetValue('EstimatedSize', 123456, 'DWord')
      $root.SetValue('Path', '%LOCALAPPDATA%\Programs', 'ExpandString')
      $root.SetValue('Approved', [byte[]](3,0,0,0,1,2,3,4), 'Binary')
      $root.SetValue('Many', [string[]]('a','b'), 'MultiString')
      $root.Close()
      $other = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($key + '\Uninstall\Other')
      $other.SetValue('DisplayName', 'Something else')
      $other.SetValue('InstallLocation', 'C:\Elsewhere')
      $other.Close()
      $byLocation = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($key + '\Uninstall\{GUID-2}')
      $byLocation.SetValue('DisplayName', 'Renamed')
      $byLocation.SetValue('InstallLocation', 'c:\users\alice\appdata\local\programs\tjommie\')
      $byLocation.Close()
      try {
        $fact = Get-RegistryKeyFact 'CurrentUser' ($key + '\Uninstall\{GUID-1}') '64'
        $found = Find-UninstallEntries 'CurrentUser' ($key + '\Uninstall') 'Tjommie' 'C:\Users\alice\AppData\Local\Programs\Tjommie' '64'
        $missing = Get-RegistryKeyFact 'CurrentUser' ($key + '\Nothing') '64'
        [ordered]@{ fact = $fact; found = @($found); missing = $missing } | ConvertTo-Json -Depth 8 -Compress
      } finally {
        [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree('Software\TjommieFactsTest', $false)
      }
    `) as { fact: { hive: string; view: string; subKeys: string[]; values: { name: string; kind: string; data: unknown }[] }; found: string[]; missing: null };
    const value = (name: string) => result.fact.values.find((entry) => entry.name === name);
    expect(result.fact.hive).toBe("CurrentUser");
    expect(result.fact.view).toBe("64");
    expect(value("DisplayName")).toEqual({ name: "DisplayName", kind: "String", data: "Tjommie" });
    expect(value("UninstallString")?.data).toBe(String.raw`"C:\Users\alice\AppData\Local\Programs\Tjommie\Uninstall Tjommie.exe" /currentuser`);
    expect(value("EstimatedSize")).toEqual({ name: "EstimatedSize", kind: "DWord", data: 123456 });
    expect(value("Path")).toEqual({ name: "Path", kind: "ExpandString", data: "%LOCALAPPDATA%\\Programs" }); // not expanded
    expect(value("Approved")).toEqual({ name: "Approved", kind: "Binary", data: "03 00 00 00 01 02 03 04" });
    expect(value("Many")).toEqual({ name: "Many", kind: "MultiString", data: ["a", "b"] });
    expect(result.fact.values.map((entry) => entry.name)).toEqual([...result.fact.values.map((entry) => entry.name)].sort((a, b) => a.localeCompare(b)));
    // By product name, or by install folder however it is spelled; not the entry for something else.
    expect(result.found).toEqual(["{GUID-1}", "{GUID-2}"]);
    expect(result.missing).toBeNull();
    // The private key is gone.
    expect(readers(String.raw`[ordered]@{ left = ([Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\TjommieFactsTest') -ne $null) } | ConvertTo-Json -Compress`)).toEqual({ left: false });
  });

  // A real .lnk with an AppUserModelID, made by a helper in this test (the readers only read): it shows the AUMID is read
  // the way the shell shows it, which is what T8.2's shortcuts must reproduce.
  it("reads a shortcut's target and its AppUserModelID", () => {
    const dir = temp();
    const folder = join(dir, "Start Menu");
    mkdirSync(join(folder, "Sub"), { recursive: true });
    const link = join(folder, "Tjommie.lnk");
    const result = readers(String.raw`
      Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class TestShortcut {
  [StructLayout(LayoutKind.Sequential, Pack = 4)] public struct PROPERTYKEY { public Guid fmtid; public uint pid; }
  [StructLayout(LayoutKind.Sequential)] public struct PROPVARIANT { public ushort vt; public ushort r1; public ushort r2; public ushort r3; public IntPtr p; public IntPtr p2; }
  [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
  public interface IPropertyStore {
    [PreserveSig] int GetCount(out uint count);
    [PreserveSig] int GetAt(uint index, out PROPERTYKEY key);
    [PreserveSig] int GetValue(ref PROPERTYKEY key, out PROPVARIANT value);
    [PreserveSig] int SetValue(ref PROPERTYKEY key, ref PROPVARIANT value);
    [PreserveSig] int Commit();
  }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
  static extern void SHGetPropertyStoreFromParsingName(string path, IntPtr bindContext, int flags, ref Guid iid, out IPropertyStore store);
  [DllImport("ole32.dll")] static extern int PropVariantClear(ref PROPVARIANT variant);
  public static void SetAppUserModelId(string path, string id) {
    Guid iid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");
    IPropertyStore store;
    SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, 2, ref iid, out store);
    PROPERTYKEY key = new PROPERTYKEY { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
    // A VT_LPWSTR (31) holding a CoTaskMem string, which PropVariantClear frees.
    PROPVARIANT value = new PROPVARIANT { vt = 31, p = Marshal.StringToCoTaskMemUni(id) };
    int hr = store.SetValue(ref key, ref value);
    if (hr == 0) hr = store.Commit();
    PropVariantClear(ref value);
    Marshal.ReleaseComObject(store);
    if (hr != 0) throw new Exception("Could not set the AppUserModelID: 0x" + hr.ToString("x"));
  }
}
'@
      $wsh = New-Object -ComObject WScript.Shell
      $link = $wsh.CreateShortcut('${link}')
      $link.TargetPath = 'C:\Windows\System32\cmd.exe'
      $link.Arguments = '/c exit'
      $link.WorkingDirectory = 'C:\Windows'
      $link.Description = 'Tjommie'
      $link.Save()
      [TestShortcut]::SetAppUserModelId('${link}', 'com.example.facts-test')
      $plain = $wsh.CreateShortcut('${join(folder, "Sub", "Plain.lnk")}')
      $plain.TargetPath = 'C:\Windows\System32\notepad.exe'
      $plain.Save()
      ConvertTo-Json -InputObject @(Get-ShortcutFacts '${folder}') -Depth 4 -Compress
    `) as { path: string; target: string; arguments: string; workingDirectory: string; description: string; appUserModelId: string; toastActivatorClsid: string }[];
    expect(result.map((entry) => entry.path)).toEqual(["Sub\\Plain.lnk", "Tjommie.lnk"]);
    const tjommie = result.find((entry) => entry.path === "Tjommie.lnk");
    expect(tjommie).toMatchObject({ target: "C:\\Windows\\System32\\cmd.exe", arguments: "/c exit", workingDirectory: "C:\\Windows", description: "Tjommie", appUserModelId: "com.example.facts-test", toastActivatorClsid: "" });
    // A shortcut with no AppUserModelID reads as an empty one, not an error.
    expect(result.find((entry) => entry.path === "Sub\\Plain.lnk")?.appUserModelId).toBe("");
    expect(readers(`ConvertTo-Json -InputObject @(Get-ShortcutFacts '${join(dir, "nowhere")}') -Compress`)).toEqual([]);
  });

  it("reads an executable's version resource, the path tokens and the shell's shortcut folders", () => {
    const cmd = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "cmd.exe");
    const result = readers(`[ordered]@{ version = (Get-ExeVersionFact '${cmd}'); tokens = (Get-PathTokens); folders = (Get-ShortcutFolders) } | ConvertTo-Json -Depth 4 -Compress`) as { version: { originalFilename: string; productName: string }; tokens: Record<string, string>; folders: Record<string, string> };
    expect(result.version.originalFilename.toLowerCase()).toContain("cmd.exe");
    expect(result.version.productName).toContain("Windows");
    expect(Object.keys(result.tokens)).toEqual(expect.arrayContaining(["%LOCALAPPDATA%", "%APPDATA%", "%USERPROFILE%"]));
    expect(result.folders.userStartMenu.toLowerCase()).toContain("start menu\\programs");
    expect(Object.keys(result.folders)).toEqual(["userStartMenu", "userDesktop", "allUsersStartMenu", "allUsersDesktop"]);
  });

  it("sees the processes that start while something runs, with their command lines", () => {
    const result = readers(String.raw`
      $before = @(Get-Process | ForEach-Object { $_.Id })
      $child = Start-Process -FilePath "$env:SystemRoot\System32\ping.exe" -ArgumentList '-n','4','127.0.0.1' -WindowStyle Hidden -PassThru
      try {
        Start-Sleep -Milliseconds 300
        ConvertTo-Json -InputObject @(Get-NewProcessFacts $before) -Depth 4 -Compress
      } finally { Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue }
    `) as { name: string; commandLine: string }[];
    const ping = result.find((entry) => entry.name.toLowerCase() === "ping.exe");
    expect(ping?.commandLine).toContain("-n 4 127.0.0.1");
  });
});

describe("the desktop-acceptance workflow", () => {
  const workflow = readFileSync(join(repo, ".github", "workflows", "desktop-acceptance.yml"), "utf8");

  it("runs on desktop-acceptance branches or by hand, never on a pull request, and only reads the repository", () => {
    expect(workflow).toMatch(/branches: \['desktop-acceptance-\*'\]/);
    expect(workflow).toContain("workflow_dispatch:");
    expect(workflow).not.toMatch(/pull_request/);
    expect(workflow).toMatch(/permissions:\s+contents: read/);
    expect(workflow).toContain("runs-on: windows-latest");
  });

  it("installs the latest published R1 and uploads the facts as r1-install-facts", () => {
    expect(workflow).toContain("install-latest-r1.ps1");
    expect(workflow).toContain("collect-r1-facts.ps1");
    expect(workflow).toMatch(/name: r1-install-facts\b/);
    expect(workflow).not.toMatch(/PRODUCTION_VERSION|0\.5\.0/); // the latest release, not a version written here
  });

  it("names only scripts that exist, and never calls an install script without the guard", () => {
    for (const script of workflow.match(/installer\/[a-z0-9-]+\.ps1/g) ?? []) expect(existsSync(join(repo, script)), script).toBe(true);
    for (const name of ["collect-r1-facts.ps1", "install-latest-r1.ps1"]) {
      // Comments may name these commands; only the code counts.
      const text = readFileSync(join(installer, name), "utf8").split("\n").filter((line) => !line.trim().startsWith("#")).join("\n");
      const guard = text.indexOf("Assert-HostedRunner");
      expect(guard, name).toBeGreaterThan(-1);
      // Before anything is installed, killed or read from a Tjommie location.
      for (const action of ["Start-Process", "taskkill", "gh release", "Get-ChildItem"]) {
        const first = text.indexOf(action);
        if (first !== -1) expect(guard, `${name}: ${action}`).toBeLessThan(first);
      }
    }
  });
});
