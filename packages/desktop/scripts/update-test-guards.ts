import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve, win32 } from "node:path";
import { inflateRawSync } from "node:zlib";
import { pathToFileURL } from "node:url";
import { parseUpdateInfo } from "electron-updater/out/providers/Provider.js";

export const testIdentity = {
  appId: "com.tsnyders.tjommie.updatetest", guid: "5891e549-73c7-5ca5-865f-6a0f99687a40",
  product: "Tjommie Update Test", packageName: "tjommie-updatetest",
} as const;
const installKey = `Software\\${testIdentity.guid}`;
const uninstallKey = `Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${testIdentity.guid}`;
const guardMarker = "Tjommie update-test target guard";
const expectedDefines = {
  APP_ID: testIdentity.appId, APP_GUID: testIdentity.guid, UNINSTALL_APP_KEY: testIdentity.guid,
  APP_FILENAME: testIdentity.packageName, APP_PACKAGE_NAME: testIdentity.packageName,
  APP_INSTALLER_STORE_FILE: `${testIdentity.packageName}-updater\\installer.exe`,
  PRODUCT_NAME: testIdentity.product, PRODUCT_FILENAME: testIdentity.product,
  APP_PRODUCT_FILENAME: testIdentity.product, SHORTCUT_NAME: testIdentity.product,
};

// NsisTarget invokes this once with the computed defines, before copying them for both compiler stages.
export function assertTestDefines(options: unknown): void {
  if (!Array.isArray(options) || !options[0] || typeof options[0] !== "object") throw new Error("Missing NSIS defines.");
  const defines = options[0] as Record<string, unknown>;
  for (const [key, expected] of Object.entries(expectedDefines)) {
    if (defines[key] !== expected) throw new Error(`Unsafe update-test NSIS ${key}: expected ${expected}.`);
  }
  if (Object.keys(defines).some((key) => /^(?:UNINSTALL|INSTALL)_REGISTRY_KEY/.test(key)) ||
    defines.APP_EXECUTABLE_FILENAME !== undefined && defines.APP_EXECUTABLE_FILENAME !== `${testIdentity.product}.exe`) {
    throw new Error("Unsafe update-test NSIS registry or executable override.");
  }
}

export function assertTestInstallDir(installDir: string): void {
  const root = dirname(resolve(installDir));
  if (basename(installDir) !== testIdentity.product || dirname(root) !== resolve(tmpdir()) ||
    !existsSync(root) || realpathSync(root).toLowerCase() !== root.toLowerCase() ||
    existsSync(installDir) && realpathSync(installDir).toLowerCase() !== resolve(installDir).toLowerCase() || /[$"\r\n]/.test(installDir)) {
    throw new Error("Unsafe update-test install directory; expected an unlinked disposable Temp fixture.");
  }
}

export type RegistryKey = { hive: "HKCU" | "HKLM"; view: 32 | 64; key: string };
export type RegistrySnapshot = RegistryKey & { present: boolean; values: { name: string; type: string; value: unknown }[] };

export function testRegistryKeys(): RegistryKey[] {
  return (["HKCU", "HKLM"] as const).flatMap((hive) => ([32, 64] as const).flatMap((view) =>
    [installKey, uninstallKey].map((key) => ({ hive, view, key }))));
}

// OpenSubKey is read-only; access errors are fatal, never mistaken for an absent registration.
export function readRegistryKeys(keys: RegistryKey[]): RegistrySnapshot[] {
  if (!keys.length) return [];
  const encoded = Buffer.from(JSON.stringify(keys)).toString("base64");
  const script = `$ErrorActionPreference='Stop'; $requests=([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${encoded}')) | ConvertFrom-Json)
$rows=@(foreach ($request in $requests) {
  $hive=if ($request.hive -eq 'HKCU') { [Microsoft.Win32.RegistryHive]::CurrentUser } else { [Microsoft.Win32.RegistryHive]::LocalMachine }
  $view=if ($request.view -eq 64) { [Microsoft.Win32.RegistryView]::Registry64 } else { [Microsoft.Win32.RegistryView]::Registry32 }
  $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey($hive,$view); $key=$null
  try {
    $key=$base.OpenSubKey($request.key,$false)
    $values=@(if ($null -ne $key) { foreach ($name in ($key.GetValueNames() | Sort-Object)) {
      @{name=$name;type=$key.GetValueKind($name).ToString();value=$key.GetValue($name,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)}
    } })
    @{hive=$request.hive;view=$request.view;key=$request.key;present=($null -ne $key);values=$values}
  } finally { if ($null -ne $key) { $key.Dispose() }; $base.Dispose() }
}); ConvertTo-Json -InputObject $rows -Depth 8 -Compress`;
  return JSON.parse(execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8", windowsHide: true, maxBuffer: 4 * 1024 * 1024,
  })) as RegistrySnapshot[];
}

function canonical(path: string): string {
  if (!/^[a-z]:[\\/]/i.test(path)) throw new Error("Expected an absolute Windows path.");
  return win32.resolve(path).toLowerCase();
}

export function assertTestRegistration(entries: RegistrySnapshot[], installDir: string, required: boolean): void {
  assertTestInstallDir(installDir);
  let registered = false;
  if (!entries.length || entries.length % 2) throw new Error("Missing test registry snapshot.");
  const stringValue = (entry: RegistrySnapshot, name: string): string => {
    const value = entry.values.find((item) => item.name === name);
    if (value?.type !== "String" || typeof value.value !== "string" || !value.value) throw new Error(`Unsafe test ${name} in ${entry.hive} (${entry.view}).`);
    return value.value;
  };
  for (let i = 0; i < entries.length; i += 2) {
    const location = entries[i], uninstall = entries[i + 1];
    if (location.hive !== uninstall.hive || location.view !== uninstall.view) throw new Error("Mismatched registry snapshot.");
    if (!location.present && !uninstall.present) continue;
    if (!location.present || !uninstall.present) throw new Error("Missing paired test registration.");
    if (canonical(stringValue(location, "InstallLocation")) !== canonical(installDir)) throw new Error("Unsafe test InstallLocation.");
    const command = stringValue(uninstall, "UninstallString");
    const match = /^"([^"\r\n]+)" \/(currentuser|allusers)$/i.exec(command);
    const mode = location.hive === "HKCU" ? "currentuser" : "allusers";
    if (!match || match[2].toLowerCase() !== mode || canonical(match[1]) !== canonical(win32.join(installDir, `Uninstall ${testIdentity.product}.exe`))) {
      throw new Error("Unsafe test UninstallString.");
    }
    if (location.hive === "HKCU" && location.view === 64) registered = true;
  }
  if (required && !registered) throw new Error("Missing test registration in HKCU (64).");
}

export function checkTestRegistration(installDir: string, required: boolean): void {
  assertTestRegistration(readRegistryKeys(testRegistryKeys()), installDir, required);
}

// NSIS requires the final /D= argument unquoted, even with spaces; the executable still needs quotes.
export function nsisSpawnOptions(file: string): { windowsVerbatimArguments: true; argv0: string } {
  return { windowsVerbatimArguments: true, argv0: `"${file}"` };
}

// Test-only runtime checks run after initMultiUser, before installSection/uninstallSection can remove files.
export function testNsisInclude(installDir: string): string {
  assertTestInstallDir(installDir);
  const uninstaller = win32.join(installDir, `Uninstall ${testIdentity.product}.exe`);
  return `
!macro testAbort
  DetailPrint "${guardMarker}"
  SetErrorLevel 91
  Quit
!macroend
!macro testTarget
  GetFullPathName $R0 "$INSTDIR"
  GetFullPathName $R1 "${installDir}"
  \${If} $R0 == ""
  \${OrIf} $R1 == ""
  \${OrIf} $R0 != $R1
  \${OrIfNot} \${FileExists} "$R1\\*.*"
    !insertmacro testAbort
  \${EndIf}
!macroend
!macro testRegistration HIVE MODE
  ReadRegStr $R1 \${HIVE} "${installKey}" "InstallLocation"
  ReadRegStr $R2 \${HIVE} "${uninstallKey}" "UninstallString"
  \${If} $R1 != ""
  \${OrIf} $R2 != ""
    GetFullPathName $R3 "$R1"
    \${If} $R1 == ""
    \${OrIf} $R3 != "${installDir}"
    \${OrIf} $R2 != '"${uninstaller}" /\${MODE}'
      !insertmacro testAbort
    \${EndIf}
  \${EndIf}
!macroend
!macro testRegistrations
  SetRegView 32
  !insertmacro testRegistration HKCU currentuser
  !insertmacro testRegistration HKLM allusers
  SetRegView 64
  !insertmacro testRegistration HKCU currentuser
  !insertmacro testRegistration HKLM allusers
!macroend
!macro testRequireRegistration
  ReadRegStr $R1 HKCU "${installKey}" "InstallLocation"
  \${If} $R1 == ""
    !insertmacro testAbort
  \${EndIf}
!macroend
!macro customInit
  !insertmacro testTarget
  !insertmacro testRegistrations
  \${If} \${isUpdated}
    !insertmacro testRequireRegistration
  \${EndIf}
!macroend
!macro customUnInit
  !insertmacro testTarget
  !insertmacro testRegistrations
  !insertmacro testRequireRegistration
!macroend
`;
}

const signature = Buffer.from("efbeadde4e756c6c736f6674496e7374", "hex");

function block(bytes: Buffer, offset: number): { data: Buffer; next: number } {
  const size = bytes.readUInt32LE(offset), length = size & 0x7fffffff;
  if (!length || offset + 4 + length > bytes.length) throw new Error("Invalid NSIS block.");
  const raw = bytes.subarray(offset + 4, offset + 4 + length);
  return { data: size >>> 31 ? inflateRawSync(raw, { maxOutputLength: 16 * 1024 * 1024 }) : raw, next: offset + 4 + length };
}

function header(bytes: Buffer): { strings: string[]; next: number } {
  const start = bytes.indexOf(signature) - 4;
  if (start < 0) throw new Error("Missing NSIS header.");
  const decoded = block(bytes, start + 28);
  if (decoded.data.length !== bytes.readUInt32LE(start + 20)) throw new Error("Invalid NSIS header size.");
  const from = decoded.data.readUInt32LE(28), to = decoded.data.readUInt32LE(36);
  if (from < 300 || from >= to || to > decoded.data.length || (to - from) % 2) throw new Error("Unsupported NSIS string table.");
  return { strings: decoded.data.subarray(from, to).toString("utf16le").split("\0"), next: decoded.next };
}

function assertHeader(strings: string[], installDir: string, installer: boolean): void {
  const required = [installKey, uninstallKey, testIdentity.appId, testIdentity.product, installDir, guardMarker];
  // NSIS interns null-terminated suffixes: a referenced string need not start after a null.
  for (const value of required) if (!strings.some((stored) => stored.endsWith(value))) throw new Error(`Missing compiled test identity: ${value}`);
  for (const suffix of [`\\${testIdentity.packageName}`, `${testIdentity.product}.exe`, `Uninstall ${testIdentity.product}.exe`,
    ...(installer ? [`\\${testIdentity.packageName}-updater\\installer.exe`] : [])]) {
    if (!strings.some((value) => value === suffix || value.endsWith(suffix.startsWith("\\") ? suffix : `\\${suffix}`))) throw new Error(`Missing compiled test identity: ${suffix}`);
  }
  if (strings.some((value) => value.toLowerCase().includes("89b4e4b4-ac49-585f-800a-537f559f4641") ||
    value.toLowerCase().endsWith("com.tsnyders.tjommie") || /(?:^|[\\/])(?:tjommie|tjommie\.exe|uninstall tjommie\.exe|tjommie-updater[\\/]installer\.exe)$/i.test(value))) {
    throw new Error("Production identity found in test NSIS header.");
  }
}

// shortcut: pinned NSIS 3 Unicode/non-solid zlib layout; fail closed if the toolchain changes it.
export function assertTestInstaller(file: string, installDir: string): void {
  assertTestInstallDir(installDir);
  const bytes = readFileSync(file), installer = header(bytes);
  assertHeader(installer.strings, installDir, true);
  let uninstallers = 0;
  for (let offset = installer.next; offset + 4 < bytes.length;) {
    const size = bytes.readUInt32LE(offset) & 0x7fffffff;
    if (!size || offset + 4 + size > bytes.length) break; // NSIS CRC/signature trailer
    // The application archive cannot be an uninstaller; avoid inflating hundreds of MB.
    if (size < 2 * 1024 * 1024) {
      const data = block(bytes, offset).data;
      if (data.includes(signature)) { assertHeader(header(data).strings, installDir, false); uninstallers++; }
    }
    offset += 4 + size;
  }
  if (uninstallers !== 1) throw new Error("Expected exactly one verified embedded test uninstaller.");
}

export function assertTestUninstaller(file: string, installDir: string): void {
  assertTestInstallDir(installDir);
  if (basename(file) !== `Uninstall ${testIdentity.product}.exe` || canonical(dirname(file)) !== canonical(installDir)) throw new Error("Unsafe test uninstaller path.");
  assertHeader(header(readFileSync(file)).strings, installDir, false);
}

export function assertTestUpdate(manifestFile: string, installer: string, installDir: string, version: string): string {
  const name = `Tjommie-Setup-${version}.exe`;
  const info = parseUpdateInfo(readFileSync(manifestFile, "utf8"), "latest.yml", pathToFileURL(manifestFile));
  if (!info || typeof info !== "object" || info.version !== version || info.path !== name || basename(installer) !== name ||
    "packages" in info || !Array.isArray(info.files) || info.files.length !== 1 || info.files[0]?.url !== name) {
    throw new Error("Unsafe update-test manifest target.");
  }
  const bytes = readFileSync(installer), sha512 = createHash("sha512").update(bytes).digest("base64");
  if (info.sha512 !== sha512 || info.files[0].sha512 !== sha512 || info.files[0].size !== bytes.length) {
    throw new Error("Update-test manifest sha512 or size mismatch.");
  }
  assertTestInstaller(installer, installDir);
  return sha512;
}

export function assertTestDownloadedUpdate(manifestFile: string, pendingDir: string, installDir: string, version: string): void {
  if (realpathSync(pendingDir).toLowerCase() !== resolve(pendingDir).toLowerCase()) throw new Error("Linked update-test download directory.");
  const name = `Tjommie-Setup-${version}.exe`;
  const sha512 = assertTestUpdate(manifestFile, join(pendingDir, name), installDir, version);
  const cached: unknown = JSON.parse(readFileSync(join(pendingDir, "update-info.json"), "utf8"));
  if (!cached || typeof cached !== "object" || !("fileName" in cached) || cached.fileName !== name ||
    !("sha512" in cached) || cached.sha512 !== sha512 || "isAdminRightsRequired" in cached && cached.isAdminRightsRequired !== false) {
    throw new Error("Unsafe update-test downloaded installer metadata.");
  }
}
