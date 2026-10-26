import { execFileSync, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { getMakeNsisPath, getNsisPluginsPath } from "app-builder-lib/out/toolsets/windows.js";
import { expect, it } from "vitest";
import { nsisSpawnOptions, testIdentity, testNsisInclude } from "../scripts/update-test-guards.ts";

it.skipIf(process.platform !== "win32")("compiled test init hooks refuse unsafe targets before the sentinel can be removed", async () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-guard-nsis-"));
  const install = join(root, testIdentity.product), sentinel = join(root, "must-survive");
  const fixtureKey = `Software\\TjommieGuardFixtures\\${randomUUID()}`;
  const reg = (args: string[]): void => { execFileSync("reg.exe", args, { windowsHide: true, stdio: "pipe" }); };
  const put = (key: string, name: string, value: string): void => reg(["add", `HKCU\\${fixtureKey}\\${key}`, "/v", name, "/t", "REG_SZ", "/d", value, "/f", "/reg:64"]);
  try {
    mkdirSync(install);
    writeFileSync(sentinel, "unchanged");
    // Only this fixture's keys are queried, even by the compiled NSIS process.
    const include = testNsisInclude(install)
      .replaceAll(`Software\\${testIdentity.guid}`, `${fixtureKey}\\Install`)
      .replaceAll(`Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${testIdentity.guid}`, `${fixtureKey}\\Uninstall`)
      .replace("!macro testAbort", `!macro testAbort\nFileOpen $R4 "${join(root, "failure.txt")}" w\nFileWrite $R4 "$INSTDIR|$R0|$R1|$R2|$R3|$CMDLINE"\nFileClose $R4`);
    writeFileSync(join(root, "guard.nsh"), include);
    const compiler = await getMakeNsisPath(undefined);
    for (const hook of ["testTarget", "customInit", "customUnInit"]) {
      const exe = join(root, `${hook}.exe`), source = join(root, `${hook}.nsi`);
      writeFileSync(source, `Unicode true
!include LogicLib.nsh
!include FileFunc.nsh
!define isUpdated '1 = 1'
!include "${join(root, "guard.nsh")}"
Name "Tjommie guard fixture"
OutFile "${exe}"
InstallDir "${install}"
RequestExecutionLevel user
SilentInstall silent
Function .onInit
  !insertmacro ${hook}
FunctionEnd
Section
  Delete "${sentinel}"
SectionEnd
`);
      execFileSync(compiler.path, ["-V2", source], { env: { ...process.env, ...compiler.env }, windowsHide: true, stdio: "pipe" });
      const attempt = (target = install): void => {
        const result = spawnSync(exe, ["/S", `/D=${target}`], { windowsHide: true, ...nsisSpawnOptions(exe), timeout: 15_000 });
        if (result.error) throw result.error;
        expect(result.status).toBe(91);
        expect(readFileSync(sentinel, "utf8")).toBe("unchanged");
      };
      if (hook === "testTarget") {
        // Isolate the path check: no later registry guard may hide a failure here.
        rmSync(install, { recursive: true, force: true });
        attempt();
        mkdirSync(install);
        continue;
      }
      // No registration (and a pinned /D) must still refuse an update/uninstall.
      attempt();
      put("Install", "InstallLocation", root);
      put("Uninstall", "UninstallString", `"${join(install, `Uninstall ${testIdentity.product}.exe`)}" /currentuser`);
      attempt();
      put("Install", "InstallLocation", install);
      put("Uninstall", "UninstallString", `"${join(install, "Uninstall Tjommie.exe")}" /currentuser`);
      attempt();
      put("Uninstall", "UninstallString", `"${join(install, `Uninstall ${testIdentity.product}.exe`)}" /currentuser`);
      attempt(root);
      // Prove the sentinel was reachable after a successful guard, not an inert fixture.
      const allowed = spawnSync(exe, ["/S", `/D=${install}`], { windowsHide: true, ...nsisSpawnOptions(exe), timeout: 15_000 });
      if (allowed.error) throw allowed.error;
      expect(allowed.status, readFileSync(join(root, "failure.txt"), "utf8")).toBe(0);
      expect(existsSync(sentinel)).toBe(false);
      writeFileSync(sentinel, "unchanged");
      reg(["delete", `HKCU\\${fixtureKey}`, "/f", "/reg:64"]);
    }
    rmSync(join(root, "failure.txt"));
    const factory = join(root, "uninstaller-factory.exe"), source = join(root, "uninstaller.nsi");
    const uninstaller = join(install, `Uninstall ${testIdentity.product}.exe`);
    const templates = resolve("node_modules/app-builder-lib/templates/nsis");
    writeFileSync(source, `Unicode true
!addincludedir "${templates}"
!addincludedir "${join(templates, "include")}"
!addplugindir /x86-unicode "${join(await getNsisPluginsPath(undefined), "x86-unicode")}"
!include LogicLib.nsh
!include StdUtils.nsh
!define INSTALL_REGISTRY_KEY "${fixtureKey}\\Install"
!define UNINSTALL_REGISTRY_KEY "${fixtureKey}\\Uninstall"
!define APP_FILENAME "${testIdentity.packageName}"
!include "${join(templates, "multiUser.nsh")}"
!include "${join(root, "guard.nsh")}"
Name "Tjommie uninstaller fixture"
OutFile "${factory}"
InstallDir "${install}"
RequestExecutionLevel user
SilentInstall silent
Section
  WriteUninstaller "${uninstaller}"
SectionEnd
Function un.onInit
  SetRegView 64
  !insertmacro setInstallModePerUser
  !insertmacro customUnInit
FunctionEnd
Section Uninstall
  Delete "${sentinel}"
SectionEnd
`);
    execFileSync(compiler.path, ["-V2", source], { env: { ...process.env, ...compiler.env }, windowsHide: true, stdio: "pipe" });
    execFileSync(factory, [], { windowsHide: true, stdio: "pipe" });
    const uninstall = async (args: string[]): Promise<void> => {
      execFileSync(uninstaller, args, { windowsHide: true, ...nsisSpawnOptions(uninstaller), stdio: "pipe" });
      for (let attempt = 0; attempt < 30 && existsSync(sentinel) && !existsSync(join(root, "failure.txt")); attempt++) {
        await new Promise((done) => setTimeout(done, 100));
      }
    };
    // Reproduce the historical /S-only path: initMultiUser has no registration and falls back.
    await uninstall(["/S"]);
    expect(existsSync(join(root, "failure.txt"))).toBe(true);
    expect(readFileSync(join(root, "failure.txt"), "utf8")).toContain(`\\${testIdentity.packageName}|`);
    expect(readFileSync(sentinel, "utf8")).toBe("unchanged");
    rmSync(join(root, "failure.txt"));
    put("Install", "InstallLocation", install);
    put("Uninstall", "UninstallString", `"${uninstaller}" /currentuser`);
    await uninstall(["/S", `/D=${install}`]);
    expect(existsSync(sentinel), existsSync(join(root, "failure.txt")) ? readFileSync(join(root, "failure.txt"), "utf8") : "uninstaller did not finish").toBe(false);
  } finally {
    // Never widen deletion to the parent namespace or a production key.
    if (!fixtureKey.startsWith("Software\\TjommieGuardFixtures\\")) throw new Error("Unexpected fixture key.");
    spawnSync("reg.exe", ["delete", `HKCU\\${fixtureKey}`, "/f", "/reg:64"], { windowsHide: true, stdio: "ignore" });
    if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Unexpected fixture directory.");
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
}, 60_000);
