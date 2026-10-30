import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { lstatSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { readRegistryKeys, type RegistrySnapshot } from "../scripts/update-test-guards.ts";

// Electron rewrites these six cache folders' data_1 shards while the owner's app is open.
const ownerCacheFolders = new Set(["Cache/Cache_Data", "DawnGraphiteCache", "DawnWebGPUCache", "GPUCache", "GrShaderCache", "ShaderCache"]);
const ownerCacheFiles = new Set([...ownerCacheFolders].map((folder) => `${folder}/data_1`));

export type OwnerSnapshot = { strict: Map<string, string>; live: Map<string, string>; running: boolean; registry: RegistrySnapshot[] };

const ownerGuid = "89b4e4b4-ac49-585f-800a-537f559f4641";
const ownerRegistryKeys = ([32, 64] as const).flatMap((view) =>
  [`Software\\${ownerGuid}`, `Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${ownerGuid}`]
    .map((key) => ({ hive: "HKCU" as const, view, key })));

function ownerAppRunning(local: string): boolean {
  const exe = join(local, "Programs", "Tjommie", "Tjommie.exe").replaceAll("'", "''");
  const command = `$ErrorActionPreference='Stop'; $processes=@(Get-CimInstance Win32_Process -Filter "Name='Tjommie.exe'" -ErrorAction Stop); if (@($processes | Where-Object { !$_.ExecutablePath }).Count) { throw 'Cannot identify a Tjommie process' }; @($processes | Where-Object { $_.ExecutablePath -eq '${exe}' }).Count -gt 0`;
  try {
    const result = execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", command], { encoding: "utf8", windowsHide: true }).trim();
    if (result !== "True" && result !== "False") throw new Error("Inconclusive process query.");
    return result === "True";
  } catch (cause) { throw new Error("Owner app may be running; its process state could not be confirmed.", { cause }); }
}

// Metadata only: never read owner file contents or follow symlinks.
export function ownerLastWriteSnapshot(
  local = process.env.LOCALAPPDATA, roaming = process.env.APPDATA, running?: boolean,
  registry = readRegistryKeys(ownerRegistryKeys),
): OwnerSnapshot {
  if (!local || !roaming) throw new Error("LOCALAPPDATA and APPDATA are required for the owner-data safety check.");
  const profile = join(roaming, "Tjommie");
  const data = join(local, "Tjommie");
  const strict = new Map<string, string>();
  const live = new Map<string, string>();
  const visit = (path: string, root: string, liveData: boolean): void => {
    let entry: ReturnType<typeof lstatSync>;
    try { entry = lstatSync(path, { bigint: true }); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      if (path === root) { strict.set(path, "missing"); live.set(path, "missing"); }
      return; // a child may vanish between readdir and stat
    }
    const name = liveData ? relative(root, path).replaceAll("\\", "/") : "";
    const value = String(entry.mtimeNs);
    strict.set(path, value);
    const accounts = root === data && (name === "accounts" || name.startsWith("accounts/"));
    const ignoredFile = liveData && entry.isFile() &&
      (root === data && (["data/tjommie.db-wal", "data/tjommie.db-shm"].includes(name) || name.startsWith("logs/")) ||
        root === profile && ownerCacheFiles.has(name));
    const ignoredDirectoryTime = liveData && entry.isDirectory() &&
      (root === data && ["data", "logs"].includes(name) || root === profile && ownerCacheFolders.has(name));
    if (!accounts && !ignoredFile) live.set(path, ignoredDirectoryTime ? "directory" : value);
    if (entry.isDirectory()) {
      let children: string[];
      try { children = readdirSync(path).sort(); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return; throw error; }
      for (const child of children) visit(join(path, child), root, liveData);
    }
  };
  for (const [path, liveData] of [
    [data, true],
    [join(local, "Programs", "Tjommie"), false],
    [join(local, "tjommie-updater"), false],
    [profile, true],
  ] as const) visit(path, path, liveData);
  for (const entry of registry) {
    const path = `${entry.hive} (${entry.view})\\${entry.key}`;
    const value = JSON.stringify([entry.present, entry.values.map(({ name, type, value }) => [name, type, value])]);
    strict.set(path, value);
    live.set(path, value);
  }
  return { strict, live, running: running ?? ownerAppRunning(local), registry };
}

export function assertOwnerUnchanged(before: OwnerSnapshot, after: OwnerSnapshot, evidenceDir = tmpdir()): string {
  const changed = ownerChangedPaths(before, after);
  const state = `owner running before=${before.running}, after=${after.running}; ${after.registry.length} registry key/view snapshots checked`;
  if (!changed.length) return `Owner-folder safety net: PASS (${state}; no watched owner path or registry key changed).`;
  const evidence = join(evidenceDir, `tjommie-owner-safety-${randomUUID()}.json`);
  writeFileSync(evidence, JSON.stringify({ changed, registryBefore: before.registry, registryAfter: after.registry }, null, 2));
  throw new Error(`Owner-folder safety net: FAIL (${changed.length} watched paths changed; ${state}). Complete evidence: ${evidence}\n${changed.join("\n")}`);
}

export function ownerChangedPaths(before: OwnerSnapshot, after: OwnerSnapshot): string[] {
  const left = before.running && after.running ? before.live : before.strict;
  const right = before.running && after.running ? after.live : after.strict;
  return [...new Set([...left.keys(), ...right.keys()])].filter((path) => left.get(path) !== right.get(path));
}
