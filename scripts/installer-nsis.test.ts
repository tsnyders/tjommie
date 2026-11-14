import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { codeOf, installer, powershell, repo, temp, windows } from "./ps-support.ts";

// T8.2: installer/tjommie.nsi against what T8.1 recorded from a real R1 install and what Core's identity table says. This reads
// the script's text and compiles it when a compiler is there (compiling only writes an installer into a temp folder). Nothing
// here runs an installer or an uninstaller: those run on a GitHub-hosted runner, behind the hosted-runner guard.

vi.setConfig({ testTimeout: 120_000 });

type RegistryValue = { name: string; kind: string; data: string | number };
type RegistryKey = { key: string; values: RegistryValue[] };
type Shortcut = { path: string; target: string; arguments: string; workingDirectory: string; iconLocation: string; description: string; windowStyle: number; appUserModelId: string; toastActivatorClsid: string };
type Facts = {
  release: { version: string };
  install: { folderAsFound: string; exe: string; exeVersion: { productName: string } };
  registry: { hkcuUninstallView64: RegistryKey[]; hkcuInstallKeys: RegistryKey[] };
  shortcuts: { userStartMenu: Shortcut[]; userDesktop: Shortcut[]; allUsersStartMenu: Shortcut[]; allUsersDesktop: Shortcut[] };
  firstStart: { run: RegistryKey };
  upgrade: { processes: Array<{ name: string; commandLine: string | null }> };
};
type Row = { appId: string; guid: string; product: string; exe: string; folder: string; root: string };
type Table = { namespace: string; publisher: string; comments: string; identities: Record<string, Row> };

function readJson<T>(path: string): T {
  const text = readFileSync(path, "utf8");
  return JSON.parse(text.charCodeAt(0) === 0xfeff ? text.slice(1) : text) as T;
}
const facts = readJson<Facts>(join(installer, "r1-install-facts.json"));
const table = readJson<Table>(join(installer, "identities.json"));
const production = table.identities.production as Row;
const script = readFileSync(join(installer, "tjommie.nsi"), "utf8").replace(/\r\n/g, "\n");
const nsi = codeOf(join(installer, "tjommie.nsi")).replace(/\r\n/g, "\n");

// electron-builder's GUID for an app id: a name-based (v5) UUID in its own namespace.
function uuidV5(name: string, namespace: string): string {
  const hash = createHash("sha1").update(Buffer.concat([Buffer.from(namespace.replace(/-/g, ""), "hex"), Buffer.from(name, "utf8")])).digest();
  hash[6] = ((hash[6] ?? 0) & 0x0f) | 0x50;
  hash[8] = ((hash[8] ?? 0) & 0x3f) | 0x80;
  const hex = hash.subarray(0, 16).toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

// What the script's text becomes for production at R1's version, in R1's install folder: its defines and $INSTDIR filled in.
const installFolder = facts.install.folderAsFound;
const defines: Record<string, string> = {
  APP_ID: production.appId, APP_GUID: production.guid, PRODUCT_NAME: production.product, EXE_NAME: production.exe, FOLDER_NAME: production.folder,
  VERSION: facts.release.version, PUBLISHER: table.publisher, COMMENTS: table.comments, UNINSTALLER: `Uninstall ${production.product}.exe`,
};
const render = (text: string): string => text.replace(/\$\{(\w+)\}/g, (whole, name: string) => defines[name] ?? whole).replaceAll("$INSTDIR", installFolder);

// The text between the quotes of an NSIS argument (double or single quotes).
function unquote(raw: string): string {
  const text = raw.trim();
  return (text.startsWith('"') && text.endsWith('"')) || (text.startsWith("'") && text.endsWith("'")) ? text.slice(1, -1) : text;
}

// Every WriteRegStr / WriteRegDWORD of one key, as { name, kind, text }.
function written(keyDefine: string): Array<{ name: string; kind: string; text: string }> {
  const pattern = new RegExp(`^\\s*WriteReg(Str|DWORD)\\s+HKCU\\s+"\\$\\{${keyDefine}\\}"\\s+"([^"]+)"\\s+(.+)$`, "gm");
  return [...nsi.matchAll(pattern)].map((match) => ({ name: match[2] ?? "", kind: match[1] === "DWORD" ? "DWord" : "String", text: (match[3] ?? "").trim() }));
}

describe("the identity table", () => {
  it("is production's R1 facts exactly: GUID, folder, app id, exe, product, publisher and comments", () => {
    const uninstall = facts.registry.hkcuUninstallView64[0] as RegistryKey;
    const value = (name: string): string | number | undefined => uninstall.values.find((entry) => entry.name === name)?.data;
    expect(uninstall.key.split("\\").pop()).toBe(production.guid);
    expect(facts.registry.hkcuInstallKeys[0]?.key).toBe(`Software\\${production.guid}`);
    expect(installFolder.split("\\").pop()).toBe(production.folder);
    expect(installFolder.toLowerCase()).toContain("\\appdata\\local\\programs\\tjommie");
    expect(facts.install.exe).toBe(production.exe);
    expect(facts.install.exeVersion.productName).toBe(production.product);
    expect(value("Publisher")).toBe(table.publisher);
    expect(value("Comments")).toBe(table.comments);
    expect(facts.shortcuts.userStartMenu.find((shortcut) => shortcut.path === "Tjommie.lnk")?.appUserModelId).toBe(production.appId);
    expect(facts.firstStart.run.values[0]?.name).toBe(production.appId);
    expect(production.root).toBe("programs");
  });

  it("derives every GUID from its app id the way electron-builder does, so a wrong one cannot be typed in", () => {
    for (const [name, row] of Object.entries(table.identities)) expect(row.guid, name).toBe(uuidV5(row.appId, table.namespace));
    // R1's own update-test identity, in its own guard table, is the same GUID.
    const guards = readFileSync(join(repo, "packages", "desktop", "scripts", "update-test-guards.ts"), "utf8");
    expect(guards).toContain(`guid: "${table.identities.updatetest?.guid}"`);
  });

  // Core's identity table (spec section 4) says what each app is called; the installer must agree with it.
  it("agrees with Core's identity table on app id, product and exe", () => {
    const rust = readFileSync(join(repo, "tjommie-rs", "crates", "desktop-host", "src", "identity.rs"), "utf8");
    const blocks = new Map([...rust.matchAll(/pub const (\w+): Identity = Identity \{([\s\S]*?)\n\};/g)].map((match) => [match[1] ?? "", match[2] ?? ""] as const));
    const field = (block: string, name: string): string => new RegExp(`\\b${name}: "([^"]+)"`).exec(block)?.[1] ?? "";
    for (const [name, constant] of [["production", "PRODUCTION"], ["dev", "DEV"], ["updatetest", "UPDATE_TEST"]] as const) {
      const block = blocks.get(constant) ?? "";
      const row = table.identities[name] as Row;
      expect({ appId: row.appId, product: row.product, exe: row.exe }, name).toEqual({ appId: field(block, "app_id"), product: field(block, "product"), exe: field(block, "exe") });
    }
  });

  // The test that production's install cannot be reached from the dev or update-test installer: nothing they write, name or
  // delete is shared with it.
  it("gives dev and updatetest a GUID, folder, product, exe, app id, shortcut and registry keys of their own", () => {
    const rows = Object.entries(table.identities);
    expect(rows.map(([name]) => name).sort()).toEqual(["dev", "production", "updatetest"]);
    const unique = (pick: (row: Row) => string): number => new Set(rows.map(([, row]) => pick(row).toLowerCase())).size;
    const things: Array<[string, (row: Row) => string]> = [
      ["GUID", (row) => row.guid], ["folder", (row) => row.folder], ["product", (row) => row.product], ["exe", (row) => row.exe],
      ["app id", (row) => row.appId], ["shortcut", (row) => `${row.product}.lnk`], ["uninstaller", (row) => `Uninstall ${row.product}.exe`],
      ["Uninstall key", (row) => `Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${row.guid}`], ["install key", (row) => `Software\\${row.guid}`],
      ["Run value", (row) => row.appId],
    ];
    for (const [label, pick] of things) expect(unique(pick), label).toBe(rows.length);
    for (const [name, row] of rows) expect(row.root === "programs" || row.root === "temp", name).toBe(true);
    expect(table.identities.updatetest?.root).toBe("temp"); // R1's update test installs under %TEMP%
  });
});

describe("tjommie.nsi against R1's recorded install", () => {
  it("writes the Uninstall key's values: the same names, types and data R1 wrote, and no others", () => {
    const uninstall = facts.registry.hkcuUninstallView64[0] as RegistryKey;
    const ours = written("UNINSTALL_KEY");
    expect(ours.map((entry) => entry.name).sort()).toEqual(uninstall.values.map((entry) => entry.name).sort());
    for (const recorded of uninstall.values) {
      const entry = ours.find((candidate) => candidate.name === recorded.name);
      expect(entry?.kind, recorded.name).toBe(recorded.kind);
      // The estimated size is the one value that cannot match: it is the size of the install, measured when it is made.
      if (recorded.name === "EstimatedSize") { expect(entry?.text).toBe("$R0"); continue; }
      const text = recorded.kind === "DWord" ? entry?.text : render(unquote(entry?.text ?? ""));
      expect(text, recorded.name).toBe(String(recorded.data));
    }
  });

  it("writes the install key HKCU\\Software\\<GUID> with the values R1 wrote", () => {
    const recorded = (facts.registry.hkcuInstallKeys[0] as RegistryKey).values;
    const ours = written("INSTALL_KEY");
    expect(ours.map((entry) => entry.name).sort()).toEqual(recorded.map((entry) => entry.name).sort());
    for (const value of recorded) expect(render(unquote(ours.find((entry) => entry.name === value.name)?.text ?? "")), value.name).toBe(String(value.data));
  });

  it("makes the two user shortcuts R1 made, with the same target, icon, description and AppUserModelID, and none for all users", () => {
    const call = /CreateShortCut\s+"\$R0"\s+"([^"]*)"\s+"([^"]*)"\s+"([^"]*)"\s+(\d+)\s+(\w+)\s+"([^"]*)"\s+"([^"]*)"/.exec(nsi);
    expect(call, "CreateShortCut in MakeShortcut").not.toBeNull();
    const recorded = facts.shortcuts.userStartMenu.find((shortcut) => shortcut.path === "Tjommie.lnk") as Shortcut;
    expect(facts.shortcuts.userDesktop.map((shortcut) => shortcut.path)).toContain("Tjommie.lnk");
    expect(facts.shortcuts.allUsersStartMenu.some((shortcut) => /tjommie/i.test(shortcut.path))).toBe(false);
    expect(recorded.toastActivatorClsid).toBe(""); // R1 sets none, so notifications are keyed by the AppUserModelID alone
    expect(render(call?.[1] ?? "")).toBe(recorded.target);
    expect(render(call?.[2] ?? "")).toBe(recorded.arguments);
    expect(`${render(call?.[3] ?? "")},${call?.[4]}`).toBe(recorded.iconLocation);
    expect(call?.[5]).toBe("SW_SHOWNORMAL");
    expect(recorded.windowStyle).toBe(1);
    expect(render(call?.[7] ?? "")).toBe(recorded.description);
    // The working directory is the folder NSIS is in when it makes the shortcut: the install folder.
    expect(recorded.workingDirectory).toBe(installFolder);
    const beforeFirstShortcut = nsi.slice(0, nsi.indexOf("Call MakeShortcut"));
    expect([...beforeFirstShortcut.matchAll(/SetOutPath\s+"([^"]+)"/g)].pop()?.[1]).toBe("$INSTDIR");
    expect(nsi).toContain('"$SMPROGRAMS\\${PRODUCT_NAME}.lnk"');
    expect(nsi).toContain('"$DESKTOP\\${PRODUCT_NAME}.lnk"');
    expect(nsi).toContain("SetShellVarContext current");
    expect(nsi).not.toMatch(/SetShellVarContext\s+all/i);
    expect(nsi).toMatch(/set-shortcut-aumid\.ps1" -Path "\$R0" -AppUserModelId "\$\{APP_ID\}"/);
  });

  it("runs the old uninstaller the way R1's own upgrade does", () => {
    const old = facts.upgrade.processes.find((process) => process.name === "old-uninstaller.exe" && process.commandLine);
    expect(old?.commandLine).toBeTruthy();
    const recordedArguments = (old?.commandLine ?? "").replace(/^"[^"]+"\s*/, "");
    const call = /ExecWait\s+'"\$PLUGINSDIR\\old-uninstaller\.exe"\s+([^']+)'/.exec(nsi);
    expect(render(call?.[1] ?? "")).toBe(recordedArguments);
    // From a temp copy, not in the install folder, and only if the install has one.
    expect(nsi).toContain('CopyFiles /SILENT "$INSTDIR\\${UNINSTALLER}" "$PLUGINSDIR\\old-uninstaller.exe"');
    expect(nsi).toContain('${If} ${FileExists} "$INSTDIR\\${UNINSTALLER}"');
  });

  it("names the start-at-login value after the app id and removes it only when it points into the install folder", () => {
    expect(facts.firstStart.run.values[0]?.name).toBe(production.appId);
    expect(String(facts.firstStart.run.values[0]?.data)).toBe(`"${installFolder}\\${production.exe}" --hidden`);
    expect(script).toMatch(/^!define REMOVE_RUN_VALUE_ON_UNINSTALL$/m); // the one-line switch, at the top level
    const removal = nsi.slice(nsi.indexOf("!ifdef REMOVE_RUN_VALUE_ON_UNINSTALL"), nsi.indexOf("DeleteRegKey HKCU"));
    expect(removal).toContain('ReadRegStr $R0 HKCU "${RUN_KEY}" "${APP_ID}"');
    expect(removal).toContain('${AndIf} $R3 == "$INSTDIR\\"');
    expect(removal).toContain('DeleteRegValue HKCU "${RUN_KEY}" "${APP_ID}"');
    expect(nsi.match(/DeleteRegValue/g)).toHaveLength(1);
    // Never during an update, which keeps the value for the installer that follows.
    expect(nsi.indexOf("${If} $Updated != 1")).toBeLessThan(nsi.indexOf("!ifdef REMOVE_RUN_VALUE_ON_UNINSTALL"));
  });

  it("accepts --updated and --force-run, starts the app only for --force-run or a normal install, and starts it with --updated for an update", () => {
    expect(nsi).toContain('"--updated"');
    expect(nsi).toContain('"--force-run"');
    const success = nsi.slice(nsi.indexOf("Function .onInstSuccess"), nsi.indexOf("Function un.onInit"));
    expect(success).toMatch(/\$\{If\} \$ForceRun == 1\s+\$\{OrIfNot\} \$\{Silent\}/);
    expect(success).toContain("Exec '\"$INSTDIR\\${EXE_NAME}\" --updated'");
  });

  it("refuses to uninstall unless InstallLocation is this identity's folder and the folder it runs for, before anything is removed", () => {
    const onInit = nsi.slice(nsi.indexOf("Function un.onInit"), nsi.indexOf('Section "Uninstall"'));
    expect(onInit).toContain('ReadRegStr $R1 HKCU "${INSTALL_KEY}" "InstallLocation"');
    expect(onInit).toContain("Call un.FolderAllowed");
    expect(onInit).toContain("Call un.SamePath");
    expect(onInit).toMatch(/SetErrorLevel \$\{EXIT_REFUSED\}\s+Quit/);
    expect(onInit).not.toMatch(/RMDir|Delete/);
    // Deletion has exactly one target, the folder the uninstaller runs for: never a registry value, never a wildcard.
    expect(nsi.match(/\bRMDir\b/g)).toHaveLength(1);
    expect(nsi).toContain('RMDir /r "$INSTDIR"');
    expect(nsi.match(/^\s*Delete\s/gm)).toHaveLength(2); // the two shortcuts
    // The programs policy is exact: the folder is %LOCALAPPDATA%\Programs\<folder>, not "somewhere under it".
    expect(nsi).toContain('StrCpy $R1 "$LOCALAPPDATA\\Programs\\${FOLDER_NAME}"');
  });

  it("never touches a data root, never elevates, and never stops a process by name", () => {
    expect(nsi).not.toMatch(/\$APPDATA|\$PROFILE/i);
    for (const use of nsi.match(/\$LOCALAPPDATA[^\s"']*/g) ?? []) expect(use).toMatch(/^\$LOCALAPPDATA\\Programs\\\$\{FOLDER_NAME\}$/);
    expect(nsi).toContain("RequestExecutionLevel user");
    expect(nsi).not.toMatch(/taskkill|tasklist|nsProcess|FindProcDLL|KillProc|wmic|\/IM\b|Stop-Process|ExecShellAsUser/i);
    expect(nsi).not.toMatch(/HKLM|HKEY_LOCAL_MACHINE|HKCR|SHCTX/i);
  });

  it("carries the app id in the installer's version resource, where the Rust updater reads it", () => {
    expect(nsi).toContain('VIAddVersionKey /LANG=1033 "AppId" "${APP_ID}"');
  });

  it("is given by the build script exactly the defines it requires", () => {
    const required = [...nsi.matchAll(/^!insertmacro RequireDefine (\w+)/gm)].map((match) => match[1] ?? "").sort();
    const build = codeOf(join(installer, "build-installer.ps1"));
    const given = [...build.matchAll(/"\/D(\w+)=/g)].map((match) => match[1] ?? "").filter((name) => name !== "IDENTITY").sort();
    expect(given).toEqual(required);
    for (const file of nsi.match(/\$\{__FILEDIR__\}\\[\w.-]+/g) ?? []) expect(existsSync(join(installer, file.split("\\")[1] ?? "")), file).toBe(true);
  });
});

// Compiling only writes an installer into a temp folder; nothing it wrote is run. Skipped where there is no NSIS (the release
// and acceptance workflows install it on their runner and build there).
const compiler = ((): string | null => {
  const candidates = [process.env.MAKENSIS, `${process.env["ProgramFiles(x86)"]}\\NSIS\\makensis.exe`, `${process.env.ProgramFiles}\\NSIS\\makensis.exe`].filter((path): path is string => Boolean(path));
  const cache = join(process.env.LOCALAPPDATA ?? "", "electron-builder", "Cache", "nsis");
  if (existsSync(cache)) for (const folder of readdirSync(cache)) candidates.push(join(cache, folder, "Bin", "makensis.exe"));
  return candidates.find((path) => existsSync(path)) ?? null;
})();

describe.skipIf(!windows)("the build script", () => {
  const build = join(installer, "build-installer.ps1");
  const run = (args: string[]) => powershell(build, args, { ...process.env, MAKENSIS: compiler ?? "" });

  it("refuses a version that is not x.y.z, a missing payload, and a payload without the identity's exe, before it compiles", () => {
    const payload = temp();
    writeFileSync(join(payload, "Tjommie.exe"), "stub");
    const out = join(temp(), "x.exe");
    const base = ["-Identity", "production", "-Version", "0.6.0", "-PayloadDir", payload, "-OutFile", out];
    const replace = (flag: string, value: string): string[] => base.map((arg, index) => (base[index - 1] === flag ? value : arg));
    const cases: Array<[string[], RegExp]> = [
      [replace("-Version", "0.6"), /x\.y\.z/], [replace("-Version", "0.6.0-rc1"), /x\.y\.z/],
      [replace("-PayloadDir", join(payload, "nope")), /does not exist/], [replace("-Identity", "dev"), /TjommieRS\.exe/],
    ];
    for (const [args, message] of cases) {
      const result = run(args);
      expect(result.status, args.join(" ")).not.toBe(0);
      expect(`${result.stdout}${result.stderr}`).toMatch(message);
    }
    expect(existsSync(out)).toBe(false);
  });

  it.skipIf(!compiler)("compiles an installer for each identity, with its own app id in the version resource and no warnings", () => {
    const payload = temp();
    const outputs = temp();
    for (const name of ["Tjommie.exe", "TjommieRS.exe", "Tjommie Update Test.exe"]) writeFileSync(join(payload, name), "stub");
    mkdirSync(join(payload, "resources", "daemon"), { recursive: true });
    writeFileSync(join(payload, "resources", "daemon", "tjommie-daemon.exe"), "stub");
    const wide = (text: string): Buffer => Buffer.from(text, "utf16le");
    for (const [name, row] of Object.entries(table.identities)) {
      const out = join(outputs, `${name}.exe`);
      const result = run(["-Identity", name, "-Version", "0.6.0", "-PayloadDir", payload, "-OutFile", out]);
      expect(result.status, `${name}: ${result.stdout}${result.stderr}`).toBe(0);
      expect(result.stdout).not.toMatch(/warning/i);
      const bytes = readFileSync(out);
      expect(bytes.length, name).toBeGreaterThan(10_000);
      // The version resource is the one part of the installer that is not compressed, and UTF-16 text: the key "AppId" and this
      // identity's app id. (The script's own strings, the GUID among them, are compressed.)
      expect(bytes.includes(wide("AppId")), `${name} AppId key`).toBe(true);
      expect(bytes.includes(wide(row.appId)), `${name} app id`).toBe(true);
    }
  });
});
